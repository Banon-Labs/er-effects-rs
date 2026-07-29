//! Load-count witnesses and the cross-check that makes them contradict each other OUT LOUD.
//!
//! WHY THIS EXISTS. A captured 3-load session (boot Continue + two System->Quit->Load-Profile
//! reloads of the SAME save slot) produced these numbers in one telemetry file:
//!
//! ```text
//! system_quit_profile_load_activate_count  4   2 picker browse/pick steps + 2 slot arms
//! system_quit_continue_confirm_allow_count 3   forwarded confirms == THE TOTAL, boot included
//! oracle_current_load_epoch                2   switch-machine commits only; boot never counted
//! oracle_save_picker_pick_count            2   picker file-picks; the boot load has none
//! oracle_save_picker_resubmit_count        2   window resubmits after a pick
//! oracle_switch_reload_committed           1   a BOOLEAN LATCH, reset per switch -- not a count
//! ```
//!
//! Six fields, four values, all reachable when asking "how many loads". Every one was CORRECT for
//! its own event class -- and precisely because each was individually correct, nothing in the run
//! noticed that recovering the number 3 required already knowing all six definitions. Two
//! independent hand-derivations from that file landed on 2 and on ">=3". Neither is the answer.
//!
//! No counter was under-reporting. There is no same-slot dedupe anywhere in the load path: nothing
//! keys on the loaded identity, and the captured run proves it -- the same save file was picked
//! twice, at `+39864ms` and `+66532ms`, with identical `len=28967888 hash=0x394158714daf526b`, and
//! BOTH picks counted. The 2s are narrower event classes than the 3s, not lost loads.
//!
//! `oracle_switch_reload_committed` deserves its own warning: it is a one-shot `compare_exchange(0,
//! 1)` latch cleared at the start of every switch, so it can never exceed 1 and can never agree with
//! any count. Reading it as "1 reload" is not an off-by-N, it is a category error.
//!
//! The defect was never a miscount. The file published six narrow event counts, no total, and no
//! statement of how the narrow counts compose into the total -- so the composition had to be
//! reconstructed by hand, and a hand reconstruction is exactly what nobody audits.
//!
//! THE DECOMPOSITION. Every world load in a session is one forwarded `continue_confirm` call.
//! That hook classifies each forward into exactly one of three buckets:
//!
//!   * `switch_reload_commits` -- a System->Quit->Load-Profile reload committed a fresh deserialize
//!     (the switch machine was mid-flight). This is the ONLY bucket the load epoch counts.
//!   * `non_switch_forwards`   -- a Continue outside the switch machine: the boot/title load.
//!   * `world_up_forwards`     -- a confirm that arrived while the previous world was still up (a
//!     state we never drive; previously logged and counted by NOTHING).
//!
//! so `forwards == switch_commits + non_switch + world_up`, exactly, by construction. That identity
//! is the load-count audit: violate it and a load landed in no bucket or was counted twice.
//!
//! THE EPOCH IS AN INDEX, NOT A COUNT. `oracle_current_load_epoch` is
//! `SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT`, incremented ONLY inside the switch machine.
//! The boot load does not increment it. So in a session whose loads are (boot, reload, reload) it
//! reads 2 -- which is simultaneously "2 switch reloads committed" and "the current load is index 2,
//! zero-based". `total = epoch + 1` therefore holds ONLY while a session performs exactly one
//! non-switch load, and silently lies otherwise (a telemetry-only run with no character load, or any
//! second non-switch Continue). Read [`LoadCountWitnesses::total_world_loads`] instead; it needs no
//! such assumption.
//!
//! WHAT THE EPOCH STILL CANNOT TELL YOU: whether those loads SUCCEEDED. Two captured runs both read
//! `oracle_current_load_epoch = 2`; one reached world residency three times, the other reached it
//! ONCE and softlocked. The epoch counts commits, not outcomes, so equal epochs across runs mean
//! "both reached a third load attempt" and nothing whatsoever about whether the worlds came up. Any
//! cross-run claim resting on epoch equality alone is void.

