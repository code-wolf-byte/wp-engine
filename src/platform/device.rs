use anyhow::{anyhow, Result};
use wgpu::{Device, DeviceDescriptor, Queue, RequestAdapterOptions};

use super::{gpu, GpuAdapter};

// ── GpuDevice ─────────────────────────────────────────────────────────────────

/// An opened GPU device ready to submit work.
///
/// Wraps the three objects you always need together:
/// - `info`   – PAL metadata about the chosen adapter
/// - `device` – records command buffers, allocates resources
/// - `queue`  – submits those command buffers to the GPU
///
/// Create one with [`GpuDevice::open_best`].  Dropping it releases all GPU
/// resources owned by this handle.
pub struct GpuDevice {
    /// Metadata about the chosen adapter (backend, name, discrete/integrated).
    pub info: GpuAdapter,
    /// wgpu device handle — use this to create buffers, textures, pipelines.
    pub device: Device,
    /// wgpu queue handle — use this to submit command encoders.
    pub queue: Queue,
    /// The instance the adapter came from — needed to create presentation surfaces.
    pub instance: wgpu::Instance,
    /// The chosen adapter — needed for surface capability queries.
    pub adapter: wgpu::Adapter,
}

impl GpuDevice {
    /// Select and open the highest-performance GPU adapter available.
    ///
    /// wgpu's `HighPerformance` preference picks a discrete GPU when one is
    /// present, falling back to an integrated GPU, then a software renderer.
    /// Returns `Err` only if no adapter at all could be found/opened.
    pub fn open_best() -> Result<Self> {
        pollster::block_on(Self::open_async(wgpu::PowerPreference::HighPerformance))
    }

    /// Select and open the lowest-power adapter (integrated / iGPU).
    ///
    /// Useful for a low-priority background process like a wallpaper renderer
    /// that shouldn't compete with foreground 3-D applications for the dGPU.
    pub fn open_low_power() -> Result<Self> {
        pollster::block_on(Self::open_async(wgpu::PowerPreference::LowPower))
    }

    // ── internal ─────────────────────────────────────────────────────────────

    async fn open_async(power: wgpu::PowerPreference) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // wgpu 27: request_adapter returns Result<Adapter, _>, not Option.
        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: power,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|e| anyhow!("no GPU adapter found: {e}"))?;

        let info = gpu::from_wgpu_info(adapter.get_info());

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor::default())
            .await
            .map_err(|e| anyhow!("failed to open GPU device '{}': {e}", info.name))?;

        Ok(Self {
            info,
            device,
            queue,
            instance,
            adapter,
        })
    }
}
