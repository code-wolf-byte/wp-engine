//! Hooks `D3D11CreateDevice` to intercept DXGI swap chain Present calls.
//!
//! WE imports `D3D11CreateDevice` (not `D3D11CreateDeviceAndSwapChain`),
//! and loads DXGI dynamically. We hook D3D11CreateDevice from the IAT, then
//! chase the COM chain to reach the DXGI factory before WE creates its swap chain:
//!
//!   ID3D11Device → QI → IDXGIDevice → GetAdapter → IDXGIAdapter
//!       → GetParent → IDXGIFactory → vtable-patch CreateSwapChain
//!           → swap chain created → vtable-patch Present → capture frames

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE};

type HRESULT = i32;

/// Opaque COM interface handle.
type ComPtr = *mut ();

// ── D3D11 constants ───────────────────────────────────────────────────────────

const D3D11_USAGE_STAGING: u32   = 3;
const D3D11_CPU_ACCESS_READ: u32 = 0x20000;
const D3D11_MAP_READ: u32        = 1;

// ── Structs ───────────────────────────────────────────────────────────────────

/// D3D11_TEXTURE2D_DESC (matches Windows SDK layout).
#[repr(C)]
struct D3D11Texture2DDesc {
    width:               u32,
    height:              u32,
    mip_levels:          u32,
    array_size:          u32,
    format:              u32,
    sample_desc_count:   u32,
    sample_desc_quality: u32,
    usage:               u32,
    bind_flags:          u32,
    cpu_access_flags:    u32,
    misc_flags:          u32,
}

/// D3D11_MAPPED_SUBRESOURCE
#[repr(C)]
struct D3D11MappedSubresource {
    p_data:      *mut u8,
    row_pitch:   u32,
    depth_pitch: u32,
}

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

// ── COM IIDs ──────────────────────────────────────────────────────────────────

// IID_ID3D11Texture2D {6f15aaf2-d208-4e89-9ab4-489535d34f9c}
const IID_ID3D11TEXTURE2D: Guid = Guid {
    data1: 0x6f15aaf2, data2: 0xd208, data3: 0x4e89,
    data4: [0x9a, 0xb4, 0x48, 0x95, 0x35, 0xd3, 0x4f, 0x9c],
};

// IID_IDXGIDevice {54ec77fa-1377-44e6-8c32-88fd5f44c84c}
const IID_IDXGI_DEVICE: Guid = Guid {
    data1: 0x54ec77fa, data2: 0x1377, data3: 0x44e6,
    data4: [0x8c, 0x32, 0x88, 0xfd, 0x5f, 0x44, 0xc8, 0x4c],
};

// IID_IDXGIFactory {7b7166ec-21c7-44ae-b21a-c9ae321ae369}
const IID_IDXGI_FACTORY: Guid = Guid {
    data1: 0x7b7166ec, data2: 0x21c7, data3: 0x44ae,
    data4: [0xb2, 0x1a, 0xc9, 0xae, 0x32, 0x1a, 0xe3, 0x69],
};

// IID_IDXGIFactory2 {50c83a1c-e072-4c48-87b0-3630fa36a6d0}
const IID_IDXGI_FACTORY2: Guid = Guid {
    data1: 0x50c83a1c, data2: 0xe072, data3: 0x4c48,
    data4: [0x87, 0xb0, 0x36, 0x30, 0xfa, 0x36, 0xa6, 0xd0],
};

// ── Global saved originals ────────────────────────────────────────────────────

static mut ORIG_D3D11_CREATE_DEVICE: Option<
    unsafe extern "system" fn(
        ComPtr, u32, ComPtr, u32,
        *const u32, u32, u32,
        *mut ComPtr, *mut u32, *mut ComPtr,
    ) -> HRESULT,
> = None;

static mut ORIG_CREATE_SWAP_CHAIN: Option<
    unsafe extern "system" fn(ComPtr, ComPtr, *const (), *mut ComPtr) -> HRESULT,
> = None;

static mut ORIG_CREATE_SWAP_CHAIN_FOR_HWND: Option<
    unsafe extern "system" fn(ComPtr, ComPtr, *mut (), *const (), *const (), ComPtr, *mut ComPtr) -> HRESULT,
> = None;

static mut ORIG_PRESENT: Option<
    unsafe extern "system" fn(ComPtr, u32, u32) -> HRESULT,
