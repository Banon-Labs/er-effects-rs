//! The native `05_010` list's own input and geometry, rebased so ten row models can represent a
//! listing of any length.
//!
//! Split out of `save_picker_menu` on 2026-09-11 when that file crossed the hard size limit. The
//! seam is not arbitrary: everything here is a detour on, or a direct read of, the game's own list
//! control -- the vertical menu-event ids, the cursor setter, the wheel delta, the point-to-index
//! hit test, and the grid's view base. The picker's own logic lives next door and calls into this.
//!
//! # Why the list is lied to rather than resized
//!
//! `ProfileSelect` stages exactly ten `ProfileSummary` rows, and the compact picker keeps it that
//! way: changing the native `GridControl` item count changes what the game believes about every
//! other surface built from the same template. So a long directory is a moving ten-row window over
//! the model, and these hooks are what keep the native control's idea of the cursor, the wheel and
//! the hit target consistent with a window that slides underneath it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::mem::{game_module_base, game_rva, game_rva_for_hook};
use er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;
use er_hook::{MH_STATUS, MhHook};
use er_title_flow::{INPUTMGR_BITMAP_90_OFFSET, MENU_EVENT_PRESSED_BIT};

use crate::host::append_autoload_debug;
use crate::save_picker_menu::*;

/// Learned `CSMenuManImp+0x90` event ids for vertical menu movement. `MoveA`(0x00) and `MoveB`(0x45)
/// are the two ids the vertical-move predicate reads, but which one is up and which is down is not
/// recorded anywhere -- so they are learned live, from a tick where exactly one id is set and
/// exactly one direction is pressed on a device. `MENU_EVENT_ID_UNLEARNED` until then.
pub(crate) static SAVE_PICKER_MENU_EVENT_DOWN_ID: AtomicUsize =
    AtomicUsize::new(MENU_EVENT_ID_UNLEARNED);
pub(crate) static SAVE_PICKER_MENU_EVENT_UP_ID: AtomicUsize =
    AtomicUsize::new(MENU_EVENT_ID_UNLEARNED);
pub(crate) const MENU_EVENT_ID_UNLEARNED: usize = usize::MAX;

/// A press deferred at an extreme row, waiting for the native wrap it is about to cause, and how
/// many pump ticks it may wait. Four ticks is generous for a wrap the list performs on the very
/// next frame, and short enough that an unredeemed press cannot resurface as a phantom step later.
pub(crate) static SAVE_PICKER_PENDING_WRAP_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_PENDING_WRAP_TICKS: AtomicUsize = AtomicUsize::new(0);
pub(crate) const PENDING_WRAP_MAX_TICKS: usize = 4;

/// Vertical menu events dropped at a listing limit; diagnostic only.
pub(crate) static SAVE_PICKER_LIMIT_SUPPRESSED_EVENTS: AtomicUsize = AtomicUsize::new(0);
/// Selection moves with no key/pad/wheel behind them, i.e. the pointer; diagnostic only.
pub(crate) static SAVE_PICKER_POINTER_CURSOR_MOVES: AtomicUsize = AtomicUsize::new(0);
/// Times the grid scrolled its own view during a select and had to be put back.
#[allow(dead_code)] // Retained: Picker diagnostic counter, beside the sibling counters that are live.
pub(crate) static SAVE_PICKER_GRID_VIEW_RESTORES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_GRID_GEOMETRY_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// The live `CSMenuManImp` keystate bitmap (`+0x90`), one byte per menu event id.
pub(crate) unsafe fn save_picker_menu_event_keystate() -> Option<*mut u8> {
    let base = game_module_base().ok()?;
    let inputmgr = unsafe {
        *((er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        )) as *const usize)
    };
    (inputmgr != 0).then(|| (inputmgr + INPUTMGR_BITMAP_90_OFFSET) as *mut u8)
}

