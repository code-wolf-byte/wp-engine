// macOS display backend stub for wp-engine.
//
// Architecture notes for when this is fully implemented:
//
// - wgpu automatically selects the Metal backend on macOS — no Metal-specific
//   crates are needed. WGSL shaders are compiled to MSL by naga inside wgpu.
//
// - winit creates a borderless window and positions it at desktop level:
//     NSWindow.setLevel(kCGDesktopWindowLevel)   // sits behind all app windows
//     NSWindow.collectionBehavior = .canJoinAllSpaces | .stationary
//   This makes it behave like the native wallpaper layer.
//
// - Unlike the Linux/Wayland path, there is NO GPU→CPU readback per frame.
//   Instead: GPU renders → wgpu Surface (Metal swapchain) → surface_texture.present()
//   This eliminates the costly RGBA readback + re-upload roundtrip.
//
// Required additions to Cargo.toml when implementing:
//   winit = "0.30"
//   objc2 = "0.5"
//   objc2-app-kit = "0.2"

#[cfg(target_os = "macos")]
pub mod macos {
    use anyhow::Result;

    pub struct MacOSDisplay {
        // winit::window::Window goes here once winit is added
        _window: (),
        // wgpu::SurfaceConfiguration goes here
        _surface_config: (),
    }

    impl MacOSDisplay {
        pub fn new() -> Result<Self> {
            anyhow::bail!(
                "macOS display not yet implemented — \
                 add winit = \"0.30\", objc2, objc2-app-kit to Cargo.toml"
            )
        }

        pub fn present_frame(&self, _frame: std::sync::Arc<image::RgbaImage>) -> Result<()> {
            Ok(())
        }

        pub fn dimensions(&self) -> (u32, u32) {
            (1920, 1080)
        }

        pub fn required_wgpu_features() -> wgpu::Features {
            wgpu::Features::empty()
        }
    }
}
