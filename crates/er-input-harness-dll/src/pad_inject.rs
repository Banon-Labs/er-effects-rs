//! In-world menu drive via the CS VIRTUAL-KEY layer (bd MENU-INPUT-LAYER-virtual-key-array-source-plus-
//! 0x88). The in-world Scaleform menu reads a per-key array at `source+0x88`, where
//! `source = *(*(base+0x485dc20)+0x18)` (FD4PadManager device 0); index = `id-1000`, ids 1000..1080, a
//! `1` byte = "down this frame". It is rebuilt EVERY frame from GLOBAL_DLUserInputManager by the builders
//! (deobf FUN_140240f20/FUN_1402411e0 dump = deobf 0x140240e70/0x140241130, CORRECTED 2026-07-23). Raw pad buttons (+0x890/+0x9f0) and inputmgr+0x90 are BOTH
//! off the read path (proven at runtime across 3 cycles). So we MinHook the builders and, AFTER the
//! original rebuilds the array, write our desired key id into `source+0x88` (a pre-original write is
//! wiped by the rebuild). Edge-triggered: hold `1` one frame then `0` >=1 frame.
//!
//! The `id -> action` map (which of 1000..1080 = up/down/confirm/tab) is DLUID virtual-key numbering,
//! recovered empirically by the `probe` mode sweeping `set_vk_id` across the id range and watching the
//! menu respond.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

use crate::log::harness_log;

// CORRECTED 2026-07-23 (bd ROOTCAUSE-padinject-builder-RVAs-are-wrong / CORRECTED-inworld-input-writer):
// the prior RVAs (0x240dc0/0x241080/0x2634b0) were WRONG -- they pointed at neighboring thunk stubs, so
// the MinHook detour never fired in-world (probe run4: builder_fires=0) and no injected input reached the
// menu. Recovered from the Ghidra dump via the mcp_bridge: the writer FUN_142663490 (dump)'s callers are
// the real builders FUN_140240f20 / FUN_1402411e0 (dump). Each builder loops ids 1000..0x438 and, for
// each down key, calls the writer FUN_142663490 with (device=source, id). dump-deobf-shift + prologue
// disasm confirm the deobf entries below (builder prologue `mov [rsp+8],rcx; push rbp/rsi/rdi/r12`;
// writer `lea eax,[rdx-0x3e8]; cmp eax,0x50` = id-1000 bounds-checked 0..80).
const BUILDER_A_RVA: usize = 0x240e70; // FUN_140240f20 (dump): rebuilds source+0x88, loops ids 1000..1080
const BUILDER_B_RVA: usize = 0x241130; // FUN_1402411e0 (dump): twin builder (second device/slot)
const WRITER_RVA: usize = 0x26634a0; // FUN_142663490 (dump): writes source+0x88[id-1000]=1 per down key
const FD4_PAD_MANAGER_RVA: usize = 0x485dc20;
/// FUN_1402413f0 (deobf; dump FUN_1402414a0): CSInGamePad* accessor(FD4PadManager*, deviceIndex). A
/// padMaps MAP lookup (bounds-checked, returns 0 out of range) -- the correct way to get a device's
/// CSInGamePad, since padMaps is a tree not a flat array (bd CORRECTION-inworld-menu-injection-NOT-solved).
const CS_INGAME_PAD_ACCESSOR_RVA: usize = 0x2413f0;
const PAD_MGR_DEVICES_18_OFFSET: usize = 0x18;
const VK_ARRAY_88_OFFSET: usize = 0x88;
const VK_ID_MIN: u32 = 1000;
const VK_ID_MAX: u32 = 1080;
const HEAP_LO: usize = 0x10000;

/// The virtual-key id the drive currently wants held (0 = released). The probe sets a raw id; the drive
/// sets it via `set_pad_button` once the id->action map is known.
static DESIRED_VK_ID: AtomicU32 = AtomicU32::new(0);
static ORIG_BUILDER_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_BUILDER_B: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRITER: AtomicUsize = AtomicUsize::new(0);
static HOOKS_ACTIVE: AtomicUsize = AtomicUsize::new(0);
// Instrumentation (bd PROCESS-instrument-autonomously): did the hooks fire, and does my computed source
// match the game's real writer source? Answers the "wrong function / wrong object" questions with no
// user input.
static BUILDER_FIRES: AtomicU32 = AtomicU32::new(0);
static WRITER_FIRES: AtomicU32 = AtomicU32::new(0);
static GAME_SOURCE: AtomicUsize = AtomicUsize::new(0); // rcx of the real writer = the game's source
static MY_SOURCE: AtomicUsize = AtomicUsize::new(0); // source my inject_vk computed
static OBSERVED_IDS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)]; // 81-bit set of ids the game's writer fired (id-1000)

