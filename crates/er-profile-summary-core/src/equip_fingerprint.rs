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

/// What was observed of the model object across a rebuild window.
///
/// # Why the renderer's own `ChrAsm` cannot answer this
///
/// [`portrait_equipment_verdict`] compares the gear the renderer was told to wear against the gear
/// the record carries. Both can be correct while the picture on screen is the previous one, because
/// setting a renderer's `ChrAsm` is a state write and the portrait is a captured render: the model
/// has to be destroyed, rebuilt from the new rows and rasterized before a pixel moves. Run
/// `br-20260911-002901-a7e0` is that exact case -- record and renderer stage agreed on the imported
/// loadout and the player still saw the old armour -- so an oracle that stops at the input reports
/// success over a screen that did not change.
///
/// These fields are the model object, not its input. `model_before`/`model_after` are the
/// `CSChrAsmModelIns` pointer; `saw_model_absent` is whether it was ever read as null while the
/// window was open, which is what a teardown looks like and what makes the detector survive an
/// allocator that hands the same address straight back; `parts_*` fingerprint the part-node array
/// the model submit actually walks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RebuildObservation {
    /// The model instance pointer before the rebuild was asked for.
    pub model_before: usize,
    /// The model instance pointer at the last sample of the window.
    pub model_after: usize,
    /// The model instance was read as null at least once while the window was open.
    pub saw_model_absent: bool,
    /// Fingerprint of the part-node array before the rebuild was asked for (0 = not read).
    pub parts_before: u64,
    /// Fingerprint of the part-node array at the last sample (0 = not read).
    pub parts_after: u64,
}

/// Whether the model object was genuinely torn down and rebuilt, and whether it came back
/// different.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortraitRebuildVerdict {
    /// No model was readable on either side, so nothing can be said.
    NotMeasured,
    /// The model went away and came back with a different set of parts. This is the only state in
    /// which the rasterized portrait can have changed.
    Rebuilt,
    /// The model went away and came back assembling the same parts -- so it was rebuilt, and it
    /// rebuilt the previous outfit. A resource request that resolved the old rows looks like this.
    RebuiltSameParts,
    /// The model object never went away and never changed. Its input was updated and nothing was
    /// re-rendered: the picture on screen is stale. This is the state run
    /// `br-20260911-002901-a7e0` was actually in while the input oracle read as a pass.
    NeverRebuilt,
}

impl PortraitRebuildVerdict {
    /// Short stable tag for a log line.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::NotMeasured => "not-measured",
            Self::Rebuilt => "rebuilt",
            Self::RebuiltSameParts => "rebuilt-same-parts",
            Self::NeverRebuilt => "never-rebuilt",
        }
    }

    /// Did the model object actually get torn down and rebuilt, whatever it came back wearing?
    #[must_use]
    pub const fn model_was_rebuilt(self) -> bool {
        matches!(self, Self::Rebuilt | Self::RebuiltSameParts)
    }

    /// The value the telemetry field carries: 0 not measured, 1 rebuilt with different parts,
    /// 2 rebuilt with the same parts, 3 never rebuilt.
    #[must_use]
    pub const fn code(self) -> usize {
        match self {
            Self::NotMeasured => 0,
            Self::Rebuilt => 1,
            Self::RebuiltSameParts => 2,
            Self::NeverRebuilt => 3,
        }
    }
}

/// Read a rebuild window's observation.
///
/// A teardown is recognised by the model instance having been absent at some point, or by the
/// pointer changing value. The absence term is the load-bearing one: a heap allocator is entitled
/// to hand the same address back for the replacement, and on that entirely ordinary outcome a
/// pointer comparison alone would report no rebuild.
#[must_use]
pub const fn portrait_rebuild_verdict(observed: RebuildObservation) -> PortraitRebuildVerdict {
    if observed.model_before == 0 && observed.model_after == 0 {
        return PortraitRebuildVerdict::NotMeasured;
    }
    let rebuilt = observed.saw_model_absent
        || (observed.model_before != 0
            && observed.model_after != 0
            && observed.model_before != observed.model_after);
    if !rebuilt {
        return PortraitRebuildVerdict::NeverRebuilt;
    }
    if observed.parts_before != 0
        && observed.parts_after != 0
        && observed.parts_before == observed.parts_after
    {
        return PortraitRebuildVerdict::RebuiltSameParts;
    }
    PortraitRebuildVerdict::Rebuilt
}

