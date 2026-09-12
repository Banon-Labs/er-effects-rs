//! The Save Game row: what it writes, what it asks first, and how it closes afterwards.
//!
//! Save Game is a vanilla row. The mod clones two character rows onto the same tab and adds a
//! third for a build link, but this one ships with the game, and the flow behind it is the save
//! path -- request a write, retract a request the player backed out of, close the menus the write
//! left open, then take the title back. `lifecycle/save_flow.rs` drives all of it and was already
//! the only reader of most of these.
//!
//! Moved out of `quit_menu/system_quit_dialog_handlers.rs`, which held this flow, the Scaleform
//! handler lifetime hooks and the picked-save ingest in one file. Pure move, no behaviour change.

use super::*;

// Reached through `quit_menu`'s glob until the rows became a feature. These three are the
// vanilla Save Game row's own text, and `er-quit-menu-core` owns them.
use er_quit_menu_core::row_text::{
    SYSTEM_QUIT_SAVE_GAME_DIALOG_W, SYSTEM_QUIT_SAVE_GAME_HELP_W, SYSTEM_QUIT_SAVE_GAME_LABEL_W,
};

pub(crate) unsafe extern "system" fn system_quit_save_game_get_and_format_hook(
    out: usize,
    getter: usize,
    text_id: i32,
    fmg_name: usize,
    abbrev: usize,
) -> usize {
    // Every message the game formats passes here, so this is where a dialog's text ids can be
    // recorded for the `msgbox-builder` line that follows a few frames later.
    unsafe { crate::experiments::note_msg_text_id(text_id, abbrev) };
    let replacement = if text_id == SYSTEM_QUIT_FIRST_ROW_MENU_TEXT_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRMT") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_LABEL_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_FIRST_ROW_LINEHELP_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRHK") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_HELP_W.as_ptr() as usize)
    } else if text_id == SYSTEM_QUIT_SAVE_GAME_DIALOG_ID
        && unsafe { wide_equals_ascii(abbrev, b"GRD") }
    {
        Some(SYSTEM_QUIT_SAVE_GAME_DIALOG_W.as_ptr() as usize)
    } else {
        None
    };
    if let Some(text_ptr) = replacement {
        match game_rva(MSG_REPOSITORY_FORMAT_RVA) {
            Ok(format_addr) => {
                let format_fn: unsafe extern "system" fn(usize, usize, u32, usize, usize) -> usize =
                    unsafe { std::mem::transmute(format_addr) };
                SYSTEM_QUIT_SAVE_GAME_TEXT_SUBSTITUTION_COUNT.fetch_add(1, Ordering::SeqCst);
                return unsafe { format_fn(out, text_ptr, text_id as u32, fmg_name, abbrev) };
            }
            Err(_) => append_autoload_debug(format_args!(
                "system-quit-save: failed to resolve MsgRepository::Format rva 0x{MSG_REPOSITORY_FORMAT_RVA:x}; forwarding id={text_id}"
            )),
        }
    }
    let orig = SYSTEM_QUIT_SAVE_GAME_GET_AND_FORMAT_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return out;
    }
    let original: unsafe extern "system" fn(usize, usize, i32, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { original(out, getter, text_id, fmg_name, abbrev) }
}

/// Native cancel-close of one menu window. `pub(crate)` because the save-flow tick closes the
/// destination browser through the same primitive the deferred IngameTop close uses.
pub(crate) unsafe fn system_quit_save_game_close_window(window: usize, label: &str) -> bool {
    if window < 0x10000 || window == TITLE_OWNER_SCAN_START_ADDRESS {
        return false;
    }
    let vt = unsafe { safe_read_usize(window) }.unwrap_or(0);
    if vt < 0x10000 {
        append_autoload_debug(format_args!(
            "system-quit-save: skip close {label}=0x{window:x}; invalid vt=0x{vt:x}"
        ));
        return false;
    }
    let Ok(close_addr) = game_rva(SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve native close rva 0x{SYSTEM_QUIT_PROFILESELECT_NATIVE_CLOSE_RVA:x}; cannot close {label}=0x{window:x}"
        ));
        return false;
    };
    let close_fn: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(close_addr) };
    unsafe { close_fn(window) };
    SYSTEM_QUIT_SAVE_GAME_CLOSE_COUNT.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "system-quit-save: native cancel-close {label}=0x{window:x} vt=0x{vt:x}"
    ));
    true
}

