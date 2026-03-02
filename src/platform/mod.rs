pub mod display;
pub mod gpu;
pub mod wayland;

pub use display::{detect_platform, WallpaperHandle};

// ── Render quality ────────────────────────────────────────────────────────────

/// Controls render quality hint passed to Wallpaper Engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderQuality {
    Ultra,
    High,
    Medium,
    Low,
}

impl RenderQuality {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ultra  => "Ultra",
            Self::High   => "High",
            Self::Medium => "Medium",
            Self::Low    => "Low",
        }
    }

    pub const ALL: [Self; 4] = [Self::Ultra, Self::High, Self::Medium, Self::Low];
}

// ── Backend ───────────────────────────────────────────────────────────────────

/// Graphics API backend reported by the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendType {
    Vulkan,
    Metal,
    Dx12,
    OpenGl,
    Unknown,
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackendType::Vulkan  => write!(f, "Vulkan"),
            BackendType::Metal   => write!(f, "Metal"),
            BackendType::Dx12    => write!(f, "D3D12"),
            BackendType::OpenGl  => write!(f, "OpenGL"),
            BackendType::Unknown => write!(f, "Unknown"),
        }
    }
}

// ── Device type ───────────────────────────────────────────────────────────────

/// Physical category of a GPU adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuDeviceType {
    Discrete,
    Integrated,
    Virtual,
    Software,
    Unknown,
}

impl std::fmt::Display for GpuDeviceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuDeviceType::Discrete   => write!(f, "Discrete"),
            GpuDeviceType::Integrated => write!(f, "Integrated"),
            GpuDeviceType::Virtual    => write!(f, "Virtual"),
            GpuDeviceType::Software   => write!(f, "Software"),
            GpuDeviceType::Unknown    => write!(f, "Unknown"),
        }
    }
}

// ── Adapter info ──────────────────────────────────────────────────────────────

/// Information about a GPU adapter visible to the process.
#[derive(Debug, Clone)]
pub struct GpuAdapter {
    pub name: String,
    pub backend: BackendType,
    pub device_type: GpuDeviceType,
    pub device_id: u32,
    pub vendor_id: u32,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Enumerate every GPU adapter available on this system (for the probe command).
pub fn probe_adapters() -> Vec<GpuAdapter> {
    gpu::enumerate_adapters()
}
