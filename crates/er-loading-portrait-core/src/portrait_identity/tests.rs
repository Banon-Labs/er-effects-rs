//! Tests for [`super`] -- the portrait's identity and window-latch decision rules.
//!
//! Split out of `portrait_identity.rs` for the file-size gate
//! (`scripts/check-rust-file-sizes.py`), matching `er-save-picker-core/src/model/tests.rs`.
//! Every test here replays a measured run: the values in the assertions come from a debug log
//! or a save container, not from what the rule was expected to do.

use super::*;

/// The defect the sign gate caused: a garbage word with bit31 set read as "not a real map" and
/// turned the comparison off instead of failing it (2 of 6 logged FAILs). Both of these are
/// real `CSRandXorshift` dwords lifted from one corpus slot's body+0x10..0x20 block.
#[test]
fn a_negative_word_is_rejected_as_implausible_not_treated_as_absent_evidence() {
    assert!(!packed_map_is_plausible(0xd139_52aau32 as i32)); // areaId 0xd1
    assert!(!packed_map_is_plausible(0xa504_8cb5u32 as i32)); // areaId 0xa5
    // a positive piece of garbage must be rejected too -- the old `> 0` gate passed it.
    assert!(!packed_map_is_plausible(0x3e20_457cu32 as i32)); // areaId 0x3e, one past the max
}

/// Honest limit of the area predicate: it is a shape check, not an identity check. One in
/// roughly eleven random dwords lands in the valid area range (measured: 68 of 726 corpus
/// body+0x14 words), so garbage can pass. That is precisely why the map term must also be
/// gated on the record being one we wrote -- the predicate alone cannot carry the comparison.
#[test]
fn a_random_word_can_still_land_inside_the_area_range() {
    assert!(packed_map_is_plausible(0x0e6d_7b38u32 as i32)); // areaId 0x0e -- pure luck
}

#[test]
fn real_corpus_area_ids_are_plausible() {
    // Every distinct areaId observed across 726 active corpus slots.
    for area in [
        0x0au8, 0x0b, 0x0c, 0x0d, 0x0e, 0x10, 0x14, 0x15, 0x1c, 0x20, 0x22, 0x23, 0x3c, 0x3d,
    ] {
        // Low word 0x0002_0000 keeps area 0x0a off the new-game sentinel (0x0a01_0000).
        let map = ((u32::from(area) << 24) | 0x0002_0000) as i32;
        assert!(
            packed_map_is_plausible(map),
            "areaId 0x{area:02x} (map 0x{map:08x}) must be plausible"
        );
    }
    // The real corpus values themselves, verbatim.
    for map in [0x0c01_0000u32, 0x1c00_0000, 0x0e00_0000, 0x3d00_0000] {
        assert!(packed_map_is_plausible(map as i32), "map 0x{map:08x}");
    }
}

#[test]
fn the_new_game_sentinel_is_never_plausible() {
    assert!(!packed_map_is_plausible(DEFAULT_MAP_C30));
    assert_eq!(packed_map_area_id(DEFAULT_MAP_C30), 0x0a);
}

#[test]
fn implausible_values_yield_no_map_evidence_rather_than_a_mismatch() {
    let real_a = 0x0c01_0000u32 as i32;
    let real_b = 0x1c00_0000u32 as i32;
    let garbage = 0x3e20_457cu32 as i32;
    assert!(packed_maps_disagree(real_a, real_b));
    assert!(!packed_maps_disagree(real_a, garbage));
    assert!(!packed_maps_disagree(garbage, real_a));
    assert!(!packed_maps_disagree(real_a, real_a));
    assert!(!packed_maps_disagree(real_a, DEFAULT_MAP_C30));
}

/// The measured failure: published slot 9 while slot 5 loaded. The wire tag is slot+1.
#[test]
fn the_2026_08_02_wrong_face_window_is_a_slot_mismatch() {
    assert_eq!(
        published_identity_verdict(9 + 1, Some(5)),
        PublishedIdentity::SlotMismatch {
            published: 9,
            loaded: 5
        }
    );
    assert!(published_identity_verdict(9 + 1, Some(5)).is_mismatch());
}

/// Slot 0 must be distinguishable from "never published", or every boot either false-fires or
/// hides a real mismatch.
#[test]
fn slot_zero_is_not_confused_with_nothing_published() {
    assert_eq!(
        published_identity_verdict(0, Some(0)),
        PublishedIdentity::NothingPublished
    );
    assert_eq!(
        published_identity_verdict(1, Some(0)),
        PublishedIdentity::Match
    );
    assert_eq!(
        published_identity_verdict(1, Some(3)),
        PublishedIdentity::SlotMismatch {
            published: 0,
            loaded: 3
        }
    );
    assert!(!published_identity_verdict(0, Some(0)).is_mismatch());
}

#[test]
fn with_no_completed_load_there_is_nothing_to_compare() {
    assert_eq!(
        published_identity_verdict(5, None),
        PublishedIdentity::NoLoadedSlot
    );
    assert!(!published_identity_verdict(5, None).is_mismatch());
}

