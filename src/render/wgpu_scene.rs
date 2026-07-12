use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::mpsc::sync_channel;

use anyhow::{anyhow, Context, Result};
use bytemuck::{Pod, Zeroable};
use image::RgbaImage;

use crate::engine::effect::HandwrittenEffectFallback;
use crate::engine::effect_diagnostics::collect_effect_diagnostics;
use crate::engine::graph::ImageNode;
use crate::engine::render_graph::{
    DiagnosticSeverity, RenderPassKind, RenderPassNode, RenderTargetDesc, RenderTargetRef,
    SceneRenderGraph, TextureSource,
};
use crate::engine::shader::{textured_quad_wgsl, ShaderTranslator};
use crate::engine::{FrameContext, SceneGraph};
use crate::platform::GpuDevice;

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct WgpuSceneRenderResult {
    pub image: RgbaImage,
    pub graph: SceneRenderGraph,
    pub diagnostics: Vec<String>,
}

pub fn render_scene_graph_to_rgba(dir: &Path) -> Result<WgpuSceneRenderResult> {
    let graph = SceneGraph::from_directory(dir)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    let context = FrameContext::for_graph(&graph, 0.0, 1.0 / 60.0, 0);
    let gpu = crate::platform::open_device()?;
    let mut renderer = WgpuSceneRenderer::new(gpu)?;
    let image = renderer.render_graph(&graph, &render_graph, &context)?;

    Ok(WgpuSceneRenderResult {
        image,
        graph: render_graph,
        diagnostics: renderer.diagnostics,
    })
}

#[allow(dead_code)]
pub fn render_first_image_layer_to_rgba(dir: &Path) -> Result<WgpuSceneRenderResult> {
    render_scene_graph_to_rgba(dir)
}

pub struct WgpuSceneRenderer {
    gpu: GpuDevice,
    pipelines: BuiltinPipelines,
    frame_bind_group_layout: wgpu::BindGroupLayout,
    texture_bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    diagnostics: Vec<String>,
}

