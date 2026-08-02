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

/// This module packs an `i32` plus a presence bit into an `AtomicUsize`; the DLL and every host test
/// target are 64-bit. Fail the build rather than silently truncate if that ever stops holding.
const _: () = assert!(usize::BITS >= 64);

/// Presence bit for a packed `i32` oracle value. A raw 0 means NEVER SAMPLED -- which matters because
/// 0 is also a perfectly representable (and, for this bug, highly diagnostic) param id.
pub(crate) const PORTRAIT_EQUIP_VALUE_PRESENT: usize = 1usize << 32;
/// What a packed slot decodes to when it was never sampled. Distinct from every real param id and
/// from the `-1` "slot legitimately empty" sentinel, so a reader can tell "no data" from "no armor".
pub(crate) const PORTRAIT_EQUIP_VALUE_UNSAMPLED: i32 = i32::MIN;

/// Bits of the per-sample failure mask. Published OR-ed across the window as
/// `oracle_portrait_equip_bad_mask`, so a failing window names WHICH condition fired.
/// A non-negative `unk0`/`unkd4`/`unkd8` -- the forced whole-outfit override; the nude root cause.
pub(crate) const PORTRAIT_EQUIP_BAD_OVERRIDE_ACTIVE: usize = 1 << 0;
/// The effective HEAD id is not the one the target save record carries.
pub(crate) const PORTRAIT_EQUIP_BAD_HEAD: usize = 1 << 1;
/// The effective CHEST id is not the one the target save record carries.
pub(crate) const PORTRAIT_EQUIP_BAD_CHEST: usize = 1 << 2;
/// The effective HANDS id is not the bare-body default the native feed equips into that slot.
pub(crate) const PORTRAIT_EQUIP_BAD_HANDS: usize = 1 << 3;
/// The effective LEGS id is not the bare-body default the native feed equips into that slot.
pub(crate) const PORTRAIT_EQUIP_BAD_LEGS: usize = 1 << 4;

/// Protector slot indices within the four the oracle covers.
pub(crate) const PORTRAIT_EQUIP_SLOT_HEAD: usize = 0;
pub(crate) const PORTRAIT_EQUIP_SLOT_CHEST: usize = 1;
pub(crate) const PORTRAIT_EQUIP_SLOT_HANDS: usize = 2;
pub(crate) const PORTRAIT_EQUIP_SLOT_LEGS: usize = 3;

/// Verdict values for `oracle_portrait_equip_capture_verdict`. Deliberately tri-state: "never sampled"
/// must NOT read as a pass, which is the `naked_kicks=0` false negative in a different costume.
pub(crate) const PORTRAIT_EQUIP_CAPTURE_NOT_SAMPLED: usize = 0;
pub(crate) const PORTRAIT_EQUIP_CAPTURE_CLEAN: usize = 1;
pub(crate) const PORTRAIT_EQUIP_CAPTURE_BAD: usize = 2;

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
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_BAD;
pub(crate) use er_telemetry::counters::PORTRAIT_EQUIP_WINDOWS_SAMPLED;

/// One frame's reading of the live stage-0 `ChrAsm`, already reduced to what the renderer will act on.
#[derive(Clone, Copy)]
pub(crate) struct PortraitEquipSample {
    /// `ChrAsm::unk0` / `unkd4` / `unkd8` verbatim. All three are `-1` on a ctor-built `ChrAsm`; a
    /// non-negative value in any of them is the bug's signature and settles boot-vs-switch in one run.
    unk0: i32,
    unkd4: i32,
    unkd8: i32,
    /// The four `EquipParamProtector` row ids `FUN_1409e6fb0` will actually request, head/chest/hands/
    /// legs, after the override arithmetic.
    effective: [i32; CHR_ASM_PROTECTOR_SLOT_COUNT],
    /// The same four slots as the TARGET save record carries them, for the comparison below.
    record: [i32; CHR_ASM_PROTECTOR_SLOT_COUNT],
}

