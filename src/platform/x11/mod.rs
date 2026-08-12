//! X11 display backend — draws into the root window's background pixmap.
//!
//! This mirrors `X11Output.cpp` in the reference rather than creating a
//! `_NET_WM_WINDOW_TYPE_DESKTOP` window: every WM and DE honours the root
//! pixmap plus the `_XROOTPMAP_ID` / `ESETROOT_PMAP_ID` convention (it is what
//! `feh --bg` and `hsetroot` set), whereas a desktop-type window competes with
//! whatever desktop surface GNOME/KDE/Xfce already draw and loses differently
//! on each.
//!
//! Unlike the Wayland backend there is no direct-presentation path: X11 has no
//! equivalent of handing the compositor a GPU surface for the desktop
//! background, so every frame is read back to RGBA and pushed with `PutImage`.
//! That readback is the same one `draw_shm` already performs, so scenes cost
//! roughly what they cost on the Wayland SHM fallback.

use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateGCAux, Gcontext, ImageFormat,
    Pixmap, PropMode, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::display::{DisplayPlatform, WallpaperHandle, WallpaperHandleInner};
use crate::engine::gpu_renderer::GpuSceneInstance;
use crate::platform;
use crate::render::{FrameSource, RenderSettings, WallpaperContent};

/// Frames per second for animated content. Matches `FrameSource`'s own target.
const TARGET_FPS: f32 = 30.0;

// ── Platform implementation ───────────────────────────────────────────────────

pub(super) struct X11Platform;

impl DisplayPlatform for X11Platform {
    fn spawn_wallpaper(
        &self,
        content: WallpaperContent,
        settings: Arc<Mutex<RenderSettings>>,
    ) -> Result<WallpaperHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let settings_thread = Arc::clone(&settings);

        let thread = thread::spawn(move || {
            if let Err(e) = wallpaper_loop(content, settings_thread, stop_thread) {
                tracing::error!(target: "wallpaper", "X11 wallpaper thread error: {e}");
            }
        });

        Ok(WallpaperHandle::new(
            Box::new(X11Handle { stop, thread }),
            settings,
        ))
    }
}

struct X11Handle {
    stop: Arc<AtomicBool>,
    thread: thread::JoinHandle<()>,
}

impl WallpaperHandleInner for X11Handle {
    fn stop(self: Box<Self>) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }

    fn wait(self: Box<Self>) {
        let _ = self.thread.join();
    }
}

// ── Outputs ───────────────────────────────────────────────────────────────────

/// One monitor's rectangle in root-window coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutputRect {
    pub x: i16,
    pub y: i16,
    pub width: u16,
    pub height: u16,
}

/// Enumerate active RandR CRTCs, falling back to the whole root window when
/// RandR is missing or reports nothing usable (headless X, Xvfb, old servers).
fn discover_outputs(
    conn: &RustConnection,
    root: Window,
    root_w: u16,
    root_h: u16,
) -> Vec<OutputRect> {
    let whole = vec![OutputRect {
        x: 0,
        y: 0,
        width: root_w,
        height: root_h,
    }];

    let Ok(cookie) = conn.randr_get_screen_resources_current(root) else {
        return whole;
    };
    let Ok(resources) = cookie.reply() else {
        return whole;
    };

    let mut rects: Vec<OutputRect> = resources
        .crtcs
        .iter()
        .filter_map(|&crtc| {
            let info = conn
                .randr_get_crtc_info(crtc, resources.config_timestamp)
                .ok()?
                .reply()
                .ok()?;
            // A CRTC with no mode is disconnected/disabled.
            (info.width > 0 && info.height > 0).then_some(OutputRect {
                x: info.x,
                y: info.y,
                width: info.width,
                height: info.height,
            })
        })
        .collect();

    // Mirrored outputs report identical rectangles; drawing one twice is waste.
    rects.dedup();
    if rects.is_empty() {
        whole
    } else {
        rects
    }
}

// ── Content ───────────────────────────────────────────────────────────────────

/// How the wallpaper produces pixels. Both arms end in a CPU RGBA frame —
/// the root pixmap has no GPU-surface equivalent.
enum ContentRenderer {
    Frames(FrameSource),
    Scene(Box<GpuSceneInstance>),
}

impl ContentRenderer {
    fn is_animated(&self) -> bool {
        match self {
            ContentRenderer::Frames(fs) => fs.is_animated(),
            ContentRenderer::Scene(_) => true,
        }
    }

    fn next_frame(&mut self) -> Result<Arc<image::RgbaImage>> {
        match self {
            ContentRenderer::Frames(fs) => {
                fs.try_advance();
                Ok(Arc::clone(fs.current_frame()))
            }
            ContentRenderer::Scene(instance) => Ok(Arc::new(instance.render_rgba()?)),
        }
    }
}