> = None;

// ── IAT entry point ───────────────────────────────────────────────────────────

/// Patch `D3D11CreateDevice` in WE's IAT — the only D3D11 import WE has.
pub unsafe fn patch_d3d11_create() {
    let base = GetModuleHandleW(std::ptr::null()) as *mut u8;
    if base.is_null() { return; }

    if let Some(entry) = crate::iat::find_iat_entry(base, "d3d11.dll", "D3D11CreateDevice") {
        ORIG_D3D11_CREATE_DEVICE = Some(std::mem::transmute(*entry));
        crate::iat::patch_ptr(entry, hooked_d3d11_create_device as *const ());
    }
}

// ── Hooked D3D11CreateDevice ──────────────────────────────────────────────────

unsafe extern "system" fn hooked_d3d11_create_device(
    p_adapter:          ComPtr,
    driver_type:        u32,
    software:           ComPtr,
    flags:              u32,
    p_feature_levels:   *const u32,
    num_feature_levels: u32,
    sdk_version:        u32,
    pp_device:          *mut ComPtr,
    p_feature_level:    *mut u32,
    pp_context:         *mut ComPtr,
) -> HRESULT {
    let orig = match ORIG_D3D11_CREATE_DEVICE { Some(f) => f, None => return -1 };

    let hr = orig(
        p_adapter, driver_type, software, flags,
        p_feature_levels, num_feature_levels, sdk_version,
        pp_device, p_feature_level, pp_context,
    );

    if hr == 0 && !pp_device.is_null() && !(*pp_device).is_null() {
        hook_via_device(*pp_device);
    }

    hr
}

// ── COM chain: device → IDXGIDevice → IDXGIAdapter → IDXGIFactory ────────────

unsafe fn hook_via_device(device: ComPtr) {
    // QueryInterface(device, IID_IDXGIDevice) — IUnknown slot 0
    let qi: unsafe extern "system" fn(ComPtr, *const Guid, *mut ComPtr) -> HRESULT =
        vtable_fn(device, 0);
    let mut dxgi_device: ComPtr = std::ptr::null_mut();
    if qi(device, &IID_IDXGI_DEVICE, &mut dxgi_device) != 0 || dxgi_device.is_null() {
        return;
    }

    // IDXGIDevice::GetAdapter — slot 7
    let get_adapter: unsafe extern "system" fn(ComPtr, *mut ComPtr) -> HRESULT =
        vtable_fn(dxgi_device, 7);
    let mut adapter: ComPtr = std::ptr::null_mut();
    if get_adapter(dxgi_device, &mut adapter) != 0 || adapter.is_null() {
        com_release(dxgi_device);
        return;
    }

    // IDXGIObject::GetParent(IID_IDXGIFactory) — IDXGIObject slot 6
    let get_parent: unsafe extern "system" fn(ComPtr, *const Guid, *mut ComPtr) -> HRESULT =
        vtable_fn(adapter, 6);
    let mut factory: ComPtr = std::ptr::null_mut();
    if get_parent(adapter, &IID_IDXGI_FACTORY, &mut factory) != 0 || factory.is_null() {
        com_release(adapter);
        com_release(dxgi_device);
        return;
    }

    hook_factory_vtable(factory);

    com_release(factory);
    com_release(adapter);
    com_release(dxgi_device);
}

// ── Factory vtable patching ───────────────────────────────────────────────────

