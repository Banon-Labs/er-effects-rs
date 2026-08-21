//! Pure loading-portrait equipment/protector oracle logic.
//!
//! The root DLL still owns the unsafe sampling of `CSMenuProfModelRend`/`ChrAsm` and the
//! telemetry publication counters. This module owns the deterministic product logic: how the
//! renderer resolves protector row ids from the live `ChrAsm`, how a sample is classified, and
//! how first-sample oracle values are packed/latch-published without losing a real zero id.

use std::sync::atomic::{AtomicUsize, Ordering};

/// This module packs an `i32` plus a presence bit into an `AtomicUsize`; the DLL and every host test
/// target are 64-bit. Fail the build rather than silently truncate if that ever stops holding.
const _: () = assert!(usize::BITS >= 64);

/// Number of protector (armor) slots covered by the loading-portrait equipment oracle:
/// head, chest, hands, legs.
pub const PORTRAIT_EQUIP_PROTECTOR_SLOT_COUNT: usize = 4;

/// The value `unk0`/`unkd4`/`unkd8` must hold for the renderer to dress a character from its per-slot
/// `equipment_param_ids`. Non-negative values force whole-outfit override arithmetic.
pub const PORTRAIT_EQUIP_OVERRIDE_ABSENT: i32 = -1;

/// Whole-outfit override category addends used by `FUN_1409e6fb0`.
pub const PORTRAIT_EQUIP_OVERRIDE_CHEST_ADDEND: i32 = 100;
pub const PORTRAIT_EQUIP_OVERRIDE_HANDS_ADDEND: i32 = 200;
pub const PORTRAIT_EQUIP_OVERRIDE_LEGS_ADDEND: i32 = 300;

/// `CS::ChrAsm::GetDefaultProtectorParamId`: 0 -> 10000, 1 -> 10100, 2 -> 10200, 3 -> 10300.
pub const PORTRAIT_EQUIP_PROTECTOR_DEFAULT_PARAM_ID_BASE: i32 = 10000;
pub const PORTRAIT_EQUIP_PROTECTOR_DEFAULT_PARAM_ID_STRIDE: i32 = 100;

/// Presence bit for a packed `i32` oracle value. A raw 0 means NEVER SAMPLED -- which matters because
/// 0 is also a perfectly representable (and, for this bug, highly diagnostic) param id.
pub const PORTRAIT_EQUIP_VALUE_PRESENT: usize = 1usize << 32;
/// What a packed slot decodes to when it was never sampled. Distinct from every real param id and
/// from the `-1` "slot legitimately empty" sentinel, so a reader can tell "no data" from "no armor".
pub const PORTRAIT_EQUIP_VALUE_UNSAMPLED: i32 = i32::MIN;

/// Bits of the per-sample failure mask. Published OR-ed across the window as
/// `oracle_portrait_equip_bad_mask`, so a failing window names WHICH condition fired.
/// A non-negative `unk0`/`unkd4`/`unkd8` -- the forced whole-outfit override; the nude root cause.
pub const PORTRAIT_EQUIP_BAD_OVERRIDE_ACTIVE: usize = 1 << 0;
/// The effective HEAD id is not the one the target save record carries.
pub const PORTRAIT_EQUIP_BAD_HEAD: usize = 1 << 1;
/// The effective CHEST id is not the one the target save record carries.
pub const PORTRAIT_EQUIP_BAD_CHEST: usize = 1 << 2;
/// The effective HANDS id is not the bare-body default the native feed equips into that slot.
pub const PORTRAIT_EQUIP_BAD_HANDS: usize = 1 << 3;
/// The effective LEGS id is not the bare-body default the native feed equips into that slot.
pub const PORTRAIT_EQUIP_BAD_LEGS: usize = 1 << 4;

/// Protector slot indices within the four the oracle covers.
pub const PORTRAIT_EQUIP_SLOT_HEAD: usize = 0;
pub const PORTRAIT_EQUIP_SLOT_CHEST: usize = 1;
pub const PORTRAIT_EQUIP_SLOT_HANDS: usize = 2;
pub const PORTRAIT_EQUIP_SLOT_LEGS: usize = 3;

/// Verdict values for `oracle_portrait_equip_capture_verdict`. Deliberately tri-state: "never sampled"
/// must NOT read as a pass, which is the `naked_kicks=0` false negative in a different costume.
pub const PORTRAIT_EQUIP_CAPTURE_NOT_SAMPLED: usize = 0;
pub const PORTRAIT_EQUIP_CAPTURE_CLEAN: usize = 1;
pub const PORTRAIT_EQUIP_CAPTURE_BAD: usize = 2;

