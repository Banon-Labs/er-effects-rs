//! Undo the two mutilations the native profile feed performs on the ChrAsm it is handed.
//!
//! `FUN_140bbe1a0` -- what `PROFILE_RENDERER_SET_MODEL_SOURCE_RVA` names and what the loading
//! portrait's per-slot build kick calls -- is not a plain "install this character's equipment".
//! Decompiled from the 1.16.2 named dump (and byte-identical on 1.17: `docs/recon/
//! rva-map-1162-to-1170.needed-verified.tsv` maps `0x140bbe1a0 -> 0x140bbf870` IDENTICAL-WHOLE,
//! score 1.000 over 119 instructions), it does exactly three things:
//!
//! ```text
//!   FUN_140bb9850(renderer, source)   -> ChrAsm::Copy(renderer+0x548, source)   // our record
//!   EquipItemBySpecialIndex(chrAsm, ~RightWeaponSlot,  {0})                     // ... x8, all
//!   EquipItemBySpecialIndex(chrAsm, ~LeftWeaponSlot,   {0})                     // with a ZERO
//!   EquipItemBySpecialIndex(chrAsm, ~RightBoltSlot,    {0})                     // InventoryItem
//!   EquipItemBySpecialIndex(chrAsm, ~LeftBoltSlot,     {0})                     // entry, i.e.
//!   EquipItemBySpecialIndex(chrAsm, 6..9,              {0})                     // a CLEAR
//!   EquipProtectorOrAccessory(chrAsm, 2, handle(GetDefaultProtectorParamId(2)))  // hands
//!   EquipProtectorOrAccessory(chrAsm, 3, handle(GetDefaultProtectorParamId(3)))  // legs
//! ```
//!
//! `EquipItemBySpecialIndex` (0x1403bf660) is `EquipItem(chrAsm, GetBySpecialIndex(chrAsm, i), &h)`
//! and `EquipItem` stores `-1` into `equipment_param_ids[slot]` when the handle resolves to nothing,
//! so those eight calls with a zeroed entry are erasures. `EquipProtectorOrAccessory` (0x1403bf490)
//! is `EquipItem(chrAsm, slot + ProtectorHead, h)`, so the last two overwrite the character's own
//! gauntlets and greaves with the bare-body rows 10200/10300.
//!
//! THAT IS THE WHOLE BUG, BOTH HALVES OF IT. The portrait shows no arm or leg armour because the
//! feed replaced them, and it shows no weapon -- and therefore no handedness -- because the feed
//! erased them. Neither is a limitation of the render path: the per-frame model-resource request
//! `FUN_1409e6fb0` resolves armaments as thoroughly as armour (`EquipParamWeapon::GetEntry`,
//! `weaponCategory` tests) and reads handedness straight out of the ChrAsm
//! (`getSelectedWeaponSlotIndex(&param_2->equipment.armStyle, 0|1)` -> `selectedWeaponSlotIndex`).
//! Hand it a ChrAsm that still has its weapons and it draws them.
//!
//! So the repair is to write the record's own `equipment_param_ids` back over the fed array after
//! the feed returns and before the build is kicked. PARAM IDS ONLY, never the gaitem handles: the
//! render path resolves equipment from the id array alone (`GetProtectorParamIdBySlot` is
//! `mov 0x7c(%rcx,%rdx,4),%eax`), while the handle array is refcounted state this process owns --
//! the feed put two REAL default-protector handles in it, and stealing or overwriting those is how
//! PR #128 broke refcounts without fixing the picture.
//!
//! This module is the deterministic half: which indices differ, and what to report about them. The
//! unsafe read/write of live renderer memory stays with the caller.

use crate::portrait_equip::{PORTRAIT_EQUIP_SLOT_HANDS, PORTRAIT_EQUIP_SLOT_LEGS};

/// Entries in `ChrAsm::equipment_param_ids`, pinned by the ctor's `mov $0x16,%ecx ; rep stos`.
/// Spelled here rather than imported from `chr_asm_layout` because that module is Windows-gated
/// (it derives offsets from the `eldenring` binding) and this logic is host-testable.
pub const PORTRAIT_EQUIP_ENTRY_COUNT: usize = 22;

