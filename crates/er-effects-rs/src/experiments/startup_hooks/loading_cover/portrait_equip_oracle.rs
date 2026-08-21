use super::*;

// LOADING-SCREEN PORTRAIT ARMOR ORACLE -- Layer 1 of bd er-effects-rs-91l5.
//
// WHY THE PREVIOUS ORACLE IS GONE RATHER THAN TUNED. `oracle_portrait_equip_slot_resolved` reported a
// clean 4/4 pass on the 2026-07-31 run the user saw render ENTIRELY nude. It was structurally
// incapable of catching the class, in four independent ways, and each one is inverted here:
//   (a) it read the renderer's ChrAsm at +0x548 -- the INBOX, which `STEP_Init_Setup` snapshots once
//       and never dereferences again. The value the model is built from is stage 0 at +0x130, which
//       `STEP_Wait_Play` re-reads EVERY frame. This samples +0x130.
//   (b) it sampled ONCE, microseconds after our own write, so nothing that changed the ChrAsm
//       afterwards could be seen. This samples every game tick a portrait model exists.
//   (c) it published through a bare `.store()`, so the emitted value belonged to no particular load
//       and a later good sample erased an earlier bad one. This publishes through per-window
//       accumulators: `fetch_add` for bad frames, `compare_exchange`-from-zero for first values, plus
//       a session total that nothing can decrement.
//   (d) it measured `equipment_param_ids`, which `FUN_1409e6fb0` OVERRIDES from `unkd4`/`unkd8`/`unk0`
//       -- a field with no causal power over the rendered result in the failing case. This replicates
//       the override arithmetic and publishes the EFFECTIVE ids the renderer actually resolves.
//
// This is a RAM oracle over the game's own memory. It proves which `EquipParamProtector` rows the
// engine asked for; it does NOT prove pixels. Layers 2 (per-part `CSPartsModelIns` binding) and 3
// (torso pixel check on the captured RT) remain open on er-effects-rs-91l5.

// Pure equipment-resolution/classification logic now lives in er-loading-portrait. This module keeps
// the root-owned unsafe memory sampling and telemetry publication, and re-exports the moved names as
// compatibility shims for existing root callsites.
pub(crate) use er_loading_portrait::portrait_equip::{
    PORTRAIT_EQUIP_CAPTURE_BAD, PORTRAIT_EQUIP_CAPTURE_CLEAN, PORTRAIT_EQUIP_CAPTURE_NOT_SAMPLED,
    PORTRAIT_EQUIP_SLOT_CHEST, PORTRAIT_EQUIP_SLOT_HANDS, PORTRAIT_EQUIP_SLOT_HEAD,
    PORTRAIT_EQUIP_SLOT_LEGS, PortraitEquipSample, portrait_effective_protector_id,
    portrait_equip_latch_first, portrait_equip_pack, portrait_equip_sample_bad_mask,
    portrait_equip_unpack,
};

pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_BAD_FRAMES;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_BAD_FRAMES_TOTAL;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_BAD_MASK;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_CAPTURE_VERDICT;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNK0;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNKD4;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_FIRST_UNKD8;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_ORACLE_SLOT;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_ORACLE_WINDOW;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_RECORD_PARAM_ID;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_SAMPLED_FRAMES;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOW_OPEN_MODEL_INS;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_BAD;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_SAMPLED;

/// Byte offset of protector `slot`'s entry within a `ChrAsm`'s `equipment_param_ids` array.
/// `CS::ChrAsm::GetProtectorParamIdBySlot` (deobf 0x1403be950) is literally
/// `lea 0xc(%rdx),%eax ; movslq %eax,%rdx ; mov 0x7c(%rcx,%rdx,4),%eax ; ret`.
pub(crate) fn chr_asm_protector_param_id_offset(slot: usize) -> usize {
    CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
        + (CHR_ASM_PROTECTOR_HEAD_INDEX + slot) * core::mem::size_of::<i32>()
}

