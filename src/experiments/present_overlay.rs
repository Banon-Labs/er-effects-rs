//! D3D12 Present overlay -- draw the captured now-loading portrait directly onto the swapchain
//! backbuffer, bypassing the Scaleform/TexRepository pipeline entirely.
//!
//! The in-pipeline routes (forge bake, re-forge, CS-texture upload) cannot drive the DISPLAYED image:
//! the forge pre-binds before the portrait renders (timing race) and Scaleform samples its own decoded
//! GFx-renderer texture copy distinct from the CS-side texture we can reach (see bd
//! `postcontinue-portrait-EXHAUSTIVE-2026-06-30`). This is the sanctioned native D3D12/game-render-layer
//! path: hook `IDXGISwapChain::Present`, and when the now-loading screen is up, copy the captured portrait
//! over the backbuffer.
//!
//! Phase 1 (this commit): install the Present hook via the standard dummy-swapchain vtable technique and
//! log that it fires + the backbuffer format/dims. NO backbuffer writes yet (lowest crash risk) -- proves
//! the hook mechanism before adding the draw.

#![allow(unused_imports)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC, D3D12_COMMAND_QUEUE_FLAG_NONE,
    D3D12CreateDevice, ID3D12CommandQueue, ID3D12Device,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_UNSPECIFIED, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1,
    DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT, IDXGIFactory4, IDXGISwapChain,
    IDXGISwapChain1,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
    UnregisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::{IUnknown, Interface, w};

use crate::mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

use super::*;

/// Original `IDXGISwapChain::Present` / `Present1` trampolines; 0 until installed.
static PRESENT_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT1_ORIG: AtomicUsize = AtomicUsize::new(0);
static PRESENT_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Per-frame Present hit counter (RAM semaphore that the overlay hook is live + firing).
pub(crate) static PRESENT_HOOK_HITS: AtomicUsize = AtomicUsize::new(0);

/// `IDXGISwapChain::Present` vtable index: IUnknown(3) + IDXGIObject(4) + IDXGIDeviceSubObject(1) = slot 8.
const PRESENT_VTABLE_INDEX: usize = 8;
/// `IDXGISwapChain1::Present1` vtable index (Present(8)..GetLastPresentCount(17), GetDesc1(18)..Present1(22)).
const PRESENT1_VTABLE_INDEX: usize = 22;

type PresentFn = unsafe extern "system" fn(*mut c_void, u32, u32) -> i32;
type Present1Fn = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> i32;

/// `GLOBAL_CSGraphics` (the GX device singleton) deobf RVA -- from the dump (0x143d71c48, stored by
/// `FUN_140b1dc40`). The 0x130-byte CS::CSGraphicsImp it points to holds the live IDXGISwapChain several
/// sub-objects deep.
const GLOBAL_CSGRAPHICS_RVA: usize = 0x3d71c48;
/// Set once we've found the GAME's swapchain and hooked its REAL Present/Present1 (the dummy-swapchain
/// vtable funcs differ under vkd3d-proton, so hooking those never fired -- we must hook the game's own).
static GAME_PRESENT_HOOKED: AtomicUsize = AtomicUsize::new(0);
static GAME_SWAPCHAIN_FIND_TRIES: AtomicUsize = AtomicUsize::new(0);
/// The dummy swapchain's Present addr -- only used as a same-module hint for the swapchain-vtable filter.
static PRESENT_RESOLVED_ADDR: AtomicUsize = AtomicUsize::new(0);

/// Detour for `IDXGISwapChain::Present(this, SyncInterval, Flags)`. Phase 1: log-only, then tail-call the
/// original. `this` IS the game's swapchain (we never created it), so a real overlay draws onto its
/// current backbuffer here. Must never panic (runs on the game's render thread every frame).
unsafe extern "system" fn present_hook(this: *mut c_void, sync: u32, flags: u32) -> i32 {
    let hits = PRESENT_HOOK_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: PRESENT(8) hook FIRED first-hit this=0x{:x}",
            this as usize
        ));
        unsafe { log_backbuffer_desc(this) };
    }
    let orig = PRESENT_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: PresentFn = unsafe { std::mem::transmute(orig) };
        unsafe { f(this, sync, flags) }
    } else {
        0
    }
}

/// Detour for `IDXGISwapChain1::Present1(this, SyncInterval, Flags, *DXGI_PRESENT_PARAMETERS)`.
unsafe extern "system" fn present1_hook(
    this: *mut c_void,
    sync: u32,
    flags: u32,
    params: *const c_void,
) -> i32 {
    let hits = PRESENT_HOOK_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    if hits == 1 {
        append_autoload_debug(format_args!(
            "present-overlay: PRESENT1(22) hook FIRED first-hit this=0x{:x}",
            this as usize
        ));
        unsafe { log_backbuffer_desc(this) };
    }
    let orig = PRESENT1_ORIG.load(Ordering::SeqCst);
    if orig != 0 {
        let f: Present1Fn = unsafe { std::mem::transmute(orig) };
        unsafe { f(this, sync, flags, params) }
    } else {
        0
    }
}

