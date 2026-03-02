//! IPC client: connects to wp-engine's Unix socket and sends captured frames.
//!
//! Frame wire format: `[u32 LE width][u32 LE height][width*height*4 bytes BGRA8]`

use windows_sys::Win32::Networking::WinSock::{
    connect as winsock_connect, send, socket, WSAStartup,
    AF_UNIX, INVALID_SOCKET, SOCK_STREAM, SOCKET, WSADATA, SOCKADDR,
};

use std::sync::OnceLock;

// Maximum Unix socket path length on Windows
const UNIX_PATH_MAX: usize = 108;

#[repr(C)]
struct SockAddrUn {
    sun_family: u16,
    sun_path: [u8; UNIX_PATH_MAX],
}

static SOCKET_FD: OnceLock<SOCKET> = OnceLock::new();

/// Connect to the Unix socket path given by the `WP_ENGINE_SOCKET` env var.
/// Safe to call multiple times — only the first call does anything.
pub fn connect() {
    if SOCKET_FD.get().is_some() {
        return;
    }

    let path = match std::env::var("WP_ENGINE_SOCKET").ok().filter(|s| !s.is_empty()) {
        Some(p) => p,
        None => return,
    };

    unsafe {
        // Initialise Winsock 2.2
        let mut wsa_data: WSADATA = std::mem::zeroed();
        if WSAStartup(0x0202, &mut wsa_data) != 0 {
            return;
        }

        let sock = socket(AF_UNIX as i32, SOCK_STREAM as i32, 0);
        if sock == INVALID_SOCKET {
            return;
        }

        let path_bytes = path.as_bytes();
        if path_bytes.len() >= UNIX_PATH_MAX {
            return;
        }

        let mut addr: SockAddrUn = std::mem::zeroed();
        addr.sun_family = AF_UNIX as u16;
        addr.sun_path[..path_bytes.len()].copy_from_slice(path_bytes);

        // addr length = size of sun_family (2) + strlen(path) + 1 (null terminator)
        let addr_len = (2 + path_bytes.len() + 1) as i32;

        if winsock_connect(sock, &addr as *const SockAddrUn as *const SOCKADDR, addr_len) != 0 {
            return;
        }

        let _ = SOCKET_FD.set(sock);
    }
}

/// Send one frame to wp-engine.
/// `bgra_pixels` must be `width * height * 4` bytes of BGRA8 data.
pub fn send_frame(width: u32, height: u32, bgra_pixels: &[u8]) {
    let sock = match SOCKET_FD.get() {
        Some(&s) => s,
        None => return,
    };

    let mut header = [0u8; 8];
    header[..4].copy_from_slice(&width.to_le_bytes());
    header[4..8].copy_from_slice(&height.to_le_bytes());

    unsafe {
        send_all(sock, &header);
        send_all(sock, bgra_pixels);
    }
}

unsafe fn send_all(sock: SOCKET, data: &[u8]) {
    let mut offset = 0usize;
    while offset < data.len() {
        let n = send(
            sock,
            data.as_ptr().add(offset),
            (data.len() - offset) as i32,
            0,
        );
        if n <= 0 {
            break;
        }
        offset += n as usize;
    }
}
