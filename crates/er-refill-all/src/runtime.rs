//! The windows-only half: the gate hook, the pad/keyboard read, and the native writes.
//!
//! # The gate is structural, not a check
//!
//! The requirement is that the hotkey "can only have an effect when the user is in the menu view
//! that would have any refillable item options". That menu is the storage box, and its dialog is
//! `CS::DepositoryDialog`. This DLL brackets that dialog's lifetime -- latching on its constructor
//! and clearing on its destructor -- and acts only while the latch is open. The dialog is genuinely
//! heap-owned rather than pooled (its scalar-deleting destructor calls `operator_delete(this,
//! 0x3190)`), so construction and destruction really are "opened" and "closed".
//!
//! # Why not the shared `MenuWindow::Update`
//!
//! The first version hooked `FUN_140745570` -- the `MenuWindow` update every dialog inherits -- and
//! identified the storage box by comparing `*this` against its vtable. That worked live, but it was
//! the wrong prologue to own, for two independent reasons:
//!
//! 1. **It is shared with every other menu window in the game, and MinHook binds one detour per
//!    address.** The second `MH_CreateHook` on an address gets `MH_ERROR_ALREADY_CREATED`; the
//!    loser reports installed, never runs, and logs nothing. `er-hook`'s own header records that as
//!    measured, not hypothetical: the product and `er-armament-icons` both detoured the Scaleform
//!    `file_open` prologue and the product ran an entire session with `installed = true` and
//!    `hits = 0`. A generic prologue is the most likely address in the game to be contended.
//! 2. **The hook union cannot carry it.** The union's shared signature is
//!    `extern "system" fn(usize, usize, usize, usize) -> usize`, but `MenuWindow::Update` is
//!    `(this, f32 delta, InputData*)` -- `delta` travels in **XMM1**, with RDX unused. The union
//!    never names XMM1, so routing that prologue through a Rust dispatcher would leave the frame
//!    delta riding in a volatile register the ABI does not model, for every menu in the game. The
//!    corruption that risks is worse than the collision it would prevent.
//!
//! So this hooks two `DepositoryDialog`-specific prologues instead, both of which take integer
//! arguments only and therefore fit the union's ABI exactly:
//!
//! | | address | signature | uniqueness |
//! |---|---|---|---|
//! | constructor | `0x1408d54a0` | `(this, SceneObjProxy*, u8) -> this` | one call site, the factory |
//! | scalar-deleting destructor | `0x1408d6430` | `(this, u64 flags) -> this` | vtable slot 1, this class only |
//!
//! Both are registered through [`er_hook::register_shared_hook`], which chains into
//! `er_quickload.dll`'s single union when the product is co-loaded and uses this DLL's own union
//! when it is not -- so no handler can ever be silently dropped, here or in another mod.
//!
//! Per-frame work does not need a hook at all: it runs from a `CSTaskImp` `FrameBegin` task, the
//! same way `er-enemynpc-effects` drives its sweep. That is also the thread `GetAsyncKeyState`
//! actually reports the user's keys on under Wine/Proton.

use std::sync::atomic::{AtomicU16, AtomicU64, AtomicUsize, Ordering};

use er_game_base::{
    mem::{game_module_base, game_rva_named, safe_read_usize},
    rva::{GAME_DATA_MAN_GLOBAL_RVA, REPLANISH_ITEMS_FROM_CHEST_RVA, SHOULD_REPLENISH_ITEM_RVA},
};

use er_hotkey_config::pad::{PadEdge, pad_chord_name};

use crate::{
    config,
    log::refill_log,
    mark::{INSERT_CEILING, MarkOutcome, next_target_state},
};

/// `CS::ItemReplenishStateTracker::SetState(tracker*, int* itemId, bool state)`. The absolute
/// setter, and the only function here that writes a new entry.
const SET_STATE_RVA: usize = 0x23dd80;

/// `GameDataMan + 0x8` -> `mainPlayerGameData`.
const GAME_DATA_MAN_MAIN_PLAYER_GAME_DATA_OFFSET: usize = 0x8;
/// `PlayerGameData + 0x2b0` (`equipGameData`, by value) `+ 0x338` (`itemReplenishStateTracker*`).
const PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET: usize = 0x5e8;