/// The precedence change (bd er-effects-rs-91zb): the user's on-screen pick outranks the
/// request register, which outranks the stale `save_slot`.
#[test]
fn the_users_pick_outranks_every_inferred_slot() {
    assert_eq!(
        portrait_target_slot_from_sources(Some(5), Some(7), Some(9), 10),
        Some(5)
    );
}

/// The measured boot window: picker had not recorded a pick at that instant, b78 held the
/// correct 5, ac0 had been dragged to 9 by the game's own selector.
#[test]
fn the_request_register_beats_a_stale_save_slot() {
    assert_eq!(
        portrait_target_slot_from_sources(None, Some(5), Some(9), 10),
        Some(5)
    );
}

#[test]
fn the_no_request_sentinel_falls_through_to_save_slot() {
    assert_eq!(
        portrait_target_slot_from_sources(None, Some(-1), Some(9), 10),
        Some(9)
    );
    assert_eq!(
        portrait_target_slot_from_sources(None, None, Some(9), 10),
        Some(9)
    );
    assert_eq!(
        portrait_target_slot_from_sources(None, Some(10), Some(9), 10),
        Some(9),
        "an out-of-range request must not win"
    );
}

/// With nothing valid the answer is None, never slot 0. Collapsing to 0 is what built and
/// published a slot-0 portrait for a non-slot-0 character.
#[test]
fn no_valid_source_yields_none_not_slot_zero() {
    assert_eq!(
        portrait_target_slot_from_sources(None, None, None, 10),
        None
    );
    assert_eq!(
        portrait_target_slot_from_sources(Some(-1), Some(-1), Some(-1), 10),
        None
    );
}

/// The 2026-08-02 21:05 regression, replayed with that run's literal values. The user picked
/// slot 0; the pipeline latched and published slot 0; then the picker term expired and the
/// sources started naming slot 9 (`save_slot` = ac0 = 9, `request_slot` = b78 = -1). Before
/// the latch, kick #2 retargeted the live window to 9 and the face changed on screen.
#[test]
fn a_window_that_committed_to_the_picked_slot_never_retargets_mid_load() {
    // Kick #1: picker still names slot 0.
    let resolved = portrait_target_slot_from_sources(Some(0), Some(-1), Some(0), 10);
    assert_eq!(resolved, Some(0));
    let (target, latching) = portrait_window_target_slot(None, resolved);
    assert_eq!(target, Some(0));
    assert!(latching, "the first resolution of a window must latch");

    // Kick #2, same window: picker spent, b78 disarmed, ac0 now 9. The sources flip...
    let resolved_later = portrait_target_slot_from_sources(None, Some(-1), Some(9), 10);
    assert_eq!(
        resolved_later,
        Some(9),
        "precedence really does name slot 9 once the picker term is spent -- this is the input that caused the bug"
    );
    // ...but the window does not.
    let (target_later, latching_later) = portrait_window_target_slot(target, resolved_later);
    assert_eq!(
        target_later,
        Some(0),
        "the window must keep the character the user clicked"
    );
    assert!(!latching_later);
}

/// The latch must not become a permanent pin: window close clears it, and the next load is
/// free to target a different character. Without this a System->Quit->Load switch would show
/// the boot character's face forever -- the same defect in the opposite direction.
#[test]
fn a_new_window_is_free_to_target_a_different_character() {
    let (first, _) = portrait_window_target_slot(None, Some(0));
    assert_eq!(first, Some(0));
    // Window closes -> caller resets the latch to None.
    let (second, latching) = portrait_window_target_slot(None, Some(9));
    assert_eq!(second, Some(9));
    assert!(latching);
}

/// The 2026-08-26 regression, replayed with that run's literal values: a boot window that
/// latched a guess must yield to the user's pick.
///
/// At +1061ms every source was invalid (`picker=None b78=None ac0=-1`), so the latch came from
/// the autoload's default hint -- slot 0, i.e. from nothing. At +1084597ms the user picked slot
/// 1 and the latch refused it, so the loading screen showed slot 0's character (`angrE RL 100`)
/// while slot 1 (level 90) was the one loading.
#[test]
fn a_guessed_window_latch_yields_to_the_users_pick() {
    // +1061ms: nothing names a slot. Precedence returns None; the caller's autoload-hint
    // fallback supplies slot 0.
    assert_eq!(
        portrait_target_slot_from_sources(None, None, Some(-1), 10),
        None,
        "picker=None b78=None ac0=-1 names no slot -- slot 0 here is a guess, not a choice"
    );
    let boot = portrait_window_target_slot_authoritative(None, false, Some(0), false);
    assert_eq!(boot.slot, Some(0));
    assert!(boot.latching);
    assert!(!boot.promoted_by_pick);

    // +1084597ms: the user picks slot 1. Precedence now names it, from the picker term.
    let resolved = portrait_target_slot_from_sources(Some(1), Some(-1), Some(-1), 10);
    assert_eq!(resolved, Some(1));

    let picked = portrait_window_target_slot_authoritative(boot.slot, false, resolved, true);
    assert_eq!(
        picked.slot,
        Some(1),
        "the pick must replace a latch that was adopted from a guess"
    );
    assert!(picked.latching);
    assert!(picked.promoted_by_pick);

    // And the promotion is spent: the window is now committed from the pick and holds.
    let after = portrait_window_target_slot_authoritative(picked.slot, true, Some(9), false);
    assert_eq!(after.slot, Some(1));
    assert!(!after.latching);
    assert!(!after.promoted_by_pick);
}