#[cfg(feature = "quit-rows")]
pub(crate) unsafe fn system_quit_save_game_request_save_only() {
    let Ok(request_save_addr) = game_rva(SYSTEM_QUIT_REQUEST_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve RequestSave rva 0x{SYSTEM_QUIT_REQUEST_SAVE_RVA:x}"
        ));
        return;
    };
    let request_save: unsafe extern "system" fn(u8) =
        unsafe { std::mem::transmute(request_save_addr) };
    unsafe { request_save(true as u8) };
    match game_rva(SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA) {
        Ok(profile_addr) => {
            let save_request_profile: unsafe extern "system" fn(u8) =
                unsafe { std::mem::transmute(profile_addr) };
            unsafe { save_request_profile(true as u8) };
        }
        Err(_) => append_autoload_debug(format_args!(
            "system-quit-save: failed to resolve SaveRequest_Profile rva 0x{SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA:x}; RequestSave already issued"
        )),
    }
}

/// Fire the native save request pair FORCED: `throttled = false` for both calls.
///
/// The bool is pinned (1.16.2 Ghidra decompile, 2026-07-28; see the RVA consts):
/// `RequestSave(true)` runs a 60-second throttle against `GameMan+0xb98` whose early
/// return sets no flags, and `SaveRequest_Profile(true)` the same against `+0xb88`
/// (`SetSeconds(0x3c)`). Under boot-time global suppression the dispatchers still run
/// their commit tails for swallowed autosaves, so those throttle timestamps stay warm --
/// a throttled user press within 60 s of any swallowed autosave would silently set
/// nothing and strand the armed one-shot bypass token. The Save Game commit therefore
/// always fires with `false`. The quit-to-desktop sites deliberately keep
/// `system_quit_save_game_request_save_only` (true/true): under suppression those become
/// intentional no-op saves -- the Save Game row is the only path that really writes.
pub(crate) unsafe fn system_quit_save_game_request_save_forced() {
    const FORCED_NOT_THROTTLED: u8 = false as u8;
    let Ok(request_save_addr) = game_rva(SYSTEM_QUIT_REQUEST_SAVE_RVA) else {
        append_autoload_debug(format_args!(
            "save-flow: failed to resolve RequestSave rva 0x{SYSTEM_QUIT_REQUEST_SAVE_RVA:x}; forced save NOT fired"
        ));
        return;
    };
    let request_save: unsafe extern "system" fn(u8) =
        unsafe { std::mem::transmute(request_save_addr) };
    unsafe { request_save(FORCED_NOT_THROTTLED) };
    match game_rva(SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA) {
        Ok(profile_addr) => {
            let save_request_profile: unsafe extern "system" fn(u8) =
                unsafe { std::mem::transmute(profile_addr) };
            unsafe { save_request_profile(FORCED_NOT_THROTTLED) };
        }
        Err(_) => append_autoload_debug(format_args!(
            "save-flow: failed to resolve SaveRequest_Profile rva 0x{SYSTEM_QUIT_SAVE_REQUEST_PROFILE_RVA:x}; forced RequestSave already issued"
        )),
    }
}

/// Call one of the game's own save-request retractions after verifying its whole body.
///
/// Fails closed through `save_flow_verify_rva`: an unresolvable address or a single drifted
/// byte skips the call and reports it. Not retracting costs CPU; calling unknown code costs
/// the process.
pub(crate) unsafe fn call_verified_retract(
    rva: u32,
    expected: &[u8],
    mask: &[u8],
    name: &str,
) -> bool {
    let Some(address) = save_flow_verify_rva(rva, expected, mask, name) else {
        return false;
    };
    let retract: unsafe extern "system" fn() = unsafe { std::mem::transmute(address) };
    unsafe { retract() };
    true
}

/// Retract the two native save-request flags through the game's own setters.
///
/// `b72` / `b73` select which flags to clear -- the caller decides ownership; this only
/// performs it. Returns the pair of "actually cleared" results.
pub(crate) unsafe fn system_quit_save_request_retract(b72: bool, b73: bool) -> (bool, bool) {
    let cleared_b72 = b72
        && unsafe {
            call_verified_retract(
                SAVE_REQUEST_RETRACT_B72_RVA,
                SAVE_REQUEST_RETRACT_B72_SIG,
                SAVE_REQUEST_RETRACT_B72_SIG_MASK,
                "b72",
            )
        };
    let cleared_b73 = b73
        && unsafe {
            call_verified_retract(
                SAVE_REQUEST_RETRACT_B73_RVA,
                SAVE_REQUEST_RETRACT_B73_SIG,
                SAVE_REQUEST_RETRACT_B73_SIG_MASK,
                "b73",
            )
        };
    (cleared_b72, cleared_b73)
}

