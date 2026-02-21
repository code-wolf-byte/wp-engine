use anyhow::{anyhow, Result};
use image::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;

use crate::workshop::{Wallpaper, WallpaperType};

// ── WallpaperContent ──────────────────────────────────────────────────────────

/// The resolved content of a wallpaper, ready to hand to the renderer.
///
/// This is the primary extension point as new wallpaper types are supported:
/// add a new variant here, implement it in `from_wallpaper`/`from_path`, and
/// add a corresponding `FrameSource` variant below.
pub enum WallpaperContent {
    /// A pre-loaded static RGBA image (PNG, JPEG, …).
    Static(Arc<RgbaImage>),
    /// A video file to be decoded frame-by-frame with FFmpeg.
    Video { path: PathBuf, fps: f64 },
    // Future variants (not yet implemented):
    // Scene { pkg: PathBuf },
    // Web   { html: PathBuf },
    // Application { exe: PathBuf },
}

impl WallpaperContent {
    /// Parse a Workshop `Wallpaper` into the appropriate content variant.
    ///
    /// Returns `Err` for types that are not yet renderable (Scene, Web,
    /// Application) so callers get a clear diagnostic instead of a generic
    /// image-loading failure.
    pub fn from_wallpaper(w: &Wallpaper) -> Result<Self> {
        match w.wallpaper_type() {
            WallpaperType::Scene => Err(anyhow!(
                "scene wallpapers require the proprietary Wallpaper Engine renderer"
            )),
            WallpaperType::Web => Err(anyhow!("web wallpapers are not yet supported")),
            WallpaperType::Application => {
                Err(anyhow!("application wallpapers are Windows-only"))
            }
            WallpaperType::Video | WallpaperType::Unknown => {
                let path = w
                    .wallpaper_file()
                    .ok_or_else(|| anyhow!("wallpaper has no file field in project.json"))?;
                if !path.exists() {
                    return Err(anyhow!("wallpaper file not found: {}", path.display()));
                }
                Self::from_path(&path)
            }
        }
    }

    /// Detect content type from a raw file path, guessing by extension.
    ///
    /// Video extensions are decoded with FFmpeg; everything else is opened
    /// by the `image` crate as a static image.
    pub fn from_path(path: &Path) -> Result<Self> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match ext.as_str() {
            "mp4" | "webm" | "mkv" | "avi" | "mov" | "flv" | "wmv" => {
                let fps = probe_fps(path)?;
                Ok(WallpaperContent::Video { path: path.to_owned(), fps })
            }
            _ => {
                let img = image::open(path)
                    .map_err(|e| anyhow!("failed to load image {}: {}", path.display(), e))?
                    .into_rgba8();
                Ok(WallpaperContent::Static(Arc::new(img)))
            }
        }
    }
}

