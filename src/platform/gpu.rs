//! GPU adapter enumeration via Linux DRM sysfs.
//!
//! Reads `/sys/class/drm/card*/device/{vendor,device,class}` to enumerate
//! available GPUs without requiring any GPU driver library dependency.

use super::{BackendType, GpuAdapter, GpuDeviceType};

/// Enumerate GPU adapters by reading Linux DRM sysfs entries.
pub(super) fn enumerate_adapters() -> Vec<GpuAdapter> {
    let mut adapters = Vec::new();

    let drm_dir = std::path::Path::new("/sys/class/drm");
    let entries = match std::fs::read_dir(drm_dir) {
        Ok(e) => e,
        Err(_) => return adapters,
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Only top-level card entries (card0, card1, …), not render nodes
        if !name_str.starts_with("card") || name_str.contains('-') {
            continue;
        }

        let device_dir = entry.path().join("device");
        let vendor_id = read_hex_id(&device_dir.join("vendor"));
        let device_id = read_hex_id(&device_dir.join("device"));
        let class = read_hex_id(&device_dir.join("class"));

        // PCI class 0x03xxxx = Display controller
        let device_type = match vendor_id {
            _ if class >> 16 != 0x03 => GpuDeviceType::Unknown,
            0x10de => GpuDeviceType::Discrete, // NVIDIA
            0x1002 => GpuDeviceType::Discrete, // AMD (assume discrete; could be APU)
            0x8086 => GpuDeviceType::Integrated, // Intel
            _ => GpuDeviceType::Unknown,
        };

        let name = read_driver_name(&device_dir)
            .unwrap_or_else(|| format!("GPU {:04x}:{:04x}", vendor_id, device_id));

        adapters.push(GpuAdapter {
            name,
            backend: BackendType::Vulkan, // most likely on Linux
            device_type,
            device_id,
            vendor_id,
        });
    }

    adapters
}

fn read_hex_id(path: &std::path::Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok())
        .unwrap_or(0)
}

fn read_driver_name(device_dir: &std::path::Path) -> Option<String> {
    // Try uevent for a human-readable name
    let uevent = std::fs::read_to_string(device_dir.join("uevent")).ok()?;
    for line in uevent.lines() {
        if let Some(val) = line.strip_prefix("DRIVER=") {
            return Some(val.to_owned());
        }
    }
    None
}
