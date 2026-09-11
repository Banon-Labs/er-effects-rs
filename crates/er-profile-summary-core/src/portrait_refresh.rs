//! Making a character panel's portrait show the build that was just imported.
//!
//! # Why an import leaves it showing the previous loadout
//!
//! The portrait is the game's own widget, not anything this workspace composites. `FUN_1409aa680`
//! (`er_loading_portrait_core::PROFILE_RENDERER_REFRESH_RVA`) is the only caller of the profile
//! renderer's set-ChrAsm `FUN_140bbe1a0` -- one call xref in the 1.16.2 dump, the other two
//! references being vtable data -- and the source it hands over is `record + 0x1a8`, the `ChrAsm`
//! image inside a `CS::ProfileSummary` record. So the gear a portrait wears is whatever that record
//! says, and nothing else.
//!
//! The record is re-derived from the live character only by the native
//! [`crate::live_player_sync`] calls, whose two game-side callers are the `GameMan` save lanes. A
//! build import writes no save, so nothing re-derives the record and the portrait keeps the
//! pre-import loadout.
//!
//! The rebuild is gated as well. Each renderer carries two request latches -- `+0x754` (a build is
//! requested) and `+0x755` (tear the current model down first) -- and the refresh skips any slot
//! where either is set. They are raised at dialog construction and consumed as the build runs, so a
//! settled portrait reads both as zero and a rebuild can be asked for. That is exactly what closing
//! and reopening the menu does by hand today.
//!
//! # So the repair is two steps, in this order
//!
//! Re-derive the record from the live character, then rebuild the model from it. Neither is
//! sufficient alone: a rebuild without the sync faithfully renders the old gear again, and a sync
//! without the rebuild changes nothing until the dialog is reconstructed.
//!
//! # Which slot, and which one this deliberately does not touch
//!
//! The record synced is the one belonging to the character that is loaded -- the same field the two
//! native save lanes pass to the native. That is the only record it is correct to overwrite with
//! live data: it is that character's own summary.
//!
//! It is worth saying what is deliberately not done. The panel's row model is built with a literal
//! slot index of 0 (`FUN_1408753f0` -> `FUN_1408759e0(row, 0, ..)`) and the populate ends in
//! `Icon_0.gotoAndStop(row[+8] + 1)`, so the panel binds profile-renderer entry 0. For a character
//! in slot 0 -- the ordinary case -- that is the same slot this syncs and everything lines up. For a
//! character in another slot it is not, and the tempting shortcut is to write the live character
//! into `record[0]` instead. That must not happen: `record[0]` belongs to a different character, the
//! whole ten-record table is serialized by the next save, and the corruption would reach disk. A
//! portrait bound to another character's renderer is a pre-existing display defect, left visible
//! rather than papered over with a destructive write. [`BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1`]
//! records the slot so a run can tell the two cases apart.
//!
//! # What is measured
//!
//! A kick count says a rebuild was requested. It does not say the model is being built from the
//! imported gear, and every existing portrait oracle compares the renderer against the record --
//! which after an import agree with each other while both disagree with the player. So the field
//! this module exists to publish is [`BUILD_URL_PORTRAIT_EQUIP_VERDICT`], taken by re-reading the
//! renderer's live stage-0 `ChrAsm` after the asynchronous build lands and comparing its equipment
//! fingerprint against the record's.

use core::sync::atomic::Ordering;

use er_game_base::mem::{game_data_addr, game_module_base, safe_read_usize};
use er_loading_portrait_core::{
    PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET, TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
    portrait_renderer_table_entry,
};
use er_telemetry_core::counters::{
    BUILD_URL_PORTRAIT_EQUIP_VERDICT, BUILD_URL_PORTRAIT_KICK_REFUSALS, BUILD_URL_PORTRAIT_KICKS,
    BUILD_URL_PORTRAIT_RECORD_FINGERPRINT, BUILD_URL_PORTRAIT_RECORD_LEVEL,
    BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1, BUILD_URL_PORTRAIT_RECORD_SYNC_STATE,
    BUILD_URL_PORTRAIT_RECORD_SYNCS, BUILD_URL_PORTRAIT_REFRESH_ATTEMPTS,
    BUILD_URL_PORTRAIT_RENDERER_FINGERPRINT, BUILD_URL_PORTRAIT_VERIFY_TICKS,
};

