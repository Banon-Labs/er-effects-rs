//! In-world menu drive via the CS virtual-key layer (bd menu-input-layer-virtual-key-array-source-plus-
//! 0x88). The per-key array lives at `FD4PadDevice+0x88`, where the device is
//! `*(*(base+0x485dc20) + 0x18 + dev*8)` -- `FD4PadManager::padDevices[dev]`; index = `id-1000`, ids
//! 1000..1080, a `1` byte = "down this frame". It is rebuilt every frame from GLOBAL_DLUserInputManager
//! by the builders
//! (deobf FUN_140240f20/FUN_1402411e0 dump = deobf 0x140240e70/0x140241130, corrected 2026-07-23). Raw pad buttons (+0x890/+0x9f0) and inputmgr+0x90 are both
//! off the read path (proven at runtime across 3 cycles). So we MinHook the builders and, after the
//! original rebuilds the array, write our desired key id into `padDevices[dev]+0x88` (a pre-original write is
//! wiped by the rebuild). Edge-triggered: hold `1` one frame then `0` >=1 frame.
//!
//! The `id -> action` map (which of 1000..1080 = up/down/confirm/tab) is DLUID virtual-key numbering,
//! recovered empirically by the `probe` mode sweeping `set_vk_id` across the id range and watching the
//! menu respond.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use er_hook::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook};

use crate::log::harness_log;

// Corrected 2026-07-23 (bd ROOTCAUSE-padinject-builder-RVAs-are-wrong / corrected-inworld-input-writer):
// the prior RVAs (0x240dc0/0x241080/0x2634b0) were wrong -- they pointed at neighboring thunk stubs, so
// the MinHook detour never fired in-world (probe run4: builder_fires=0) and no injected input reached the
// menu. Recovered from the Ghidra dump: the writer 0x1426634a0's callers are the real builders
// 0x140240e70 / 0x140241130. Each builder loops ids 1000..0x438 and, for each down key, calls the
// writer with (device = padDevices[dev], id).
//
// RE-confirmed 2026-08-31 against the 1.16.2 dump on :8765, which is what the MCP now serves; the
// old "dump = deobf, shifted" bookkeeping above and the `dump-deobf-shift` reference that used to
// stand here are both obsolete (that tool was cross-version and has been deleted -- for 1.16.2 the
// dump VA, the deobf VA and the runtime VA are the same address). The three entries below are byte
// checks in `eldenring-deobf.bin` (builder prologue `mov [rsp+8],rcx; push rbp/rsi/rdi/r12`; writer
// `lea eax,[rdx-0x3e8]; cmp eax,0x50` = id-1000 bounds-checked 0..80), and all three carry held on
// 1.17: the builders sit at the same addresses with 195/195 instructions aligned, and the writer
// pairs to 0x142665cb0 byte-for-byte.
const BUILDER_A_RVA: usize = 0x240e70; // rebuilds padDevices[dev]+0x88, loops ids 1000..1080
const BUILDER_B_RVA: usize = 0x241130; // FUN_1402411e0 (dump): twin builder (second device/slot)
const WRITER_RVA: usize = 0x26634a0; // writes padDevices[dev]+0x88[id-1000]=1 per down key; 1.17 0x2665cb0
const FD4_PAD_MANAGER_RVA: usize = 0x485dc20;
/// `FD4PadManager::padDevices` -- `DLFixedVector<FD4PadDevice*,4>` at +0x18 (Ghidra structure), the
/// array the game's own key writer indexes. Element `dev` is at `manager + 0x18 + dev*8`.
///
/// The `DLFixedVector` accessor emits an alignment fudge, `(-(u32)(manager + 0x18)) & 7`, before the
/// index; `manager` is 8-byte-aligned heap, so that term is always 0 and the address really is
/// `manager + 0x18 + dev*8`. Witnessed held on 1.17 by builder A (0x140240e70, 195/195 instructions
/// aligned, base rsi/rcx = the manager).
const PAD_MGR_DEVICES_18_OFFSET: usize = 0x18;
/// `FD4PadManager::padDevices.count`. `padDevices` is 0x30 bytes at +0x18, so its count sits at
/// +0x40 -- which is where the game itself bounds-checks (`cmp rbp, [rcx+0x40]` at the head of every
/// builder). Also witnessed held on 1.17 by that same alignment.
const PAD_DEVICES_COUNT_40_OFFSET: usize = 0x40;
/// Highest device index the fixed vector can hold, per its `DLFixedVector<...,4>` declaration. Used
/// only to clamp a count read out of live memory.
const PAD_DEVICES_MAX: usize = 4;
/// The per-key "down this frame" array on `FD4::FD4PadDevice`; entry `id-1000` is at
/// `device + 0x88 + (id-1000)*2` (2-byte stride, low byte written).
///
/// Owner and value both RE-measured 2026-08-31 (bd
/// `vk-array-88-owner-is-FD4PadDevice-not-CSInGamePad-and-held-on-1170-2026-08-31`).
///
/// * The offset did not move on 1.17. The one function that writes this array, 1.16.2 0x1426634a0,
///   pairs to 1.17 0x142665cb0 and is byte-identical -- `mov byte [rcx+rdx*2+0x88],1` after a
///   `cmp eax,0x50` bound on `id-1000` -- so 0x88 is measured in both images, not carried.
/// * The owner is `FD4::FD4PadDevice`, not `CS::CSInGamePad`. All four call sites of that writer
///   (0x140240e70, 0x140241130, 0x140e321b0, 0x140e32470) load `rcx` from `padDevices[dev]`, with no
///   exception; `FD4PadManager::Init` fills that array with `HeapAlloc(0x3c0)` + `FD4PadDevice::
///   FD4PadDevice` + `FD4PadDevice::vftable`. `FD4PadDevice`'s constructor (0x142663880 ->
///   0x142666090) aligns 168/168 with zero moved offsets and its allocation size is still 0x3c0.
///
/// The `padMaps` accessor FUN_1402413f0 is deliberately not declared here any more: it returns the
/// CSInGamePad, which merely holds the device at `+0x10` (Ghidra names the type `CSInGamePad0x10`
/// after that field, and its constructor 0x1426647a0 does `param_1[2] = padDevices[dev]`), so it is
/// one indirection away from this array rather than a route to it.
///
/// That correction is load-bearing, because the previous code wrote this offset onto the
/// CSInGamePad from `padMaps` instead. The CSInGamePad is `HeapAlloc(0x98)` = 152 bytes, so
/// `0x88 + (id-1000)*2` leaves the object at id 1008 and every id above it wrote past the end of a
/// live game allocation -- silently, since the write is fault-safe and a heap overrun does not
/// fault. `scripts/check-object-field-offsets-1170.py` now pins this row.
const VK_ARRAY_88_OFFSET: usize = 0x88;
const VK_ID_MIN: u32 = 1000;
const VK_ID_MAX: u32 = 1080;
const HEAP_LO: usize = 0x10000;