impl WgpuSceneRenderer {
    pub fn new(gpu: GpuDevice) -> Result<Self> {
        crate::engine::shader::validate_builtin_textured_quad()?;
        let device = &gpu.device;

        let frame_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("wp-engine frame bind group layout"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });

        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("wp-engine texture bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wp-engine builtin textured quad"),
            source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(textured_quad_wgsl())),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wp-engine scene pipeline layout"),
            bind_group_layouts: &[&frame_bind_group_layout, &texture_bind_group_layout],
            push_constant_ranges: &[],
        });

        let pipelines = BuiltinPipelines::new(device, &pipeline_layout, &shader);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wp-engine linear clamp sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Ok(Self {
            gpu,
            pipelines,
            frame_bind_group_layout,
            texture_bind_group_layout,
            sampler,
            diagnostics: Vec::new(),
        })
    }

    pub fn render_graph(
        &mut self,
        graph: &SceneGraph,
        render_graph: &SceneRenderGraph,
        context: &FrameContext,
    ) -> Result<RgbaImage> {
        self.diagnostics.clear();
        self.push_graph_diagnostics(render_graph);
        self.push_effect_diagnostics(graph);

        let plan = render_graph.execution_plan();
        self.diagnostics.push(format!(
            "execution plan: {} ordered passes, {} graph targets",
            plan.pass_indices.len(),
            render_graph.targets.len()
        ));

        if render_graph.objects.is_empty() {
            return Err(anyhow!("render graph has no visible image objects"));
        }

        let targets = GpuRenderTargets::new(&self.gpu.device, render_graph, &mut self.diagnostics);
        let mut uploaded_textures = UploadedTextureCache::default();
        let mut cleared_targets = BTreeSet::new();

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wp-engine graph render encoder"),
            });

        for pass_index in plan.pass_indices {
            let Some(pass) = render_graph.passes.get(pass_index) else {
                continue;
            };

            match pass.kind {
                RenderPassKind::Material => self.execute_material_pass(
                    &mut encoder,
                    graph,
                    render_graph,
                    context,
                    pass,
                    &targets,
                    &mut uploaded_textures,
                    &mut cleared_targets,
                )?,
                RenderPassKind::Copy => self.execute_copy_pass(
                    &mut encoder,
                    pass,
                    &targets,
                    &mut cleared_targets,
                    "copy",
                ),
                RenderPassKind::Effect => self.execute_effect_pass(
                    &mut encoder,
                    render_graph,
                    context,
                    pass,
                    &targets,
                    &mut cleared_targets,
                ),
                RenderPassKind::Swap => {
                    self.diagnostics.push(format!(
                        "unsupported pass '{}': render target swap is not implemented yet",
                        pass.label
                    ));
                }
            }
        }

        if !cleared_targets.contains(target_key(&RenderTargetRef::Backbuffer).as_str()) {
            clear_target(
                &mut encoder,
                &targets.backbuffer.view,
                "wp-engine clear empty backbuffer",
            );
        }

        self.execute_final_composite(&mut encoder, render_graph, &targets, &mut cleared_targets);

        read_texture_to_rgba(
            &self.gpu,
            &targets.output.texture,
            encoder,
            targets.output.size[0],
            targets.output.size[1],
        )
    }

    fn execute_material_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        graph: &SceneGraph,
        render_graph: &SceneRenderGraph,
        context: &FrameContext,
        pass: &RenderPassNode,
        targets: &GpuRenderTargets,
        uploaded_textures: &mut UploadedTextureCache,
        cleared_targets: &mut BTreeSet<String>,
    ) -> Result<()> {
        let Some(object_id) = pass.object_id else {
            self.diagnostics.push(format!(
                "skipping material pass '{}': pass is not attached to an object",
                pass.label
            ));
            return Ok(());
        };

        let Some(object) = render_graph.objects.get(object_id) else {
            self.diagnostics.push(format!(
                "skipping material pass '{}': object {} is missing",
                pass.label, object_id
            ));
            return Ok(());
        };

        let Some(node) = graph
            .images
            .iter()
            .find(|node| node.object_index == object.scene_object_index)
        else {
            self.diagnostics.push(format!(
                "skipping material pass '{}': scene object {} no longer maps to an image node",
                pass.label, object.scene_object_index
            ));
            return Ok(());
        };

        if node.is_render_target_layer() {
            self.diagnostics.push(format!(
                "skipping material pass '{}': '{}' is a render-target/special layer",
                pass.label,
                display_object_name(object.name.as_str())
            ));
            return Ok(());
        }

        self.probe_material_shader(graph, node, pass);

        let Some(selected_texture) = select_material_texture(pass, object.base_texture.as_deref())
        else {
            self.diagnostics.push(format!(
                "skipping material pass '{}': no material/base texture is resolved",
                pass.label
            ));
            return Ok(());
        };

        let (source_view, source_size) = match selected_texture.source {
            TextureSource::MaterialTexture | TextureSource::UserTexture => {
                let texture = uploaded_textures.get_or_upload(
                    &self.gpu.device,
                    &self.gpu.queue,
                    &graph.assets,
                    selected_texture.name,
                    &mut self.diagnostics,
                )?;
                (&texture.view, texture.size)
            }
            TextureSource::RenderTarget => {
                let source_ref = RenderTargetRef::Named(selected_texture.name.to_string());
                let Some(texture) = targets.texture(&source_ref) else {
                    self.diagnostics.push(format!(
                        "skipping material pass '{}': render target texture '{}' is not allocated",
                        pass.label, selected_texture.name
                    ));
                    return Ok(());
                };
                (&texture.view, texture.size)
            }
            TextureSource::EngineRuntime => {
                self.diagnostics.push(format!(
                    "skipping material pass '{}': runtime texture '{}' is not implemented",
                    pass.label, selected_texture.name
                ));
                return Ok(());
            }
        };

        if pass.textures.len() > 1 {
            self.diagnostics.push(format!(
                "pass '{}': {} texture bindings resolved; builtin material fallback samples only slot 0",
                pass.label,
                pass.textures.len()
            ));
        }

        let vertices = image_layer_vertices(graph, node, context, source_size);
        if let Some(model) = &node.model {
            if model.puppet.is_some() {
                self.diagnostics.push(format!(
                    "pass '{}': puppet/binary mesh geometry is pending; generated model quad is used",
                    pass.label
                ));
            }
        }

        let Some(target) = targets.texture(&pass.target) else {
            self.diagnostics.push(format!(
                "skipping material pass '{}': target {} is not allocated",
                pass.label,
                describe_target(&pass.target)
            ));
            return Ok(());
        };

        let frame_bind_group = self.create_frame_bind_group(context, node.object_index);
        let texture_bind_group = self.create_texture_bind_group(source_view);
        let vertex_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine image layer quad vertices",
            wgpu::BufferUsages::VERTEX,
            &vertices,
        );
        if let Some(blend) = unsupported_blend_mode(pass.blending.as_deref()) {
            self.diagnostics.push(format!(
                "pass '{}': unsupported blend mode '{blend}', using alpha blending",
                pass.label
            ));
        }
        let pipeline = self.pipeline_for_blend(pass.blending.as_deref(), object.blend_mode);
        let clear = mark_target_for_load(cleared_targets, &pass.target);

        self.draw_textured_quad(
            encoder,
            &target.view,
            pipeline,
            &frame_bind_group,
            &texture_bind_group,
            &vertex_buffer,
            vertices.len() as u32,
            clear,
            &format!("wp-engine material pass {}", pass.label),
        );

        Ok(())
    }

    fn execute_copy_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        pass: &RenderPassNode,
        targets: &GpuRenderTargets,
        cleared_targets: &mut BTreeSet<String>,
        reason: &str,
    ) {
        let source_ref = pass_source_ref(pass);
        if target_key(&source_ref) == target_key(&pass.target) {
            self.diagnostics.push(format!(
                "skipping {reason} pass '{}': source and target are both {}",
                pass.label,
                describe_target(&pass.target)
            ));
            return;
        }

        let Some(source) = targets.texture(&source_ref) else {
            self.diagnostics.push(format!(
                "skipping {reason} pass '{}': source {} is not allocated",
                pass.label,
                describe_target(&source_ref)
            ));
            return;
        };
        let Some(target) = targets.texture(&pass.target) else {
            self.diagnostics.push(format!(
                "skipping {reason} pass '{}': target {} is not allocated",
                pass.label,
                describe_target(&pass.target)
            ));
            return;
        };

        let frame_bind_group = self.create_default_frame_bind_group(source.size);
        let texture_bind_group = self.create_texture_bind_group(&source.view);
        let vertices = fullscreen_quad_vertices();
        let vertex_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine fullscreen copy vertices",
            wgpu::BufferUsages::VERTEX,
            &vertices,
        );
        let clear = mark_target_for_load(cleared_targets, &pass.target);

        self.draw_textured_quad(
            encoder,
            &target.view,
            &self.pipelines.replace,
            &frame_bind_group,
            &texture_bind_group,
            &vertex_buffer,
            vertices.len() as u32,
            clear,
            &format!("wp-engine {reason} pass {}", pass.label),
        );
    }

    fn execute_effect_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        render_graph: &SceneRenderGraph,
        context: &FrameContext,
        pass: &RenderPassNode,
        targets: &GpuRenderTargets,
        cleared_targets: &mut BTreeSet<String>,
    ) {
        if let Some(fallback) = HandwrittenEffectFallback::from_label(&pass.label) {
            self.diagnostics.push(format!(
                "effect pass '{}': using handwritten WGSL fallback '{}'",
                pass.label,
                fallback.name()
            ));
            self.execute_effect_fallback_pass(
                encoder,
                render_graph,
                context,
                pass,
                targets,
                cleared_targets,
                fallback,
            );
            return;
        }

        self.diagnostics.push(format!(
            "effect pass '{}': effect shader execution is pending; copying selected source to target",
            pass.label
        ));
        self.execute_copy_pass(encoder, pass, targets, cleared_targets, "effect fallback");
    }

    fn execute_effect_fallback_pass(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        render_graph: &SceneRenderGraph,
        context: &FrameContext,
        pass: &RenderPassNode,
        targets: &GpuRenderTargets,
        cleared_targets: &mut BTreeSet<String>,
        fallback: HandwrittenEffectFallback,
    ) {
        let source_ref = pass_source_ref(pass);
        if target_key(&source_ref) == target_key(&pass.target) {
            self.diagnostics.push(format!(
                "skipping effect fallback pass '{}': source and target are both {}",
                pass.label,
                describe_target(&pass.target)
            ));
            return;
        }

        let Some(source) = targets.texture(&source_ref) else {
            self.diagnostics.push(format!(
                "skipping effect fallback pass '{}': source {} is not allocated",
                pass.label,
                describe_target(&source_ref)
            ));
            return;
        };
        let Some(target) = targets.texture(&pass.target) else {
            self.diagnostics.push(format!(
                "skipping effect fallback pass '{}': target {} is not allocated",
                pass.label,
                describe_target(&pass.target)
            ));
            return;
        };

        let frame_bind_group = pass
            .object_id
            .and_then(|object_id| render_graph.objects.get(object_id))
            .map(|object| self.create_frame_bind_group(context, object.scene_object_index))
            .unwrap_or_else(|| self.create_default_frame_bind_group(source.size));
        let texture_bind_group = self.create_texture_bind_group(&source.view);
        let vertices = fullscreen_quad_vertices();
        let vertex_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine effect fallback vertices",
            wgpu::BufferUsages::VERTEX,
            &vertices,
        );
        let clear = mark_target_for_load(cleared_targets, &pass.target);
        let pipeline = self.pipelines.effect(fallback);

        self.draw_textured_quad(
            encoder,
            &target.view,
            pipeline,
            &frame_bind_group,
            &texture_bind_group,
            &vertex_buffer,
            vertices.len() as u32,
            clear,
            &format!("wp-engine effect fallback {}", pass.label),
        );
    }

    fn execute_final_composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        render_graph: &SceneRenderGraph,
        targets: &GpuRenderTargets,
        cleared_targets: &mut BTreeSet<String>,
    ) {
        let Some(target) = targets.texture(&render_graph.final_pass.target) else {
            self.diagnostics.push(format!(
                "final composite skipped: target {} is not allocated",
                describe_target(&render_graph.final_pass.target)
            ));
            return;
        };

        let vertices = fullscreen_quad_vertices();
        let vertex_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine final composite vertices",
            wgpu::BufferUsages::VERTEX,
            &vertices,
        );
        let mut drew_any = false;

        for source_ref in render_graph.final_pass.inputs.iter().cloned() {
            if target_key(&source_ref) == target_key(&render_graph.final_pass.target) {
                self.diagnostics.push(format!(
                    "final composite skipped input: source and target are both {}",
                    describe_target(&source_ref)
                ));
                continue;
            }

            let Some(source) = targets.texture(&source_ref) else {
                self.diagnostics.push(format!(
                    "final composite skipped input: source {} is not allocated",
                    describe_target(&source_ref)
                ));
                continue;
            };

            let frame_bind_group = self.create_default_frame_bind_group(source.size);
            let texture_bind_group = self.create_texture_bind_group(&source.view);
            let clear = if drew_any {
                false
            } else {
                mark_target_for_load(cleared_targets, &render_graph.final_pass.target)
            };

            self.draw_textured_quad(
                encoder,
                &target.view,
                &self.pipelines.replace,
                &frame_bind_group,
                &texture_bind_group,
                &vertex_buffer,
                vertices.len() as u32,
                clear,
                "wp-engine final composite",
            );
            drew_any = true;
        }

        if !drew_any {
            self.diagnostics.push(format!(
                "final composite skipped: no valid inputs were available for {}",
                describe_target(&render_graph.final_pass.target)
            ));
        }
    }

    fn draw_textured_quad(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        frame_bind_group: &wgpu::BindGroup,
        texture_bind_group: &wgpu::BindGroup,
        vertex_buffer: &wgpu::Buffer,
        vertex_count: u32,
        clear: bool,
        label: &str,
    ) {
        let color_attachment = Some(wgpu::RenderPassColorAttachment {
            view: target_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[color_attachment],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, frame_bind_group, &[]);
        pass.set_bind_group(1, texture_bind_group, &[]);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.draw(0..vertex_count, 0..1);
    }

    fn create_frame_bind_group(
        &self,
        context: &FrameContext,
        object_index: usize,
    ) -> wgpu::BindGroup {
        let frame_uniforms = GpuFrameUniforms::from_context(context, object_index);
        self.create_frame_bind_group_from_uniforms(frame_uniforms)
    }

    fn create_default_frame_bind_group(&self, size: [u32; 2]) -> wgpu::BindGroup {
        self.create_frame_bind_group_from_uniforms(GpuFrameUniforms::for_size(size))
    }

    fn create_frame_bind_group_from_uniforms(
        &self,
        frame_uniforms: GpuFrameUniforms,
    ) -> wgpu::BindGroup {
        let frame_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine frame uniforms",
            wgpu::BufferUsages::UNIFORM,
            &[frame_uniforms],
        );
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wp-engine frame bind group"),
                layout: &self.frame_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                }],
            })
    }

    fn create_texture_bind_group(&self, source_view: &wgpu::TextureView) -> wgpu::BindGroup {
        self.gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wp-engine texture bind group"),
                layout: &self.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            })
    }

    fn pipeline_for_blend(&self, blend: Option<&str>, blend_mode: u32) -> &wgpu::RenderPipeline {
        match blend.unwrap_or("normal").to_ascii_lowercase().as_str() {
            "disabled" | "none" | "replace" => &self.pipelines.replace,
            "add" | "additive" => &self.pipelines.additive,
            "multiply" | "mul" => &self.pipelines.multiply,
            "max" | "lighten" => &self.pipelines.max,
            "normal" | "alpha" | "translucent" | "premultiplied" => match blend_mode {
                2 => &self.pipelines.additive,
                4 => &self.pipelines.multiply,
                5 => &self.pipelines.max,
                _ => &self.pipelines.alpha,
            },
            _ => match blend_mode {
                2 => &self.pipelines.additive,
                4 => &self.pipelines.multiply,
                5 => &self.pipelines.max,
                _ => &self.pipelines.alpha,
            },
        }
    }

    fn probe_material_shader(
        &mut self,
        graph: &SceneGraph,
        node: &ImageNode,
        pass: &RenderPassNode,
    ) {
        let Some(model) = &node.model else {
            return;
        };
        let Some(material_pass) = model
            .material
            .passes
            .iter()
            .find(|material_pass| material_pass.shader == pass.shader)
            .or_else(|| model.material.passes.first())
        else {
            return;
        };

        let translator = ShaderTranslator;
        for translation in translator.translate_material_pass(&graph.assets, material_pass) {
            self.diagnostics.push(format!(
                "shader probe {:?} {:?} {}: {}",
                translation.language,
                translation.stage,
                translation.label,
                if translation.succeeded() {
                    "translated"
                } else {
                    "using builtin fallback"
                }
            ));
            for diagnostic in translation.diagnostics {
                self.diagnostics
                    .push(format!("shader diagnostic: {:?}", diagnostic));
            }
        }
    }

    fn push_graph_diagnostics(&mut self, render_graph: &SceneRenderGraph) {
        for diagnostic in &render_graph.diagnostics {
            let severity = match diagnostic.severity {
                DiagnosticSeverity::Warning => "warning",
                DiagnosticSeverity::Error => "error",
            };
            let object = diagnostic
                .object_id
                .map(|id| format!(" object={id}"))
                .unwrap_or_default();
            self.diagnostics
                .push(format!("graph {severity}{object}: {}", diagnostic.message));
        }
    }

    fn push_effect_diagnostics(&mut self, graph: &SceneGraph) {
        let report = collect_effect_diagnostics(graph);
        for line in report.summary_lines() {
            self.diagnostics.push(format!("effect diagnostic: {line}"));
        }
        for line in report.missing_feature_lines() {
            self.diagnostics.push(format!("missing feature: {line}"));
        }
    }
}