/// One failed load-count invariant. Bit flags rather than a message so the set survives as a single
/// integer in telemetry and can be asserted on exactly in tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoadCountMismatch(u32);

impl LoadCountMismatch {
    /// `forwards != switch_commits + non_switch + world_up`. The hard one: the continue_confirm hook
    /// puts every forward in exactly one bucket, so any drift means a load was dropped from the
    /// total, double-counted into it, or classified into a bucket that does not exist yet.
    pub const DECOMPOSITION: u32 = 1 << 0;
    /// `picker_activations + slot_activations != total_activations`. The two sub-counters are
    /// incremented on mutually exclusive branches of one hook, so they must partition the total.
    pub const ACTIVATION_SPLIT: u32 = 1 << 1;
    /// `picks + pick_rejects + repopulates > picker_activations`, checked ONLY when
    /// `picker_activations > 0`. Each in-game-picker activation produces at most one of those
    /// outcomes, so their sum cannot exceed the activation count.
    ///
    /// The zero-activation exemption is not laziness -- it is measured. A captured run on the
    /// OS-native picker surface recorded 2 picks with 0 picker activations, because an OS file-dialog
    /// pick never passes through the ProfileLoadDialog activate hook that increments the activation
    /// counter. Asserting unconditionally would fire on every OS-picker run and make this whole
    /// mismatch field untrustworthy on arrival, which is precisely the failure being fixed. Zero
    /// picker activations therefore means "the picks came from another surface, so the in-game
    /// invariant does not apply", not "the invariant held".
    pub const PICKER_OUTCOMES: u32 = 1 << 2;
    /// `switch_commits > 0` while `forwards == 0`. Every commit forwards a confirm, so a commit with
    /// no forward behind it means the total-load witness itself stopped counting.
    pub const COMMIT_WITHOUT_FORWARD: u32 = 1 << 3;
    /// `switch_commits > slot_activations`. A switch reload committed with no slot activation to arm
    /// it -- i.e. something armed the load without going through the user's pick. Fires on a
    /// programmatic/autopilot arm, which is the point: that is NOT the user path under test.
    pub const SLOT_ARM_DEFICIT: u32 = 1 << 4;

    const ALL: [(u32, &'static str); 5] = [
        (Self::DECOMPOSITION, "decomposition"),
        (Self::ACTIVATION_SPLIT, "activation-split"),
        (Self::PICKER_OUTCOMES, "picker-outcomes"),
        (Self::COMMIT_WITHOUT_FORWARD, "commit-without-forward"),
        (Self::SLOT_ARM_DEFICIT, "slot-arm-deficit"),
    ];

    /// The raw flag set, for telemetry.
    pub fn bits(self) -> u32 {
        self.0
    }

    /// True when every invariant held.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// True when `flag` (one of the associated constants) failed.
    pub fn contains(self, flag: u32) -> bool {
        self.0 & flag != 0
    }

    /// How many invariants failed -- the single number a watcher can alarm on.
    pub fn count(self) -> u32 {
        self.0.count_ones()
    }

    /// `"none"`, or the failed invariants joined by `|`, for the telemetry signature.
    pub fn names(self) -> String {
        if self.is_empty() {
            return "none".to_owned();
        }
        let mut out = String::new();
        for (flag, name) in Self::ALL {
            if self.contains(flag) {
                if !out.is_empty() {
                    out.push('|');
                }
                out.push_str(name);
            }
        }
        out
    }
}

/// Every counter a reader might reach for when asking "how many times did this session load a
/// world?", gathered in one place so the answer never has to be assembled by hand again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LoadCountWitnesses {
    /// Forwarded `continue_confirm` calls: the authoritative total world-load count. Boot included.
    pub continue_confirm_forwards: usize,
    /// Fresh deserializes committed inside the switch machine == `oracle_current_load_epoch`.
    pub switch_reload_commits: usize,
    /// Forwards from outside the switch machine (the boot/title Continue).
    pub non_switch_forwards: usize,
    /// Forwards that arrived while the previous world was still up.
    pub world_up_forwards: usize,
    /// Confirms refused outright. These return early and are NOT forwards, so they are excluded
    /// from the decomposition on purpose.
    pub continue_confirm_blocks: usize,
    /// ProfileLoadDialog activations routed to the save-file browser (browse steps AND file picks).
    pub picker_activations: usize,
    /// ProfileLoadDialog activations that armed a character slot load.
    pub slot_activations: usize,
    /// Both of the above together -- the long-standing `system_quit_profile_load_activate_count`.
    pub total_activations: usize,
    /// Save files accepted by the picker's ingest.
    pub picker_picks: usize,
    /// Save files the picker's ingest refused.
    pub picker_pick_rejects: usize,
    /// Picker activations that re-listed the browse rows instead of picking.
    pub picker_repopulates: usize,
}