/// The exception must not reopen the 2026-08-02 defect: a latch that came from the pick never
/// yields, not even to another pick-shaped resolution.
#[test]
fn a_picked_window_latch_never_yields() {
    let picked = portrait_window_target_slot_authoritative(None, false, Some(0), true);
    assert_eq!(picked.slot, Some(0));
    assert!(picked.latching);

    for (resolved, from_pick) in [(Some(9), false), (Some(9), true), (None, false)] {
        let held =
            portrait_window_target_slot_authoritative(picked.slot, true, resolved, from_pick);
        assert_eq!(
            held.slot,
            Some(0),
            "a window committed from the user's pick keeps it (resolved={resolved:?} from_pick={from_pick})"
        );
        assert!(!held.latching);
        assert!(!held.promoted_by_pick);
    }
}

/// A pick that agrees with the guessed latch is not a promotion -- nothing changed, so nothing
/// should be counted or re-latched.
#[test]
fn a_pick_matching_the_guess_is_not_a_promotion() {
    let boot = portrait_window_target_slot_authoritative(None, false, Some(4), false);
    assert_eq!(boot.slot, Some(4));
    let same = portrait_window_target_slot_authoritative(boot.slot, false, Some(4), true);
    assert_eq!(same.slot, Some(4));
    assert!(!same.latching);
    assert!(!same.promoted_by_pick);
}

/// A guessed latch still refuses every non-pick source, which is the whole reason the latch
/// exists: only the user outranks it.
#[test]
fn a_guessed_window_latch_still_refuses_game_inferred_retargets() {
    let boot = portrait_window_target_slot_authoritative(None, false, Some(0), false);
    for resolved in [Some(9), Some(1), None] {
        let held = portrait_window_target_slot_authoritative(boot.slot, false, resolved, false);
        assert_eq!(
            held.slot,
            Some(0),
            "ac0/b78 must never move a committed window (resolved={resolved:?})"
        );
        assert!(!held.latching);
    }
}

/// The br-20260831-014208-b1d6 regression, replayed with that run's literal values: with every
/// measured source invalid the window must commit to nothing, because the only thing left to
/// commit to is a config hint.
///
/// ```text
/// [+1184ms]   window LATCHED portrait target slot 0 (picker=None b78=None ac0=-1 from_pick=false)
/// [+19552ms]  SUPPRESSED a mid-window retarget 0 -> 2 (picker=None b78=Some(-1) ac0=2)
/// ```
///
/// Slot 2 (`Bean Smith`) was the character loading, and correctly so -- the run had no
/// configured slot, so the native builder used the container's persisted last-used slot. Slot
/// 0 (`Bonky Bean`) reached the screen because the old caller fell through to the autoload
/// hint and the latch then refused the real `ac0` 3371 times.
#[test]
fn a_window_with_no_measured_source_commits_to_nothing() {
    // +1184ms: picker=None, b78=None, ac0=-1. No measurement names a slot.
    let at_open = portrait_target_slot_attributed(None, None, Some(-1), 10);
    assert_eq!(at_open, None, "picker=None b78=None ac0=-1 names no slot");

    let opened = portrait_window_target_slot_by_evidence(None, None, at_open);
    assert_eq!(
        opened.slot, None,
        "an uncommitted window must stay uncommitted -- rendering no portrait beats \
         committing the window to a config hint"
    );
    assert!(!opened.latching);
    assert_eq!(opened.source, None);

    // +19552ms: ac0 finally reads the slot that is actually loading.
    let arrived = portrait_target_slot_attributed(None, Some(-1), Some(2), 10);
    assert_eq!(arrived, Some((2, PortraitSlotSource::SaveSlot)));
    let committed = portrait_window_target_slot_by_evidence(opened.slot, opened.source, arrived);
    assert_eq!(
        committed.slot,
        Some(2),
        "the first real measurement is what the window commits to"
    );
    assert!(committed.latching);
    assert_eq!(committed.source, Some(PortraitSlotSource::SaveSlot));
}