struct BuiltinPipelines {
    replace: wgpu::RenderPipeline,
    alpha: wgpu::RenderPipeline,
    additive: wgpu::RenderPipeline,
    multiply: wgpu::RenderPipeline,
    max: wgpu::RenderPipeline,
    effect_tint: wgpu::RenderPipeline,
    effect_opacity: wgpu::RenderPipeline,
    effect_pulse: wgpu::RenderPipeline,
    effect_shake: wgpu::RenderPipeline,
    effect_scroll: wgpu::RenderPipeline,
    effect_spin: wgpu::RenderPipeline,
}

impl BuiltinPipelines {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        shader: &wgpu::ShaderModule,
    ) -> Self {
        Self {
            replace: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine replace pipeline",
                "fs_main",
                None,
            ),
            alpha: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine alpha pipeline",
                "fs_main",
                Some(wgpu::BlendState::ALPHA_BLENDING),
            ),
            additive: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine additive pipeline",
                "fs_main",
                Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
            ),
            multiply: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine multiply pipeline",
                "fs_main",
                Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Dst,
                        dst_factor: wgpu::BlendFactor::Zero,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
            ),
            max: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine max pipeline",
                "fs_main",
                Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Max,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
            ),
            effect_tint: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine tint fallback pipeline",
                HandwrittenEffectFallback::Tint.fragment_entry_point(),
                None,
            ),
            effect_opacity: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine opacity fallback pipeline",
                HandwrittenEffectFallback::Opacity.fragment_entry_point(),
                None,
            ),
            effect_pulse: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine pulse fallback pipeline",
                HandwrittenEffectFallback::Pulse.fragment_entry_point(),
                None,
            ),
            effect_shake: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine shake fallback pipeline",
                HandwrittenEffectFallback::Shake.fragment_entry_point(),
                None,
            ),
            effect_scroll: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine scroll fallback pipeline",
                HandwrittenEffectFallback::Scroll.fragment_entry_point(),
                None,
            ),
            effect_spin: create_pipeline(
                device,
                layout,
                shader,
                "wp-engine spin fallback pipeline",
                HandwrittenEffectFallback::Spin.fragment_entry_point(),
                None,
            ),
        }
    }

    fn effect(&self, fallback: HandwrittenEffectFallback) -> &wgpu::RenderPipeline {
        match fallback {
            HandwrittenEffectFallback::Tint => &self.effect_tint,
            HandwrittenEffectFallback::Opacity => &self.effect_opacity,
            HandwrittenEffectFallback::Pulse => &self.effect_pulse,
            HandwrittenEffectFallback::Shake => &self.effect_shake,
            HandwrittenEffectFallback::Scroll => &self.effect_scroll,
            HandwrittenEffectFallback::Spin => &self.effect_spin,
        }
    }
}

