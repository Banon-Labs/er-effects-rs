//! Cross-DLL hook plumbing, copied from `er-reload-trace-dll`'s union path.
//!
//! The direct-input-memory writes (`input_inject`) must land on the GAME THREAD each frame, in the
//! same frame the engine re-polls the keystate bitmap. The harness gets a per-frame game-thread
//! callback by detouring a per-frame game function. Because the product DLL (`er_effects_rs.dll`) may
//! already hook the same game address, two independent MinHook instances patching one address corrupt
//! each other's trampolines -- so when the product is co-loaded the harness routes its detour through
//! the product's single MinHook instance via the `er_effects_union_register` export (product side:
//! crates/er-effects-rs/src/mh.rs). If the product is absent (standalone), it falls back to its own
//! MinHook instance (built by build.rs) -- no product means no shared address means no corruption.
//!
//! PER-FRAME ANCHOR (needs runtime validation -- `Do NOT run the game` was in effect at authoring):
//! the drive is hooked onto the menu selector tick `0x826d50` (`cap_selector_tick`, one of the
//! functions `er-reload-trace-dll` also hooks). That fires each frame WHILE A MENU/SELECTOR IS UP,
//! which is when the menu-nav injection must run. A per-frame anchor that also covers the in-world,
//! pre-menu window (to inject the escape-menu OPEN) is a separate open item -- see drive.rs.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::log::harness_log;
use crate::win32::{GetModuleHandleA, GetProcAddress, Sleep};

const MH_OK: i32 = 0;

/// The 4-arg `UnionFn`/detour ABI shared by the product union and MinHook (`mh.rs` `UnionFn`).
pub type HookFn = unsafe extern "system" fn(usize, usize, usize, usize) -> usize;
/// C-ABI of the product's `er_effects_union_register(target, handler, *mut orig_slot) -> i32`.
type UnionRegisterFn = unsafe extern "system" fn(usize, HookFn, *mut usize) -> i32;

const PRODUCT_DLL_NAME: &[u8] = b"er_effects_rs.dll\0";
const UNION_REGISTER_EXPORT: &[u8] = b"er_effects_union_register\0";
const UNION_RESOLVE_TRIES: u32 = 60;
const UNION_RESOLVE_SLEEP_MS: u32 = 50;

/// Per-frame anchor RVA (see module doc). Menu selector tick `cap_selector_tick_826d50`.
pub const DRIVE_ANCHOR_RVA: usize = 0x826d50;

/// Trampoline (or next chained handler) for the anchor. Read by the detour to call the original.
pub static ANCHOR_ORIG: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" {
    fn MH_Initialize() -> i32;
    fn MH_CreateHook(target: *mut c_void, detour: *mut c_void, original: *mut *mut c_void) -> i32;
    fn MH_EnableHook(target: *mut c_void) -> i32;
}

/// Call the anchor's original (trampoline or next chained union handler).
pub unsafe fn call_anchor_original(a: usize, b: usize, c: usize, d: usize) -> usize {
    let orig = ANCHOR_ORIG.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let f: HookFn = unsafe { std::mem::transmute(orig) };
    unsafe { f(a, b, c, d) }
}

fn resolve_union_register() -> Option<UnionRegisterFn> {
    for _ in 0..UNION_RESOLVE_TRIES {
        let hmod = unsafe { GetModuleHandleA(PRODUCT_DLL_NAME.as_ptr()) };
        if !hmod.is_null() {
            let proc = unsafe { GetProcAddress(hmod, UNION_REGISTER_EXPORT.as_ptr()) };
            if !proc.is_null() {
                // SAFETY: the export's C-ABI is fixed by the product DLL; both DLLs live for the
                // process lifetime so the pointer stays valid.
                return Some(unsafe { std::mem::transmute::<*mut c_void, UnionRegisterFn>(proc) });
            }
        }
        unsafe { Sleep(UNION_RESOLVE_SLEEP_MS) };
    }
    None
}

/// Install the per-frame drive detour: through the product union when co-loaded, else via an own
/// MinHook instance. `base` is the game image base; `detour` is the drive callback.
pub fn install_drive_hook(base: usize, detour: HookFn) {
    let target = base + DRIVE_ANCHOR_RVA;
    if let Some(reg) = resolve_union_register() {
        let rc = unsafe { reg(target, detour, ANCHOR_ORIG.as_ptr()) };
        if rc == 0 {
            harness_log!(
                "union: drive hook target=0x{target:x} (rva=0x{DRIVE_ANCHOR_RVA:x}) registered via product er_effects_union_register (chained)"
            );
        } else {
            harness_log!("union: drive hook target=0x{target:x} union register FAILED rc={rc}");
        }
        return;
    }
    harness_log!(
        "union: product export absent (standalone) -> own MinHook instance for the drive hook"
    );
    if unsafe { MH_Initialize() } != MH_OK {
        // ALREADY_INITIALIZED is also fine; any other error is logged and we bail.
    }
    let mut trampoline: *mut c_void = null_mut();
    let rc = unsafe {
        MH_CreateHook(
            target as *mut c_void,
            detour as *mut c_void,
            &mut trampoline,
        )
    };
    if rc != MH_OK {
        harness_log!("union: standalone MH_CreateHook target=0x{target:x} failed status={rc}");
        return;
    }
    ANCHOR_ORIG.store(trampoline as usize, Ordering::SeqCst);
    let en = unsafe { MH_EnableHook(target as *mut c_void) };
    harness_log!(
        "union: standalone drive hook target=0x{target:x} enable status={en} trampoline=0x{:x}",
        trampoline as usize
    );
}
