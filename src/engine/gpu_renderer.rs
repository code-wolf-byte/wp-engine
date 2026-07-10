//! GPU scene renderer: pipelines, effect/FBO pass chaining, and the
//! per-scene [`GpuSceneInstance`] runtime.
//!
//! Pass model (mirrors linux-wallpaperengine's CWallpaper/CPass chain):
//! - every layer's base texture runs through its effect passes, ping-ponging
//!   between two persistent scene-sized render targets;
//! - effect passes may render into named FBOs (`effect.json` `fbos` +
//!   `target`) and re-bind them as texture slots (`bind`);
//! - each layer is then composited into the scene target with its blend mode,
//!   camera-dynamics UV offset (shake + parallax) and fade opacity;
//! - when scene bloom is enabled a threshold → blur chain runs over
//!   quarter/eighth-res buffers (`_rt_4FrameBuffer` / `_rt_8FrameBuffer`)
//!   and is added back onto the scene target.

use anyhow::{anyhow, Context, Result};
use image::RgbaImage;
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::engine::camera_dynamics::{CameraDynamics, CameraFrameDynamics};
use crate::engine::fbo::RenderTargetPool;
use crate::engine::model::{ShaderModel, WEBlending};
use crate::engine::particle;
use crate::engine::scene::{parse_value_bool, parse_value_f32};
use crate::engine::shaders::{effect_def, loader, resolver, transpiler};
use crate::platform::GpuDevice;

const SHADER_SRC: &str = include_str!("shaders/gpu_shaders.wgsl");

/// Per-layer ping-pong buffer key in the FBO pool (each layer's effect chain
/// gets its own pair, sized to that layer's object size).
fn pingpong_key(layer_idx: usize, slot: u8) -> String {
    format!("_rt_pingpong{slot}_{layer_idx}")
}

/// Built-in effects with handwritten WGSL kernels.
const HARDCODED_EFFECTS: &[&str] = &[
    "pulse",
    "scroll",
    "shake",
    "tint",
    "opacity",
    "waterripple",
    "waterwaves",
    "spin",
];

