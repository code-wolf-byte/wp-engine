//! Application lifecycle (the Rust counterpart of the C++
//! `WallpaperApplication`).
//!
//! Owns the flow from configuration to a running wallpaper:
//! resolve content → install property overrides → spawn the platform
//! renderer → block until a stop request (SIGINT/SIGTERM or [`stop`]).

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::platform::{self, WallpaperHandle};
use crate::render::WallpaperContent;

pub mod application_context;
pub use application_context::ApplicationContext;

/// Set by the signal handler; polled by [`WallpaperApplication::show`].
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_stop_signal(_sig: libc::c_int) {
    STOP_REQUESTED.store(true, Ordering::SeqCst);
}

pub struct WallpaperApplication {
    context: ApplicationContext,
    handle: Option<WallpaperHandle>,
}

impl WallpaperApplication {
    /// Prepare the application: installs the context's property overrides so
    /// every scene loaded afterwards resolves user settings against them.
    pub fn new(context: ApplicationContext) -> Self {
        crate::engine::properties::set_global_overrides(context.properties.clone());
        Self {
            context,
            handle: None,
        }
    }

    pub fn context(&self) -> &ApplicationContext {
        &self.context
    }

    /// Load the configured background and start rendering on all outputs.
    pub fn setup(&mut self) -> Result<()> {
        let content = WallpaperContent::from_any_path(&self.context.background)
            .with_context(|| format!("loading wallpaper {}", self.context.background.display()))?;
        let handle =
            platform::detect_platform().spawn_wallpaper(content, self.context.settings.clone())?;
        self.handle = Some(handle);
        Ok(())
    }

    /// Run the wallpaper until SIGINT/SIGTERM (or [`Self::stop`] from another
    /// handle) is received, then shut the renderer down cleanly.
    ///
    /// Calls [`Self::setup`] first when it hasn't run yet.
    pub fn show(&mut self) -> Result<()> {
        if self.handle.is_none() {
            self.setup()?;
        }

        STOP_REQUESTED.store(false, Ordering::SeqCst);
        let handler =
            handle_stop_signal as extern "C" fn(libc::c_int) as *const () as libc::sighandler_t;
        unsafe {
            libc::signal(libc::SIGINT, handler);
            libc::signal(libc::SIGTERM, handler);
        }

        while !STOP_REQUESTED.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }

        self.cleanup();
        Ok(())
    }

    /// Request the render loop to end (usable from signal-safe contexts).
    pub fn stop() {
        STOP_REQUESTED.store(true, Ordering::SeqCst);
    }

    /// Stop the platform renderer and wait for its thread to exit.
    pub fn cleanup(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.stop();
        }
    }
}

impl Drop for WallpaperApplication {
    fn drop(&mut self) {
        self.cleanup();
    }
}
