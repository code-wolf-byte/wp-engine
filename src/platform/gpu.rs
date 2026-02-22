use wgpu::{Backends, Instance, InstanceDescriptor};

use super::{BackendType, GpuAdapter, GpuDeviceType};

// ── Shared conversion helper ──────────────────────────────────────────────────

/// Convert a raw wgpu `AdapterInfo` into the PAL's backend-agnostic `GpuAdapter`.
/// Kept in this file because all wgpu-specific logic lives here.
pub(super) fn from_wgpu_info(info: wgpu::AdapterInfo) -> GpuAdapter {
    let backend = match info.backend {
        wgpu::Backend::Vulkan => BackendType::Vulkan,
        wgpu::Backend::Metal  => BackendType::Metal,
        wgpu::Backend::Dx12   => BackendType::Dx12,
        wgpu::Backend::Gl     => BackendType::OpenGl,
        _                     => BackendType::Unknown,
    };

    let device_type = match info.device_type {
        wgpu::DeviceType::DiscreteGpu   => GpuDeviceType::Discrete,
        wgpu::DeviceType::IntegratedGpu => GpuDeviceType::Integrated,
        wgpu::DeviceType::VirtualGpu    => GpuDeviceType::Virtual,
        wgpu::DeviceType::Cpu           => GpuDeviceType::Software,
        _                               => GpuDeviceType::Unknown,
    };

    GpuAdapter {
        name: info.name,
        backend,
        device_type,
        device_id: info.device,
        vendor_id: info.vendor,
    }
}

// ── Adapter enumeration ───────────────────────────────────────────────────────

/// Enumerate all GPU adapters using wgpu.
///
/// Synchronous and non-blocking — only queries driver metadata, no device
/// is created and no GPU work is performed.
pub(super) fn enumerate_adapters() -> Vec<GpuAdapter> {
    let instance = Instance::new(&InstanceDescriptor {
        backends: Backends::all(),
        ..Default::default()
    });

    instance
        .enumerate_adapters(Backends::all())
        .into_iter()
        .map(|a: wgpu::Adapter| from_wgpu_info(a.get_info()))
        .collect()
}