struct GpuRenderTargets {
    backbuffer: GpuTexture,
    output: GpuTexture,
    named: BTreeMap<String, GpuTexture>,
}

impl GpuRenderTargets {
    fn new(
        device: &wgpu::Device,
        render_graph: &SceneRenderGraph,
        diagnostics: &mut Vec<String>,
    ) -> Self {
        let size = render_graph.size;
        let backbuffer = GpuTexture::render_target(device, "wp-engine backbuffer", size);
        let output = GpuTexture::render_target(device, "wp-engine output", size);
        let mut named = BTreeMap::new();

        for (name, desc) in &render_graph.targets {
            let target_size = render_target_size(size, desc, diagnostics);
            diagnostics.push(format!(
                "allocated render target '{}' {}x{} format={} unique={}",
                desc.name, target_size[0], target_size[1], desc.format, desc.unique
            ));
            named.insert(
                name.clone(),
                GpuTexture::render_target(device, &format!("wp-engine target {name}"), target_size),
            );
        }

        Self {
            backbuffer,
            output,
            named,
        }
    }

    fn texture(&self, target: &RenderTargetRef) -> Option<&GpuTexture> {
        match target {
            RenderTargetRef::Backbuffer => Some(&self.backbuffer),
            RenderTargetRef::Output => Some(&self.output),
            RenderTargetRef::Named(name) => self.named.get(name),
        }
    }
}