/// Run the proven Save Game close-all sequence and hand the flow to `save_flow_tick`.
///
/// `commit` selects the destination stage: `true` = stage 6 CLOSING_COMMIT (the tick fires
/// the forced save request once the menus are closed and the RAM gates are green), `false` =
/// stage 5 CLOSING_ABORT (the user declined; the tick returns to the world having written
/// nothing).
///
/// Close-then-fire (save-game-flow WP1, 2026-07-28): the save request is never fired here.
/// Firing while menus are open is a dispatch-split hazard -- `ShouldSave` (the b72 lane)
/// requires `!CanShowSaveMenu()` (`CSMenuMan+0x13c == 0`) but the b73 gate `FUN_140679370`
/// does not, so an open-menu fire lets the b73-only lane dispatch a system-only submit first,
/// that submit consumes the one-shot suppression-bypass token, and the later char-slot submit
/// is swallowed. Staging the commit and firing at stage 7 produces a single combined
/// `b72 && b73` -> `FUN_14067b940` -> one `FUN_140e6ef60` submit -> one enqueue -> one token.
///
/// WP1/WP2 commit plan: overwrite the loaded save (WP3 adds a destination target).
pub(crate) unsafe fn system_quit_save_game_close_menus(
    dialog: usize,
    source: &str,
    commit: bool,
) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "system-quit-save: {source} abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    let option = SYSTEM_QUIT_OPTION_SETTING_WINDOW.load(Ordering::SeqCst);
    let top = SYSTEM_QUIT_INGAME_TOP_WINDOW.load(Ordering::SeqCst);
    // Stage the outcome before the close sequence so the save-flow tick owns the flow
    // from the next game-task frame onward.
    SAVE_FLOW_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(
        if commit {
            SAVE_FLOW_STAGE_CLOSING_COMMIT
        } else {
            SAVE_FLOW_STAGE_CLOSING_ABORT
        },
        Ordering::SeqCst,
    );
    if commit {
        SYSTEM_QUIT_SAVE_GAME_CONFIRM_COUNT.fetch_add(1, Ordering::SeqCst);
    }
    // `dialog` is the System/Quit tab's PropertyEditDialog, not a `MenuWindow`; calling the
    // MenuWindow cancel-close primitive on it dispatches the wrong vfunc. Close the owning menu
    // windows only, matching the Escape/back stack semantics instead of treating the row dialog as a
    // window.
    let closed_dialog = false;
    let closed_option = if option != 0 {
        unsafe { system_quit_save_game_close_window(option, "option_window") }
    } else {
        false
    };
    // Do not close the root IngameTop in the same call stack: runtime proof showed that closing the
    // full root stack immediately after the row action terminates the process. The native Escape
    // flow unwinds from the active submenu first, so close OptionSetting now and schedule IngameTop
    // for a later game-task tick.
    let closed_top = false;
    if top != 0 && top != option {
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW.store(top, Ordering::SeqCst);
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.store(2, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "system-quit-save: {source} -> staged {} + native menu-window close-all dialog=0x{dialog:x} option=0x{option:x} top=0x{top:x} closed_dialog={closed_dialog} closed_option={closed_option} closed_top={closed_top}",
        if commit {
            "CLOSING_COMMIT (close-then-fire)"
        } else {
            "CLOSING_ABORT (nothing will be written)"
        }
    ));
    closed_option || closed_top
}