pub struct GpuSceneRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Indexed by `(no_interpolation, clamp_uvs)` — the `.tex` `NoInterpolation`
    /// and `ClampUVs`/`ClampUVsBorder` flags select nearest-vs-linear filtering
    /// and clamp-vs-repeat addressing per layer.
    samplers: [wgpu::Sampler; 4],
    base_bgl: wgpu::BindGroupLayout,
    effect_bgl: wgpu::BindGroupLayout,
    composite_pipelines: Vec<(u32, wgpu::RenderPipeline)>,
    /// Same vs_composite_quad/fs_composite pair as `composite_pipelines[0]`
    /// but with blending disabled (plain overwrite): used for the intra-chain
    /// base material pass, which always renders into a freshly-cleared,
    /// transparent object FBO. Reusing the alpha-blended composite pipeline
    /// there would double-attenuate rgb by alpha (once via the shader's own
    /// `s.rgb * opacity` and again via the GPU blend equation).
    base_pass_pipeline: wgpu::RenderPipeline,
    effect_pipelines: Vec<(&'static str, wgpu::RenderPipeline)>,
    dynamic_pipelines: HashMap<String, wgpu::RenderPipeline>,
    // pipeline key → UBO layout (uniform key, glsl_type, typed default)
    dynamic_uniform_keys: HashMap<String, Vec<transpiler::UniformEntry>>,
    // pipeline key → vertex attributes (glsl type, name); empty = fullscreen
    // triangle with no vertex buffers (synthetic VS path)
    dynamic_vertex_attrs: HashMap<String, Vec<(String, String)>>,
    dynamic_textures: HashMap<String, Vec<wgpu::Texture>>,
    // (kind, dims) → shared quad attribute buffer (kind: 0=pos NDC, 1=uv, 2=zero)
    attr_buffers: HashMap<(u8, u8), wgpu::Buffer>,
    composite_pipeline_layout: wgpu::PipelineLayout,
    effect_pipeline_layout: wgpu::PipelineLayout,
    shader_module: wgpu::ShaderModule,
    dummy_tex: wgpu::Texture,
    // Scene bloom chain
    bloom_threshold_pipeline: wgpu::RenderPipeline,
    bloom_blur_pipeline: wgpu::RenderPipeline,
    bloom_combine_pipeline: wgpu::RenderPipeline,
    // Format → blit pipeline (surface presentation, FBO up/down-sampling)
    blit_pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl GpuSceneRenderer {
    pub fn new(gpu: GpuDevice) -> Result<Self> {
        let GpuDevice { device, queue, .. } = gpu;
        Self::with_device(device, queue)
    }

    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self> {
        let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene_shaders"),
            source: wgpu::ShaderSource::Wgsl(SHADER_SRC.into()),
        });

        let make_sampler = |no_interpolation: bool, clamp: bool| {
            let filter = if no_interpolation {
                wgpu::FilterMode::Nearest
            } else {
                wgpu::FilterMode::Linear
            };
            let address = if clamp {
                wgpu::AddressMode::ClampToEdge
            } else {
                wgpu::AddressMode::Repeat
            };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("scene_sampler"),
                mag_filter: filter,
                min_filter: filter,
                address_mode_u: address,
                address_mode_v: address,
                ..Default::default()
            })
        };
        // Index by (no_interpolation as usize) << 1 | (clamp as usize).
        let samplers = [
            make_sampler(false, false),
            make_sampler(false, true),
            make_sampler(true, false),
            make_sampler(true, true),
        ];

        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                multisampled: false,
                view_dimension: wgpu::TextureViewDimension::D2,
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
            },
            count: None,
        };
        let base_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("base_bgl"),
            entries: &[
                tex_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    // VERTEX too: vs_composite_quad reads the object rect.
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // Extra texture slots for multi-texture effects (g_Texture1..6)
                tex_entry(3),
                tex_entry(4),
                tex_entry(5),
                tex_entry(6),
                tex_entry(7),
                tex_entry(8),
            ],
        });

        let effect_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                // VERTEX too: real WE vertex shaders read the shared UBO
                // (g_ModelViewProjectionMatrix, animation params, ...).
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("composite_layout"),
                bind_group_layouts: &[&base_bgl],
                push_constant_ranges: &[],
            });

        // Normal (colorBlendMode == 0) layers use hardware alpha blending.
        // Any other colorBlendMode is a Photoshop-style blend that needs to
        // read the destination in-shader (see fs_composite_blend), so that
        // pipeline writes its already-blended result straight through.
        let mut composite_pipelines = Vec::new();
        composite_pipelines.push((
            0u32,
            Self::create_pipeline(
                &device,
                &composite_pipeline_layout,
                &shader_module,
                "vs_composite_quad",
                "fs_composite",
                "composite_normal",
                Some(wgpu::BlendState::ALPHA_BLENDING),
                wgpu::TextureFormat::Rgba8Unorm,
            ),
        ));
        composite_pipelines.push((
            1u32,
            Self::create_pipeline(
                &device,
                &composite_pipeline_layout,
                &shader_module,
                "vs_composite_quad",
                "fs_composite_blend",
                "composite_blend",
                Some(wgpu::BlendState::REPLACE),
                wgpu::TextureFormat::Rgba8Unorm,
            ),
        ));
        // Base material pass: same quad/tint shader, no blending — the
        // destination (a layer's own FBO) is always freshly cleared to
        // transparent, so this is a plain "write the tinted pixel" pass, not
        // a composite (see BlendingMode_Normal == GL_ONE,GL_ZERO in CPass.cpp).
        let base_pass_pipeline = Self::create_pipeline(
            &device,
            &composite_pipeline_layout,
            &shader_module,
            "vs_composite_quad",
            "fs_composite",
            "base_material_pass",
            None,
            wgpu::TextureFormat::Rgba8Unorm,
        );

        let effect_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("effect_layout"),
            bind_group_layouts: &[&base_bgl, &effect_bgl],
            push_constant_ranges: &[],
        });

        let effect_entry_points: Vec<(&str, &str)> = HARDCODED_EFFECTS
            .iter()
            .map(|n| {
                (
                    *n,
                    match *n {
                        "pulse" => "fs_pulse",
                        "scroll" => "fs_scroll",
                        "shake" => "fs_shake",
                        "tint" => "fs_tint",
                        "opacity" => "fs_opacity",
                        "waterripple" => "fs_waterripple",
                        "waterwaves" => "fs_waterwaves",
                        "spin" => "fs_spin",
                        _ => "fs_composite",
                    },
                )
            })
            .collect();

        let mut effect_pipelines = Vec::new();
        for (name, entry) in &effect_entry_points {
            let pipeline = Self::create_pipeline(
                &device,
                &effect_layout,
                &shader_module,
                "vs_fullscreen",
                entry,
                name,
                None,
                wgpu::TextureFormat::Rgba8Unorm,
            );
            effect_pipelines.push((*name, pipeline));
        }

        // Bloom chain pipelines (all render into Rgba8Unorm pool targets).
        let bloom_threshold_pipeline = Self::create_pipeline(
            &device,
            &effect_layout,
            &shader_module,
            "vs_fullscreen",
            "fs_bloom_threshold",
            "bloom_threshold",
            None,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let bloom_blur_pipeline = Self::create_pipeline(
            &device,
            &effect_layout,
            &shader_module,
            "vs_fullscreen",
            "fs_blur9",
            "bloom_blur",
            None,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        // fs_bloom_combine now samples the pre-bloom scene copy itself and
        // adds bloom in-shader (matching combine.frag), so this pass is a
        // plain overwrite (WE's `combine.json` pass uses default/"normal"
        // blending, not a GPU-side additive blend).
        let bloom_combine_pipeline = Self::create_pipeline(
            &device,
            &effect_layout,
            &shader_module,
            "vs_fullscreen",
            "fs_bloom_combine",
            "bloom_combine",
            None,
            wgpu::TextureFormat::Rgba8Unorm,
        );

        let dummy_tex = Self::create_white_1x1_texture(&device, &queue);

        Ok(Self {
            device,
            queue,
            samplers,
            base_bgl,
            effect_bgl,
            composite_pipelines,
            base_pass_pipeline,
            effect_pipelines,
            dynamic_pipelines: HashMap::new(),
            dynamic_uniform_keys: HashMap::new(),
            dynamic_vertex_attrs: HashMap::new(),
            dynamic_textures: HashMap::new(),
            attr_buffers: HashMap::new(),
            composite_pipeline_layout,
            effect_pipeline_layout: effect_layout,
            shader_module,
            dummy_tex,
            bloom_threshold_pipeline,
            bloom_blur_pipeline,
            bloom_combine_pipeline,
            blit_pipelines: HashMap::new(),
        })
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn create_white_1x1_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy_white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            tex.as_image_copy(),
            &[255u8, 255, 255, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        tex
    }

    #[allow(clippy::too_many_arguments)]
    fn create_pipeline(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        module: &wgpu::ShaderModule,
        vs_entry: &str,
        fs_entry: &str,
        label: &str,
        blend: Option<wgpu::BlendState>,
        format: wgpu::TextureFormat,
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
                    format,
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

    #[allow(clippy::too_many_arguments)]
    fn create_pipeline_split_modules(
        device: &wgpu::Device,
        layout: &wgpu::PipelineLayout,
        vs_module: &wgpu::ShaderModule,
        vs_entry: &str,
        fs_module: &wgpu::ShaderModule,
        fs_entry: &str,
        label: &str,
        blend: Option<wgpu::BlendState>,
        vertex_layouts: &[wgpu::VertexBufferLayout<'_>],
    ) -> wgpu::RenderPipeline {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module: vs_module,
                entry_point: Some(vs_entry),
                buffers: vertex_layouts,
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

    #[allow(clippy::too_many_arguments)]
    pub fn add_dynamic_pipeline(
        &mut self,
        key: String,
        wgsl: &str,
        vert_wgsl: Option<&str>,
        uniform_keys: Vec<transpiler::UniformEntry>,
        attributes: &[(String, String)],
        blend: Option<wgpu::BlendState>,
    ) -> Result<()> {
        use std::panic::AssertUnwindSafe;
        let wgsl_owned: std::borrow::Cow<'static, str> = wgsl.to_string().into();
        let label = key.clone();
        let device = &self.device;
        let fs_module = std::panic::catch_unwind(AssertUnwindSafe(|| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label.as_str()),
                source: wgpu::ShaderSource::Wgsl(wgsl_owned),
            })
        }))
        .map_err(|e| {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "(unknown)".into());
            anyhow!("shader module panicked for '{key}': {msg}")
        })?;

        // One vertex buffer per attribute; formats follow the GLSL types.
        let attr_descs: Vec<[wgpu::VertexAttribute; 1]> = attributes
            .iter()
            .enumerate()
            .map(|(i, (ty, _))| {
                [wgpu::VertexAttribute {
                    format: vertex_format_for(ty),
                    offset: 0,
                    shader_location: i as u32,
                }]
            })
            .collect();
        let vertex_layouts: Vec<wgpu::VertexBufferLayout> = attr_descs
            .iter()
            .map(|attrs| wgpu::VertexBufferLayout {
                array_stride: attrs[0].format.size(),
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: attrs,
            })
            .collect();

        let device = &self.device;
        let layout = &self.effect_pipeline_layout;
        let base = &self.shader_module;
        let label2 = key.clone();
        let vert_src_owned: Option<String> = vert_wgsl.map(|s| s.to_string());
        let pipeline = std::panic::catch_unwind(AssertUnwindSafe(|| {
            if let Some(ref vert_src) = vert_src_owned {
                let vert_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(&format!("{label2}-vs")),
                    source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(vert_src.as_str())),
                });
                Self::create_pipeline_split_modules(
                    device,
                    layout,
                    &vert_module,
                    "main",
                    &fs_module,
                    "main",
                    &label2,
                    blend,
                    &vertex_layouts,
                )
            } else {
                Self::create_pipeline_split_modules(
                    device,
                    layout,
                    base,
                    "vs_we_effect",
                    &fs_module,
                    "main",
                    &label2,
                    blend,
                    &[],
                )
            }
        }))
        .map_err(|e| {
            let msg = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "(unknown)".into());
            anyhow!("pipeline panicked for '{key}': {msg}")
        })?;

        self.dynamic_pipelines.insert(key.clone(), pipeline);
        self.dynamic_uniform_keys.insert(key.clone(), uniform_keys);
        self.dynamic_vertex_attrs.insert(key, attributes.to_vec());
        Ok(())
    }

    /// Shared quad buffer for one vertex attribute. `kind`: 0 = NDC position,
    /// 1 = UV (v=0 at the vertex that lands on screen top after naga's Y
    /// flip — same convention as the synthetic VS), 2 = zeros.
    fn attr_buffer(&mut self, kind: u8, dims: u8) -> &wgpu::Buffer {
        let device = &self.device;
        self.attr_buffers.entry((kind, dims)).or_insert_with(|| {
            const CORNERS: [[f32; 2]; 6] = [
                [-1.0, -1.0],
                [1.0, -1.0],
                [-1.0, 1.0],
                [1.0, -1.0],
                [1.0, 1.0],
                [-1.0, 1.0],
            ];
            let mut data: Vec<f32> = Vec::with_capacity(6 * dims as usize);
            for [x, y] in CORNERS {
                let full: [f32; 4] = match kind {
                    0 => [x, y, 0.0, 1.0],
                    1 => {
                        let u = (x + 1.0) * 0.5;
                        let v = (y + 1.0) * 0.5;
                        [u, v, u, v]
                    }
                    _ => [0.0; 4],
                };
                data.extend_from_slice(&full[..dims as usize]);
            }
            let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("quad_attr"),
                size: bytes.len() as u64,
                usage: wgpu::BufferUsages::VERTEX,
                mapped_at_creation: true,
            });
            buf.slice(..).get_mapped_range_mut()[..].copy_from_slice(&bytes);
            buf.unmap();
            buf
        })
    }

    /// Resolve the quad buffers for a translated shader's attribute list.
    fn attr_buffers_for(&mut self, attributes: &[(String, String)]) -> Vec<wgpu::Buffer> {
        let specs: Vec<(u8, u8)> = attributes
            .iter()
            .map(|(ty, name)| {
                let dims = match ty.as_str() {
                    "float" => 1,
                    "vec2" => 2,
                    "vec3" => 3,
                    _ => 4,
                };
                let kind = if name.contains("Position") {
                    0
                } else if name.contains("TexCoord") {
                    1
                } else {
                    2
                };
                (kind, dims)
            })
            .collect();
        specs
            .into_iter()
            .map(|(kind, dims)| self.attr_buffer(kind, dims).clone())
            .collect()
    }

    /// Get (or lazily create) the blit pipeline for a given output format.
    /// Used for surface presentation and FBO up/down-sampling.
    pub fn blit_pipeline(&mut self, format: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        if !self.blit_pipelines.contains_key(&format) {
            let p = Self::create_pipeline(
                &self.device,
                &self.composite_pipeline_layout,
                &self.shader_module,
                "vs_fullscreen",
                "fs_blit",
                "blit",
                None,
                format,
            );
            self.blit_pipelines.insert(format, p);
        }
        &self.blit_pipelines[&format]
    }

    pub fn upload_texture(&self, img: &RgbaImage) -> wgpu::Texture {
        let (w, h) = img.dimensions();
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("layer_tex"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            img.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        tex
    }

    pub fn create_render_target(&self, w: u32, h: u32) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render_target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
    }

    fn find_effect_pipeline(&self, key: &str) -> Option<&wgpu::RenderPipeline> {
        self.effect_pipelines
            .iter()
            .find(|(n, _)| *n == key)
            .map(|(_, p)| p)
            .or_else(|| self.dynamic_pipelines.get(key))
    }

    /// Pipeline key: 0 = normal alpha blend, 1 = Photoshop-style dest-read
    /// blend (any nonzero WE colorBlendMode; the mode itself travels in
    /// CompositeParams.mode).
    fn composite_pipeline(&self, blend_mode: u32) -> &wgpu::RenderPipeline {
        let key = if blend_mode == 0 { 0 } else { 1 };
        self.composite_pipelines
            .iter()
            .find(|(m, _)| *m == key)
            .or_else(|| self.composite_pipelines.first())
            .map(|(_, p)| p)
            .unwrap()
    }

    /// Select the sampler matching a `.tex`'s `NoInterpolation`/`ClampUVs`
    /// flags (nearest-vs-linear filtering, clamp-vs-repeat addressing).
    fn sampler_for(&self, no_interpolation: bool, clamp_uvs: bool) -> &wgpu::Sampler {
        let idx = ((no_interpolation as usize) << 1) | (clamp_uvs as usize);
        &self.samplers[idx]
    }

    /// Create the group(0) bind group: source texture, sampler, a uniform
    /// buffer, and up to six extra texture slots (dummy white when absent).
    fn make_base_bind_group(
        &self,
        src_view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        uniform_buf: &wgpu::Buffer,
        extra: &[Option<wgpu::TextureView>],
    ) -> wgpu::BindGroup {
        let dummy_view = self.dummy_tex.create_view(&Default::default());
        let slot = |i: usize| -> &wgpu::TextureView {
            extra.get(i).and_then(|v| v.as_ref()).unwrap_or(&dummy_view)
        };
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.base_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(slot(0)),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(slot(1)),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(slot(2)),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(slot(3)),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(slot(4)),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(slot(5)),
                },
            ],
        })
    }

    fn make_uniform_buffer(&self, data: &[u8], min_size: u64) -> wgpu::Buffer {
        let size = (data.len() as u64).max(min_size);
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut padded = vec![0u8; size as usize];
        padded[..data.len()].copy_from_slice(data);
        self.queue.write_buffer(&buf, 0, &padded);
        buf
    }

    fn make_effect_bind_group(&self, param_buf: &wgpu::Buffer) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.effect_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: param_buf.as_entire_binding(),
            }],
        })
    }

    /// Record one pass drawing `vertex_count` vertices (3 = fullscreen
    /// triangle, 6 = object quad / real-VS quad with vertex buffers).
    #[allow(clippy::too_many_arguments)]
    fn run_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pipeline: &wgpu::RenderPipeline,
        base_bg: &wgpu::BindGroup,
        effect_bg: Option<&wgpu::BindGroup>,
        dst_view: &wgpu::TextureView,
        load: wgpu::LoadOp<wgpu::Color>,
        label: &str,
        vertex_count: u32,
        vertex_buffers: &[wgpu::Buffer],
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: dst_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, base_bg, &[]);
        if let Some(bg) = effect_bg {
            pass.set_bind_group(1, bg, &[]);
        }
        for (i, buf) in vertex_buffers.iter().enumerate() {
            pass.set_vertex_buffer(i as u32, buf.slice(..));
        }
        pass.draw(0..vertex_count, 0..1);
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
                texture: target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(std::iter::once(encoder.finish()));

        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
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

        RgbaImage::from_raw(w, h, pixels).context("creating RgbaImage from GPU readback")
    }
}

// ── Effect runtimes ───────────────────────────────────────────────────────────

fn vertex_format_for(glsl_type: &str) -> wgpu::VertexFormat {
    match glsl_type {
        "float" => wgpu::VertexFormat::Float32,
        "vec2" => wgpu::VertexFormat::Float32x2,
        "vec3" => wgpu::VertexFormat::Float32x3,
        _ => wgpu::VertexFormat::Float32x4,
    }
}

