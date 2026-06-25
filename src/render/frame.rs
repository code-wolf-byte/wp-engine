use anyhow::{anyhow, Result};
use image::RgbaImage;
use std::sync::mpsc::{sync_channel, Receiver};
use std::sync::Arc;
use std::thread;

use crate::engine::ResolvedScene;
use super::content::WallpaperContent;

/// A live source of RGBA frames for the Wayland renderer.
///
/// `Static` always returns the same frame. `Video` owns a background decoder
/// thread that streams decoded frames through a bounded channel (size 2),
/// providing natural back-pressure so the decoder never runs more than two
/// frames ahead of the display.
pub enum FrameSource {
    Static(Arc<RgbaImage>),
    Video {
        /// The most recently decoded frame available to draw.
        current: Arc<RgbaImage>,
        /// Receive end of the bounded channel from the decoder thread.
        rx: Receiver<Arc<RgbaImage>>,
        fps: f64,
    },
}

impl FrameSource {
    /// Build a `FrameSource` from parsed `WallpaperContent`.
    ///
    /// For video this blocks until the first frame is decoded so the wallpaper
    /// surface is never shown blank.
    pub fn from_content(content: WallpaperContent) -> Result<Self> {
        match content {
            WallpaperContent::Static(img) => Ok(FrameSource::Static(img)),
            WallpaperContent::Scene { dir } => {
                let resolved = ResolvedScene::from_directory(&dir)?;
                let img = resolved.render();
                Ok(FrameSource::Static(Arc::new(img)))
            }
            WallpaperContent::Video { path, fps } => {
                let (tx, rx) = sync_channel::<Arc<RgbaImage>>(2);

                thread::spawn(move || {
                    if let Err(e) = super::ffmpeg::video_decode_loop(&path, &tx) {
                        eprintln!("video decoder error for '{}': {e}", path.display());
                    }
                });

                // Wait for the very first frame before returning so callers
                // can draw immediately without a blank flash.
                let first = rx
                    .recv()
                    .map_err(|_| anyhow!("video decoder exited before producing any frames"))?;

                Ok(FrameSource::Video { current: first, rx, fps })
            }
        }
    }

    /// The most recently decoded frame. `Arc::clone` by the caller is cheap
    /// (atomic refcount bump — no pixel data is copied).
    #[inline]
    pub fn current_frame(&self) -> &Arc<RgbaImage> {
        match self {
            FrameSource::Static(img) => img,
            FrameSource::Video { current, .. } => current,
        }
    }

    /// Advance to the next decoded frame if one is available.
    /// Returns `true` if the frame changed (caller should redraw).
    ///
    /// Takes exactly one frame per call so the 8 ms poll timer in the Wayland
    /// loop consumes frames at the same rate the PTS-paced decoder produces
    /// them.
    pub fn try_advance(&mut self) -> bool {
        match self {
            FrameSource::Static(_) => false,
            FrameSource::Video { current, rx, .. } => {
                if let Ok(frame) = rx.try_recv() {
                    *current = frame;
                    true
                } else {
                    false
                }
            }
        }
    }

    /// `true` for animated sources that require a per-frame timer.
    #[inline]
    pub fn is_animated(&self) -> bool {
        matches!(self, FrameSource::Video { .. })
    }

    /// Native frame rate. Returns `0.0` for static sources.
    #[inline]
    pub fn fps(&self) -> f64 {
        match self {
            FrameSource::Static(_) => 0.0,
            FrameSource::Video { fps, .. } => *fps,
        }
    }
}