struct GpuTexture {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

impl GpuTexture {
    fn render_target(device: &wgpu::Device, label: &str, size: [u32; 2]) -> Self {
        let texture = create_render_texture(device, label, size[0], size[1]);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            view,
            size,
        }
    }
}

#[derive(Default)]
struct UploadedTextureCache {
    textures: BTreeMap<String, UploadedTexture>,
}

impl UploadedTextureCache {
    fn get_or_upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        assets: &crate::engine::assets::AssetStore,
        name: &str,
        diagnostics: &mut Vec<String>,
    ) -> Result<&UploadedTexture> {
        if !self.textures.contains_key(name) {
            let source_image = assets
                .read_texture_rgba(name)
                .with_context(|| format!("loading base texture {name}"))?;
            diagnostics.push(format!(
                "uploaded texture '{}' {}x{}",
                name,
                source_image.width(),
                source_image.height()
            ));
            let texture = upload_rgba_texture(
                device,
                queue,
                &format!("wp-engine source texture {name}"),
                &source_image,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.textures.insert(
                name.to_string(),
                UploadedTexture {
                    _texture: texture,
                    view,
                    size: [source_image.width(), source_image.height()],
                },
            );
        }

        Ok(self
            .textures
            .get(name)
            .expect("uploaded texture cache entry should exist"))
    }
}

