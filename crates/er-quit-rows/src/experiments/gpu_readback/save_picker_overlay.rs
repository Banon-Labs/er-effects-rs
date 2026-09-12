//! Product-side compatibility shim for the save-picker extraction: the boot picker overlay lives
//! in `er-save-picker-core`; this module keeps existing product callsites stable while the remaining
//! picker/quit-menu seams are moved.

pub(crate) use er_save_picker_core::overlay::{
    SAVE_PICKER_KBD_HOOK_HITS, SAVE_PICKER_OVERLAY_ARMED, SAVE_PICKER_OVERLAY_DRAW_HITS,
    SAVE_PICKER_OVERLAY_HELD_POLLS, SAVE_PICKER_OVERLAY_INPUT_HITS, SAVE_PICKER_OVERLAY_OPEN_COUNT,
    SAVE_PICKER_OVERLAY_PICK_COUNT, SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT,
    SAVE_PICKER_OVERLAY_POLL_COUNT, boot_stage_picked_save_for_character_choice,
    ensure_save_picker_keyboard_hook, missing_save_picker_selected_slot,
    save_picker_overlay_active, save_picker_overlay_process_completion,
};

// The picker's compositing entry point. Only the boot cover composes the picker into its own
// frame, so without `loading-cover` nothing in this DLL calls it -- the picker still draws, through
// the Present overlay that stays compiled either way.
#[cfg(feature = "loading-cover")]
pub(crate) use er_save_picker_core::overlay::overlay_save_picker_onto;

pub(crate) fn boot_arm_missing_save_picker_in_game() -> bool {
    // The boot picker exists to choose the save the autoload is about to load, so without the
    // `autoload` feature there is nothing for it to answer and arming it is actively harmful:
    // `TitleTopDialog::open_menu` is held while a picker is pending, and with no autoload nothing
    // ever releases it. Measured 2026-09-11 -- the title sat through twelve confirms with
    // `title-open-menu-suppress: ... native menu-open held while missing-save picker pending` as
    // the only explanation in the log.
    //
    // This is the boot arm only. The Quit tab's own Load Character from File browser is a separate
    // entry point and is untouched.
    if !cfg!(feature = "autoload") {
        return false;
    }
    er_save_picker_core::overlay::arm_boot_picker()
}

pub(crate) fn save_picker_overlay_input_tick() {
    crate::experiments::boot_open_missing_save_picker_if_pending();
    er_save_picker_core::overlay::save_picker_overlay_input_tick();
}
