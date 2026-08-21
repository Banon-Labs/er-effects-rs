//! Product-side compatibility shim for the save-picker extraction: the boot picker overlay lives
//! in `er-save-picker`; this module keeps existing product callsites stable while the remaining
//! picker/quit-menu seams are moved.

pub(crate) use er_save_picker::overlay::{
    SAVE_PICKER_KBD_HOOK_HITS, SAVE_PICKER_OVERLAY_ARMED, SAVE_PICKER_OVERLAY_DRAW_HITS,
    SAVE_PICKER_OVERLAY_HELD_POLLS, SAVE_PICKER_OVERLAY_INPUT_HITS, SAVE_PICKER_OVERLAY_OPEN_COUNT,
    SAVE_PICKER_OVERLAY_PICK_COUNT, SAVE_PICKER_OVERLAY_PICK_REJECT_COUNT,
    SAVE_PICKER_OVERLAY_POLL_COUNT, boot_stage_picked_save_for_character_choice,
    ensure_save_picker_keyboard_hook, missing_save_picker_selected_slot, overlay_save_picker_onto,
    save_picker_overlay_active, save_picker_overlay_process_completion,
};

pub(crate) fn boot_arm_missing_save_picker_in_game() -> bool {
    er_save_picker::overlay::arm_boot_picker()
}

pub(crate) fn save_picker_overlay_input_tick() {
    crate::experiments::boot_open_missing_save_picker_if_pending();
    er_save_picker::overlay::save_picker_overlay_input_tick();
}