// ── FrameSource ───────────────────────────────────────────────────────────────

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
    /// surface is never shown blank. This is acceptable because the caller
    /// (`spawn_wallpaper`) already blocks waiting for its Wayland LoopSignal.
    pub fn from_content(content: WallpaperContent) -> Result<Self> {
        match content {
            WallpaperContent::Static(img) => Ok(FrameSource::Static(img)),
            WallpaperContent::Video { path, fps } => {
                let (tx, rx) = sync_channel::<Arc<RgbaImage>>(2);

                thread::spawn(move || {
                    if let Err(e) = video_decode_loop_inner(&path, &tx) {
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
    /// them — no more draining two slots in one tick and playing at 2× speed.
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

// ── FFmpeg helpers ─────────────────────────────────────────────────────────────

/// Open the container and read the video stream's average frame rate.
/// Does not decode any frames.
pub fn probe_fps(path: &Path) -> Result<f64> {
    ffmpeg_next::init().map_err(|e| anyhow!("FFmpeg init: {e}"))?;

    let ctx = ffmpeg_next::format::input(&path)
        .map_err(|e| anyhow!("cannot open '{}': {e}", path.display()))?;

    let stream = ctx
        .streams()
        .best(ffmpeg_next::media::Type::Video)
        .ok_or_else(|| anyhow!("no video stream found in '{}'", path.display()))?;

    let rate = stream.avg_frame_rate();
    let fps = rate.numerator() as f64 / rate.denominator() as f64;

    if !fps.is_finite() || fps <= 0.0 {
        return Err(anyhow!(
            "could not determine FPS for '{}' (reported {}/{})",
            path.display(),
            rate.numerator(),
            rate.denominator(),
        ));
    }

    Ok(fps)
}

/// Decode every frame of the video file and send each as an `Arc<RgbaImage>`
/// through `tx`. When the file ends the decoder loops back to the beginning
/// for seamless repeat. Exits when `tx.send()` returns `Err` (receiver dropped).
fn video_decode_loop_inner(
    path: &Path,
    tx: &SyncSender<Arc<RgbaImage>>,
) -> Result<()> {
    use ffmpeg_next::{
        codec::context::Context as CodecCtx,
        format,
        format::Pixel,
        media::Type,
        software::scaling::{context::Context as ScaleCtx, flag::Flags},
        util::frame::video::Video as VideoFrame,
    };
    use std::time::{Duration, Instant};

    ffmpeg_next::init().map_err(|e| anyhow!("FFmpeg init: {e}"))?;

    // Outer loop: re-open the file on each iteration to loop the video.
    loop {
        let mut ictx = format::input(&path)
            .map_err(|e| anyhow!("cannot open '{}': {e}", path.display()))?;

        let stream_idx = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow!("no video stream in '{}'", path.display()))?
            .index();

        // Capture the stream's time base (num/den) before borrowing ictx for
        // decoding — Rational is Copy so this is free.
        let time_base = ictx.stream(stream_idx).unwrap().time_base();

        // Build the codec context and video decoder.
        let mut decoder = {
            let stream = ictx.stream(stream_idx).unwrap();
            CodecCtx::from_parameters(stream.parameters())
                .map_err(|e| anyhow!("codec context: {e}"))?
                .decoder()
                .video()
                .map_err(|e| anyhow!("video decoder: {e}"))?
        };

        // Build a software scaler: decoded pixel format → packed RGBA8.
        // The output dimensions match the input so scaling is handled later
        // in `draw_at` (which scales to each monitor's resolution).
        let mut scaler = ScaleCtx::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGBA,
            decoder.width(),
            decoder.height(),
            Flags::BILINEAR,
        )
        .map_err(|e| anyhow!("scaler context: {e}"))?;

        let mut raw = VideoFrame::empty();
        let mut scaled = VideoFrame::empty();

        // PTS state — reset on every loop-back so playback restarts correctly.
        let mut first_pts: Option<i64> = None;
        let mut loop_start: Option<Instant> = None;

        // Inner packet loop.
        for (stream, packet) in ictx.packets() {
            if stream.index() != stream_idx {
                continue;
            }
            if decoder.send_packet(&packet).is_err() {
                continue;
            }

            while decoder.receive_frame(&mut raw).is_ok() {
                // ── PTS-based real-time pacing ────────────────────────────
                // Sleep until this frame's presentation time has elapsed so
                // the decoder sends frames at the video's native speed rather
                // than as fast as the CPU allows.
                if let Some(pts) = raw.pts() {
                    let tb_num = time_base.numerator() as f64;
                    let tb_den = time_base.denominator() as f64;

                    if tb_den > 0.0 {
                        let first  = *first_pts.get_or_insert(pts);
                        let start  = *loop_start.get_or_insert_with(Instant::now);

                        let frame_offset = Duration::from_secs_f64(
                            ((pts - first) as f64 * tb_num / tb_den).max(0.0),
                        );

                        if let Some(wait) = frame_offset.checked_sub(start.elapsed()) {
                            thread::sleep(wait);
                        }
                    }
                }

                if scaler.run(&raw, &mut scaled).is_err() {
                    continue;
                }

                if let Some(img) = frame_to_rgba(&scaled) {
                    // `send` blocks when the channel is full (back-pressure).
                    // It returns `Err` when the receiver is dropped → exit.
                    if tx.send(Arc::new(img)).is_err() {
                        return Ok(());
                    }
                }
            }
        }

        // Flush any frames buffered inside the decoder.
        let _ = decoder.send_eof();
        while decoder.receive_frame(&mut raw).is_ok() {
            if scaler.run(&raw, &mut scaled).is_err() {
                continue;
            }
            if let Some(img) = frame_to_rgba(&scaled) {
                if tx.send(Arc::new(img)).is_err() {
                    return Ok(());
                }
            }
        }

        // File ended — the outer `loop` re-opens it for seamless repeat.
    }
}

/// Copy one RGBA `VideoFrame` into an `RgbaImage`, trimming stride padding.
fn frame_to_rgba(frame: &ffmpeg_next::util::frame::video::Video) -> Option<RgbaImage> {
    let w = frame.width();
    let h = frame.height();
    let stride = frame.stride(0);
    let data = frame.data(0);

    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for row in 0..h as usize {
        let start = row * stride;
        pixels.extend_from_slice(&data[start..start + w as usize * 4]);
    }

    RgbaImage::from_raw(w, h, pixels)
}