/// First `equipment_param_ids` index of the six armament slots -- left/right x primary/secondary/
/// tertiary. `GetBySpecialIndex` (0x1403be430) returns `selected*2` for `LeftWeaponSlot` and
/// `selected*2 + 1` for `RightWeaponSlot`, so the pairing is left-even / right-odd from zero.
pub const PORTRAIT_EQUIP_WEAPON_FIRST: usize = 0;
pub const PORTRAIT_EQUIP_WEAPON_COUNT: usize = 6;
/// First index of the six ammunition slots; `er_build_import_core::equip::CHR_ASM_SLOT_AMMO_1`
/// carries the same 6 and the two are checked against each other by this module's tests.
pub const PORTRAIT_EQUIP_AMMO_FIRST: usize = 6;
pub const PORTRAIT_EQUIP_AMMO_COUNT: usize = 6;
/// First index of the four protector slots (head, chest, hands, legs).
/// `CS::ChrAsm::EquipProtectorOrAccessory` is literally `add $0xc,%edx ; jmp EquipItem`.
pub const PORTRAIT_EQUIP_PROTECTOR_FIRST: usize = 12;

/// An empty slot, in both the record and the fed array. `EquipItem` writes it on a failed handle
/// lookup, and the ctor `rep stos`es the whole array with it.
pub const PORTRAIT_EQUIP_EMPTY_ID: i32 = -1;

/// What the feed did to one character's equipment, and what putting it back changes.
///
/// Every count is "indices where the fed array disagrees with the record", so a character who
/// genuinely wears nothing on their arms and carries nothing in either hand produces a report of
/// all zeroes -- the repair is a no-op for them, and the oracle must not read that as a failure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PortraitEquipRestore {
    /// Armament indices the feed erased that the record fills.
    pub weapon_slots_restored: usize,
    /// Ammunition indices the feed erased that the record fills.
    pub ammo_slots_restored: usize,
    /// Protector indices the feed overwrote (in practice hands and legs, never head or chest).
    pub protector_slots_restored: usize,
    /// The record's first non-empty RIGHT-hand armament (odd indices), or [`PORTRAIT_EQUIP_EMPTY_ID`].
    pub right_weapon_id: i32,
    /// The record's first non-empty LEFT-hand armament (even indices).
    pub left_weapon_id: i32,
    /// The record's own hands (gauntlets) row, whatever the feed replaced it with.
    pub hands_id: i32,
    /// The record's own legs (greaves) row.
    pub legs_id: i32,
}

/// Compare the record's `equipment_param_ids` against the array the native feed left behind.
///
/// Both slices are the FULL 22-entry arrays. Returns `None` when either is the wrong length, so a
/// short read at the call site cannot be mistaken for "nothing to restore".
pub fn portrait_equip_restore_report(record: &[i32], fed: &[i32]) -> Option<PortraitEquipRestore> {
    if record.len() != PORTRAIT_EQUIP_ENTRY_COUNT || fed.len() != PORTRAIT_EQUIP_ENTRY_COUNT {
        return None;
    }
    let differs = |index: usize| record[index] != fed[index];
    let weapon_range =
        PORTRAIT_EQUIP_WEAPON_FIRST..PORTRAIT_EQUIP_WEAPON_FIRST + PORTRAIT_EQUIP_WEAPON_COUNT;
    let ammo_range =
        PORTRAIT_EQUIP_AMMO_FIRST..PORTRAIT_EQUIP_AMMO_FIRST + PORTRAIT_EQUIP_AMMO_COUNT;
    let hands = PORTRAIT_EQUIP_PROTECTOR_FIRST + PORTRAIT_EQUIP_SLOT_HANDS;
    let legs = PORTRAIT_EQUIP_PROTECTOR_FIRST + PORTRAIT_EQUIP_SLOT_LEGS;
    Some(PortraitEquipRestore {
        weapon_slots_restored: weapon_range.clone().filter(|index| differs(*index)).count(),
        ammo_slots_restored: ammo_range.filter(|index| differs(*index)).count(),
        protector_slots_restored: (PORTRAIT_EQUIP_PROTECTOR_FIRST
            ..PORTRAIT_EQUIP_PROTECTOR_FIRST + 4)
            .filter(|index| differs(*index))
            .count(),
        right_weapon_id: first_non_empty(record, weapon_range.clone().filter(|i| i % 2 == 1)),
        left_weapon_id: first_non_empty(record, weapon_range.filter(|i| i % 2 == 0)),
        hands_id: record[hands],
        legs_id: record[legs],
    })
}

