use std::fmt;

#[derive(Debug)]
pub struct PlatformInfo {
    pub os: &'static str,
    pub gpu_backend: &'static str,
    pub shader_compiler: &'static str,
    pub display_protocol: &'static str,
}

impl PlatformInfo {
    pub fn detect() -> Self {
        #[cfg(target_os = "linux")]
        {
            return PlatformInfo {
                os: "linux",
                gpu_backend: "vulkan",
                shader_compiler: "shaderc",
                display_protocol: "wayland",
            };
        }
        #[cfg(target_os = "macos")]
        {
            return PlatformInfo {
                os: "macos",
                gpu_backend: "metal",
                shader_compiler: "shaderc",
                display_protocol: "quartz",
            };
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        PlatformInfo {
            os: "unknown",
            gpu_backend: "unknown",
            shader_compiler: "shaderc",
            display_protocol: "unknown",
        }
    }
}

impl fmt::Display for PlatformInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "wp-engine | OS: {} | GPU: {} | Shaders: {} | Display: {}",
            self.os, self.gpu_backend, self.shader_compiler, self.display_protocol
        )
    }
}

pub fn log_platform() {
    tracing::info!(target: "platform", "{}", PlatformInfo::detect());
}