static FACTORY2_ORIG: AtomicUsize = AtomicUsize::new(0);
type Factory2Fn =
    unsafe extern "system" fn(u32, *const windows::core::GUID, *mut *mut c_void) -> i32;

/// Detour for the `dxgi.dll!CreateDXGIFactory2` EXPORT -- logs that the GAME created a DXGI factory AFTER
/// our hook installed (the timing precondition for catching its swapchain creation via the export chain).
unsafe extern "system" fn factory2_hook(
    flags: u32,
    riid: *const windows::core::GUID,
    out: *mut *mut c_void,
) -> i32 {
    let orig = FACTORY2_ORIG.load(Ordering::SeqCst);
    let hr = if orig != 0 {
        let f: Factory2Fn = unsafe { std::mem::transmute(orig) };
        unsafe { f(flags, riid, out) }
    } else {
        -1
    };
    let factory = if out.is_null() {
        0
    } else {
        (unsafe { *out }) as usize
    };
    append_autoload_debug(format_args!(
        "present-overlay: GAME called CreateDXGIFactory2 (export) -> hr={hr} factory=0x{factory:x} (export chain viable)"
    ));
    hr
}

/// Hook the `dxgi.dll!CreateDXGIFactory2` export (a fixed export address, reliable under Wine) to learn
/// whether the game creates its DXGI factory after our install -- the precondition for the export chain
/// (factory -> CreateSwapChainForHwnd -> swapchain -> Present) that catches the game's ACTUAL swapchain.
fn install_dxgi_factory_export_hook() {
    let dxgi = match unsafe { GetModuleHandleW(windows::core::w!("dxgi.dll")) } {
        Ok(h) => h,
        Err(_) => {
            append_autoload_debug(format_args!("present-overlay: dxgi.dll not loaded yet"));
            return;
        }
    };
    let proc = unsafe {
        windows::Win32::System::LibraryLoader::GetProcAddress(
            dxgi,
            windows::core::s!("CreateDXGIFactory2"),
        )
    };
    let Some(addr) = proc else {
        append_autoload_debug(format_args!(
            "present-overlay: GetProcAddress(CreateDXGIFactory2) failed"
        ));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, factory2_hook as *mut c_void) } {
        Ok(hook) => {
            FACTORY2_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            std::mem::forget(hook);
            append_autoload_debug(format_args!(
                "present-overlay: hooked dxgi.dll!CreateDXGIFactory2 export 0x{:x}",
                addr as usize
            ));
        }
        Err(e) => append_autoload_debug(format_args!(
            "present-overlay: hook CreateDXGIFactory2 export failed: {e:?}"
        )),
    }
}

/// Prep the Present overlay ONCE (early): init MinHook + build a throwaway dummy swapchain only to learn
/// the IDXGISwapChain vtable module (the same-module hint for the runtime swapchain scan). The dummy's own
/// vtable funcs are NOT hooked -- under vkd3d-proton the game's swapchain is a different object, so the
/// REAL Present hook is installed later by `try_install_game_present_hook` once the GX device is up.
pub(crate) fn install_present_overlay_hook() {
    if PRESENT_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "present-overlay: MH_Initialize failed: {status:?}"
            ));
            PRESENT_HOOK_INSTALLED.store(0, Ordering::SeqCst);
            return;
        }
    }
    // Module hint only: the dummy's Present addr identifies which module implements IDXGISwapChain, so the
    // runtime BFS can filter swapchain candidates by vtable-in-that-module.
    if let Some((present_addr, _)) = unsafe { resolve_present_addrs() } {
        PRESENT_RESOLVED_ADDR.store(present_addr, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "present-overlay: prepared (module hint Present=0x{present_addr:x}); scanning for game swapchain per-frame"
        ));
    } else {
        append_autoload_debug(format_args!(
            "present-overlay: prepared (no module hint; will filter by Wine-module window)"
        ));
    }
}

