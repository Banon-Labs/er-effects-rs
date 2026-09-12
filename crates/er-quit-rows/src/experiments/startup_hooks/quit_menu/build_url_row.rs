//! Product re-export facade: the System>Quit **Load Build from URL** row moved to
//! `er_quit_menu_core::build_url_row`, and the link field it opens to
//! `er_quit_menu_core::build_url_editor`.
//!
//! Two things stay here, both of them the product's rather than the row's. The per-frame tick is
//! wrapped so the loading-cover portrait verify window keeps being polled -- it outlives the frame
//! an import lands on, and a shell with no such pipeline must not be made to carry it. And the
//! character-panel portrait refresh an applied import triggers reaches the row through the
//! `QuitMenuHost` seam's `build_import_applied`.

use super::*;

/// One frame of the build importer, driven from the product's recurring `FrameBegin` task.
///
/// # Safety
///
/// Game task thread only -- the context every mutation inside the runtime requires.
pub(crate) unsafe fn system_quit_build_import_tick() {
    // The portrait verify window outlives the frame the import landed on, so it is polled before
    // the moved tick's own early return rather than after it. Safety: the caller's game-task
    // contract; the tick costs one lock-free load while the window is closed, which is every frame
    // but the few after an import.
    unsafe { er_profile_summary_core::portrait_verify_tick() };
    // Safety: same game-task contract.
    unsafe { er_quit_menu_core::build_url_row::system_quit_build_import_tick() };
}

// ---- the character panel's portrait --------------------------------------------------------
//
// Everything about why this is needed, which slot it may touch, and what it measures lives in
// `er_profile_summary_core::portrait_refresh`. What has to stay here is the one step that cannot:
// `kick_target_profile_slot` is the per-slot replica of the engine's data-change sequence and it
// belongs beside the loading-cover pipeline that also drives it.

/// Re-derive the live character's own record and ask both character portraits to rebuild from it.
///
/// Two independent surfaces, driven in turn because they have two producers:
///
/// * the `CSMenuAsmModelRend` whose offscreen target Scaleform sees as `SYSTEX_Menu_Profile{NN}`,
///   which is what `05_010_ProfileSelect` shows. That is everything below, and it needs the record
///   re-derived first because the profile renderer is dressed from a record and from nothing else;
/// * the `CSMenuFaceModelRend` at `OptionSettingTopDialog+0x1890`, which fills
///   `SYSTEX_Menu_StatusFace` -- the portrait on the Quit panel the player is looking at when they
///   press the row. It reads no record at all; it is rebuilt from the live `PlayerGameData` by
///   re-invoking its builder, and the call belongs to the menu pump rather than to this game task,
///   so this only arms it. See `er_profile_summary_core::quit_panel_portrait`.
///
/// # Safety
///
/// Game task thread, character in the world, called once per applied import.
pub(crate) unsafe fn build_url_refresh_character_portrait() {
    // Armed first and unconditionally: the Quit panel's portrait does not read a record, so a slot
    // this code cannot attribute -- which stops the profile refresh below dead -- is no reason to
    // leave the panel the player is actually looking at showing the previous face.
    er_profile_summary_core::arm_quit_panel_portrait_refresh();
    let Some(slot) = portrait_loaded_slot_confirmed() else {
        er_profile_summary_core::note_unattributable_slot();
        return;
    };
    // Step one, the record. Safety: game task thread and a live character, which is what the
    // native's own save-lane callers hold.
    if !matches!(
        unsafe { er_profile_summary_core::sync_record_for_import(slot) },
        er_profile_summary_core::LiveSync::Synced { .. }
    ) {
        return;
    }
    // Step two, the model. Safety: same context; the target is vtable-checked before it is used.
    let Some(target) = (unsafe { er_profile_summary_core::portrait_rebuild_target(slot) }) else {
        return;
    };
    // The kick refuses a slot it has already kicked on this renderer. That latch paces a per-frame
    // cadence; an import is an edge, so the one rebuild it rations is exactly the one owed here.
    PORTRAIT_KICK_SLOT_KEY.store(0, Ordering::SeqCst);
    PORTRAIT_KICK_RENDERER.store(0, Ordering::SeqCst);
    // Safety: live summary and a renderer whose vtable was just checked against the profile
    // renderer's, on the game task thread.
    let fired =
        unsafe { kick_target_profile_slot(target.base, target.summary, target.renderer, slot) };
    er_profile_summary_core::note_portrait_rebuild(slot, target.renderer, fired);
}
