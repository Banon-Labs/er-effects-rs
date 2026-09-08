//! The order release happens in, as a state machine that cannot be run out of order or stopped
//! half way.
//!
//! # Why the order is not a comment in one function
//!
//! Release runs down three different paths -- the hotkey, the possessed creature dying, and
//! `DLL_PROCESS_DETACH` -- and each one is a place where somebody can write the steps in a
//! plausible order that is subtly the wrong one. Six of the nine steps have a consequence if
//! they move:
//!
//! * **The HUD is handed back first.** The retarget post-pass reads the creature's
//!   `CSChrDataModule` every frame until it is told to stop, and every step below it either reads
//!   through that creature or is what makes it stop resolving.
//! * **The camera must be cleared after the player has been moved.** Clearing first gives a
//!   visible frame of the old body standing wherever it was; moving first means the camera snaps
//!   to a player already standing in the right place.
//! * **The body is made mortal last of the body steps -- after it has been placed.** Invincibility
//!   is the only thing stopping `CSChrFallModule::Update` charging the body for the difference
//!   between its `lastGroundedPosition` and where it actually is (the gate is byte-read in
//!   [`crate::possess::fall`]), so clearing it before the placement hands back a mortal body at a
//!   position nothing has checked. Both abnormal releases in the reference log did exactly that
//!   and then failed the move.
//! * **The camera's size must go back before the camera does.** While `WorldChrManDbg+0xb8` still
//!   names the creature, `ChrExFollowCam+0x468` still names the patched `LockCamParam` row -- so
//!   undoing them in this order shows at most one frame of the creature framed for a player, and
//!   the other order shows one frame of the player framed for a dragon.
//! * **`ChrCtrl+0x3b0` must be cleared before anything can tear the character down.**
//!   `ChrCtrl::Unref` compares that slot against zero and **DLPanics** when it is non-null. This
//!   is the step that must run even when every step before it failed.
//! * **A spawned creature is removed after that clear**, for exactly the same reason from the other
//!   side: `WorldChrManImp::RemoveChrIns` hands the character to `CSDelayDeleteMan`, and the
//!   destruction that eventually follows is the thing that runs `ChrCtrl::Unref`. Despawning first
//!   arms the crash the step before it exists to prevent -- on a delay, in a destructor.
//! * **Save suppression is lifted last**, because `PlayerIns::UpdateSafePosition` and
//!   `UpdateBlockPosition` write the save's respawn fields the instant that gate opens, and they
//!   must not do it from a position the player is about to leave.
//!
//! # The property that matters more than the order
//!
//! **A failing step does not stop the run.** A release that gives up half way leaves a live
//! `ChrCtrl+0x3b0`, and the next time that character is unloaded the game DLPanics -- so the
//! failure mode of "be careful" is a crash minutes later in an unrelated place. [`Teardown`]
//! therefore records what failed and keeps going, and the tests below pin that.

// Pure state handling; stays ungated so its tests run on the host.
#![cfg_attr(not(windows), allow(dead_code))]