/// Build a throwaway COMPOSITION swapchain (no HWND/window needed) and read its `Present` vtable entry.
/// All resources are local and dropped at scope end; only the function pointer (shared across all
/// IDXGISwapChain instances) is kept.
unsafe fn resolve_present_addrs() -> Option<(usize, usize)> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let factory: IDXGIFactory4 = match CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) {
            Ok(f) => f,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateDXGIFactory2 failed: {e:?}"
                ));
                return None;
            }
        };
        let mut device_opt: Option<ID3D12Device> = None;
        if let Err(e) = D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut device_opt) {
            append_autoload_debug(format_args!(
                "present-overlay: D3D12CreateDevice failed: {e:?}"
            ));
            return None;
        }
        let device = device_opt?;
        let queue_desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            Priority: 0,
            Flags: D3D12_COMMAND_QUEUE_FLAG_NONE,
            NodeMask: 0,
        };
        let queue: ID3D12CommandQueue = match device.CreateCommandQueue(&queue_desc) {
            Ok(q) => q,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateCommandQueue failed: {e:?}"
                ));
                return None;
            }
        };
        // Hidden dummy window (Wine/vkd3d has no DirectComposition, so we need a real HWND).
        let hinstance = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: GetModuleHandleW failed: {e:?}"
                ));
                return None;
            }
        };
        let class_name = w!("ErEffectsOverlayDummyWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(dummy_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _atom = RegisterClassW(&wc);
        let hwnd = match CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            w!("er-effects-overlay"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            64,
            64,
            None,
            None,
            Some(hinstance.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateWindowExW failed: {e:?}"
                ));
                let _ = UnregisterClassW(class_name, Some(hinstance.into()));
                return None;
            }
        };
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: 64,
            Height: 64,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
            Flags: 0,
        };
        let swapchain_res = factory.CreateSwapChainForHwnd(&queue, hwnd, &desc, None, None);
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(class_name, Some(hinstance.into()));
        let swapchain: IDXGISwapChain1 = match swapchain_res {
            Ok(s) => s,
            Err(e) => {
                append_autoload_debug(format_args!(
                    "present-overlay: CreateSwapChainForHwnd failed: {e:?}"
                ));
                return None;
            }
        };
        // The COM object's first qword is the vtable pointer; read Present(8) + Present1(22).
        let obj = swapchain.as_raw() as *const *const usize;
        let vtable = *obj;
        let present_addr = *vtable.add(PRESENT_VTABLE_INDEX) as usize;
        let present1_addr = *vtable.add(PRESENT1_VTABLE_INDEX) as usize;
        append_autoload_debug(format_args!(
            "present-overlay: resolved Present=0x{present_addr:x} Present1=0x{present1_addr:x}"
        ));
        if present_addr > 0x10000 && present1_addr > 0x10000 {
            Some((present_addr, present1_addr))
        } else {
            None
        }
    }))
    .ok()
    .flatten()
}

/// Best-effort log of the swapchain's backbuffer dims/format (separate from the fired-log so a GetDesc1
/// failure doesn't hide that the hook fired).
unsafe fn log_backbuffer_desc(this: *mut c_void) {
    if this as usize <= 0x10000 {
        return;
    }
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Some(sc) = unsafe { IDXGISwapChain1::from_raw_borrowed(&this) } {
            if let Ok(desc) = unsafe { sc.GetDesc1() } {
                append_autoload_debug(format_args!(
                    "present-overlay: backbuffer {}x{} format={} buffers={}",
                    desc.Width, desc.Height, desc.Format.0, desc.BufferCount
                ));
            }
        }
    }));
}

/// True if `obj` is a live COM object that QIs as `IDXGISwapChain`. Borrow-wraps (no AddRef on `obj`);
/// the QI result is owned + dropped (its Release balances the QI AddRef), leaving the game object net 0.
unsafe fn is_idxgi_swapchain(obj: usize) -> bool {
    if obj < 0x10000 {
        return false;
    }
    let raw = obj as *mut c_void;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        match unsafe { IUnknown::from_raw_borrowed(&raw) } {
            Some(unk) => unk.cast::<IDXGISwapChain>().is_ok(),
            None => false,
        }
    }))
    .unwrap_or(false)
}