/// Map a material pass's `blending` mode to a GPU blend state. Effect passes
/// always render a fullscreen-covering quad into a just-cleared (transparent)
/// target, so this only changes output for passes whose shader legitimately
/// writes alpha<1 — Normal/Disabled write it through unmodified (replace),
/// while Additive/Translucent premultiply color by that alpha, matching the
/// reference's (ONE,ZERO) / (SRC_ALPHA,ONE) / (SRC_ALPHA,1-SRC_ALPHA) blend
/// functions for a zero destination.
fn wgpu_blend_state(blending: &WEBlending) -> Option<wgpu::BlendState> {
    match blending {
        WEBlending::Normal | WEBlending::Disabled => None,
        WEBlending::Additive => Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        }),
        WEBlending::Translucent => Some(wgpu::BlendState::ALPHA_BLENDING),
    }
}

/// Scene.json overrides for one effect pass: shader combos plus constant
/// values (indexed by material key, with normalized aliases and per-component
/// `_r/_g/_b`/`_x/_y/_z/_w` entries).
#[derive(Debug, Clone, Default)]
struct PassOverride {
    combos: HashMap<String, i32>,
    values: ShaderVals,
    /// Positional scene.json texture override for this pass (e.g. a custom
    /// opacity mask or noise map); `None` entries mean "leave this slot at
    /// the material's own default". See `scene::Pass::textures`.
    textures: Vec<Option<String>>,
}

/// One effect attached to one layer (an *instance* — the same effect on two
/// layers gets separate pipelines because combos/constants can differ).
struct EffectInstanceDef {
    layer_idx: usize,
    name: String,
    /// Verbatim scene.json effect `file` path (e.g. `"effects/clouds/effect.json"`
    /// or a workshop-nested `"effects/workshop/2138904733/foo/effect.json"`) —
    /// used as-is rather than reconstructed from `name`, since custom-effect
    /// bundles don't follow the `effects/{name}/` convention.
    file: String,
    pass_overrides: Vec<PassOverride>,
}

/// One executable pass of an effect.
struct EffectPassRuntime {
    /// Pipeline lookup key: the effect name for hardcoded kernels,
    /// `"fx{instance}:{effect}#{pass}"` for translated WE shaders.
    key: String,
    hardcoded: bool,
    /// Named FBO to render into (skips the ping-pong advance).
    target: Option<String>,
    /// (texture slot index, FBO name) bindings.
    binds: Vec<(u32, String)>,
    /// Resolved constants: material defaults overridden by scene values.
    values: ShaderVals,
    /// Quad attribute buffers for the real-VS path; empty = synthetic VS
    /// (fullscreen triangle, no vertex buffers).
    vertex_buffers: Vec<wgpu::Buffer>,
}

/// A fully loaded effect instance: its passes plus auxiliary FBO declarations.
struct EffectRuntime {
    passes: Vec<EffectPassRuntime>,
    /// (name, downscale divisor) — allocated per instance in the pool.
    fbos: Vec<(String, f32)>,
}

/// Per-frame values for WE's engine-provided uniforms (the counterpart of
/// CPass::setupUniforms in the reference).
#[derive(Debug, Clone, Copy)]
struct EngineUniforms {
    time: f32,
    /// Fraction of the day in [0,1).
    daytime: f32,
    texel_size: [f32; 2],
    pointer: [f32; 2],
    /// g_Texture{N}Resolution: (tex_w, tex_h, real_w, real_h) per slot.
    resolutions: [[f32; 4]; 8],
    color: [f32; 3],
    alpha: f32,
    brightness: f32,
}

impl EngineUniforms {
    /// Look up an engine uniform by its GLSL name. Returns up to 4 components.
    fn get(&self, key: &str) -> Option<[f32; 4]> {
        let v = |a: f32, b: f32, c: f32, d: f32| Some([a, b, c, d]);
        match key {
            "g_Time" | "g_AnimationTime" | "g_FrameTime" => v(self.time, 0.0, 0.0, 0.0),
            "g_Daytime" => v(self.daytime, 0.0, 0.0, 0.0),
            "g_TexelSize" => v(self.texel_size[0], self.texel_size[1], 0.0, 0.0),
            "g_TexelSizeHalf" => v(self.texel_size[0] * 0.5, self.texel_size[1] * 0.5, 0.0, 0.0),
            "g_PointerPosition" | "g_PointerPositionLast" => {
                v(self.pointer[0], self.pointer[1], 0.0, 0.0)
            }
            "g_Color" | "g_CompositeColor" | "g_TintColor" => {
                v(self.color[0], self.color[1], self.color[2], 1.0)
            }
            "g_Color4" => v(self.color[0], self.color[1], self.color[2], self.alpha),
            "g_Alpha" | "g_UserAlpha" | "g_BlendAlpha" | "g_CompositeAlpha" => {
                v(self.alpha, 0.0, 0.0, 0.0)
            }
            "g_Brightness" => v(self.brightness, 0.0, 0.0, 0.0),
            "g_TextureReductionScale" => v(1.0, 1.0, 1.0, 1.0),
            _ => {
                if let Some(rest) = key.strip_prefix("g_Texture") {
                    if let Some(idx) = rest.strip_suffix("Resolution") {
                        let i: usize = idx.parse().ok()?;
                        return self.resolutions.get(i).copied();
                    }
                }
                None
            }
        }
    }
}

/// Scene-level bloom settings (general.bloom / bloomstrength / bloomthreshold).
#[derive(Debug, Clone, Default)]
struct BloomSettings {
    enabled: bool,
    strength: f32,
    threshold: f32,
}

struct SceneLayerGpu {
    frames: Vec<wgpu::Texture>,
    /// Animated puppet runtime — `frames[0]` is re-rasterized from the
    /// posed mesh at `PUPPET_UPDATE_INTERVAL` (skinning + CPU raster costs
    /// ~30ms at 4K, so it runs at a capped rate rather than per frame;
    /// idle-style puppet animations are slow enough not to notice).
    puppet: Option<std::sync::Arc<crate::engine::puppet::PuppetRuntime>>,
    /// Last animation time `frames[0]` was posed at, seconds.
    puppet_posed_at: f32,
    blend_mode: u32,
    alpha: f32,
    color: [f32; 3],
    brightness: f32,
    frame_duration_ms: u32,
    parallax_depth: [f32; 2],
    /// z-rotation in radians (WE `angles.z`).
    angle: f32,
    /// Object quad: (center_ndc.x, center_ndc.y, half_extent_ndc.x, half_extent_ndc.y).
    rect: [f32; 4],
    no_interpolation: bool,
    clamp_uvs: bool,
    /// Unscaled object size in pixels — matches the reference's `CImage::m_size`
    /// (explicit scene.json `size`, falling back to the texture's native
    /// resolution). Effect-chain FBOs (ping-pong + named targets) are sized to
    /// this, not the scene resolution, per `CImage.cpp`/`FBOProvider::create`.
    object_size: (u32, u32),
    /// This layer's position in `scene.visible_objects()` — lets `render()`
    /// interleave with particle systems in true scene z-order.
    order_index: usize,
}

// ── GpuSceneInstance ──────────────────────────────────────────────────────────

/// A loaded scene ready to render frames on the GPU.
///
/// Owns the renderer, layer textures, effect runtimes, the persistent FBO
/// pool, and per-frame camera dynamics. Render either to RGBA (readback
/// paths: preview/testing/SHM fallback) or straight into an external texture
/// view (Wayland surface presentation — no readback).
pub struct GpuSceneInstance {
    renderer: GpuSceneRenderer,
    layers: Vec<SceneLayerGpu>,
    scene_effects: Vec<EffectInstanceDef>,
    /// Parallel to `scene_effects`; None = the instance failed to load.
    effect_runtimes: Vec<Option<EffectRuntime>>,
    fbo_pool: RenderTargetPool,
    target: wgpu::Texture,
    /// Snapshot of `target` before each colorBlendMode>0 layer composites, so
    /// the dest-read Photoshop blend can sample "what's already there"
    /// without reading and writing the same attachment in one pass.
    scene_copy: wgpu::Texture,
    width: u32,
    height: u32,
    clear_color: [f64; 3],
    dynamics: CameraDynamics,
    bloom: BloomSettings,
    /// Each owns independent simulation state; stepped and CPU-rendered into a
    /// scene-sized RGBA buffer every frame, then uploaded and composited like
    /// any other layer (see `render()` step 3). A known simplification: real
    /// particle objects interleave into the scene's actual render order, but
    /// these always draw on top of images (before bloom, so bright particles
    /// still contribute to the bloom pass) — see [[wp_engine_project]] memory.
    particle_systems: Vec<particle::ParticleSystem>,
    /// Parallel to `particle_systems`: each system's resolved sprite (from
    /// its config's `material`, if any), sampled CPU-side by `render_onto`'s
    /// textured path. `None` falls back to flat-color circles.
    particle_sprites: Vec<Option<particle::ParticleSprite>>,
    /// Parallel to `particle_systems`: each system's `order_index` (from its
    /// source `ParticleLayer`), so `render()` can interleave particles with
    /// image layers in true scene z-order.
    particle_order: Vec<usize>,
    /// Parallel to `particle_systems`: true when the resolved sprite's
    /// material declares `"blending":"additive"` (fog/smoke/embers/rain/
    /// lightning — most real particle materials). Composited additively
    /// instead of the default alpha-over, which otherwise makes a sprite's
    /// near-black background visibly darken/box the scene behind it.
    particle_additive: Vec<bool>,
    start: Instant,
    last_time: f32,
    mouse_norm: [f32; 2],
}

impl GpuSceneInstance {
    /// Open a scene from a wallpaper directory with a freshly opened GPU device.
    pub fn open(dir: &Path) -> Result<Self> {
        let gpu = GpuDevice::open_low_power()
            .or_else(|_| GpuDevice::open_best())
            .context("opening GPU device")?;
        Self::with_gpu(gpu, dir)
    }

    /// Open a scene using an existing device/queue (e.g. the one that owns a
    /// Wayland surface, so rendered frames can be presented directly).
    pub fn with_device(device: wgpu::Device, queue: wgpu::Queue, dir: &Path) -> Result<Self> {
        let renderer = GpuSceneRenderer::with_device(device, queue)?;
        Self::build(renderer, dir)
    }

    pub fn with_gpu(gpu: GpuDevice, dir: &Path) -> Result<Self> {
        let renderer = GpuSceneRenderer::new(gpu)?;
        Self::build(renderer, dir)
    }