/// Learn which vertical event id means down and which means up, from an unambiguous frame.
///
/// Ambiguous frames are skipped rather than guessed: getting this backwards would suppress the
/// direction that still has somewhere to go, which is worse than not suppressing at all.
pub(crate) unsafe fn save_picker_learn_vertical_menu_event_ids(down: bool, up: bool) {
    if down == up
        || SAVE_PICKER_MENU_EVENT_DOWN_ID.load(Ordering::SeqCst) != MENU_EVENT_ID_UNLEARNED
    {
        return;
    }
    let Some(keystate) = (unsafe { save_picker_menu_event_keystate() }) else {
        return;
    };
    let a_set = unsafe { *keystate.add(MENU_EVENT_MOVE_A_00) } & MENU_EVENT_PRESSED_BIT != 0;
    let b_set = unsafe { *keystate.add(MENU_EVENT_MOVE_B_45) } & MENU_EVENT_PRESSED_BIT != 0;
    if a_set == b_set {
        return;
    }
    let pressed_id = if a_set {
        MENU_EVENT_MOVE_A_00
    } else {
        MENU_EVENT_MOVE_B_45
    };
    let other_id = if a_set {
        MENU_EVENT_MOVE_B_45
    } else {
        MENU_EVENT_MOVE_A_00
    };
    let (down_id, up_id) = if down {
        (pressed_id, other_id)
    } else {
        (other_id, pressed_id)
    };
    SAVE_PICKER_MENU_EVENT_UP_ID.store(up_id, Ordering::SeqCst);
    SAVE_PICKER_MENU_EVENT_DOWN_ID.store(down_id, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker: learned vertical menu event ids down=0x{down_id:x} up=0x{up_id:x}"
    ));
}

pub(crate) static SAVE_PICKER_SET_CURSOR_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_SET_CURSOR_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_SET_CURSOR_NEUTRALISED: AtomicUsize = AtomicUsize::new(0);
/// Wheel detents the native grid refused (view base at a clamp) that this pump stepped instead.
pub(crate) static SAVE_PICKER_WHEEL_NATIVE_STEPS: AtomicUsize = AtomicUsize::new(0);

/// `FUN_14073bc10` detour: neutralise the ensure-visible base for every select on the picker's list.
///
/// The wheel step can zero the base around its own call, but the game makes this call itself on
/// every mouse hover and click, and those resets are what re-orient the list under a stationary
/// pointer. Hooking is the only place to reach them: the base and the index are in different spaces
/// (scrollbar model-space vs view-space 0..9) and only this function compares the two.
pub(crate) unsafe extern "system" fn save_picker_set_cursor_hook(list: usize, index: u32) -> u64 {
    let orig_addr = SAVE_PICKER_SET_CURSOR_ORIG.load(Ordering::SeqCst);
    if orig_addr == 0 {
        return 0;
    }
    let orig: unsafe extern "system" fn(usize, u32) -> u64 =
        unsafe { std::mem::transmute(orig_addr) };
    // Only the picker's own list, and only while the picker owns the screen: every other menu in
    // the game uses this grid the way it was designed and must keep its native scrolling.
    let dialog = save_picker_live_profile_dialog();
    let ours = dialog != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
        && list == dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET;
    if !ours {
        return unsafe { orig(list, index) };
    }
    let before = unsafe { save_picker_grid_view_base(list) };
    if before != (0, 0) {
        unsafe { save_picker_set_grid_view_base(list, (0, 0)) };
    }
    let ret = unsafe { orig(list, index) };
    let after = unsafe { save_picker_grid_view_base(list) };
    if after != before {
        unsafe { save_picker_set_grid_view_base(list, before) };
        let n = SAVE_PICKER_SET_CURSOR_NEUTRALISED.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 20 || n.is_multiple_of(50) {
            append_autoload_debug(format_args!(
                "save-picker: native select neutralised #{n} index={index} view {before:?} (call left {after:?})"
            ));
        }
    }
    ret
}

pub fn install_save_picker_set_cursor_hook() {
    if SAVE_PICKER_SET_CURSOR_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Ok(addr) = game_rva_for_hook(MENU_ITEM_LIST_SET_CURSOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve select-index rva 0x{MENU_ITEM_LIST_SET_CURSOR_RVA:x}"
        ));
        SAVE_PICKER_SET_CURSOR_HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_set_cursor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_SET_CURSOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable select-index failed: {status:?}"
                ));
                return;
            }
            match unsafe { er_hook::MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked list select-index FUN_14073bc10 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: select-index MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new select-index failed: {status:?}"
        )),
    }
}

/// `FUN_140757c70` -- the only place the grid reads a wheel notch. Byte-verified unique in the
/// 1.16.2 deobf image at `0x140757c70` (`48 89 5c 24 08 57 48 83 ec 20 48 8b da 48 8b f9 ba 2c ..`).
///
/// It resolves the wheel to a `(col, row)` step from menu event ids `0x2c` (up, row -1) and `0x2d`
/// (down, row +1) via `FUN_14075d8f0`, and its only two callers are the grid mouse handler
/// `FUN_14073a5c0` and `FUN_140781460`.
pub(crate) const MENU_EVENT_WHEEL_DELTA_ACCESSOR_RVA: usize = 0x757c70;
pub(crate) static SAVE_PICKER_WHEEL_DELTA_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_WHEEL_DELTA_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_WHEEL_DELTA_SILENCED: AtomicUsize = AtomicUsize::new(0);