/// The virtual-key id the drive currently wants held (0 = released). Set through `set_vk_id`; the
/// id->action map this was meant to be driven by was never recovered (see `set_vk_id`).
static DESIRED_VK_ID: AtomicU32 = AtomicU32::new(0);
static ORIG_BUILDER_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_BUILDER_B: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRITER: AtomicUsize = AtomicUsize::new(0);
static HOOKS_ACTIVE: AtomicUsize = AtomicUsize::new(0);
// Instrumentation (bd process-instrument-autonomously): did the hooks fire, and does my computed source
// match the game's real writer source? Answers the "wrong function / wrong object" questions with no
// user input.
static BUILDER_FIRES: AtomicU32 = AtomicU32::new(0);
static WRITER_FIRES: AtomicU32 = AtomicU32::new(0);
static GAME_SOURCE: AtomicUsize = AtomicUsize::new(0); // rcx of the real writer = the game's source
static MY_SOURCE: AtomicUsize = AtomicUsize::new(0); // source my inject_vk computed
static CACHED_PAD: AtomicUsize = AtomicUsize::new(0); // resolved FD4PadDevice, cached to skip per-frame RPM
/// One-shot latch for the "could not resolve a device" line. Without it the message would repeat
/// every frame, which is why the previous code said nothing at all -- and saying nothing is how the
/// drive stayed inert for six weeks with no fault, no refusal line and no counter moving.
static INERT_LOGGED: AtomicUsize = AtomicUsize::new(0);
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