/// `CS::DepositoryDialog::DepositoryDialog(this, SceneObjProxy*, u8) -> this`. Integer arguments
/// only, and reached from exactly one call site (the dialog factory `FUN_1408d6470`), so owning
/// this prologue contends with nothing.
const DEPOSITORY_DIALOG_CTOR_RVA: usize = 0x8d54a0;
/// `DepositoryDialog`'s scalar-deleting destructor, `(this, u64 flags) -> this` -- vtable slot 1,
/// referenced by this class's vtable and nothing else. It calls `operator_delete(this, 0x3190)`,
/// which is also the evidence the dialog is heap-owned rather than pooled: construction and
/// destruction really do bracket "the storage box is open".
const DEPOSITORY_DIALOG_DTOR_RVA: usize = 0x8d6430;
/// `FUN_1408f13b0(DepositoryDialog*, int)` -- Rebuild the displayed item list.
///
/// Writing replenish state changes nothing on screen by itself. The vanilla toggle
/// (`FUN_1408d87d0`) calls `SetItemReplenishState` and then immediately calls this with `1`; it
/// tail-calls `FUN_1408f6dc0`, which resets the dialog's gaitem list and re-runs the row-build
/// lambda through vtable slots `0xf`/`0x10`. Without it the rows keep whatever refill icon they
/// were built with, so the tracker and the screen disagree -- which is indistinguishable, from the
/// player's side, from the hotkey having done nothing at all. Measured 2026-08-25: a run wrote all
/// 449 entries and the icons did not move.
const DEPOSITORY_DIALOG_REFRESH_RVA: usize = 0x8f13b0;

/// `CS::DepositoryDialog::vftable`. Assigned only by that class's constructor (verified: two
/// references in the image, both in `DepositoryDialog::DepositoryDialog`), so `*this == this`
/// is a sound identity test for "the storage box dialog is the one updating".
const DEPOSITORY_DIALOG_VFTABLE_RVA: usize = 0x2aebba0;

/// `ItemReplenishStateTracker.count`, a `longlong` after `ItemReplenishStateEntry[2048]`.
const TRACKER_COUNT_OFFSET: usize = 0x4008;
/// `ItemReplenishStateEntry { i32 ItemId; bool autoReplenish; }`, padded to 8.
const TRACKER_ENTRY_SIZE: usize = 8;
const TRACKER_ENTRY_AUTO_REPLENISH_OFFSET: usize = 4;

/// Goods item ids are `0x40000000 | rowId`; weapon ids are the row id unchanged. These are the
/// only two high nibbles `GetEquipParamReplenishType` accepts.
const GOODS_ITEM_ID_FLAG: u32 = 0x4000_0000;

static GAME_BASE: AtomicUsize = AtomicUsize::new(0);
static ORIG_DEPOSITORY_CTOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_DEPOSITORY_DTOR: AtomicUsize = AtomicUsize::new(0);
static HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// The live `DepositoryDialog*`, or 0 while the storage box is closed. The gate.
static LIVE_DEPOSITORY_DIALOG: AtomicUsize = AtomicUsize::new(0);

/// Frames the storage box was actually open, counted by the per-frame task.
static DEPOSITORY_FRAMES: AtomicU64 = AtomicU64::new(0);
/// Storage-box opens seen, so a gate that never opens is distinguishable from a dead hotkey.
static DEPOSITORY_OPENS: AtomicU64 = AtomicU64::new(0);
static CYCLES_RUN: AtomicU64 = AtomicU64::new(0);
/// Last pad sample, so the edge detector and the rebind seed share one read per frame.
static LAST_PAD_BUTTONS: AtomicU16 = AtomicU16::new(0);

type SetStateFn = unsafe extern "system" fn(usize, *mut i32, bool);
type ShouldReplenishItemFn = unsafe extern "system" fn(usize, *mut i32) -> bool;
type ReplanishItemsFromChestFn = unsafe extern "system" fn();
type DepositoryRefreshFn = unsafe extern "system" fn(*mut core::ffi::c_void, u32);

/// Per-frame state that only the game's menu thread touches.
///
/// Not behind a mutex: the update hook is called from the one thread that runs menu updates, and a
/// lock on a per-frame path would be paid every frame to protect against a second caller that does
/// not exist. Reached through a `static mut`-style cell rather than a `Mutex` for that reason.
struct Edges {
    pad: PadEdge,
    keyboard_was_down: bool,
}

static mut EDGES: Option<Edges> = None;