/// Byte offset of protector `slot`'s entry within a `ChrAsm`'s `equipment_param_ids` array.
/// `CS::ChrAsm::GetProtectorParamIdBySlot` (deobf 0x1403be950) is literally
/// `lea 0xc(%rdx),%eax ; movslq %eax,%rdx ; mov 0x7c(%rcx,%rdx,4),%eax ; ret`.
pub(crate) fn chr_asm_protector_param_id_offset(slot: usize) -> usize {
    CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
        + (CHR_ASM_PROTECTOR_HEAD_INDEX + slot) * core::mem::size_of::<i32>()
}

/// The bare-body row `CS::ChrAsm::GetDefaultProtectorParamId` returns for a protector slot, and which
/// the native profile feed equips into HANDS and LEGS on every `set_model_source`.
pub(crate) fn protector_default_param_id(slot: usize) -> i32 {
    PROTECTOR_DEFAULT_PARAM_ID_BASE + PROTECTOR_DEFAULT_PARAM_ID_STRIDE * slot as i32
}

/// Replicate `FUN_1409e6fb0`'s protector resolution (deobf 0x1409e7553..0x1409e75b6, every test a
/// SIGNED `js`) for one slot. The per-slot param id is the baseline; a non-negative override field
/// replaces it outright.
pub(crate) fn portrait_effective_protector_id(
    slot: usize,
    param_id: i32,
    unk0: i32,
    unkd4: i32,
    unkd8: i32,
) -> i32 {
    match slot {
        PORTRAIT_EQUIP_SLOT_HEAD if unkd4 >= 0 => unkd4,
        PORTRAIT_EQUIP_SLOT_CHEST if unkd8 >= 0 => unkd8 + CHR_ASM_OVERRIDE_CHEST_ADDEND,
        PORTRAIT_EQUIP_SLOT_HANDS if unkd8 >= 0 => unkd8 + CHR_ASM_OVERRIDE_HANDS_ADDEND,
        PORTRAIT_EQUIP_SLOT_HANDS if unk0 >= 0 => unk0 + CHR_ASM_OVERRIDE_HANDS_ADDEND,
        PORTRAIT_EQUIP_SLOT_LEGS if unkd8 >= 0 => unkd8 + CHR_ASM_OVERRIDE_LEGS_ADDEND,
        _ => param_id,
    }
}

/// Classify one sample. Returns the OR of the `PORTRAIT_EQUIP_BAD_*` bits; 0 = this frame would render
/// the character's own armor.
///
/// HEAD and CHEST are compared against the RECORD's own ids, which is stronger than any absolute
/// row-id floor and needs no unverifiable magic number: an empty slot is `-1` in both places and
/// passes, exactly as a bare-headed character should. HANDS and LEGS are compared against the
/// bare-body defaults instead, because the native feed overwrites those two with
/// `GetDefaultProtectorParamId(2)` / `(3)` immediately after copying the record -- a portrait wearing
/// its own gauntlets would be the deviation, not the fix.
pub(crate) fn portrait_equip_sample_bad_mask(sample: &PortraitEquipSample) -> usize {
    let mut mask = 0usize;
    if sample.unk0 >= 0 || sample.unkd4 >= 0 || sample.unkd8 >= 0 {
        mask |= PORTRAIT_EQUIP_BAD_OVERRIDE_ACTIVE;
    }
    if sample.effective[PORTRAIT_EQUIP_SLOT_HEAD] != sample.record[PORTRAIT_EQUIP_SLOT_HEAD] {
        mask |= PORTRAIT_EQUIP_BAD_HEAD;
    }
    if sample.effective[PORTRAIT_EQUIP_SLOT_CHEST] != sample.record[PORTRAIT_EQUIP_SLOT_CHEST] {
        mask |= PORTRAIT_EQUIP_BAD_CHEST;
    }
    if sample.effective[PORTRAIT_EQUIP_SLOT_HANDS]
        != protector_default_param_id(PORTRAIT_EQUIP_SLOT_HANDS)
    {
        mask |= PORTRAIT_EQUIP_BAD_HANDS;
    }
    if sample.effective[PORTRAIT_EQUIP_SLOT_LEGS]
        != protector_default_param_id(PORTRAIT_EQUIP_SLOT_LEGS)
    {
        mask |= PORTRAIT_EQUIP_BAD_LEGS;
    }
    mask
}