/// Find the GAME's live `IDXGISwapChain` by a bounded BFS over the GX device (`GLOBAL_CSGraphics`) object
/// tree: any reachable object whose vtable is in dxgi.dll AND QIs as IDXGISwapChain is it. Read-only
/// pointer-walking + a confirming QI; returns the swapchain object pointer.
unsafe fn find_game_swapchain(base: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gx = unsafe { safe_read_usize(base + GLOBAL_CSGRAPHICS_RVA) }.unwrap_or(0);
    if gx == 0 || gx == null {
        return None;
    }
    let dxgi = unsafe { module_range(b"dxgi.dll\0") };
    let hint = PRESENT_RESOLVED_ADDR.load(Ordering::SeqCst);
    let vt_ok = |v: usize| -> bool {
        if let Some((lo, hi)) = dxgi {
            lo <= v && v < hi
        } else {
            // same high-module window as the resolved Present (Wine modules at 0x6fff_xxxx_xxxx).
            hint != 0 && (v >> 24) == (hint >> 24)
        }
    };
    let mut visited: Vec<usize> = Vec::with_capacity(2048);
    let mut queue: Vec<(usize, u32)> = vec![(gx, 0)];
    let mut budget = 0u32;
    let mut dxgi_candidates = 0u32;
    while let Some((obj, depth)) = queue.pop() {
        if budget >= 24000 {
            break;
        }
        budget += 1;
        if obj < 0x10000 || obj == null || visited.contains(&obj) {
            continue;
        }
        visited.push(obj);
        let vt = match unsafe { safe_read_usize(obj) } {
            Some(v) => v,
            None => continue,
        };
        if vt_ok(vt) {
            dxgi_candidates += 1;
            if unsafe { is_idxgi_swapchain(obj) } {
                append_autoload_debug(format_args!(
                    "present-overlay: FOUND game swapchain 0x{obj:x} (vtable=0x{vt:x}) after {budget} objs depth={depth}"
                ));
                return Some(obj);
            }
        }
        if depth < 12 {
            let mut off = 0usize;
            while off < 0x300 {
                if let Some(v) = unsafe { safe_read_usize(obj + off) } {
                    if v >= 0x10000 && v < 0x8000_0000_0000 && (v & 7) == 0 {
                        queue.push((v, depth + 1));
                    }
                }
                off += 8;
            }
        }
    }
    // Throttled diagnostic (the scan runs every frame): why no swapchain found.
    let t = GAME_SWAPCHAIN_FIND_TRIES.load(Ordering::SeqCst);
    if t == 1 || t == 300 || t == 1200 {
        append_autoload_debug(format_args!(
            "present-overlay: scan miss try={t} gx=0x{gx:x} dxgi_module={} objs={budget} dxgi_vtable_candidates={dxgi_candidates} visited={}",
            dxgi.map(|(l, h)| format!("[0x{l:x},0x{h:x})"))
                .unwrap_or_else(|| "NONE".to_string()),
            visited.len()
        ));
    }
    None
}

/// Per-frame (from a recurring game task): once the GX device is up, find the GAME's swapchain and hook
/// its REAL Present(8)/Present1(22) -- the dummy-swapchain funcs differ under vkd3d-proton, so this is the
/// only way the hook fires on the game's frames. One-shot (latched on success); bounded retries.
pub(crate) unsafe fn try_install_game_present_hook(base: usize) {
    if !portrait_lookat_enabled()
        || PRESENT_HOOK_INSTALLED.load(Ordering::SeqCst) == 0
        || GAME_PRESENT_HOOKED.load(Ordering::SeqCst) != 0
    {
        return;
    }
    // Bound the find attempts (each is a budgeted BFS) so a never-appearing swapchain can't spin forever.
    let tries = GAME_SWAPCHAIN_FIND_TRIES.fetch_add(1, Ordering::SeqCst) + 1;
    if tries > 3600 {
        return;
    }
    let sc = match unsafe { find_game_swapchain(base) } {
        Some(s) => s,
        None => return,
    };
    let vt = match unsafe { safe_read_usize(sc) } {
        Some(v) => v,
        None => return,
    };
    let present8 = unsafe { safe_read_usize(vt + PRESENT_VTABLE_INDEX * 8) }.unwrap_or(0);
    let present22 = unsafe { safe_read_usize(vt + PRESENT1_VTABLE_INDEX * 8) }.unwrap_or(0);
    if present8 <= 0x10000 || present22 <= 0x10000 {
        return;
    }
    // Latch BEFORE hooking so a retry can't double-install.
    if GAME_PRESENT_HOOKED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    match unsafe { MhHook::new(present8 as *mut c_void, present_hook as *mut c_void) } {
        Ok(hook) => {
            PRESENT_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            std::mem::forget(hook);
        }
        Err(e) => append_autoload_debug(format_args!(
            "present-overlay: hook game Present failed: {e:?}"
        )),
    }
    match unsafe { MhHook::new(present22 as *mut c_void, present1_hook as *mut c_void) } {
        Ok(hook) => {
            PRESENT1_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            std::mem::forget(hook);
        }
        Err(e) => append_autoload_debug(format_args!(
            "present-overlay: hook game Present1 failed: {e:?}"
        )),
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "present-overlay: HOOKED game swapchain 0x{sc:x} Present8=0x{present8:x} Present1_22=0x{present22:x} (tries={tries})"
        )),
        status => append_autoload_debug(format_args!(
            "present-overlay: game-present MH_ApplyQueued failed: {status:?}"
        )),
    }
}

unsafe extern "system" fn dummy_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