use crate::equip_fingerprint::{LiveSync, PortraitEquipmentVerdict, portrait_equipment_verdict};
use crate::host::append_autoload_debug;
use crate::live_player_sync::{chr_asm_equipment_fingerprint, sync_record_from_live_player};
use crate::live_records::system_quit_profile_summary_ptr;

/// How many frames the renderer stage is re-read for after a rebuild is asked for.
///
/// The model build is asynchronous -- measured at ~94ms from kick to a live model instance, a
/// handful of frames -- so the verdict cannot be taken on the frame the kick fires. A bounded window
/// is what keeps that from becoming an open-ended per-frame read of game memory; 240 ticks is ~4s at
/// 60Hz, generous against a loaded machine and still finite.
pub const PORTRAIT_VERIFY_WINDOW_TICKS: usize = 240;

/// The profile renderer a rebuild would act on, once it is known to be one.
#[derive(Clone, Copy, Debug)]
pub struct PortraitRebuildTarget {
    /// The game module base the table was resolved against.
    pub base: usize,
    /// The live `CS::ProfileSummary`.
    pub summary: usize,
    /// `DAT_143d6d8d0[slot]`, vtable-checked.
    pub renderer: usize,
}

/// Step one: re-derive `slot`'s record from the live character, and publish what that did.
///
/// Returns the outcome so the caller can decline to ask for a rebuild that would render the
/// previous loadout. Every counter this writes is read back out as an `oracle_build_url_portrait_*`
/// field.
///
/// # Safety
///
/// Game task thread, character in the world -- the contract
/// [`sync_record_from_live_player`] carries.
pub unsafe fn sync_record_for_import(slot: i32) -> LiveSync {
    BUILD_URL_PORTRAIT_REFRESH_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    // Safety: the caller's contract carries through unchanged.
    let sync = unsafe { sync_record_from_live_player(slot) };
    BUILD_URL_PORTRAIT_RECORD_SYNC_STATE.store(sync.code(), Ordering::SeqCst);
    BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1.store(slot as usize + 1, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_RECORD_LEVEL.store(sync.level_after().max(0) as usize, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_RECORD_FINGERPRINT.store(sync.fingerprint_after(), Ordering::SeqCst);
    if sync.equipment_changed() {
        BUILD_URL_PORTRAIT_RECORD_SYNCS.fetch_add(1, Ordering::SeqCst);
    }
    if !matches!(sync, LiveSync::Synced { .. }) {
        BUILD_URL_PORTRAIT_KICK_REFUSALS.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-portrait: leaving slot {slot} alone -- its record could not be re-derived ({}); a rebuild now would render the previous loadout",
            sync.tag()
        ));
    }
    sync
}

/// Record that the caller could not name a loaded slot, so nothing was synced.
///
/// Separate from the refusals inside [`sync_record_for_import`] because it happens before a slot
/// exists to attribute anything to, and syncing a record this code cannot attribute would overwrite
/// some other character's summary.
pub fn note_unattributable_slot() {
    BUILD_URL_PORTRAIT_REFRESH_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_KICK_REFUSALS.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "profile-portrait: leaving the portrait alone -- no source names the loaded slot, and a record this code cannot attribute must not be overwritten with the live character"
    ));
}

/// Step two, part one: find the profile renderer for `slot`, or say why there is none.
///
/// The vtable check is what makes this a profile renderer rather than whatever now occupies a
/// recycled table entry; the native builder derefs `table[slot]+0x754` with no null check of its
/// own, so a caller that skipped this would be handing it an access violation.
///
/// # Safety
///
/// Game task thread. Every read is fault-guarded, so an absent table reads as `None` rather than
/// faulting.
pub unsafe fn portrait_rebuild_target(slot: i32) -> Option<PortraitRebuildTarget> {
    let base = game_module_base().unwrap_or(0);
    // Safety: fault-guarded walk of `GameDataMan+0x78`.
    let summary = unsafe { system_quit_profile_summary_ptr() };
    // Safety: one pointer read at a resolved table entry.
    let renderer =
        unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
    let vtable_matches = renderer != 0
        // Safety: the object's first qword, fault-guarded.
        && unsafe { safe_read_usize(renderer) }.unwrap_or(0)
            == game_data_addr(
                base,
                TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
            );
    if base == 0 || summary == 0 || !vtable_matches {
        BUILD_URL_PORTRAIT_KICK_REFUSALS.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-portrait: the record for slot {slot} now describes the imported build, but no profile renderer is live to rebuild from it (renderer=0x{renderer:x} summary=0x{summary:x}); it updates on the next menu open"
        ));
        return None;
    }
    Some(PortraitRebuildTarget {
        base,
        summary,
        renderer,
    })
}