/// bd `er-effects-rs-fmy6`, replayed: a latch taken off a stale `ac0` yields to the load
/// request register, which by definition describes the load in flight.
///
/// Across save files the redirect swap leaves the slot registers momentarily stale, so at
/// +56125ms `ac0=0` was a real read of an obsolete value and the window latched slot 0. At
/// +60296ms `b78`/`ac0` both named the real slot 1 and the retarget was refused, because the
/// old rule could only ask "was it the pick", and it was not.
#[test]
fn a_latch_taken_off_a_stale_save_slot_yields_to_the_load_request() {
    let latched = portrait_window_target_slot_by_evidence(
        None,
        None,
        portrait_target_slot_attributed(None, Some(-1), Some(0), 10),
    );
    assert_eq!(latched.slot, Some(0));
    assert_eq!(latched.source, Some(PortraitSlotSource::SaveSlot));

    let fresh = portrait_target_slot_attributed(None, Some(1), Some(1), 10);
    assert_eq!(fresh, Some((1, PortraitSlotSource::RequestSlot)));
    let followed = portrait_window_target_slot_by_evidence(latched.slot, latched.source, fresh);
    assert_eq!(
        followed.slot,
        Some(1),
        "the load-REQUEST register outranks a save_slot read that is stale by definition"
    );
    assert!(followed.latching);
    assert!(
        !followed.promoted_by_pick,
        "this promotion is on evidence, not on a user pick -- it must not be counted as one"
    );

    // And it is spent: ac0 flapping back afterwards cannot drag the window with it.
    let after = portrait_window_target_slot_by_evidence(
        followed.slot,
        followed.source,
        Some((0, PortraitSlotSource::SaveSlot)),
    );
    assert_eq!(after.slot, Some(1));
    assert!(!after.latching);
}

/// The rank rule must not reopen the 2026-08-02 defect. Equal or weaker evidence never moves a
/// window, at every rank.
#[test]
fn equal_or_weaker_evidence_never_moves_a_committed_window() {
    use PortraitSlotSource::{RequestSlot, SaveSlot, UserPick};
    for held_source in [SaveSlot, RequestSlot, UserPick] {
        for fresh_source in [SaveSlot, RequestSlot, UserPick] {
            if fresh_source > held_source {
                continue;
            }
            let held = portrait_window_target_slot_by_evidence(
                Some(3),
                Some(held_source),
                Some((9, fresh_source)),
            );
            assert_eq!(
                held.slot,
                Some(3),
                "{fresh_source:?} must not move a window committed from {held_source:?}"
            );
            assert!(!held.latching);
            assert_eq!(held.source, Some(held_source));
        }
    }
}

/// A resolution that agrees with the held slot but carries stronger evidence upgrades the
/// latch's provenance without re-latching -- otherwise the user's confirmation of the slot
/// already on screen would be forgotten and a later `b78` could still move it.
#[test]
fn agreeing_evidence_upgrades_the_latch_without_changing_the_face() {
    let confirmed = portrait_window_target_slot_by_evidence(
        Some(5),
        Some(PortraitSlotSource::SaveSlot),
        Some((5, PortraitSlotSource::UserPick)),
    );
    assert_eq!(confirmed.slot, Some(5));
    assert!(
        !confirmed.latching,
        "the face did not change; do not re-latch"
    );
    assert!(!confirmed.promoted_by_pick);
    assert_eq!(confirmed.source, Some(PortraitSlotSource::UserPick));

    let held = portrait_window_target_slot_by_evidence(
        confirmed.slot,
        confirmed.source,
        Some((7, PortraitSlotSource::RequestSlot)),
    );
    assert_eq!(
        held.slot,
        Some(5),
        "the upgraded latch is now pick-authority and nothing outranks it"
    );
}

/// A momentarily-invalid register must not demote an established latch: `None` resolutions
/// leave both the slot and its provenance alone.
#[test]
fn an_invalid_read_does_not_demote_an_established_latch() {
    let held =
        portrait_window_target_slot_by_evidence(Some(2), Some(PortraitSlotSource::UserPick), None);
    assert_eq!(held.slot, Some(2));
    assert_eq!(held.source, Some(PortraitSlotSource::UserPick));
    assert!(!held.latching);
}

/// The wire form the `PORTRAIT_WINDOW_TARGET_SOURCE` atomic uses must round-trip, and `0` must
/// mean "no latch" rather than the weakest source.
#[test]
fn the_source_rank_wire_form_round_trips_and_zero_is_no_latch() {
    use PortraitSlotSource::{RequestSlot, SaveSlot, UserPick};
    assert_eq!(PortraitSlotSource::from_rank(0), None);
    for source in [SaveSlot, RequestSlot, UserPick] {
        assert_eq!(PortraitSlotSource::from_rank(source.rank()), Some(source));
    }
    assert!(SaveSlot.rank() < RequestSlot.rank() && RequestSlot.rank() < UserPick.rank());
    assert_eq!(PortraitSlotSource::from_rank(usize::MAX), None);
}

/// Attribution must agree with the precedence the un-attributed form has always had, or the
/// two would name different characters for the same reads.
#[test]
fn attribution_agrees_with_the_precedence_it_replaced() {
    for picker in [None, Some(-1), Some(4), Some(10)] {
        for request in [None, Some(-1), Some(5)] {
            for save in [None, Some(-1), Some(6)] {
                assert_eq!(
                    portrait_target_slot_attributed(picker, request, save, 10)
                        .map(|(slot, _)| slot),
                    portrait_target_slot_from_sources(picker, request, save, 10),
                    "picker={picker:?} request={request:?} save={save:?}"
                );
            }
        }
    }
    assert_eq!(
        portrait_target_slot_attributed(Some(4), Some(5), Some(6), 10),
        Some((4, PortraitSlotSource::UserPick))
    );
    assert_eq!(
        portrait_target_slot_attributed(Some(10), Some(5), Some(6), 10),
        Some((5, PortraitSlotSource::RequestSlot)),
        "an out-of-range picker falls through, it does not claim the answer"
    );
}

