use anyhow::{anyhow, Result};
use image::{imageops::FilterType, RgbaImage};

use super::{GpuDevice, RenderQuality};

// ── WGSL compute shader ───────────────────────────────────────────────────────
//
// Bilinear-samples the source RGBA texture and writes ARGB8888 u32 values to
// the output storage buffer.  On little-endian hosts the packed u32
// 0xAA_RR_GG_BB maps to bytes [B, G, R, A] in memory — exactly what Wayland
// expects for wl_shm::Format::Argb8888.

const SHADER: &str = r#"
struct Params { dst_w: u32, dst_h: u32 }

@group(0) @binding(0) var src_tex  : texture_2d<f32>;
@group(0) @binding(1) var src_samp : sampler;
@group(0) @binding(2) var<storage, read_write> dst : array<u32>;
@group(0) @binding(3) var<uniform> params : Params;

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.dst_w || gid.y >= params.dst_h { return; }
    let uv = (vec2<f32>(gid.xy) + 0.5)
             / vec2<f32>(f32(params.dst_w), f32(params.dst_h));
    let c  = textureSampleLevel(src_tex, src_samp, uv, 0.0);
    let r  = u32(clamp(c.r, 0.0, 1.0) * 255.0 + 0.5);
    let g  = u32(clamp(c.g, 0.0, 1.0) * 255.0 + 0.5);
    let b  = u32(clamp(c.b, 0.0, 1.0) * 255.0 + 0.5);
    // ARGB8888 LE: bytes [B, G, R, A] → packed u32 = 0xFF_RR_GG_BB
    dst[gid.y * params.dst_w + gid.x] = (255u << 24u) | (r << 16u) | (g << 8u) | b;
}
"#;

// ── GpuScaler ─────────────────────────────────────────────────────────────────

/// GPU-accelerated image scaler.
///
/// Consumes a [`GpuDevice`] and compiles the compute pipeline once.  Every
/// call to [`scale`][Self::scale] uploads the source frame, dispatches the
/// shader, and reads back ARGB8888 bytes ready to copy into a Wayland SHM
/// buffer.  Falls back to a CPU Lanczos3 path on any GPU error.
pub struct GpuScaler {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    sampler: wgpu::Sampler,
    bgl: wgpu::BindGroupLayout,
}

impl GpuScaler {
    /// Consume a [`GpuDevice`], compile the pipeline, and return a ready scaler.
    pub fn from_device(gpu: GpuDevice) -> Result<Self> {
        let GpuDevice { device, queue, .. } = gpu;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scaler_shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scaler_bgl"),
            entries: &[
                // 0 — source texture
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                // 1 — bilinear sampler
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // 2 — output storage buffer
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // 3 — params uniform (dst_w, dst_h)
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
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
            label: Some("scaler_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("scaler_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("scaler_sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        Ok(Self {
            device,
            queue,
            pipeline,
            sampler,
            bgl,
        })
    }

    /// Scale `src` to `(dst_w, dst_h)`, returning ARGB8888 bytes.
    ///
    /// Applies an optional CPU pre-downsample according to `quality.source_scale()`,
    /// then runs the GPU compute shader.  Falls back to a CPU Lanczos3 path on
    /// any GPU error.
    pub fn scale(
        &self,
        src: &RgbaImage,
        dst_w: u32,
        dst_h: u32,
        quality: RenderQuality,
    ) -> Vec<u8> {
        let scale = quality.source_scale();
        let downsampled: Option<RgbaImage> = if scale < 1.0 {
            let sw = ((src.width() as f32) * scale).max(1.0) as u32;
            let sh = ((src.height() as f32) * scale).max(1.0) as u32;
            Some(image::imageops::resize(src, sw, sh, FilterType::Nearest))
        } else {
            None
        };
        let upload_src: &RgbaImage = downsampled.as_ref().unwrap_or(src);

        match self.scale_gpu(upload_src, dst_w, dst_h) {
            Ok(pixels) => pixels,
            Err(e) => {
                tracing::warn!(target: "scaler", "GPU scale failed, falling back to CPU: {e}");
                self.scale_cpu(src, dst_w, dst_h)
            }
        }
    }

    // ── GPU path ──────────────────────────────────────────────────────────────

    fn scale_gpu(&self, src: &RgbaImage, dst_w: u32, dst_h: u32) -> Result<Vec<u8>> {
        let src_w = src.width();
        let src_h = src.height();

        // Upload source as Rgba8Unorm texture.
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scaler_src_tex"),
            size: wgpu::Extent3d {
                width: src_w,
                height: src_h,
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
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            src.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(src_w * 4),
                rows_per_image: Some(src_h),
            },
            wgpu::Extent3d {
                width: src_w,
                height: src_h,
                depth_or_array_layers: 1,
            },
        );

        let tex_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Output storage buffer (4 bytes / pixel as packed u32).
        let output_size = (dst_w * dst_h * 4) as u64;
        let output_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scaler_output"),
            size: output_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        // Staging buffer for CPU readback.
        let staging_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scaler_staging"),
            size: output_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Params uniform: [dst_w, dst_h] packed as two LE u32s.
        let mut params_bytes = [0u8; 8];
        params_bytes[..4].copy_from_slice(&dst_w.to_le_bytes());
        params_bytes[4..].copy_from_slice(&dst_h.to_le_bytes());
        let params_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scaler_params"),
            size: 8,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&params_buf, 0, &params_bytes);

        // Bind group.
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scaler_bg"),
            layout: &self.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&tex_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: output_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buf.as_entire_binding(),
                },
            ],
        });

        // Encode compute pass + copy.
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("scaler_encoder"),
            });

        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("scaler_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            let wx = (dst_w + 15) / 16;
            let wy = (dst_h + 15) / 16;
            pass.dispatch_workgroups(wx, wy, 1);
        }

        encoder.copy_buffer_to_buffer(&output_buf, 0, &staging_buf, 0, output_size);
        self.queue.submit(std::iter::once(encoder.finish()));

        // Readback.
        let slice = staging_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::sync_channel::<Result<(), wgpu::BufferAsyncError>>(1);
        slice.map_async(wgpu::MapMode::Read, move |res| {
            let _ = tx.send(res);
        });
        let _ = self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv()
            .map_err(|_| anyhow!("GPU readback channel disconnected"))?
            .map_err(|e| anyhow!("GPU buffer map failed: {e:?}"))?;

        let data = slice.get_mapped_range();
        let pixels = data.to_vec();
        drop(data);
        staging_buf.unmap();

        Ok(pixels)
    }

    // ── CPU fallback ──────────────────────────────────────────────────────────

    fn scale_cpu(&self, src: &RgbaImage, dst_w: u32, dst_h: u32) -> Vec<u8> {
        let scaled = image::imageops::resize(src, dst_w, dst_h, FilterType::Lanczos3);
        let mut out = Vec::with_capacity((dst_w * dst_h * 4) as usize);
        for chunk in scaled.chunks_exact(4) {
            out.push(chunk[2]); // B
            out.push(chunk[1]); // G
            out.push(chunk[0]); // R
            out.push(chunk[3]); // A
        }
        out
    }
}
