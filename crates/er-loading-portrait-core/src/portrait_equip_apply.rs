//! Put the character's own equipment back into the ChrAsm the native feed just stripped.
//!
//! The WHY, the disassembly it rests on, and the reason this is param-ids-only are all in
//! [`crate::portrait_equip_restore`]. This module is the unsafe half: it reads the two live
//! `equipment_param_ids` arrays, asks that module what differs, writes the record's values into the
//! renderer inbox, and publishes the semaphores.
//!
//! ORDERING IS THE WHOLE CORRECTNESS ARGUMENT. This must run AFTER `set_model_source` (which is what
//! does the damage) and BEFORE the `+0x754` build kick (which is what consumes the result). The call
//! site in the loading-cover per-slot kick is the only place both of those are true.
//!
//! A FAILED WRITE IS NOT A SILENT ONE. Every read is fault-guarded and a failure at any point bumps
//! `PORTRAIT_EQUIP_RESTORE_FAILURES` and returns `None`, so a run whose portrait was built from the
//! mutilated array says so in telemetry instead of looking like a run where the repair simply had
//! nothing to do -- the exact false negative that let PR #128 report a pass over a nude character.

use core::sync::atomic::Ordering;

use crate::prelude::*;

/// Kicks whose restore is logged. The counters carry every kick; the log is for the first few, in
/// the shape every other loading-cover log line in this pipeline uses.
const RESTORE_LOG_KICK_LIMIT: usize = 4;

/// Count and NAME a failed restore. A portrait built from the feed's stripped ChrAsm must say so,
/// or it is indistinguishable from a run where the repair had nothing to do.
fn restore_failed(slot: i32) {
    PORTRAIT_EQUIP_RESTORE_FAILURES.fetch_add(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "loading-portrait: equipment restore FAILED for slot {slot} -- the portrait is being built from the feed's stripped ChrAsm (no weapon, bare hands and legs)"
    ));
}

/// Byte offset of `equipment_param_ids[index]` within a `ChrAsm`.
fn chr_asm_param_id_offset(index: usize) -> usize {
    CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + index * core::mem::size_of::<i32>()
}

/// Read a whole 22-entry `equipment_param_ids` array out of a live `ChrAsm`.
///
/// # Safety
///
/// `chr_asm` must be a live `CS::ChrAsm`. Every read is fault-guarded, so a stale pointer yields
/// `None` rather than a fault -- but a pointer to a DIFFERENT object would be read as one.
unsafe fn read_param_ids(chr_asm: usize) -> Option<[i32; PORTRAIT_EQUIP_ENTRY_COUNT]> {
    let mut ids = [PORTRAIT_EQUIP_EMPTY_ID; PORTRAIT_EQUIP_ENTRY_COUNT];
    for (index, id) in ids.iter_mut().enumerate() {
        *id = unsafe { safe_read_i32(chr_asm + chr_asm_param_id_offset(index)) }?;
    }
    Some(ids)
}