pub(crate) fn install(base: usize) {
    use er_hook::register_shared_hook;

    if HOOK_INSTALLED.swap(1, Ordering::SeqCst) == 1 {
        return;
    }
    GAME_BASE.store(base, Ordering::SeqCst);

    // Through the union, never a bare `MhHook`. MinHook binds one detour per address, and the
    // second `MH_CreateHook` on an address gets `MH_ERROR_ALREADY_CREATED`: the loser reports
    // installed, never runs, and logs nothing. `register_shared_hook` chains into the product
    // DLL's single union when `er_quickload.dll` is co-loaded and uses this DLL's own union when
    // it is not, so a standalone run behaves identically and no handler is ever dropped. Both
    // targets take integer arguments only, which is exactly what the union's four-`usize` ABI
    // models -- see the module header for why the `MenuWindow::Update` prologue could not be.
    //
    // DTOR first, CTOR second, and the order is load-bearing (2026-08-31).
    //
    // `register_shared_hook` enables immediately -- it is not MinHook's deferred queue -- so the
    // first successful registration in this loop is live the instant it returns. The pair is only
    // safe as a pair: the ctor latches `LIVE_DEPOSITORY_DIALOG` and the dtor is the sole code that
    // clears it. Registered ctor-first, a refused dtor address (which is exactly what 1.17 hands
    // back for an RVA with no verified mapping) left the ctor detour armed with no way to unlatch,
    // and the `Err` arm's `return` did not undo it -- `er_hook` has no unregister. The caller then
    // registers the `FrameBegin` task regardless of this function returning early, so `tick()` went
    // on calling `live_depository_dialog()` every frame and reading through a freed
    // `DepositoryDialog*` for the rest of the session. Only the vftable sanity read in
    // `live_depository_dialog` stood between that and acting on a dead object.
    //
    // With the dtor first the partial states are both inert: a refused dtor means the ctor is never
    // registered, so nothing ever latches; a refused ctor leaves the dtor live but harmless,
    // because its `== a` test can never match the 0 the latch keeps. Same class as
    // bd `one-refused-hook-must-not-abort-the-installer-2026-08-30`, reached through the
    // immediate-enable registrar instead of through `MH_ApplyQueued`.
    for (name, target, handler, slot) in [
        (
            // Raw `base + rva`, not `game_data_addr` -- matching the ctor row below.
            //
            // `register_shared_hook` takes its target unresolved, deliberately, and resolves it
            // exactly once in whichever image will own the detour (`er-hook/src/lib.rs`, the
            // comment beginning "UNRESOLVED, deliberately"). This row used to hand it
            // `game_data_addr(base, DEPOSITORY_DIALOG_DTOR_RVA, ..)`, which is
            // `resolve_game_address(base + rva).unwrap_or(0)` -- so the address was translated
            // 1.16.2 -> 1.17 here and then handed to a registrar that translates it again. The
            // second translation looks up a 1.17 address in a table keyed by 1.16.2 addresses:
            // it either refuses (the feature silently never installs) or, where a 1.17 address
            // happens to collide with a 1.16.2 key, lands on an unrelated function. Its own
            // sibling one row down always passed raw, so the two rows disagreed about the
            // contract of the API they both call.
            //
            // `scripts/check-double-resolved-hook-targets.py` is the gate for this class and did
            // not catch it: its taint follows `let` bindings, and this target is an element of an
            // array literal destructured by the `for` pattern, never bound to a local.
            "DepositoryDialog::dtor",
            base + DEPOSITORY_DIALOG_DTOR_RVA,
            depository_dtor_union as er_hook::UnionFn,
            &ORIG_DEPOSITORY_DTOR,
        ),
        (
            "DepositoryDialog::ctor",
            base + DEPOSITORY_DIALOG_CTOR_RVA,
            depository_ctor_union as er_hook::UnionFn,
            &ORIG_DEPOSITORY_CTOR,
        ),
    ] {
        match unsafe { register_shared_hook(target, handler, slot) } {
            Ok(route) => refill_log(format_args!(
                "install: {name} @0x{target:x} registered via {route:?}"
            )),
            Err(status) => {
                refill_log(format_args!(
                    "install: register_shared_hook({name} @0x{target:x}) failed: {status:?}"
                ));
                HOOK_INSTALLED.store(0, Ordering::SeqCst);
                return;
            }
        }
    }

    let config = config::init_config();
    // Safety: written once here, before the FrameBegin task that reads it is registered.
    unsafe {
        EDGES = Some(Edges {
            pad: PadEdge::new(config.pad()),
            keyboard_was_down: false,
        });
    }
    refill_log(format_args!(
        "install: gate = DepositoryDialog lifetime (ctor @0x{:x}, dtor @0x{:x}); \
         gamepad_hotkey={} hotkey={} refill_immediately={} config={}",
        er_game_base::mem::game_data_addr(
            base,
            DEPOSITORY_DIALOG_CTOR_RVA,
            "DEPOSITORY_DIALOG_CTOR_RVA"
        ),
        er_game_base::mem::game_data_addr(
            base,
            DEPOSITORY_DIALOG_DTOR_RVA,
            "DEPOSITORY_DIALOG_DTOR_RVA"
        ),
        pad_chord_name(config.pad()),
        config
            .keyboard()
            .map_or_else(|| "(none)".to_owned(), er_hotkey_config::chord_name),
        config.refill_immediately,
        config.config_path.display(),
    ));
}