impl LoadCountWitnesses {
    /// The number this whole module exists to publish: world loads this session, boot included.
    ///
    /// Do NOT derive this from the activation count. Activations are per browse step and per slot
    /// arm, so `activations / 2` matches the load count only in a session with zero directory
    /// navigation -- true of the captured run by luck (`repopulates == 0`), false in general.
    pub fn total_world_loads(&self) -> usize {
        self.continue_confirm_forwards
    }

    /// Zero-based index of the load in flight, or `None` before the first load. This is the value
    /// `oracle_current_load_epoch` is usually MEANT to be; it differs from the epoch by exactly the
    /// non-switch (boot) loads the epoch skips.
    pub fn current_load_index(&self) -> Option<usize> {
        self.continue_confirm_forwards.checked_sub(1)
    }

    /// Check every load-count invariant. An empty result means the witnesses agree; anything else
    /// means the run contradicted itself and the numbers need a human before they are quoted.
    pub fn mismatches(&self) -> LoadCountMismatch {
        let mut bits = 0u32;
        if self.continue_confirm_forwards
            != self.switch_reload_commits + self.non_switch_forwards + self.world_up_forwards
        {
            bits |= LoadCountMismatch::DECOMPOSITION;
        }
        if self.picker_activations + self.slot_activations != self.total_activations {
            bits |= LoadCountMismatch::ACTIVATION_SPLIT;
        }
        if self.picker_activations > 0
            && self.picker_picks + self.picker_pick_rejects + self.picker_repopulates
                > self.picker_activations
        {
            bits |= LoadCountMismatch::PICKER_OUTCOMES;
        }
        if self.switch_reload_commits > 0 && self.continue_confirm_forwards == 0 {
            bits |= LoadCountMismatch::COMMIT_WITHOUT_FORWARD;
        }
        if self.switch_reload_commits > self.slot_activations {
            bits |= LoadCountMismatch::SLOT_ARM_DEFICIT;
        }
        LoadCountMismatch(bits)
    }

