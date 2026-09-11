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
//! The rebuild is gated as well, by a step machine. `CSMenuAsmModelRend` derives from
//! `FD4StepTemplateBase`, its step table is filled by `FUN_1400a75c0`, and its current step index
//! lives at `renderer+0x40`. A settled portrait sits in step 6, `STEP_Wait_Play`, which reads three
//! request bytes in strict priority:
//!
//! ```text
//! +0x756 -> SetNextStep(7) and run it this frame     // hard teardown
//! +0x755 -> SetNextStep(7)                           // teardown next frame
//! +0x754 -> SetNextStep(1)                           // re-promote the stages, no teardown
//! none   -> the live per-frame block
//! ```
//!
//! Arming both is what produces a real rebuild, and the walk closes only because of one function:
//! `STEP_Finish_Play` clears `+0x755` and `+0x756` together (`*(u16*)(renderer+0x755) = 0`) and
//! leaves `+0x754` alone, so step 8 `STEP_Finish` finds it still set and sends the machine back to
//! step 1. The whole cycle is `6 -> 7 -> 8 -> 1 -> 2 -> 3 -> 4 -> 5 -> 6`, and step 4
//! `STEP_Finish_Setup` is the one that matters: it promotes the staged `ChrAsm` into the live one at
//! `+0x130`, allocates the model, registers its parts into the offscreen's layer holder
//! (`FUN_1409e9790`) and registers the per-frame submit task at group 100.
//!
//! Two arming mistakes are worth naming because each looks plausible. `+0x755` alone parks the
//! machine at step 8 forever and the portrait goes permanently blank. `+0x754` alone walks
//! `1 -> 2 -> 3 -> 4 -> 5 -> 6` with no teardown, and the builder `FUN_140bbb3b0` is null-guarded on
//! every allocation, so it is a complete no-op: the stages are re-promoted and the model is not
//! rebuilt at all. Only the pair does the work.
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
//! live data: it is that character's own summary. The tempting shortcut is to write the live
//! character into `record[0]` instead, on the theory that the panel always binds entry 0. That must
//! not happen whatever the display turns out to bind: `record[0]` belongs to a different character,
//! the whole ten-record table is serialized by the next save, and the corruption would reach disk.
//! [`BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1`] records the slot a run actually used.
//!
//! # Which surface this drives, and the one it is now known not to be
//!
//! Everything below drives a `CSMenuAsmModelRend` and its offscreen render target, published to
//! Scaleform as `SYSTEX_Menu_Profile{NN}`. That is what the `05_010_ProfileSelect` list shows: its
//! `MENU_DummyProfileFace_01..10` external-image symbols are bound to those ten targets.
//!
//! The System>Quit panel is not one of them, and an earlier version of this file asserted that it
//! was. Read out of the vanilla movie (`menu/win/02_040_optionsetting.gfx`): the Quit Game panel is
//! sprite 138 `MENU_FL_QuitGame` placing `PlayerInfo` = sprite 137 `GameEnd`, whose `Icon_0` is
//! sprite 130 -- a one-frame sprite holding image char 74, `MENU_DummyStatus_Face`, 512x256. The
//! movie contains zero occurrences of `DummyProfileFace`. So `Icon_0.gotoAndStop(row[+8] + 1)`, the
//! call the old reasoning rested on, is inert on this surface; it is load-bearing only on 05_010,
//! whose `Icon_0` really does have ten frames.
//!
//! What `MENU_DummyStatus_Face` is backed by is not recorded anywhere in this workspace, and until
//! it is, the honest statement is that this module's producer-side work is proven correct for the
//! ProfileSelect portraits and unproven for the Quit panel. The measurement is cheap: the Scaleform
//! bind observer in `title_resources_stats_text` already logs symbol and target for every bind whose
//! symbol contains `menu_`, so one run naming that pair settles it.
//!
//! # What is measured, and the measurement that was not enough
//!
//! A kick count says a rebuild was requested, so the first oracle here compared the renderer's live
//! stage-0 `ChrAsm` against the record and published [`BUILD_URL_PORTRAIT_EQUIP_VERDICT`]. Run
//! `br-20260911-002901-a7e0` then read that field as a pass -- record and renderer stage agreed on
//! the imported loadout, the kick was accepted, the level moved to the imported 150 -- while the
//! player was still looking at the previous armour.
//!
//! The field was true and useless. Setting a renderer's `ChrAsm` is a state write, and the portrait
//! is a captured render: until the model is destroyed, reassembled from the new rows and
//! rasterized, the picture is the old one however correct the input. An oracle that stops at the
//! input cannot fail on the defect it is supposed to catch.
//!
//! So the headline is now [`BUILD_URL_PORTRAIT_RENDER_VERDICT`], a conjunction. It reaches its pass
//! value only when the input matched and the model object was observed being torn down and rebuilt
//! with a different set of parts -- `renderer+0x778` going absent, and the part-node array the model
//! submit walks coming back different. Its other values name the failures apart: the input took and
//! the model never moved, the model rebuilt and came back identical, the input was wrong.
//!
//! That is still RAM rather than pixels, and the honest limit is worth stating: a rebuilt model with
//! different parts is the last thing observable in memory before the rasterizer, not the rasterizer
//! itself. A gate on the pixels would have to read back the renderer's own offscreen render target
//! (`renderer+0xa8` -> `CSEzOffscreenRend`, `+0x10` -> `CSRuntimeTexResCap`, `+0x78` ->
//! `CSGxTexture`, which `er_loading_portrait_core::resource_readback` already knows how to fetch)
//! before the import and after the rebuild, and require the two images to differ.

