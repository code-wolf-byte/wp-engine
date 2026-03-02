//! Unix socket IPC server — receives BGRA frames from the hook DLL.
//!
//! Frame wire format: [u32 LE width][u32 LE height][width * height * 4 bytes BGRA8]

use anyhow::{Context, Result};
use image::RgbaImage;
use std::io::{self, Read};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{mpsc::SyncSender, Arc};

pub struct IpcServer {
    pub socket_path: PathBuf,
    listener: UnixListener,
}

impl IpcServer {
    pub fn bind() -> Result<Self> {
        let socket_path = PathBuf::from(format!("/tmp/wp-engine-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket_path);

        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind Unix socket at {}", socket_path.display()))?;

        eprintln!("[ipc] server bound at {}", socket_path.display());
        Ok(Self { socket_path, listener })
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Spawn a background thread that accepts one connection from the hook DLL
/// and forwards frames through `tx`.
pub fn start_frame_receiver(server: IpcServer, tx: SyncSender<Arc<RgbaImage>>) {
    eprintln!("[ipc] waiting for hook DLL to connect…");

    std::thread::spawn(move || {
        let (mut stream, addr) = match server.listener.accept() {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("[ipc] accept failed: {e}");
                return;
            }
        };
        eprintln!("[ipc] hook DLL connected ({addr:?})");

        let mut frame_count = 0u64;

        loop {
            let mut header = [0u8; 8];
            match read_exact_or_eof(&mut stream, &mut header) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    eprintln!("[ipc] connection closed after {frame_count} frames");
                    break;
                }
                Err(e) => {
                    eprintln!("[ipc] header read error after {frame_count} frames: {e}");
                    break;
                }
            }

            let width  = u32::from_le_bytes(header[..4].try_into().unwrap());
            let height = u32::from_le_bytes(header[4..].try_into().unwrap());

            if width == 0 || height == 0 || width > 16384 || height > 16384 {
                eprintln!("[ipc] invalid frame dimensions {width}x{height}, closing");
                break;
            }

            if frame_count == 0 {
                eprintln!("[ipc] first frame received: {width}x{height}");
            }

            let pixel_count = (width as usize) * (height as usize) * 4;
            let mut raw = vec![0u8; pixel_count];
            if let Err(e) = read_exact_or_eof(&mut stream, &mut raw) {
                eprintln!("[ipc] pixel read error: {e}");
                break;
            }

            // Hook sends BGRA8; swap B↔R to get RGBA for RgbaImage.
            for pixel in raw.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }

            let img = match RgbaImage::from_raw(width, height, raw) {
                Some(i) => Arc::new(i),
                None => {
                    eprintln!("[ipc] RgbaImage::from_raw failed for {width}x{height}");
                    break;
                }
            };

            frame_count += 1;

            use std::sync::mpsc::TrySendError;
            match tx.try_send(img) {
                Ok(_) => {}
                Err(TrySendError::Full(_)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    eprintln!("[ipc] renderer dropped channel, exiting");
                    break;
                }
            }
        }

        eprintln!("[ipc] frame receiver exited ({frame_count} total frames)");
    });
}

fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> io::Result<()> {
    r.read_exact(buf)
}