/// The storage box opened: latch the dialog.
///
/// Union shape, so the arguments arrive as four `usize`. The game passes three
/// `(this, SceneObjProxy*, u8)`; the fourth is ignored, which is what the union's fixed ABI is for.
unsafe extern "system" fn depository_ctor_union(a: usize, b: usize, c: usize, d: usize) -> usize {
    let orig = ORIG_DEPOSITORY_CTOR.load(Ordering::SeqCst);
    // The original constructs the object; only after it returns is the vfptr installed and the
    // pointer worth latching.
    let this = if orig == 0 {
        a
    } else {
        let orig: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
        unsafe { orig(a, b, c, d) }
    };
    if this != 0 {
        LIVE_DEPOSITORY_DIALOG.store(this, Ordering::SeqCst);
        DEPOSITORY_OPENS.fetch_add(1, Ordering::Relaxed);
    }
    this
}

/// The storage box closed: clear the latch before the memory is freed.
///
/// Cleared first, then the original runs and `operator_delete`s the object. Doing it the other way
/// round would leave a window in which the per-frame task could read a freed dialog.
unsafe extern "system" fn depository_dtor_union(a: usize, b: usize, c: usize, d: usize) -> usize {
    if LIVE_DEPOSITORY_DIALOG.load(Ordering::SeqCst) == a {
        LIVE_DEPOSITORY_DIALOG.store(0, Ordering::SeqCst);
    }
    let orig = ORIG_DEPOSITORY_DTOR.load(Ordering::SeqCst);
    if orig == 0 {
        return a;
    }
    let orig: er_hook::UnionFn = unsafe { std::mem::transmute(orig) };
    unsafe { orig(a, b, c, d) }
}

/// The storage box, if it is open right now. `None` closed, and `None` for a latched pointer whose
/// vtable is no longer this class's -- a cheap belt against a dialog freed without its destructor
/// running through our hook.
fn live_depository_dialog(base: usize) -> Option<*mut core::ffi::c_void> {
    let dialog = LIVE_DEPOSITORY_DIALOG.load(Ordering::SeqCst);
    if dialog == 0 {
        return None;
    }
    let vfptr = unsafe { safe_read_usize(dialog) }?;
    (vfptr
        == er_game_base::mem::game_data_addr(
            base,
            DEPOSITORY_DIALOG_VFTABLE_RVA,
            "DEPOSITORY_DIALOG_VFTABLE_RVA",
        ))
    .then_some(dialog as *mut core::ffi::c_void)
}