use core::sync::atomic::Ordering;

use er_game_base::mem::{game_data_addr, game_module_base, safe_read_usize};
use er_loading_portrait_core::{
    CHR_ASM_MODEL_INS_PARTS_NODE_COUNT, CHR_ASM_MODEL_INS_PARTS_NODE_OFFSET,
    CHR_ASM_MODEL_INS_SCENE_OFFSET, PROFILE_OFFSCREEN_SCENE_REGISTERED_OFFSET,
    PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET, PROFILE_RENDERER_DRAW_TASK_PROXY_OFFSET,
    PROFILE_RENDERER_MODEL_INS_OFFSET, PROFILE_RENDERER_MODEL_RES_OFFSET,
    PROFILE_RENDERER_STEP_INDEX_OFFSET, PROFILE_RENDERER_STEP_MAX,
    TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET,
    TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA, portrait_renderer_table_entry,
};
use er_telemetry_core::counters::{
    BUILD_URL_PORTRAIT_DRAW_BITS, BUILD_URL_PORTRAIT_DRAW_CALLS_AT_KICK,
    BUILD_URL_PORTRAIT_DRAW_TASK_CALLS, BUILD_URL_PORTRAIT_EQUIP_VERDICT,
    BUILD_URL_PORTRAIT_KICK_REFUSALS, BUILD_URL_PORTRAIT_KICKS,
    BUILD_URL_PORTRAIT_MODEL_ABSENT_SEEN, BUILD_URL_PORTRAIT_MODEL_INS_AFTER,
    BUILD_URL_PORTRAIT_MODEL_INS_BEFORE, BUILD_URL_PORTRAIT_MODELRES_PENDING,
    BUILD_URL_PORTRAIT_MODELRES_REQUESTED, BUILD_URL_PORTRAIT_MODELRES_RESOLVED,
    BUILD_URL_PORTRAIT_PARTS_AFTER, BUILD_URL_PORTRAIT_PARTS_BEFORE,
    BUILD_URL_PORTRAIT_REBUILD_VERDICT, BUILD_URL_PORTRAIT_RECORD_FINGERPRINT,
    BUILD_URL_PORTRAIT_RECORD_LEVEL, BUILD_URL_PORTRAIT_RECORD_SLOT_PLUS1,
    BUILD_URL_PORTRAIT_RECORD_SYNC_STATE, BUILD_URL_PORTRAIT_RECORD_SYNCS,
    BUILD_URL_PORTRAIT_REFRESH_ATTEMPTS, BUILD_URL_PORTRAIT_RENDER_VERDICT,
    BUILD_URL_PORTRAIT_RENDERER_FINGERPRINT, BUILD_URL_PORTRAIT_STEPS_SEEN,
    BUILD_URL_PORTRAIT_TARGET_RENDERER, BUILD_URL_PORTRAIT_VERIFY_TICKS,
    PROFILE_PERFRAME_HOOK_INSTALLED,
};