/// Probe API: inject a raw virtual-key id (1000..1080) into `padDevices[dev]+0x88` each frame (0 = release).
///
/// This stays a raw-id API on purpose. A typed `PadButton` wrapper used to sit in front of it and was
/// removed 2026-08-21: the planned `padDevices[dev]+0x88` id -> action map was never recovered, so every one of
/// its variants mapped to id `0` and the enum carried no reverse-engineered information at all.
///
/// The evidence behind that is negative and specific (bd
/// `DECISIVE-source88-does-NOT-drive-pausemenu-fullsweep`): ids 1000..1080 were swept against the
/// in-world pause menu and none produced a reproducible job/flags/tab/return-title response. The menu
/// is driven through `inputmgr+0x90+eventId` (`crate::input_inject::tap_menu_event`) instead. This entry
/// point survives only for explicit raw-id diagnostics, should a later RE pass find the real consumer.
pub fn set_vk_id(id: u32) {
    DESIRED_VK_ID.store(id, Ordering::SeqCst);
}

/// After a builder rebuilds `padDevices[dev]+0x88`, stamp the desired key id down. `manager` is the builder's
/// first arg (GLOBAL_FD4PadManager); `dev` is its device index (edx).
/// `FD4PadDevice + 0x78` -- the pointer every menu-input read dereferences. It is a different struct
/// from the `+0x88` per-virtual-key array below, which is why sweeping ids 1000..1080 against the
/// pause menu drove nothing (bd decisive-source88-does-not-drive-pausemenu-fullsweep): that sweep was
/// the right idea aimed at the wrong field. Every reader on `CS::CSEzMenuViewerPad` goes through it --
/// `FUN_140e34fb0` reads `+0x08`, `FUN_140e35040` reads `+0x10`, `FUN_140e35080` reads `+0x28`.
const PAD_MENU_STATE_78_OFFSET: usize = 0x78;
/// The menu's list-scroll AXIS inside that struct. `FUN_140e35080` returns `*(int*)(state+0x28) /
/// 0x78`, so scrolling is an analog MAGNITUDE, not a button edge -- one row per `0x78` of value.
/// `FUN_14075d8f0` turns the result into a repeat count, and `FUN_140756000` picks the sign from the
/// CSPcKeyConfig binding id (9 = list down, 10 = list up, both measured live), so a single signed
/// write drives either direction.
const PAD_MENU_SCROLL_AXIS_28_OFFSET: usize = 0x28;
/// One row of list scroll, in raw axis units, before `FUN_140e35080` divides by it.
pub const PAD_MENU_SCROLL_UNIT: i32 = 0x78;

/// Raw axis value to stamp into the menu scroll field each frame (0 = neutral).
static DESIRED_MENU_AXIS: AtomicU32 = AtomicU32::new(0);
/// Last value observed in the axis field before we wrote it, so a run reports the game's own resting
/// value rather than leaving the field's meaning assumed. Local to this DLL (it does not link
/// er-telemetry-core), read back through `menu_axis_observed()`.
static PAD_MENU_AXIS_OBSERVED: AtomicU32 = AtomicU32::new(0);

// `menu_axis_observed()` is gone: it reported the value at padDevices[dev]+0x78+0x28, and that walk
// is the wrong object for menu input -- it returned 0x401c0000, an IEEE float, where the reader does
// an integer divide. `menu_scroll_reader_state()` replaces it by reporting the device the reader
// itself dereferenced, which needs no reimplementation of FUN_1402414a0's red-black walk.

/// Request a menu list-scroll axis value. `rows` is signed; 0 releases.
pub fn set_menu_scroll(rows: i32) {
    DESIRED_MENU_AXIS.store((rows * PAD_MENU_SCROLL_UNIT) as u32, Ordering::SeqCst);
}

/// Stamp the requested scroll axis into the struct the menu actually reads. Mirrors `inject_vk`'s
/// device walk and guarding; a null `+0x78` (no menu pad state yet) is a no-op.
unsafe fn inject_menu_axis(manager: usize, dev: usize) {
    let axis = DESIRED_MENU_AXIS.load(Ordering::SeqCst) as i32;
    let dev = dev & 0xffff_ffff;
    if manager < HEAP_LO {
        return;
    }
    let source = unsafe { *((manager + PAD_MGR_DEVICES_18_OFFSET + dev * 8) as *const usize) };
    if source < HEAP_LO {
        return;
    }
    let state = unsafe { *((source + PAD_MENU_STATE_78_OFFSET) as *const usize) };
    if state < HEAP_LO {
        return;
    }
    // SAFETY: `state` is the struct every CSEzMenuViewerPad reader dereferences, resolved the same way
    // they resolve it; `+0x28` is the int `FUN_140e35080` reads.
    let slot = (state + PAD_MENU_SCROLL_AXIS_28_OFFSET) as *mut i32;
    let observed = unsafe { *slot };
    PAD_MENU_AXIS_OBSERVED.store(observed as u32, Ordering::Relaxed);
    if axis != 0 {
        unsafe { *slot = axis };
    }
}

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
    // SAFETY: `source` is `padDevices[dev]`, the live `FD4PadDevice`, read exactly the way the game's
    // own writer computes its `this` (`mov rcx,[rcx+rsi+0x18]` at all four of its call sites);
    // +0x88+(id-1000)*2 is the per-key byte the
    // builder itself writes (RE-verified writer 0x1426634a0).
    unsafe {
        *((source + VK_ARRAY_88_OFFSET + ((id - VK_ID_MIN) as usize) * 2) as *mut u8) = 1;
    }
}

