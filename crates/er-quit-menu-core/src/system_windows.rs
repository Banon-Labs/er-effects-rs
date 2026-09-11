//! The System windows a ProfileSelect overlay has to hide, and put back.
//!
//! Opening `05_010_ProfileSelect` over the pause menu leaves the two windows the player came
//! through -- `02_000_IngameTop` and `02_040_OptionSetting` -- drawn underneath it. Hiding them is
//! not a cosmetic choice: both keep taking input, so the picker opens onto a menu that still
//! responds to the same presses. Putting them back afterwards has the mirror problem, because the
//! OptionSetting pane the player was looking at comes back with `DisplayInfo.Visible = 0` unless
//! its visibility is re-applied -- the blank Game Options tab.
//!
//! # Why this is in the feature crate
//!
//! It was inside `er-quickload`, reachable only from the 288-line post-`MenuWindowJob::Run` body
//! that also owns the return-title chain, the save-flow boxes, the save picker and the portrait
//! tick. A standalone shell has none of those and cannot call any of it, so a shell could open the
//! picker and then had no way to hide the menu behind it -- which is what made the standalone
//! **Load Character** row open a picker the player could not use.
//!
//! The product-only steps are [`SystemWindowHooks`], not `#[cfg]`s: a shell passes
//! [`SystemWindowHooks::NONE`] and the same code runs with those steps absent.

use std::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::mem::{game_rva, safe_read_i32, safe_read_u8, safe_read_u16, safe_read_usize};
use er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;
use er_telemetry_core::counters::{
    SAVE_PICKER_REBUILD_PENDING_DIALOG, SYSTEM_QUIT_HIDE_REAL_WINDOWS_COUNT,
    SYSTEM_QUIT_INGAME_TOP_WINDOW, SYSTEM_QUIT_OPTION_SETTING_WINDOW,
    SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE, SYSTEM_QUIT_PROFILE_SELECT_WINDOW,
    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED, SYSTEM_QUIT_REAL_WINDOWS_HIDDEN,
    SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT, SYSTEM_QUIT_TOP_HIDE_LIST,
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW, SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID,
    SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW,
};
use er_title_flow::{
    MENU_WINDOW_ROOT_PROXY_CTOR_RVA, MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA,
    MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE, OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET,
    OPTIONSETTING_COMPOSITE_OFFSET, OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT,
    OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET, OPTIONSETTING_CURRENT_TAB,
    OPTIONSETTING_DIALOG_PANE_PROXY_OFFSET, OPTIONSETTING_DIALOG_REFRESH_SELECTED_ROW_RVA,
    OPTIONSETTING_MENU_ID, OPTIONSETTING_TAB_CONTROL_OFFSET, OPTIONSETTING_TAB_VIEW_OFFSET,
    OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET, SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_COUNT,
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_LAST_SELECTED,
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_OLD_CURRENT,
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_SELECTED,
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_TAB,
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_REAPPLY_COUNT,
    TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK, TITLE_PRESS_START_SET_VISIBLE_RVA,
};

use crate::host::append_autoload_debug;

/// Stand-in for a null pointer in the log lines below, so a missing read and a real zero read the
/// same way they do everywhere else in this workspace.
const NULL: usize = usize::MAX;
/// Below this, an address is not a heap pointer and is not worth dereferencing.
const HEAP_LO: usize = 0x10000;
/// The Quit tab's visual index on the OptionSetting tab strip.
pub const OPTIONSETTING_QUIT_TAB_INDEX: usize = 8;

/// The steps only a host with a character switch behind it can supply.
///
/// Each is `None` in a standalone shell, and the function that would have called it says so in its
/// own log line rather than silently doing nothing.
#[derive(Clone, Copy, Default)]
pub struct SystemWindowHooks {
    /// Drop the save picker's own per-dialog state. The product owns the picker; a shell that does
    /// not ship it has nothing to reset.
    pub save_picker_reset: Option<fn(&str)>,
    /// Forget the live-layout editor's `05_010` row field targets, which are about to be freed.
    pub forget_profile_editor_field_targets: Option<fn(&str)>,
    /// Whether a character switch is mid-flight. While one is, the restore is skipped entirely:
    /// the old System UI must stay hidden across the native transition.
    pub switch_in_flight: Option<fn() -> bool>,
    /// Run the switch's own restore-time work -- the quit-save unblock and the return-title submit.
    /// Answers whether the chain was submitted, which is what lets the caller reset its state.
    pub switch_restore: Option<unsafe fn(usize, &str) -> bool>,
    /// Put the profile summary back after a save swap.
    pub save_swap_restore_profile_summary: Option<unsafe fn(&str)>,
}

