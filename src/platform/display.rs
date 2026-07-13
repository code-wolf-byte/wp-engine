use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::render::{RenderSettings, WallpaperContent};

// ── Private inner trait ───────────────────────────────────────────────────────

pub(crate) trait WallpaperHandleInner: Send {
    fn stop(self: Box<Self>);
    fn wait(self: Box<Self>);
}

// ── Public abstract handle ────────────────────────────────────────────────────

/// Platform-agnostic handle to a running wallpaper renderer.
///
/// Drop or call `stop()` to remove the wallpaper.
pub struct WallpaperHandle {
    inner: Box<dyn WallpaperHandleInner>,
}

impl WallpaperHandle {
    pub(crate) fn new(
        inner: Box<dyn WallpaperHandleInner>,
        _settings: Arc<Mutex<RenderSettings>>,
    ) -> Self {
        Self { inner }
    }

    /// Stop the renderer and wait for its thread to exit.
    pub fn stop(self) {
        self.inner.stop();
    }

    /// Block the calling thread until the renderer exits (e.g. on Ctrl-C).
    pub fn wait(self) {
        self.inner.wait();
    }
}

// ── Platform trait ────────────────────────────────────────────────────────────

pub trait DisplayPlatform {
    /// Spawn a wallpaper renderer for `content` on every output.
    ///
    /// The platform decides how to render: scene wallpapers draw directly
    /// into GPU surfaces when the compositor allows it; other content (and
    /// fallback paths) go through CPU frames + SHM buffers.
    fn spawn_wallpaper(
        &self,
        content: WallpaperContent,
        settings: Arc<Mutex<RenderSettings>>,
    ) -> Result<WallpaperHandle>;
}

// ── Runtime detection ─────────────────────────────────────────────────────────

/// Detect the current display platform at runtime and return a boxed implementation.
///
/// On Linux, checks `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` and returns a
/// `WaylandPlatform`. Returns an error if no supported platform is found.
pub fn detect_platform() -> Box<dyn DisplayPlatform> {
    if std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("WAYLAND_SOCKET").is_ok() {
        tracing::debug!(target: "platform", "detected Wayland display platform");
        return Box::new(super::wayland::WaylandPlatform);
    }
    tracing::error!(target: "platform", "no supported display platform detected (WAYLAND_DISPLAY/WAYLAND_SOCKET unset)");
    eprintln!(
        "error: no supported display platform detected.\n\
         wp-engine requires a Wayland compositor with wlr-layer-shell support.\n\
         (WAYLAND_DISPLAY and WAYLAND_SOCKET are both unset)\n\
         \n\
         Supported compositors: Sway, Hyprland, river, labwc, wayfire, etc.\n\
         X11 is not yet supported."
    );
    std::process::exit(1);
}
