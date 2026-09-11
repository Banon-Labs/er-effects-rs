//! Pure, host-testable: did a record's equipment change, and is a portrait wearing it?
//!
//! The unsafe half -- reading the arrays out of game memory and calling the game's own
//! live-character-into-a-record writer -- is [`crate::live_player_sync`]. Everything here is
//! arithmetic over values the caller already read, so the rules a portrait-refresh proof rests on
//! are provable by `cargo test` instead of only by a game launch.

use er_game_base::fnv1a::{FNV1A64_OFFSET_BASIS, fnv1a64_mix};

/// A record's equipment as the model build will read it: the level beside it, and a fingerprint
/// over the `equipment_param_ids` the renderer resolves armour and armaments from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordEquipment {
    /// `record+0x24`, the Rune Level the row prints.
    pub level: i32,
    /// [`equipment_fingerprint`] over the record's `ChrAsm::equipment_param_ids`.
    pub fingerprint: u64,
}

/// What a sync attempt did, and when it did nothing, which thing was missing.
///
/// Every refusal is its own variant rather than a bare `None`, because they are not one condition:
/// a summary that is not allocated yet is a timing question, a slot outside the table is a caller
/// bug, and an unmapped native is a build-support question. A caller that folds them together
/// reports "the portrait did not refresh" and names none of the three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSync {
    /// The native ran. `before` is what the record said on the way in, `after` what it says now.
    Synced {
        slot: i32,
        before: Option<RecordEquipment>,
        after: Option<RecordEquipment>,
    },
    /// `GameDataMan+0x78` read as zero, so there is no record table to write into.
    NoSummary,
    /// The slot is not one of the ten. The native refuses these itself; refusing them first is what
    /// makes the refusal attributable to the caller that asked.
    SlotOutOfRange(i32),
    /// This build has no verified mapping for the native, so nothing was called.
    NativeUnmapped,
}

impl LiveSync {
    /// Did the equipment block actually change?
    ///
    /// A sync whose fingerprints match is not a failure -- re-importing the build already worn
    /// changes nothing, and neither does a sync that follows the game's own save by a frame. It is
    /// still worth telling apart from one that moved the record, because only the second explains
    /// a portrait that is about to look different.
    #[must_use]
    pub fn equipment_changed(&self) -> bool {
        match self {
            Self::Synced {
                before: Some(before),
                after: Some(after),
                ..
            } => before.fingerprint != after.fingerprint,
            _ => false,
        }
    }

    /// The record's equipment fingerprint after the sync, or 0 when there was no readable record.
    #[must_use]
    pub fn fingerprint_after(&self) -> u64 {
        match self {
            Self::Synced {
                after: Some(after), ..
            } => after.fingerprint,
            _ => 0,
        }
    }

    /// The record's level after the sync, or 0 when there was no readable record.
    #[must_use]
    pub fn level_after(&self) -> i32 {
        match self {
            Self::Synced {
                after: Some(after), ..
            } => after.level,
            _ => 0,
        }
    }

    /// Short stable tag for a log line and for the telemetry state field.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::Synced { .. } => "synced",
            Self::NoSummary => "no-summary",
            Self::SlotOutOfRange(_) => "slot-out-of-range",
            Self::NativeUnmapped => "native-unmapped",
        }
    }

    /// The value the telemetry field carries. Distinct per outcome, and 0 reserved for "this never
    /// ran at all" so a counter that was never written cannot read as a successful sync.
    #[must_use]
    pub const fn code(&self) -> usize {
        match self {
            Self::Synced { .. } => 1,
            Self::NoSummary => 2,
            Self::SlotOutOfRange(_) => 3,
            Self::NativeUnmapped => 4,
        }
    }
}

/// The telemetry value meaning no sync has been attempted in this session.
pub const LIVE_SYNC_NOT_ATTEMPTED: usize = 0;

/// Fingerprint a `ChrAsm::equipment_param_ids` array.
///
/// One multiply per entry over the value, not over its bytes: these are the dwords the
/// model-resource request reads, the array is fixed-length, and what a caller needs is "did this
/// change", so a field-wise mix is both cheaper and exactly as discriminating. `-1` is the empty
/// slot and is mixed like any other value, which is what makes taking a piece off as visible as
/// putting one on.
#[must_use]
pub fn equipment_fingerprint(ids: &[i32]) -> u64 {
    let mut hash = FNV1A64_OFFSET_BASIS;
    for id in ids {
        hash = fnv1a64_mix(hash, *id as u32 as u64);
    }
    hash
}

