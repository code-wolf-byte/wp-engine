//! Unix socket IPC server — receives BGRA frames from the hook DLL and
//! forwards them through a channel to the Wayland renderer.
//!
//! Frame wire format (little-endian):
//!   [u32 width][u32 height][width * height * 4 bytes BGRA8]
//!
//! The server binds one socket, accepts a single connection (the hook DLL
//! inside Wallpaper Engine), and reads frames in a tight loop.  A bounded
//! channel of size 2 provides natural back-pressure so the sender never
//! runs more than two frames ahead of the display.

use anyhow::{Context, Result};
use image::RgbaImage;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{mpsc::SyncSender, Arc};

/// A Unix socket server that receives rendered frames from the hook DLL.
pub struct IpcServer {
    /// Path of the bound socket file.
    pub socket_path: PathBuf,
    listener: UnixListener,
}

impl IpcServer {
    /// Create a new server socket at a unique path under `/tmp`.
    pub fn bind() -> Result<Self> {
        let socket_path = PathBuf::from(format!("/tmp/wp-engine-{}.sock", std::process::id()));

        // Remove stale socket file if it exists.
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind Unix socket at {}", socket_path.display()))?;

        Ok(Self { socket_path, listener })
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Spawn a background thread that:
/// 1. Waits for the hook DLL to connect
/// 2. Reads frames and sends them through `tx`
///
/// The thread exits when the connection is closed or `tx` is dropped.
pub fn start_frame_receiver(server: IpcServer, tx: SyncSender<Arc<RgbaImage>>) {
    std::thread::spawn(move || {
        // Accept exactly one connection (from the hook DLL inside WE).
        let (mut stream, _) = match server.listener.accept() {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("ipc: accept failed: {e}");
                return;
            }
        };

        loop {
            // Read 8-byte header: [width: u32 LE][height: u32 LE]
            let mut header = [0u8; 8];
            if read_exact_or_eof(&mut stream, &mut header).is_err() {
                break;
            }

            let width = u32::from_le_bytes(header[..4].try_into().unwrap());
            let height = u32::from_le_bytes(header[4..].try_into().unwrap());

            if width == 0 || height == 0 || width > 16384 || height > 16384 {
                eprintln!("ipc: invalid frame dimensions {width}x{height}, closing");
                break;
            }

            let pixel_count = (width as usize) * (height as usize) * 4;
            let mut raw = vec![0u8; pixel_count];
            if read_exact_or_eof(&mut stream, &mut raw).is_err() {
                break;
            }

            // The hook sends BGRA8; swap B↔R to produce RGBA for RgbaImage.
            for pixel in raw.chunks_exact_mut(4) {
                pixel.swap(0, 2); // B ↔ R
            }

            let img = match RgbaImage::from_raw(width, height, raw) {
                Some(i) => Arc::new(i),
                None => {
                    eprintln!("ipc: RgbaImage::from_raw failed for {width}x{height}");
                    break;
                }
            };

            // Send; drop frame if full (back-pressure), exit if disconnected.
            use std::sync::mpsc::TrySendError;
            match tx.try_send(img) {
                Ok(_) => {}
                Err(TrySendError::Full(_)) => {} // drop this frame, keep going
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
    });
}

fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<()> {
    r.read_exact(buf)
}