/// One step of a release, in the order it must happen.
///
/// The discriminants are the order. `Step::ALL` is the whole sequence and the tests assert it is
/// sorted, so adding a step in the wrong place fails rather than reorders the release.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Step {
    /// Give the HP / FP / stamina bars back to the real player.
    ///
    /// First, and for the same reason the camera is cleared before the player is moved: it is the
    /// most externally visible lie the possession is telling, so it is the first one to stop.
    /// It must also precede every step below on its own account -- all of them either read
    /// through the creature or make it stop resolving, and the HUD post-pass reads the creature's
    /// `CSChrDataModule` once a frame until it is told not to.
    StopHudRetarget = 0,
    /// Stop writing the player's proxy position every frame. Nothing to undo -- the last write
    /// already landed -- but the per-frame driver has to be told before the player is moved, or it
    /// would put them back on the creature next frame.
    StopColocating = 1,
    /// Put the player on the resolved ground point, and pin their fall bookkeeping to it. Usually
    /// a no-op in effect, because co-location already left them exactly there; it matters when the
    /// creature died airborne and the point resolves to its last grounded position instead.
    ///
    /// **before [`Self::RestoreBody`], and that REORDERING is a bug fix.** See that variant.
    MovePlayer = 2,
    /// Invincibility off, alpha back to opaque, `debugFlags` cleared. The player's own body
    /// becomes an ordinary character again.
    ///
    /// **after [`Self::MovePlayer`], because this step is the one that makes the body mortal and
    /// it must not run while the body is still wherever the world last pushed it.** The two
    /// abnormal releases in the reference log both reported
    /// `FAILED=[restore-body move-player restore-manipulator-vtable]`: `RestoreBody` ran first and
    /// its five writes are and-ed into one result, so the invincibility clear can land while the
    /// move that was supposed to put the body somewhere survivable never happens at all. Running
    /// the placement first means the body is put down and its `lastGroundedPosition` pinned while
    /// it is still immune, and mortality is the last thing handed back.
    ///
    /// The old order had no reason behind it -- the module docs above justify six of the nine
    /// steps and this was not one of them -- and nothing else in the release depends on the body
    /// being restored early: the camera steps read the creature, and the body scale this also
    /// undoes is render-only (`ChrCtrl::SetScaleSize` touches no physics field).
    RestoreBody = 3,
    /// Put `ChrExFollowCam+0x468` and the patched `LockCamParam` row back exactly as they were.
    ///
    /// Before the camera is handed back, for two reasons. The override is still pointing at the
    /// patched row at this moment, so undoing them in this order costs at most one frame of the
    /// creature framed with the player's parameters rather than one frame of the player framed
    /// with a dragon's. And the row is shared game state that anything could read, unlike
    /// `+0x468`, which nothing in the game ever touches -- so the shared thing goes back first.
    RestoreCameraSize = 4,
    /// Clear `WorldChrManDbg+0xb8`. Camera and lock-on return to the real player.
    ClearCameraOverride = 5,
    /// Put the creature's own vtable pointer back and give it its AI back. The step that must
    /// happen.
    ///
    /// Possession swizzles the real `ComManipulator`'s vptr to a patched copy of its own table in
    /// a page we allocated. That page is freed when the `Thunk` drops, so a creature left pointing
    /// at it dispatches through unmapped memory on its very next tick.
    RestoreManipulatorVtable = 6,
    /// Hand a creature this mod spawned back to the game. A no-op for a creature the map placed.
    ///
    /// **after [`Self::RestoreManipulatorVtable`], and that is a crash if it moves.**
    /// `WorldChrManImp::RemoveChrIns` passes the character to `CSDelayDeleteMan`, and the eventual
    /// destruction dispatches through the MANIPULATOR'S VTABLE -- which, until the step before has
    /// run, is ours, in a page that is freed with the possession. Removing first therefore arms a
    /// call through unmapped memory inside the engine's own teardown, where nothing of ours is
    /// left to notice.
    ///
    /// The ordering predates the swizzle and the reason changed with it: under the old
    /// `ChrCtrl+0x3b0` override it was `ChrCtrl::Unref`, which DLPanics on a non-null slot. That
    /// hazard is retired -- this design leaves `+0x3b0` null -- but the order it forced is still
    /// exactly right, so the test that pins it stayed while its justification was replaced.
    DespawnCreature = 7,
    /// Whatever was holding the save off, released. Last, always.
    LiftSaveSuppression = 8,
}

impl Step {
    /// The release, in order.
    pub(crate) const ALL: [Self; 9] = [
        Self::StopHudRetarget,
        Self::StopColocating,
        Self::MovePlayer,
        Self::RestoreBody,
        Self::RestoreCameraSize,
        Self::ClearCameraOverride,
        Self::RestoreManipulatorVtable,
        Self::DespawnCreature,
        Self::LiftSaveSuppression,
    ];