/// Whether a profile renderer is dressing its model in the gear the record now carries.
///
/// The two inputs are fingerprints of the same array read from two places: the record's `ChrAsm`
/// block, and the renderer's live stage-0 `ChrAsm` -- which is the block the per-frame
/// model-resource request actually reads, not the inbox the feed writes. Equal means the model
/// being built is the imported loadout; unequal means it is still the previous one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortraitEquipmentVerdict {
    /// Both sides read, and they agree.
    Matches,
    /// Both sides read, and they disagree.
    Differs,
    /// One side could not be read whole, so there is no comparison. Deliberately not folded into
    /// [`Self::Differs`]: an unreadable renderer is a missing measurement, and reporting it as a
    /// mismatch would invent a defect out of a failed read.
    Unmeasured,
}

impl PortraitEquipmentVerdict {
    /// Short stable tag for a log line.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Matches => "matches",
            Self::Differs => "differs",
            Self::Unmeasured => "unmeasured",
        }
    }

    /// The value the telemetry field carries: 0 unmeasured, 1 matches, 2 differs. Tri-state on
    /// purpose -- "never measured" must not read as a pass.
    #[must_use]
    pub const fn code(self) -> usize {
        match self {
            Self::Unmeasured => 0,
            Self::Matches => 1,
            Self::Differs => 2,
        }
    }
}