/// One `FrameBegin` tick. Returns immediately unless the storage box is open.
///
/// A game task rather than a hook: per-frame work needs no prologue of its own, and this is the
/// thread `GetAsyncKeyState` actually reports the user's keys on under Wine/Proton.
pub(crate) fn tick() {
    let base = GAME_BASE.load(Ordering::SeqCst);
    if base == 0 {
        return;
    }
    // The gate.
    let Some(dialog) = live_depository_dialog(base) else {
        return;
    };
    DEPOSITORY_FRAMES.fetch_add(1, Ordering::Relaxed);

    // Sample the pad before the config reload, so a rebind can seed its latch from the buttons
    // that are down at that instant instead of clearing it and manufacturing a press.
    let buttons = read_pad_buttons();
    LAST_PAD_BUTTONS.store(buttons, Ordering::Relaxed);

    if let Some(update) = config::poll_reload() {
        config::log_update(&update);
    }
    let config = config::config();

    // Safety: the FrameBegin task runs on one thread, and this is the only code that touches edges.
    let edges = unsafe { (&raw mut EDGES).as_mut() }.and_then(Option::as_mut);
    let Some(edges) = edges else { return };

    if edges.pad.rebind(config.pad(), buttons) {
        refill_log(format_args!(
            "hotkey: gamepad binding now {}",
            pad_chord_name(config.pad())
        ));
    }

    let pad_pressed = edges.pad.feed(buttons);
    let keyboard_pressed = match config.keyboard() {
        Some(chord) => keyboard_edge(chord, &mut edges.keyboard_was_down),
        None => {
            edges.keyboard_was_down = false;
            false
        }
    };

    if pad_pressed || keyboard_pressed {
        let source = if pad_pressed { "gamepad" } else { "keyboard" };
        unsafe { run_cycle(base, dialog, source, config.refill_immediately) };
    }
}

/// Read controller 0's `wButtons`. Zero when no pad is connected or XInput is absent.
#[cfg(windows)]
fn read_pad_buttons() -> u16 {
    use windows::{
        Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
        core::PCSTR,
    };

    /// Resolved `XInputGetState`, or `PROC_ABSENT` once we know there is none.
    static XINPUT_GET_STATE: AtomicUsize = AtomicUsize::new(0);
    const PROC_ABSENT: usize = usize::MAX;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct XInputGamepadRaw {
        buttons: u16,
        left_trigger: u8,
        right_trigger: u8,
        thumb_lx: i16,
        thumb_ly: i16,
        thumb_rx: i16,
        thumb_ry: i16,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct XInputStateRaw {
        packet: u32,
        gamepad: XInputGamepadRaw,
    }
    type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XInputStateRaw) -> u32;

    let cached = XINPUT_GET_STATE.load(Ordering::SeqCst);
    if cached == PROC_ABSENT {
        return 0;
    }
    let proc: XInputGetStateFn = if cached == 0 {
        // The game loads XInput for its own gamepad support, so this resolves without a
        // LoadLibrary. If it is absent the session is keyboard-only and the pad binding is simply
        // unavailable -- not an error worth logging every frame.
        let mut found = 0usize;
        for dll in [c"xinput1_4.dll", c"xinput1_3.dll", c"xinput9_1_0.dll"] {
            let Ok(module) = (unsafe { GetModuleHandleA(PCSTR(dll.as_ptr().cast::<u8>())) }) else {
                continue;
            };
            if let Some(address) =
                unsafe { GetProcAddress(module, PCSTR(c"XInputGetState".as_ptr().cast::<u8>())) }
            {
                found = address as usize;
                break;
            }
        }
        if found == 0 {
            XINPUT_GET_STATE.store(PROC_ABSENT, Ordering::SeqCst);
            return 0;
        }
        XINPUT_GET_STATE.store(found, Ordering::SeqCst);
        unsafe { std::mem::transmute::<usize, XInputGetStateFn>(found) }
    } else {
        unsafe { std::mem::transmute::<usize, XInputGetStateFn>(cached) }
    };

    let mut state = XInputStateRaw::default();
    // ERROR_SUCCESS(0) == connected. Any other result means no pad in slot 0.
    if unsafe { proc(0, &raw mut state) } == 0 {
        state.gamepad.buttons
    } else {
        0
    }
}

#[cfg(not(windows))]
const fn read_pad_buttons() -> u16 {
    0
}