struct UploadedTexture {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
}

#[derive(Clone, Copy)]
struct SelectedTexture<'a> {
    name: &'a str,
    source: TextureSource,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct QuadVertex {
    position: [f32; 2],
    uv: [f32; 2],
}

impl QuadVertex {
    fn layout<'a>() -> wgpu::VertexBufferLayout<'a> {
        const ATTRIBUTES: [wgpu::VertexAttribute; 2] = [
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: std::mem::size_of::<[f32; 2]>() as u64,
                shader_location: 1,
            },
        ];

        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<QuadVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRIBUTES,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuFrameUniforms {
    resolution_time_delta: [f32; 4],
    frame_mouse: [f32; 4],
    camera_eye_parallax: [f32; 4],
    camera_center_audio: [f32; 4],
    object_origin_alpha: [f32; 4],
    object_size_flags: [f32; 4],
    object_color_tint: [f32; 4],
    audio_bands: [f32; 4],
    model: [[f32; 4]; 4],
    view_projection: [[f32; 4]; 4],
}

impl GpuFrameUniforms {
    fn from_context(context: &FrameContext, object_index: usize) -> Self {
        let object = context.object_state(object_index);
        Self {
            resolution_time_delta: [
                context.resolution[0] as f32,
                context.resolution[1] as f32,
                context.time,
                context.delta_time,
            ],
            frame_mouse: [
                context.frame_index as f32,
                context.mouse.position[0],
                context.mouse.position[1],
                context.mouse.buttons[0],
            ],
            camera_eye_parallax: [
                context.camera.eye[0],
                context.camera.eye[1],
                context.camera.eye[2],
                context.camera.parallax_amount,
            ],
            camera_center_audio: [
                context.camera.center[0],
                context.camera.center[1],
                context.camera.center[2],
                context.audio.level,
            ],
            object_origin_alpha: object
                .map(|state| {
                    [
                        state.origin[0],
                        state.origin[1],
                        state.origin[2],
                        state.alpha,
                    ]
                })
                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
            object_size_flags: object
                .map(|state| [state.size[0], state.size[1], state.scale[0], state.scale[1]])
                .unwrap_or([0.0, 0.0, 0.0, 1.0]),
            object_color_tint: object
                .map(|state| [state.color[0], state.color[1], state.color[2], state.alpha])
                .unwrap_or([1.0, 1.0, 1.0, 1.0]),
            audio_bands: context.audio.bands,
            model: object
                .map(|state| state.transform)
                .unwrap_or_else(crate::engine::frame_context::identity_matrix),
            view_projection: context.camera.view_projection,
        }
    }

    fn for_size(size: [u32; 2]) -> Self {
        Self {
            resolution_time_delta: [size[0] as f32, size[1] as f32, 0.0, 0.0],
            frame_mouse: [0.0, 0.0, 0.0, 0.0],
            camera_eye_parallax: [0.0, 0.0, 0.0, 0.0],
            camera_center_audio: [0.0, 0.0, 0.0, 0.0],
            object_origin_alpha: [0.0, 0.0, 0.0, 1.0],
            object_size_flags: [0.0, 0.0, 0.0, 1.0],
            object_color_tint: [1.0, 1.0, 1.0, 1.0],
            audio_bands: [0.0, 0.0, 0.0, 0.0],
            model: crate::engine::frame_context::identity_matrix(),
            view_projection: crate::engine::frame_context::identity_matrix(),
        }
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    label: &str,
    fragment_entry: &str,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[QuadVertex::layout()],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            unclipped_depth: false,
            polygon_mode: wgpu::PolygonMode::Fill,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fragment_entry),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: OUTPUT_FORMAT,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview: None,
        cache: None,
    })
}

fn select_material_texture<'a>(
    pass: &'a RenderPassNode,
    base_texture: Option<&'a str>,
) -> Option<SelectedTexture<'a>> {
    pass.textures
        .iter()
        .find(|texture| texture.slot == 0)
        .or_else(|| pass.textures.first())
        .map(|texture| SelectedTexture {
            name: texture.name.as_str(),
            source: texture.source,
        })
        .or_else(|| {
            base_texture.map(|name| SelectedTexture {
                name,
                source: TextureSource::MaterialTexture,
            })
        })
}