/// The INTERLOCK: while the picker owns the screen, the game's own grid never sees a wheel notch.
///
/// Two mechanisms can scroll this list for one detent -- the native grid handler and this pump --
/// and the double scroll is simply both of them running. Every attempt to arbitrate them by timing
/// failed, and the live log says why: the handler acts later than the tick the detent arrives on and
/// later than the tick after it too (our step at `+107884ms`, the handler's move only visible at
/// `+107911ms`), so there is no tick on which the pump can ask "did the game already take this one?"
/// and get a true answer. Deferring by a fixed number of ticks just moves the guess.
///
/// So do not arbitrate: remove one of the two mechanisms. Zeroing the delta here makes the wheel
/// branch in `FUN_14073a5c0` (`if (delta.col != 0 || delta.row != 0)`) fall through, so the native
/// grid performs no view scroll and no cursor move at all, and the pump is the sole owner of the
/// wheel with no timing assumption anywhere. It also removes the reason the wheel was uneven in the
/// first place: the native step was gated on the grid's own view base being able to move, which is
/// false at a clamp, so the game was an unreliable owner even when it was the only one.
///
/// Scoped to the picker's own screen, and it silences a read rather than dropping the user's input:
/// our own wheel latch comes from `GetRawInputData` and is untouched, so the detent still reaches
/// the picker. Every other menu keeps its native wheel exactly as designed.
pub(crate) unsafe extern "system" fn save_picker_wheel_delta_hook(
    msg: usize,
    out: *mut i32,
) -> *mut i32 {
    let orig_addr = SAVE_PICKER_WHEEL_DELTA_ORIG.load(Ordering::SeqCst);
    if orig_addr == 0 {
        return out;
    }
    let orig: unsafe extern "system" fn(usize, *mut i32) -> *mut i32 =
        unsafe { std::mem::transmute(orig_addr) };
    let ret = unsafe { orig(msg, out) };
    let owned = save_picker_live_profile_dialog() != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0;
    if !owned || out.is_null() {
        return ret;
    }
    let had_notch = unsafe { out.read_unaligned() != 0 || out.add(1).read_unaligned() != 0 };
    if had_notch {
        unsafe {
            out.write_unaligned(0);
            out.add(1).write_unaligned(0);
        }
        let n = SAVE_PICKER_WHEEL_DELTA_SILENCED.fetch_add(1, Ordering::SeqCst) + 1;
        if n <= 20 || n.is_multiple_of(50) {
            append_autoload_debug(format_args!(
                "save-picker: silenced native wheel notch #{n} (the pump owns the wheel)"
            ));
        }
    }
    ret
}

pub fn install_save_picker_wheel_delta_hook() {
    if SAVE_PICKER_WHEEL_DELTA_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Ok(addr) = game_rva_for_hook(MENU_EVENT_WHEEL_DELTA_ACCESSOR_RVA as u32) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve wheel-delta rva 0x{MENU_EVENT_WHEEL_DELTA_ACCESSOR_RVA:x}"
        ));
        SAVE_PICKER_WHEEL_DELTA_HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_wheel_delta_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_WHEEL_DELTA_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable wheel-delta failed: {status:?}"
                ));
                return;
            }
            match unsafe { er_hook::MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked wheel-delta FUN_140757c70 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: wheel-delta MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new wheel-delta failed: {status:?}"
        )),
    }
}