/// The headline field a run is read through: did the portrait actually re-render with the imported
/// gear?
///
/// It is a conjunction on purpose. The previous headline was the input comparison alone, and that
/// is precisely what reported a pass over an unchanged screen, so this one cannot reach
/// [`Self::Proven`] without the model object having been rebuilt as well.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortraitRenderVerdict {
    /// The window has not produced a reading yet.
    Unproven,
    /// The renderer was given the record's gear and the model was torn down and rebuilt with a
    /// different set of parts.
    Proven,
    /// The renderer was given the right gear and the model was never rebuilt -- the input took and
    /// the image did not.
    InputOnlyImageStale,
    /// The model was rebuilt and came back wearing the same parts.
    RebuiltUnchanged,
    /// The renderer was not even given the right gear.
    InputWrong,
}

impl PortraitRenderVerdict {
    /// The value the telemetry field carries. 0 is reserved for "no reading", so a counter that was
    /// never written cannot be mistaken for a proof.
    #[must_use]
    pub const fn code(self) -> usize {
        match self {
            Self::Unproven => 0,
            Self::Proven => 1,
            Self::InputOnlyImageStale => 2,
            Self::RebuiltUnchanged => 3,
            Self::InputWrong => 4,
        }
    }

    /// Short stable tag for a log line.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Unproven => "unproven",
            Self::Proven => "proven",
            Self::InputOnlyImageStale => "input-only-image-stale",
            Self::RebuiltUnchanged => "rebuilt-unchanged",
            Self::InputWrong => "input-wrong",
        }
    }
}

/// Fold the input comparison and the model-object observation into the one field a run is judged
/// on.
#[must_use]
pub const fn portrait_render_verdict(
    equipment: PortraitEquipmentVerdict,
    rebuild: PortraitRebuildVerdict,
) -> PortraitRenderVerdict {
    match equipment {
        PortraitEquipmentVerdict::Unmeasured => PortraitRenderVerdict::Unproven,
        PortraitEquipmentVerdict::Differs => PortraitRenderVerdict::InputWrong,
        PortraitEquipmentVerdict::Matches => match rebuild {
            PortraitRebuildVerdict::NotMeasured => PortraitRenderVerdict::Unproven,
            PortraitRebuildVerdict::NeverRebuilt => PortraitRenderVerdict::InputOnlyImageStale,
            PortraitRebuildVerdict::RebuiltSameParts => PortraitRenderVerdict::RebuiltUnchanged,
            PortraitRebuildVerdict::Rebuilt => PortraitRenderVerdict::Proven,
        },
    }
}