fn unsupported_blend_mode(blend: Option<&str>) -> Option<&str> {
    let blend = blend?;
    match blend.to_ascii_lowercase().as_str() {
        "disabled" | "none" | "replace" | "add" | "additive" | "multiply" | "mul" | "max"
        | "lighten" | "normal" | "alpha" | "translucent" | "premultiplied" => None,
        _ => Some(blend),
    }
}

fn pass_source_ref(pass: &RenderPassNode) -> RenderTargetRef {
    pass.source.clone().unwrap_or_else(|| {
        pass.textures
            .iter()
            .find(|texture| texture.source == TextureSource::RenderTarget)
            .map(|texture| RenderTargetRef::Named(texture.name.clone()))
            .unwrap_or(RenderTargetRef::Backbuffer)
    })
}

fn image_layer_vertices(
    graph: &SceneGraph,
    node: &ImageNode,
    context: &FrameContext,
    source_size: [u32; 2],
) -> [QuadVertex; 6] {
    if node.is_fullscreen_layer() {
        return fullscreen_quad_vertices();
    }

    let object = graph.object(node);
    let state = context.object_state(node.object_index);
    let origin = state
        .map(|state| {
            [
                state.origin[0] as f64,
                state.origin[1] as f64,
                state.origin[2] as f64,
            ]
        })
        .unwrap_or_else(|| {
            let origin = object.parsed_origin();
            [origin[0], origin[1], origin[2]]
        });
    let object_size = state
        .map(|state| {
            [
                state.size[0] as f64,
                state.size[1] as f64,
                state.size[2] as f64,
            ]
        })
        .unwrap_or_else(|| {
            let size = object.parsed_size();
            [size[0], size[1], size[2]]
        });
    let scale = state
        .map(|state| {
            [
                state.scale[0] as f64,
                state.scale[1] as f64,
                state.scale[2] as f64,
            ]
        })
        .unwrap_or_else(|| {
            let scale = object
                .scale
                .as_ref()
                .and_then(crate::engine::scene::parse_value_vec3)
                .unwrap_or([1.0, 1.0, 1.0]);
            [scale[0] as f64, scale[1] as f64, scale[2] as f64]
        });
    let rotation_z = state
        .map(|state| state.rotation_z as f64)
        .unwrap_or_else(|| {
            object
                .angles
                .as_ref()
                .and_then(crate::engine::scene::parse_value_vec3)
                .map(|rotation| rotation[2] as f64)
                .unwrap_or(0.0)
        });
    let model_size = node
        .model
        .as_ref()
        .map(|model| {
            [
                model.width.unwrap_or(source_size[0]) as f64,
                model.height.unwrap_or(source_size[1]) as f64,
            ]
        })
        .unwrap_or([source_size[0] as f64, source_size[1] as f64]);

    let draw_w = if object_size[0] > 0.0 {
        object_size[0] * scale[0]
    } else {
        model_size[0] * scale[0]
    }
    .max(1.0);
    let draw_h = if object_size[1] > 0.0 {
        object_size[1] * scale[1]
    } else {
        model_size[1] * scale[1]
    }
    .max(1.0);

    let width = context.resolution[0] as f64;
    let height = context.resolution[1] as f64;
    let center_x = width / 2.0 + origin[0];
    let center_y = height / 2.0 - origin[1];
    let half_w = draw_w / 2.0;
    let half_h = draw_h / 2.0;
    let angle = rotation_z.to_radians();
    let (sin, cos) = angle.sin_cos();

    rotated_quad_vertices(center_x, center_y, half_w, half_h, cos, sin, width, height)
}

fn fullscreen_quad_vertices() -> [QuadVertex; 6] {
    quad_vertices(-1.0, 1.0, -1.0, 1.0)
}

fn rotated_quad_vertices(
    center_x: f64,
    center_y: f64,
    half_w: f64,
    half_h: f64,
    cos: f64,
    sin: f64,
    width: f64,
    height: f64,
) -> [QuadVertex; 6] {
    let corners = [
        (-half_w, -half_h, [0.0, 1.0]),
        (half_w, -half_h, [1.0, 1.0]),
        (half_w, half_h, [1.0, 0.0]),
        (-half_w, half_h, [0.0, 0.0]),
    ];

    let mut verts = [QuadVertex {
        position: [0.0, 0.0],
        uv: [0.0, 0.0],
    }; 6];
    let indices = [(0, 1, 2), (0, 2, 3)];

    for (tri_index, (a, b, c)) in indices.into_iter().enumerate() {
        for (local_index, corner_index) in [a, b, c].into_iter().enumerate() {
            let (dx, dy, uv) = corners[corner_index];
            let rx = dx * cos - dy * sin;
            let ry = dx * sin + dy * cos;
            let x = center_x + rx;
            let y = center_y + ry;
            let vertex = QuadVertex {
                position: [
                    (x / width * 2.0 - 1.0) as f32,
                    (1.0 - y / height * 2.0) as f32,
                ],
                uv,
            };
            verts[tri_index * 3 + local_index] = vertex;
        }
    }

    verts
}