/// Move the picker's selection one row for a wheel detent the native grid declined to act on.
///
/// This calls `FUN_14073bc10` -- the list's own select-index primitive, the same call the grid's
/// mouse hit test makes (`FUN_14073a5c0` tail) and the same one the wheel path would have reached
/// via `FUN_14073b0c0` had its view-base gate let it through. Going through the select rather than
/// writing `list+0xd4` is what carries the chrome with the selection; a bare field write moves the
/// index and leaves the highlight where it was, which is the "rows scroll but the chrome doesn't
/// travel" half of the report. The call re-enters our own detour above, so the view base stays
/// pinned exactly as it does for a hover or a click.
pub(crate) unsafe fn save_picker_wheel_step_native_cursor(
    dialog: usize,
    model_row: usize,
    from_cursor: i32,
) -> i32 {
    let Ok(index) = i32::try_from(model_row)
        .map(|row| row.saturating_add(PROFILE_SELECT_NATIVE_ROW_MODEL_OFFSET))
        .and_then(u32::try_from)
    else {
        return from_cursor;
    };
    let Ok(select) = game_rva(MENU_ITEM_LIST_SET_CURSOR_RVA as u32) else {
        return from_cursor;
    };
    let select: unsafe extern "system" fn(usize, u32) -> u64 =
        unsafe { std::mem::transmute(select) };
    let ret = unsafe { select(dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET, index) };
    // Keep the pump's edge sampling honest: the next tick compares against this, and leaving the
    // pre-step row here would read our own step back as a native move and swallow the next detent.
    SAVE_PICKER_EDGE_SCROLL_PREV_CURSOR.store(
        usize::try_from(index).unwrap_or(EDGE_SCROLL_NO_PREV_CURSOR),
        Ordering::SeqCst,
    );
    let n = SAVE_PICKER_WHEEL_NATIVE_STEPS.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= 20 || n.is_multiple_of(25) {
        append_autoload_debug(format_args!(
            "save-picker: wheel step #{n} the grid declined from={from_cursor} to_index={index} select_ret={ret}"
        ));
    }
    i32::try_from(index).unwrap_or(from_cursor)
}

/// `FUN_140736c90(grid, point)` -- the grid's pointer hit test, byte-verified unique at
/// `0x140736c90` in the 1.16.2 deobf image.
pub(crate) const MENU_ITEM_LIST_POINT_TO_INDEX_RVA: usize = 0x736c90;
pub(crate) static SAVE_PICKER_HIT_TEST_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_HIT_TEST_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_PICKER_HIT_TEST_REBASED: AtomicUsize = AtomicUsize::new(0);

/// Neutralise the view base for the grid's pointer hit test, the same way the select hook does for
/// the select itself.
///
/// The hit test walks the visible cells and turns the one under the pointer into an absolute item
/// index by adding the view base, then discards the hit if that index is past the item count:
///
///     140736d41  MOV  R11D, [RSI + 0x348]   ; view row base
///     140736d80  LEA  EDI, [R10 + R11*1]    ; view row + base
///     140736daf  CMP  [RSI + 0xd0], EDI     ; count vs index
///     140736db5  JLE  ...                   ; index >= count -> report no hit
///
/// The picker keeps its model's scroll offset in that base so the native scrollbar thumb tracks a
/// listing far longer than the ten staged records (`save-picker: native scrollbar sync`). For the
/// hit test that offset is poison: with base 10 against 10 records every visible cell computes an
/// index >= count, so the pointer hits nothing, nothing is selected, and the game's click
/// activation has nothing to act on. Clicking therefore worked only while the scrollbar sat at the
/// very top, where the base happens to be 0 -- reported 2026-08-12, and the same shape as the wheel
/// dying at a clamped base.
///
/// Zeroing the base for the duration of the call makes the hit test return a view-relative index
/// `0..9`, which is exactly the space the ten staged records live in and the space the select hook
/// already leaves `+0xd4` in. The base is restored immediately afterwards, so the scrollbar thumb is
/// unaffected.
pub(crate) unsafe extern "system" fn save_picker_hit_test_hook(list: usize, point: usize) -> u32 {
    let orig_addr = SAVE_PICKER_HIT_TEST_ORIG.load(Ordering::SeqCst);
    if orig_addr == 0 {
        return u32::MAX;
    }
    let orig: unsafe extern "system" fn(usize, usize) -> u32 =
        unsafe { std::mem::transmute(orig_addr) };
    let dialog = save_picker_live_profile_dialog();
    let ours = dialog != 0
        && SAVE_PICKER_MODE_ACTIVE.load(Ordering::SeqCst) != 0
        && list == dialog + PROFILE_LOAD_DIALOG_ITEM_LIST_OFFSET;
    if !ours {
        return unsafe { orig(list, point) };
    }
    let before = unsafe { save_picker_grid_view_base(list) };
    if before == (0, 0) {
        return unsafe { orig(list, point) };
    }
    unsafe { save_picker_set_grid_view_base(list, (0, 0)) };
    let ret = unsafe { orig(list, point) };
    unsafe { save_picker_set_grid_view_base(list, before) };
    let n = SAVE_PICKER_HIT_TEST_REBASED.fetch_add(1, Ordering::SeqCst) + 1;
    if n <= 20 || n.is_multiple_of(100) {
        append_autoload_debug(format_args!(
            "save-picker: hit test rebased #{n} view {before:?} -> (0, 0) index={ret}"
        ));
    }
    ret
}