    /// Every witness on one line, with the total stated rather than implied. This is the field a
    /// future reader should quote instead of picking one counter and dividing.
    pub fn signature(&self) -> String {
        let mismatches = self.mismatches();
        format!(
            "loads={} idx={} fwd={}(switch={}+boot={}+worldup={}) blocked={} act={}(picker={}+slot={}) pick={} reject={} repop={} mismatch={}",
            self.total_world_loads(),
            self.current_load_index()
                .map(|index| index as isize)
                .unwrap_or(-1),
            self.continue_confirm_forwards,
            self.switch_reload_commits,
            self.non_switch_forwards,
            self.world_up_forwards,
            self.continue_confirm_blocks,
            self.total_activations,
            self.picker_activations,
            self.slot_activations,
            self.picker_picks,
            self.picker_pick_rejects,
            self.picker_repopulates,
            mismatches.names(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The captured 3-load session, in witness form: boot Continue + two reloads of the SAME save
    /// slot through the in-game picker. Ground truth for these numbers is the run captured in
    /// `er-effects-telemetry.json` / `er-effects-autoload-debug.log`, whose log carries three
    /// `+0x35(phase)=10` world-residency transitions, four ProfileLoadDialog `ACTIVATE` lines (two
    /// picker + two slot), and two `applied selected save preview` picks.
    fn captured_three_load_run() -> LoadCountWitnesses {
        LoadCountWitnesses {
            continue_confirm_forwards: 3,
            switch_reload_commits: 2,
            non_switch_forwards: 1,
            world_up_forwards: 0,
            continue_confirm_blocks: 0,
            picker_activations: 2,
            slot_activations: 2,
            total_activations: 4,
            picker_picks: 2,
            picker_pick_rejects: 0,
            picker_repopulates: 0,
        }
    }

    #[test]
    fn captured_run_reports_three_loads_and_agrees_with_itself() {
        let run = captured_three_load_run();
        assert_eq!(run.total_world_loads(), 3, "three worlds were loaded");
        assert_eq!(
            run.current_load_index(),
            Some(2),
            "zero-based index of load 3"
        );
        assert!(
            run.mismatches().is_empty(),
            "captured run must be self-consistent, got {}",
            run.mismatches().names()
        );
    }

    /// The epoch reads 2 for a 3-load session. That is the whole 2-vs-3 discrepancy, and it is an
    /// index/count confusion, not a lost load: the boot load never increments the epoch.
    #[test]
    fn epoch_lags_the_load_count_by_the_non_switch_loads() {
        let run = captured_three_load_run();
        assert_eq!(run.switch_reload_commits, 2, "the epoch's own value");
        assert_eq!(
            run.total_world_loads() - run.switch_reload_commits,
            run.non_switch_forwards,
            "the gap between the epoch and the load count IS the non-switch loads"
        );
    }

    /// The point of the fix: loading the SAME slot again moves the total. Nothing keys on the loaded
    /// identity, so a repeat is a first-class load.
    #[test]
    fn repeat_load_of_the_same_slot_increments_the_total() {
        let mut run = captured_three_load_run();
        let before = run.total_world_loads();

        // A fourth load of the same save: one more picker activation + pick, one more slot
        // activation, one more forwarded confirm committing a switch reload.
        run.picker_activations += 1;
        run.picker_picks += 1;
        run.slot_activations += 1;
        run.total_activations += 2;
        run.continue_confirm_forwards += 1;
        run.switch_reload_commits += 1;

        assert_eq!(run.total_world_loads(), before + 1, "repeat load counted");
        assert_eq!(run.current_load_index(), Some(3));
        assert!(
            run.mismatches().is_empty(),
            "a repeat load is ordinary, got {}",
            run.mismatches().names()
        );
    }

    /// A session with no load at all has no current index -- distinguishable from load index 0,
    /// which the epoch alone cannot do (it reads 0 in both cases).
    #[test]
    fn no_load_has_no_index_but_epoch_zero_is_ambiguous() {
        let empty = LoadCountWitnesses::default();
        assert_eq!(empty.total_world_loads(), 0);
        assert_eq!(empty.current_load_index(), None);
        assert!(empty.mismatches().is_empty());

        let booted = LoadCountWitnesses {
            continue_confirm_forwards: 1,
            non_switch_forwards: 1,
            ..LoadCountWitnesses::default()
        };
        assert_eq!(booted.current_load_index(), Some(0));
        assert_eq!(
            empty.switch_reload_commits, booted.switch_reload_commits,
            "the epoch cannot tell these two sessions apart; the index can"
        );
    }

    /// The regression this module is armed against: the continue_confirm hook used to increment its
    /// allow counter TWICE for one call on the `!native_slot_proven` branch. That inflated the
    /// total-load witness by one per unproven reload. The decomposition catches it.
    #[test]
    fn double_counted_forward_trips_the_decomposition_check() {
        let inflated = LoadCountWitnesses {
            // 4 forwards recorded, but only 3 loads happened (2 switch + 1 boot).
            continue_confirm_forwards: 4,
            ..captured_three_load_run()
        };
        let mismatches = inflated.mismatches();
        assert!(
            mismatches.contains(LoadCountMismatch::DECOMPOSITION),
            "an over-counted forward must be caught, got {}",
            mismatches.names()
        );
        assert_eq!(mismatches.count(), 1, "exactly one invariant broke");
        assert_eq!(mismatches.names(), "decomposition");
    }

    /// A load that lands in no bucket -- the undercount shape -- is the same identity failing from
    /// the other side.
    #[test]
    fn unclassified_load_trips_the_decomposition_check() {
        let dropped = LoadCountWitnesses {
            continue_confirm_forwards: 3,
            switch_reload_commits: 2,
            non_switch_forwards: 0,
            ..captured_three_load_run()
        };
        assert!(
            dropped
                .mismatches()
                .contains(LoadCountMismatch::DECOMPOSITION),
            "a forward in no bucket must be caught"
        );
    }

    /// Two counters that must partition one hook's calls, disagreeing by 2x -- the shape that went
    /// unnoticed by hand.
    #[test]
    fn activation_split_that_does_not_add_up_is_caught() {
        let skewed = LoadCountWitnesses {
            picker_activations: 2,
            slot_activations: 2,
            total_activations: 8,
            ..captured_three_load_run()
        };
        let mismatches = skewed.mismatches();
        assert!(mismatches.contains(LoadCountMismatch::ACTIVATION_SPLIT));
        assert!(
            !mismatches.contains(LoadCountMismatch::DECOMPOSITION),
            "the load total is unaffected by an activation miscount"
        );
    }

    /// More picker outcomes than picker activations: an outcome was recorded without the activation
    /// that must precede it.
    #[test]
    fn more_picker_outcomes_than_activations_is_caught() {
        let skewed = LoadCountWitnesses {
            picker_activations: 2,
            picker_picks: 2,
            picker_pick_rejects: 1,
            ..captured_three_load_run()
        };
        assert!(
            skewed
                .mismatches()
                .contains(LoadCountMismatch::PICKER_OUTCOMES),
            "picks + rejects + repopulates cannot exceed activations"
        );
    }

    /// The OS-native picker surface: picks arrive from an OS file dialog, never through the
    /// ProfileLoadDialog activate hook, so `picker_activations` is 0 while `picks` is not. Taken from
    /// a captured run (`oracle_save_picker_surface = 1`, picks 2, activations 0 of 2 total, all
    /// slot). Must NOT fire -- an unconditional check here would alarm on every OS-picker run.
    #[test]
    fn os_picker_picks_without_in_game_activations_do_not_fire() {
        let os_surface = LoadCountWitnesses {
            continue_confirm_forwards: 3,
            switch_reload_commits: 2,
            non_switch_forwards: 1,
            world_up_forwards: 0,
            continue_confirm_blocks: 0,
            picker_activations: 0,
            slot_activations: 2,
            total_activations: 2,
            picker_picks: 2,
            picker_pick_rejects: 0,
            picker_repopulates: 0,
        };
        assert!(
            !os_surface
                .mismatches()
                .contains(LoadCountMismatch::PICKER_OUTCOMES),
            "OS-picker picks bypass the activate hook by design"
        );
        assert!(
            os_surface.mismatches().is_empty(),
            "the OS-picker run is self-consistent, got {}",
            os_surface.mismatches().names()
        );
        assert_eq!(
            os_surface.total_world_loads(),
            3,
            "same three loads as the in-game run"
        );
    }

    /// With the in-game picker demonstrably driving, the outcome check is live again.
    #[test]
    fn picker_outcomes_still_checked_once_the_in_game_picker_is_driving() {
        let in_game = LoadCountWitnesses {
            picker_activations: 1,
            picker_picks: 2,
            picker_pick_rejects: 0,
            picker_repopulates: 0,
            total_activations: 3,
            slot_activations: 2,
            ..captured_three_load_run()
        };
        assert!(
            in_game
                .mismatches()
                .contains(LoadCountMismatch::PICKER_OUTCOMES),
            "2 picks cannot come from 1 activation"
        );
    }

    /// A commit with no slot activation behind it: something armed the load without the user's pick.
    #[test]
    fn commit_without_a_slot_arm_is_caught() {
        let programmatic = LoadCountWitnesses {
            switch_reload_commits: 2,
            slot_activations: 0,
            total_activations: 2,
            ..captured_three_load_run()
        };
        assert!(
            programmatic
                .mismatches()
                .contains(LoadCountMismatch::SLOT_ARM_DEFICIT),
            "a load armed outside the user path must be visible"
        );
    }

    /// Commits recorded while the total-load witness reads zero.
    #[test]
    fn commit_without_any_forward_is_caught() {
        let broken = LoadCountWitnesses {
            continue_confirm_forwards: 0,
            switch_reload_commits: 2,
            non_switch_forwards: 0,
            world_up_forwards: 0,
            ..captured_three_load_run()
        };
        let mismatches = broken.mismatches();
        assert!(mismatches.contains(LoadCountMismatch::COMMIT_WITHOUT_FORWARD));
        assert!(mismatches.contains(LoadCountMismatch::DECOMPOSITION));
        assert!(mismatches.count() >= 2);
    }

    /// A confirm arriving on a live world is a real forward and must appear in the total.
    #[test]
    fn world_up_forward_is_part_of_the_total() {
        let run = LoadCountWitnesses {
            continue_confirm_forwards: 4,
            switch_reload_commits: 2,
            non_switch_forwards: 1,
            world_up_forwards: 1,
            ..captured_three_load_run()
        };
        assert!(
            run.mismatches().is_empty(),
            "world_up has its own bucket, got {}",
            run.mismatches().names()
        );
        assert_eq!(run.total_world_loads(), 4);
    }

    /// Blocked confirms return early and are not forwards, so they must not enter the identity.
    #[test]
    fn blocked_confirms_stay_out_of_the_decomposition() {
        let run = LoadCountWitnesses {
            continue_confirm_blocks: 5,
            ..captured_three_load_run()
        };
        assert!(
            run.mismatches().is_empty(),
            "blocks are excluded by design, got {}",
            run.mismatches().names()
        );
        assert_eq!(run.total_world_loads(), 3, "blocks are not loads");
    }

    #[test]
    fn signature_states_the_total_and_the_verdict() {
        let signature = captured_three_load_run().signature();
        assert!(signature.contains("loads=3"), "{signature}");
        assert!(signature.contains("idx=2"), "{signature}");
        assert!(
            signature.contains("fwd=3(switch=2+boot=1+worldup=0)"),
            "{signature}"
        );
        assert!(signature.contains("act=4(picker=2+slot=2)"), "{signature}");
        assert!(signature.contains("mismatch=none"), "{signature}");
    }

    #[test]
    fn signature_names_every_broken_invariant() {
        let broken = LoadCountWitnesses {
            continue_confirm_forwards: 9,
            total_activations: 99,
            ..captured_three_load_run()
        };
        let signature = broken.signature();
        assert!(signature.contains("loads=9"), "{signature}");
        assert!(
            signature.contains("mismatch=decomposition|activation-split"),
            "{signature}"
        );
    }

    #[test]
    fn no_load_signature_reports_index_minus_one() {
        let signature = LoadCountWitnesses::default().signature();
        assert!(signature.contains("loads=0"), "{signature}");
        assert!(signature.contains("idx=-1"), "{signature}");
    }
}
