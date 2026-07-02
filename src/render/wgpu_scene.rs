use std::borrow::Cow;
use std::path::Path;
use std::sync::mpsc::sync_channel;

use anyhow::{anyhow, Context, Result};
use bytemuck::{Pod, Zeroable};
use image::RgbaImage;

use crate::engine::shader::{textured_quad_wgsl, ShaderTranslator};
use crate::engine::{FrameContext, SceneGraph, SceneRenderGraph};
use crate::platform::GpuDevice;

const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct WgpuSceneRenderResult {
    pub image: RgbaImage,
    pub graph: SceneRenderGraph,
    pub diagnostics: Vec<String>,
}

pub fn render_first_image_layer_to_rgba(dir: &Path) -> Result<WgpuSceneRenderResult> {
    let graph = SceneGraph::from_directory(dir)?;
    let render_graph = SceneRenderGraph::from_scene_graph(&graph);
    let context = FrameContext::for_graph(&graph, 0.0, 1.0 / 60.0, 0);
    let gpu = crate::platform::open_device()?;
    let mut renderer = WgpuSceneRenderer::new(gpu)?;
    let image = renderer.render_first_image_layer(&graph, &render_graph, &context)?;

    Ok(WgpuSceneRenderResult {
        image,
        graph: render_graph,
        diagnostics: renderer.diagnostics,
    })
}

pub struct WgpuSceneRenderer {
    gpu: GpuDevice,
    pipeline: wgpu::RenderPipeline,
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

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wp-engine scene textured image pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
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
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUTPUT_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

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
            pipeline,
            frame_bind_group_layout,
            texture_bind_group_layout,
            sampler,
            diagnostics: Vec::new(),
        })
    }

    pub fn render_first_image_layer(
        &mut self,
        graph: &SceneGraph,
        render_graph: &SceneRenderGraph,
        context: &FrameContext,
    ) -> Result<RgbaImage> {
        let object = render_graph
            .first_textured_object()
            .ok_or_else(|| anyhow!("render graph has no model-backed image with a base texture"))?;
        let node = graph
            .images
            .iter()
            .find(|node| node.object_index == object.scene_object_index)
            .ok_or_else(|| anyhow!("render graph object does not map back to an image node"))?;
        let texture_name = node
            .base_texture
            .as_deref()
            .ok_or_else(|| anyhow!("selected image node has no base texture"))?;

        self.diagnostics.push(format!(
            "selected object '{}' model={} texture={}",
            if object.name.is_empty() {
                "<unnamed>"
            } else {
                &object.name
            },
            object.model.as_deref().unwrap_or("<none>"),
            texture_name
        ));
        self.diagnostics.push(
            "geometry: generated image quad; puppet/binary mesh loading is pending".to_string(),
        );

        if let Some(model) = &node.model {
            if let Some(pass) = model.material.passes.first() {
                let translator = ShaderTranslator;
                for translation in translator.translate_material_pass(&graph.assets, pass) {
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
        }

        let source_image = graph
            .assets
            .read_texture_rgba(texture_name)
            .with_context(|| format!("loading base texture {texture_name}"))?;

        let [width, height] = context.resolution;
        let output_texture = create_render_texture(&self.gpu.device, width, height);
        let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let source_texture = upload_rgba_texture(
            &self.gpu.device,
            &self.gpu.queue,
            "wp-engine source image texture",
            &source_image,
        );
        let source_view = source_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let frame_uniforms = GpuFrameUniforms::from_context(context, node.object_index);
        let frame_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine frame uniforms",
            wgpu::BufferUsages::UNIFORM,
            &[frame_uniforms],
        );
        let frame_bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wp-engine frame bind group"),
                layout: &self.frame_bind_group_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                }],
            });
        let texture_bind_group = self
            .gpu
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("wp-engine texture bind group"),
                layout: &self.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });

        let vertices = fullscreen_quad_vertices();
        let vertex_buffer = create_buffer_init(
            &self.gpu.device,
            "wp-engine fullscreen quad vertices",
            wgpu::BufferUsages::VERTEX,
            &vertices,
        );

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("wp-engine scene render encoder"),
            });
        {
            let color_attachment = Some(wgpu::RenderPassColorAttachment {
                view: &output_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("wp-engine first image pass"),
                color_attachments: &[color_attachment],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &frame_bind_group, &[]);
            pass.set_bind_group(1, &texture_bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            pass.draw(0..vertices.len() as u32, 0..1);
        }

        read_texture_to_rgba(&self.gpu, &output_texture, encoder, width, height)
    }
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
    resolution: [f32; 2],
    time: f32,
    delta_time: f32,
    model: [[f32; 4]; 4],
    view_projection: [[f32; 4]; 4],
}

impl GpuFrameUniforms {
    fn from_context(context: &FrameContext, object_index: usize) -> Self {
        Self {
            resolution: [context.resolution[0] as f32, context.resolution[1] as f32],
            time: context.time,
            delta_time: context.delta_time,
            model: context
                .object_state(object_index)
                .map(|state| state.transform)
                .unwrap_or_else(crate::engine::frame_context::identity_matrix),
            view_projection: context.camera.view_projection,
        }
    }
}

fn fullscreen_quad_vertices() -> [QuadVertex; 6] {
    [
        QuadVertex {
            position: [-1.0, -1.0],
            uv: [0.0, 1.0],
        },
        QuadVertex {
            position: [1.0, -1.0],
            uv: [1.0, 1.0],
        },
        QuadVertex {
            position: [1.0, 1.0],
            uv: [1.0, 0.0],
        },
        QuadVertex {
            position: [-1.0, -1.0],
            uv: [0.0, 1.0],
        },
        QuadVertex {
            position: [1.0, 1.0],
            uv: [1.0, 0.0],
        },
        QuadVertex {
            position: [-1.0, 1.0],
            uv: [0.0, 0.0],
        },
    ]
}

fn create_buffer_init<T: Pod>(
    device: &wgpu::Device,
    label: &'static str,
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

fn create_render_texture(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("wp-engine scene output texture"),
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
    label: &'static str,
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

fn align_to(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}