/// The legacy two-argument form keeps its exact old behaviour, so callers that do not track
/// latch authority are unchanged.
#[test]
fn the_legacy_pair_form_is_unchanged() {
    assert_eq!(portrait_window_target_slot(None, Some(3)), (Some(3), true));
    assert_eq!(
        portrait_window_target_slot(Some(0), Some(1)),
        (Some(0), false)
    );
    assert_eq!(portrait_window_target_slot(None, None), (None, false));
}

/// The parent-requested GATE: a picked slot N != 0 must produce slot N's stats.
///
/// Composed end to end from the 2026-08-26 run's literal values so it fails on the real bug:
/// boot window latches the autoload's guessed slot 0, the user picks slot 1, and the stats
/// panel must follow the pick. Before the latch-authority fix the last assertion read `0`, and
/// the loading screen showed `angrE RL 100` (slot 0) while slot 1 (level 90) was loading.
#[test]
fn a_picked_slot_other_than_zero_drives_the_stats_panel() {
    const SLOTS: i32 = 10;
    // `SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT` is unset on a boot autoload -- the run reported
    // 18446744073709551615 -- so the switch term must not claim the answer.
    const SWITCH_UNSET: usize = usize::MAX;
    assert_eq!(
        loading_screen_stats_slot_source(SWITCH_UNSET, None, None, SLOTS),
        StatsSlotSource::BestActiveFallback
    );

    // +1061ms: boot window latches slot 0 from the autoload hint (no source named a slot).
    let boot = portrait_window_target_slot_authoritative(None, false, Some(0), false);
    assert_eq!(
        loading_screen_stats_slot_source(SWITCH_UNSET, boot.slot, None, SLOTS),
        StatsSlotSource::PortraitWindow(0)
    );

    // +1084597ms: the user picks slot 1.
    let resolved = portrait_target_slot_from_sources(Some(1), Some(-1), Some(-1), SLOTS);
    let picked = portrait_window_target_slot_authoritative(boot.slot, false, resolved, true);
    assert!(picked.promoted_by_pick);
    assert_eq!(
        loading_screen_stats_slot_source(SWITCH_UNSET, picked.slot, None, SLOTS),
        StatsSlotSource::PortraitWindow(1),
        "the stats panel must describe the character the user picked, not the boot guess"
    );

    // Every non-zero slot behaves the same -- slot 1 is not a special case.
    for slot in 1..SLOTS {
        let boot = portrait_window_target_slot_authoritative(None, false, Some(0), false);
        let picked = portrait_window_target_slot_authoritative(boot.slot, false, Some(slot), true);
        assert_eq!(
            loading_screen_stats_slot_source(SWITCH_UNSET, picked.slot, None, SLOTS),
            StatsSlotSource::PortraitWindow(slot)
        );
    }
}

/// A System->Quit switch selection still outranks the window target, and its unset/out-of-range
/// wire forms must not be mistaken for slot 0 -- `usize::MAX as i32` is -1, not a slot.
#[test]
fn the_switch_selection_outranks_the_window_and_its_sentinels_do_not() {
    const SLOTS: i32 = 10;
    assert_eq!(
        loading_screen_stats_slot_source(3, Some(7), None, SLOTS),
        StatsSlotSource::SwitchSelection(3)
    );
    for sentinel in [
        usize::MAX,
        usize::MAX - 1,
        SLOTS as usize,
        i32::MAX as usize,
    ] {
        assert_eq!(
            loading_screen_stats_slot_source(sentinel, Some(7), None, SLOTS),
            StatsSlotSource::PortraitWindow(7),
            "sentinel {sentinel} must fall through, never resolve to a slot"
        );
    }
    // An out-of-range window target is not a slot either.
    assert_eq!(
        loading_screen_stats_slot_source(usize::MAX, Some(SLOTS), None, SLOTS),
        StatsSlotSource::BestActiveFallback
    );
    assert_eq!(
        loading_screen_stats_slot_source(usize::MAX, Some(-1), None, SLOTS),
        StatsSlotSource::BestActiveFallback
    );
}

/// A window that has not resolved any slot yet must stay uncommitted rather than latch a
/// placeholder -- latching `0` here would reintroduce the `unwrap_or(0)` lie the rest of this
/// module exists to remove.
#[test]
fn an_unresolved_window_latches_nothing() {
    assert_eq!(portrait_window_target_slot(None, None), (None, false));
    // ...and a later real resolution still latches normally.
    assert_eq!(portrait_window_target_slot(None, Some(3)), (Some(3), true));
}

/// Literal values from run br-20260822-040913-f0f4, window #3.
const RUN_HELD_NAME_HASH: usize = 0x909a_2595_c413_a1b3;
const RUN_RECORD_FACE_HASH: usize = 0xbbd2_ad40_6f84_9c65;
const RUN_PREVIEW_FACE_HASH: usize = 0xc6af_b8c3_7ec7_b617;