/// Per-frame direct stamp of `id` into `FD4PadDevice+0x88`, resolving the device from the game base
/// (bd decisive-builder-not-perframe-in-menu-need-perframe-direct-stamp). The builder that
/// `builder_*_hook` stamps after does not run per-frame while a menu is open (builder_fires stuck), so
/// builder-hook injection is too sparse to drive the menu; the array is read every frame, so the drive
/// must write it every frame. `id`=0 (or out of range) is a no-op release. Guarded by HEAP_LO on every
/// deref; never panics.
///
/// The device is `padDevices[dev]`, not the `padMaps` CSInGamePad. Between 2026-07-23 and 2026-08-31
/// this walked `padMaps` (+0x48) for a `CS::CSInGamePad_UserInput1` by TypeID and stamped that object's
/// +0x88. Every one of the four call sites of the game's own writer 0x1426634a0 loads `rcx` from
/// `manager + 0x18 + dev*8` instead, so the array is a field of `FD4::FD4PadDevice` and the tree-walk
/// was aimed at the wrong object -- one that is only `HeapAlloc(0x98)` = 152 bytes, so ids from 1008 up
/// wrote past the end of a live game allocation. It never fired in practice (the TypeID needles are
/// `.data` RVAs with no 1.17 mapping, so `game_data_addr` refused them and the search matched nothing),
/// which is the only reason the overrun was never observed rather than a reason it was safe.
/// `CS::CSEzMenuViewerPad` list-scroll axis reader -- 1.16.2 `0x140e35080`, 1.17 `0x140e36e80`
/// (mapped +0x1e00, unique 40-byte signature, and the 1.17 body was read: same
/// `*(int*)(*(this+0x10)+0x78)+0x28) / 0x78` shape). Hooking it is input delivery at the boundary the
/// game reads, the same shape as stamping the DInput keyboard buffer -- not a write of the outcome
/// the game would have computed.
const MENU_SCROLL_AXIS_READER_RVA: usize = 0xe35080;
static ORIG_MENU_SCROLL_READER: AtomicUsize = AtomicUsize::new(0);
/// The device pointer the reader dereferenced (`*(this+0x10)`), captured so the padMaps object the
/// menu actually uses can be identified without reimplementing FUN_1402414a0's red-black walk.
static MENU_PAD_DEVICE_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Raw value the reader found in the axis field, before any override.
static MENU_AXIS_RAW_SEEN: AtomicU32 = AtomicU32::new(0);
/// How many times the reader ran -- 0 means the menu never asked, so an override proves nothing.
static MENU_SCROLL_READER_CALLS: AtomicU32 = AtomicU32::new(0);

/// `(device, raw_axis, reader_calls)` observed at the menu's own axis read.
pub fn menu_scroll_reader_state() -> (usize, i32, u32) {
    (
        MENU_PAD_DEVICE_SEEN.load(Ordering::Relaxed),
        MENU_AXIS_RAW_SEEN.load(Ordering::Relaxed) as i32,
        MENU_SCROLL_READER_CALLS.load(Ordering::Relaxed),
    )
}

/// Detour: record what the game saw, then return our requested scroll when one is armed.
unsafe extern "system" fn menu_scroll_reader_hook(this: usize) -> i32 {
    MENU_SCROLL_READER_CALLS.fetch_add(1, Ordering::Relaxed);
    if this >= HEAP_LO
        && let Some(device) = unsafe { crate::win32::read_usize(this + 0x10) }
        && device >= HEAP_LO
    {
        MENU_PAD_DEVICE_SEEN.store(device, Ordering::Relaxed);
        if let Some(state) = unsafe { crate::win32::read_usize(device + PAD_MENU_STATE_78_OFFSET) }
            && state >= HEAP_LO
            && let Some(raw) =
                unsafe { crate::win32::read_usize(state + PAD_MENU_SCROLL_AXIS_28_OFFSET) }
        {
            MENU_AXIS_RAW_SEEN.store(raw as u32, Ordering::Relaxed);
        }
    }
    let requested = DESIRED_MENU_AXIS.load(Ordering::SeqCst) as i32;
    if requested != 0 {
        return requested / PAD_MENU_SCROLL_UNIT;
    }
    let orig = ORIG_MENU_SCROLL_READER.load(Ordering::SeqCst);
    if orig == 0 {
        return 0;
    }
    let f: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(orig) };
    unsafe { f(this) }
}

