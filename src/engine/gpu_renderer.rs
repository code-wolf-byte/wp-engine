use anyhow::{anyhow, Context, Result};
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::engine::model::{ShaderModel, WEBlending};
use crate::engine::shaders::{effect_def, loader, transpiler};
use crate::platform::GpuDevice;

const SHADER_SRC: &str = include_str!("shaders/gpu_shaders.wgsl");

pub struct GpuSceneRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    sampler: wgpu::Sampler,
    base_bgl: wgpu::BindGroupLayout,
    effect_bgl: wgpu::BindGroupLayout,
    composite_pipelines: Vec<(u32, wgpu::RenderPipeline)>,
    effect_pipelines: Vec<(&'static str, wgpu::RenderPipeline)>,
    dynamic_pipelines: HashMap<String, wgpu::RenderPipeline>,
    dynamic_uniform_keys: HashMap<String, Vec<(String, f32)>>,
    effect_pipeline_layout: wgpu::PipelineLayout,
    shader_module: wgpu::ShaderModule,
}

impl GpuSceneRenderer {
    pub fn new(gpu: GpuDevice) -> Result<Self> {
        let GpuDevice { device, queue, .. } = gpu;

        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_shaders"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scene_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });

        let base_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("base_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let effect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("composite_layout"),
            bind_group_layouts: &[&base_bgl],
            push_constant_ranges: &[],
        });

        let blend_states: &[(u32, wgpu::BlendState)] = &[
            (0, wgpu::BlendState::ALPHA_BLENDING),
            (2, wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
            (4, wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Dst,
                    dst_factor: wgpu::BlendFactor::Zero,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
            (5, wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Max,
                },
                alpha: wgpu::BlendComponent::OVER,
            }),
        ];

        let mut composite_pipelines = Vec::new();
        for (mode, blend) in blend_states {
            let p = Self::create_pipeline(
                &device, &pipeline_layout, &shader_module,
                "vs_fullscreen", "fs_composite", "composite",
                Some(*blend),
            );
            composite_pipelines.push((*mode, p));
        }