/// Patch the factory's vtable to intercept swap chain creation.
///
/// IDXGIFactory vtable layout:
///   0-2:  IUnknown (QI, AddRef, Release)
///   3-6:  IDXGIObject (SetPrivateData, SetPrivateDataInterface, GetPrivateData, GetParent)
///   7-11: IDXGIFactory (EnumAdapters, MakeWindowAssociation, GetWindowAssociation,
///                       CreateSwapChain [10], CreateSoftwareAdapter)
///   12-13: IDXGIFactory1 (EnumAdapters1, IsCurrent)
///   14-17: IDXGIFactory2 (IsWindowedStereoEnabled, CreateSwapChainForHwnd [15],
///                         CreateSwapChainForCoreWindow, CreateSwapChainForComposition)
unsafe fn hook_factory_vtable(factory: ComPtr) {
    let vtable = *(factory as *mut *mut usize);

    // Always patch CreateSwapChain (slot 10) — available on all IDXGIFactory versions
    if ORIG_CREATE_SWAP_CHAIN.is_none() {
        let slot = vtable.add(10) as *mut usize;
        ORIG_CREATE_SWAP_CHAIN = Some(std::mem::transmute(*slot));
        patch_usize(slot, hooked_create_swap_chain as *const () as usize);
    }

    // Patch CreateSwapChainForHwnd (slot 15) only if the factory is IDXGIFactory2
    if ORIG_CREATE_SWAP_CHAIN_FOR_HWND.is_none() {
        let qi: unsafe extern "system" fn(ComPtr, *const Guid, *mut ComPtr) -> HRESULT =
            vtable_fn(factory, 0);
        let mut factory2: ComPtr = std::ptr::null_mut();
        if qi(factory, &IID_IDXGI_FACTORY2, &mut factory2) == 0 && !factory2.is_null() {
            // factory and factory2 share the same vtable — patch slot 15 on it
            let slot = vtable.add(15) as *mut usize;
            ORIG_CREATE_SWAP_CHAIN_FOR_HWND = Some(std::mem::transmute(*slot));
            patch_usize(slot, hooked_create_swap_chain_for_hwnd as *const () as usize);
            com_release(factory2);
        }
    }
}

// ── Hooked CreateSwapChain ────────────────────────────────────────────────────

unsafe extern "system" fn hooked_create_swap_chain(
    this:          ComPtr,
    p_device:      ComPtr,
    p_desc:        *const (),
    pp_swap_chain: *mut ComPtr,
) -> HRESULT {
    let orig = match ORIG_CREATE_SWAP_CHAIN { Some(f) => f, None => return -1 };
    let hr = orig(this, p_device, p_desc, pp_swap_chain);
    if hr == 0 && !pp_swap_chain.is_null() && !(*pp_swap_chain).is_null() {
        patch_present_vtable(*pp_swap_chain);
    }
    hr
}

// ── Hooked CreateSwapChainForHwnd ─────────────────────────────────────────────

unsafe extern "system" fn hooked_create_swap_chain_for_hwnd(
    this:              ComPtr,
    p_device:          ComPtr,
    hwnd:              *mut (),
    p_desc:            *const (),
    p_fullscreen_desc: *const (),
    p_restrict_output: ComPtr,
    pp_swap_chain:     *mut ComPtr,
) -> HRESULT {
    let orig = match ORIG_CREATE_SWAP_CHAIN_FOR_HWND { Some(f) => f, None => return -1 };
    let hr = orig(this, p_device, hwnd, p_desc, p_fullscreen_desc, p_restrict_output, pp_swap_chain);
    if hr == 0 && !pp_swap_chain.is_null() && !(*pp_swap_chain).is_null() {
        patch_present_vtable(*pp_swap_chain);
    }
    hr
}

// ── Present vtable patch ──────────────────────────────────────────────────────

unsafe fn patch_present_vtable(swap_chain: ComPtr) {
    // IDXGISwapChain vtable slot 8 = Present
    const PRESENT_SLOT: usize = 8;
    let vtable = *(swap_chain as *mut *mut usize);
    let slot = vtable.add(PRESENT_SLOT) as *mut usize;
    ORIG_PRESENT = Some(std::mem::transmute(*slot));
    patch_usize(slot, hooked_present as *const () as usize);
}

// ── Hooked Present ────────────────────────────────────────────────────────────

unsafe extern "system" fn hooked_present(
    this:          ComPtr,
    sync_interval: u32,
    flags:         u32,
) -> HRESULT {
    let hr = match ORIG_PRESENT {
        Some(f) => f(this, sync_interval, flags),
        None    => return -1,
    };
    capture_and_send(this);
    hr
}

// ── Frame capture ─────────────────────────────────────────────────────────────