#[cfg(feature = "quit-rows")]
/// Enter the Save Game flow from the row press: Straight to the destination list.
///
/// Captures the System/Quit dialog the whole flow is anchored on, then hands the browser open to
/// the menu pump via `SAVE_DEST_OPEN_PICKER_PENDING` and parks in stage 3. Nothing is asked here
/// and nothing is written here.
///
/// Why the pump and not an inline open. Opening the destination browser stages ProfileSummary row
/// records and submits an `05_010` MenuJob; `system_quit_menu_window_run_post` is the context that
/// was runtime-proven to own that submit (2026-07-29, three consecutive commits), and it is the
/// same hand-off the OS surface needs in order to block inside comdlg32 off the row-action stack.
/// The row press therefore stages the request rather than performing it, and stage 3's
/// `SAVE_DEST_PICKER_OPEN_TIMEOUT_TICKS` bounds a pump that never picks it up.
///
/// There is no "DEGRADE TO AN IMMEDIATE COMMIT" path any more, and its removal is a safety fix
/// rather than a simplification. It existed because the old flow could not ask its first question
/// without the MessageBoxBuilder recipe, so a build that failed the prologue check wrote to the
/// loaded save with no confirm at all. Opening a list needs no message box, so a broken recipe now
/// costs only the overwrite confirm -- and an unconfirmable overwrite is refused at the pick
/// (`save_dest_handle_picked_target`), never performed silently. A free destination name still
/// commits, because it never needed a confirm in the first place.
pub(crate) unsafe fn system_quit_save_game_start_flow(dialog: usize) -> bool {
    if dialog < 0x10000 || dialog == TITLE_OWNER_SCAN_START_ADDRESS {
        append_autoload_debug(format_args!(
            "save-flow: row press abort -- dialog=0x{dialog:x} is not heap-like"
        ));
        return false;
    }
    save_flow_box_clear();
    // Defensive: no destination state may survive from an earlier flow (a stale target would send
    // this save to the wrong file).
    save_dest_reset("save game row press");
    SAVE_FLOW_DIALOG.store(dialog, Ordering::SeqCst);
    SAVE_FLOW_STAGE_TICKS.store(0, Ordering::SeqCst);
    // The overwrite confirm is only answerable if the MessageBoxDialog builder hook is live --
    // that detour is what captures the dialog pointer the stage machine polls. It is normally
    // installed at boot (`online_disable_enabled()` path in the game task), but make sure:
    // this call is idempotent, and the row press is the menu thread, i.e. the one context in
    // which no other thread can be executing the builder while MinHook patches it.
    install_auto_accept_hook();
    // Same reasoning for the answer observer: `CS::MenuJob::EmitResult` is what tells us which
    // button the user pressed on the branch where the dialog never stores its result. Install
    // is idempotent; if it is not live the poll falls back to `dialog+0x1e8` and reports
    // UNDECIDABLE rather than guessing, so log the state here where the run can see it.
    install_menu_job_emit_result_hook();
    let capture_live = MSGBOX_BUILDER_ORIG.load(Ordering::SeqCst) != HOOK_ORIGINAL_UNSET;
    // One press = one bypass arm = one commit. Counting presses is what makes
    // `oracle_save_bypass_allowed_total = 2` readable as "two presses" instead of "a double-arm
    // bug"; without it the two are indistinguishable from telemetry alone.
    let press = SAVE_FLOW_ROW_PRESS_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
    append_autoload_debug(format_args!(
        "save-flow: row press #{press} dialog=0x{dialog:x} -> opening the destination list (no confirm is asked here); overwrite_confirm_available={} builder_capture_live={capture_live} emit_observer_installed={}",
        save_flow_box_recipe_available(),
        MENU_JOB_EMIT_RESULT_INSTALLED.load(Ordering::SeqCst)
    ));
    if !save_flow_box_recipe_available() || !capture_live {
        // Not fatal to the press, and deliberately so: the list needs no message box. Only a pick
        // that would clobber an existing file needs one, and that pick is refused rather than
        // written blind. Say it once here so a run can attribute a later refusal.
        append_autoload_debug(format_args!(
            "save-flow: the overwrite confirm cannot be built on this build (recipe_ok={} builder_capture_live={capture_live}) -- the destination list still opens; picking an EXISTING file will be refused, picking a free name still commits",
            save_flow_box_recipe_available()
        ));
    }
    // Menu-pump owns the open (see this function's docs). Stage 3 waits for it and bounds it.
    SAVE_DEST_OPEN_PICKER_PENDING.store(1, Ordering::SeqCst);
    SAVE_FLOW_STAGE.store(SAVE_FLOW_STAGE_DEST_BROWSE, Ordering::SeqCst);
    true
}