        let effect_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effect_layout"),
            bind_group_layouts: &[&base_bgl, &effect_bgl],
            push_constant_ranges: &[],
        });

        let effect_names = [
            "pulse", "scroll", "shake", "tint", "opacity",
            "waterripple", "waterwaves", "spin",
        ];
        let effect_entry_points: Vec<(&str, &str)> = effect_names.iter()
            .map(|n| (*n, match *n {
                "pulse" => "fs_pulse",
                "scroll" => "fs_scroll",
                "shake" => "fs_shake",
                "tint" => "fs_tint",
                "opacity" => "fs_opacity",
                "waterripple" => "fs_waterripple",
                "waterwaves" => "fs_waterwaves",
                "spin" => "fs_spin",
                _ => "fs_composite",
            }))
            .collect();

        let mut effect_pipelines = Vec::new();
        for (name, entry) in &effect_entry_points {
            let pipeline = Self::create_pipeline(
                &device, &effect_layout, &shader_module,
                "vs_fullscreen", entry, name, None,
            );
            effect_pipelines.push((*name, pipeline));
        }

        Ok(Self {
            device, queue, sampler, base_bgl, effect_bgl,
            composite_pipelines, effect_pipelines,
            dynamic_pipelines: HashMap::new(),
            dynamic_uniform_keys: HashMap::new(),
            effect_pipeline_layout: effect_layout,
            shader_module,
        })
    }

    fn create_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        module: &wgpu::ShaderModule,
        vs_entry: &str,
        fs_entry: &str,
        label: &str,
        blend: Option<wgpu::BlendState>,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(vs_entry),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(fs_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        })
    }

    fn create_pipeline_split_modules(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        vs_module: &wgpu::ShaderModule,
        vs_entry: &str,
        fs_module: &wgpu::ShaderModule,
        fs_entry: &str,
        label: &str,
        blend: Option<wgpu::BlendState>,
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: vs_module,
                entry_point: Some(vs_entry),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: fs_module,
                entry_point: Some(fs_entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        })
    }

    pub fn add_dynamic_pipeline(&mut self, key: String, wgsl: &str, uniform_keys: Vec<(String, f32)>) -> Result<()> {
        let fs_module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&key),
            source: wgpu::ShaderSource::Wgsl(wgsl.into()),
        });
        let pipeline = Self::create_pipeline_split_modules(
            &self.device, &self.effect_pipeline_layout,
            &self.shader_module, "vs_fullscreen",
            &fs_module, "main",
            &key, None,
        );
        self.dynamic_pipelines.insert(key.clone(), pipeline);
        self.dynamic_uniform_keys.insert(key, uniform_keys);
        Ok(())
    }

    pub fn upload_texture(&self, img: &RgbaImage) -> wgpu::Texture {
        let (w, h) = img.dimensions();
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer_tex"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex, mip_level: 0,
                origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
            },
            img.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        tex
    }

    pub fn create_render_target(&self, w: u32, h: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render_target"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn find_effect_pipeline(&self, name: &str) -> Option<&wgpu::RenderPipeline> {
        self.effect_pipelines.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, p)| p)
            .or_else(|| self.dynamic_pipelines.get(name))
    }

    fn composite_pipeline(&self, blend_mode: u32) -> &wgpu::RenderPipeline {
        self.composite_pipelines.iter()
            .find(|(m, _)| *m == blend_mode)
            .or_else(|| self.composite_pipelines.first())
            .map(|(_, p)| p)
            .unwrap()
    }

    pub fn render_frame(
        &self,
        layers: &[(&wgpu::Texture, f32, u32, [f32; 2], [f32; 2])],
        effects: &[(usize, &str, &[u8])],
        target: &wgpu::Texture,
        w: u32, h: u32,
        clear_color: [f64; 3],
    ) {
        let target_view = target.create_view(&Default::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame_encoder"),
        });

        {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear_color[0], g: clear_color[1], b: clear_color[2], a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
        }

        for (i, (layer_tex, opacity, blend_mode, offset_norm, scale)) in layers.iter().enumerate() {
            let mut current_view = layer_tex.create_view(&Default::default());
            let mut temp_target: Option<wgpu::Texture> = None;

            let layer_effects: Vec<_> = effects.iter()
                .filter(|(idx, _, _)| *idx == i)
                .collect();

            for (_, effect_name, params) in &layer_effects {
                if let Some(pipeline) = self.find_effect_pipeline(effect_name) {
                    let tmp = self.create_render_target(w, h);
                    let tmp_view = tmp.create_view(&Default::default());

                    let composite_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: None, size: 32,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    // CompositeParams: opacity=1, color=(1,1,1) — pass-through for effect intermediates
                    let mut dummy_composite = [0u8; 32];
                    dummy_composite[0..4].copy_from_slice(&1.0f32.to_le_bytes());   // opacity
                    dummy_composite[16..20].copy_from_slice(&1.0f32.to_le_bytes()); // color.r
                    dummy_composite[20..24].copy_from_slice(&1.0f32.to_le_bytes()); // color.g
                    dummy_composite[24..28].copy_from_slice(&1.0f32.to_le_bytes()); // color.b
                    self.queue.write_buffer(&composite_buf, 0, &dummy_composite);

                    let base_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None, layout: &self.base_bgl,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&current_view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                            wgpu::BindGroupEntry { binding: 2, resource: composite_buf.as_entire_binding() },
                        ],
                    });

                    let param_size = (params.len() as u64).max(16);
                    let param_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: None, size: param_size,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    let mut padded = vec![0u8; param_size as usize];
                    padded[..params.len()].copy_from_slice(params);
                    self.queue.write_buffer(&param_buf, 0, &padded);

                    let effect_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: None, layout: &self.effect_bgl,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: param_buf.as_entire_binding() },
                        ],
                    });

                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: Some("effect_pass"),
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &tmp_view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                    store: wgpu::StoreOp::Store,
                                },
                                depth_slice: None,
                            })],
                            ..Default::default()
                        });
                        pass.set_pipeline(pipeline);
                        pass.set_bind_group(0, &base_bg, &[]);
                        pass.set_bind_group(1, &effect_bg, &[]);
                        pass.draw(0..3, 0..1);
                    }

                    current_view = tmp_view;
                    temp_target = Some(tmp);
                }
            }

            // CompositeParams WGSL layout: opacity(0), pad(4-15), color.r(16), color.g(20), color.b(24)
            let mut composite_data = [0u8; 32];
            composite_data[0..4].copy_from_slice(&opacity.to_le_bytes());
            composite_data[16..20].copy_from_slice(&1.0f32.to_le_bytes()); // color.r
            composite_data[20..24].copy_from_slice(&1.0f32.to_le_bytes()); // color.g
            composite_data[24..28].copy_from_slice(&1.0f32.to_le_bytes()); // color.b

            let composite_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: None, size: 32,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.queue.write_buffer(&composite_buf, 0, &composite_data);

            let base_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &self.base_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&current_view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: composite_buf.as_entire_binding() },
                ],
            });

            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("composite_pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(self.composite_pipeline(*blend_mode));
                pass.set_bind_group(0, &base_bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    pub fn readback(&self, target: &wgpu::Texture, w: u32, h: u32) -> Result<RgbaImage> {
        // wgpu requires bytes_per_row to be a multiple of COPY_BYTES_PER_ROW_ALIGNMENT (256).
        // Widths not divisible by 64 (e.g. 1080, 1366) would panic without this padding.
        let unpadded_bpr = w * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bpr = (unpadded_bpr + align - 1) / align * align;
        let staging_size = (padded_bpr * h) as u64;

        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: staging_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: target, mip_level: 0,
                origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        let _ = self.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
        rx.recv()
            .map_err(|_| anyhow!("readback channel disconnected"))?
            .map_err(|e| anyhow!("buffer map failed: {e:?}"))?;

        let data = slice.get_mapped_range();
        // Strip row padding to produce a tightly-packed RGBA image.
        let pixels: Vec<u8> = data
            .chunks(padded_bpr as usize)
            .flat_map(|row| &row[..unpadded_bpr as usize])
            .copied()
            .collect();
        drop(data);
        staging.unmap();

        RgbaImage::from_raw(w, h, pixels)
            .context("creating RgbaImage from GPU readback")
    }
}

pub fn gpu_scene_render_loop(
    dir: &std::path::Path,
    tx: &SyncSender<Arc<RgbaImage>>,
    target_fps: f64,
) -> Result<()> {
    let gpu = GpuDevice::open_low_power()
        .or_else(|_| GpuDevice::open_best())
        .context("opening GPU device")?;

    let mut renderer = GpuSceneRenderer::new(gpu)?;
    let resolved = super::render::ResolvedScene::from_directory(dir)?;
    let w = resolved.width;
    let h = resolved.height;

    let scene_model = crate::engine::model::scene_to_model(&resolved.scene)?;

    let clear_color: [f64; 3] = resolved.scene.general.as_ref()
        .and_then(|g| g.clear_color.as_deref())
        .and_then(|s| {
            let parts: Vec<f64> = s.split_whitespace().filter_map(|p| p.parse().ok()).collect();
            if parts.len() >= 3 { Some([parts[0], parts[1], parts[2]]) } else { None }
        })
        .unwrap_or([0.0, 0.0, 0.0]);

    let gpu_layers: Vec<_> = resolved.layers.iter().enumerate()
        .map(|(i, l)| {
            let obj = scene_model.objects.get(i);
            let offset = obj.map(|o| {
                let xyz = o.origin_xyz();
                [xyz[0] / w as f32, xyz[1] / h as f32]
            }).unwrap_or([0.0f32, 0.0]);
            let scale = obj.map(|o| {
                let xyz = o.scale_xyz();
                [xyz[0], xyz[1]]
            }).unwrap_or([1.0f32, 1.0]);
            let resized = if l.image.width() != w || l.image.height() != h {
                image::imageops::resize(&l.image, w, h, image::imageops::FilterType::Lanczos3)
            } else {
                l.image.clone()
            };
            (renderer.upload_texture(&resized), 1.0f32, l.blend_mode, offset, scale)
        })
        .collect();

    let scene_effects = collect_effects(&scene_model);

    // Load and translate WE shaders for any effect not in the hardcoded 8.
    const HARDCODED: &[&str] = &["pulse","scroll","shake","tint","opacity","waterripple","waterwaves","spin"];
    if let Some(assets_dir) = loader::find_we_assets_dir() {
        for (_, effect_name, _) in &scene_effects {
            if HARDCODED.contains(&effect_name.as_str()) { continue; }
            if renderer.dynamic_pipelines.contains_key(effect_name) { continue; }
            let Ok(eff_def) = effect_def::load_effect_from_dir(&assets_dir, effect_name) else { continue; };
            let Some(mat_path) = eff_def.passes.first().and_then(|p| p.material.as_deref()) else { continue; };
            let Ok(mat_def) = effect_def::load_material_from_dir(&assets_dir, mat_path) else { continue; };
            let Some(shader_name) = mat_def.passes.first().and_then(|p| p.shader.as_deref()) else { continue; };
            let Ok((frag_glsl, _vert_glsl)) = loader::load_glsl_shader(&assets_dir, shader_name) else { continue; };
            let blending_str = mat_def.passes.first()
                .and_then(|p| p.blending.as_deref())
                .unwrap_or("normal");
            let model = ShaderModel::from_resolved_glsl(
                shader_name.to_string(),
                frag_glsl,
                HashMap::new(),
                WEBlending::from_str(blending_str),
            );
            let Ok(translated) = transpiler::translate(&model) else { continue; };
            let keys: Vec<(String, f32)> = translated.uniform_keys.iter().map(|u| (u.key.clone(), u.default)).collect();
            if let Err(e) = renderer.add_dynamic_pipeline(effect_name.clone(), &translated.wgsl, keys) {
                eprintln!("dynamic shader '{}' failed: {e}", effect_name);
            }
        }
    }

    let target = renderer.create_render_target(w, h);
    let frame_duration = Duration::from_secs_f64(1.0 / target_fps);
    let start = Instant::now();

    loop {
        let time = start.elapsed().as_secs_f32();

        let effects: Vec<(usize, &str, Vec<u8>)> = scene_effects.iter()
            .map(|(idx, name, vals)| {
                let params = if HARDCODED.contains(&name.as_str()) {
                    make_effect_params(name, time, vals)
                } else {
                    let keys = renderer.dynamic_uniform_keys.get(name).map(|v| v.as_slice()).unwrap_or(&[]);
                    make_params_from_translated(keys, time, vals)
                };
                (*idx, name.as_str(), params)
            })
            .collect();

        let effect_refs: Vec<(usize, &str, &[u8])> = effects.iter()
            .map(|(i, n, p)| (*i, *n, p.as_slice()))
            .collect();

        let layer_refs: Vec<(&wgpu::Texture, f32, u32, [f32; 2], [f32; 2])> = gpu_layers.iter()
            .map(|(t, o, b, off, sc)| (t, *o, *b, *off, *sc))
            .collect();
        renderer.render_frame(&layer_refs, &effect_refs, &target, w, h, clear_color);

        let frame = renderer.readback(&target, w, h)?;
        if tx.send(Arc::new(frame)).is_err() {
            return Ok(());
        }

        let elapsed = start.elapsed();
        let next = Duration::from_secs_f64((elapsed.as_secs_f64() / frame_duration.as_secs_f64()).ceil() * frame_duration.as_secs_f64());
        if next > elapsed {
            std::thread::sleep(next - elapsed);
        }
    }
}

type ShaderVals = std::collections::HashMap<String, f32>;

fn collect_effects(scene: &crate::engine::model::SceneModel) -> Vec<(usize, String, ShaderVals)> {
    let mut result = Vec::new();
    for (i, obj) in scene.objects.iter().enumerate() {
        if !obj.is_visible() { continue; }
        for eff in &obj.effects {
            let file = match &eff.file {
                Some(s) => s,
                None => continue,
            };
            let name = file.rsplit('/').next().unwrap_or(file)
                .trim_end_matches(".json")
                .to_lowercase();
            let vals: ShaderVals = eff.passes.first()
                .map(|p| {
                    let mut map = ShaderVals::new();
                    for (k, v) in &p.shader_values {
                        if let Some(f) = v.as_float() {
                            map.insert(k.clone(), f);
                        } else if let Some([r, g, b]) = v.as_vec3() {
                            map.insert(format!("{k}_r"), r);
                            map.insert(format!("{k}_g"), g);
                            map.insert(format!("{k}_b"), b);
                        }
                    }
                    map
                })
                .unwrap_or_default();
            result.push((i, name, vals));
        }
    }
    result
}

fn make_params_from_translated(keys: &[(String, f32)], time: f32, vals: &ShaderVals) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (key, default) in keys {
        let k = key.to_lowercase();
        let val = if k.contains("time") {
            time
        } else if k == "g_alpha" || k == "g_useralpha" {
            vals.get(key).copied().unwrap_or(1.0)
        } else if k == "g_brightness" {
            vals.get(key).copied().unwrap_or(1.0)
        } else if k == "g_color" {
            vals.get(key).copied().unwrap_or(1.0)
        } else if k == "g_lightambientcolor" || k == "g_lightskylightcolor" {
            vals.get(key).copied().unwrap_or(1.0)
        } else {
            vals.get(key).copied().unwrap_or(*default)
        };
        bytes.extend_from_slice(&val.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 12]); // std140 scalar slot = 16 bytes
    }
    if bytes.is_empty() {
        bytes.extend_from_slice(&[0u8; 16]);
    }
    bytes
}