/// Step two, part two: record whether the rebuild was actually taken, and open the verify window.
///
/// The kick itself stays with the caller: it is the per-slot replica of the engine's own
/// data-change sequence and it lives beside the loading-cover pipeline that also drives it.
pub fn note_portrait_rebuild(slot: i32, renderer: usize, fired: bool) {
    if !fired {
        BUILD_URL_PORTRAIT_KICK_REFUSALS.fetch_add(1, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-portrait: slot {slot} refused the rebuild (a build is already in flight, or the record reads as no character); the record is correct either way, so the next rebuild renders the imported build"
        ));
        return;
    }
    BUILD_URL_PORTRAIT_KICKS.fetch_add(1, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_EQUIP_VERDICT.store(
        PortraitEquipmentVerdict::Unmeasured.code(),
        Ordering::SeqCst,
    );
    BUILD_URL_PORTRAIT_RENDERER_FINGERPRINT.store(0, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_VERIFY_TICKS.store(PORTRAIT_VERIFY_WINDOW_TICKS, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "profile-portrait: asked slot {slot} to rebuild from the re-derived record (renderer=0x{renderer:x}); the verdict lands in oracle_build_url_portrait_equip_verdict once the async build completes"
    ));
}

/// Read the renderer stage back while the window is open, and latch what it says.
///
/// This is the measurement the whole repair is judged on. It reads the renderer's live stage-0
/// `ChrAsm` -- `+0x130`, the block the per-frame model-resource request actually reads, not the
/// `+0x548` inbox the feed writes -- and compares its equipment fingerprint against the record's.
///
/// A match closes the window. Anything else keeps sampling until the window runs out, so a window
/// that expires carries the last thing it actually saw rather than an optimistic default. Costs one
/// lock-free load per frame while closed, which is every frame but the few after an import.
///
/// # Safety
///
/// Game task thread. Every read is fault-guarded, so a renderer freed mid-window reads as no
/// measurement rather than faulting.
pub unsafe fn portrait_verify_tick() {
    if BUILD_URL_PORTRAIT_VERIFY_TICKS.load(Ordering::SeqCst) == 0 {
        return;
    }
    BUILD_URL_PORTRAIT_VERIFY_TICKS.fetch_sub(1, Ordering::SeqCst);
    let slot = match BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1.load(Ordering::SeqCst) {
        0 => return,
        plus_one => plus_one as i32 - 1,
    };
    let base = game_module_base().unwrap_or(0);
    if base == 0 {
        return;
    }
    // Safety: one pointer read at a resolved table entry.
    let renderer =
        unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }.unwrap_or(0);
    if renderer == 0 {
        return;
    }
    // Safety: the renderer's own stage-0 `ChrAsm`, read dword by fault-guarded dword.
    let Some(live) =
        (unsafe { chr_asm_equipment_fingerprint(renderer + PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET) })
    else {
        return;
    };
    BUILD_URL_PORTRAIT_RENDERER_FINGERPRINT.store(live, Ordering::SeqCst);
    let record = BUILD_URL_PORTRAIT_RECORD_FINGERPRINT.load(Ordering::SeqCst);
    let verdict = portrait_equipment_verdict(record, live);
    BUILD_URL_PORTRAIT_EQUIP_VERDICT.store(verdict.code(), Ordering::SeqCst);
    if verdict == PortraitEquipmentVerdict::Matches {
        BUILD_URL_PORTRAIT_VERIFY_TICKS.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-portrait: slot {slot} is now dressing its model from the imported build (record and renderer stage agree at 0x{record:016x})"
        ));
    }
}