    /// For the log line.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::StopHudRetarget => "stop-hud-retarget",
            Self::StopColocating => "stop-colocating",
            Self::MovePlayer => "move-player",
            Self::RestoreBody => "restore-body",
            Self::RestoreCameraSize => "restore-camera-size",
            Self::ClearCameraOverride => "clear-camera-override",
            Self::RestoreManipulatorVtable => "restore-manipulator-vtable",
            Self::DespawnCreature => "despawn-creature",
            Self::LiftSaveSuppression => "lift-save-suppression",
        }
    }

    /// Is this a step whose failure leaves the game in a state that will crash later?
    ///
    /// Exactly one qualifies. A failed `RestoreBody` leaves the player invisible until the next
    /// possession, which is bad and survivable; a failed `RestoreManipulatorVtable` leaves a
    /// DLPanic armed inside `ChrCtrl::Unref` for whenever that creature is unloaded.
    ///
    /// `DespawnCreature` deliberately does not qualify, and the difference is the point: its
    /// failure leaves a live NPC standing in the world, which is visible, survivable, and cleaned
    /// up by the next map load. It gets a [`Self::failure_note`] instead, because "despawn-creature
    /// failed" does not tell the reader what is now standing behind them.
    pub(crate) const fn is_critical(self) -> bool {
        matches!(self, Self::RestoreManipulatorVtable)
    }

    /// What the reader needs to know when this step fails, spelled out because the consequence is
    /// nowhere near the cause.
    ///
    /// `None` for the steps whose failure is self-explanatory from the name.
    pub(crate) const fn failure_note(self) -> Option<&'static str> {
        match self {
            Self::RestoreManipulatorVtable => {
                Some("ChrCtrl+0x3b0 IS STILL ARMED; ChrCtrl::Unref will DLPanic")
            }
            Self::DespawnCreature => Some(
                "the creature this mod created is STILL IN THE WORLD and nothing else will remove \
                 it; it holds one of the fourteen buddy roster slots until the next map load",
            ),
            _ => None,
        }
    }
}

/// Why a release was asked for. Carried into the log so "it let go on its own" and "the player
/// pressed the key" are never the same line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reason {
    /// The possess hotkey, pressed again.
    Hotkey,
    /// The possessed creature died. `[target].release_on_death`.
    CreatureDied,
    /// The possessed creature stopped being readable -- despawned, unloaded, or the pointer chain
    /// stopped resolving.
    CreatureGone,
    /// A spawned creature never became drivable inside `[spawn].readiness_ms`.
    ///
    /// Its own reason rather than [`Self::CreatureGone`], which is what it was first written as and
    /// is the opposite claim: the creature is still there, and still ours to remove. Reporting a
    /// deadline as a disappearance would send the reader looking for what removed it.
    SpawnTimedOut,
    /// `DLL_PROCESS_DETACH`, or the shell shutting down. The path that exists purely so the
    /// override slot is not left armed in a process that is about to unload us.
    Shutdown,
}

impl Reason {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Hotkey => "hotkey",
            Self::CreatureDied => "creature-died",
            Self::CreatureGone => "creature-gone",
            Self::SpawnTimedOut => "spawn-timed-out",
            Self::Shutdown => "shutdown",
        }
    }
}

/// A release in progress: runs [`Step::ALL`] in order, records what failed, and never stops early.
#[derive(Clone, Debug)]
pub(crate) struct Teardown {
    reason: Reason,
    done: Vec<Step>,
    failed: Vec<Step>,
}

impl Teardown {
    pub(crate) const fn new(reason: Reason) -> Self {
        Self {
            reason,
            done: Vec::new(),
            failed: Vec::new(),
        }
    }

    /// Run the whole release. `step` performs one step and answers whether it worked.
    ///
    /// Every step is attempted, in order, whatever the ones before it answered. See the module
    /// docs for why "stop on the first failure" is the wrong shape here.
    pub(crate) fn run(&mut self, mut step: impl FnMut(Step) -> bool) {
        for one in Step::ALL {
            if step(one) {
                self.done.push(one);
            } else {
                self.failed.push(one);
            }
        }
    }

    /// Did every step work?
    fn is_clean(&self) -> bool {
        self.failed.is_empty()
    }