/// The defect, stated as a test: on a same-slot repeat the hold predicate agrees with itself.
///
/// Both hashes are slot 0's ProfileSummary record read at two different times, so re-selecting
/// the same slot makes them equal by construction -- even in the measured run where that record
/// said `Maddened Bean` and slot 0's own deserialize produced `Ordinary Bean`. This test is not
/// asserting desirable behaviour; it pins the blind spot that makes the hold provisional.
#[test]
fn the_same_slot_repeat_hold_matches_even_when_the_record_is_wrong() {
    assert!(
        same_identity_bridge_hold(true, 1, RUN_HELD_NAME_HASH, 1, RUN_HELD_NAME_HASH),
        "a same-slot reselect always matches: this predicate compares one record with itself"
    );
}

/// The cases it still decides correctly, which is why it survives as the first filter.
#[test]
fn a_changed_name_or_slot_or_missing_head_still_clears() {
    // Different character in the same slot -- the record's name did change.
    assert!(!same_identity_bridge_hold(true, 1, 0xaaaa, 1, 0xbbbb));
    // Different slot entirely.
    assert!(!same_identity_bridge_hold(true, 2, 0xaaaa, 1, 0xaaaa));
    // Nothing published to hold on to.
    assert!(!same_identity_bridge_hold(false, 1, 0xaaaa, 1, 0xaaaa));
    // Unknown name on either side is never agreement.
    assert!(!same_identity_bridge_hold(true, 1, 0, 1, 0));
    // No incoming slot.
    assert!(!same_identity_bridge_hold(true, 0, 0xaaaa, 0, 0xaaaa));
}

/// The revocation, replayed with the run's literal hashes: the hold was taken on slot 0 at
/// +107006ms, and the first build kick for slot 0 at +108385ms found a record whose face was
/// not the one the preview fingerprinted. That is the falsification the hold could not produce
/// for itself.
#[test]
fn the_2026_08_22_face_mismatch_revokes_the_held_head() {
    let verdict = bridge_hold_face_verdict(1, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH);
    assert_eq!(verdict, BridgeHoldVerdict::Revoke);
    assert!(verdict.revokes());
}

/// Make-before-break must survive. Windows 1/2/4 of the same run held and then published
/// 259-281 frames each; an intact record hands back `Unrefuted`, which drops nothing. A fix
/// that revoked here would turn every legitimate same-character reload back into a flash of
/// empty loading screen.
#[test]
fn an_intact_record_does_not_revoke_a_legitimate_hold() {
    let verdict = bridge_hold_face_verdict(1, 0, RUN_PREVIEW_FACE_HASH, RUN_PREVIEW_FACE_HASH);
    assert_eq!(verdict, BridgeHoldVerdict::Unrefuted);
    assert!(!verdict.revokes());
}