pub fn install_save_picker_hit_test_hook() {
    if SAVE_PICKER_HIT_TEST_HOOK_INSTALLED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let Ok(addr) = game_rva_for_hook(MENU_ITEM_LIST_POINT_TO_INDEX_RVA as u32) else {
        append_autoload_debug(format_args!(
            "save-picker: failed to resolve hit-test rva 0x{MENU_ITEM_LIST_POINT_TO_INDEX_RVA:x}"
        ));
        SAVE_PICKER_HIT_TEST_HOOK_INSTALLED.store(0, Ordering::SeqCst);
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            save_picker_hit_test_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SAVE_PICKER_HIT_TEST_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-picker: queue_enable hit-test failed: {status:?}"
                ));
                return;
            }
            match unsafe { er_hook::MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    leak_installed_hook(hook);
                    append_autoload_debug(format_args!(
                        "save-picker: hooked pointer hit test FUN_140736c90 0x{addr:x}"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "save-picker: hit-test MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-picker: MhHook::new hit-test failed: {status:?}"
        )),
    }
}

/// The grid's own view-scroll base as `(column, row)`.
pub(crate) unsafe fn save_picker_grid_view_base(list: usize) -> (i32, i32) {
    unsafe {
        (
            *((list + GRID_CONTROL_VIEW_COL_BASE_OFFSET) as *const i32),
            *((list + GRID_CONTROL_VIEW_ROW_BASE_OFFSET) as *const i32),
        )
    }
}

pub(crate) unsafe fn save_picker_set_grid_view_base(list: usize, base: (i32, i32)) {
    unsafe {
        *((list + GRID_CONTROL_VIEW_COL_BASE_OFFSET) as *mut i32) = base.0;
        *((list + GRID_CONTROL_VIEW_ROW_BASE_OFFSET) as *mut i32) = base.1;
    }
}

/// Log the grid's index space once per picker session: the select call bounds-checks against these,
/// and whether the cursor index is absolute or view-relative depends on them.
pub(crate) unsafe fn save_picker_log_grid_geometry_once(list: usize) {
    if SAVE_PICKER_GRID_GEOMETRY_LOGGED.swap(1, Ordering::SeqCst) != 0 {
        return;
    }
    let (count, cols, rows) = unsafe {
        (
            *((list + GRID_CONTROL_ITEM_COUNT_OFFSET) as *const i32),
            *((list + GRID_CONTROL_COLUMNS_OFFSET) as *const i32),
            *((list + GRID_CONTROL_ROWS_OFFSET) as *const i32),
        )
    };
    let view = unsafe { save_picker_grid_view_base(list) };
    append_autoload_debug(format_args!(
        "save-picker: grid geometry count={count} cols={cols} rows={rows} view_base={view:?}"
    ));
}

/// Clear this frame's vertical menu event so the native list never moves.
///
/// The list animates its own cursor move the instant it consumes the event, so a correction written
/// afterwards still lets the animation play -- which is what a player sees at the end of a listing
/// as a scroll that "happens" and then undoes itself. This runs from the MenuWindowJob::Run post
/// hook: `Run` is the producer that sets `+0x90[id] |= 1`, and the menu's own Update consumes it
/// later in the frame, so clearing here lands between the two.
pub(crate) unsafe fn save_picker_clear_vertical_menu_event(down: bool) -> bool {
    let id = if down {
        SAVE_PICKER_MENU_EVENT_DOWN_ID.load(Ordering::SeqCst)
    } else {
        SAVE_PICKER_MENU_EVENT_UP_ID.load(Ordering::SeqCst)
    };
    if id == MENU_EVENT_ID_UNLEARNED {
        return false;
    }
    let Some(keystate) = (unsafe { save_picker_menu_event_keystate() }) else {
        return false;
    };
    let byte = unsafe { keystate.add(id) };
    if unsafe { *byte } & MENU_EVENT_PRESSED_BIT == 0 {
        return false;
    }
    unsafe { *byte &= !MENU_EVENT_PRESSED_BIT };
    true
}