unsafe fn capture_and_send(swap_chain: ComPtr) {
    // GetBuffer(0, IID_ID3D11Texture2D, &back_buffer) — IDXGISwapChain slot 9
    let get_buffer: unsafe extern "system" fn(ComPtr, u32, *const Guid, *mut ComPtr) -> HRESULT =
        vtable_fn(swap_chain, 9);
    let mut back_buffer: ComPtr = std::ptr::null_mut();
    if get_buffer(swap_chain, 0, &IID_ID3D11TEXTURE2D, &mut back_buffer) != 0 || back_buffer.is_null() {
        return;
    }

    // ID3D11Texture2D::GetDesc — slot 10
    let get_desc: unsafe extern "system" fn(ComPtr, *mut D3D11Texture2DDesc) =
        vtable_fn(back_buffer, 10);
    let mut desc: D3D11Texture2DDesc = std::mem::zeroed();
    get_desc(back_buffer, &mut desc);
    if desc.width == 0 || desc.height == 0 {
        com_release(back_buffer);
        return;
    }

    // ID3D11DeviceChild::GetDevice — slot 3
    let get_device: unsafe extern "system" fn(ComPtr, *mut ComPtr) = vtable_fn(back_buffer, 3);
    let mut device: ComPtr = std::ptr::null_mut();
    get_device(back_buffer, &mut device);
    if device.is_null() {
        com_release(back_buffer);
        return;
    }

    // ID3D11Device::GetImmediateContext — slot 40
    let get_ctx: unsafe extern "system" fn(ComPtr, *mut ComPtr) = vtable_fn(device, 40);
    let mut ctx: ComPtr = std::ptr::null_mut();
    get_ctx(device, &mut ctx);
    if ctx.is_null() {
        com_release(device);
        com_release(back_buffer);
        return;
    }

    // Create CPU-readable staging texture
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

    // ID3D11Device::CreateTexture2D — slot 5
    let create_tex: unsafe extern "system" fn(
        ComPtr, *const D3D11Texture2DDesc, *const (), *mut ComPtr,
    ) -> HRESULT = vtable_fn(device, 5);
    let mut staging: ComPtr = std::ptr::null_mut();
    if create_tex(device, &staging_desc, std::ptr::null(), &mut staging) != 0 || staging.is_null() {
        com_release(ctx);
        com_release(device);
        com_release(back_buffer);
        return;
    }

    // ID3D11DeviceContext::CopyResource — slot 47
    let copy_res: unsafe extern "system" fn(ComPtr, ComPtr, ComPtr) = vtable_fn(ctx, 47);
    copy_res(ctx, staging, back_buffer);

    // ID3D11DeviceContext::Map — slot 14
    let map_fn: unsafe extern "system" fn(
        ComPtr, ComPtr, u32, u32, u32, *mut D3D11MappedSubresource,
    ) -> HRESULT = vtable_fn(ctx, 14);
    let mut mapped: D3D11MappedSubresource = std::mem::zeroed();
    if map_fn(ctx, staging, 0, D3D11_MAP_READ, 0, &mut mapped) == 0 {
        let w = desc.width as usize;
        let h = desc.height as usize;
        let row_pitch = mapped.row_pitch as usize;
        let stride = w * 4;

        // Strip row padding into a contiguous BGRA buffer
        let mut pixels = Vec::with_capacity(stride * h);
        for row in 0..h {
            let src = std::slice::from_raw_parts(mapped.p_data.add(row * row_pitch), stride);
            pixels.extend_from_slice(src);
        }
        crate::ipc::send_frame(desc.width, desc.height, &pixels);

        // ID3D11DeviceContext::Unmap — slot 15
        let unmap_fn: unsafe extern "system" fn(ComPtr, ComPtr, u32) = vtable_fn(ctx, 15);
        unmap_fn(ctx, staging, 0);
    }

    com_release(staging);
    com_release(ctx);
    com_release(device);
    com_release(back_buffer);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract a typed function pointer from a COM object's vtable at `slot`.
#[inline]
unsafe fn vtable_fn<F: Copy>(obj: ComPtr, slot: usize) -> F {
    let vtable = *(obj as *mut *mut usize);
    std::mem::transmute_copy(&*vtable.add(slot))
}

/// Overwrite a vtable slot with a new function address, temporarily making the page writable.
unsafe fn patch_usize(slot: *mut usize, new_fn: usize) {
    let mut old: u32 = 0;
    VirtualProtect(slot as *const _, 8, PAGE_EXECUTE_READWRITE, &mut old);
    *slot = new_fn;
    let mut dummy: u32 = 0;
    VirtualProtect(slot as *const _, 8, old, &mut dummy);
}

/// Call `IUnknown::Release` (slot 2) on a COM object.
unsafe fn com_release(obj: ComPtr) {
    if !obj.is_null() {
        let release: unsafe extern "system" fn(ComPtr) -> u32 = vtable_fn(obj, 2);
        release(obj);
    }
}