pub(crate) unsafe fn system_quit_save_game_deferred_close_tick() {
    let frames = SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.load(Ordering::SeqCst);
    if frames == 0 {
        return;
    }
    if frames > 1 {
        SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.fetch_sub(1, Ordering::SeqCst);
        return;
    }
    SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_FRAMES.store(0, Ordering::SeqCst);
    let top = SYSTEM_QUIT_SAVE_GAME_DEFER_TOP_WINDOW.swap(0, Ordering::SeqCst);
    if top != 0 {
        let closed =
            unsafe { system_quit_save_game_close_window(top, "deferred_ingame_top_window") };
        append_autoload_debug(format_args!(
            "system-quit-save: deferred IngameTop close top=0x{top:x} closed={closed}"
        ));
    }
}

/// A 1.16.2-only stack band, and one that cannot be carried forward.
///
/// The pair below is a 4 KB window of `.text`, not a function plus an offset, so there is nothing
/// for the 1.16.2 -> 1.17 map to key on. That is not a gap in the map; it is a property of the
/// band. Measured across the 33 `.pdata` functions it overlaps: 12 are unmapped outright and the
/// 21 that map move by seven different deltas (`+0xdf0`, `+0xe20`, `+0xe30`, `+0xe40`, `+0xe80`,
/// `+0xe90`, `+0x1560`). A band whose contents move apart has no translated width, so no anchor
/// rescues it -- unlike the GX transport band, whose 12 functions all move `+0x1e00` together and
/// which is therefore anchored rather than refused.
///
/// It is also the weakest-evidenced comparison in the tree. `SYSTEM_QUIT_RETURN_TITLE_REQUEST_RVA`
/// (`0x67a3a0`) has exactly one direct caller in the whole 1.16.2 image, at `0x59d90e` inside
/// `FUN_14059d8b0`, which is nowhere near this band; nothing records what the band was measured
/// from. So the honest treatment is to decline on any build it was not measured on and say so,
/// rather than invent a 1.17 window. The branch it guards is documented dormant -- the product row
/// path clears the arming latch before this hook runs -- so declining costs a safety net that
/// nothing currently reaches, and 1.16.2 behaviour is unchanged.
const LEGACY_CONFIRM_CALLER_BAND: core::ops::Range<usize> = 0x7a3000..0x7a4000;

/// Say once, and at most a handful of times, that a build comparison has no answer on this build.
///
/// The same shape `er_quit_menu_core::row_cloner` uses for the row-return addresses: a refusal that
/// is loud, bounded, and names the constant rather than going quiet.
fn note_unsupported_build_comparison(what: &str) {
    use std::sync::atomic::AtomicUsize;
    static SAID: AtomicUsize = AtomicUsize::new(0);
    const SAY_AT_MOST: usize = 8;
    if SAID.fetch_add(1, Ordering::SeqCst) < SAY_AT_MOST {
        append_autoload_debug(format_args!(
            "system-quit: {what} cannot be compared on {}; treating it as no match",
            er_game_base::game_build::describe_build()
        ));
    }
}

pub(crate) unsafe extern "system" fn system_quit_save_game_return_title_request_hook() {
    let dialog = SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.swap(0, Ordering::SeqCst);
    let legacy_confirm_caller = if er_game_base::game_build::is_supported_build() {
        callstack_contains_game_rva(
            LEGACY_CONFIRM_CALLER_BAND.start,
            LEGACY_CONFIRM_CALLER_BAND.end,
        )
    } else {
        note_unsupported_build_comparison(
            "the legacy Save Game confirm caller band (0x7a3000..0x7a4000, untranslatable)",
        );
        false
    };
    if dialog >= 0x10000 && legacy_confirm_caller {
        // Dormant safety net (the product row path clears the arming latch). It keeps the
        // WP1 semantics -- close-then-fire straight to the commit, no confirm chain.
        unsafe { system_quit_save_game_close_menus(dialog, "legacy_confirm", true) };
        append_autoload_debug(format_args!(
            "system-quit-save: legacy confirmation path suppressed native return-title request dialog=0x{dialog:x}"
        ));
        return;
    }
    if dialog != 0 {
        SYSTEM_QUIT_SAVE_GAME_ARMED_DIALOG.store(dialog, Ordering::SeqCst);
    }
    let orig = SYSTEM_QUIT_SAVE_GAME_RETURN_TITLE_REQUEST_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "system-quit-save: return-title request trampoline unset and no active Save Game dialog; return"
        ));
        return;
    }
    let original: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { original() };
}