pub(crate) fn portrait_equip_pack(value: i32) -> usize {
    PORTRAIT_EQUIP_VALUE_PRESENT | (value as u32 as usize)
}

/// Decode a packed slot for publication. `PORTRAIT_EQUIP_VALUE_UNSAMPLED` when nothing was ever
/// latched -- never a plausible-looking 0.
pub(crate) fn portrait_equip_unpack(raw: usize) -> i32 {
    if raw & PORTRAIT_EQUIP_VALUE_PRESENT == 0 {
        PORTRAIT_EQUIP_VALUE_UNSAMPLED
    } else {
        raw as u32 as i32
    }
}

/// First writer wins. `compare_exchange` from 0 rather than `.store()` is the whole point: the value
/// the reader gets belongs to the FIRST frame of the window, not to whichever tick happened to run
/// last before the telemetry writer sampled.
pub(crate) fn portrait_equip_latch_first(cell: &AtomicUsize, value: i32) {
    let _ = cell.compare_exchange(
        0,
        portrait_equip_pack(value),
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
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
    for slot_index in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
        param_ids[slot_index] =
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
    let record =
        summary + PROFILE_SUMMARY_RECORD_BASE + slot as usize * PROFILE_SUMMARY_RECORD_STRIDE;
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
                "PORTRAIT-EQUIP FAIL slot={target_slot} mask=0b{mask:05b} effective=[head {} chest {} hands {} legs {}] record=[head {} chest {}] unk0={} unkd4={} unkd8={} -- the portrait will not render this character's armor",
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
        for slot in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
            PORTRAIT_EQUIP_CAPTURE_EFFECTIVE_ID[slot].store(
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
mod portrait_equip_oracle_tests {

    fn sample(
        unk0: i32,
        unkd4: i32,
        unkd8: i32,
        param_ids: [i32; 4],
        record: [i32; 4],
    ) -> PortraitEquipSample {
        let mut effective = [CHR_ASM_OVERRIDE_ABSENT; CHR_ASM_PROTECTOR_SLOT_COUNT];
        for slot in 0..CHR_ASM_PROTECTOR_SLOT_COUNT {
            effective[slot] =
                portrait_effective_protector_id(slot, param_ids[slot], unk0, unkd4, unkd8);
        }
        PortraitEquipSample {
            unk0,
            unkd4,
            unkd8,
            effective,
            record,
        }
    }

    /// The exact state our zero-filled image produced: all four slots resolve to rows that do not
    /// exist, so nothing renders -- default underwear included. This is the sample the OLD oracle
    /// scored as a clean 4/4 pass.
    #[test]
    fn a_zeroed_chr_asm_resolves_every_protector_slot_to_a_bogus_row_and_is_flagged() {
        let s = sample(
            0,
            0,
            0,
            [21000, 21100, 10200, 10300],
            [21000, 21100, -1, -1],
        );
        assert_eq!(s.effective, [0, 100, 200, 300]);
        let mask = portrait_equip_sample_bad_mask(&s);
        assert!(mask & PORTRAIT_EQUIP_BAD_OVERRIDE_ACTIVE != 0);
        assert!(mask & PORTRAIT_EQUIP_BAD_HEAD != 0);
        assert!(mask & PORTRAIT_EQUIP_BAD_CHEST != 0);
        assert!(mask & PORTRAIT_EQUIP_BAD_HANDS != 0);
        assert!(mask & PORTRAIT_EQUIP_BAD_LEGS != 0);
    }

    /// The fixed image: sentinels at -1, so the per-slot param ids stand and the record's armor is
    /// what the renderer asks for.
    #[test]
    fn the_sentinel_image_resolves_the_records_own_armor_and_passes() {
        let s = sample(
            -1,
            -1,
            -1,
            [21000, 21100, 10200, 10300],
            [21000, 21100, -1, -1],
        );
        assert_eq!(s.effective, [21000, 21100, 10200, 10300]);
        assert_eq!(portrait_equip_sample_bad_mask(&s), 0);
    }

    /// A character wearing NO head or chest armor is legitimately bare there. Comparing against the
    /// record rather than an absolute row-id floor is what keeps that from reading as a failure.
    #[test]
    fn an_unarmored_character_is_not_a_failure() {
        let s = sample(-1, -1, -1, [-1, -1, 10200, 10300], [-1, -1, -1, -1]);
        assert_eq!(portrait_equip_sample_bad_mask(&s), 0);
    }

    /// A dead handle makes `EquipItem` write -1 over a good param id. The record still names the
    /// armor, so the mismatch is caught -- the class PR #128 was built around, still covered.
    #[test]
    fn armor_lost_between_the_record_and_the_live_chr_asm_is_flagged() {
        let s = sample(-1, -1, -1, [-1, -1, 10200, 10300], [21000, 21100, -1, -1]);
        assert_eq!(
            portrait_equip_sample_bad_mask(&s),
            PORTRAIT_EQUIP_BAD_HEAD | PORTRAIT_EQUIP_BAD_CHEST
        );
    }

    /// The hands-only fallback branch: `unkd8` negative but `unk0` non-negative still overrides hands
    /// (`mov (%rcx),%eax ; test ; js ; lea 0xc8(%rax),%ebx` at deobf 0x1409e758f).
    #[test]
    fn a_non_negative_unk0_overrides_hands_alone() {
        let s = sample(
            5,
            -1,
            -1,
            [21000, 21100, 10200, 10300],
            [21000, 21100, -1, -1],
        );
        assert_eq!(s.effective, [21000, 21100, 205, 10300]);
        assert_eq!(
            portrait_equip_sample_bad_mask(&s),
            PORTRAIT_EQUIP_BAD_OVERRIDE_ACTIVE | PORTRAIT_EQUIP_BAD_HANDS
        );
    }

    /// 0 is a representable param id AND this bug's signature, so "never sampled" must not decode to
    /// it. `compare_exchange` from 0 must also latch the FIRST value, not the last.
    #[test]
    fn an_unsampled_slot_never_decodes_to_a_plausible_param_id() {
        let cell = AtomicUsize::new(0);
        assert_eq!(
            portrait_equip_unpack(cell.load(Ordering::SeqCst)),
            PORTRAIT_EQUIP_VALUE_UNSAMPLED
        );
        portrait_equip_latch_first(&cell, 0);
        assert_eq!(portrait_equip_unpack(cell.load(Ordering::SeqCst)), 0);
        portrait_equip_latch_first(&cell, 21000);
        assert_eq!(
            portrait_equip_unpack(cell.load(Ordering::SeqCst)),
            0,
            "first value must win; a later good sample cannot erase a bad one"
        );
    }

    #[test]
    fn negative_param_ids_survive_the_pack_round_trip() {
        let cell = AtomicUsize::new(0);
        portrait_equip_latch_first(&cell, CHR_ASM_OVERRIDE_ABSENT);
        assert_eq!(
            portrait_equip_unpack(cell.load(Ordering::SeqCst)),
            CHR_ASM_OVERRIDE_ABSENT
        );
    }

    /// The bare-body rows the native feed equips into hands and legs, straight off the switch in
    /// `GetDefaultProtectorParamId`.
    #[test]
    fn the_default_protector_rows_are_the_documented_switch_values() {
        assert_eq!(protector_default_param_id(PORTRAIT_EQUIP_SLOT_HEAD), 10000);
        assert_eq!(protector_default_param_id(PORTRAIT_EQUIP_SLOT_CHEST), 10100);
        assert_eq!(protector_default_param_id(PORTRAIT_EQUIP_SLOT_HANDS), 10200);
        assert_eq!(protector_default_param_id(PORTRAIT_EQUIP_SLOT_LEGS), 10300);
    }

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