/// One frame's reading of the live stage-0 `ChrAsm`, already reduced to what the renderer will act on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PortraitEquipSample {
    /// `ChrAsm::unk0` / `unkd4` / `unkd8` verbatim. All three are `-1` on a ctor-built `ChrAsm`; a
    /// non-negative value in any of them is the bug's signature and settles boot-vs-switch in one run.
    pub unk0: i32,
    pub unkd4: i32,
    pub unkd8: i32,
    /// The four `EquipParamProtector` row ids `FUN_1409e6fb0` will actually request, head/chest/hands/
    /// legs, after the override arithmetic.
    pub effective: [i32; PORTRAIT_EQUIP_PROTECTOR_SLOT_COUNT],
    /// The same four slots as the TARGET save record carries them, for classification.
    pub record: [i32; PORTRAIT_EQUIP_PROTECTOR_SLOT_COUNT],
    /// The `CS::ModelIns` this sample was taken against, carried so a reader can tell WHEN the
    /// sample happened relative to the swap. It is deliberately NOT an input to
    /// `portrait_equip_sample_bad_mask`: the open question (bd er-effects-rs-7m5y) is whether the
    /// non-capture bad frames all precede the model being rebuilt for the new character, and a
    /// value that both classifies and explains would beg that question.
    pub model_ins: usize,
}

/// The bare-body row `CS::ChrAsm::GetDefaultProtectorParamId` returns for a protector slot, and which
/// the native profile feed equips into HANDS and LEGS on every `set_model_source`.
pub fn protector_default_param_id(slot: usize) -> i32 {
    PORTRAIT_EQUIP_PROTECTOR_DEFAULT_PARAM_ID_BASE
        + PORTRAIT_EQUIP_PROTECTOR_DEFAULT_PARAM_ID_STRIDE * slot as i32
}

/// Replicate `FUN_1409e6fb0`'s protector resolution (deobf 0x1409e7553..0x1409e75b6, every test a
/// SIGNED `js`) for one slot. The per-slot param id is the baseline; a non-negative override field
/// replaces it outright.
pub fn portrait_effective_protector_id(
    slot: usize,
    param_id: i32,
    unk0: i32,
    unkd4: i32,
    unkd8: i32,
) -> i32 {
    match slot {
        PORTRAIT_EQUIP_SLOT_HEAD if unkd4 >= 0 => unkd4,
        PORTRAIT_EQUIP_SLOT_CHEST if unkd8 >= 0 => unkd8 + PORTRAIT_EQUIP_OVERRIDE_CHEST_ADDEND,
        PORTRAIT_EQUIP_SLOT_HANDS if unkd8 >= 0 => unkd8 + PORTRAIT_EQUIP_OVERRIDE_HANDS_ADDEND,
        PORTRAIT_EQUIP_SLOT_HANDS if unk0 >= 0 => unk0 + PORTRAIT_EQUIP_OVERRIDE_HANDS_ADDEND,
        PORTRAIT_EQUIP_SLOT_LEGS if unkd8 >= 0 => unkd8 + PORTRAIT_EQUIP_OVERRIDE_LEGS_ADDEND,
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
pub fn portrait_equip_sample_bad_mask(sample: &PortraitEquipSample) -> usize {
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

pub fn portrait_equip_pack(value: i32) -> usize {
    PORTRAIT_EQUIP_VALUE_PRESENT | (value as u32 as usize)
}

/// Decode a packed slot for publication. `PORTRAIT_EQUIP_VALUE_UNSAMPLED` when nothing was ever
/// latched -- never a plausible-looking 0.
pub fn portrait_equip_unpack(raw: usize) -> i32 {
    if raw & PORTRAIT_EQUIP_VALUE_PRESENT == 0 {
        PORTRAIT_EQUIP_VALUE_UNSAMPLED
    } else {
        raw as u32 as i32
    }
}

/// First writer wins. `compare_exchange` from 0 rather than `.store()` is the whole point: the value
/// the reader gets belongs to the FIRST frame of the window, not to whichever tick happened to run
/// last before the telemetry writer sampled.
pub fn portrait_equip_latch_first(cell: &AtomicUsize, value: i32) {
    let _ = cell.compare_exchange(
        0,
        portrait_equip_pack(value),
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        unk0: i32,
        unkd4: i32,
        unkd8: i32,
        param_ids: [i32; 4],
        record: [i32; 4],
    ) -> PortraitEquipSample {
        let mut effective = [PORTRAIT_EQUIP_OVERRIDE_ABSENT; PORTRAIT_EQUIP_PROTECTOR_SLOT_COUNT];
        for slot in 0..PORTRAIT_EQUIP_PROTECTOR_SLOT_COUNT {
            effective[slot] =
                portrait_effective_protector_id(slot, param_ids[slot], unk0, unkd4, unkd8);
        }
        PortraitEquipSample {
            unk0,
            unkd4,
            unkd8,
            effective,
            record,
            // Fixed: the classifier must not read it, and these cases assert exactly that.
            model_ins: 0,
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
        portrait_equip_latch_first(&cell, PORTRAIT_EQUIP_OVERRIDE_ABSENT);
        assert_eq!(
            portrait_equip_unpack(cell.load(Ordering::SeqCst)),
            PORTRAIT_EQUIP_OVERRIDE_ABSENT
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
}