// ── PutImage ──────────────────────────────────────────────────────────────────

/// Bytes of fixed overhead in a `PutImage` request (24-byte header, plus slack
/// for the length field's 4-byte padding).
const PUT_IMAGE_OVERHEAD: usize = 64;

/// Push `pixels` (BGRA, `rect.width * rect.height * 4` bytes) into `pixmap` at
/// `rect`'s offset, split into horizontal bands that each fit in one request.
///
/// A single 4K frame is ~33 MB, far past the ~16 MB ceiling even with
/// BIG-REQUESTS, so chunking is required rather than defensive.
fn put_image_chunked(
    conn: &RustConnection,
    pixmap: Pixmap,
    gc: Gcontext,
    depth: u8,
    rect: OutputRect,
    pixels: &[u8],
) -> Result<()> {
    let row_bytes = rect.width as usize * 4;
    if row_bytes == 0 {
        return Ok(());
    }

    let budget = conn
        .maximum_request_bytes()
        .saturating_sub(PUT_IMAGE_OVERHEAD);
    let rows_per_chunk = (budget / row_bytes).max(1).min(rect.height as usize);

    for band_start in (0..rect.height as usize).step_by(rows_per_chunk) {
        let band_rows = rows_per_chunk.min(rect.height as usize - band_start);
        let start = band_start * row_bytes;
        let end = start + band_rows * row_bytes;
        let Some(band) = pixels.get(start..end) else {
            return Err(anyhow!(
                "frame buffer is {} bytes, short of the {} needed for {}x{}",
                pixels.len(),
                rect.height as usize * row_bytes,
                rect.width,
                rect.height
            ));
        };

        conn.put_image(
            ImageFormat::Z_PIXMAP,
            pixmap,
            gc,
            rect.width,
            band_rows as u16,
            rect.x,
            rect.y + band_start as i16,
            0,
            depth,
            band,
        )?;
    }
    Ok(())
}

// ── Main loop ─────────────────────────────────────────────────────────────────