impl SystemWindowHooks {
    /// A host with no character switch: every product-only step absent.
    pub const NONE: Self = Self {
        save_picker_reset: None,
        forget_profile_editor_field_targets: None,
        switch_in_flight: None,
        switch_restore: None,
        save_swap_restore_profile_summary: None,
    };
}

#[repr(C, align(8))]
pub struct RootProxyScratch {
    bytes: [u8; MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE],
}

/// Set a `MenuWindow`'s visibility through the game's own setter and stamp the matching visual
/// flags, returning whether the setter was dispatched.
///
/// # Safety
///
/// Menu thread, with `window` a live `MenuWindow` and `base` the game module base.
pub unsafe fn menu_window_set_visible_and_flags(
    base: usize,
    window: usize,
    visible: bool,
    source: &str,
) -> bool {
    if window < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- window=0x{window:x} not heap-like"
        ));
        return false;
    }
    let window_vt = unsafe { safe_read_usize(window) }.unwrap_or(NULL);
    if window_vt < HEAP_LO {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- window=0x{window:x} vt=0x{window_vt:x} invalid"
        ));
        return false;
    }
    let mut scratch = RootProxyScratch {
        bytes: [0; MENU_WINDOW_ROOT_PROXY_SCRATCH_SIZE],
    };
    let Ok(root_proxy_ctor_addr) = game_rva(MENU_WINDOW_ROOT_PROXY_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve root proxy ctor rva 0x{MENU_WINDOW_ROOT_PROXY_CTOR_RVA:x}"
        ));
        return false;
    };
    let Ok(set_visible_addr) = game_rva(TITLE_PRESS_START_SET_VISIBLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve SetVisible rva 0x{TITLE_PRESS_START_SET_VISIBLE_RVA:x}"
        ));
        return false;
    };
    let Ok(dtor_addr) = game_rva(MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window visibility skipped -- failed to resolve root proxy scratch dtor rva 0x{MENU_WINDOW_ROOT_PROXY_SCRATCH_DTOR_RVA:x}"
        ));
        return false;
    };
    let root_proxy_ctor: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(root_proxy_ctor_addr) };
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(set_visible_addr) };
    let dtor: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(dtor_addr) };
    let scratch_ptr = scratch.bytes.as_mut_ptr() as usize;
    let root_proxy = unsafe { root_proxy_ctor(window, scratch_ptr) };
    if root_proxy != scratch_ptr {
        append_autoload_debug(format_args!(
            "system-quit-dup: {source} top-window root-proxy ctor returned unexpected 0x{root_proxy:x} scratch=0x{scratch_ptr:x}; still using returned proxy"
        ));
    }
    unsafe { set_visible(root_proxy, u8::from(visible)) };
    unsafe { dtor(scratch_ptr + 0x28) };

    let menu_id = unsafe { safe_read_u16(window + 0x180) }.unwrap_or(u16::MAX);
    let cs_menu_man = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CS_MENU_MAN_GLOBAL_RVA,
            "CS_MENU_MAN_GLOBAL_RVA",
        ))
    }
    .unwrap_or(NULL);
    let mut flags_before = NULL;
    let mut flags_after = NULL;
    if menu_id < 0x47 && cs_menu_man >= HEAP_LO {
        let flags_addr = cs_menu_man + 0x90 + menu_id as usize;
        if let Some(flags) = unsafe { safe_read_u8(flags_addr) } {
            flags_before = flags as usize;
            let new_flags = if visible {
                flags | TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK
            } else {
                flags & 1
            };
            unsafe { (flags_addr as *mut u8).write_volatile(new_flags) };
            flags_after = new_flags as usize;
        }
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: {source} top-window visibility window=0x{window:x} vt=0x{window_vt:x} visible={visible} root_proxy=0x{root_proxy:x} menu_id=0x{menu_id:x} flags=0x{flags_before:x}->0x{flags_after:x}"
    ));
    true
}

/// Hide `02_000_IngameTop` and `02_040_OptionSetting` so a submitted ProfileSelect overlay is not
/// drawn over the pause menu it came from.
///
/// # Safety
///
/// As [`menu_window_set_visible_and_flags`]: menu thread, `base` the game module base. The windows
/// are resolved from the live list rather than passed in.
pub unsafe fn hide_real_system_windows(base: usize, source: &str) {
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if profile == 0 || SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) != 0 {
        return;
    }
    let hid_top = if top != 0 && top != profile {
        unsafe { menu_window_set_visible_and_flags(base, top, false, source) }
    } else {
        false
    };
    let hid_option = if option != 0 && option != profile && option != top {
        unsafe { menu_window_set_visible_and_flags(base, option, false, source) }
    } else {
        false
    };
    if hid_top || hid_option {
        SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.store(1, Ordering::SeqCst);
        SYSTEM_QUIT_HIDE_REAL_WINDOWS_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: real-system-window hide source={source} top=0x{top:x} option=0x{option:x} profile=0x{profile:x} hid_top={hid_top} hid_option={hid_option}"
    ));
}