/// Keyboard edge for the optional chord.
///
/// Both bits of `GetAsyncKeyState` are used and the low one is not optional: it means "pressed
/// since the previous call on this thread", so it catches a press that happened and was released
/// between two frames. This runs on the game's menu thread, which is the only place the call is
/// reliable under Wine/Proton -- a dedicated poll thread measured 1089 polls for 5 observed
/// key-downs.
#[cfg(windows)]
fn keyboard_edge(chord: er_hotkey_config::keys::Chord, was_down: &mut bool) -> bool {
    use er_hotkey_config::keys::{MODIFIER_CTRL, MODIFIER_SHIFT};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    const HELD: u16 = 0x8000;
    const PRESSED_SINCE_LAST_CALL: u16 = 0x0001;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_SHIFT: i32 = 0x10;

    let held = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & HELD != 0;
    if (chord.modifiers & MODIFIER_CTRL != 0 && !held(VK_CONTROL))
        || (chord.needs_alt() && !held(VK_MENU))
        || (chord.modifiers & MODIFIER_SHIFT != 0 && !held(VK_SHIFT))
    {
        // A modifier is up. Drop the latch so releasing the trigger later is not read as a press.
        *was_down = false;
        return false;
    }
    let state = unsafe { GetAsyncKeyState(chord.vk as i32) } as u16;
    let down = state & HELD != 0;
    let edge = (down && !*was_down) || state & PRESSED_SINCE_LAST_CALL != 0;
    *was_down = down;
    edge
}

#[cfg(not(windows))]
const fn keyboard_edge(_chord: er_hotkey_config::keys::Chord, _was_down: &mut bool) -> bool {
    false
}

/// `GameDataMan -> mainPlayerGameData -> equipGameData.itemReplenishStateTracker`.
unsafe fn resolve_tracker(base: usize) -> Option<usize> {
    let game_data_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            GAME_DATA_MAN_GLOBAL_RVA,
            "GAME_DATA_MAN_GLOBAL_RVA",
        ))
    }?;
    let player =
        unsafe { safe_read_usize(game_data_man + GAME_DATA_MAN_MAIN_PLAYER_GAME_DATA_OFFSET) }?;
    unsafe { safe_read_usize(player + PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET) }
        .filter(|&tracker| tracker != 0)
}

/// Every item id the game would accept a replenish state for, read from the live param tables.
///
/// Live rather than a table generated offline, so a modded regulation stays correct. `rows()`
/// PANICS on a table that has not streamed in yet -- `get_param_file` does
/// `holder.get_res_cap(0).expect(..)` -- so both holders are checked first; that panic killed two
/// earlier catalog runs elsewhere in this workspace.
#[cfg(windows)]
fn eligible_item_ids() -> Vec<i32> {
    use eldenring::cs::{EquipParamGoods, EquipParamWeapon, SoloParam, SoloParamRepository};
    use fromsoftware_shared::FromStatic;

    fn holder_ready<P: SoloParam>(repo: &SoloParamRepository) -> bool {
        repo.solo_param_holders
            .get(P::INDEX as usize)
            .and_then(|holder| holder.get_res_cap(0))
            .is_some()
    }

    // Safety: `instance()` yields a reference only when the singleton is populated, and every row
    // below is read, never written.
    let Ok(repo) = (unsafe { SoloParamRepository::instance() }) else {
        return Vec::new();
    };
    if !holder_ready::<EquipParamGoods>(repo) || !holder_ready::<EquipParamWeapon>(repo) {
        return Vec::new();
    }

    let mut ids = Vec::new();
    // Weapons: `GetEquipParamReplenishType` takes the high nibble 0 branch, so the row id is the
    // item id unchanged. In the stock regulation these are the 71 ammunition rows.
    for (row_id, row) in repo.rows::<EquipParamWeapon>() {
        if row.auto_replenish_type() != 0 {
            ids.push(row_id as i32);
        }
    }
    // Goods: high nibble 4.
    for (row_id, row) in repo.rows::<EquipParamGoods>() {
        if row.auto_replenish_type() != 0 {
            ids.push((row_id | GOODS_ITEM_ID_FLAG) as i32);
        }
    }
    ids
}

#[cfg(not(windows))]
fn eligible_item_ids() -> Vec<i32> {
    Vec::new()
}

