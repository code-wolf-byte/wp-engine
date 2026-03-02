//! version.dll — Wallpaper Engine hook DLL.
//!
//! Loaded by Wine via `WINEDLLOVERRIDES=version=n` (native, meaning ours is
//! loaded instead of Wine's built-in version.dll stub).
//!
//! On `DLL_PROCESS_ATTACH`, spawns a setup thread that:
//! 1. Suppresses WE's windows (IAT patch on ShowWindow/SetParent)
//! 2. Hooks D3D11CreateDeviceAndSwapChain to intercept swap chain Present
//! 3. Connects to wp-engine's Unix socket IPC channel

#![allow(non_snake_case, unsafe_op_in_unsafe_fn)]

mod dxgi;
mod iat;
mod ipc;
mod stubs;

use windows_sys::Win32::Foundation::{BOOL, HINSTANCE};

const DLL_PROCESS_ATTACH: u32 = 1;

#[no_mangle]
pub unsafe extern "system" fn DllMain(
    _hinstDLL: HINSTANCE,
    fdwReason: u32,
    _lpReserved: *mut std::ffi::c_void,
) -> BOOL {
    if fdwReason == DLL_PROCESS_ATTACH {
        // Spawn setup on a background thread so DllMain returns quickly.
        std::thread::spawn(|| {
            // Small sleep to let WE finish its own DLL initialisation.
            std::thread::sleep(std::time::Duration::from_millis(500));
            setup();
        });
    }
    1 // TRUE
}

fn setup() {
    unsafe {
        stubs::patch_show_window();
        dxgi::patch_d3d11_create();
        ipc::connect();
    }
}

// ── version.dll forwarding stubs ──────────────────────────────────────────────
//
// Wine's built-in version.dll exports these ordinals. We must re-export them
// so any WE code that links against version.dll for its legitimate purpose
// (e.g. GetFileVersionInfoW) continues to work. We forward to the real Wine
// version.dll (loaded as "version_real.dll" by the override mechanism) or
// simply return failure.

#[no_mangle]
pub unsafe extern "system" fn GetFileVersionInfoA(
    _: *const u8, _: u32, _: u32, _: *mut std::ffi::c_void,
) -> BOOL {
    0
}

#[no_mangle]
pub unsafe extern "system" fn GetFileVersionInfoW(
    _: *const u16, _: u32, _: u32, _: *mut std::ffi::c_void,
) -> BOOL {
    0
}

#[no_mangle]
pub unsafe extern "system" fn GetFileVersionInfoSizeA(
    _: *const u8, _: *mut u32,
) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn GetFileVersionInfoSizeW(
    _: *const u16, _: *mut u32,
) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "system" fn VerQueryValueA(
    _: *const std::ffi::c_void, _: *const u8,
    _: *mut *mut std::ffi::c_void, _: *mut u32,
) -> BOOL {
    0
}

#[no_mangle]
pub unsafe extern "system" fn VerQueryValueW(
    _: *const std::ffi::c_void, _: *const u16,
    _: *mut *mut std::ffi::c_void, _: *mut u32,
) -> BOOL {
    0
}
