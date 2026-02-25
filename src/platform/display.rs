use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::render::{FrameSource, RenderSettings};

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
    /// Live render settings shared with the wallpaper thread.
    pub settings: Arc<Mutex<RenderSettings>>,
}

impl WallpaperHandle {
    pub(crate) fn new(
        inner: Box<dyn WallpaperHandleInner>,
        settings: Arc<Mutex<RenderSettings>>,
    ) -> Self {
        Self { inner, settings }
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
    fn spawn_wallpaper(
        &self,
        frame_source: FrameSource,
        settings: Arc<Mutex<RenderSettings>>,
    ) -> Result<WallpaperHandle>;
}

// ── Runtime detection ─────────────────────────────────────────────────────────

/// Detect the current display platform at runtime and return a boxed implementation.
///
/// On Linux, checks `WAYLAND_DISPLAY` or `WAYLAND_SOCKET` and returns a
/// `WaylandPlatform`. Panics with a clear message if no supported platform is found.
pub fn detect_platform() -> Box<dyn DisplayPlatform> {
    if std::env::var("WAYLAND_DISPLAY").is_ok() || std::env::var("WAYLAND_SOCKET").is_ok() {
        return Box::new(super::wayland::WaylandPlatform);
    }
    panic!(
        "no supported display platform detected \
         (WAYLAND_DISPLAY and WAYLAND_SOCKET are both unset)"
    );
}