/// One press: work out which way to go, write it, and optionally refill straight away.
unsafe fn run_cycle(
    base: usize,
    dialog: *mut core::ffi::c_void,
    source: &str,
    refill_immediately: bool,
) {
    let Some(tracker) = (unsafe { resolve_tracker(base) }) else {
        refill_log(format_args!(
            "{source}: press ignored: no ItemReplenishStateTracker yet"
        ));
        return;
    };
    let ids = eligible_item_ids();
    if ids.is_empty() {
        refill_log(format_args!(
            "{source}: press ignored: param tables not streamed in yet (no eligible items)"
        ));
        return;
    }

    // Through the 1.17 gate, not `base + rva`: the map knowing where a function went is no help
    // if the call never asks it. A refusal here costs the press; calling the 1.16.2 address on a
    // build that moved the function costs the process.
    let (Ok(should_replenish_addr), Ok(set_state_addr)) = (
        game_rva_named(
            SHOULD_REPLENISH_ITEM_RVA as u32,
            "SHOULD_REPLENISH_ITEM_RVA",
        ),
        game_rva_named(SET_STATE_RVA as u32, "SET_STATE_RVA"),
    ) else {
        refill_log(format_args!(
            "{source}: press ignored: ShouldReplenishItem/SetState have no verified address for this build"
        ));
        return;
    };
    let should_replenish: ShouldReplenishItemFn =
        unsafe { std::mem::transmute(should_replenish_addr) };
    let set_state: SetStateFn = unsafe { std::mem::transmute(set_state_addr) };

    // Ask the game, rather than reading entries: `ShouldReplenishItem` applies the per-type
    // defaults for an item that has no entry at all (type 2 defaults on, type 1 off), so it is the
    // only answer that matches what the storage box would show.
    let mut currently_on = 0u32;
    for id in &ids {
        let mut probe = *id;
        if unsafe { should_replenish(tracker, &raw mut probe) } {
            currently_on += 1;
        }
    }
    let eligible = u32::try_from(ids.len()).unwrap_or(u32::MAX);
    let target = next_target_state(eligible, currently_on);

    let mut outcome = MarkOutcome {
        eligible,
        ..MarkOutcome::default()
    };
    for id in &ids {
        let mut probe = *id;
        if unsafe { should_replenish(tracker, &raw mut probe) } == target {
            outcome.unchanged += 1;
            continue;
        }
        // An entry that already exists is flipped in place: no insert, no capacity risk, and the
        // sorted order that `FindItem` binary-searches is left undisturbed.
        if unsafe { flip_existing_entry(tracker, *id, target) } {
            outcome.flipped += 1;
            continue;
        }
        // Otherwise the native setter inserts one -- and this is the path that can DLPanic.
        let count = unsafe { tracker_count(tracker) };
        if count >= INSERT_CEILING {
            outcome.skipped_full += 1;
            continue;
        }
        let mut write = *id;
        unsafe { set_state(tracker, &raw mut write, target) };
        outcome.inserted += 1;
    }

    let cycles = CYCLES_RUN.fetch_add(1, Ordering::SeqCst) + 1;
    refill_log(format_args!(
        "{source}: cycle#{cycles} -> {} | eligible={} flipped={} inserted={} unchanged={} \
         skipped_full={} tracker_count={} depository_frames={} depository_opens={}",
        if target { "REFILL ALL" } else { "NO REFILLS" },
        outcome.eligible,
        outcome.flipped,
        outcome.inserted,
        outcome.unchanged,
        outcome.skipped_full,
        unsafe { tracker_count(tracker) },
        DEPOSITORY_FRAMES.load(Ordering::Relaxed),
        DEPOSITORY_OPENS.load(Ordering::Relaxed),
    ));
    if outcome.skipped_full > 0 {
        refill_log(format_args!(
            "{source}: {} item(s) left unmarked: the replenish tracker is near its 2048-entry \
             limit and inserting past it DLPanics the game",
            outcome.skipped_full
        ));
    }

    // Marking alone moves nothing: the transfer is `ReplanishItemsFromChest`, which vanilla runs
    // at a grace or after a load.
    if refill_immediately && target && outcome.wrote_anything() {
        match game_rva_named(
            REPLANISH_ITEMS_FROM_CHEST_RVA as u32,
            "REPLANISH_ITEMS_FROM_CHEST_RVA",
        ) {
            Ok(address) => {
                let replenish: ReplanishItemsFromChestFn = unsafe { std::mem::transmute(address) };
                unsafe { replenish() };
                refill_log(format_args!(
                    "{source}: cycle#{cycles} ran ReplanishItemsFromChest"
                ));
            }
            Err(why) => refill_log(format_args!(
                "{source}: cycle#{cycles} could not run ReplanishItemsFromChest -- {why}"
            )),
        }
    }

    // Repaint. The rows were built before the write, so without this they keep showing the old
    // icons and the whole feature reads as inert -- which is exactly how it presented the first
    // time it was tested live. Vanilla calls the same function after its own single-item toggle,
    // so any cursor movement this causes is the game's own behaviour rather than something new.
    match game_rva_named(
        DEPOSITORY_DIALOG_REFRESH_RVA as u32,
        "DEPOSITORY_DIALOG_REFRESH_RVA",
    ) {
        Ok(address) => {
            let refresh: DepositoryRefreshFn = unsafe { std::mem::transmute(address) };
            unsafe { refresh(dialog, 1) };
            refill_log(format_args!(
                "{source}: cycle#{cycles} rebuilt the storage list"
            ));
        }
        // Without the repaint the rows keep showing the old icons and the feature reads as inert,
        // so say so rather than leaving the user to conclude the press did nothing.
        Err(why) => refill_log(format_args!(
            "{source}: cycle#{cycles} wrote the flags but could not rebuild the storage list -- {why}"
        )),
    }
}