/// Re-assert the `02_040_OptionSetting` pane's visibility after a restore, optionally forcing the
/// tab the dialog should come back on.
///
/// # Safety
///
/// Menu thread, with `option_window` a live `02_040_OptionSetting` window.
pub unsafe fn reapply_optionsetting_pane_visibility(
    _base: usize,
    option_window: usize,
    forced_tab: Option<usize>,
    source: &str,
) {
    if option_window < HEAP_LO {
        return;
    }
    let menu_id = unsafe { safe_read_u16(option_window + 0x180) }.unwrap_or(u16::MAX);
    if menu_id != OPTIONSETTING_MENU_ID {
        // Not the OptionSetting window (e.g. the IngameTop top-menu, menu_id 0xffff) -- this composite
        // layout is OptionSetting-specific; skip.
        return;
    }
    let composite = option_window + OPTIONSETTING_COMPOSITE_OFFSET;
    let current =
        unsafe { safe_read_usize(composite + OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET) }
            .unwrap_or(0);
    if current < HEAP_LO {
        return;
    }
    // The real selected tab the user is viewing: SettingTabControl at window+0x1870, its tab view at
    // +0x10, selected index at view+0xd4 (`FUN_140739f20` = `*(view+0xd4)`). Use this, not the composite's
    // `current` pane pointer -- after our detour `current` (composite+0xb8) is stale (observed: it matched
    // cache slot 9 while the user was on the Game tab), so re-applying its index re-shows the wrong pane.
    // When restoring after Back from our child ProfileSelect, the previous menu is always the Quit tab:
    // write the tab view's selected index to Quit before the self-copy native refresh so the tab strip,
    // current-pane pointer, and visible pane all agree with the parent the user came from.
    let tab_view = unsafe {
        safe_read_usize(
            option_window + OPTIONSETTING_TAB_CONTROL_OFFSET + OPTIONSETTING_TAB_VIEW_OFFSET,
        )
    }
    .unwrap_or(0);
    let live_tab = if tab_view >= HEAP_LO {
        unsafe { safe_read_i32(tab_view + OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET) }
            .map(|v| v as usize)
            .filter(|&t| t < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT)
    } else {
        None
    };
    let real_tab = forced_tab
        .filter(|&t| t < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT)
        .or(live_tab);
    // The forced tab is written only after its backing pane is proven present, further down. Writing
    // it here (as this did until 2026-08-12) wedges the menu whenever the pane is absent: the tab
    // strip commits to Quit, the pane reapply below bails, and OptionSetting stays actively_shown
    // with no visible pane -- input captured, nothing drawn, no way out. Reproduced by opening the
    // picker twice: the second close lands on a recreated OptionSetting window (composite address
    // changes) whose cache slots 8/9 were never built, so slot 9 reads null.
    // Diagnostic: which cache slot the (possibly stale) current pane pointer matches.
    let mut cache_tab: Option<usize> = None;
    for i in 0..OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT {
        let cached = unsafe {
            safe_read_usize(composite + OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET + i * 8)
        }
        .unwrap_or(0);
        if cached == current {
            cache_tab = Some(i);
            break;
        }
    }
    let Some(tab_index) = real_tab else {
        append_autoload_debug(format_args!(
            "system-quit-dup: optionsetting pane-reapply skipped source={source} -- no real tab index (tab_view=0x{tab_view:x} current=0x{current:x} live_tab={live_tab:?} forced_tab={forced_tab:?} cache_tab={cache_tab:?} composite=0x{composite:x})"
        ));
        return;
    };
    // OptionSetting has one extra cached pane before the visible tab panes: natural telemetry showed
    // visual tab 8 (Quit) backed by cache slot 9, while cache slot 8 is the tab immediately to its
    // left. Use the visual tab for the tab strip, but the +1 cache slot for the composite current pane
    // and native SetVisible pass; otherwise Back returns to the Quit tab label with the left tab's rows.
    let pane_index = (tab_index + 1).min(OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT - 1);
    let Ok(set_visible_addr) = game_rva(TITLE_PRESS_START_SET_VISIBLE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "system-quit-dup: optionsetting pane-reapply skipped source={source} -- SetVisible rva 0x{TITLE_PRESS_START_SET_VISIBLE_RVA:x} unresolved"
        ));
        return;
    };
    let selected = unsafe {
        safe_read_usize(composite + OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET + pane_index * 8)
    }
    .unwrap_or(0);
    if selected < HEAP_LO {
        // Leave the native tab selection alone. Forcing it here would point the tab strip at a tab
        // with no pane, which reads to the player as a menu that owns input but draws nothing.
        append_autoload_debug(format_args!(
            "system-quit-dup: optionsetting pane-reapply skipped source={source} -- selected cached pane missing tab_index={tab_index} composite=0x{composite:x}"
        ));
        return;
    }
    // The backing pane is now proven present, so committing the tab strip to it cannot strand the
    // menu without a pane. This write is deliberately downstream of the check above.
    if let (Some(tab), true) = (
        forced_tab.filter(|&t| t < OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT),
        tab_view >= HEAP_LO,
    ) {
        unsafe {
            *((tab_view + OPTIONSETTING_TAB_VIEW_SELECTED_INDEX_OFFSET) as *mut i32) = tab as i32;
        }
        OPTIONSETTING_CURRENT_TAB.store(tab, Ordering::SeqCst);
    }
    unsafe {
        *((composite + OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET) as *mut usize) = selected;
    }
    let mut refreshed = false;
    if let Ok(refresh_addr) = game_rva(OPTIONSETTING_DIALOG_REFRESH_SELECTED_ROW_RVA) {
        let select_tab: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(refresh_addr) };
        // Native tab-select copies old current pane state into the new pane before refreshing. Because
        // we pre-set current=selected above, the copy is selected->selected (safe), but the helper still
        // runs the internal Scaleform/row refresh that manual SetVisible did not reproduce. It indexes
        // the composite pane cache, not the visual tab strip, so pass pane_index.
        unsafe { select_tab(composite, pane_index as i32) };
        SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_COUNT.fetch_add(1, Ordering::SeqCst);
        SYSTEM_QUIT_OPTIONSETTING_DIRECT_REFRESH_LAST_SELECTED.store(selected, Ordering::SeqCst);
        refreshed = true;
    } else {
        append_autoload_debug(format_args!(
            "system-quit-dup: optionsetting pane-reapply native select skipped source={source} -- refresh rva 0x{OPTIONSETTING_DIALOG_REFRESH_SELECTED_ROW_RVA:x} unresolved"
        ));
    }
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_REAPPLY_COUNT.fetch_add(1, Ordering::SeqCst);
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_TAB.store(tab_index, Ordering::SeqCst);
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_OLD_CURRENT.store(current, Ordering::SeqCst);
    SYSTEM_QUIT_OPTIONSETTING_DIRECT_VISIBLE_LAST_SELECTED.store(selected, Ordering::SeqCst);
    let set_visible: unsafe extern "system" fn(usize, u8) =
        unsafe { std::mem::transmute(set_visible_addr) };
    let mut visible_mask: usize = 0;
    for i in 0..OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT {
        let cached = unsafe {
            safe_read_usize(composite + OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET + i * 8)
        }
        .unwrap_or(0);
        if cached >= HEAP_LO {
            let visible = (i == pane_index) as u8;
            unsafe { set_visible(cached + OPTIONSETTING_DIALOG_PANE_PROXY_OFFSET, visible) };
            if visible != 0 {
                visible_mask |= 1usize << i;
            }
        }
    }
    append_autoload_debug(format_args!(
        "system-quit-dup: optionsetting pane-reapply native-select source={source} composite=0x{composite:x} old_current=0x{current:x} selected=0x{selected:x} tab_index={tab_index} pane_index={pane_index} live_tab={live_tab:?} forced_tab={forced_tab:?} cache_tab={cache_tab:?} visible_mask=0x{visible_mask:x} refreshed={refreshed} select_addr=0x{:x} set_visible=0x{set_visible_addr:x} (pre-repaired self-copy)",
        game_rva(OPTIONSETTING_DIALOG_REFRESH_SELECTED_ROW_RVA).unwrap_or(0)
    ));
}