/// Roll the per-window accumulators over when a new loading-owned profile table is built.
/// `PROFILE_LOADSCREEN_TABLE_BUILDS` increments exactly once per load window, in
/// `maybe_build_profile_table_for_loading`, which is also where the spec's window starts.
pub(crate) fn portrait_equip_roll_window(window: usize) {
    if PORTRAIT_EQUIP_ORACLE_WINDOW.swap(window, Ordering::SeqCst) == window {
        return;
    }
    PORTRAIT_EQUIP_SAMPLED_FRAMES.store(0, Ordering::SeqCst);
    PORTRAIT_EQUIP_BAD_FRAMES.store(0, Ordering::SeqCst);
    PORTRAIT_EQUIP_BAD_MASK.store(0, Ordering::SeqCst);
    PORTRAIT_EQUIP_CAPTURE_VERDICT.store(PORTRAIT_EQUIP_CAPTURE_NOT_SAMPLED, Ordering::SeqCst);
    PORTRAIT_EQUIP_ORACLE_SLOT.store(0, Ordering::SeqCst);
    PORTRAIT_EQUIP_FIRST_UNK0.store(0, Ordering::SeqCst);
    PORTRAIT_EQUIP_FIRST_UNKD4.store(0, Ordering::SeqCst);
    PORTRAIT_EQUIP_FIRST_UNKD8.store(0, Ordering::SeqCst);
    for slot in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
        PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID[slot].store(0, Ordering::SeqCst);
        PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID[slot].store(0, Ordering::SeqCst);
        PORTRAIT_EQUIP_RECORD_PARAM_ID[slot].store(0, Ordering::SeqCst);
    }
}

/// Read the LIVE stage-0 `ChrAsm` of `slot`'s profile renderer, or `None` when there is nothing
/// meaningful to measure this tick. The pool pointer is re-read on EVERY call and vtable-guarded --
/// `FUN_1409af3a0` deletes and reconstructs all ten renderers on every `TitleTopDialog` construction,
/// so a cached pointer goes stale mid-session.
///
/// `None` (rather than a sample) when:
///   * the pool entry is null or does not carry the `CSMenuProfModelRend` vtable;
///   * no model instance exists at +0x778 -- nothing is being resolved, so there is no rendered
///     outcome to judge;
///   * stage 0 is still ctor-fresh (every protector id `-1`). The native feed always equips the
///     bare-body defaults into hands and legs, so a configured stage 0 always has at least those two
///     non-negative; counting the pre-feed frames would report a failure for a renderer that simply
///     has not been fed yet;
///   * any of the fault-guarded reads comes back unmapped, including the one-per-window read of the
///     target record's own protector ids (the next tick simply retries).
///
/// A window that produces zero samples is itself a FAILURE verdict -- see `oracle_portrait_equip_sampled_frames`.
pub(crate) unsafe fn portrait_equip_read_sample(
    base: usize,
    summary: usize,
    slot: i32,
) -> Option<PortraitEquipSample> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return None;
    }
    let renderer = unsafe { safe_read_usize(portrait_renderer_table_entry(base, slot)) }?;
    if renderer == 0 || renderer == null {
        return None;
    }
    if unsafe { safe_read_usize(renderer) }?
        != base + TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA
    {
        return None;
    }
    let model_ins = unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }?;
    if model_ins == 0 || model_ins == null {
        return None;
    }
    let chr_asm = renderer + PROFILE_RENDERER_CHR_ASM_LIVE_OFFSET;
    let unk0 = unsafe { safe_read_i32(chr_asm + CHR_ASM_UNK0_OFFSET) }?;
    let unkd4 = unsafe { safe_read_i32(chr_asm + CHR_ASM_UNKD4_OFFSET) }?;
    let unkd8 = unsafe { safe_read_i32(chr_asm + CHR_ASM_UNKD8_OFFSET) }?;
    let mut param_ids = [CHR_ASM_OVERRIDE_ABSENT; CHR_ASM_PROTECTOR_SLOT_COUNT];
    for (slot_index, param_id) in param_ids.iter_mut().enumerate() {
        *param_id =
            unsafe { safe_read_i32(chr_asm + chr_asm_protector_param_id_offset(slot_index)) }?;
    }
    if param_ids.iter().all(|id| *id < 0) {
        return None; // ctor-fresh stage 0: the renderer has not been fed yet.
    }
    // The record's own ids are read ONCE per window and then served from the latch. Two reasons, and
    // the second is the load-bearing one: it keeps 4 `ReadProcessMemory` calls off a per-tick path
    // that already runs dozens, and it freezes the pass criterion for the whole window, so a mid-window
    // rewrite of the record cannot retroactively make an earlier bad frame look correct.
    let mut record_ids = [CHR_ASM_OVERRIDE_ABSENT; CHR_ASM_PROTECTOR_SLOT_COUNT];
    let cached = PORTRAIT_EQUIP_RECORD_PARAM_ID[0].load(Ordering::SeqCst) != 0;
    let record = profile_summary_record_address(summary, slot as usize);
    for slot_index in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
        record_ids[slot_index] = if cached {
            portrait_equip_unpack(PORTRAIT_EQUIP_RECORD_PARAM_ID[slot_index].load(Ordering::SeqCst))
        } else {
            unsafe {
                safe_read_i32(
                    record
                        + PROFILE_SUMMARY_CHR_ASM_OFFSET
                        + chr_asm_protector_param_id_offset(slot_index),
                )
            }?
        };
    }
    let mut effective = [CHR_ASM_OVERRIDE_ABSENT; CHR_ASM_PROTECTOR_SLOT_COUNT];
    for slot_index in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
        effective[slot_index] =
            portrait_effective_protector_id(slot_index, param_ids[slot_index], unk0, unkd4, unkd8);
    }
    Some(PortraitEquipSample {
        unk0,
        unkd4,
        unkd8,
        effective,
        record: record_ids,
        model_ins,
    })
}

