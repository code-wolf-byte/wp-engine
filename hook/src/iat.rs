//! PE Import Address Table (IAT) walking utilities.

use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE};

/// Walk the import directory of the PE image at `base` and return a mutable
/// pointer to the IAT entry for `func_name` in `dll_name`.
///
/// Both `dll_name` and `func_name` must be ASCII strings (no null terminator needed).
pub unsafe fn find_iat_entry(
    base: *mut u8,
    dll_name: &str,
    func_name: &str,
) -> Option<*mut *mut ()> {
    // ── Parse DOS header ──────────────────────────────────────────────────────
    let dos = &*(base as *const ImageDosHeader);
    if dos.e_magic != 0x5A4D {
        // "MZ"
        return None;
    }

    let nt = &*(base.add(dos.e_lfanew as usize) as *const ImageNtHeaders64);
    if nt.signature != 0x00004550 {
        // "PE\0\0"
        return None;
    }

    let import_dir = &nt.optional_header.data_directory[1]; // IMAGE_DIRECTORY_ENTRY_IMPORT
    if import_dir.virtual_address == 0 {
        return None;
    }

    let mut desc =
        base.add(import_dir.virtual_address as usize) as *const ImageImportDescriptor;

    // ── Walk import descriptors (one per imported DLL) ────────────────────────
    loop {
        if (*desc).original_first_thunk == 0 && (*desc).first_thunk == 0 {
            break; // null-terminator entry
        }

        let name_rva = (*desc).name as usize;
        let dll = cstr_at(base.add(name_rva));

        if dll.eq_ignore_ascii_case(dll_name) {
            // ── Found the right DLL — walk its thunk arrays ───────────────────
            let mut oft = base.add((*desc).original_first_thunk as usize) as *const usize;
            let mut ft  = base.add((*desc).first_thunk as usize) as *mut *mut ();

            loop {
                let thunk = *oft;
                if thunk == 0 {
                    break;
                }

                // High bit set → ordinal import (skip)
                let is_ordinal = thunk & (1usize << 63) != 0;
                if !is_ordinal {
                    let ibn = &*(base.add(thunk) as *const ImageImportByName);
                    let imported = cstr_at(ibn.name.as_ptr());
                    if imported == func_name {
                        return Some(ft);
                    }
                }

                oft = oft.add(1);
                ft  = ft.add(1);
            }
        }

        desc = desc.add(1);
    }

    None
}

/// Overwrite an IAT entry pointer, temporarily making the page writable.
pub unsafe fn patch_ptr(entry: *mut *mut (), new_fn: *const ()) {
    let mut old_protect: u32 = 0;
    VirtualProtect(
        entry as *const _,
        core::mem::size_of::<usize>(),
        PAGE_EXECUTE_READWRITE,
        &mut old_protect,
    );
    *entry = new_fn as *mut ();
    VirtualProtect(
        entry as *const _,
        core::mem::size_of::<usize>(),
        old_protect,
        &mut old_protect,
    );
}

// ── C-string helper ───────────────────────────────────────────────────────────

unsafe fn cstr_at<'a>(ptr: *const u8) -> &'a str {
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
    }
    core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, len))
}

// ── PE structure definitions (x64) ───────────────────────────────────────────

#[repr(C)]
struct ImageDosHeader {
    e_magic: u16,
    _pad: [u16; 29],
    e_lfanew: i32,
}

#[repr(C)]
struct ImageNtHeaders64 {
    signature: u32,
    file_header: ImageFileHeader,
    optional_header: ImageOptionalHeader64,
}

#[repr(C)]
struct ImageFileHeader {
    machine: u16,
    number_of_sections: u16,
    time_date_stamp: u32,
    pointer_to_symbol_table: u32,
    number_of_symbols: u32,
    size_of_optional_header: u16,
    characteristics: u16,
}

#[repr(C)]
struct ImageDataDirectory {
    virtual_address: u32,
    size: u32,
}

#[repr(C)]
struct ImageOptionalHeader64 {
    magic: u16,
    major_linker_version: u8,
    minor_linker_version: u8,
    size_of_code: u32,
    size_of_initialized_data: u32,
    size_of_uninitialized_data: u32,
    address_of_entry_point: u32,
    base_of_code: u32,
    image_base: u64,
    section_alignment: u32,
    file_alignment: u32,
    major_os_version: u16,
    minor_os_version: u16,
    major_image_version: u16,
    minor_image_version: u16,
    major_subsystem_version: u16,
    minor_subsystem_version: u16,
    win32_version_value: u32,
    size_of_image: u32,
    size_of_headers: u32,
    check_sum: u32,
    subsystem: u16,
    dll_characteristics: u16,
    size_of_stack_reserve: u64,
    size_of_stack_commit: u64,
    size_of_heap_reserve: u64,
    size_of_heap_commit: u64,
    loader_flags: u32,
    number_of_rva_and_sizes: u32,
    data_directory: [ImageDataDirectory; 16],
}

#[repr(C)]
struct ImageImportDescriptor {
    original_first_thunk: u32,
    time_date_stamp: u32,
    forwarder_chain: u32,
    name: u32,
    first_thunk: u32,
}

#[repr(C)]
struct ImageImportByName {
    hint: u16,
    name: [u8; 1], // variable-length field
}