    /// Did a step whose failure arms a later crash fail?
    pub(crate) fn has_critical_failure(&self) -> bool {
        self.failed.iter().copied().any(Step::is_critical)
    }

    /// `release: reason=hotkey steps=9/9 ok` -- or the same line naming what did not work.
    pub(crate) fn line(&self) -> String {
        let mut line = format!(
            "release: reason={} steps={}/{}",
            self.reason.name(),
            self.done.len(),
            Step::ALL.len()
        );
        if self.is_clean() {
            line.push_str(" ok");
            return line;
        }
        line.push_str(" FAILED=[");
        for (index, failure) in self.failed.iter().enumerate() {
            if index > 0 {
                line.push(' ');
            }
            line.push_str(failure.name());
        }
        line.push(']');
        // Spelled out because the consequence is nowhere near the cause -- a DLPanic the next time
        // that character is unloaded, or an NPC left standing in the world -- and neither is
        // guessable from the step's name.
        for failure in &self.failed {
            if let Some(note) = failure.failure_note() {
                line.push_str(" -- ");
                line.push_str(note);
            }
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The order is the discriminants, and the list must be sorted. A step inserted in the wrong
    /// place fails here rather than silently reordering the release.
    #[test]
    fn the_steps_are_in_the_order_they_must_happen() {
        let mut sorted = Step::ALL;
        sorted.sort_unstable();
        assert_eq!(sorted, Step::ALL);
        assert_eq!(Step::ALL[0], Step::StopHudRetarget);
        assert_eq!(*Step::ALL.last().unwrap(), Step::LiftSaveSuppression);
    }

    /// The two orderings with a visible consequence, asserted as relations rather than indices so
    /// they survive a step being inserted between them.
    #[test]
    fn the_player_moves_before_the_camera_is_released_and_the_save_is_last() {
        assert!(
            Step::MovePlayer < Step::ClearCameraOverride,
            "clearing the camera first shows a frame of the abandoned body"
        );
        assert!(
            Step::StopColocating < Step::MovePlayer,
            "co-location still running would put the player back on the creature"
        );
        assert!(
            Step::RestoreCameraSize < Step::ClearCameraOverride,
            "handing the camera back first shows a frame of the player framed for a dragon"
        );
        assert!(
            Step::MovePlayer < Step::RestoreCameraSize,
            "the creature's framing is still the right one until the player has been moved"
        );
        // The ordering this fix exists for. `RestoreBody` is what takes the invincibility bit off,
        // and that bit is the only thing stopping `CSChrFallModule::Update` charging the body for
        // `lastGroundedPosition.y - position.y`. It must not run until the body has been put
        // somewhere and that field has been pinned to it.
        assert!(
            Step::MovePlayer < Step::RestoreBody,
            "a body made mortal before it is placed is charged for wherever the world left it"
        );
        for step in Step::ALL {
            if step != Step::LiftSaveSuppression {
                assert!(
                    step < Step::LiftSaveSuppression,
                    "{} must precede the save gate opening",
                    step.name()
                );
            }
        }
    }

    /// The HUD goes back first. Every step after it either reads through the creature or is what
    /// stops the creature resolving, and the retarget reads its `CSChrDataModule` once a frame
    /// until this step runs.
    #[test]
    fn the_hud_is_handed_back_before_anything_else_touches_the_creature() {
        for step in Step::ALL {
            if step != Step::StopHudRetarget {
                assert!(
                    Step::StopHudRetarget < step,
                    "{} must not precede the HUD hand-back",
                    step.name()
                );
            }
        }
    }

    /// A clean run does every step once, in order.
    #[test]
    fn a_clean_run_performs_every_step_in_order() {
        let mut seen = Vec::new();
        let mut teardown = Teardown::new(Reason::Hotkey);
        teardown.run(|step| {
            seen.push(step);
            true
        });
        assert_eq!(seen, Step::ALL.to_vec());
        assert!(teardown.is_clean());
        assert!(!teardown.has_critical_failure());
        assert_eq!(teardown.line(), "release: reason=hotkey steps=9/9 ok");
    }

    /// The ordering that is a crash if it moves. `WorldChrManImp::RemoveChrIns` hands the character
    /// to `CSDelayDeleteMan`, and the delayed destruction runs `ChrCtrl::Unref`, which DLPanics on a
    /// non-null `ChrCtrl+0x3b0`. Despawning before the override is cleared therefore arms the exact
    /// crash the clear exists to prevent -- and arms it on a delay, in a destructor.
    #[test]
    fn the_creature_is_despawned_only_after_its_override_slot_has_been_cleared() {
        assert!(
            Step::RestoreManipulatorVtable < Step::DespawnCreature,
            "RemoveChrIns before the override clear is a delayed DLPanic in ChrCtrl::Unref"
        );
        // ...and the run order matches, not merely the discriminants.
        let clear = Step::ALL
            .iter()
            .position(|step| *step == Step::RestoreManipulatorVtable)
            .expect("present");
        let despawn = Step::ALL
            .iter()
            .position(|step| *step == Step::DespawnCreature)
            .expect("present");
        assert!(clear < despawn);
    }

    /// A despawn that did not happen leaves a live NPC nobody will remove, which is not guessable
    /// from the step's name -- and is not the crash class, so it must not claim to be.
    #[test]
    fn a_failed_despawn_names_the_orphan_and_is_not_reported_as_a_crash() {
        let mut teardown = Teardown::new(Reason::Hotkey);
        teardown.run(|step| step != Step::DespawnCreature);
        assert!(
            !teardown.has_critical_failure(),
            "an orphan is not a DLPanic"
        );
        let line = teardown.line();
        assert!(line.contains("despawn-creature"), "{line}");
        assert!(line.contains("STILL IN THE WORLD"), "{line}");
        assert!(!line.contains("DLPanic"), "{line}");
    }

    /// The property that matters. A step failing must not stop the run -- in particular the
    /// override clear must still be attempted after everything before it has failed.
    #[test]
    fn a_failing_step_does_not_stop_the_ones_after_it() {
        let mut seen = Vec::new();
        let mut teardown = Teardown::new(Reason::CreatureDied);
        teardown.run(|step| {
            seen.push(step);
            // Everything before the override clear fails.
            step >= Step::RestoreManipulatorVtable
        });
        assert_eq!(seen, Step::ALL.to_vec(), "every step was still attempted");
        assert!(!teardown.is_clean());
        assert!(
            !teardown.has_critical_failure(),
            "the one that matters did run"
        );
        let line = teardown.line();
        assert!(line.contains("reason=creature-died"), "{line}");
        assert!(line.contains("steps=3/9"), "{line}");
        assert!(line.contains("restore-body"), "{line}");
        assert!(!line.contains("DLPanic"), "{line}");
    }

    /// Exactly one step arms a later crash, and its failure says so in words rather than leaving
    /// the reader to notice which of six names is the dangerous one.
    #[test]
    fn only_the_override_clear_is_critical_and_it_says_so_when_it_fails() {
        for step in Step::ALL {
            assert_eq!(
                step.is_critical(),
                step == Step::RestoreManipulatorVtable,
                "{}",
                step.name()
            );
        }
        let mut teardown = Teardown::new(Reason::Shutdown);
        teardown.run(|step| step != Step::RestoreManipulatorVtable);
        assert!(teardown.has_critical_failure());
        let line = teardown.line();
        assert!(line.contains("ChrCtrl+0x3b0 IS STILL ARMED"), "{line}");
        assert!(line.contains("DLPanic"), "{line}");
        assert!(line.contains("steps=8/9"), "{line}");
    }

    /// Every reason spells itself for the log, so "it let go on its own" and "the player pressed
    /// the key" are never the same line.
    #[test]
    fn every_reason_has_its_own_name() {
        let names: Vec<&str> = [
            Reason::Hotkey,
            Reason::CreatureDied,
            Reason::CreatureGone,
            Reason::SpawnTimedOut,
            Reason::Shutdown,
        ]
        .into_iter()
        .map(Reason::name)
        .collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "{names:?}");
    }
}