/// Pointer position, the fourth and last thing `CS::CSEzMenuViewerPad` exposes. `FUN_140e34ff0`
/// (1.17 `0x140e36df0`, body read: identical) returns `*(int*)(*(this+0x10)+0x78)+0x20)` as X and
/// `+0x24` as Y. It needs no hook of its own: those two ints live in the same struct as the buttons
/// and the axis, so once the device is known the whole menu input state is writable directly.
///
/// This is the field CS::GridControl actually consults. Its input wrappers (`FUN_140758a10`,
/// `FUN_1407589b0`, `FUN_140758950`) carry no code immediate at all -- unlike the tab pair and the
/// SpinCtrl pair they pass a bare lambda that interrogates the pad -- and the pause menu's tab
/// bindings read pad_primary=0 for both directions, so the grid is pointer-driven, not button-driven.
const PAD_MENU_POINTER_X_20_OFFSET: usize = 0x20;
const PAD_MENU_POINTER_Y_24_OFFSET: usize = 0x24;
/// Resting X/Y the game itself had in those fields, sampled before any write.
static MENU_POINTER_SEEN: AtomicU32 = AtomicU32::new(0);
static MENU_POINTER_SEEN_Y: AtomicU32 = AtomicU32::new(0);

/// The game's own pointer position as the menu sees it, sampled at the last observation.
pub fn menu_pointer_observed() -> (i32, i32) {
    (
        MENU_POINTER_SEEN.load(Ordering::Relaxed) as i32,
        MENU_POINTER_SEEN_Y.load(Ordering::Relaxed) as i32,
    )
}

/// `g_GxDrawContext` (1.16.2 RVA; maps to 0x47f33e0 on 1.17, agreed by 1178 references). Resolved
/// from `mov rax, [rip+0x4091bee]` at 0x14075d76b, inside FUN_14075d6e0 -- the same function that
/// corrects the menu pointer pair.
const GX_DRAW_CONTEXT_GLOBAL_RVA: usize = 0x47ef360;
/// The correction FUN_14075d6e0 applies: it subtracts the floats at `g_GxDrawContext+0x128 -> +0x110`
/// and `+0x114` from the raw pointer pair. Reading them is what turns the raw ints (-3, -29 measured)
/// into a space a coordinate can be chosen in, instead of writing screen pixels into a field that is
/// not screen pixels.
const GX_CONTEXT_INNER_128_OFFSET: usize = 0x128;
const GX_CORRECTION_X_110_OFFSET: usize = 0x110;
const GX_CORRECTION_Y_114_OFFSET: usize = 0x114;
static GX_CORRECTION_X: AtomicU32 = AtomicU32::new(0);
static GX_CORRECTION_Y: AtomicU32 = AtomicU32::new(0);

/// The two correction floats, as raw bits (reinterpret as f32).
pub fn menu_pointer_correction() -> (u32, u32) {
    (
        GX_CORRECTION_X.load(Ordering::Relaxed),
        GX_CORRECTION_Y.load(Ordering::Relaxed),
    )
}

/// Read the pointer correction the menu applies. Read-only.
pub fn sample_pointer_correction(base: usize) {
    if base < HEAP_LO {
        return;
    }
    let Some(ctx) = (unsafe {
        crate::win32::read_usize(er_game_base::mem::game_data_addr(
            base,
            GX_DRAW_CONTEXT_GLOBAL_RVA,
            "GX_DRAW_CONTEXT_GLOBAL_RVA",
        ))
    })
    .filter(|c| *c >= HEAP_LO) else {
        return;
    };
    let Some(inner) = (unsafe { crate::win32::read_usize(ctx + GX_CONTEXT_INNER_128_OFFSET) })
        .filter(|i| *i >= HEAP_LO)
    else {
        return;
    };
    if let Some(x) = unsafe { crate::win32::read_usize(inner + GX_CORRECTION_X_110_OFFSET) } {
        GX_CORRECTION_X.store(x as u32, Ordering::Relaxed);
    }
    if let Some(y) = unsafe { crate::win32::read_usize(inner + GX_CORRECTION_Y_114_OFFSET) } {
        GX_CORRECTION_Y.store(y as u32, Ordering::Relaxed);
    }
}