    fn build(mut renderer: GpuSceneRenderer, dir: &Path) -> Result<Self> {
        let resolved = super::render::ResolvedScene::from_directory(dir)?;
        let w = resolved.width;
        let h = resolved.height;

        let scene_model = crate::engine::model::scene_to_model(&resolved.scene)?;

        let general = resolved.scene.general.as_ref();
        let clear_color: [f64; 3] = general
            .and_then(|g| g.clear_color.as_ref())
            .and_then(|v| {
                let s = if let Some(s) = v.as_str() {
                    s.to_string()
                } else if let Some(inner) = v.get("value").and_then(|i| i.as_str()) {
                    inner.to_string()
                } else {
                    return None;
                };
                let parts: Vec<f64> = s
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .collect();
                if parts.len() >= 3 {
                    Some([parts[0], parts[1], parts[2]])
                } else {
                    None
                }
            })
            .unwrap_or([0.0, 0.0, 0.0]);

        let bloom = BloomSettings {
            enabled: general
                .and_then(|g| g.bloom.as_ref())
                .and_then(parse_value_bool)
                .unwrap_or(false),
            strength: general
                .and_then(|g| g.bloom_strength.as_ref())
                .and_then(parse_value_f32)
                .unwrap_or(1.0),
            threshold: general
                .and_then(|g| g.bloom_threshold.as_ref())
                .and_then(parse_value_f32)
                .unwrap_or(0.5),
        };

        let dynamics = CameraDynamics::from_scene(&resolved.scene);

        let layers: Vec<SceneLayerGpu> = resolved
            .layers
            .iter()
            .map(|l| {
                let all_frames: Vec<&RgbaImage> = std::iter::once(&l.image)
                    .chain(l.extra_frames.iter())
                    .collect();
                // Textures stay at native resolution; the composite quad and
                // sampler handle scaling to the object's on-screen size.
                let frames: Vec<wgpu::Texture> = all_frames
                    .iter()
                    .map(|frame| renderer.upload_texture(frame))
                    .collect();

                // Unscaled object size: explicit scene.json size, else the
                // texture's native resolution (CImage.cpp lines 213-222).
                // True scene-fullscreen layers (models/util/fullscreenlayer.json
                // etc.) never reach here — they're filtered out upstream by
                // `is_special_layer` — so an unspecified size always means
                // "use the texture's own dimensions", never "cover the scene".
                let effective_size = if l.size[0] > 0.0 && l.size[1] > 0.0 {
                    [l.size[0], l.size[1]]
                } else {
                    [l.image.width() as f64, l.image.height() as f64]
                };
                let object_size = (
                    effective_size[0].round() as u32,
                    effective_size[1].round() as u32,
                );

                // Object quad rect in NDC. WE origins are absolute scene
                // coordinates (Y-up, bottom-left origin) pointing at the
                // object's center.
                let size_px = [
                    effective_size[0] * l.scale[0],
                    effective_size[1] * l.scale[1],
                ];
                let rect = [
                    (2.0 * l.origin[0] / w as f64 - 1.0) as f32,
                    (2.0 * l.origin[1] / h as f64 - 1.0) as f32,
                    (size_px[0] / w as f64) as f32,
                    (size_px[1] / h as f64) as f32,
                ];

                SceneLayerGpu {
                    frames,
                    puppet: l.puppet.clone(),
                    puppet_posed_at: 0.0,
                    blend_mode: l.blend_mode,
                    alpha: l.alpha,
                    color: l.color,
                    brightness: l.brightness,
                    frame_duration_ms: l.frame_duration_ms,
                    parallax_depth: [l.parallax_depth[0] as f32, l.parallax_depth[1] as f32],
                    angle: l.angle,
                    rect,
                    object_size,
                    no_interpolation: l.no_interpolation,
                    clamp_uvs: l.clamp_uvs,
                    order_index: l.order_index,
                }
            })
            .collect();

        let scene_effects = collect_effects(&scene_model);
        let effect_runtimes = load_effect_runtimes(&mut renderer, dir, &scene_effects);

        // Allocate all persistent render targets up front so the render loop
        // can look them up immutably. Per-layer effect-chain FBOs (ping-pong
        // pair + named targets) are sized to that layer's own object size,
        // matching the reference (`CImage`'s m_mainFBO/m_subFBO and
        // `FBOProvider::create` both use the object's unscaled size, not the
        // scene resolution).
        let mut fbo_pool = RenderTargetPool::new();
        for (layer_idx, layer) in layers.iter().enumerate() {
            let (ow, oh) = (layer.object_size.0.max(1), layer.object_size.1.max(1));
            fbo_pool.get_or_create(renderer.device(), &pingpong_key(layer_idx, 0), ow, oh);
            fbo_pool.get_or_create(renderer.device(), &pingpong_key(layer_idx, 1), ow, oh);
        }
        for (instance_idx, runtime) in effect_runtimes.iter().enumerate() {
            if let Some(runtime) = runtime {
                // `inst.layer_idx` is a raw scene-object index; find the
                // loaded layer that came from that object (positional
                // indexing would misalign whenever particles/skipped
                // objects precede the layer, sizing FBOs off — and routing
                // effects onto — a completely unrelated layer).
                let (ow, oh) = scene_effects
                    .get(instance_idx)
                    .and_then(|inst| layers.iter().find(|l| l.order_index == inst.layer_idx))
                    .map(|layer| (layer.object_size.0.max(1), layer.object_size.1.max(1)))
                    .unwrap_or((w, h));
                for (fbo_name, scale) in &runtime.fbos {
                    let div = scale.max(1.0);
                    let key = named_fbo_key(instance_idx, fbo_name);
                    fbo_pool.get_or_create(
                        renderer.device(),
                        &key,
                        ((ow as f32 / div) as u32).max(1),
                        ((oh as f32 / div) as u32).max(1),
                    );
                }
            }
        }
        if bloom.enabled {
            // Matches the reference's three-buffer chain exactly (CScene.cpp):
            // no ping-pong needed since each stage writes a *different* buffer.
            let (qw, qh) = ((w / 4).max(1), (h / 4).max(1));
            let (ew, eh) = ((w / 8).max(1), (h / 8).max(1));
            fbo_pool.get_or_create(renderer.device(), "_rt_4FrameBuffer", qw, qh);
            fbo_pool.get_or_create(renderer.device(), "_rt_8FrameBuffer", ew, eh);
            fbo_pool.get_or_create(renderer.device(), "_rt_Bloom", ew, eh);
        }

        let target = renderer.create_render_target(w, h);
        let scene_copy = renderer.create_render_target(w, h);

        let particle_systems: Vec<particle::ParticleSystem> = resolved
            .particle_layers
            .iter()
            .map(|pl| {
                let spawn_center = [pl.origin[0] as f32, h as f32 - pl.origin[1] as f32];
                let mut system = particle::ParticleSystem::from_config(
                    &pl.config,
                    spawn_center,
                    pl.overrides.as_ref(),
                );
                if let Some(sprite) = &pl.sprite_texture {
                    system.set_sprite_frames(sprite.frames.len(), sprite.duration);
                }
                system
            })
            .collect();
        let particle_sprites: Vec<Option<particle::ParticleSprite>> = resolved
            .particle_layers
            .iter()
            .map(|pl| pl.sprite_texture.clone())
            .collect();
        let particle_order: Vec<usize> = resolved
            .particle_layers
            .iter()
            .map(|pl| pl.order_index)
            .collect();
        let particle_additive: Vec<bool> = resolved
            .particle_layers
            .iter()
            .map(|pl| pl.additive_blend)
            .collect();

        Ok(Self {
            renderer,
            layers,
            scene_effects,
            effect_runtimes,
            fbo_pool,
            target,
            scene_copy,
            width: w,
            height: h,
            clear_color,
            dynamics,
            bloom,
            particle_systems,
            particle_sprites,
            particle_order,
            particle_additive,
            start: Instant::now(),
            last_time: 0.0,
            mouse_norm: [0.5, 0.5],
        })
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn device(&self) -> &wgpu::Device {
        self.renderer.device()
    }

    pub fn queue(&self) -> &wgpu::Queue {
        self.renderer.queue()
    }

    /// Feed pointer position in [0,1]² (drives camera parallax).
    pub fn set_mouse(&mut self, norm: [f32; 2]) {
        self.mouse_norm = norm;
    }

    /// Render the next frame into the internal scene target.
    pub fn render(&mut self) {
        let time = self.start.elapsed().as_secs_f32();
        let delta = (time - self.last_time).max(0.0);
        self.last_time = time;
        let dynamics = self.dynamics.update(time, delta, self.mouse_norm);

        let target_view = self.target.create_view(&Default::default());
        let mut encoder =
            self.renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame_encoder"),
                });