/// Restore the record's `equipment_param_ids` over the ones `set_model_source` left in the renderer
/// inbox, and report what that changed.
///
/// Returns `None` when either array could not be read whole, having counted the failure. The caller
/// carries on either way: a portrait built from the feed's array is the status quo ante, not a crash.
///
/// # Safety
///
/// `renderer` must be a live `CSMenuProfModelRend` whose `set_model_source` has already run, and
/// `record_chr_asm` the `ChrAsm` inside the matching `ProfileSummary` record. Must be called on the
/// GAME thread, between the feed and the build kick -- the renderer's own steps read this memory.
pub unsafe fn portrait_equip_restore_apply(
    renderer: usize,
    record_chr_asm: usize,
    slot: i32,
) -> Option<PortraitEquipRestore> {
    let inbox = renderer + PROFILE_RENDERER_CHR_ASM_INBOX_OFFSET;
    let Some(record_ids) = (unsafe { read_param_ids(record_chr_asm) }) else {
        restore_failed(slot);
        return None;
    };
    let Some(fed_ids) = (unsafe { read_param_ids(inbox) }) else {
        restore_failed(slot);
        return None;
    };
    let Some(report) = portrait_equip_restore_report(&record_ids, &fed_ids) else {
        restore_failed(slot);
        return None;
    };
    for (index, wanted) in record_ids.iter().enumerate() {
        if fed_ids[index] == *wanted {
            continue;
        }
        let address = inbox + chr_asm_param_id_offset(index);
        // The read above already proved this dword mapped and readable, on this thread, moments ago.
        // SAFETY: `address` is inside the renderer's own inbox `ChrAsm`, whose extent is fixed by
        // `CHR_ASM_SIZE`, and `index` is bounded by the array the read filled.
        unsafe { core::ptr::write_volatile(address as *mut i32, *wanted) };
    }
    // The record's own grip, latched here because this is the one place the record's `ChrAsm` is in
    // hand on the game thread. The renderer's live stage does NOT carry it (see the counter's own
    // doc), so the idle-anim choice reads this rather than `renderer+0x130`.
    if let Some(arm_style) = unsafe { safe_read_i32(record_chr_asm + CHR_ASM_EQUIPMENT_OFFSET) } {
        portrait_equip_latch_first(&PORTRAIT_EQUIP_RECORD_ARM_STYLE, arm_style);
    }
    PORTRAIT_EQUIP_RESTORE_KICKS.fetch_add(1, Ordering::SeqCst);
    if portrait_equip_restore_is_material(&report) {
        PORTRAIT_EQUIP_RESTORE_WEAPON_SLOTS
            .fetch_add(report.weapon_slots_restored, Ordering::SeqCst);
        PORTRAIT_EQUIP_RESTORE_AMMO_SLOTS.fetch_add(report.ammo_slots_restored, Ordering::SeqCst);
        PORTRAIT_EQUIP_RESTORE_PROTECTOR_SLOTS
            .fetch_add(report.protector_slots_restored, Ordering::SeqCst);
    } else {
        PORTRAIT_EQUIP_RESTORE_NOOP_KICKS.fetch_add(1, Ordering::SeqCst);
    }
    // Spelled out per index rather than zipped over `.iter()`: `scripts/check-oracle-writers.py`
    // recognises a writer by `&NAME[..]` or `NAME[..].store(`, and an iterator over the array is
    // neither -- so the zipped form read as a counter that is emitted but never written, which is
    // exactly the permanently-zero shape that gate exists to catch.
    portrait_equip_latch_first(&PORTRAIT_EQUIP_RESTORE_RECORD_ID[0], report.right_weapon_id);
    portrait_equip_latch_first(&PORTRAIT_EQUIP_RESTORE_RECORD_ID[1], report.left_weapon_id);
    portrait_equip_latch_first(&PORTRAIT_EQUIP_RESTORE_RECORD_ID[2], report.hands_id);
    portrait_equip_latch_first(&PORTRAIT_EQUIP_RESTORE_RECORD_ID[3], report.legs_id);
    // Logging lives HERE rather than at the call site: `scripts/check-crate-extraction-roadmap.py`
    // ratchets er-quickload's `experiments/**` down and never up, so the shim keeps only the seam
    // that has to sit between the native feed and the build kick, and everything that can be
    // reasoned about outside the DLL crate is reasoned about outside it.
    if PORTRAIT_EQUIP_RESTORE_KICKS.load(Ordering::SeqCst) <= RESTORE_LOG_KICK_LIMIT {
        append_autoload_debug(format_args!(
            "loading-portrait: equipment restored over the native feed for slot {slot}: weapons={} ammo={} protectors={} (record right={} left={} hands={} legs={})",
            report.weapon_slots_restored,
            report.ammo_slots_restored,
            report.protector_slots_restored,
            report.right_weapon_id,
            report.left_weapon_id,
            report.hands_id,
            report.legs_id,
        ));
    }
    Some(report)
}