/// Fingerprint the model's part-node array. Null slots are mixed like any other value, so a piece
/// of armour appearing or disappearing moves the number.
#[must_use]
pub fn parts_fingerprint(nodes: &[usize]) -> u64 {
    let mut hash = FNV1A64_OFFSET_BASIS;
    for node in nodes {
        hash = fnv1a64_mix(hash, *node as u64);
    }
    hash
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

    /// The regression this whole verdict exists for, as a test.
    ///
    /// Run `br-20260911-002901-a7e0`: the record synced, the renderer's stage-0 `ChrAsm` carried
    /// the imported ids, the kick was accepted, and the player saw the old armour. The model object
    /// never went away, so nothing was re-rasterized. The headline field must say so rather than
    /// report the input match as a pass.
    #[test]
    fn an_updated_input_over_an_untouched_model_is_not_a_pass() {
        let observed = RebuildObservation {
            model_before: 0x2000_0000,
            model_after: 0x2000_0000,
            saw_model_absent: false,
            parts_before: 0xaaaa,
            parts_after: 0xaaaa,
        };
        assert_eq!(
            portrait_rebuild_verdict(observed),
            PortraitRebuildVerdict::NeverRebuilt
        );
        assert_eq!(
            portrait_render_verdict(
                PortraitEquipmentVerdict::Matches,
                portrait_rebuild_verdict(observed)
            ),
            PortraitRenderVerdict::InputOnlyImageStale,
            "the input matching is exactly what the old oracle called success"
        );
    }

    /// The state the fix is trying to reach: the model went away and came back assembling a
    /// different set of parts.
    #[test]
    fn a_torn_down_and_differently_rebuilt_model_is_the_only_proof() {
        let observed = RebuildObservation {
            model_before: 0x2000_0000,
            model_after: 0x3000_0000,
            saw_model_absent: true,
            parts_before: 0xaaaa,
            parts_after: 0xbbbb,
        };
        assert_eq!(
            portrait_rebuild_verdict(observed),
            PortraitRebuildVerdict::Rebuilt
        );
        assert_eq!(
            portrait_render_verdict(
                PortraitEquipmentVerdict::Matches,
                portrait_rebuild_verdict(observed)
            ),
            PortraitRenderVerdict::Proven
        );
    }

    /// A heap allocator is entitled to hand the replacement the address the old model just freed.
    /// Without the absence term, that entirely ordinary outcome would be reported as no rebuild --
    /// a false negative that would send the next reader hunting a defect that is not there.
    #[test]
    fn an_address_reused_by_the_allocator_is_still_a_rebuild() {
        let observed = RebuildObservation {
            model_before: 0x2000_0000,
            model_after: 0x2000_0000,
            saw_model_absent: true,
            parts_before: 0xaaaa,
            parts_after: 0xbbbb,
        };
        assert_eq!(
            portrait_rebuild_verdict(observed),
            PortraitRebuildVerdict::Rebuilt
        );
    }

    /// A rebuild that resolved the previous rows is its own diagnosis, and it is not a pass. It
    /// says the teardown worked and the resource request asked for the old armour, which is a
    /// different defect from the model never being rebuilt at all.
    #[test]
    fn a_rebuild_that_came_back_identical_is_reported_apart() {
        let observed = RebuildObservation {
            model_before: 0x2000_0000,
            model_after: 0x3000_0000,
            saw_model_absent: true,
            parts_before: 0xaaaa,
            parts_after: 0xaaaa,
        };
        assert_eq!(
            portrait_rebuild_verdict(observed),
            PortraitRebuildVerdict::RebuiltSameParts
        );
        assert_eq!(
            portrait_render_verdict(
                PortraitEquipmentVerdict::Matches,
                portrait_rebuild_verdict(observed)
            ),
            PortraitRenderVerdict::RebuiltUnchanged
        );
        assert!(PortraitRebuildVerdict::RebuiltSameParts.model_was_rebuilt());
    }

    /// Nothing read on either side is not a rebuild and not a failure -- it is no measurement, and
    /// the headline has to stay unproven rather than inventing either verdict.
    #[test]
    fn an_unreadable_model_measures_nothing() {
        assert_eq!(
            portrait_rebuild_verdict(RebuildObservation::default()),
            PortraitRebuildVerdict::NotMeasured
        );
        assert_eq!(
            portrait_render_verdict(
                PortraitEquipmentVerdict::Matches,
                PortraitRebuildVerdict::NotMeasured
            ),
            PortraitRenderVerdict::Unproven
        );
        assert_eq!(PortraitRenderVerdict::Unproven.code(), 0);
    }

    /// A wrong input outranks whatever the model did: there is no point reporting a rebuild as a
    /// pass when the gear fed into it was never right.
    #[test]
    fn a_wrong_input_is_reported_as_such_whatever_the_model_did() {
        for rebuild in [
            PortraitRebuildVerdict::Rebuilt,
            PortraitRebuildVerdict::NeverRebuilt,
            PortraitRebuildVerdict::NotMeasured,
        ] {
            assert_eq!(
                portrait_render_verdict(PortraitEquipmentVerdict::Differs, rebuild),
                PortraitRenderVerdict::InputWrong
            );
        }
    }

    /// Only one of the five headline states may read as a proof, and none of them may collide.
    #[test]
    fn exactly_one_render_verdict_is_a_pass_and_the_codes_are_distinct() {
        let all = [
            PortraitRenderVerdict::Unproven,
            PortraitRenderVerdict::Proven,
            PortraitRenderVerdict::InputOnlyImageStale,
            PortraitRenderVerdict::RebuiltUnchanged,
            PortraitRenderVerdict::InputWrong,
        ];
        let passes = all
            .iter()
            .filter(|verdict| **verdict == PortraitRenderVerdict::Proven)
            .count();
        assert_eq!(passes, 1);
        for (at, verdict) in all.iter().enumerate() {
            for other in &all[at + 1..] {
                assert_ne!(verdict.code(), other.code());
                assert_ne!(verdict.tag(), other.tag());
            }
        }
    }

    /// A piece of armour appearing or disappearing changes the part array, so the fingerprint has
    /// to move on a null slot becoming populated -- otherwise a character who put a helmet on would
    /// score as rebuilt-unchanged.
    #[test]
    fn a_populated_slot_moves_the_parts_fingerprint() {
        let bare = [0usize, 0, 0x1000, 0];
        let mut helmeted = bare;
        helmeted[0] = 0x2000;
        assert_ne!(parts_fingerprint(&bare), parts_fingerprint(&helmeted));
        assert_ne!(
            parts_fingerprint(&bare),
            0,
            "an empty model is not an unread one"
        );
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