/// ProfileSelect window whose native `MenuWindowJob` finalizer has completed. The finalizer runs
/// inside the original `MenuWindowJob::Run`; restoration waits for the post-original hook so no
/// GFx/menu calls are made from inside native teardown.
static PROFILE_SELECT_FINALIZED_PENDING: AtomicUsize = AtomicUsize::new(0);

/// Read (and consume) the window whose finalizer completed since the last call.
pub fn take_finalized_profile_select() -> usize {
    PROFILE_SELECT_FINALIZED_PENDING.swap(0, Ordering::SeqCst)
}

/// Record that the native finalizer for `window` has run.
pub fn note_profile_select_finalized(window: usize) {
    if window == 0 {
        return;
    }
    if SYSTEM_QUIT_PROFILE_SELECT_WINDOW
        .compare_exchange(window, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        PROFILE_SELECT_FINALIZED_PENDING.store(window, Ordering::SeqCst);
        // A cancel/path-label refresh may have queued a records-changed rebuild immediately before
        // outer Back finalized this exact dialog. It is obsolete now and would target freed memory.
        let _ = SAVE_PICKER_REBUILD_PENDING_DIALOG.compare_exchange(
            window,
            0,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        append_autoload_debug(format_args!(
            "system-quit-dup: native ProfileSelect finalizer completed window=0x{window:x}; queued post-Run restore and cleared matching stale rebuild"
        ));
    }
}

/// Drop every piece of state the ProfileSelect overlay put up.
///
/// # Safety
///
/// Menu-pump context: the hooks it calls resolve live menu objects.
pub unsafe fn reset_profile_select_state(source: &str, hooks: &SystemWindowHooks) {
    if let Some(reset) = hooks.save_picker_reset {
        reset(source);
    }
    SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILE_SELECT_WINDOW.store(0, Ordering::SeqCst);
    // The 05_010 rows are going away, so the live-layout editor must stop believing it can still
    // write to their text fields. Only the profile-row surface is dropped: the title-load current
    // row is owned by the title screen and outlives this teardown.
    if let Some(forget) = hooks.forget_profile_editor_field_targets {
        forget("profile-row-populate");
    }
    // End the profile-load flow so the legit Quit-Game/Return-to-Desktop confirm MessageBox is no
    // longer suppressed once ProfileSelect is gone (the flag was set at the Load Character click).
    SYSTEM_QUIT_PROFILE_LOAD_FLOW_ACTIVE.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_FIRED.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_LIST.store(0, Ordering::SeqCst);
    SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID.store(usize::MAX, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-dup: reset ProfileSelect hide state source={source}"
    ));
}