/// True when the repair has anything to do. A character in bare hands and bare arms legitimately
/// yields `false`; the caller still writes the array (writing identical values costs nothing and
/// keeps one code path), but the semaphores can tell the two situations apart.
pub fn portrait_equip_restore_is_material(report: &PortraitEquipRestore) -> bool {
    report.weapon_slots_restored > 0
        || report.ammo_slots_restored > 0
        || report.protector_slots_restored > 0
}

fn first_non_empty(record: &[i32], indices: impl Iterator<Item = usize>) -> i32 {
    for index in indices {
        if record[index] != PORTRAIT_EQUIP_EMPTY_ID {
            return record[index];
        }
    }
    PORTRAIT_EQUIP_EMPTY_ID
}

pub const PORTRAIT_IDLE_ANIM_IDS: [i32; 3] = [3000000, 100022, 99900];

/// The same list for a character who is TWO-HANDING, with the two-handed standing idle in front.
///
/// 12000000 is grounded exactly the way 3000000 above was -- off our own in-world telemetry, not
/// from a table. Measured 2026-09-07 on Onyx Lord: `current_animation_id` held at 12000000 across
/// nine samples over 22 seconds while the user reported he was standing still and two-handing,
/// where the one-handed standing idle reads 3000000. The rest of the list is unchanged, so a build
/// where 12000000 does not resolve falls through to the one-handed idle rather than to no pose.
pub const PORTRAIT_IDLE_ANIM_IDS_TWO_HANDED: [i32; 4] = [12000000, 3000000, 100022, 99900];

/// `ChrAsmArmStyle` values that mean two-handing. Not a guess and not a name: `CS::ChrIns::
/// IsTwoHanding` (deobf 0x1403f4930) is the whole function `ADD EAX,-0x2 ; CMP EAX,0x1 ; SETBE`,
/// so exactly `{2, 3}` -- `LeftBothHands` and `RightBothHands` -- answer true.
pub const CHR_ASM_ARM_STYLE_TWO_HANDED: [i32; 2] = [2, 3];

/// Which idle to try on the portrait, given the arm style of the `ChrAsm` it is being built from.
///
/// The stance is NOT something the model build derives: `FUN_1409e6fb0` reads the equipment array
/// and `selectedSlots`, and the only caller of `PlayerIns::GetArmStyle` in the whole image is
/// `UpdatePlayerComponents` -- the live player. So a two-handed portrait has to come from the
/// animation, which is why this is a list of anim ids and not a flag.
pub fn portrait_idle_anim_ids(arm_style: i32) -> &'static [i32] {
    if CHR_ASM_ARM_STYLE_TWO_HANDED.contains(&arm_style) {
        &PORTRAIT_IDLE_ANIM_IDS_TWO_HANDED
    } else {
        &PORTRAIT_IDLE_ANIM_IDS
    }
}

#[cfg(test)]
mod idle_anim_tests {
    use super::*;

    #[test]
    fn two_handing_leads_with_the_two_handed_idle() {
        for arm_style in CHR_ASM_ARM_STYLE_TWO_HANDED {
            assert_eq!(portrait_idle_anim_ids(arm_style)[0], 12000000);
        }
    }

    /// One-handed, and the ctor-fresh/unknown values, keep the list that has been shipping.
    #[test]
    fn everything_else_keeps_the_one_handed_idle() {
        for arm_style in [-1, 0, 1, 4, 99] {
            assert_eq!(portrait_idle_anim_ids(arm_style)[0], 3000000);
        }
    }

