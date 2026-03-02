use image::RgbaImage;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// A live source of RGBA frames for the Wayland renderer.
///
/// Frames are received from Wallpaper Engine via Unix socket IPC.
/// The bounded channel (size 2) provides natural back-pressure so the
/// producer never runs more than two frames ahead of the display.
pub enum FrameSource {
    /// A static image (no animation).
    Static(Arc<RgbaImage>),
    /// Live frames received from the hook DLL via Unix socket IPC.
    Ipc {
        /// The most recently received frame.
        current: Arc<RgbaImage>,
        /// Receive end of the bounded channel from the IPC reader thread.
        rx: Receiver<Arc<RgbaImage>>,
    },
}

impl FrameSource {
    /// Build a static frame source from an already-loaded image.
    pub fn from_image(img: Arc<RgbaImage>) -> Self {
        FrameSource::Static(img)
    }

    /// Build a live IPC frame source.
    ///
    /// `first` is the first frame (received synchronously before this call so
    /// the wallpaper surface is never blank), `rx` receives subsequent frames.
    pub fn from_ipc(first: Arc<RgbaImage>, rx: Receiver<Arc<RgbaImage>>) -> Self {
        FrameSource::Ipc { current: first, rx }
    }

    /// The most recently available frame.
    #[inline]
    pub fn current_frame(&self) -> &Arc<RgbaImage> {
        match self {
            FrameSource::Static(img) => img,
            FrameSource::Ipc { current, .. } => current,
        }
    }

    /// Advance to the next frame if one is available.
    /// Returns `true` if the frame changed (caller should redraw).
    pub fn try_advance(&mut self) -> bool {
        match self {
            FrameSource::Static(_) => false,
            FrameSource::Ipc { current, rx } => {
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
        matches!(self, FrameSource::Ipc { .. })
    }
}
