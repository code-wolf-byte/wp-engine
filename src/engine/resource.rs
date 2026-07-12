//! Central GPU resource manager.
//!
//! [`ResourceManager`] owns the wgpu device/queue references and caches all
//! GPU-side textures. Every other module that needs a texture should obtain it
//! through this manager rather than uploading its own copy.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::engine::pkg::Package;
use crate::engine::shaders::loader;
use crate::engine::tex::TexFile;

pub struct ResourceManager {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    sampler: wgpu::Sampler,
    dummy_texture: Arc<wgpu::Texture>,
    texture_cache: Mutex<HashMap<String, Arc<wgpu::Texture>>>,
}

impl ResourceManager {
    /// Create a new `ResourceManager` owning the given device and queue.
    /// Creates a bilinear-repeat sampler and a 1×1 white dummy texture.
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rm_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let dummy_texture = Arc::new(Self::create_white_1x1(&device, &queue));
        Self {
            device,
            queue,
            sampler,
            dummy_texture,
            texture_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Return a cached texture or load it on demand.
    ///
    /// Resolution order:
    /// 1. In-memory cache.
    /// 2. `pkg`, if provided — decoded via [`TexFile`].
    /// 3. WE global assets directory (via [`loader::find_we_assets_dir`]).
    pub fn get_or_load(&self, key: &str, pkg: Option<&Package>) -> Result<Arc<wgpu::Texture>> {
        {
            let cache = self.texture_cache.lock().unwrap();
            if let Some(tex) = cache.get(key) {
                return Ok(Arc::clone(tex));
            }
        }

        if let Some(package) = pkg {
            if let Some(bytes) = package.get(key) {
                let img = TexFile::parse(bytes)
                    .with_context(|| format!("parsing tex from pkg: {key}"))?
                    .to_rgba()
                    .with_context(|| format!("decoding RGBA from pkg tex: {key}"))?;
                let tex = Arc::new(self.upload_rgba(&img));
                self.texture_cache
                    .lock()
                    .unwrap()
                    .insert(key.to_owned(), Arc::clone(&tex));
                return Ok(tex);
            }
        }

        let tex = Arc::new(
            self.load_from_disk(key)
                .with_context(|| format!("loading texture from disk: {key}"))?,
        );
        self.texture_cache
            .lock()
            .unwrap()
            .insert(key.to_owned(), Arc::clone(&tex));
        Ok(tex)
    }

    /// Store a pre-built texture under `key`.
    pub fn store(&self, key: String, tex: Arc<wgpu::Texture>) {
        self.texture_cache.lock().unwrap().insert(key, tex);
    }

    /// Upload an [`RgbaImage`] to the GPU. Not cached — use [`store`] if caching is needed.
    pub fn upload_rgba(&self, img: &RgbaImage) -> wgpu::Texture {
        let (w, h) = img.dimensions();
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("rm_rgba_tex"),
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

    /// Upload multiple animation frames. Returns Vec in frame order. Not cached.
    pub fn upload_frames(&self, frames: Vec<RgbaImage>) -> Vec<Arc<wgpu::Texture>> {
        frames
            .iter()
            .map(|img| Arc::new(self.upload_rgba(img)))
            .collect()
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    pub fn sampler(&self) -> &wgpu::Sampler {
        &self.sampler
    }

    /// 1×1 opaque-white texture for unbound texture slots.
    pub fn dummy_texture(&self) -> Arc<wgpu::Texture> {
        Arc::clone(&self.dummy_texture)
    }

    fn create_white_1x1(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::Texture {
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("dummy_white_1x1"),
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

    fn load_from_disk(&self, key: &str) -> Result<wgpu::Texture> {
        let assets_dir = loader::find_we_assets_dir()
            .with_context(|| format!("WE assets dir not found while loading '{key}'"))?;
        let candidates = [assets_dir.join(key), assets_dir.join(format!("{key}.tex"))];
        let bytes = candidates
            .iter()
            .find_map(|p| std::fs::read(p).ok())
            .with_context(|| format!("texture '{key}' not found in WE assets dir"))?;
        let img = TexFile::parse(&bytes)
            .with_context(|| format!("parsing tex '{key}'"))?
            .to_rgba()
            .with_context(|| format!("decoding RGBA '{key}'"))?;
        Ok(self.upload_rgba(&img))
    }
}
