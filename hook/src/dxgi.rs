//! Hooks `D3D11CreateDeviceAndSwapChain` to intercept swap chain Present calls.
//!
//! We use raw vtable manipulation (not windows-sys D3D11 bindings) because:
//! - It avoids a dependency on the full `windows` crate
//! - D3D11 COM interfaces are simple vtable pointers at the binary level
//!
//! COM interface memory layout (x64):
//!   *object → vtable_ptr → [*fn0, *fn1, *fn2, ...]

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE};

type HRESULT = i32;

use crate::iat::{find_iat_entry, patch_ptr};

// ── Opaque COM interface types (raw pointers only) ────────────────────────────

/// Opaque handle to any COM interface — used as a raw void pointer.
type ComPtr = *mut ();

// ── D3D11 constants (from d3d11.h) ────────────────────────────────────────────

const D3D11_USAGE_STAGING: u32     = 3;
const D3D11_CPU_ACCESS_READ: u32   = 0x20000;
const D3D11_MAP_READ: u32          = 1;

// ── DXGI_FORMAT for BGRA8 swap chains ────────────────────────────────────────

/// D3D11_TEXTURE2D_DESC (simplified, matches the Windows SDK layout)
#[repr(C)]
struct D3D11Texture2DDesc {
    width:              u32,
    height:             u32,
    mip_levels:         u32,
    array_size:         u32,
    format:             u32,
    sample_desc_count:  u32,
    sample_desc_quality:u32,
    usage:              u32,
    bind_flags:         u32,
    cpu_access_flags:   u32,
    misc_flags:         u32,
}

/// D3D11_MAPPED_SUBRESOURCE
#[repr(C)]
struct D3D11MappedSubresource {
    p_data:    *mut u8,
    row_pitch: u32,
    depth_pitch: u32,
}

// IID for ID3D11Texture2D {6f15aaf2-d208-4e89-9ab4-489535d34f9c}
#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const IID_ID3D11TEXTURE2D: Guid = Guid {
    data1: 0x6f15aaf2,
    data2: 0xd208,
    data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

// ── Global state ──────────────────────────────────────────────────────────────

static mut ORIG_PRESENT: Option<unsafe extern "system" fn(ComPtr, u32, u32) -> HRESULT> = None;

static mut ORIG_D3D11_CREATE: Option<
    unsafe extern "system" fn(
        ComPtr, u32, ComPtr, u32,
        *const u32, u32, u32,
        *const (), *mut ComPtr, *mut ComPtr, *mut u32, *mut ComPtr,
    ) -> HRESULT,
> = None;

// ── IAT patch entry point ─────────────────────────────────────────────────────

/// Patch the IAT entry for `D3D11CreateDeviceAndSwapChain` in WE's PE.
pub unsafe fn patch_d3d11_create() {
    let base = GetModuleHandleW(std::ptr::null()) as *mut u8;
    if base.is_null() {
        return;
    }

    if let Some(entry) = find_iat_entry(base, "d3d11.dll", "D3D11CreateDeviceAndSwapChain") {
        ORIG_D3D11_CREATE = Some(std::mem::transmute(*entry));
        patch_ptr(entry, hooked_d3d11_create as *const ());
    }
}

// ── Hooked D3D11CreateDeviceAndSwapChain ──────────────────────────────────────

unsafe extern "system" fn hooked_d3d11_create(
    adapter: ComPtr,
    driver_type: u32,
    software: ComPtr,
    flags: u32,
    feature_levels: *const u32,
    num_feature_levels: u32,
    sdk_version: u32,
    swap_chain_desc: *const (),
    pp_swap_chain: *mut ComPtr,
    pp_device: *mut ComPtr,
    p_feature_level: *mut u32,
    pp_immediate_context: *mut ComPtr,
) -> HRESULT {
    let orig = match ORIG_D3D11_CREATE {
        Some(f) => f,
        None => return -1,
    };

    let hr = orig(
        adapter, driver_type, software, flags,
        feature_levels, num_feature_levels, sdk_version,
        swap_chain_desc, pp_swap_chain, pp_device,
        p_feature_level, pp_immediate_context,
    );

    if hr == 0 && !pp_swap_chain.is_null() && !(*pp_swap_chain).is_null() {
        patch_present_vtable(*pp_swap_chain);
    }

    hr
}

// ── Vtable Present patch ──────────────────────────────────────────────────────

unsafe fn patch_present_vtable(swap_chain: ComPtr) {
    // IDXGISwapChain vtable (x64):
    //   0: QueryInterface, 1: AddRef, 2: Release         (IUnknown)
    //   3-6: IDXGIObject methods
    //   7: GetDevice                                      (IDXGIDeviceSubObject)
    //   8: Present  ← we patch this slot
    const PRESENT_SLOT: usize = 8;

    let vtable = *(swap_chain as *mut *mut *mut ());
    let present_entry = vtable.add(PRESENT_SLOT) as *mut usize;

    ORIG_PRESENT = Some(std::mem::transmute(*present_entry));

    let mut old: u32 = 0;
    VirtualProtect(present_entry as *const _, 8, PAGE_EXECUTE_READWRITE, &mut old);
    *(present_entry as *mut usize) = hooked_present as *const () as usize;
    let mut dummy: u32 = 0;
    VirtualProtect(present_entry as *const _, 8, old, &mut dummy);
}

// ── Hooked Present ────────────────────────────────────────────────────────────