fn quad_vertices(left: f32, right: f32, bottom: f32, top: f32) -> [QuadVertex; 6] {
    [
        QuadVertex {
            position: [left, bottom],
            uv: [0.0, 1.0],
        },
        QuadVertex {
            position: [right, bottom],
            uv: [1.0, 1.0],
        },
        QuadVertex {
            position: [right, top],
            uv: [1.0, 0.0],
        },
        QuadVertex {
            position: [left, bottom],
            uv: [0.0, 1.0],
        },
        QuadVertex {
            position: [right, top],
            uv: [1.0, 0.0],
        },
        QuadVertex {
            position: [left, top],
            uv: [0.0, 0.0],
        },
    ]
}

fn create_buffer_init<T: Pod>(
    device: &wgpu::Device,
    label: &str,
    usage: wgpu::BufferUsages,
    data: &[T],
) -> wgpu::Buffer {
    let bytes = bytemuck::cast_slice(data);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.len() as u64,
        usage,
        mapped_at_creation: true,
    });
    buffer
        .slice(..)
        .get_mapped_range_mut()
        .copy_from_slice(bytes);
    buffer.unmap();
    buffer
}

fn create_render_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

fn upload_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    image: &RgbaImage,
) -> wgpu::Texture {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: OUTPUT_FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });

    queue.write_texture(
        texture.as_image_copy(),
        image.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4 * image.width()),
            rows_per_image: Some(image.height()),
        },
        wgpu::Extent3d {
            width: image.width(),
            height: image.height(),
            depth_or_array_layers: 1,
        },
    );

    texture
}

fn clear_target(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, label: &str) {
    let color_attachment = Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    });
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[color_attachment],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
}

fn read_texture_to_rgba(
    gpu: &GpuDevice,
    texture: &wgpu::Texture,
    mut encoder: wgpu::CommandEncoder,
    width: u32,
    height: u32,
) -> Result<RgbaImage> {
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = align_to(unpadded_bytes_per_row, wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let output_buffer_size = padded_bytes_per_row as u64 * height as u64;
    let output_buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("wp-engine scene readback buffer"),
        size: output_buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &output_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    let command_buffer = encoder.finish();
    gpu.queue.submit(Some(command_buffer));

    let slice = output_buffer.slice(..);
    let (tx, rx) = sync_channel(1);
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|err| anyhow!("polling GPU readback: {err:?}"))?;
    rx.recv()
        .context("waiting for GPU readback map")?
        .context("mapping GPU readback buffer")?;

    let mapped = slice.get_mapped_range();
    let mut pixels = vec![0u8; (width * height * 4) as usize];
    for row in 0..height as usize {
        let src_start = row * padded_bytes_per_row as usize;
        let src_end = src_start + unpadded_bytes_per_row as usize;
        let dst_start = row * unpadded_bytes_per_row as usize;
        let dst_end = dst_start + unpadded_bytes_per_row as usize;
        pixels[dst_start..dst_end].copy_from_slice(&mapped[src_start..src_end]);
    }
    drop(mapped);
    output_buffer.unmap();

    RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| anyhow!("GPU readback produced invalid RGBA image dimensions"))
}

fn render_target_size(
    graph_size: [u32; 2],
    desc: &RenderTargetDesc,
    diagnostics: &mut Vec<String>,
) -> [u32; 2] {
    if !matches!(
        desc.format.to_ascii_lowercase().as_str(),
        "rgba8888" | "rgba8" | "rgba"
    ) {
        diagnostics.push(format!(
            "render target '{}' requests unsupported format '{}'; using rgba8",
            desc.name, desc.format
        ));
    }

    let scale = if desc.scale.is_finite() && desc.scale > 0.0 {
        desc.scale
    } else {
        diagnostics.push(format!(
            "render target '{}' has invalid scale {}; using 1.0",
            desc.name, desc.scale
        ));
        1.0
    };

    [
        ((graph_size[0] as f32 * scale).round() as u32).max(1),
        ((graph_size[1] as f32 * scale).round() as u32).max(1),
    ]
}

fn mark_target_for_load(cleared_targets: &mut BTreeSet<String>, target: &RenderTargetRef) -> bool {
    cleared_targets.insert(target_key(target))
}

fn target_key(target: &RenderTargetRef) -> String {
    match target {
        RenderTargetRef::Backbuffer => "$backbuffer".to_string(),
        RenderTargetRef::Output => "$output".to_string(),
        RenderTargetRef::Named(name) => name.clone(),
    }
}

fn describe_target(target: &RenderTargetRef) -> String {
    match target {
        RenderTargetRef::Backbuffer => "backbuffer".to_string(),
        RenderTargetRef::Output => "output".to_string(),
        RenderTargetRef::Named(name) => format!("target '{name}'"),
    }
}

fn display_object_name(name: &str) -> &str {
    if name.is_empty() {
        "<unnamed>"
    } else {
        name
    }
}

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