        // 1. Clear scene target.
        {
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear_pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: self.clear_color[0],
                            g: self.clear_color[1],
                            b: self.clear_color[2],
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                ..Default::default()
            });
        }

        // Re-pose animated puppet layers at a capped rate: skin the mesh at
        // the current animation time and re-upload frames[0]. CPU skinning +
        // rasterization runs ~30ms at 4K, so this ticks at
        // PUPPET_UPDATE_INTERVAL rather than every frame — the idle-style
        // animations these carry (breathing, hair sway) are far slower than
        // even that.
        const PUPPET_UPDATE_INTERVAL: f32 = 1.0 / 15.0;
        for i in 0..self.layers.len() {
            let Some(runtime) = self.layers[i].puppet.clone() else {
                continue;
            };
            if (time - self.layers[i].puppet_posed_at).abs() < PUPPET_UPDATE_INTERVAL {
                continue;
            }
            let (w, h) = (runtime.atlas.width(), runtime.atlas.height());
            let posed = runtime.render_at(time, w, h);
            let tex = self.renderer.upload_texture(&posed);
            self.layers[i].frames[0] = tex;
            self.layers[i].puppet_posed_at = time;
        }

        // 2/3. Image layers and particle systems, interleaved by
        // `order_index` (true scene z-order, matching the reference's single
        // shared per-object render order) instead of drawing all particles
        // after all images.
        enum DrawItem {
            Image(usize),
            Particle(usize),
        }
        let mut items: Vec<(usize, DrawItem)> = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| (l.order_index, DrawItem::Image(i)))
            .chain(
                self.particle_systems
                    .iter()
                    .enumerate()
                    .map(|(i, _)| (self.particle_order[i], DrawItem::Particle(i))),
            )
            .collect();
        items.sort_by_key(|(order, _)| *order);

        for (_, item) in items {
            match item {
                DrawItem::Image(layer_idx) => {
                    self.draw_image_layer_gpu(&mut encoder, &target_view, layer_idx, time, dynamics);
                }
                DrawItem::Particle(idx) => {
                    self.draw_particle_layer_gpu(&mut encoder, &target_view, idx, delta);
                }
            }
        }

        // 4. Scene bloom chain.
        if self.bloom.enabled {
            self.record_bloom(&mut encoder, &target_view);
        }

        self.renderer.queue.submit(Some(encoder.finish()));
    }

    fn draw_image_layer_gpu(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        layer_idx: usize,
        time: f32,
        dynamics: CameraFrameDynamics,
    ) {
        {
            let layer = &self.layers[layer_idx];
            let frame_idx = if layer.frames.len() > 1 && layer.frame_duration_ms > 0 {
                ((time * 1000.0 / layer.frame_duration_ms as f32) as usize) % layer.frames.len()
            } else {
                0
            };
            let base_tex = &layer.frames[frame_idx];
            let base_view = base_tex.create_view(&Default::default());
            let tex_res = |t: &wgpu::Texture| -> [f32; 4] {
                let (tw, th) = (t.width() as f32, t.height() as f32);
                [tw, th, tw, th]
            };

            // Engine uniforms shared by every pass of this layer.
            let engine_base = EngineUniforms {
                time,
                daytime: daytime_fraction(),
                texel_size: [1.0 / self.width as f32, 1.0 / self.height as f32],
                pointer: self.mouse_norm,
                resolutions: [[
                    self.width as f32,
                    self.height as f32,
                    self.width as f32,
                    self.height as f32,
                ]; 8],
                color: layer.color,
                alpha: layer.alpha,
                brightness: layer.brightness,
            };

            // Ping-pong through this layer's effect passes (own object-sized pair).
            let pp = [
                self.fbo_pool.get(&pingpong_key(layer_idx, 0)).unwrap(),
                self.fbo_pool.get(&pingpong_key(layer_idx, 1)).unwrap(),
            ];
            let layer_sampler = self
                .renderer
                .sampler_for(layer.no_interpolation, layer.clamp_uvs);
            // Effects reference their owner by raw scene-object index
            // (layer.order_index), NOT by position in `self.layers` —
            // positional matching applied effects to whichever layer
            // happened to share the count once particles/skipped objects
            // offset the two spaces.
            let obj_index = layer.order_index;
            let has_effects = self
                .scene_effects
                .iter()
                .any(|inst| inst.layer_idx == obj_index);

            // Base material pass (genericimage3-equivalent): reference always
            // renders `texture * g_Color4` into the object's own FBO *before*
            // any effects run, so effects see the tinted/alpha'd image rather
            // than the raw texture (CImage::setup pushes the material's own
            // passes first, ahead of the effect chain). Skipped when the layer
            // has no effects: with nothing in between, tinting at the final
            // composite step produces an identical result for a fraction of
            // the draw calls.
            let mut cur: Option<usize> = None; // None = still reading base texture
            if has_effects {
                let base_tint = [
                    layer.color[0] * layer.brightness,
                    layer.color[1] * layer.brightness,
                    layer.color[2] * layer.brightness,
                ];
                let base_pass_buf = self.renderer.make_uniform_buffer(
                    &composite_params(
                        layer.alpha,
                        0,
                        [0.0, 0.0],
                        base_tint,
                        0.0,
                        FULLSCREEN_RECT,
                        1.0,
                        [1.0, 1.0],
                    ),
                    64,
                );
                let base_pass_bg = self.renderer.make_base_bind_group(
                    &base_view,
                    layer_sampler,
                    &base_pass_buf,
                    &[],
                );
                self.renderer.run_pass(
                    encoder,
                    &self.renderer.base_pass_pipeline,
                    &base_pass_bg,
                    None,
                    &pp[0].view(),
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    "base_material_pass",
                    6,
                    &[],
                );
                cur = Some(0);
            }
            for (instance_idx, inst) in self
                .scene_effects
                .iter()
                .enumerate()
                .filter(|(_, inst)| inst.layer_idx == obj_index)
            {
                let Some(runtime) = self
                    .effect_runtimes
                    .get(instance_idx)
                    .and_then(|r| r.as_ref())
                else {
                    continue;
                };
                for pass in &runtime.passes {
                    let Some(pipeline) = self.renderer.find_effect_pipeline(&pass.key) else {
                        continue;
                    };

                    // Chain source before bind-0 overrides ("previous" binds
                    // and the default input both refer to it).
                    let (chain_view, chain_res) = match cur {
                        None => (base_view.clone(), tex_res(base_tex)),
                        Some(i) => (pp[i].view(), tex_res(&pp[i].texture)),
                    };

                    let mut engine = engine_base;
                    engine.resolutions[0] = chain_res;

                    // Extra texture slots: shader/material textures first,
                    // FBO binds override.
                    let mut extra: Vec<Option<wgpu::TextureView>> = vec![None; 6];
                    if let Some(texs) = self.renderer.dynamic_textures.get(&pass.key) {
                        for (i, tex) in texs.iter().enumerate().take(6) {
                            extra[i] = Some(tex.create_view(&Default::default()));
                            engine.resolutions[i + 1] = tex_res(tex);
                        }
                    }
                    let mut src_view = chain_view.clone();
                    for (slot, fbo_name) in &pass.binds {
                        // A bind named "previous" is the chain input itself.
                        let (view, res) = if fbo_name == "previous" {
                            (chain_view.clone(), chain_res)
                        } else {
                            let key = named_fbo_key(instance_idx, fbo_name);
                            match self.fbo_pool.get(&key) {
                                Some(rt) => (rt.view(), tex_res(&rt.texture)),
                                None => continue,
                            }
                        };
                        if *slot == 0 {
                            src_view = view;
                            engine.resolutions[0] = res;
                        } else if (*slot as usize) <= 6 {
                            extra[(*slot - 1) as usize] = Some(view);
                            engine.resolutions[*slot as usize] = res;
                        }
                    }

                    let params = if pass.hardcoded {
                        make_effect_params(&inst.name, time, &pass.values)
                    } else {
                        let keys = self
                            .renderer
                            .dynamic_uniform_keys
                            .get(&pass.key)
                            .map(|v| v.as_slice())
                            .unwrap_or(&[]);
                        make_params_from_translated_typed(keys, &engine, &pass.values)
                    };

                    let composite_buf = self
                        .renderer
                        .make_uniform_buffer(&passthrough_composite_params(), 64);
                    let layer_sampler = self
                        .renderer
                        .sampler_for(layer.no_interpolation, layer.clamp_uvs);
                    let base_bg = self.renderer.make_base_bind_group(
                        &src_view,
                        layer_sampler,
                        &composite_buf,
                        &extra,
                    );
                    let param_buf = self.renderer.make_uniform_buffer(&params, 16);
                    let effect_bg = self.renderer.make_effect_bind_group(&param_buf);

                    let next = cur.map(|i| 1 - i).unwrap_or(0);
                    let dst_view = match &pass.target {
                        Some(fbo_name) => {
                            let key = named_fbo_key(instance_idx, fbo_name);
                            match self.fbo_pool.get(&key) {
                                Some(rt) => rt.view(),
                                None => pp[next].view(),
                            }
                        }
                        None => pp[next].view(),
                    };

                    // Real-VS pipelines draw a 6-vertex quad from attribute
                    // buffers; synthetic-VS ones draw the fullscreen triangle.
                    let vertex_count = if pass.vertex_buffers.is_empty() { 3 } else { 6 };

                    self.renderer.run_pass(
                        encoder,
                        pipeline,
                        &base_bg,
                        Some(&effect_bg),
                        &dst_view,
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        "effect_pass",
                        vertex_count,
                        &pass.vertex_buffers,
                    );

                    // Named-target passes render aside; the chain continues
                    // from the same source.
                    if pass.target.is_none() {
                        cur = Some(next);
                    }
                }
            }

            // Composite the layer onto the scene target with camera dynamics.
            let uv_offset = [
                dynamics.shake_offset[0]
                    + dynamics.parallax_displacement[0] * layer.parallax_depth[0],
                dynamics.shake_offset[1]
                    + dynamics.parallax_displacement[1] * layer.parallax_depth[1],
            ];
            // Color/alpha/brightness were already baked in by the base
            // material pass above when this layer has effects; re-applying
            // them here would double them up.
            let (tint, opacity) = if has_effects {
                ([1.0, 1.0, 1.0], dynamics.fade)
            } else {
                (
                    [
                        layer.color[0] * layer.brightness,
                        layer.color[1] * layer.brightness,
                        layer.color[2] * layer.brightness,
                    ],
                    dynamics.fade * layer.alpha,
                )
            };
            let composite_buf = self.renderer.make_uniform_buffer(
                &composite_params(
                    opacity,
                    layer.blend_mode as i32,
                    uv_offset,
                    tint,
                    layer.angle,
                    layer.rect,
                    self.width as f32 / self.height as f32,
                    [self.width as f32, self.height as f32],
                ),
                64,
            );
            let src_view = match cur {
                None => base_view,
                Some(i) => pp[i].view(),
            };
            // Photoshop-style blend modes read the destination in-shader, so
            // snapshot the scene target before drawing over it.
            let extra: Vec<Option<wgpu::TextureView>> = if layer.blend_mode != 0 {
                encoder.copy_texture_to_texture(
                    self.target.as_image_copy(),
                    self.scene_copy.as_image_copy(),
                    wgpu::Extent3d {
                        width: self.width,
                        height: self.height,
                        depth_or_array_layers: 1,
                    },
                );
                vec![Some(self.scene_copy.create_view(&Default::default()))]
            } else {
                vec![]
            };
            let base_bg = self.renderer.make_base_bind_group(
                &src_view,
                layer_sampler,
                &composite_buf,
                &extra,
            );
            let pipeline = self.renderer.composite_pipeline(layer.blend_mode);
            self.renderer.run_pass(
                encoder,
                pipeline,
                &base_bg,
                None,
                target_view,
                wgpu::LoadOp::Load,
                "composite_pass",
                6,
                &[],
            );
        }
    }

    /// Steps and CPU-renders one particle system into a scene-sized RGBA
    /// buffer, uploads it, and composites it like a normal fullscreen layer.
    /// Steps a particle system and, if anything is alive, CPU-rasters +
    /// uploads only its bounding box (`ParticleSystem::bounds()`) instead of
    /// a full scene-sized buffer, compositing it as a positioned quad (the
    /// same `rect`/`CompositeParams` mechanism image layers use) rather than
    /// a fullscreen passthrough. Systems with no living particles are
    /// skipped entirely — no raster, no allocation, no upload, no draw call.
    /// A further possible optimization (not done here): reusing one
    /// persistent GPU texture per system across frames instead of a fresh
    /// `upload_texture` each frame — deferred since safely doing that while
    /// the bbox size changes frame-to-frame needs UV-subregion plumbing the
    /// composite shader doesn't have today.
    fn draw_particle_layer_gpu(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        idx: usize,
        delta: f32,
    ) {
        let system = &mut self.particle_systems[idx];
        system.step(delta);
        let Some((min_x, min_y, max_x, max_y)) = system.bounds() else {
            return;
        };
        let min_x = min_x.max(0.0);
        let min_y = min_y.max(0.0);
        let max_x = max_x.min(self.width as f32);
        let max_y = max_y.min(self.height as f32);
        if max_x <= min_x || max_y <= min_y {
            return;
        }
        let bw = (max_x - min_x).ceil().max(1.0) as u32;
        let bh = (max_y - min_y).ceil().max(1.0) as u32;

        let additive = self.particle_additive.get(idx).copied().unwrap_or(false);
        let mut buf = RgbaImage::new(bw, bh);
        system.render_onto_blended(
            &mut buf,
            self.particle_sprites[idx].as_ref(),
            [min_x, min_y],
            additive,
        );
        let tex = self.renderer.upload_texture(&buf);
        let view = tex.create_view(&Default::default());
        let sampler = self.renderer.sampler_for(false, false);

        // Same NDC `rect` convention as image layers (`gpu_shaders.wgsl`'s
        // `vs_composite_quad`): center + half-extent, Y-up — flip our
        // pixel-space (Y-down) bbox center back into WE's Y-up scene
        // convention first, matching `spawn_center`'s own flip.
        let cx_px = min_x + bw as f32 / 2.0;
        let cy_px = min_y + bh as f32 / 2.0;
        let we_cy = self.height as f32 - cy_px;
        let rect = [
            2.0 * cx_px / self.width as f32 - 1.0,
            2.0 * we_cy / self.height as f32 - 1.0,
            bw as f32 / self.width as f32,
            bh as f32 / self.height as f32,
        ];
        // Additive particle materials (fog/smoke/embers/rain/lightning — the
        // overwhelming majority of real particle materials) need the
        // dest-read blend pipeline. Mode 30 is our pure premultiplied add:
        // the rasterizer above already accumulated `src * src_a` per
        // particle (the reference's GL_SRC_ALPHA/GL_ONE quad blending), so
        // the composite adds the buffer's RGB to the scene as-is. Plain
        // alpha-over (mode 0) otherwise makes a sprite's near-black
        // background visibly darken/box the scene instead of contributing
        // nothing, since particle compositing previously always used mode 0.
        const BLEND_MODE_PARTICLE_ADD: i32 = 30;
        let mode = if additive { BLEND_MODE_PARTICLE_ADD } else { 0 };
        let composite_buf = self.renderer.make_uniform_buffer(
            &composite_params(
                1.0,
                mode,
                [0.0, 0.0],
                [1.0, 1.0, 1.0],
                0.0,
                rect,
                self.width as f32 / self.height as f32,
                [self.width as f32, self.height as f32],
            ),
            64,
        );
        let extra: Vec<Option<wgpu::TextureView>> = if additive {
            encoder.copy_texture_to_texture(
                self.target.as_image_copy(),
                self.scene_copy.as_image_copy(),
                wgpu::Extent3d {
                    width: self.width,
                    height: self.height,
                    depth_or_array_layers: 1,
                },
            );
            vec![Some(self.scene_copy.create_view(&Default::default()))]
        } else {
            vec![]
        };
        let base_bg = self
            .renderer
            .make_base_bind_group(&view, sampler, &composite_buf, &extra);
        self.renderer.run_pass(
            encoder,
            self.renderer.composite_pipeline(mode as u32),
            &base_bg,
            None,
            target_view,
            wgpu::LoadOp::Load,
            "particle_composite",
            6,
            &[],
        );
    }

    /// Bloom: threshold at quarter res, gaussian blur at quarter and eighth
    /// res, then additive combine back onto the scene target.
    /// Reproduces the reference's exact four-pass chain (`downsample_quarter_bloom`
    /// → `downsample_eighth_blur_v` → `blur_h_bloom` → `combine`, all real WE
    /// utility shaders the linux port wires up as a hidden fullscreen effect —
    /// see CScene.cpp/WallpaperApplication.cpp). All texel offsets use the
    /// *scene's* texel size, matching WE's always-scene-relative `g_TexelSize`.
    fn record_bloom(&self, encoder: &mut wgpu::CommandEncoder, target_view: &wgpu::TextureView) {
        let q = self.fbo_pool.get("_rt_4FrameBuffer").unwrap();
        let e = self.fbo_pool.get("_rt_8FrameBuffer").unwrap();
        let bloom_rt = self.fbo_pool.get("_rt_Bloom").unwrap();
        let scene_view = self.target.create_view(&Default::default());
        let scene_texel = [1.0 / self.width as f32, 1.0 / self.height as f32];

        // Snapshot the pre-bloom scene: the final combine pass both reads
        // and writes `self.target`, which a GPU can't do within one pass.
        encoder.copy_texture_to_texture(
            self.target.as_image_copy(),
            self.scene_copy.as_image_copy(),
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        let scene_copy_view = self.scene_copy.create_view(&Default::default());

        let passthrough = self
            .renderer
            .make_uniform_buffer(&passthrough_composite_params(), 64);

        let run = |encoder: &mut wgpu::CommandEncoder,
                   pipeline: &wgpu::RenderPipeline,
                   src: &wgpu::TextureView,
                   extra: &[Option<wgpu::TextureView>],
                   dst: &wgpu::TextureView,
                   params: &[u8]| {
            let default_sampler = self.renderer.sampler_for(false, false);
            let base_bg =
                self.renderer
                    .make_base_bind_group(src, default_sampler, &passthrough, extra);
            let param_buf = self.renderer.make_uniform_buffer(params, 16);
            let effect_bg = self.renderer.make_effect_bind_group(&param_buf);
            self.renderer.run_pass(
                encoder,
                pipeline,
                &base_bg,
                Some(&effect_bg),
                dst,
                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                "bloom",
                3,
                &[],
            );
        };

        let f32s =
            |vals: &[f32]| -> Vec<u8> { vals.iter().flat_map(|f| f.to_le_bytes()).collect() };

        // Pass 1: threshold + 4-tap box downsample, scene → quarter
        // (downsample_quarter_bloom.frag: texel/threshold/strength; tint fixed
        // at white since it isn't exposed as a scene setting).
        run(
            encoder,
            &self.renderer.bloom_threshold_pipeline,
            &scene_view,
            &[],
            &q.view(),
            &f32s(&[
                scene_texel[0],
                scene_texel[1],
                self.bloom.threshold,
                self.bloom.strength,
            ]),
        );
        // Pass 2: 13-tap blur along x (step = scene texel * 8), quarter → eighth.
        run(
            encoder,
            &self.renderer.bloom_blur_pipeline,
            &q.view(),
            &[],
            &e.view(),
            &f32s(&[scene_texel[0] * 8.0, 0.0, 0.0, 0.0]),
        );
        // Pass 3: 13-tap blur along y (step = scene texel * 8), eighth → bloom.
        run(
            encoder,
            &self.renderer.bloom_blur_pipeline,
            &e.view(),
            &[],
            &bloom_rt.view(),
            &f32s(&[0.0, scene_texel[1] * 8.0, 0.0, 0.0]),
        );
        // Pass 4: combine.frag — plain add of the pre-bloom scene and bloom.
        run(
            encoder,
            &self.renderer.bloom_combine_pipeline,
            &bloom_rt.view(),
            &[Some(scene_copy_view)],
            target_view,
            &f32s(&[0.0, 0.0, 0.0, 0.0]),
        );
    }

    /// Render one frame and read it back to CPU RGBA (preview / test / SHM
    /// fallback paths).
    pub fn render_rgba(&mut self) -> Result<RgbaImage> {
        self.render();
        // Temporary debug hook: dump every pooled render target to PNG.
        if std::env::var("WP_DEBUG_DUMP_FBOS").is_ok() {
            for (name, rt) in self.fbo_pool.iter() {
                if let Ok(img) = self.renderer.readback(&rt.texture, rt.width, rt.height) {
                    let safe = name.replace([':', '/'], "_");
                    let _ = img.save(format!("/tmp/fbo_{safe}.png"));
                    let mut a = img.clone();
                    for p in a.pixels_mut() {
                        p.0 = [p.0[3], p.0[3], p.0[3], 255];
                    }
                    let _ = a.save(format!("/tmp/fbo_{safe}_alpha.png"));
                }
            }
        }
        self.renderer
            .readback(&self.target, self.width, self.height)
    }

    /// Render one frame and blit it aspect-fill into an external texture view
    /// (e.g. an acquired Wayland surface frame). No CPU readback happens.
    pub fn render_to_view(
        &mut self,
        view: &wgpu::TextureView,
        view_w: u32,
        view_h: u32,
        format: wgpu::TextureFormat,
    ) {
        self.render();

        // Aspect-fill (cover): sample a centered sub-rect of the scene.
        let scene_aspect = self.width as f32 / self.height as f32;
        let view_aspect = view_w.max(1) as f32 / view_h.max(1) as f32;
        let (uv_scale, uv_offset) = if view_aspect >= scene_aspect {
            let sy = scene_aspect / view_aspect;
            ([1.0f32, sy], [0.0f32, (1.0 - sy) * 0.5])
        } else {
            let sx = view_aspect / scene_aspect;
            ([sx, 1.0f32], [(1.0 - sx) * 0.5, 0.0f32])
        };
        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&uv_scale[0].to_le_bytes());
        params[4..8].copy_from_slice(&uv_scale[1].to_le_bytes());
        params[8..12].copy_from_slice(&uv_offset[0].to_le_bytes());
        params[12..16].copy_from_slice(&uv_offset[1].to_le_bytes());

        let blit_buf = self.renderer.make_uniform_buffer(&params, 16);
        let scene_view = self.target.create_view(&Default::default());
        let default_sampler = self.renderer.sampler_for(false, false);
        let base_bg =
            self.renderer
                .make_base_bind_group(&scene_view, default_sampler, &blit_buf, &[]);

        // Ensure the pipeline exists before borrowing immutably for the pass.
        self.renderer.blit_pipeline(format);
        let pipeline = &self.renderer.blit_pipelines[&format];

        let mut encoder =
            self.renderer
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("present_blit"),
                });
        self.renderer.run_pass(
            &mut encoder,
            pipeline,
            &base_bg,
            None,
            view,
            wgpu::LoadOp::Clear(wgpu::Color::BLACK),
            "present_blit",
            3,
            &[],
        );
        self.renderer
            .queue
            .submit(std::iter::once(encoder.finish()));
    }
}