/// Put `02_000_IngameTop` and `02_040_OptionSetting` back, and re-show the pane the player was on.
///
/// # Safety
///
/// Menu-pump context: this calls the game's own `SetVisible` on live window proxies.
pub unsafe fn restore_real_system_windows(base: usize, source: &str, hooks: &SystemWindowHooks) {
    if SYSTEM_QUIT_REAL_WINDOWS_HIDDEN.load(Ordering::SeqCst) == 0 {
        unsafe { reset_profile_select_state(source, hooks) };
        return;
    }
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let profile = SYSTEM_QUIT_PROFILE_SELECT_WINDOW.load(Ordering::SeqCst);
    if hooks.switch_in_flight.is_some_and(|active| active()) {
        // A switch owns the teardown from here. Restoring the old System UI mid-transition would
        // put the menu the player is leaving back on screen over a world that is being torn down.
        let submitted = hooks
            .switch_restore
            .is_some_and(|restore| unsafe { restore(base, source) });
        append_autoload_debug(format_args!(
            "system-quit-dup: skip restore real windows during switch source={source} profile=0x{profile:x} top=0x{top:x} option=0x{option:x} chain_submitted={submitted}; leaving old System UI hidden"
        ));
        if submitted {
            unsafe { reset_profile_select_state(source, hooks) };
        }
        return;
    }
    let restored_top = if top != 0 {
        unsafe { menu_window_set_visible_and_flags(base, top, true, source) }
    } else {
        false
    };
    let restored_option = if option != 0 && option != top {
        let restored = unsafe { menu_window_set_visible_and_flags(base, option, true, source) };
        unsafe {
            reapply_optionsetting_pane_visibility(
                base,
                option,
                Some(OPTIONSETTING_QUIT_TAB_INDEX),
                source,
            )
        };
        restored
    } else {
        false
    };
    append_autoload_debug(format_args!(
        "system-quit-dup: restore real windows source={source} profile=0x{profile:x} top=0x{top:x} option=0x{option:x} restored_top={restored_top} restored_option={restored_option}"
    ));
    if let Some(restore_summary) = hooks.save_swap_restore_profile_summary {
        unsafe { restore_summary(source) };
    }
    unsafe { reset_profile_select_state(source, hooks) };
    if restored_top || restored_option {
        SYSTEM_QUIT_RESTORE_REAL_WINDOWS_COUNT.fetch_add(1, Ordering::SeqCst);
    }
}