/// Compare a record's equipment fingerprint against a renderer stage's. Zero on either side means
/// that side was never read.
#[must_use]
pub const fn portrait_equipment_verdict(record: u64, renderer: u64) -> PortraitEquipmentVerdict {
    if record == 0 || renderer == 0 {
        PortraitEquipmentVerdict::Unmeasured
    } else if record == renderer {
        PortraitEquipmentVerdict::Matches
    } else {
        PortraitEquipmentVerdict::Differs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same length as `CS::ChrAsm::equipment_param_ids`. Spelled here rather than imported because
    /// the typed constant lives behind the game bindings, which are a Windows-only dependency, and
    /// these cases have to run on the host. `live_player_sync` asserts the two agree.
    const ENTRY_COUNT: usize = 22;

    fn ids(values: &[i32]) -> [i32; ENTRY_COUNT] {
        let mut out = [-1i32; ENTRY_COUNT];
        out[..values.len()].copy_from_slice(values);
        out
    }

    /// The whole point of the fingerprint: swapping one piece of armour has to move it. A hash
    /// that missed a single-slot change would report a successful sync over a record that still
    /// describes the previous outfit.
    #[test]
    fn one_changed_slot_moves_the_fingerprint() {
        let worn = ids(&[21000, 21100, 21200, 21300]);
        let mut swapped = worn;
        swapped[1] = 22100;
        assert_ne!(
            equipment_fingerprint(&worn),
            equipment_fingerprint(&swapped),
            "a changed chest piece must change the fingerprint"
        );
    }

    /// Taking a piece off is a change in the other direction, and `-1` is how the array spells it.
    /// Mixing the empty sentinel like any other value is what keeps that visible.
    #[test]
    fn removing_a_piece_moves_the_fingerprint() {
        let worn = ids(&[21000, 21100, 21200, 21300]);
        let mut bare = worn;
        bare[0] = -1;
        assert_ne!(equipment_fingerprint(&worn), equipment_fingerprint(&bare));
    }

    /// Two positions holding each other's armament are a different loadout, so order has to count.
    #[test]
    fn the_fingerprint_is_order_sensitive() {
        assert_ne!(
            equipment_fingerprint(&ids(&[1000, 2000])),
            equipment_fingerprint(&ids(&[2000, 1000]))
        );
    }

    /// The same gear twice is the same number, which is what makes re-importing the build already
    /// worn report `equipment_changed() == false` rather than a spurious change.
    #[test]
    fn the_same_loadout_fingerprints_the_same() {
        let worn = ids(&[60500125, -1, 21000, 21100, 21200, 21300]);
        assert_eq!(equipment_fingerprint(&worn), equipment_fingerprint(&worn));
    }

    /// An empty array still hashes to the basis rather than to zero, because zero is this module's
    /// "never read" value and a legitimately-naked character must not be indistinguishable from a
    /// failed read.
    #[test]
    fn a_bare_character_is_not_an_unread_one() {
        assert_ne!(equipment_fingerprint(&ids(&[])), 0);
    }

    /// A record the sync moved, and one it did not. Only the first explains a portrait that is
    /// about to look different, and a caller that cannot tell them apart has no way to say which
    /// of the two it just did.
    #[test]
    fn equipment_changed_reports_only_a_moved_record() {
        let moved = LiveSync::Synced {
            slot: 3,
            before: Some(RecordEquipment {
                level: 100,
                fingerprint: 0x1111,
            }),
            after: Some(RecordEquipment {
                level: 150,
                fingerprint: 0x2222,
            }),
        };
        assert!(moved.equipment_changed());
        assert_eq!(moved.level_after(), 150);
        assert_eq!(moved.fingerprint_after(), 0x2222);

        let unmoved = LiveSync::Synced {
            slot: 3,
            before: Some(RecordEquipment {
                level: 150,
                fingerprint: 0x2222,
            }),
            after: Some(RecordEquipment {
                level: 150,
                fingerprint: 0x2222,
            }),
        };
        assert!(!unmoved.equipment_changed());
    }

    /// A refusal is not a change, and it is not a level either. Reporting a level of 0 for a
    /// refused sync would be indistinguishable from a record that genuinely reads 0, which is why
    /// the tag and the code are what a caller branches on.
    #[test]
    fn a_refusal_carries_no_measurement() {
        for refusal in [
            LiveSync::NoSummary,
            LiveSync::SlotOutOfRange(11),
            LiveSync::NativeUnmapped,
        ] {
            assert!(!refusal.equipment_changed());
            assert_eq!(refusal.fingerprint_after(), 0);
            assert_eq!(refusal.level_after(), 0);
            assert_ne!(refusal.tag(), "synced");
        }
    }

    /// Every outcome names itself, and none of them collides with "never attempted". Folding them
    /// together would report "the portrait did not refresh" and say nothing about which of four
    /// unrelated conditions caused it.
    #[test]
    fn the_outcomes_are_distinguishable_by_tag_and_by_code() {
        let outcomes = [
            LiveSync::Synced {
                slot: 0,
                before: None,
                after: None,
            },
            LiveSync::NoSummary,
            LiveSync::SlotOutOfRange(11),
            LiveSync::NativeUnmapped,
        ];
        for (at, outcome) in outcomes.iter().enumerate() {
            assert_ne!(
                outcome.code(),
                LIVE_SYNC_NOT_ATTEMPTED,
                "{} must not read as never attempted",
                outcome.tag()
            );
            for other in &outcomes[at + 1..] {
                assert_ne!(outcome.tag(), other.tag());
                assert_ne!(outcome.code(), other.code());
            }
        }
    }

    /// An unreadable side is a missing measurement, never a mismatch. Scoring it as one would
    /// invent a defect out of a failed read -- the same class of lie, in reverse, that the
    /// loading-screen side keeps `PORTRAIT_EQUIP_VALUE_UNSAMPLED` for.
    #[test]
    fn an_unread_side_is_unmeasured_not_a_mismatch() {
        assert_eq!(
            portrait_equipment_verdict(0, 0x1234),
            PortraitEquipmentVerdict::Unmeasured
        );
        assert_eq!(
            portrait_equipment_verdict(0x1234, 0),
            PortraitEquipmentVerdict::Unmeasured
        );
        assert_eq!(PortraitEquipmentVerdict::Unmeasured.code(), 0);
    }

    /// The verdict a combined runtime run is read through: equal fingerprints mean the renderer is
    /// dressing the model in the record's gear, unequal mean it is still on the previous one.
    #[test]
    fn equal_fingerprints_are_a_match_and_unequal_a_difference() {
        assert_eq!(
            portrait_equipment_verdict(0xabcd, 0xabcd),
            PortraitEquipmentVerdict::Matches
        );
        assert_eq!(
            portrait_equipment_verdict(0xabcd, 0xdcba),
            PortraitEquipmentVerdict::Differs
        );
        assert_eq!(PortraitEquipmentVerdict::Matches.code(), 1);
        assert_eq!(PortraitEquipmentVerdict::Differs.code(), 2);
    }

    /// The three codes are distinct, because the telemetry field is a number and a reader has only
    /// these three values to tell the states apart with.
    #[test]
    fn the_verdict_codes_are_distinct() {
        let codes = [
            PortraitEquipmentVerdict::Unmeasured.code(),
            PortraitEquipmentVerdict::Matches.code(),
            PortraitEquipmentVerdict::Differs.code(),
        ];
        for (at, code) in codes.iter().enumerate() {
            assert!(
                !codes[at + 1..].contains(code),
                "two verdicts share code {code}"
            );
        }
    }
}