/// The three arms that must never revoke: no hold outstanding, a kick for another slot, and a
/// slot with no fingerprint to compare against. Absence of evidence is not refutation -- but it
/// is not agreement either, which is why `NoEvidence` is its own answer rather than `Unrefuted`.
#[test]
fn absent_or_unrelated_evidence_never_revokes() {
    assert_eq!(
        bridge_hold_face_verdict(0, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
        BridgeHoldVerdict::NoHold
    );
    assert_eq!(
        bridge_hold_face_verdict(1, 4, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
        BridgeHoldVerdict::OtherSlot
    );
    assert_eq!(
        bridge_hold_face_verdict(1, -1, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
        BridgeHoldVerdict::OtherSlot
    );
    assert_eq!(
        bridge_hold_face_verdict(1, 0, RUN_RECORD_FACE_HASH, 0),
        BridgeHoldVerdict::NoEvidence
    );
    for verdict in [
        BridgeHoldVerdict::NoHold,
        BridgeHoldVerdict::OtherSlot,
        BridgeHoldVerdict::NoEvidence,
    ] {
        assert!(!verdict.revokes());
    }
}

/// Slot 0 must be distinguishable from "no hold", the same +1 biasing trap the published-slot
/// tag has: without it a hold on slot 0 would be unrevokable.
#[test]
fn a_hold_on_slot_zero_is_not_read_as_no_hold() {
    // Tag 1 is slot 0. The whole measured failure sat on slot 0.
    assert_eq!(
        bridge_hold_face_verdict(1, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
        BridgeHoldVerdict::Revoke
    );
    assert_eq!(
        bridge_hold_face_verdict(0, 0, RUN_RECORD_FACE_HASH, RUN_PREVIEW_FACE_HASH),
        BridgeHoldVerdict::NoHold,
        "tag 0 is 'no hold', never slot 0"
    );
}

// === The record must be a character before the loading-screen stats panel reads it ==============
//
// Every value below is one the user actually saw on a loading screen (run 2026-08-29) or one the
// picker model demonstrably produces, not an invented example.

/// The reported defect. Both labels were on the user's loading screens beside `RL 0`, because the
/// save picker had written its browse rows into the live ProfileSummary and the restore never ran.
#[test]
fn the_two_picker_labels_the_user_saw_are_not_characters() {
    // `[..] {parent}` -- the ParentDir row, whose parent of `...\76561197986456766` is `EldenRing`.
    assert_eq!(
        profile_record_character_verdict("[..] EldenRing", 0, 0),
        RecordCharacterVerdict::NoLevel
    );
    // `PICKER_NEW_FILE_LABEL`, the destination picker's `[ new ]` row.
    assert_eq!(
        profile_record_character_verdict("[ new ]", 0, 0),
        RecordCharacterVerdict::NoLevel
    );
    // ...and the level term is not the only one holding them out: give the record a level and the
    // label shape still refuses it.
    assert_eq!(
        profile_record_character_verdict("[..] EldenRing", 139, 0x0c01_0000),
        RecordCharacterVerdict::PickerRowLabel
    );
    assert_eq!(
        profile_record_character_verdict("[ new ]", 139, 0x0c01_0000),
        RecordCharacterVerdict::PickerRowLabel
    );
}

/// Every other label `SavePickerModel::row_label_utf16` can produce, at a level it must never keep.
#[test]
fn the_remaining_picker_row_labels_are_refused_by_shape() {
    for label in [
        "[ .. up ]",
        "[ root ]",
        "[ SCROLL ^ ]",
        "[ SCROLL v ]",
        "[C:]",
        "saves/",
        "Z:\\",
    ] {
        assert_eq!(
            profile_record_character_verdict(label, 100, 0x0c01_0000),
            RecordCharacterVerdict::PickerRowLabel,
            "picker label {label:?} must not read as a character"
        );
    }
}

/// A ZEROED record -- what `save_picker_write_row_records` leaves behind for a slot beyond the
/// listing, and what an unpopulated slot looks like before the boot ProfileSummary read.
#[test]
fn a_zeroed_record_is_not_a_character() {
    assert_eq!(
        profile_record_character_verdict("", 0, 0),
        RecordCharacterVerdict::EmptyName
    );
    assert_eq!(
        profile_record_character_verdict("   ", 0, 0),
        RecordCharacterVerdict::EmptyName
    );
}

/// Real characters must still render. These are the values the panel had on screen correctly in
/// the same run (the `ok-full` windows), plus the boot state of a character who has never left the
/// tutorial: `DEFAULT_MAP_C30` is a legitimate map, not a refutation.
#[test]
fn a_real_character_passes_including_a_brand_new_one() {
    assert!(profile_record_character_verdict("Maddened Bean", 139, 0x0c01_0000).is_character());
    // Brand-new character: the new-game sentinel map must not blank the panel.
    assert!(profile_record_character_verdict("Tarnished", 1, DEFAULT_MAP_C30).is_character());
    // Map word not yet populated: absence of map evidence is not evidence of garbage.
    assert!(profile_record_character_verdict("Tarnished", 1, 0).is_character());
    // A name that merely contains a bracket or a slash is fine -- only the ends carry the shape.
    assert!(profile_record_character_verdict("Ash[of]War", 60, 0x0c01_0000).is_character());
}

/// A populated-but-nonsense map word means the record is neither a character nor a zeroed slot.
/// The two garbage dwords are the same corpus `CSRandXorshift` words the map-plausibility tests
/// above use, so this shares their provenance.
#[test]
fn a_populated_but_implausible_map_refutes_the_record() {
    assert_eq!(
        profile_record_character_verdict("Tarnished", 60, 0xd139_52aau32 as i32),
        RecordCharacterVerdict::ImplausibleMap
    );
    assert_eq!(
        profile_record_character_verdict("Tarnished", 60, 0x3e20_457cu32 as i32),
        RecordCharacterVerdict::ImplausibleMap
    );
}

/// A file row's label is a bare filename, which is shape-indistinguishable from a character name.
/// Pinned so nobody "improves" the shape check into one that blanks real characters: the level
/// term is what rejects file rows, and it does.
#[test]
fn a_file_row_label_is_rejected_by_level_not_by_shape() {
    assert!(!name_looks_like_picker_row_label("ER0000.sl2"));
    assert_eq!(
        profile_record_character_verdict("ER0000.sl2", 0, 0),
        RecordCharacterVerdict::NoLevel
    );
}

// === The live-record scan: which occupied slots are not characters =============================

fn character(name: &'static str, level: i32) -> LiveRecordSample<'static> {
    LiveRecordSample {
        occupied: true,
        name,
        level,
        map: 0x0c01_0000,
    }
}

fn empty_slot() -> LiveRecordSample<'static> {
    LiveRecordSample {
        occupied: false,
        name: "",
        level: 0,
        map: 0,
    }
}

fn staged_row(name: &'static str) -> LiveRecordSample<'static> {
    LiveRecordSample {
        occupied: true,
        name,
        level: 0,
        map: 0,
    }
}

/// The control. A healthy three-character save: three occupied character records and seven zeroed,
/// unoccupied slots. This is the shape of every ordinary boot, and it must read zero -- an oracle
/// that is non-zero in the normal case teaches the reader to ignore it.
#[test]
fn a_healthy_save_container_reports_no_orphaned_records() {
    let table = [
        character("Maddened Bean", 139),
        character("angrE", 90),
        character("Tarnished", 1),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
    ];
    let scan = scan_live_records(table);
    assert_eq!(scan.orphaned_mask, 0);
    assert_eq!(
        scan.characters, 3,
        "the table is readable and holds three characters"
    );
}

/// The reported defect, exactly as the PICKER leaves it. `save_picker_write_row_records` zeroes all
/// ten records, writes a label into the first `visible` of them and marks those occupied, and marks
/// every slot beyond the listing unoccupied. So during the defect not one slot holds a character.
///
/// This is the case that killed the obvious gate. A rule of "answer 0 unless this sample holds a
/// character" -- written to stop an unread table being judged -- would return 0 here, i.e. blind
/// the oracle in precisely the state it exists to report. `characters == 0` is reported instead,
/// and the caller gates on a latch of having once seen a populated table.
#[test]
fn staged_browse_rows_left_behind_set_exactly_their_own_bits() {
    let table = [
        staged_row("[..] EldenRing"),
        staged_row("[ new ]"),
        staged_row("ER0000.sl2"),
        staged_row("saves/"),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
        empty_slot(),
    ];
    let scan = scan_live_records(table);
    assert_eq!(
        scan.orphaned_mask, 0b1111,
        "the four staged rows, and only those"
    );
    assert_eq!(
        scan.characters, 0,
        "a fully staged table holds no character at all -- which is why the caller cannot use this \
         count alone to decide whether the table is worth judging"
    );
}

/// A label in an UNOCCUPIED slot is not on screen. The native list builder appends a row only where
/// `saveSlotsStates[slot]` is set, so an unoccupied slot's bytes describe nothing the user can see.
/// Pinned because dropping the occupancy term is the obvious "simplification" and it would make the
/// mask non-zero on every healthy boot with fewer than ten characters.
#[test]
fn an_unoccupied_slot_never_contributes_a_bit() {
    let table = [
        character("Maddened Bean", 139),
        LiveRecordSample {
            occupied: false,
            name: "[..] EldenRing",
            level: 0,
            map: 0,
        },
    ];
    assert_eq!(scan_live_records(table).orphaned_mask, 0);
}

/// The assembled-save case must not fire. `save-files/100-Lilbro` holds bodies copied between
/// files, so a record can describe whoever used to hold the slot -- a real character, wrong
/// identity. That is a legitimate save the user plays, and this scan is deliberately blind to it:
/// it answers "are these bytes a character", never "are they the RIGHT character".
#[test]
fn a_record_naming_the_wrong_character_is_still_a_character() {
    let table = [
        character("Dark Moon Bean", 7),
        character("Maddened Bean", 139),
    ];
    assert_eq!(scan_live_records(table).orphaned_mask, 0);
}

/// A genuinely empty save container: ten unoccupied zeroed slots. No bits, and no characters --
/// indistinguishable, here, from a table that has not been deserialized yet. Both are states the
/// caller's latch declines to judge, and neither has a character row to get wrong.
#[test]
fn an_empty_container_yields_neither_bits_nor_characters() {
    let scan = scan_live_records([empty_slot(); 10]);
    assert_eq!(scan.orphaned_mask, 0);
    assert_eq!(scan.characters, 0);
}

/// The configured AUTOLOAD slot OUTRANKS the highest-level scan (run br-20260903-204517-82d2).
///
/// Slot 4 (`Hero` RL7) was the configured autoload slot and the one that loaded. For the 1.2s
/// before the portrait window latched, `read_loading_screen_stats` had no switch selection and no
/// window target, fell through to `BestActiveFallback`, and `best_active_slot()` -- "the active
/// slot holding the most-progressed real character (highest level)" -- answered slot 0, `angrE`
/// RL 100. The user watched a stats panel for a character that was not loading.
#[test]
fn the_configured_autoload_slot_beats_the_highest_level_scan() {
    const SLOTS: i32 = 10;
    const SWITCH_UNSET: usize = usize::MAX;
    assert_eq!(
        loading_screen_stats_slot_source(SWITCH_UNSET, None, Some(4), SLOTS),
        StatsSlotSource::ConfiguredAutoload(4),
        "the configured slot is the one the loader was told to use"
    );
    // ...but only while nothing has observed a slot. Both stronger terms still win outright, so
    // the panel can never disagree with the face the window committed to.
    assert_eq!(
        loading_screen_stats_slot_source(SWITCH_UNSET, Some(2), Some(4), SLOTS),
        StatsSlotSource::PortraitWindow(2)
    );
    assert_eq!(
        loading_screen_stats_slot_source(6, Some(2), Some(4), SLOTS),
        StatsSlotSource::SwitchSelection(6)
    );
    // An unconfigured or nonsense configured slot keeps the old scan rather than inventing one.
    assert_eq!(
        loading_screen_stats_slot_source(SWITCH_UNSET, None, None, SLOTS),
        StatsSlotSource::BestActiveFallback
    );
    for bad in [-1, SLOTS, i32::MIN, i32::MAX] {
        assert_eq!(
            loading_screen_stats_slot_source(SWITCH_UNSET, None, Some(bad), SLOTS),
            StatsSlotSource::BestActiveFallback,
            "configured slot {bad} is not a slot"
        );
    }
}