/// Sample the menu input struct on the device the axis reader captured. Read-ONLY: it reports what
/// the game has, which is the prerequisite for choosing coordinates instead of guessing them -- the
/// caller of `FUN_140e34ff0` subtracts `g_GxDrawContext+0x128 +0x110/+0x114` from the pair, so the
/// space these ints live in has to be observed, not assumed to be screen pixels.
pub fn sample_menu_pointer() {
    let device = MENU_PAD_DEVICE_SEEN.load(Ordering::Relaxed);
    if device < HEAP_LO {
        return;
    }
    let Some(state) = (unsafe { crate::win32::read_usize(device + PAD_MENU_STATE_78_OFFSET) })
        .filter(|s| *s >= HEAP_LO)
    else {
        return;
    };
    if let Some(x) = unsafe { crate::win32::read_usize(state + PAD_MENU_POINTER_X_20_OFFSET) } {
        MENU_POINTER_SEEN.store(x as u32, Ordering::Relaxed);
    }
    if let Some(y) = unsafe { crate::win32::read_usize(state + PAD_MENU_POINTER_Y_24_OFFSET) } {
        MENU_POINTER_SEEN_Y.store(y as u32, Ordering::Relaxed);
    }
}

/// Inject a virtual-key id into the menu's own pad device, not `padDevices`.
///
/// Why a second INJECTOR exists. `set_vk_id` writes `padDevices[dev]+0x88`, which is the right array
/// and the wrong device for menus: this repo already measured that the menu's device is not in
/// `padDevices` (`manager+0x18`) at all -- `FUN_1402414a0` resolves it by a red-black walk over
/// `padMaps` (`manager+0x48`). Injecting into `padDevices` therefore cannot reach a menu, and a
/// sweep of all 81 ids through it produced no cursor movement in the save-file picker while proving
/// nothing about the pad channel. `MENU_PAD_DEVICE_SEEN` is the device the axis reader hook actually
/// caught the game dereferencing, so it is the device the menu reads by observation rather than by
/// derivation.
///
/// `0` releases. Returns false when no menu device has been observed yet -- "not injected" and
/// "injected and ignored" must stay distinguishable, which is the whole lesson of the padDevices
/// sweep above.
pub fn set_menu_vk_id(id: u32) -> bool {
    let device = MENU_PAD_DEVICE_SEEN.load(Ordering::Relaxed);
    if device < HEAP_LO {
        return false;
    }
    // Same layout the game's own writer uses: `mov byte [rcx+rdx*2+0x88],1` after bounding
    // `id-1000` at 0x50, so the stride is 2 and only the low byte is written.
    for slot in 0..VK_ID_SPAN {
        let address = device + VK_ARRAY_88_OFFSET + slot * 2;
        let want = if id >= VK_ID_BASE && (id - VK_ID_BASE) as usize == slot {
            1u8
        } else {
            0u8
        };
        if want != 0 {
            unsafe { crate::win32::write_u8(address, want) };
        }
    }
    true
}

/// The id range the game's own writer bounds at (`cmp eax,0x50` on `id-1000`).
const VK_ID_BASE: u32 = 1000;
const VK_ID_SPAN: usize = 0x50;

/// Write the menu pointer the pause-menu cursor follows, and report whether the write landed.
///
/// Proven to be the right field by stimulus, not by inference (2026-09-05, br-20260905-174357-df99).
/// While the user nudged a real mouse across the open pause menu, `CS::GridControl` `0x8b370ab8`'s
/// selected cell at `+0xd4` tracked it through 5, 6, 1, 2, 4, 3, 2, 17 -- and every other live
/// GridControl in the process held still across all 30 samples. So the pause menu is
/// pointer-DRIVEN: the cursor is a hit-test of this coordinate pair, not a list index that a
/// direction key increments. That is why every axis and button write this module made was ignored.
///
/// The coordinates are in the same space `sample_menu_pointer` reads back, which is why the read
/// side had to exist first: the caller of `FUN_140e34ff0` subtracts the correction at
/// `g_GxDrawContext+0x128 +0x110/+0x114`, so "screen pixels" was an assumption worth refusing.
pub fn write_menu_pointer(x: i32, y: i32) -> bool {
    let device = MENU_PAD_DEVICE_SEEN.load(Ordering::Relaxed);
    if device < HEAP_LO {
        return false;
    }
    let Some(state) = (unsafe { crate::win32::read_usize(device + PAD_MENU_STATE_78_OFFSET) })
        .filter(|s| *s >= HEAP_LO)
    else {
        return false;
    };
    let wrote_x = unsafe { crate::win32::write_i32(state + PAD_MENU_POINTER_X_20_OFFSET, x) };
    let wrote_y = unsafe { crate::win32::write_i32(state + PAD_MENU_POINTER_Y_24_OFFSET, y) };
    wrote_x && wrote_y
}

