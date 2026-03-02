//! IAT patches that prevent Wallpaper Engine's window from ever appearing.
//!
//! We patch two imports in WE's PE:
//! - `ShowWindow`  → always returns TRUE without calling the real function
//! - `SetParent`   → no-op returning the input HWND (WE thinks it's embedded)

use windows_sys::Win32::Foundation::{BOOL, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::iat::{find_iat_entry, patch_ptr};

pub unsafe fn patch_show_window() {
    let base = GetModuleHandleW(std::ptr::null()) as *mut u8;
    if base.is_null() {
        return;
    }

    // Patch ShowWindow in user32.dll imports
    if let Some(entry) = find_iat_entry(base, "user32.dll", "ShowWindow") {
        patch_ptr(entry, stub_show_window as *const ());
    }

    // Patch SetParent in user32.dll imports
    if let Some(entry) = find_iat_entry(base, "user32.dll", "SetParent") {
        patch_ptr(entry, stub_set_parent as *const ());
    }
}

/// Stub for ShowWindow — always returns TRUE without showing anything.
unsafe extern "system" fn stub_show_window(_hwnd: HWND, _cmd: i32) -> BOOL {
    1 // TRUE
}

/// Stub for SetParent — no-op, returns the child HWND unchanged.
unsafe extern "system" fn stub_set_parent(child: HWND, _new_parent: HWND) -> HWND {
    child
}