fn named_fbo_key(instance_idx: usize, fbo_name: &str) -> String {
    format!("fx{instance_idx}:{fbo_name}")
}

/// Fraction of the local day in [0,1) (the reference's g_Daytime).
fn daytime_fraction() -> f32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    ((secs % 86_400) as f32) / 86_400.0
}

#[allow(clippy::too_many_arguments)]
fn composite_params(
    opacity: f32,
    mode: i32,
    uv_offset: [f32; 2],
    color: [f32; 3],
    angle: f32,
    rect: [f32; 4],
    aspect: f32,
    resolution: [f32; 2],
) -> [u8; 64] {
    // WGSL CompositeParams layout: opacity@0, mode@4, uv_offset@8, color@16,
    // angle@28, rect@32, aspect@48, resolution@56 (bytes 52-55 are padding).
    let mut data = [0u8; 64];
    data[0..4].copy_from_slice(&opacity.to_le_bytes());
    data[4..8].copy_from_slice(&mode.to_le_bytes());
    data[8..12].copy_from_slice(&uv_offset[0].to_le_bytes());
    data[12..16].copy_from_slice(&uv_offset[1].to_le_bytes());
    data[16..20].copy_from_slice(&color[0].to_le_bytes());
    data[20..24].copy_from_slice(&color[1].to_le_bytes());
    data[24..28].copy_from_slice(&color[2].to_le_bytes());
    data[28..32].copy_from_slice(&angle.to_le_bytes());
    for (i, v) in rect.iter().enumerate() {
        data[32 + i * 4..36 + i * 4].copy_from_slice(&v.to_le_bytes());
    }
    data[48..52].copy_from_slice(&aspect.to_le_bytes());
    data[56..60].copy_from_slice(&resolution[0].to_le_bytes());
    data[60..64].copy_from_slice(&resolution[1].to_le_bytes());
    data
}