/// Snapshot for the probe/drive to log: (builder_fires, writer_fires, game_source, my_source, obs_ids).
pub fn pad_snapshot() -> (u32, u32, usize, usize, [u32; 3]) {
    (
        BUILDER_FIRES.load(Ordering::SeqCst),
        WRITER_FIRES.load(Ordering::SeqCst),
        GAME_SOURCE.load(Ordering::SeqCst),
        MY_SOURCE.load(Ordering::SeqCst),
        [
            OBSERVED_IDS[0].load(Ordering::SeqCst),
            OBSERVED_IDS[1].load(Ordering::SeqCst),
            OBSERVED_IDS[2].load(Ordering::SeqCst),
        ],
    )
}

/// Menu-nav buttons -> DLUID virtual-key ids. UNKNOWN until the probe sweep discovers them; filled in
/// after the id->action map is recovered. Until then only the probe (`set_vk_id`) can drive.
#[derive(Clone, Copy)]
pub enum PadButton {
    None,
    Up,
    Down,
    Confirm,
    Cancel,
    TabLeft,
    TabRight,
}

impl PadButton {
    fn vk_id(self) -> u32 {
        // TODO(id-map): fill from the probe id-sweep discovery (bd MENU-INPUT-LAYER...).
        match self {
            PadButton::None => 0,
            _ => 0,
        }
    }
}

/// Drive API: set the button to inject (`None` = release, for a clean edge). No-op until the id map is
/// filled in `PadButton::vk_id`.
pub fn set_pad_button(button: PadButton) {
    DESIRED_VK_ID.store(button.vk_id(), Ordering::SeqCst);
}

/// Probe API: inject a RAW virtual-key id (1000..1080) into `source+0x88` each frame (0 = release).
pub fn set_vk_id(id: u32) {
    DESIRED_VK_ID.store(id, Ordering::SeqCst);
}

pub fn pad_hook_active() -> bool {
    HOOKS_ACTIVE.load(Ordering::SeqCst) != 0
}

/// After a builder rebuilds `source+0x88`, stamp the desired key id down. `manager` is the builder's
/// first arg (GLOBAL_FD4PadManager); `dev` is its device index (edx).
unsafe fn inject_vk(manager: usize, dev: usize) {
    let id = DESIRED_VK_ID.load(Ordering::SeqCst);
    if !(VK_ID_MIN..=VK_ID_MAX).contains(&id) {
        return;
    }
    let dev = dev & 0xffff_ffff;
    if manager < HEAP_LO {
        return;
    }
    let source = unsafe { *((manager + PAD_MGR_DEVICES_18_OFFSET + dev * 8) as *const usize) };
    MY_SOURCE.store(source, Ordering::SeqCst);
    if source < HEAP_LO {
        return;
    }
    // SAFETY: `source` is the live CSInGamePad "source"; +0x88+(id-1000)*2 is the per-key byte the
    // builder itself writes (RE-verified writer 0x1426634b0).
    unsafe {
        *((source + VK_ARRAY_88_OFFSET + ((id - VK_ID_MIN) as usize) * 2) as *mut u8) = 1;
    }
}

/// PER-FRAME DIRECT stamp of `id` into `source+0x88` (device 0), resolving the source from the game base
/// (bd DECISIVE-builder-not-perframe-in-menu-need-perframe-direct-stamp). The builder that `builder_*_hook`
/// stamps after does NOT run per-frame while a menu is open (builder_fires stuck), so builder-hook
/// injection is too sparse to drive the menu; the menu READS `source+0x88` every frame, so the drive must
/// WRITE it every frame. `source = *(*(base+FD4_PAD_MANAGER_RVA)+0x18)` (device 0). `id`=0 (or out of
/// range) is a no-op release. Guarded by HEAP_LO on every deref; never panics.
pub unsafe fn stamp_vk_direct(base: usize, id: u32) {
    if !(VK_ID_MIN..=VK_ID_MAX).contains(&id) || base < HEAP_LO {
        return;
    }
    let manager = unsafe { *((base + FD4_PAD_MANAGER_RVA) as *const usize) };
    if manager < HEAP_LO {
        return;
    }
    // padMaps is a MAP (red-black tree), NOT a flat array, so `*(manager+0x18+dev*8)` resolves the WRONG
    // object and writes never reach the menu. CALLING the game accessor FUN_1402413f0(manager,dev) DID
    // resolve the real pad (run9 msrc=0x1bdedf31200) but FROZE the game after one frame -- the accessor
    // takes an input-system lock our CSTaskImp task deadlocks on (bd CORRECTION-inworld-menu-injection-NOT-
    // solved / accessor-call-hangs). So neither path drives the menu yet. Keep the (harmless) device-0
    // flat write as a placeholder; the real fix is to SAFELY replicate the padMaps lookup in Rust (read the
    // tree, no game call) OR find the higher menu-input layer the pause menu actually reads.
    let _ = CS_INGAME_PAD_ACCESSOR_RVA;
    let source = unsafe { *((manager + PAD_MGR_DEVICES_18_OFFSET) as *const usize) };
    if source < HEAP_LO {
        return;
    }
    MY_SOURCE.store(source, Ordering::SeqCst);
    unsafe {
        *((source + VK_ARRAY_88_OFFSET + ((id - VK_ID_MIN) as usize) * 2) as *mut u8) = 1;
    }
}