fn wallpaper_loop(
    content: WallpaperContent,
    settings: Arc<Mutex<RenderSettings>>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|e| anyhow!("cannot connect to X display: {e}"))?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let depth = screen.root_depth;
    let (root_w, root_h) = (screen.width_in_pixels, screen.height_in_pixels);

    // `GpuScaler::from_device` consumes the device, so clone the parts the
    // scene renderer needs first (same dance as the Wayland backend).
    let gpu = platform::GpuDevice::open_low_power()
        .or_else(|_| platform::GpuDevice::open_best())
        .map_err(|e| anyhow!("no GPU device available: {e}"))?;
    let device = gpu.device.clone();
    let queue = gpu.queue.clone();
    let gpu_scaler = platform::GpuScaler::from_device(gpu)
        .map_err(|e| anyhow!("GPU scaler init failed: {e}"))?;

    let mut renderer = match content {
        WallpaperContent::Scene { dir } => {
            match GpuSceneInstance::with_device(device, queue, &dir) {
                Ok(instance) => ContentRenderer::Scene(Box::new(instance)),
                Err(e) => {
                    tracing::warn!(target: "wallpaper", "GPU scene init failed ({e}); using frame-loop fallback");
                    ContentRenderer::Frames(FrameSource::from_content(WallpaperContent::Scene {
                        dir,
                    })?)
                }
            }
        }
        other => ContentRenderer::Frames(FrameSource::from_content(other)?),
    };

    let outputs = discover_outputs(&conn, root, root_w, root_h);
    tracing::info!(
        target: "wallpaper",
        "X11 root pixmap {root_w}x{root_h}, depth {depth}, {} output(s)",
        outputs.len()
    );

    // The pixmap stays owned by this connection: when the process exits the
    // server frees it and the previous desktop background comes back.
    let pixmap = conn.generate_id()?;
    conn.create_pixmap(depth, pixmap, root, root_w, root_h)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, pixmap, &CreateGCAux::new())?;

    let prop_root = conn.intern_atom(false, b"_XROOTPMAP_ID")?.reply()?.atom;
    let prop_esetroot = conn.intern_atom(false, b"ESETROOT_PMAP_ID")?.reply()?.atom;

    let frame_budget = Duration::from_secs_f32(1.0 / TARGET_FPS);
    let animated = renderer.is_animated();

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let started = Instant::now();

        let frame = renderer.next_frame()?;
        let quality = settings.lock().unwrap().quality;

        for rect in &outputs {
            // `GpuScaler` emits ARGB8888-LE, i.e. bytes [B, G, R, A] — already
            // the byte order a Z_PIXMAP wants on a little-endian server, and
            // the same order the reference gets from its GL_BGRA readback. Do
            // not "fix" this into an RGBA swap.
            let pixels = gpu_scaler.scale(
                frame.as_ref(),
                rect.width as u32,
                rect.height as u32,
                quality,
            );
            put_image_chunked(&conn, pixmap, gc, depth, *rect, &pixels)?;
        }

        // Publish the pixmap. Compositors (picom et al.) watch these atoms and
        // will otherwise paint over the background themselves.
        conn.change_property32(
            PropMode::REPLACE,
            root,
            prop_root,
            AtomEnum::PIXMAP,
            &[pixmap],
        )?;
        conn.change_property32(
            PropMode::REPLACE,
            root,
            prop_esetroot,
            AtomEnum::PIXMAP,
            &[pixmap],
        )?;
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
        )?;
        // Repaint the root from its new background.
        conn.clear_area(false, root, 0, 0, 0, 0)?;
        conn.flush()?;

        if !animated {
            // Static image: hold the pixmap until asked to stop.
            while !stop.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
            }
            break;
        }

        // Global cursor position drives parallax. X11 hands us this regardless
        // of which window has focus, so unlike the Wayland backend there is no
        // "cursor left our surface" blind spot.
        if let ContentRenderer::Scene(scene) = &mut renderer {
            if let Ok(pointer) = conn.query_pointer(root)?.reply() {
                let norm = [
                    (pointer.root_x as f32 / root_w.max(1) as f32).clamp(0.0, 1.0),
                    (pointer.root_y as f32 / root_h.max(1) as f32).clamp(0.0, 1.0),
                ];
                scene.set_mouse(norm);
            }
        }

        if let Some(rest) = frame_budget.checked_sub(started.elapsed()) {
            thread::sleep(rest);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bands must tile the image exactly: no gaps, no overlap, no lost rows.
    /// Getting this wrong shows up as horizontal stripes of stale pixels.
    #[test]
    fn chunk_bands_tile_the_image_exactly() {
        for (height, rows_per_chunk) in [(1080usize, 7usize), (4u32 as usize, 4), (1000, 999)] {
            let mut covered = vec![0u8; height];
            for band_start in (0..height).step_by(rows_per_chunk) {
                let band_rows = rows_per_chunk.min(height - band_start);
                assert!(band_rows > 0);
                for row in band_start..band_start + band_rows {
                    covered[row] += 1;
                }
            }
            assert!(
                covered.iter().all(|&c| c == 1),
                "height {height} in chunks of {rows_per_chunk} did not tile exactly"
            );
        }
    }

    /// Round-trip a known colour through a real X server to prove the byte
    /// order `GpuScaler` emits is what `Z_PIXMAP` expects. Skipped when no
    /// display is available (CI, headless builds).
    #[test]
    fn put_image_round_trips_bgra_through_the_server() {
        if std::env::var_os("DISPLAY").is_none() {
            eprintln!("skipping: no DISPLAY");
            return;
        }
        let Ok((conn, screen_num)) = x11rb::connect(None) else {
            eprintln!("skipping: cannot connect to X display");
            return;
        };
        let screen = &conn.setup().roots[screen_num];
        let (root, depth) = (screen.root, screen.root_depth);
        let rect = OutputRect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        };

        let pixmap = conn.generate_id().unwrap();
        conn.create_pixmap(depth, pixmap, root, rect.width, rect.height)
            .unwrap();
        let gc = conn.generate_id().unwrap();
        conn.create_gc(gc, pixmap, &CreateGCAux::new()).unwrap();

        // Opaque red, in the [B, G, R, A] order scaler.rs documents emitting.
        let pixels: Vec<u8> = std::iter::repeat([0u8, 0, 255, 255])
            .take(rect.width as usize * rect.height as usize)
            .flatten()
            .collect();
        put_image_chunked(&conn, pixmap, gc, depth, rect, &pixels).unwrap();
        conn.flush().unwrap();

        let got = conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                0,
                0,
                rect.width,
                rect.height,
                !0,
            )
            .unwrap()
            .reply()
            .unwrap();

        // Red must come back in the third byte. If it lands in the first, the
        // server wants RGBA here and the scaler output needs swapping.
        assert_eq!(
            (got.data[0], got.data[1], got.data[2]),
            (0, 0, 255),
            "byte order mismatch: got {:?}, expected red in byte 2 (BGRA)",
            &got.data[..4]
        );

        conn.free_gc(gc).unwrap();
        conn.free_pixmap(pixmap).unwrap();
        conn.flush().unwrap();
    }

    #[test]
    fn outputs_dedup_mirrored_crtcs() {
        let mut rects = vec![
            OutputRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            OutputRect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            OutputRect {
                x: 1920,
                y: 0,
                width: 2560,
                height: 1440,
            },
        ];
        rects.dedup();
        assert_eq!(rects.len(), 2);
    }
}