/// The two `CS::CSEzMenuViewerPad` button readers, beside the axis one. 1.16.2 -> 1.17 pairs are in
/// the verified map and both 1.17 bodies were read: identical `*(byte*)(*(this+0x10)+0x78)+off)`
/// shape, sizes 49/49. `+0x08` is the one `FUN_14075d6e0` folds into bit 1 of its result and `+0x10`
/// into bit 4 -- which of those the menu treats as confirm is not assumed here; both are drivable and
/// a run says which one moves the pane.
const MENU_BUTTON_A_READER_RVA: usize = 0xe34fb0;
const MENU_BUTTON_B_READER_RVA: usize = 0xe35040;
static ORIG_MENU_BUTTON_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_MENU_BUTTON_B: AtomicUsize = AtomicUsize::new(0);
/// Which menu buttons the harness is holding: bit 0 = the `+0x08` reader, bit 1 = the `+0x10` one.
static DESIRED_MENU_BUTTONS: AtomicU32 = AtomicU32::new(0);
static MENU_BUTTON_READER_CALLS: AtomicU32 = AtomicU32::new(0);

/// Hold (or release, with 0) the menu buttons. Bit 0 = `+0x08`, bit 1 = `+0x10`.
pub fn set_menu_buttons(mask: u32) {
    DESIRED_MENU_BUTTONS.store(mask, Ordering::SeqCst);
}

/// How many times either button reader ran -- 0 means the menu never asked.
pub fn menu_button_reader_calls() -> u32 {
    MENU_BUTTON_READER_CALLS.load(Ordering::Relaxed)
}

fn menu_button_hook(this: usize, bit: u32, orig: &AtomicUsize) -> u64 {
    MENU_BUTTON_READER_CALLS.fetch_add(1, Ordering::Relaxed);
    if DESIRED_MENU_BUTTONS.load(Ordering::SeqCst) & bit != 0 {
        return 1;
    }
    let o = orig.load(Ordering::SeqCst);
    if o == 0 {
        return 0;
    }
    let f: unsafe extern "system" fn(usize) -> u64 = unsafe { std::mem::transmute(o) };
    unsafe { f(this) }
}

unsafe extern "system" fn menu_button_a_hook(this: usize) -> u64 {
    menu_button_hook(this, 1, &ORIG_MENU_BUTTON_A)
}

unsafe extern "system" fn menu_button_b_hook(this: usize) -> u64 {
    menu_button_hook(this, 2, &ORIG_MENU_BUTTON_B)
}

/// Install the axis-reader detour once. Idempotent; shares `install_one`'s idiom and guards.
pub fn install_menu_scroll_hook(base: usize) {
    if ORIG_MENU_SCROLL_READER.load(Ordering::SeqCst) != 0 || base < HEAP_LO {
        return;
    }
    if unsafe { MH_Initialize() } == MH_STATUS::MH_ERROR_MEMORY_ALLOC {
        return;
    }
    let mut queued = install_one(
        base,
        MENU_SCROLL_AXIS_READER_RVA,
        menu_scroll_reader_hook as *mut c_void,
        &ORIG_MENU_SCROLL_READER,
        "CSEzMenuViewerPad axis reader",
    );
    queued |= install_one(
        base,
        MENU_BUTTON_A_READER_RVA,
        menu_button_a_hook as *mut c_void,
        &ORIG_MENU_BUTTON_A,
        "CSEzMenuViewerPad button +0x08",
    );
    queued |= install_one(
        base,
        MENU_BUTTON_B_READER_RVA,
        menu_button_b_hook as *mut c_void,
        &ORIG_MENU_BUTTON_B,
        "CSEzMenuViewerPad button +0x10",
    );
    if queued {
        let _ = unsafe { MH_ApplyQueued() };
    }
}