unsafe extern "system" fn hooked_present(
    this: ComPtr,
    sync_interval: u32,
    flags: u32,
) -> HRESULT {
    let hr = match ORIG_PRESENT {
        Some(f) => f(this, sync_interval, flags),
        None => return -1,
    };

    capture_and_send(this);

    hr
}

// ── Frame capture ─────────────────────────────────────────────────────────────

unsafe fn capture_and_send(swap_chain: ComPtr) {
    // --- GetBuffer(0, IID_ID3D11Texture2D, &back_buffer) [vtable slot 9] ---
    let get_buffer: unsafe extern "system" fn(ComPtr, u32, *const Guid, *mut ComPtr) -> HRESULT =
        vtable_fn(swap_chain, 9);

    let mut back_buffer: ComPtr = std::ptr::null_mut();
    if get_buffer(swap_chain, 0, &IID_ID3D11TEXTURE2D, &mut back_buffer) != 0
        || back_buffer.is_null()
    {
        return;
    }

    // --- ID3D11Resource::GetDesc [slot 7 of ID3D11Texture2D] ---
    // ID3D11Texture2D vtable:
    //   0-2: IUnknown, 3-4: ID3D11DeviceChild, 5-6: ID3D11Resource, 7: GetDesc
    let get_desc: unsafe extern "system" fn(ComPtr, *mut D3D11Texture2DDesc) =
        vtable_fn(back_buffer, 10); // GetDesc is slot 10 in ID3D11Texture2D

    let mut desc: D3D11Texture2DDesc = std::mem::zeroed();
    get_desc(back_buffer, &mut desc);

    if desc.width == 0 || desc.height == 0 {
        com_release(back_buffer);
        return;
    }

    // --- ID3D11DeviceChild::GetDevice [slot 3] → ID3D11Device ---
    let get_device: unsafe extern "system" fn(ComPtr, *mut ComPtr) = vtable_fn(back_buffer, 3);
    let mut device: ComPtr = std::ptr::null_mut();
    get_device(back_buffer, &mut device);
    if device.is_null() {
        com_release(back_buffer);
        return;
    }

    // --- ID3D11Device::GetImmediateContext [slot 40] ---
    let get_ctx: unsafe extern "system" fn(ComPtr, *mut ComPtr) = vtable_fn(device, 40);
    let mut ctx: ComPtr = std::ptr::null_mut();
    get_ctx(device, &mut ctx);
    if ctx.is_null() {
        com_release(device);
        com_release(back_buffer);
        return;
    }

    // --- Create staging texture ---
    let staging_desc = D3D11Texture2DDesc {
        width:               desc.width,
        height:              desc.height,
        mip_levels:          1,
        array_size:          1,
        format:              desc.format,
        sample_desc_count:   1,
        sample_desc_quality: 0,
        usage:               D3D11_USAGE_STAGING,
        bind_flags:          0,
        cpu_access_flags:    D3D11_CPU_ACCESS_READ,
        misc_flags:          0,
    };

    // ID3D11Device::CreateTexture2D [slot 5]
    let create_tex: unsafe extern "system" fn(
        ComPtr, *const D3D11Texture2DDesc, *const (), *mut ComPtr,
    ) -> HRESULT = vtable_fn(device, 5);

    let mut staging: ComPtr = std::ptr::null_mut();
    if create_tex(device, &staging_desc, std::ptr::null(), &mut staging) != 0 || staging.is_null()
    {
        com_release(ctx);
        com_release(device);
        com_release(back_buffer);
        return;
    }

    // --- ID3D11DeviceContext::CopyResource [slot 47] ---
    let copy_res: unsafe extern "system" fn(ComPtr, ComPtr, ComPtr) = vtable_fn(ctx, 47);
    copy_res(ctx, staging, back_buffer);

    // --- ID3D11DeviceContext::Map [slot 14] ---
    let map_fn: unsafe extern "system" fn(
        ComPtr, ComPtr, u32, u32, u32, *mut D3D11MappedSubresource,
    ) -> HRESULT = vtable_fn(ctx, 14);

    let mut mapped: D3D11MappedSubresource = std::mem::zeroed();
    if map_fn(ctx, staging, 0, D3D11_MAP_READ, 0, &mut mapped) == 0 {
        let w = desc.width as usize;
        let h = desc.height as usize;
        let row_pitch = mapped.row_pitch as usize;
        let stride = w * 4;

        // Collect rows into a contiguous buffer, stripping any row padding
        let mut pixels = Vec::with_capacity(stride * h);
        for row in 0..h {
            let src = std::slice::from_raw_parts(mapped.p_data.add(row * row_pitch), stride);
            pixels.extend_from_slice(src);
        }

        crate::ipc::send_frame(desc.width, desc.height, &pixels);

        // ID3D11DeviceContext::Unmap [slot 15]
        let unmap_fn: unsafe extern "system" fn(ComPtr, ComPtr, u32) = vtable_fn(ctx, 15);
        unmap_fn(ctx, staging, 0);
    }

    com_release(staging);
    com_release(ctx);
    com_release(device);
    com_release(back_buffer);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract a function pointer from a COM object's vtable at the given slot.
#[inline]
unsafe fn vtable_fn<F: Copy>(obj: ComPtr, slot: usize) -> F {
    let vtable = *(obj as *mut *mut usize);
    std::mem::transmute_copy(&*vtable.add(slot))
}

/// Call Release() (IUnknown slot 2) on a COM object.
unsafe fn com_release(obj: ComPtr) {
    if !obj.is_null() {
        let release: unsafe extern "system" fn(ComPtr) -> u32 = vtable_fn(obj, 2);
        release(obj);
    }
}