/// Fullscreen rect: centered, full extents.
const FULLSCREEN_RECT: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// Always mode 0 (`fs_composite`, never reads `dest_copy_tex`), so the
/// resolution field is unused — a dummy value is harmless here.
fn passthrough_composite_params() -> [u8; 64] {
    composite_params(
        1.0,
        0,
        [0.0, 0.0],
        [1.0, 1.0, 1.0],
        0.0,
        FULLSCREEN_RECT,
        1.0,
        [1.0, 1.0],
    )
}

/// Load pipelines/textures for every effect instance in the scene: hardcoded
/// kernels get single-pass runtimes, everything else is loaded from the WE
/// assets directory pass-by-pass and translated GLSL→WGSL with the instance's
/// combos baked in.
fn load_effect_runtimes(
    renderer: &mut GpuSceneRenderer,
    wallpaper_dir: &Path,
    scene_effects: &[EffectInstanceDef],
) -> Vec<Option<EffectRuntime>> {
    let assets_dir = loader::find_we_assets_dir();
    // Wallpaper-local effects/materials/shaders (loose files or scene.pkg)
    // take priority over the global Steam assets dir, mirroring the
    // reference's Container mount order (dir, scene.pkg, gifscene.pkg, assets).
    let resolver = resolver::AssetResolver::new(Some(wallpaper_dir), assets_dir);

    // Debug escape hatch: WP_ENGINE_SKIP_EFFECTS=clouds,bloom disables the
    // named effects (useful to bisect a misbehaving shader translation).
    let skip: Vec<String> = std::env::var("WP_ENGINE_SKIP_EFFECTS")
        .map(|v| v.split(',').map(|s| s.trim().to_lowercase()).collect())
        .unwrap_or_default();

    scene_effects
        .iter()
        .enumerate()
        .map(|(instance_idx, inst)| {
            if skip.contains(&inst.name) {
                eprintln!(
                    "[effect] SKIP '{}': disabled via WP_ENGINE_SKIP_EFFECTS",
                    inst.name
                );
                return None;
            }
            load_effect_instance(renderer, &resolver, instance_idx, inst)
        })
        .collect()
}

fn load_effect_instance(
    renderer: &mut GpuSceneRenderer,
    resolver: &resolver::AssetResolver,
    instance_idx: usize,
    inst: &EffectInstanceDef,
) -> Option<EffectRuntime> {
    let effect_name = &inst.name;
    if HARDCODED_EFFECTS.contains(&effect_name.as_str()) {
        return Some(EffectRuntime {
            passes: vec![EffectPassRuntime {
                key: effect_name.clone(),
                hardcoded: true,
                target: None,
                binds: Vec::new(),
                values: inst
                    .pass_overrides
                    .first()
                    .map(|o| o.values.clone())
                    .unwrap_or_default(),
                vertex_buffers: Vec::new(),
            }],
            fbos: Vec::new(),
        });
    }
    let Ok(eff_def) = effect_def::load_effect_by_file(resolver, &inst.file) else {
        eprintln!(
            "[effect] SKIP '{effect_name}': no effect.json at '{}'",
            inst.file
        );
        return None;
    };
    // Bundle directory for this effect (e.g. "effects/clouds" or a
    // workshop-nested "effects/workshop/2138904733/foo"), used to check for
    // bundled materials/shaders before falling back to root-relative paths.
    let effect_dir = inst
        .file
        .rsplit_once('/')
        .map(|(dir, _)| dir)
        .unwrap_or(inst.file.as_str());

    let fbos: Vec<(String, f32)> = eff_def
        .fbos
        .iter()
        .filter_map(|f| {
            f.name
                .as_ref()
                .map(|n| (n.clone(), f.scale.unwrap_or(1.0) as f32))
        })
        .collect();

    let default_override = PassOverride::default();
    let mut passes: Vec<EffectPassRuntime> = Vec::new();
    for (pass_idx, pass) in eff_def.passes.iter().enumerate() {
        let Some(mat_path) = pass.material.as_deref() else {
            continue;
        };
        let Ok(mat_def) = effect_def::load_material_from_effect(resolver, effect_dir, mat_path)
        else {
            eprintln!(
                "[effect] SKIP '{effect_name}' pass {pass_idx}: material '{mat_path}' not found"
            );
            continue;
        };
        let Some(mat_pass) = mat_def.passes.first() else {
            continue;
        };
        let Some(shader_name) = mat_pass.shader.as_deref() else {
            continue;
        };
        let Ok((frag_glsl, vert_glsl)) =
            resolver.load_glsl_shader_for_effect(shader_name, Some(effect_dir))
        else {
            eprintln!(
                "[effect] SKIP '{effect_name}' pass {pass_idx}: shader '{shader_name}' not found"
            );
            continue;
        };
        // Some WE downsample/blur shaders read a fixed-size array varying
        // through a `for` loop (`v_TexCoord[i]`) rather than literal indices;
        // unroll that loop into literal accesses first so the array-varying
        // pass below (which only handles literal indices) can then unroll
        // the declaration too.
        let frag_glsl = transpiler::unroll_simple_for_loops(&frag_glsl);
        let vert_glsl = transpiler::unroll_simple_for_loops(&vert_glsl);
        // WE's downsample/blur/bloom shaders declare literally-indexed array
        // varyings (`v_TexCoord[4]`) that naga's GLSL frontend can't accept
        // as entry-point I/O; unroll them to scalar varyings before anything
        // else touches the source.
        let frag_glsl = transpiler::unroll_array_varyings(&frag_glsl);
        let vert_glsl = transpiler::unroll_array_varyings(&vert_glsl);
        // Some shaders call max/min/clamp with a bare integer literal where a
        // float is expected (e.g. nitro.frag's `max(0, albedo.rgb)`) — legacy
        // NVIDIA compilers implicitly promoted it, naga/shaderc don't.
        let frag_glsl = transpiler::coerce_int_literal_builtin_args(&frag_glsl);
        let vert_glsl = transpiler::coerce_int_literal_builtin_args(&vert_glsl);
        // Some shaders assign a swizzle of a wider vector from `mix()` using
        // the un-swizzled vector as the first arg (e.g. shift_hue.frag's
        // `albedo.rgb = mix(albedo, newAlbedo, mask)` where `albedo` is
        // vec4) — WE's compiler truncates implicitly, naga/shaderc don't.
        let frag_glsl = transpiler::coerce_swizzle_mismatched_mix_arg(&frag_glsl);
        let vert_glsl = transpiler::coerce_swizzle_mismatched_mix_arg(&vert_glsl);

        // Scene.json pass overrides align with effect.json pass indices
        // (the reference's ImageEffectPassOverride).
        let override_ = inst
            .pass_overrides
            .get(pass_idx)
            .unwrap_or(&default_override);

        // Combos: material's, then the scene override's on top, uppercased
        // (the reference uppercases names when emitting #defines).
        let mut combos: HashMap<String, i32> = HashMap::new();
        for (k, v) in &mat_pass.combos {
            combos.insert(k.to_uppercase(), *v);
        }
        for (k, v) in &override_.combos {
            combos.insert(k.to_uppercase(), *v);
        }
        // Texture bindings: the material's own list, with the scene
        // instance's per-pass override applied on top position-by-position
        // (a scene `null` entry means "keep the material's default", not
        // "clear it" — matches the reference's ImageEffectPassOverride).
        let merged_textures: Vec<Option<String>> = {
            let len = mat_pass.textures.len().max(override_.textures.len());
            (0..len)
                .map(|i| {
                    override_
                        .textures
                        .get(i)
                        .and_then(|t| t.clone())
                        .or_else(|| mat_pass.textures.get(i).and_then(|t| t.clone()))
                })
                .collect()
        };
        // Some sampler2D uniforms annotate a `"combo":"NAME"` (e.g. nitro's
        // opacitymask slot) whose real semantics is "enabled whenever a real
        // texture is assigned to this slot" rather than a JSON-declared
        // combo default. Without this, an optional mask texture the scene
        // *does* provide never actually gates anything — the masked effect
        // washes over the whole layer instead of being confined to the mask
        // (e.g. nitro.frag's `#if MASK` block silently compiling out).
        let sampler_combos: Vec<Option<String>> =
            crate::engine::shaders::uniform_meta::parse_uniform_metadata(&frag_glsl)
                .into_iter()
                .filter(|m| m.uniform_type == "sampler2D")
                .map(|m| m.combo)
                .collect();
        for (i, tex) in merged_textures.iter().enumerate() {
            let Some(name) = tex else { continue };
            if name.is_empty() || name.starts_with("_rt_") || name.starts_with("_alias_") {
                continue;
            }
            if let Some(Some(combo_name)) = sampler_combos.get(i) {
                combos.insert(combo_name.to_uppercase(), 1);
            }
        }

        // Constants: material JSON defaults, overridden by scene values.
        let mut values: ShaderVals = ShaderVals::new();
        for (k, v) in &mat_pass.constant_shader_values {
            insert_value_components(&mut values, k, v);
        }
        for (k, v) in &override_.values {
            values.insert(k.clone(), *v);
        }

        let blending_str = mat_pass.blending.as_deref().unwrap_or("normal");
        let model = ShaderModel::from_resolved_glsl(
            shader_name.to_string(),
            frag_glsl,
            combos,
            WEBlending::from_str(blending_str),
        );
        let translated = match transpiler::translate_full(&model, Some(&vert_glsl)) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("[effect] SKIP '{effect_name}' pass {pass_idx}: GLSL→WGSL failed: {e:#}");
                continue;
            }
        };
        let key = format!("fx{instance_idx}:{effect_name}#{pass_idx}");
        // Temporary debug hook: dump translated WGSL per pass.
        if std::env::var("WP_DEBUG_DUMP_WGSL").is_ok() {
            let safe = key.replace([':', '/', '#'], "_");
            let _ = std::fs::write(format!("/tmp/wgsl_{safe}.frag.wgsl"), &translated.wgsl);
            if let Some(v) = &translated.vert_wgsl {
                let _ = std::fs::write(format!("/tmp/wgsl_{safe}.vert.wgsl"), v);
            }
        }
        let blend = wgpu_blend_state(&model.blending);
        let mut pipeline_attrs = translated.attributes.clone();
        let mut added = renderer.add_dynamic_pipeline(
            key.clone(),
            &translated.wgsl,
            translated.vert_wgsl.as_deref(),
            translated.uniform_keys.clone(),
            &pipeline_attrs,
            blend,
        );
        if added.is_err() && !pipeline_attrs.is_empty() {
            // Real-VS pipeline failed (usually an interface mismatch the
            // preprocessor couldn't reconcile) — retry with the synthetic VS.
            eprintln!(
                "[effect] '{effect_name}' pass {pass_idx}: real VS failed ({}), synthetic fallback",
                added.as_ref().err().map(|e| e.to_string()).unwrap_or_default()
            );
            if let Ok(fallback) = transpiler::translate(&model) {
                pipeline_attrs = Vec::new();
                added = renderer.add_dynamic_pipeline(
                    key.clone(),
                    &fallback.wgsl,
                    fallback.vert_wgsl.as_deref(),
                    fallback.uniform_keys.clone(),
                    &[],
                    blend,
                );
            }
        }
        if let Err(e) = added {
            eprintln!("[effect] SKIP '{effect_name}' pass {pass_idx}: pipeline failed: {e}");
            continue;
        }
        let vertex_buffers = renderer.attr_buffers_for(&pipeline_attrs);

        // Secondary textures (g_Texture1..): shader-annotation defaults,
        // overridden by the material pass's texture list (with the scene
        // instance's own override merged in above). Slot indices must stay
        // aligned, so failed loads push a white 1×1 stand-in instead of
        // shifting later slots down.
        let mut texture_names: Vec<Option<String>> = model
            .texture_slots
            .iter()
            .skip(1)
            .map(|slot| slot.default_path.clone())
            .collect();
        for (i, tex) in merged_textures.iter().enumerate().skip(1) {
            let slot = i - 1;
            if slot >= texture_names.len() {
                texture_names.resize(slot + 1, None);
            }
            if let Some(name) = tex {
                // _rt_* names are framebuffer refs resolved via binds, not files.
                if !name.starts_with("_rt_") && !name.starts_with("_alias_") {
                    texture_names[slot] = Some(name.clone());
                }
            }
        }
        let mut textures: Vec<wgpu::Texture> = Vec::new();
        for name in &texture_names {
            let path = name.as_deref().unwrap_or("");
            // WE texture names resolve under materials/ first
            // (e.g. "util/clouds_256" → materials/util/clouds_256.tex).
            let candidates = [format!("materials/{path}.tex"), format!("{path}.tex")];
            let img = candidates.iter().find_map(|rel| {
                let bytes = resolver.read(rel)?;
                crate::engine::tex::TexFile::parse(&bytes)
                    .ok()?
                    .to_rgba()
                    .ok()
            });
            match img {
                Some(img) => textures.push(renderer.upload_texture(&img)),
                None => {
                    if !path.is_empty() {
                        eprintln!(
                            "[effect] '{effect_name}': texture '{path}' not found — using white"
                        );
                    }
                    textures.push(renderer.upload_texture(&RgbaImage::from_pixel(
                        1,
                        1,
                        image::Rgba([255, 255, 255, 255]),
                    )));
                }
            }
        }
        renderer.dynamic_textures.insert(key.clone(), textures);

        passes.push(EffectPassRuntime {
            key,
            hardcoded: false,
            target: pass.target.clone(),
            binds: pass
                .bind
                .iter()
                .filter_map(|b| b.name.as_ref().map(|n| (b.index.unwrap_or(0), n.clone())))
                .collect(),
            values,
            vertex_buffers,
        });
    }

    if passes.is_empty() {
        eprintln!("[effect] SKIP '{effect_name}': no usable passes");
        return None;
    }
    eprintln!("[effect] LOADED '{effect_name}' ({} passes)", passes.len());
    Some(EffectRuntime { passes, fbos })
}