use crate::equip_fingerprint::{
    LiveSync, PORTRAIT_DRAW_HOOK_INSTALLED, PORTRAIT_DRAW_TASK_LIVE, PORTRAIT_DRAW_TASK_RAN,
    PORTRAIT_OFFSCREEN_REGISTERED, PORTRAIT_PARTS_IN_SCENE, PortraitEquipmentVerdict,
    PortraitRenderVerdict, RebuildObservation, parts_fingerprint, portrait_equipment_verdict,
    portrait_rebuild_verdict, portrait_render_verdict, portrait_step_bit, portrait_walked_rebuild,
};
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
    // The model object as it stands before anything is asked of it. Taken here because this is the
    // last moment before the kick, and the whole rebuild detector is a comparison against it.
    // Safety: fault-guarded reads of the renderer's own model instance.
    let (model, parts) = unsafe { read_model_and_parts(renderer) };
    BUILD_URL_PORTRAIT_MODEL_INS_BEFORE.store(model, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_PARTS_BEFORE.store(parts, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_MODEL_INS_AFTER.store(0, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_PARTS_AFTER.store(0, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_MODEL_ABSENT_SEEN.store(0, Ordering::SeqCst);
    Some(PortraitRebuildTarget {
        base,
        summary,
        renderer,
    })
}

/// The renderer's model instance and a fingerprint of the parts it is assembled from.
///
/// Returns `(0, 0)` when there is no model, which is a legitimate reading rather than a failure:
/// mid-teardown is exactly when it happens, and it is the observation the rebuild detector needs
/// most.
///
/// # Safety
///
/// Game task thread, `renderer` a live `CSMenuProfModelRend`. Every read is fault-guarded.
unsafe fn read_model_and_parts(renderer: usize) -> (usize, u64) {
    let model =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    if model == 0 {
        return (0, 0);
    }
    let mut nodes = [0usize; CHR_ASM_MODEL_INS_PARTS_NODE_COUNT];
    for (index, node) in nodes.iter_mut().enumerate() {
        let at =
            model + CHR_ASM_MODEL_INS_PARTS_NODE_OFFSET + index * core::mem::size_of::<usize>();
        // A node that cannot be read counts as absent rather than aborting the whole sample: the
        // fingerprint is a change detector, and a consistently unreadable slot is consistently
        // zero on both sides.
        *node = unsafe { safe_read_usize(at) }.unwrap_or(0);
    }
    (model, parts_fingerprint(&nodes))
}

/// The three things that have to be true for anything to be drawing this portrait, as a bitmask.
///
/// None of them is a rasterize counter -- the renderer, the model and the offscreen carry no frame
/// or generation number for the render target, so no such value exists to read. Together they are
/// the last thing observable in memory before the rasterizer: the parts exist and are attached to a
/// scene, that scene is registered with the render system, and a task submits it every frame.
///
/// # Safety
///
/// Game task thread, `renderer` a live `CSMenuAsmModelRend`. Every read is fault-guarded.
unsafe fn read_draw_bits(renderer: usize) -> usize {
    let mut bits = 0usize;
    if unsafe { safe_read_usize(renderer + PROFILE_RENDERER_DRAW_TASK_PROXY_OFFSET) }
        .is_some_and(|proxy| proxy != 0)
    {
        bits |= PORTRAIT_DRAW_TASK_LIVE;
    }
    let offscreen = unsafe {
        safe_read_usize(renderer + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
    }
    .unwrap_or(0);
    if offscreen != 0
        && unsafe {
            er_game_base::mem::safe_read_u8(offscreen + PROFILE_OFFSCREEN_SCENE_REGISTERED_OFFSET)
        }
        .is_some_and(|registered| registered != 0)
    {
        bits |= PORTRAIT_OFFSCREEN_REGISTERED;
    }
    let model =
        unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
    if model != 0
        && unsafe { safe_read_usize(model + CHR_ASM_MODEL_INS_SCENE_OFFSET) }
            .is_some_and(|scene| scene != 0)
    {
        bits |= PORTRAIT_PARTS_IN_SCENE;
    }
    // Whether the draw task has actually run since the rebuild was asked for, which is a different
    // question from whether it is registered, and the one the two previous verdicts never asked.
    if PROFILE_PERFRAME_HOOK_INSTALLED.load(Ordering::SeqCst) != 0 {
        bits |= PORTRAIT_DRAW_HOOK_INSTALLED;
        if BUILD_URL_PORTRAIT_DRAW_TASK_CALLS.load(Ordering::SeqCst)
            > BUILD_URL_PORTRAIT_DRAW_CALLS_AT_KICK.load(Ordering::SeqCst)
        {
            bits |= PORTRAIT_DRAW_TASK_RAN;
        }
    }
    bits
}

/// Bytes per `ChrAsmModelRes` entry, and where the entry array starts inside it.
///
/// `STEP_Wait_Play` passes `renderer+0x768` to the model-resource request `FUN_1409e6fb0`, which
/// walks entries of `0x40` bytes from `+0x30`. Each carries the resolved param id at `+0x00` and the
/// id that was requested at `+0x04`; while they disagree the parts file for the new row is still
/// loading, and the request early-outs rather than rebuilding the model from it.
const MODEL_RES_ENTRY_BASE: usize = 0x30;
const MODEL_RES_ENTRY_STRIDE: usize = 0x40;
/// Entries sampled. The protector and armament slots the portrait shows are at the front of the
/// array, and a bounded walk keeps this a fixed cost on a per-frame path.
const MODEL_RES_ENTRIES_SAMPLED: usize = 8;

/// Publish the model-resource request state: entry 0's resolved and requested ids, and how many of
/// the sampled entries still disagree.
///
/// This is what separates "the rebuild loaded the old thing" from "the new thing has not finished
/// loading yet" -- the second is a slow resource load, and a verify window shorter than the load
/// would report it as a failure.
///
/// # Safety
///
/// Game task thread, `renderer` a live `CSMenuAsmModelRend`. Every read is fault-guarded.
unsafe fn sample_model_resource(renderer: usize) {
    let res = unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_RES_OFFSET) }.unwrap_or(0);
    if res == 0 {
        return;
    }
    let mut pending = 0usize;
    for index in 0..MODEL_RES_ENTRIES_SAMPLED {
        let entry = res + MODEL_RES_ENTRY_BASE + index * MODEL_RES_ENTRY_STRIDE;
        let (Some(resolved), Some(requested)) =
            (unsafe { er_game_base::mem::safe_read_i32(entry) }, unsafe {
                er_game_base::mem::safe_read_i32(entry + 4)
            })
        else {
            continue;
        };
        if index == 0 {
            BUILD_URL_PORTRAIT_MODELRES_RESOLVED.store(resolved as u32 as usize, Ordering::SeqCst);
            BUILD_URL_PORTRAIT_MODELRES_REQUESTED
                .store(requested as u32 as usize, Ordering::SeqCst);
        }
        if resolved != requested {
            pending += 1;
        }
    }
    BUILD_URL_PORTRAIT_MODELRES_PENDING.store(pending, Ordering::SeqCst);
}

/// Fold this tick's step index into the mask of steps the window has seen.
///
/// # Safety
///
/// Game task thread, `renderer` a live `CSMenuAsmModelRend`. Fault-guarded.
unsafe fn note_step(renderer: usize) {
    // Parenthesised because a `let ... else` initializer may not end in a block-like expression.
    let Some(step) = (unsafe {
        er_game_base::mem::safe_read_i32(renderer + PROFILE_RENDERER_STEP_INDEX_OFFSET)
    }) else {
        return;
    };
    // An index outside the table is a read of something that is not this step machine, so it is
    // dropped rather than shifted into a bit nobody can interpret.
    let Ok(step) = usize::try_from(step) else {
        return;
    };
    if step > PROFILE_RENDERER_STEP_MAX {
        return;
    }
    BUILD_URL_PORTRAIT_STEPS_SEEN.fetch_or(portrait_step_bit(step), Ordering::SeqCst);
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
    // Both verdicts start at "no reading" for this window rather than carrying a previous import's
    // answer forward, so a second import that fails cannot be read through the first one's pass.
    BUILD_URL_PORTRAIT_REBUILD_VERDICT.store(0, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_RENDER_VERDICT
        .store(PortraitRenderVerdict::Unproven.code(), Ordering::SeqCst);
    BUILD_URL_PORTRAIT_STEPS_SEEN.store(0, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_DRAW_BITS.store(0, Ordering::SeqCst);
    // The draw-task delta is measured from here, and the detour needs to know which renderer to
    // count for. Both are set at the kick rather than in the window so the very first sample already
    // has a baseline to compare against.
    BUILD_URL_PORTRAIT_TARGET_RENDERER.store(renderer, Ordering::SeqCst);
    BUILD_URL_PORTRAIT_DRAW_CALLS_AT_KICK.store(
        BUILD_URL_PORTRAIT_DRAW_TASK_CALLS.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    BUILD_URL_PORTRAIT_VERIFY_TICKS.store(PORTRAIT_VERIFY_WINDOW_TICKS, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "profile-portrait: asked slot {slot} to rebuild from the re-derived record (renderer=0x{renderer:x}); the verdict lands in oracle_build_url_portrait_equip_verdict once the async build completes"
    ));
}

/// Sample the renderer while the window is open, and latch what it says.
///
/// Two independent measurements per tick, and the second is the one added after run
/// `br-20260911-002901-a7e0`:
///
/// * the **input** -- the renderer's live stage-0 `ChrAsm` at `+0x130`, the block the per-frame
///   model-resource request actually reads, compared against the record's fingerprint;
/// * the **model object** -- `renderer+0x778` and the part-node array it points at, compared
///   against what they were before the rebuild was asked for.
///
/// The first alone is what reported a pass over a screen that had not changed. Setting a
/// renderer's `ChrAsm` is a state write; the portrait is a captured render, so until the model is
/// destroyed and reassembled the picture is the previous one no matter how correct the input is.
/// Only [`PortraitRenderVerdict::Proven`] -- input matched and the model came back with different
/// parts -- closes the window, so a rebuild that never happens keeps sampling and the window
/// expires carrying the failure rather than an early optimistic pass.
///
/// Costs one lock-free load per frame while closed, which is every frame but the few after an
/// import.
///
/// # Safety
///
/// Game task thread. Every read is fault-guarded, so a renderer freed mid-window reads as no
/// measurement rather than faulting.
pub unsafe fn portrait_verify_tick() {
    if BUILD_URL_PORTRAIT_VERIFY_TICKS.load(Ordering::SeqCst) == 0 {
        return;
    }
    let remaining = BUILD_URL_PORTRAIT_VERIFY_TICKS.fetch_sub(1, Ordering::SeqCst);
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
    // The model object. Sampled first and unconditionally, because the teardown half of a rebuild
    // is a state the input read below cannot be taken in -- and it is the half that was missing.
    // Safety: fault-guarded reads of the renderer's own model instance.
    let (model, parts) = unsafe { read_model_and_parts(renderer) };
    if model == 0 {
        BUILD_URL_PORTRAIT_MODEL_ABSENT_SEEN.store(1, Ordering::SeqCst);
    } else {
        BUILD_URL_PORTRAIT_MODEL_INS_AFTER.store(model, Ordering::SeqCst);
        BUILD_URL_PORTRAIT_PARTS_AFTER.store(parts, Ordering::SeqCst);
    }
    let observed = RebuildObservation {
        model_before: BUILD_URL_PORTRAIT_MODEL_INS_BEFORE.load(Ordering::SeqCst),
        model_after: BUILD_URL_PORTRAIT_MODEL_INS_AFTER.load(Ordering::SeqCst),
        saw_model_absent: BUILD_URL_PORTRAIT_MODEL_ABSENT_SEEN.load(Ordering::SeqCst) != 0,
        parts_before: BUILD_URL_PORTRAIT_PARTS_BEFORE.load(Ordering::SeqCst),
        parts_after: BUILD_URL_PORTRAIT_PARTS_AFTER.load(Ordering::SeqCst),
    };
    let rebuild = portrait_rebuild_verdict(observed);
    BUILD_URL_PORTRAIT_REBUILD_VERDICT.store(rebuild.code(), Ordering::SeqCst);
    // Where the step machine is, and whether anything would draw the result. Safety: fault-guarded
    // reads of the renderer, its offscreen and its model.
    unsafe { note_step(renderer) };
    let draw_bits = unsafe { read_draw_bits(renderer) };
    BUILD_URL_PORTRAIT_DRAW_BITS.store(draw_bits, Ordering::SeqCst);
    unsafe { sample_model_resource(renderer) };

    // The input. A model that is mid-teardown has no readable stage to compare, which is not a
    // mismatch -- so an unreadable stage leaves the previous equipment verdict standing.
    let equipment = match unsafe {
        chr_asm_equipment_fingerprint(renderer + PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET)
    } {
        Some(live) => {
            BUILD_URL_PORTRAIT_RENDERER_FINGERPRINT.store(live, Ordering::SeqCst);
            let record = BUILD_URL_PORTRAIT_RECORD_FINGERPRINT.load(Ordering::SeqCst);
            let equipment = portrait_equipment_verdict(record, live);
            BUILD_URL_PORTRAIT_EQUIP_VERDICT.store(equipment.code(), Ordering::SeqCst);
            equipment
        }
        None => {
            equipment_verdict_from_code(BUILD_URL_PORTRAIT_EQUIP_VERDICT.load(Ordering::SeqCst))
        }
    };

    let render = portrait_render_verdict(equipment, rebuild, draw_bits);
    BUILD_URL_PORTRAIT_RENDER_VERDICT.store(render.code(), Ordering::SeqCst);
    if render == PortraitRenderVerdict::Proven {
        BUILD_URL_PORTRAIT_VERIFY_TICKS.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-portrait: slot {slot} re-rendered -- the model was torn down and rebuilt with different parts (0x{:016x} -> 0x{:016x}) and its stage carries the imported gear",
            observed.parts_before, observed.parts_after
        ));
        return;
    }
    // The window ran out without a proof. Say which of the failures it was, once, at the moment the
    // evidence is final -- a run whose portrait did not change must not have to be diagnosed from
    // an absent log line.
    if remaining == 1 {
        append_autoload_debug(format_args!(
            "profile-portrait: slot {slot} did NOT re-render within the verify window -- {} (equipment {}, model 0x{:x} -> 0x{:x}, absent_seen={}, parts 0x{:016x} -> 0x{:016x})",
            render.tag(),
            equipment.tag(),
            observed.model_before,
            observed.model_after,
            observed.saw_model_absent,
            observed.parts_before,
            observed.parts_after,
        ));
        let steps = BUILD_URL_PORTRAIT_STEPS_SEEN.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "profile-portrait: slot {slot} step machine walked mask 0x{steps:x} (a rebuild has to pass through 2 and 4), draw chain 0x{draw_bits:x} (bit0 submit task, bit1 offscreen scene, bit2 parts in scene), rebuild verdict {}{}",
            rebuild.tag(),
            if portrait_walked_rebuild(steps) {
                ""
            } else {
                " -- the machine never re-entered setup, so nothing was rebuilt to draw"
            }
        ));
    }
}

/// Recover the last published equipment verdict when this tick could not read the stage.
///
/// A mid-teardown renderer has no stage to compare, and treating that as a fresh
/// [`PortraitEquipmentVerdict::Unmeasured`] would erase a match already established on an earlier
/// tick -- turning the very teardown the detector is waiting for into a reason to forget what it
/// had seen.
const fn equipment_verdict_from_code(code: usize) -> PortraitEquipmentVerdict {
    match code {
        1 => PortraitEquipmentVerdict::Matches,
        2 => PortraitEquipmentVerdict::Differs,
        _ => PortraitEquipmentVerdict::Unmeasured,
    }
}