unsafe extern "system" fn builder_a_hook(manager: usize, dev: usize, c: usize, d: usize) -> usize {
    BUILDER_FIRES.fetch_add(1, Ordering::SeqCst);
    let orig = ORIG_BUILDER_A.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(manager, dev, c, d) }
    } else {
        0
    };
    unsafe { inject_vk(manager, dev) };
    ret
}

unsafe extern "system" fn builder_b_hook(manager: usize, dev: usize, c: usize, d: usize) -> usize {
    BUILDER_FIRES.fetch_add(1, Ordering::SeqCst);
    let orig = ORIG_BUILDER_B.load(Ordering::SeqCst);
    let ret = if orig != 0 {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(manager, dev, c, d) }
    } else {
        0
    };
    unsafe { inject_vk(manager, dev) };
    ret
}

/// Instrumentation hook on the real per-key writer (FUN_1426634a0): captures the game's actual `source`
/// (rcx) and the ids it writes (edx), so we can compare to our computed source and see the id map.
unsafe extern "system" fn writer_hook(source: usize, id: usize, c: usize, d: usize) -> usize {
    WRITER_FIRES.fetch_add(1, Ordering::SeqCst);
    GAME_SOURCE.store(source, Ordering::SeqCst);
    let vid = (id & 0xffff_ffff) as u32;
    if (VK_ID_MIN..=VK_ID_MAX).contains(&vid) {
        let rel = vid - VK_ID_MIN;
        let word = (rel / 32) as usize;
        if word < 3 {
            OBSERVED_IDS[word].fetch_or(1u32 << (rel % 32), Ordering::SeqCst);
        }
    }
    let orig = ORIG_WRITER.load(Ordering::SeqCst);
    if orig != 0 {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(source, id, c, d) }
    } else {
        0
    }
}

fn install_one(
    base: usize,
    rva: usize,
    detour: *mut c_void,
    orig: &AtomicUsize,
    name: &str,
) -> bool {
    let addr = (base + rva) as *mut c_void;
    match unsafe { MhHook::new(addr, detour) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if unsafe { hook.queue_enable() }.is_ok() {
                std::mem::forget(hook);
                harness_log!("pad-inject: hooked {name} at 0x{:x}", addr as usize);
                true
            } else {
                harness_log!("pad-inject: {name} queue_enable failed");
                false
            }
        }
        Err(status) => {
            harness_log!("pad-inject: {name} MhHook::new failed: {status:?}");
            false
        }
    }
}

/// Install the virtual-key builder hooks once. Returns true when active.
pub fn install_pad_poll_hook(base: usize) -> bool {
    let _ = FD4_PAD_MANAGER_RVA; // manager arg is passed to the builders; global kept for reference
    if HOOKS_ACTIVE.load(Ordering::SeqCst) != 0 {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            harness_log!("pad-inject: MH_Initialize failed: {status:?}");
            return false;
        }
    }
    let a = install_one(
        base,
        BUILDER_A_RVA,
        builder_a_hook as *mut c_void,
        &ORIG_BUILDER_A,
        "builder_a",
    );
    let b = install_one(
        base,
        BUILDER_B_RVA,
        builder_b_hook as *mut c_void,
        &ORIG_BUILDER_B,
        "builder_b",
    );
    let w = install_one(
        base,
        WRITER_RVA,
        writer_hook as *mut c_void,
        &ORIG_WRITER,
        "writer(instrument)",
    );
    if (a || b || w) && matches!(unsafe { MH_ApplyQueued() }, MH_STATUS::MH_OK) {
        HOOKS_ACTIVE.store(1, Ordering::SeqCst);
        harness_log!(
            "pad-inject: virtual-key builder hooks active (inject into source+0x88; a={a} b={b})"
        );
        true
    } else {
        harness_log!("pad-inject: MH_ApplyQueued failed or no hook");
        false
    }
}