/// Sample the live stage-0 `ChrAsm` of the profile renderer the loading-screen pipeline is driving,
/// and fold the result into this load window's accumulators. Called from `force_profile_render_tick`
/// on EVERY game tick the pipeline runs, with the same `target_slot` the tick kicks and captures --
/// so the oracle can never end up measuring a renderer other than the displayed one.
///
/// The MANDATORY capture-frame sample rides the same tick loop: when `PROFILE_BAKE_RGBA_CAPTURED`
/// reads set and this window has not recorded a capture verdict yet, the sample is additionally
/// latched into the `..._capture_*` oracles. Stated plainly, that is the first GAME TICK on which the
/// latch is observable, not literally the worker's own frame -- the readback worker sets the latch off
/// the game thread and must not read game memory. Stage 0 only changes at `STEP_Finish_Setup`, so a
/// one-tick skew cannot straddle a rebuild without `sampled_frames` also showing it.
pub(crate) unsafe fn portrait_equip_oracle_sample(base: usize, summary: usize, target_slot: i32) {
    portrait_equip_roll_window(PROFILE_LOADSCREEN_TABLE_BUILDS.load(Ordering::SeqCst));
    let Some(sample) = (unsafe { portrait_equip_read_sample(base, summary, target_slot) }) else {
        return;
    };
    let mask = portrait_equip_sample_bad_mask(&sample);
    let sampled = PORTRAIT_EQUIP_SAMPLED_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
    if sampled == 1 {
        PORTRAIT_EQUIP_WINDOWS_SAMPLED.fetch_add(1, Ordering::SeqCst);
        PORTRAIT_EQUIP_ORACLE_SLOT.store((target_slot + 1) as usize, Ordering::SeqCst);
        // The model the window OPENED against. If every bad frame carries this value and the clean
        // ones carry a different one, the mismatch is a pre-propagation sampling artifact and the
        // counting edge is `model_ins != this`. If a bad frame carries a DIFFERENT model, it is a
        // real post-rebuild defect. One run decides it; nothing here assumes which.
        PORTRAIT_EQUIP_WINDOW_OPEN_MODEL_INS.store(sample.model_ins, Ordering::SeqCst);
    }
    portrait_equip_latch_first(&PORTRAIT_EQUIP_FIRST_UNK0, sample.unk0);
    portrait_equip_latch_first(&PORTRAIT_EQUIP_FIRST_UNKD4, sample.unkd4);
    portrait_equip_latch_first(&PORTRAIT_EQUIP_FIRST_UNKD8, sample.unkd8);
    for slot in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
        portrait_equip_latch_first(
            &PORTRAIT_EQUIP_FIRST_EFFECTIVE_ID[slot],
            sample.effective[slot],
        );
        portrait_equip_latch_first(&PORTRAIT_EQUIP_RECORD_PARAM_ID[slot], sample.record[slot]);
    }
    if mask != 0 {
        PORTRAIT_EQUIP_BAD_MASK.fetch_or(mask, Ordering::SeqCst);
        let bad = PORTRAIT_EQUIP_BAD_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
        PORTRAIT_EQUIP_BAD_FRAMES_TOTAL.fetch_add(1, Ordering::SeqCst);
        if bad == 1 {
            PORTRAIT_EQUIP_WINDOWS_BAD.fetch_add(1, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "PORTRAIT-EQUIP FAIL slot={target_slot} mask=0b{mask:05b} model_ins=0x{:x} window_open_model_ins=0x{:x} effective=[head {} chest {} hands {} legs {}] record=[head {} chest {}] unk0={} unkd4={} unkd8={} -- the portrait will not render this character's armor",
                sample.model_ins,
                PORTRAIT_EQUIP_WINDOW_OPEN_MODEL_INS.load(Ordering::SeqCst),
                sample.effective[PORTRAIT_EQUIP_SLOT_HEAD],
                sample.effective[PORTRAIT_EQUIP_SLOT_CHEST],
                sample.effective[PORTRAIT_EQUIP_SLOT_HANDS],
                sample.effective[PORTRAIT_EQUIP_SLOT_LEGS],
                sample.record[PORTRAIT_EQUIP_SLOT_HEAD],
                sample.record[PORTRAIT_EQUIP_SLOT_CHEST],
                sample.unk0,
                sample.unkd4,
                sample.unkd8,
            ));
        }
    }
    if PROFILE_BAKE_RGBA_CAPTURED.load(Ordering::SeqCst) != 0
        && PORTRAIT_EQUIP_CAPTURE_VERDICT.load(Ordering::SeqCst)
            == PORTRAIT_EQUIP_CAPTURE_NOT_SAMPLED
    {
        for (slot, captured) in PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID
            .iter()
            .enumerate()
            .take(CHR_ASM_PROTECTOR_SLOT_COUNT)
        {
            captured.store(
                portrait_equip_pack(sample.effective[slot]),
                Ordering::SeqCst,
            );
        }
        PORTRAIT_EQUIP_CAPTURE_VERDICT.store(
            if mask == 0 {
                PORTRAIT_EQUIP_CAPTURE_CLEAN
            } else {
                PORTRAIT_EQUIP_CAPTURE_BAD
            },
            Ordering::SeqCst,
        );
        append_autoload_debug(format_args!(
            "portrait-equip: capture-frame sample slot={target_slot} verdict={} effective=[head {} chest {} hands {} legs {}] (window {} sampled_frames={} bad_frames={})",
            if mask == 0 { "clean" } else { "BAD" },
            sample.effective[PORTRAIT_EQUIP_SLOT_HEAD],
            sample.effective[PORTRAIT_EQUIP_SLOT_CHEST],
            sample.effective[PORTRAIT_EQUIP_SLOT_HANDS],
            sample.effective[PORTRAIT_EQUIP_SLOT_LEGS],
            PORTRAIT_EQUIP_ORACLE_WINDOW.load(Ordering::SeqCst),
            sampled,
            PORTRAIT_EQUIP_BAD_FRAMES.load(Ordering::SeqCst),
        ));
    }
}

#[cfg(test)]
mod portrait_equip_oracle_root_tests {
    use super::*;

    /// The protector param-id offsets are pinned by `GetProtectorParamIdBySlot`'s `lea 0xc(%rdx)` +
    /// `mov 0x7c(%rcx,%rdx,4)`, and all four must stay inside the struct.
    #[test]
    fn protector_param_id_offsets_are_head_chest_hands_legs_and_stay_in_bounds() {
        let head = chr_asm_protector_param_id_offset(PORTRAIT_EQUIP_SLOT_HEAD);
        assert_eq!(
            head,
            CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
                + CHR_ASM_PROTECTOR_HEAD_INDEX * core::mem::size_of::<i32>()
        );
        for slot in 1..CHR_ASM_PROTECTOR_SLOT_COUNT {
            assert_eq!(
                chr_asm_protector_param_id_offset(slot),
                head + slot * core::mem::size_of::<i32>()
            );
        }
        let last = chr_asm_protector_param_id_offset(CHR_ASM_PROTECTOR_SLOT_COUNT - 1);
        assert!(last + core::mem::size_of::<i32>() <= CHR_ASM_SIZE);
    }
}