/// Continuously render scene frames into a channel (preview window, headless
/// tests, and the CPU/SHM fallback path).
pub fn gpu_scene_render_loop(
    dir: &std::path::Path,
    tx: &SyncSender<Arc<RgbaImage>>,
    target_fps: f64,
) -> Result<()> {
    let mut instance = GpuSceneInstance::open(dir)?;
    let frame_duration = Duration::from_secs_f64(1.0 / target_fps);
    let start = Instant::now();

    loop {
        let frame = instance.render_rgba()?;
        if tx.send(Arc::new(frame)).is_err() {
            return Ok(());
        }

        let elapsed = start.elapsed();
        let next = Duration::from_secs_f64(
            (elapsed.as_secs_f64() / frame_duration.as_secs_f64()).ceil()
                * frame_duration.as_secs_f64(),
        );
        if next > elapsed {
            std::thread::sleep(next - elapsed);
        }
    }
}

type ShaderVals = std::collections::HashMap<String, f32>;

/// Insert a constant under every spelling the packer might look up: the raw
/// key, its normalized alias (scene constants use
/// "ui_editor_properties_color_start" while asset shaders annotate materials
/// as "colorstart"), and per-component suffixes for vector values.
fn insert_value_components(map: &mut ShaderVals, key: &str, value: &serde_json::Value) {
    let alias = |k: &str| -> String {
        k.strip_prefix("ui_editor_properties_")
            .unwrap_or(k)
            .replace('_', "")
            .to_lowercase()
    };
    let animated = crate::engine::model::json_to_animated(value);
    insert_animated_components(map, key, &alias(key), &animated);
}

fn insert_animated_components(
    map: &mut ShaderVals,
    key: &str,
    alias: &str,
    v: &crate::engine::model::AnimatedValue,
) {
    if let Some(f) = v.as_float() {
        map.insert(key.to_string(), f);
        map.insert(alias.to_string(), f);
    }
    if let Some([r, g, b]) = v.as_vec3() {
        for (name, val) in [("r", r), ("g", g), ("b", b)] {
            map.insert(format!("{key}_{name}"), val);
            map.insert(format!("{alias}_{name}"), val);
        }
        // Vec2/vec4 lookups use _x/_y/_z/_w suffixes.
        for (name, val) in [("x", r), ("y", g), ("z", b), ("w", 1.0)] {
            map.insert(format!("{key}_{name}"), val);
            map.insert(format!("{alias}_{name}"), val);
        }
    }
}

fn collect_effects(scene: &crate::engine::model::SceneModel) -> Vec<EffectInstanceDef> {
    let alias = |k: &str| -> String {
        k.strip_prefix("ui_editor_properties_")
            .unwrap_or(k)
            .replace('_', "")
            .to_lowercase()
    };
    let mut result = Vec::new();
    for (i, obj) in scene.objects.iter().enumerate() {
        if !obj.is_visible() {
            continue;
        }
        for eff in &obj.effects {
            if !eff.visible {
                continue;
            }
            let file = match &eff.file {
                Some(s) => s,
                None => continue,
            };
            // WE effect paths look like "effects/{name}/effect.json"; the effect
            // name is the directory, not the filename (which is always "effect.json").
            let mut parts = file.rsplitn(3, '/');
            let _filename = parts.next().unwrap_or(""); // "effect.json"
            let name = parts
                .next()
                .unwrap_or(file) // "waterflow"
                .trim_end_matches(".json")
                .to_lowercase();

            // Scene.json pass entries align with effect.json pass indices.
            let pass_overrides: Vec<PassOverride> = eff
                .passes
                .iter()
                .map(|p| {
                    let mut values = ShaderVals::new();
                    for (k, v) in &p.shader_values {
                        insert_animated_components(&mut values, k, &alias(k), v);
                    }
                    PassOverride {
                        combos: p.combos.clone(),
                        values,
                        textures: p.textures.clone(),
                    }
                })
                .collect();

            result.push(EffectInstanceDef {
                layer_idx: i,
                name,
                file: file.clone(),
                pass_overrides,
            });
        }
    }
    result
}

/// Pack effect uniform values using std140 layout rules — the transpiled
/// UBO (GLSL uniform block → SPIR-V → WGSL) uses std140 offsets, so each
/// member is aligned to its own alignment, NOT padded to 16 bytes:
/// float/int align 4, vec2 align 8, vec3/vec4/matrix columns align 16.
///
/// Value resolution per uniform (mirrors CPass::setupUniforms +
/// setupShaderVariables): engine-provided uniforms first, then material/scene
/// constants, then the shader annotation's default.
fn make_params_from_translated_typed(
    keys: &[transpiler::UniformEntry],
    engine: &EngineUniforms,
    vals: &ShaderVals,
) -> Vec<u8> {
    let mut bytes: Vec<u8> = Vec::new();

    // Resolve one uniform into up to 4 components.
    let resolve = |key: &str, default: &[f32; 4]| -> [f32; 4] {
        if let Some(v) = engine.get(key) {
            return v;
        }
        let mut out = *default;
        if let Some(f) = vals.get(key) {
            out = [*f; 4];
        }
        // Per-component spellings win over the scalar broadcast.
        for (i, suffixes) in [
            &["_x", "_r"][..],
            &["_y", "_g"][..],
            &["_z", "_b"][..],
            &["_w", "_a"][..],
        ]
        .iter()
        .enumerate()
        {
            for s in suffixes.iter() {
                if let Some(f) = vals.get(&format!("{key}{s}")) {
                    out[i] = *f;
                    break;
                }
            }
        }
        out
    };

    fn align_to(bytes: &mut Vec<u8>, align: usize) {
        let rem = bytes.len() % align;
        if rem != 0 {
            bytes.resize(bytes.len() + (align - rem), 0);
        }
    }
    fn push_f32(bytes: &mut Vec<u8>, v: f32) {
        bytes.extend_from_slice(&v.to_le_bytes());
    }

    for entry in keys {
        let key = entry.key.as_str();
        match entry.glsl_type.as_str() {
            "vec2" => {
                let v = resolve(key, &entry.default);
                align_to(&mut bytes, 8);
                push_f32(&mut bytes, v[0]);
                push_f32(&mut bytes, v[1]);
            }
            "vec3" => {
                let v = resolve(key, &entry.default);
                align_to(&mut bytes, 16);
                push_f32(&mut bytes, v[0]);
                push_f32(&mut bytes, v[1]);
                push_f32(&mut bytes, v[2]);
                // std140 vec3 has size 12; a following float packs into the
                // tail 4 bytes, so do NOT pad here.
            }
            "vec4" => {
                let v = resolve(key, &entry.default);
                align_to(&mut bytes, 16);
                for f in v {
                    push_f32(&mut bytes, f);
                }
            }
            "mat4" => {
                align_to(&mut bytes, 16);
                let identity: [f32; 16] = [
                    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
                ];
                for f in identity {
                    push_f32(&mut bytes, f);
                }
            }
            "mat3" => {
                // std140 mat3: 3 columns × (vec3 + 4 bytes pad) = 48 bytes
                align_to(&mut bytes, 16);
                let cols: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                for col in cols {
                    for f in col {
                        push_f32(&mut bytes, f);
                    }
                    bytes.extend_from_slice(&[0u8; 4]);
                }
            }
            _ => {
                // float / int / bool / unknown: 4 bytes, align 4
                let v = resolve(key, &entry.default);
                align_to(&mut bytes, 4);
                push_f32(&mut bytes, v[0]);
            }
        }
    }

    // Uniform buffers must be non-empty and sized to a 16-byte multiple.
    if bytes.is_empty() {
        bytes.resize(16, 0);
    }
    align_to(&mut bytes, 16);
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
            0.0,
            0.0,
            0.0,
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
            0.0,
            0.0,
            0.0,
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
            0.0,
            0.0,
            0.0,
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
                0.0,
                0.0,
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