/// Read `tracker->count`.
unsafe fn tracker_count(tracker: usize) -> u64 {
    unsafe { ((tracker + TRACKER_COUNT_OFFSET) as *const u64).read() }
}

/// Set `autoReplenish` on an entry that is already in the vector. False when there is none.
///
/// A linear scan rather than the game's binary search: 2048 entries is nothing next to a frame,
/// and reimplementing `FindItem`'s comparison (which is over unsigned ids, and whose end sentinel
/// carries an alignment fudge) would be a second place for that subtlety to be got wrong.
unsafe fn flip_existing_entry(tracker: usize, item_id: i32, state: bool) -> bool {
    let count = unsafe { tracker_count(tracker) };
    let count = usize::try_from(count.min(crate::mark::TRACKER_CAPACITY)).unwrap_or(0);
    for index in 0..count {
        let entry = tracker + index * TRACKER_ENTRY_SIZE;
        if unsafe { (entry as *const i32).read() } == item_id {
            unsafe {
                ((entry + TRACKER_ENTRY_AUTO_REPLENISH_OFFSET) as *mut bool).write(state);
            }
            return true;
        }
    }
    false
}

/// Wait for the game module, then install. Called once from `DllMain`.
/// Wait for the task scheduler. Only ever called from the spawned thread, never the loader lock.
fn wait_for_task_instance() -> Option<&'static eldenring::cs::CSTaskImp> {
    // `instance()` comes from this trait, not from the type.
    use fromsoftware_shared::FromStatic;

    // Bounded (2026-08-29). This was `loop { yield_now() }`. On 1.17 the singleton did not turn
    // up promptly and two such loops starved the wineserver: the game reached 104 CPU ticks in
    // three minutes while these threads burned 19,000 each, half of it system time. See
    // er_game_base::wait for the measurement.
    er_game_base::wait::poll_until(|| unsafe { eldenring::cs::CSTaskImp::instance() }.ok())
}

pub(crate) fn spawn(_module_base: usize) {
    use eldenring::cs::CSTaskGroupIndex;
    use eldenring::fd4::FD4TaskData;
    use fromsoftware_shared::SharedTaskImpExt;

    let _ = std::thread::Builder::new()
        .name("er-refill-all".to_owned())
        .spawn(move || {
            let mut attempts = 0u64;
            // Bounded (2026-08-29): an unbounded `loop { yield_now() }` in two other shells starved the
            // wineserver and hung a whole boot -- see er_game_base::wait. Same shape, same fix.
            let found = er_game_base::wait::poll_until(|| match game_module_base() {
                Ok(base) => Some(base),
                Err(err) => {
                    if attempts == 0 || attempts.is_multiple_of(4096) {
                        refill_log(format_args!("install: waiting for game module base: {err}"));
                    }
                    attempts = attempts.saturating_add(1);
                    None
                }
            });
            let Some(base) = found else {
                refill_log(format_args!(
                    "install: no game module base; nothing installed"
                ));
                return;
            };
            install(base);
            // Per-frame work is a game task, not a hook: it owns no prologue and so can contend
            // with nothing, and FrameBegin is a thread where `GetAsyncKeyState` actually reports
            // the user's keys under Wine/Proton.
            let Some(task) = wait_for_task_instance() else {
                refill_log(format_args!(
                    "install: CSTaskImp never appeared; this shell stays inert rather than spinning"
                ));
                return;
            };
            refill_log(format_args!("install: registering FrameBegin tick"));
            task.run_recurring(
                move |_data: &FD4TaskData| tick(),
                CSTaskGroupIndex::FrameBegin,
            );
        });
}