// Deleted 2026-09-05: `stamp_menu_scroll_direct`. It resolved every device out of
// `FD4PadManager.padDevices` and wrote the scroll axis into each one -- and measured on
// br-20260905-234626-ce9a that write reaches nothing the menu reads: `menu_scroll_reader_hook`
// reported `raw_axis=0` on all 1,553 of its calls while the stamp ran every frame. The menu's device
// comes from the padMaps tree, not padDevices (bd
// menu-pad-device-comes-from-the-padmaps-tree-not-paddevices-2026-09-05), so the two were never the
// same object. What actually delivered the scroll was the `set_menu_scroll` call the function made on
// its way in, whose value the reader hook returns to the game. Callers therefore got input by side
// effect while believing it came from the write, which is the worst shape a helper can have. Call
// `set_menu_scroll` directly, and note that it is a held state: it is returned on every read until
// something sets it back to 0, so a caller that never releases is holding the direction down.

pub unsafe fn stamp_vk_direct(base: usize, id: u32, val: u8) {
    if !(VK_ID_MIN..=VK_ID_MAX).contains(&id) || base < HEAP_LO {
        return;
    }
    // Fault-safe reads (ReadProcessMemory pseudo-handle) so a wrong offset returns None instead of
    // crashing the game (raw derefs froze run10/run11).
    let rd = |p: usize| -> Option<usize> {
        if p < HEAP_LO {
            None
        } else {
            unsafe { crate::win32::read_usize(p) }
        }
    };
    let off = VK_ARRAY_88_OFFSET + ((id - VK_ID_MIN) as usize) * 2;
    // Cache the resolved device (bd BISECT-stamp_vk_direct-stops-drive): per-frame RPM walking stalls
    // the CSTaskImp task and stops the drive. Resolve once, then do one fault-safe write per frame.
    let cached = CACHED_PAD.load(Ordering::SeqCst);
    if cached >= HEAP_LO {
        unsafe {
            let _ = crate::win32::write_u8(cached + off, val);
        }
        return;
    }
    let Some(manager) = rd(er_game_base::mem::game_data_addr(
        base,
        FD4_PAD_MANAGER_RVA,
        "FD4_PAD_MANAGER_RVA",
    ))
    .filter(|m| *m >= HEAP_LO) else {
        report_inert("GLOBAL_FD4PadManager did not resolve or read back");
        return;
    };
    // The game bounds-checks `dev` against padDevices.count before every write; do the same, and clamp
    // to the vector's declared capacity so a garbage count cannot walk off the struct.
    let ndev = rd(manager + PAD_DEVICES_COUNT_40_OFFSET)
        .unwrap_or(1)
        .min(PAD_DEVICES_MAX);
    for dev in 0..ndev {
        let Some(device) =
            rd(manager + PAD_MGR_DEVICES_18_OFFSET + dev * 8).filter(|d| *d >= HEAP_LO)
        else {
            continue;
        };
        MY_SOURCE.store(device, Ordering::SeqCst);
        CACHED_PAD.store(device, Ordering::SeqCst);
        unsafe {
            let _ = crate::win32::write_u8(device + off, val);
        }
        harness_log!("pad-inject: padDevices[{dev}] = 0x{device:x} (vk array at +0x88)");
        return;
    }
    report_inert("no usable padDevices entry under the manager");
}

/// Say once that the in-world virtual-key drive could not resolve a device. A drive that quietly
/// does nothing is indistinguishable from a drive whose input the game ignored, and this path has
/// already produced that exact confusion once.
fn report_inert(why: &str) {
    if INERT_LOGGED
        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        harness_log!("pad-inject: {why}; the in-world virtual-key drive is INERT this run");
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
    unsafe { inject_menu_axis(manager, dev) };
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
    unsafe { inject_menu_axis(manager, dev) };
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
                // The hook must stay installed for the life of the process. `er_hook` declares no
                // `Drop` impl at all (uninstalling is `MH_RemoveHook`, which nothing here calls), so
                // letting the handle fall out of scope leaks it by construction -- the
                // `std::mem::forget` that used to sit here was a no-op dressed up as an intent.
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
            "pad-inject: virtual-key builder hooks active (inject into padDevices+0x88; a={a} b={b})"
        );
        true
    } else {
        harness_log!("pad-inject: MH_ApplyQueued failed or no hook");
        false
    }
}