fn make_effect_params(name: &str, time: f32, vals: &ShaderVals) -> Vec<u8> {
    fn get(vals: &ShaderVals, key: &str, default: f32) -> f32 {
        vals.get(key).copied().unwrap_or(default)
    }
    fn pack(floats: &[f32]) -> Vec<u8> {
        floats.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    match name {
        "pulse" => pack(&[
            time,
            get(vals, "ui_editor_properties_pulse_speed", 3.0),
            get(vals, "ui_editor_properties_pulse_amount", 1.0),
            get(vals, "ui_editor_properties_power", 1.0),
            get(vals, "ui_editor_properties_pulse_phase", 0.0),
            0.0, 0.0, 0.0,
            // tint_low: 0.5 so pulse oscillates 50%–100% brightness
            get(vals, "ui_editor_properties_tint_low_r", 0.5),
            get(vals, "ui_editor_properties_tint_low_g", 0.5),
            get(vals, "ui_editor_properties_tint_low_b", 0.5),
            0.0, // pad
            // tint_high RGB (default white)
            get(vals, "ui_editor_properties_tint_high_r", 1.0),
            get(vals, "ui_editor_properties_tint_high_g", 1.0),
            get(vals, "ui_editor_properties_tint_high_b", 1.0),
            0.0, // pad
        ]),
        "scroll" => pack(&[
            time,
            get(vals, "ui_editor_properties_speed_x", 0.1),
            get(vals, "ui_editor_properties_speed_y", 0.0),
            get(vals, "ui_editor_properties_scale_x", 1.0),
            get(vals, "ui_editor_properties_scale_y", 1.0),
            0.0, 0.0, 0.0,
        ]),
        "shake" => pack(&[
            time,
            get(vals, "ui_editor_properties_speed", 1.0),
            get(vals, "ui_editor_properties_strength", 0.1),
            0.0,
        ]),
        "tint" => {
            let r = get(vals, "ui_editor_properties_color_r", 1.0);
            let g = get(vals, "ui_editor_properties_color_g", 1.0);
            let b = get(vals, "ui_editor_properties_color_b", 1.0);
            let a = get(vals, "ui_editor_properties_opacity", 0.5);
            pack(&[r, g, b, a])
        }
        "opacity" => pack(&[
            get(vals, "ui_editor_properties_opacity", 1.0),
            0.0, 0.0, 0.0,
        ]),
        "waterripple" => pack(&[
            time,
            get(vals, "ui_editor_properties_strength", 0.1),
            get(vals, "ui_editor_properties_speed", 0.15),
            get(vals, "ui_editor_properties_scale", 1.0),
        ]),
        "waterwaves" => {
            let dir = get(vals, "ui_editor_properties_direction", 0.0);
            pack(&[
                time,
                get(vals, "ui_editor_properties_speed", 5.0),
                get(vals, "ui_editor_properties_scale", 200.0),
                get(vals, "ui_editor_properties_strength", 0.1),
                dir.cos(),
                dir.sin(),
                0.0, 0.0,
            ])
        }
        "spin" => pack(&[
            time,
            get(vals, "ui_editor_properties_speed", 1.0),
            get(vals, "ui_editor_properties_center_x", 0.5),
            get(vals, "ui_editor_properties_center_y", 0.5),
        ]),
        _ => vec![0u8; 32],
    }
}