    /// Both lists END the same way, so a build where the leading id does not resolve falls through
    /// to the same fallbacks rather than to no pose at all.
    #[test]
    fn both_lists_share_their_fallback_tail() {
        let one = PORTRAIT_IDLE_ANIM_IDS;
        let two = PORTRAIT_IDLE_ANIM_IDS_TWO_HANDED;
        assert_eq!(&two[two.len() - one.len()..], &one[..]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bare-body rows the feed forces into hands and legs, from
    /// `CS::ChrAsm::GetDefaultProtectorParamId`: slot -> 10000 + 100 * slot.
    const DEFAULT_HANDS: i32 = 10200;
    const DEFAULT_LEGS: i32 = 10300;

    fn empty_array() -> [i32; PORTRAIT_EQUIP_ENTRY_COUNT] {
        [PORTRAIT_EQUIP_EMPTY_ID; PORTRAIT_EQUIP_ENTRY_COUNT]
    }

    /// A character wearing a full set and holding a weapon in each hand, as the record has them.
    fn dressed_record() -> [i32; PORTRAIT_EQUIP_ENTRY_COUNT] {
        let mut record = empty_array();
        record[0] = 3000000; // left primary
        record[1] = 2000000; // right primary
        record[PORTRAIT_EQUIP_PROTECTOR_FIRST] = 1000000; // head
        record[PORTRAIT_EQUIP_PROTECTOR_FIRST + 1] = 1000100; // chest
        record[PORTRAIT_EQUIP_PROTECTOR_FIRST + 2] = 1000200; // hands
        record[PORTRAIT_EQUIP_PROTECTOR_FIRST + 3] = 1000300; // legs
        record
    }

    /// What `FUN_140bbe1a0` leaves behind when handed [`dressed_record`]: weapons erased, hands and
    /// legs replaced with the bare-body rows, head and chest untouched.
    fn after_native_feed() -> [i32; PORTRAIT_EQUIP_ENTRY_COUNT] {
        let mut fed = dressed_record();
        fed[0] = PORTRAIT_EQUIP_EMPTY_ID;
        fed[1] = PORTRAIT_EQUIP_EMPTY_ID;
        fed[PORTRAIT_EQUIP_PROTECTOR_FIRST + 2] = DEFAULT_HANDS;
        fed[PORTRAIT_EQUIP_PROTECTOR_FIRST + 3] = DEFAULT_LEGS;
        fed
    }

    #[test]
    fn the_feeds_damage_is_reported_slot_for_slot() {
        let report =
            portrait_equip_restore_report(&dressed_record(), &after_native_feed()).unwrap();
        assert_eq!(report.weapon_slots_restored, 2);
        assert_eq!(report.ammo_slots_restored, 0);
        assert_eq!(
            report.protector_slots_restored, 2,
            "head and chest survive the feed"
        );
        assert_eq!(report.right_weapon_id, 2000000);
        assert_eq!(report.left_weapon_id, 3000000);
        assert_eq!(report.hands_id, 1000200);
        assert_eq!(report.legs_id, 1000300);
        assert!(portrait_equip_restore_is_material(&report));
    }

    /// The case that must NOT read as a failure: a character who really is bare-handed and
    /// bare-armed. The feed's clear and its default-protector write both land on values the record
    /// already holds, so there is nothing to restore and the semaphores say so.
    #[test]
    fn a_genuinely_bare_character_needs_no_repair() {
        let mut record = empty_array();
        record[PORTRAIT_EQUIP_PROTECTOR_FIRST + 2] = DEFAULT_HANDS;
        record[PORTRAIT_EQUIP_PROTECTOR_FIRST + 3] = DEFAULT_LEGS;
        let report = portrait_equip_restore_report(&record, &record).unwrap();
        assert_eq!(report.weapon_slots_restored, 0);
        assert_eq!(report.protector_slots_restored, 0);
        assert_eq!(report.right_weapon_id, PORTRAIT_EQUIP_EMPTY_ID);
        assert!(!portrait_equip_restore_is_material(&report));
    }

    /// Handedness reporting reads the record, not the fed array -- the fed array has no weapons
    /// left to read. Odd indices are the right hand, even the left.
    #[test]
    fn the_hands_are_read_off_the_odd_even_split() {
        let mut record = empty_array();
        record[3] = 2200000; // right SECONDARY only
        record[4] = 3300000; // left TERTIARY only
        let report = portrait_equip_restore_report(&record, &empty_array()).unwrap();
        assert_eq!(report.right_weapon_id, 2200000);
        assert_eq!(report.left_weapon_id, 3300000);
    }

    #[test]
    fn a_short_array_is_refused_rather_than_padded() {
        let short = [PORTRAIT_EQUIP_EMPTY_ID; PORTRAIT_EQUIP_ENTRY_COUNT - 1];
        assert!(portrait_equip_restore_report(&short, &empty_array()).is_none());
        assert!(portrait_equip_restore_report(&empty_array(), &short).is_none());
    }

    /// The ammunition base index is the same number the build importer independently measured.
    #[test]
    fn the_ammo_base_agrees_with_the_build_importer() {
        assert_eq!(PORTRAIT_EQUIP_AMMO_FIRST as i32, 6);
        assert_eq!(PORTRAIT_EQUIP_PROTECTOR_FIRST as i32, 12);
    }
}
