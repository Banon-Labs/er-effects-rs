//! The engine itself: what possession start, each frame, and release actually do.
//!
//! Split out of `possess/mod.rs` so the module tree stays honest about what is host-testable.
//! Everything in here reads or writes live game memory through the windows-only `eldenring`
//! bindings; the offset table, the thunk emitter, the teardown ordering and the movement math are
//! siblings of this file precisely so they can be proved by `cargo test` without a game.
//!
//! See the module docs on [`crate::possess`] for what the whole thing does and what it leaves out.

use er_game_base::game_build::{describe_build, game_file_version};

use crate::camera;
use crate::engine::{PossessionEngine, PossessionOutcome, PossessionRequest};
use crate::hud;
use crate::input::FaceEdges;
use crate::log::possess_log;
use crate::moveset::banner;
use crate::moveset::chain::{Availability, Held, Playing, Reading, Source};
use crate::moveset::dispatch::{
    Context, Dispatcher, Hand, Input, Locomotion, NoMove, PageTurn, Press, Released,
};
use crate::moveset::table::{self, Denial};
use crate::moveset::watchdog::{Sample, Verdict, Watchdog};
use crate::moveset::{derived, table as moveset_table};
use crate::possess::game::{self, AnimFrame, Chr};
use crate::possess::layout::chr_ins;
use crate::possess::teardown::{Reason, Step, Teardown};
use crate::possess::thunk::Thunk;
use crate::possess::{body_size, fall, intent, layout, netdamage};
use crate::settings::{Bucket, TargetMode};
use crate::spawn::game as spawn_game;
use crate::spawn::placement;
use crate::spawn::readiness::{Gate, Poll, Readiness};
use crate::spawn::request::SpawnSpec;

/// Fully opaque, for the player's own body while somebody else is being worn.
const ALPHA_INVISIBLE: f32 = 0.0;
/// ...and back.
const ALPHA_OPAQUE: f32 = 1.0;

/// The animation the watchdog fires to get a stuck creature back to neutral.
///
/// `W_Event0000` -> `a000_000000`, the base idle. Every creature has it: it is the bottom of the
/// 0-999 idle band that the animation-id banding convention reserves, and the graph's `%04d`
/// formatting spells zero as four digits rather than as a special case.
const IDLE_ANIMATION: i32 = 0;

/// The body'S render scale while somebody else is being worn, and what it was before.
///
/// `None` on a [`Possessing`] means the body was left at its own size -- either the two heights
/// did not read, or the creature is close enough to player-sized that scaling would be a write
/// with nothing to show for it. Both are ordinary; see [`body_size`].
#[derive(Clone, Copy)]
struct BodyScale {
    /// What `ChrCtrl+0x2d4..0x2dc` held before the possession. Restored verbatim, rather than
    /// assumed to be `1.0`, so a body another mod had already scaled goes back to its size and not
    /// to ours.
    original: [f32; 3],
    /// ...and what it is set to while the possession lasts: the same vector with `Y` multiplied.
    worn: [f32; 3],
}

/// A creature this mod created, carried by whatever is wearing it so the teardown knows there is
/// something to give back.
///
/// `None` on a `Possessing` means the map placed the character and removing it is not ours to do.
#[derive(Clone, Copy)]
struct SpawnedBody {
    spawned: spawn_game::Spawned,
    /// `[spawn].despawn_on_release`. `false` leaves the creature standing when the possession
    /// ends, which is a real choice and not a failure -- the log says which it was.
    despawn_on_release: bool,
}

/// A spawn that has happened and is not yet drivable.
///
/// A state of its own rather than a flag on [`Possessing`], because almost nothing that is true of
/// a possession is true here: there is no thunk, no camera override, no neutered body, and the
/// creature cannot be driven or even read through for most of it. The one thing it shares is that
/// the hotkey has been pressed and the player is waiting.
struct Pending {
    spawned: spawn_game::Spawned,
    player: Chr,
    /// The request that started it, replayed into [`NpcPossessionEngine::enter`] once ready.
    request: PossessionRequest,
    readiness: Readiness,
    /// `layout::ene_dat_cap_offsets` for the running build, resolved once at spawn rather than per
    /// frame. `None` makes the asset gate undecidable, which the readiness machine skips.
    caps: Option<(usize, usize)>,
    /// Where the creature is put the instant it becomes placeable, at the configured distance.
    /// Captured at spawn, from the player's position then -- so a player who walks away while it
    /// loads does not drag the spawn point with them.
    ///
    /// The fallback rather than the answer: [`Self::place_from`] and
    /// [`Self::configured_distance_m`] are what the real placement is recomputed from once the
    /// creature's own capsule can be read. This point is used when it cannot.
    place_at: [f32; 3],
    /// The player's position at the press, so the placement can be re-derived at a different
    /// distance without moving the spawn point to wherever the player has since walked.
    place_from: [f32; 3],
    /// `[spawn].distance_m`, kept because it is now a floor rather than the whole answer. See
    /// [`crate::spawn::placement`].
    configured_distance_m: f32,
    place_yaw: f32,
    started: std::time::Instant,
    /// The last gate a line was written about, so progress is logged on change rather than sixty
    /// times a second.
    reported: Option<Gate>,
    despawn_on_release: bool,
    chr_id: u32,
}

/// One possession in flight.
struct Possessing {
    creature: Chr,
    player: Chr,
    /// Set only when this mod created the creature; see [`SpawnedBody`].
    spawned: Option<SpawnedBody>,
    /// The lock-on anchor, expressed as the only thing that moves it; see [`BodyScale`] and
    /// [`body_size`]. `None` means the body was left at its own size.
    body_scale: Option<BodyScale>,
    /// Kept alive for exactly as long as `ChrCtrl+0x3b0` points at it. Dropping it frees the page,
    /// so it must outlive the clear -- which is why teardown owns this whole struct and drops it
    /// only after [`Step::RestoreManipulatorVtable`] has run.
    thunk: Thunk,
    /// `ChrIns.debugFlags` for the running build, or `None` on a build nobody has measured -- in
    /// which case the no-attack neuter is skipped rather than written eight bytes off target.
    debug_flags_offset: Option<usize>,
    release_on_death: bool,
    /// The creature'S position, CACHED every frame.
    ///
    /// Death must never need a late read. `WorldChrManImp::RemoveChrIns` nulls the camera override
    /// during removal, so reading the corpse's position at that point is a use-after-free race.
    /// This is read while the creature is definitely alive and used afterwards.
    last_position: [f32; 3],
    /// Likewise, and the fallback when the creature dies airborne.
    last_grounded: Option<[f32; 3]>,
    /// The frame on which the movement telemetry line was last written, so a held key does not
    /// produce sixty log lines a second. A frame count rather than an `Instant` because the
    /// no-timeouts gate bans wall-clock `elapsed()` control flow, and because the frame loop is
    /// the clock the thing being measured actually runs on. `None` until the first line.
    last_movement_log: Option<u64>,
    /// Was the creature on solid ground at the last good read?
    last_on_ground: bool,
    frames: u64,
    /// The MOVESET, or `None` when this creature is not in the shipped table.
    ///
    /// `None` is a real and ordinary state -- 20 of the 408 creatures the generator looked at have
    /// nothing fireable of their own -- so it is an `Option` rather than an empty dispatcher that
    /// would silently swallow every press. (It said 163 until the TimeAct join was fixed; the other
    /// 143 were creatures whose animations the generator was reading under the wrong chr id.)
    moveset: Option<Dispatcher>,
    /// Rising-edge detector for the four face inputs.
    face: FaceEdges,
    /// The two attack-set page keys, as rising edges. Same latch as [`Self::face`]; see
    /// [`crate::input::read_page_inputs`].
    pages: FaceEdges,
    /// The camera, sized to this creature. Present even when nothing was adapted, because it also
    /// carries the reason -- see [`crate::camera`].
    camera: camera::Session,
    watchdog: Watchdog,
    /// Milliseconds since the possession started. The clock the combo window and the watchdog
    /// both measure against.
    elapsed_ms: u64,
    started: std::time::Instant,
    /// `NpcParam` id / 10000 -- `4500` for a Flying Dragon. The table's key, and what the derived
    /// file is written under. Captured once at possession start, because the creature could
    /// despawn before it is next needed.
    chr_id: u32,
    /// Whether a "this button has nothing to fire" line has already been logged, per input. The
    /// log opens and closes the file per line, so an unguarded one on a held button would cost
    /// sixty opens a second.
    reported_dead_input: [bool; 4],
    /// The last availability state reported, so the log carries one line per change.
    reported_availability: Option<(Availability, Source)>,
    /// Has the "a movement move fired" note been said this possession?
    reported_movement_fire: bool,
    /// Has the "your press is waiting rather than cancelling" line been said this possession?
    ///
    /// Once, not per press, for the same reason as [`Self::reported_dead_input`]: the log opens
    /// and closes the file per line, and this is the state a player mashing through a long
    /// animation is in on every one of sixty frames a second.
    reported_buffered_press: bool,
    /// Whether the "this hand has one page" line has been said, per hand.
    reported_one_page: [bool; 2],
    /// What team the creature was on before it was worn, so release puts it back on it.
    ///
    /// `None` means the byte did not read and nothing was written, which is also what stops the
    /// release from writing a guess over a live field. See [`game::Chr::set_team_type`].
    original_team: Option<u8>,
    /// The frame the aim telemetry line was last written on. Same throttle and same reason as
    /// [`Self::last_movement_log`], but not gated on an input: aiming is the thing you do while
    /// standing still, so a line that only appeared while a key was held would be silent on
    /// exactly the case the user reported.
    last_aim_log: Option<u64>,
    /// Who else is in the session, and whether their HP is moving.
    ///
    /// The user reported "I can't damage other players ... neither HP bar goes down", and the
    /// answer is a table in the binary rather than anything this crate does -- see
    /// [`netdamage`] for the trace. This carries the verdict into the log next to an observed HP
    /// series, so the claim is falsifiable rather than asserted.
    net: netdamage::Ledger,
}

impl Possessing {
    /// Where the player should be standing once this is over.
    ///
    /// No sphere cast: the creature's own `standingOnSolidGround` answers whether its last
    /// position already qualifies, and `lastGroundedPosition` is a point it demonstrably stood on
    /// when it does not. A cast would be better under a flying dragon and is the one thing here
    /// that needs a game address, so it is left out of this layer deliberately.
    fn release_point(&self) -> [f32; 3] {
        if self.last_on_ground {
            return self.last_position;
        }
        self.last_grounded.unwrap_or(self.last_position)
    }
}

/// The real engine. One possession at a time, by construction.
#[derive(Default)]
pub(crate) struct NpcPossessionEngine {
    active: Option<Possessing>,
    /// A spawn waiting to become drivable. Mutually exclusive with [`Self::active`]: the pending
    /// one becomes the active one, and nothing else can start while either is set.
    pending: Option<Pending>,
}

impl NpcPossessionEngine {
    pub(crate) const fn new() -> Self {
        Self {
            active: None,
            pending: None,
        }
    }

    /// Everything that has to be true of the player's body while somebody else is being worn.
    ///
    /// Re-applied every frame rather than set once, and each of the four has its own reason:
    /// the alpha has a per-frame decay modifier the engine drives, the mute is cleared by four
    /// (re)spawn/teleport paths, `debugFlags` is cheap enough that checking whether it drifted
    /// costs more than writing it, and `lastGroundedPosition` has to track the body every frame or
    /// it is not tracking it at all.
    ///
    /// **the INVINCIBILITY bit covers more than this comment used to say, and that is the
    /// correction it now carries.** `layout::chr_ins::INVINCIBLE` was re-verified on 1.17 rather
    /// than assumed -- `ChrIns::IsImmuneToAttack` at `0x1403f3dc0` is `TESTB $0x10,0x1c5(%r9)`, the
    /// same instruction 1.16.2 has at `0x1403f3c76`, and `+0x1c5` sits below the `+0x3b8` insertion
    /// that grew `ChrIns` on this build, so it could not have moved. This block used to claim the
    /// bit gates hit resolution "and nothing else", and that the fall path never asks whether the
    /// victim is immune. It does: `CSChrFallModule::Update`'s damage block is gated on
    /// `FUN_14044e730`, which calls exactly that vtable slot and returns false when it answers yes
    /// (`0x14044e866`; the byte trace is in [`crate::possess::fall`]). So a possessed body cannot
    /// be charged for a fall at all while the bit is on.
    ///
    /// The `lastGroundedPosition` write below is therefore not what keeps the body alive during a
    /// possession -- it is what keeps the field honest for the frames after one, when the bit comes
    /// off and the engine starts believing that number again. It is written every frame, and
    /// CLAMPED to the body's own height rather than the creature's, so the difference the engine
    /// subtracts can never come out positive at any subject size.
    ///
    /// **the INVINCIBILITY is also why the possessed creature'S grabs never land, and that is
    /// kept deliberately.** `layout::chr_ins::INVINCIBLE` carries the trace: every route into the
    /// throw system is behind `IsImmuneToAttack`, which reads exactly this bit, and the only
    /// legal victim for 189 of the 190 creature `ThrowParam` rows is the player's own body. The
    /// two shapes the fix could take were both checked and both refused. Dropping the bit for a
    /// grab window makes the body damageable by everything for that window and lets the creature
    /// the player is wearing throw, hurt and kill them -- a state this crate's teardown does not
    /// model, since `release_on_death` watches the creature. Keeping it unhittable by everything
    /// except the possessed creature is not expressible: the predicate has no per-attacker
    /// exemption, and the game's one throw entry that skips it (`RequestThrow_AllChr`, driven
    /// from `ChrCtrl` for a non-default manipulator) builds its `ThrowData` with `throwTypeId`
    /// zero, so it can only ever match a backstab/riposte row and never a creature grab.
    ///
    /// The co-location is an independent second refusal even if the bit were dropped:
    /// [`Self::tick_active`]'s `request_move` keeps the body at the creature's own root, so the
    /// attacker-to-defender vector `ThrowPoseChecks` builds is ~zero, `NormalizeVector` answers
    /// the zero vector, and the `DiffAngMyToDef` cone test compares a dot product of zero against
    /// `cos(angle)` -- failing for every row under 90 degrees.
    fn neuter(state: &Possessing, first_frame: bool) {
        state.player.set_invincible(true);
        state.player.set_alpha(ALPHA_INVISIBLE);
        // `stop_playing` is a one-shot the engine consumes, so it is asked for only once.
        state.player.set_muted(true, first_frame);
        if let Some(offset) = state.debug_flags_offset {
            state.player.set_no_attack(offset, true);
        }
        // The lock-on anchor. Re-asserted with the rest rather than written once at possession
        // start, for the cheapest of reasons: nothing in the game recomputes `scaleSize` (its only
        // writers are the `ChrCtrl` constructor and `SetScaleSize`, whose one caller is character
        // construction), so this is insurance against a respawn rebuilding the body mid-possession
        // rather than a fight with a per-frame writer. Three float stores and their mirror.
        if let Some(scale) = state.body_scale {
            state.player.set_body_scale(scale.worn);
        }
        // The co-location target for this frame, which is where the body is about to be put, and
        // the one line that says what was written and what the body's own grounding read as.
        Self::pin_and_report(state, "carried", state.last_position);
    }

    /// Pin the body's fall bookkeeping to a point, and say what was written and what the body'S
    /// grounding was.
    ///
    /// # Why this line exists
    ///
    /// The only account this engine could previously give of a player who died mid-possession was
    /// the player's own: "I got sent through the floor and died." Every field the engine's fall
    /// path actually reads was invisible. The creature's position was logged; the body's was not,
    /// and the body is the thing that dies.
    ///
    /// So this reports, in order: the point the co-location is writing, the point the body is
    /// actually at, the gap between them, the `lastGroundedPosition` that was there before the pin
    /// and the one written after it, and `lastGroundedPosition.y - position.y` -- the exact
    /// subtraction `CS::CSChrFallModule::GetFallDeathTime` makes and the number
    /// `CSChrFallModule::Update` charges the body for on the frame it lands.
    ///
    /// # Two lines, throttled differently
    ///
    /// The routine line is one a second (see [`fall::colocation_line_due`]). The alarm -- the body
    /// is further below the co-location target than its own `maxStepHeight` -- is not throttled at
    /// all, because it is rare by construction and the frame it happens on is the frame worth
    /// having.
    fn pin_and_report(state: &Possessing, phase: &str, target: [f32; 3]) {
        // Before the pin, so the line can show what the field held on the way in. One pointer walk
        // for everything the fall path reads.
        let before = state.player.fall_state();
        let pinned = state.player.pin_last_grounded(target);
        let alarm_m = fall::drift_alarm_m(before.max_step_height);
        let drift = fall::drift_below_m(target[1], before.body_y(), alarm_m);

        if let Some(below) = drift {
            possess_log(format_args!(
                "colocation: THE BODY IS {below:.2} m BELOW WHERE IT WAS PUT ({phase}) -- target \
                 {target}, body {body}. Its own step height is {alarm_m:.2} m, so this is not a \
                 step down: the co-location teleport was resolved somewhere else by the body's own \
                 character proxy, which `ChrCtrl::updatePos` integrates through \
                 `CSChrPhysicsModule::doUpdates` immediately AFTER draining our write, and which \
                 still carries a full-height capsule (half-height {half} m) whatever the render \
                 scale on `ChrCtrl+0x2d8` says. `lastGroundedPosition` is pinned to the BODY's \
                 height rather than the target's, so `CSChrFallModule` cannot charge this as a \
                 fall -- see crates/er-npc-possess/src/possess/fall.rs for the byte proof of that \
                 gate",
                target = describe_point(Some(target)),
                body = describe_point(before.position),
                half = describe_metres(before.capsule_half_height),
            ));
        } else if !fall::colocation_line_due() {
            return;
        }

        let after = state.player.fall_state();
        possess_log(format_args!(
            "colocation: {phase} -- wrote lastGroundedPosition <- {after_grounded} (target \
             {target}, was {before_grounded}, write={pinned}) | body at {body} | fall charge \
             lastGrounded.y - position.y = {charge} m before, {charge_after} m after. That \
             subtraction IS `CS::CSChrFallModule::GetFallDeathTime` (1.16.2 0x14044dce0, the SUBSS \
             at 0x14044dd1f); `CSChrFallModule::Update` evaluates it only on a frame where \
             standingOnSolidGround is set, and charges hpMax * ratio(it) through \
             `CSChrDataModule::ChangeHP` with deathType=Fall. Positive means the engine believes \
             the body fell that far, and the pin holds it at or below zero at every subject size, \
             so this should read <= 0.00 whatever the creature is | onGround={ground} \
             touching={touching} falling={falling} | maxStep={step} m capsuleHalf={half} m \
             (UNSCALED -- the lock-on anchor's body scale writes `ChrCtrl+0x2d4..0x2dc` and the \
             `CSChrDataModule+0x54` mirror and touches no physics field, so a body worn at 0.47x \
             still collides at full height)",
            target = describe_point(Some(target)),
            before_grounded = describe_point(before.last_grounded),
            after_grounded = describe_point(after.last_grounded),
            body = describe_point(after.position),
            charge = describe_metres(before.charge_m()),
            charge_after = describe_metres(after.charge_m()),
            ground = describe_flag(after.on_ground),
            touching = describe_flag(after.touching_ground),
            falling = describe_flag(after.falling),
            step = describe_metres(after.max_step_height),
            half = describe_metres(after.capsule_half_height),
        ));
    }

    /// Undo [`Self::neuter`]. Returns whether every part of it took.
    fn restore(state: &Possessing) -> bool {
        let invincible = state.player.set_invincible(false);
        let alpha = state.player.set_alpha(ALPHA_OPAQUE);
        let muted = state.player.set_muted(false, false);
        let flags = state
            .debug_flags_offset
            .is_none_or(|offset| state.player.set_no_attack(offset, false));
        // Back to whatever it was, which is not necessarily 1.0 -- see [`BodyScale::original`].
        let scale = state
            .body_scale
            .is_none_or(|scale| state.player.set_body_scale(scale.original));
        invincible && alpha && muted && flags && scale
    }

    /// Cancel a spawn that never became drivable, or that the player pressed the key out of.
    ///
    /// A separate path from [`Self::release_with`] on purpose: none of the six possession steps
    /// applies -- nothing was neutered, no camera was moved, no override was installed -- and
    /// running them would report six successes for work that was never done. The only thing owed is
    /// the creature.
    fn cancel_pending(pending: Pending, reason: Reason, despawn: bool) -> bool {
        let removed = if despawn && pending.despawn_on_release {
            spawn_game::despawn(pending.spawned.creature)
        } else {
            false
        };
        possess_log(format_args!(
            "spawn: cancelled c{:04} in roster slot {} after {} ms (reason={}) -- creature {}",
            pending.chr_id,
            pending.spawned.slot,
            pending.started.elapsed().as_millis(),
            reason.name(),
            if removed {
                "removed"
            } else if !despawn {
                "LEFT IN THE WORLD; RemoveChrIns is a game call and this is not the game thread"
            } else if pending.despawn_on_release {
                "COULD NOT BE REMOVED and is still in the world"
            } else {
                "left standing, as [spawn].despawn_on_release asks"
            }
        ));
        removed
    }

    /// Run the release, in order, whatever fails. See [`teardown`].
    fn release_with(&mut self, reason: Reason) -> PossessionOutcome {
        // A press that lands while a spawn is still coming up cancels it. Handled before the
        // active state because the two are mutually exclusive and this one has its own teardown.
        if let Some(pending) = self.pending.take() {
            // A HOTKEY press inside the readiness window is ignored, not treated as a cancel.
            // A spawn takes up to `[spawn].readiness_ms` to become drivable and nothing on screen
            // says one is in flight, so a player who taps twice -- or holds the key a beat too
            // long -- used to create a creature and delete it 166 ms later, which is
            // indistinguishable from the key doing nothing at all. Measured live 2026-09-02:
            // `spawn: cancelled c2110 in roster slot 6 after 166 ms (reason=hotkey)`. The
            // deadline is what ends a spawn that will never arrive; a second press is not.
            // Shutdown still cancels, because there is no later frame to arrive in.
            if reason != Reason::Shutdown {
                possess_log(format_args!(
                    "possess-hotkey: IGNORED -- a spawn has been coming up for {} ms. A press \
                     while one is in flight is not a cancel; wait for it, or let \
                     [spawn].readiness_ms end it",
                    pending.started.elapsed().as_millis(),
                ));
                self.pending = Some(pending);
                return PossessionOutcome::Accepted;
            }
            Self::cancel_pending(pending, reason, false);
            return PossessionOutcome::Accepted;
        }
        // `mut` because the camera restore below mutates the saved state as it unwinds it.
        let Some(mut state) = self.active.take() else {
            // Releasing when nothing is possessed must be safe and quiet: the shutdown path calls
            // it unconditionally, and so does the state machine after a refusal.
            return PossessionOutcome::Accepted;
        };
        // The panel comes down here, ahead of the teardown and outside [`Step`]. Every way a
        // possession can end -- the hotkey, death, despawn, a config reload, shutdown -- passes
        // through this function after `active.take()` succeeded, so this is the one place that
        // covers all of them. It is not a `Step` because the steps exist to pin an order between
        // things that read through the creature, and this reads through nothing: it stores `None`
        // behind a mutex and clears an atomic. Leaving it up would put a panel describing four
        // buttons on screen after those buttons stopped doing anything, which is worse than no
        // panel.
        banner::clear();
        // The net-damage watch closes here, beside the banner and for the same reason: every way
        // a possession can end passes through this point, and a ledger that only reported while
        // it was running would leave the question "did anybody's HP move" unanswered exactly when
        // the player has stopped and gone to read the log.
        possess_log(format_args!("{}", state.net.summary()));
        let mut run = Teardown::new(reason);
        let (swizzled_com, original_vptr, patched_vptr) = (
            state.thunk.real_com(),
            state.thunk.original_vptr(),
            state.thunk.vtable_address(),
        );
        let release_point = state.release_point();
        run.run(|step| match step {
            // Infallible -- one atomic store -- so this always reports success. It is a step
            // rather than a line at the top of this function because the order is the thing the
            // state machine exists to enforce: every step below reads through the creature.
            Step::StopHudRetarget => {
                hud::stop();
                true
            }
            Step::RestoreBody => Self::restore(&state),
            // Nothing to undo: the per-frame driver stops the moment `active` is `None`, which
            // happened at the `take()` above. The step exists so the order is a thing the state
            // machine enforces rather than a comment.
            Step::StopColocating => true,
            // Pinned before the move, not after: this is the last teleport the body takes and it
            // is the one most likely to be downward -- `release_point` falls back to the creature's
            // `lastGroundedPosition` when it died airborne, which is by construction below where
            // the body has been riding. Without this the very act of giving the body back reads to
            // `CSChrFallModule` as a fall of exactly that height. See
            // [`game::Chr::pin_last_grounded`].
            Step::MovePlayer => {
                // The same pin as every carried frame, and reported the same way -- this is the
                // teleport most likely to be downward and the last one taken while the body is
                // still immune, so it is the one whose numbers a post-mortem needs.
                Self::pin_and_report(&state, "released", release_point);
                state.player.request_move(release_point, None)
            }
            Step::RestoreCameraSize => state.camera.restore(),
            Step::ClearCameraOverride => game::set_camera_override(None),
            // The AI move request is cancelled before the vtable goes back, because after it the
            // creature is the AI's again and our last order is a run command it did not issue.
            // `stop_move_intent` writes exactly what `CS::AiIns::ClearMoveRequest` writes, so what
            // the AI wakes up to is a state its own code produces. Its failure is not this step's
            // failure: the restore is the step that must happen -- our patched table lives in a
            // page that is about to be freed -- and a creature that jogs for one frame is not a
            // reason to report the release broken.
            Step::RestoreManipulatorVtable => {
                // The team goes back here, with the AI, because they are the same handover: a
                // creature whose goal selection is live again must be on the team its own think
                // logic was written for. Its result is deliberately not folded into this step's
                // verdict -- for the same reason `stop_move_intent`'s is not, and one step further
                // along: a creature left on `Charmed` is a mis-aggroed NPC, while a creature left
                // dispatching through a freed vtable is a crash, and only the second is what
                // `is_critical` means.
                if let Some(team) = state
                    .original_team
                    .filter(|team| !state.creature.set_team_type(*team))
                {
                    possess_log(format_args!(
                        "lock-on: the creature's teamType could NOT be put back to {team} -- it \
                         stays on {}, which leaves it fighting its own side until the next map \
                         load",
                        chr_ins::TEAM_TYPE_CHARMED
                    ));
                }
                state.creature.stop_move_intent();
                game::Chr::restore_manipulator_vtable(swizzled_com, original_vptr, patched_vptr)
            }
            // After the override clear, always; see `Step::DespawnCreature`, whose discriminant is
            // that ordering and whose test fails if anyone moves it.
            Step::DespawnCreature => match state.spawned {
                // The map placed this character. Removing it is not ours to do.
                None => true,
                // The player asked for it to stay. Nothing failed.
                Some(body) if !body.despawn_on_release => true,
                // The game already removed it, which is the only thing `CreatureGone` can mean:
                // `Chr::is_live` fails when `ChrCtrl.owner` no longer points back at the `ChrIns`,
                // and that is a destroyed character rather than a transient read. `RemoveChrIns`
                // hands its argument to `CSDelayDeleteMan`, so calling it on a character the game
                // has already queued for destruction queues a freed `ChrIns` for a second one --
                // and `spawn_game::despawn`'s own guard cannot catch it, because it proves the
                // first qword is mapped and freed heap still is.
                //
                // `crate::spawn::readiness::Poll::Vanished` already refuses this on the pending
                // path with the same reasoning; the active path did not, and a release whose whole
                // reason is "the creature is gone" is exactly where it bites. This is the release
                // that ran, and did not finish, in the reference log.
                Some(body) if reason == Reason::CreatureGone => {
                    possess_log(format_args!(
                        "spawn: the creature in roster slot {} was removed by the GAME, so \
                         RemoveChrIns is NOT being called on it -- a second removal would queue an \
                         already-freed ChrIns for a second destruction",
                        body.spawned.slot
                    ));
                    true
                }
                // `DLL_PROCESS_DETACH` runs on a thread that is not the game thread, under the
                // loader lock, with the other threads possibly already gone. `RemoveChrIns` walks
                // four singletons, calls a virtual on the character and DLPanics if any of them is
                // missing -- so it is refused here, and an orphaned NPC is accepted instead.
                //
                // That is a real, deliberate restriction and not an oversight: the creature is an
                // ordinary `EnemyIns` in the buddy roster with its AI already given back by the
                // step above, and the next map load removes it. A crash inside `DllMain` would not
                // be survivable and would be attributed to whatever unloaded us.
                Some(body) => {
                    if reason == Reason::Shutdown {
                        possess_log(format_args!(
                            "spawn: the creature in roster slot {} is being LEFT IN THE WORLD -- \
                             RemoveChrIns is a game call and DLL_PROCESS_DETACH is not the game \
                             thread. It has its own AI back and the next map load clears it",
                            body.spawned.slot
                        ));
                        true
                    } else {
                        spawn_game::despawn(body.spawned.creature)
                    }
                }
            },
            Step::LiftSaveSuppression => true,
        });
        possess_log(format_args!("{}", run.line()));
        if run.has_critical_failure() {
            // The page stays mapped. `ChrCtrl+0x3b0` still points into it, and freeing memory the
            // game is about to dispatch through turns a DLPanic into an arbitrary jump. Leaking
            // one page is the cheaper of the two.
            core::mem::forget(state.thunk);
            return PossessionOutcome::Refused(
                "the manipulator override could not be cleared".to_owned(),
            );
        }
        // Everything else has happened, so the page can go.
        drop(state);
        PossessionOutcome::Accepted
    }

    /// Look the creature up in the shipped table and apply the player's `[chr.cNNNN]` corrections.
    ///
    /// The order is deliberate: `unusable` is applied before `usable`, so a player who lists the
    /// same animation in both gets it -- the more specific instruction ("offer this") wins over
    /// the broader one, rather than the answer depending on which line they typed first.
    fn build_moveset(chr_id: u32, request: &PossessionRequest) -> Option<Dispatcher> {
        let mut moveset = moveset_table::lookup(chr_id)?;
        let overrides = crate::config::chr_override(chr_id);
        if let Some(over) = overrides.as_ref() {
            for animation in &over.unusable {
                moveset.deny(*animation, Denial::UnusableAtRuntime);
            }
            for animation in &over.usable {
                moveset.admit(*animation);
            }
        }
        if moveset.is_empty() {
            // Everything this creature had was denied -- by the table, by the player, or both.
            // A dispatcher over nothing would answer every press with `NoMoveset`, which is the
            // same outcome with more moving parts.
            return None;
        }
        let mut dispatcher = Dispatcher::new(moveset, request.mapping, request.buttons);
        for (name, animation) in overrides.iter().flat_map(|over| over.pin.iter()) {
            // Case-insensitive: the config file spells these lowercase and so does `Input::name`,
            // but `R2 = 3046` is the obvious typo and rejecting it teaches nothing.
            match Input::ALL
                .iter()
                .find(|input| name.eq_ignore_ascii_case(input.name()))
            {
                Some(input) => dispatcher.pin(*input, *animation),
                None => possess_log(format_args!(
                    "moveset: [chr.c{chr_id:04}] pin names input \"{name}\", which is not one of \
                     r1/r2/l1/l2 -- ignored"
                )),
            }
        }
        Some(dispatcher)
    }

    /// One frame of the moveset: watch what is playing, then act on what was pressed.
    ///
    /// Order matters and is not arbitrary. The watchdog runs first, so a press arriving on the
    /// same frame a stuck animation is detected does not queue behind it -- the forced idle wins
    /// and the press is spent on a frame that could not have fired anyway. Firing first would
    /// leave the new request overwritten by the idle a line later, which reads to the player as a
    /// button that sometimes does nothing.
    ///
    /// `moving` is whether this frame actually asked the creature to move -- the gait written into
    /// `AiIns.walkType`, not whether a stick was touched. The two differ on exactly the frames
    /// where the request was refused (an unreadable heading, a junk `speed_scale`), and on those
    /// the body is standing still, so offering it a running attack would be wrong.
    fn tick_moveset(state: &mut Possessing, moving: bool) {
        let Some(dispatcher) = state.moveset.as_mut() else {
            return;
        };
        // The page keys are handled first and outside every early return below. Turning a page
        // fires nothing, so it is not subject to the one-request-per-frame rule, to what the
        // creature is currently playing, or to the "nothing pressed" shortcut -- a player who taps
        // right arrow mid-swing means it for the next press, and dropping it there would make the
        // key feel unreliable in exactly the moment it is most useful.
        //
        // One pad read answers both questions -- see [`crate::input::read_moveset_inputs`] -- so
        // the face levels are sampled here and spent a few lines below rather than re-read.
        let (face_held, page_held) = crate::input::read_moveset_inputs(dispatcher.buttons());
        let page_pressed = state.pages.feed(page_held);
        for hand in Hand::ALL {
            if page_pressed & (1 << hand.index()) == 0 {
                continue;
            }
            match dispatcher.turn_page(hand) {
                PageTurn::Turned { page, pages } => {
                    let leads = |input: Input| match dispatcher.leads_with(input) {
                        Some(entry) => format!("{} {}", input.name(), entry.fire),
                        None => format!("{} nothing", input.name()),
                    };
                    let [first, second] = hand.inputs();
                    possess_log(format_args!(
                        "moveset: {} attack set {page}/{pages} -- {}, {}",
                        hand.name(),
                        leads(first),
                        leads(second),
                    ));
                    // On screen, and this is the half the player actually sees. The two writes
                    // below are both "say what the buttons do now" and they differ only in where
                    // a player can read them from: the derived file needs an alt-tab, the banner
                    // is in the corner of the game they are looking at. Publishing the hand that
                    // moved makes the panel highlight it, which is what turns "the sets changed"
                    // from an inference into an observation.
                    Self::publish_banner(state.chr_id, dispatcher, Some(hand));
                    // The report is rewritten so the page the player is on, and what is on it, can
                    // be read rather than remembered. Cheap: a few kilobytes, only on a key press.
                    Self::write_derived(
                        state.chr_id,
                        &state.camera,
                        state.creature,
                        Some(dispatcher),
                    );
                }
                PageTurn::OnePage => {
                    // Once per hand per possession. The log opens and closes the file per line and
                    // a player who has discovered the key does press it more than once.
                    if !state.reported_one_page[hand.index()] {
                        state.reported_one_page[hand.index()] = true;
                        possess_log(format_args!(
                            "moveset: c{:04} has only one {} attack set, so that page key does \
                             nothing here",
                            state.chr_id,
                            hand.name(),
                        ));
                    }
                }
            }
        }

        let pressed = state.face.feed(face_held);
        // Consumed, not held, and now the engine's answer rather than ours. See
        // [`Sample::input_consumed`]: a press that fired nothing is evidence of being stuck, not
        // evidence against it, so a player mashing at a softlock must not be able to hold the
        // watchdog off indefinitely. This used to be "we wrote the request field last frame",
        // which is not the same claim -- `CSChrEventModule::Update` has four gates in front of
        // the dispatch and drops a refused request silently. `+0x24` bit 0 is what it sets when
        // the request really did become a graph event.
        let consumed = state.creature.animation_request_dispatched();
        // What the creature is doing, and whether it is willing to be left. Read once per frame
        // and shared by the release and the press, so a buffered press and a fresh one on the
        // same frame cannot disagree about what is playing. `None` means the creature is
        // animating nothing -- see `Chr::current_anim_frame`, which refuses to hand back a stale
        // queue entry.
        let frame = state.creature.current_anim_frame();
        let playing = frame.map(|frame| Playing {
            animation: frame.animation,
            elapsed_s: frame.local_time,
            length_s: frame.anim_length,
            // Filled in by `Dispatcher::reading`, which owns the table this is asked against.
            is_known_move: false,
            cancel_allowed: state.creature.attack_cancel_allowed(),
        });
        let reading = dispatcher.reading(playing);
        Self::report_availability(&mut state.reported_availability, frame, playing, reading);
        // `mut` because a release that fires spends the frame's one request slot, which makes
        // anything else pressed this frame mid-animation by definition -- see below.
        let mut availability = reading.availability;

        {
            // The watchdog runs on every frame, including the ones with nothing playing. That is
            // what makes `ReturnedToNeutral` fire when a creature stops animating rather than only
            // when it animates something neutral -- and a cursor that never resets is a combo that
            // never starts again.
            let sample = Sample {
                animation: frame.map_or(IDLE_ANIMATION, |frame| frame.animation),
                local_time: frame.and_then(|frame| frame.local_time),
                // The same membership question the chain gate asked, and the same answer -- both
                // layers have to agree on whether the creature is doing something of ours, or
                // the watchdog arms on an idle loop while the gate calls it idle.
                is_known_move: reading.source != Source::NotOurMove,
                input_consumed: consumed,
                now_ms: state.elapsed_ms,
            };
            match state.watchdog.observe(sample) {
                Verdict::Fine => {}
                Verdict::ReturnedToNeutral => dispatcher.on_neutral(),
                Verdict::ForceIdle { idle, blame } => {
                    state.creature.request_animation(idle);
                    dispatcher
                        .moveset_mut()
                        .deny(blame, Denial::UnusableAtRuntime);
                    // A press queued behind an animation that has just been declared unusable is
                    // stale: it was aimed at continuing a chain that turned out to be a softlock.
                    // Dropped explicitly, because `on_neutral` deliberately does not reset while
                    // something is waiting.
                    dispatcher.forget_buffered_press();
                    dispatcher.on_neutral();
                    // The number is the possession clock, not the stuck duration, and the wording
                    // used to say the opposite -- "stuck ... for {elapsed_ms} ms" reported a
                    // 29,701 ms hang on a watchdog whose threshold is four seconds. The stuck
                    // duration lives in `Watchdog::armed.suspect_since_ms` and is not on
                    // `Verdict::ForceIdle`; until it is, this says what it actually knows. How
                    // long it was stuck is `[mapping].watchdog_seconds`, by construction.
                    possess_log(format_args!(
                        "moveset: animation {blame} stopped ADVANCING -- its playhead did not \
                         move for the full [mapping].watchdog_seconds while nothing was asked of \
                         it -- and was forced back to idle {} ms into this possession, denied for \
                         the rest of this session. It is in {} as unusable-at-runtime; copy it \
                         into er-npc-possess.toml under [chr.c{:04}] to keep it denied.",
                        state.elapsed_ms,
                        crate::config::DERIVED_CONFIG_FILE_NAME,
                        state.chr_id,
                    ));
                    Self::write_derived(
                        state.chr_id,
                        &state.camera,
                        state.creature,
                        Some(dispatcher),
                    );
                    return;
                }
            }
        }

        // Nothing pressed and nothing waiting: there is no work, and in particular no reason to
        // pay for the distance sweep a `Context` needs.
        if pressed == 0 && !dispatcher.is_holding() {
            return;
        }
        // One request per frame, and only into an empty slot. `requestAnimationId` holds a single
        // id which `CSChrEventModule::Update` consumes once per frame; a second write before that
        // runs discards the first silently. Two buttons in one frame therefore has to mean one
        // attack, not one attack lost.
        if state.creature.animation_request_pending() {
            return;
        }
        let context = Context {
            distance_m: game::nearest_hostile_distance(state.creature),
            locomotion: if moving {
                Locomotion::Moving
            } else {
                Locomotion::Neutral
            },
            now_ms: state.elapsed_ms,
        };

        // The waiting press goes first, and it has to. A press held through an attack is older
        // than anything arriving this frame, and the frame the animation lets go is exactly the
        // frame a fresh press would also be allowed -- so spending the slot on the new one would
        // silently eat the old one and give the player one attack for two presses.
        match dispatcher.release(context, availability) {
            Released::Nothing => {}
            Released::Fire(_, chosen) => {
                Self::fire_chosen(
                    state.creature,
                    &mut state.watchdog,
                    &mut state.reported_movement_fire,
                    chosen,
                );
                // The frame's one request slot is now spent on an attack that has just started,
                // so anything also pressed this frame is by definition arriving mid-animation.
                // Saying so rather than returning is what keeps it: it buffers instead of being
                // thrown away, and the chain carries on.
                availability = Availability::Committed;
            }
            Released::Expired(input) => possess_log(format_args!(
                "moveset: the {} press made while the creature was mid-attack waited out \
                 [mapping] input_buffer_ms and was dropped. Raise it if you want presses made \
                 early in a long animation to still arrive.",
                input.name(),
            )),
            Released::Empty(input, reason) => {
                Self::report_dead_input(&mut state.reported_dead_input, state.chr_id, input, reason)
            }
        }

        if pressed == 0 {
            return;
        }
        let Some(input) = Input::ALL
            .iter()
            .copied()
            .find(|input| pressed & (1 << Self::input_bit(*input)) != 0)
        else {
            return;
        };
        match dispatcher.press(input, context, availability) {
            Press::Fire(chosen) => Self::fire_chosen(
                state.creature,
                &mut state.watchdog,
                &mut state.reported_movement_fire,
                chosen,
            ),
            Press::Waiting(held) => {
                // Once per possession, not per press. The point is to tell a player who expected
                // a cancel that the press was kept rather than eaten; repeating it sixty times a
                // second would bury the log.
                if !state.reported_buffered_press {
                    state.reported_buffered_press = true;
                    possess_log(format_args!(
                        "moveset: {} landed while the creature was still committed to what it is \
                         playing, so it is waiting rather than cancelling it. It fires the moment \
                         the game says a chain is allowed, or when the animation ends -- \
                         whichever comes first. {}",
                        input.name(),
                        match held {
                            Held::Queued => "Nothing else was waiting.",
                            Held::Replaced => "It replaced an earlier press; only one is kept.",
                        },
                    ));
                }
            }
            Press::Nothing(reason) => {
                Self::report_dead_input(&mut state.reported_dead_input, state.chr_id, input, reason)
            }
        }
    }

    /// Say what the availability oracle read and which branch it took, once per change.
    ///
    /// This exists because the 2026-09-02 run could not be diagnosed from its own log. Four lines
    /// said presses were being held and dropped; nothing said what the resolver had looked at, so
    /// which of three possible causes it was had to be settled by reading the binary afterwards.
    /// A line per frame would be sixty file opens a second, and a line per possession would miss
    /// the transition that matters -- so it is a line per change of the resolved state, which for
    /// a creature being played is a handful per attack and exactly zero while nothing happens.
    ///
    /// Takes the one field it mutates rather than `&mut Possessing`, because the caller is
    /// holding a mutable borrow of the sibling `moveset` field.
    fn report_availability(
        reported: &mut Option<(Availability, Source)>,
        frame: Option<AnimFrame>,
        playing: Option<Playing>,
        reading: Reading,
    ) {
        let now = (reading.availability, reading.source);
        if *reported == Some(now) {
            return;
        }
        *reported = Some(now);
        let clock = |value: Option<f32>| match value {
            Some(value) => format!("{value:.3}"),
            None => "unread".to_owned(),
        };
        possess_log(format_args!(
            "moveset: availability={} via {} -- anim={} playhead={}/{} taeCancels={}",
            match reading.availability {
                Availability::Idle => "idle",
                Availability::Chainable => "chainable",
                Availability::Committed => "committed",
            },
            reading.source.name(),
            // "none" is the answer that matters most here: it is the engine saying this creature
            // drove no animation this frame, which is what "idle" is made of.
            frame.map_or("none".to_owned(), |frame| frame.animation.to_string()),
            clock(frame.and_then(|frame| frame.local_time)),
            clock(frame.and_then(|frame| frame.anim_length)),
            match playing.and_then(|playing| playing.cancel_allowed) {
                Some(true) => "cancel-allowed",
                Some(false) => "cancel-denied",
                None => "unread",
            },
        ));
    }

    /// Fire a chosen move and record that the frame spent its one request on it.
    ///
    /// Takes the three fields it touches rather than `&mut Possessing`, because every caller is
    /// holding a mutable borrow of the sibling `moveset` field while it calls this.
    fn fire_chosen(
        creature: Chr,
        watchdog: &mut Watchdog,
        reported_movement_fire: &mut bool,
        chosen: table::Move,
    ) {
        if !Self::fire(creature, chosen) {
            return;
        }
        watchdog.armed_with(chosen.fire);
        // A movement-bucket move that fires and goes nowhere looks like a dead button, and it is
        // a different fault from the one the availability gate can cause. `l2` is the creature's
        // own dodges and steps, and while the locomotion layer cannot translate the body, those
        // clips play in place: the press was honoured and the animation is running. Said once,
        // so the two failures are not mistaken for each other in a log.
        if chosen.bucket == Bucket::Movement && !*reported_movement_fire {
            *reported_movement_fire = true;
            possess_log(format_args!(
                "moveset: fired movement animation {} -- if the creature animated but did not \
                 travel, that is the locomotion layer and not this button being dead; the press \
                 was honoured and the clip is running",
                chosen.fire,
            ));
        }
    }

    /// Said once per possession per input rather than per press: a player holding a dead button
    /// would otherwise write the log file at sixty lines a second.
    fn report_dead_input(reported: &mut [bool; 4], chr_id: u32, input: Input, reason: NoMove) {
        if reported[Self::input_bit(input)] {
            return;
        }
        reported[Self::input_bit(input)] = true;
        possess_log(format_args!(
            "moveset: {} has nothing to fire on c{:04} ({}). See {} for what this creature has \
             and why the rest was withheld.",
            input.name(),
            chr_id,
            reason.explanation(),
            crate::config::DERIVED_CONFIG_FILE_NAME,
        ));
    }

    /// Fire one move, by whichever of the two paths its prefix says.
    ///
    /// The split is the point. `Prefix::Event` -- 4,667 of the shipped moves -- is a write to
    /// `CSChrEventModule+0x18` and resolves no game address, so it keeps working on a build nobody
    /// has mapped. Everything else builds the event name the graph actually declares and calls
    /// `PlayAnimationByBehaviorName`, which costs the one address this crate resolves. Preferring
    /// the field write is not an optimisation, it is what confines the address to the minority of
    /// moves that cannot be reached any other way.
    fn fire(creature: Chr, chosen: table::Move) -> bool {
        if chosen.prefix.is_field_write() {
            return creature.request_animation(chosen.fire);
        }
        // `%04d`, exactly: the graph's literals are `W_Step6000` and `W_Event0000`, minimum width
        // four, longer ids printed raw. A three-digit id spelled without the leading zero resolves
        // to nothing and no-ops silently, which is the failure this whole layer exists to avoid.
        let mut name: Vec<u16> = chosen.prefix.name().encode_utf16().collect();
        name.extend(format!("{:04}", chosen.fire).encode_utf16());
        name.push(0);
        creature.play_animation_by_name(&name)
    }

    const fn input_bit(input: Input) -> usize {
        match input {
            Input::R1 => 0,
            Input::R2 => 1,
            Input::L1 => 2,
            Input::L2 => 3,
        }
    }

    /// Put what the four buttons do onto the screen, for the render thread to pick up.
    ///
    /// The counterpart of [`Self::write_derived`] and deliberately called beside it every time:
    /// the derived file is the reference a player reads with the game paused or alt-tabbed, this
    /// is the one they read while playing. `flash` names the hand whose page key caused this
    /// publish, which is what the panel highlights for a couple of seconds.
    ///
    /// Called on possession start and on every page turn -- never per frame. The name lookup
    /// walks the 408-row catalogue and would be wasteful at 60 Hz; at a keypress it is free.
    fn publish_banner(chr_id: u32, dispatcher: &Dispatcher, flash: Option<Hand>) {
        banner::publish(banner::Banner::from_dispatcher(
            chr_id,
            crate::picker::catalog::name_of(chr_id),
            dispatcher,
            flash,
        ));
    }

    /// Write `er-npc-possess.derived.toml`.
    ///
    /// Best-effort by design: the game directory can be read-only, and a possession that works is
    /// worth more than a report about it. A failed write is silent because the only thing lost is
    /// a diagnostic, and a log line about failing to write a log-adjacent file is noise.
    ///
    /// The camera and HUD blocks are appended rather than interleaved, so each layer owns its own
    /// stretch of one file. `camera` is taken as a plain reference rather than reached through
    /// `state` because the moveset call site is holding a mutable borrow of a sibling field.
    fn write_derived(
        chr_id: u32,
        camera: &camera::Session,
        creature: Chr,
        dispatcher: Option<&Dispatcher>,
    ) {
        let mut text = match dispatcher {
            Some(dispatcher) => {
                let mut text = derived::render(chr_id, dispatcher.moveset(), &dispatcher.summary());
                text.push_str(&derived::pages_block(dispatcher));
                text
            }
            // Written anyway for a creature with no shipped moveset, because it still gets a
            // camera and still gets the HUD block -- and skipping the write would leave the
            // previous possession's file on disk, describing a different character, which is
            // worse than no report when its whole job is to explain numbers the player is
            // looking at right now. `render` already has a block for a moveset that is empty.
            None => derived::render(
                chr_id,
                &moveset_table::Moveset::default(),
                "no shipped moveset",
            ),
        };
        text.push_str(&camera.derived_block(chr_id));
        text.push_str(&Self::hud_block(chr_id, creature));
        let _ = std::fs::write(crate::config::DERIVED_CONFIG_FILE_NAME, text);
    }

    /// The `[hud]` block: which character the bars are reading, and why, for this creature.
    fn hud_block(chr_id: u32, creature: Chr) -> String {
        let decision = match hud::outcome().off_reason() {
            Some(off) => hud::Decision::Off(off),
            None if !crate::config::hud().enabled => hud::Decision::Off(hud::Off::Disabled),
            None => match hud::read_source(creature.address()) {
                Some(source) => hud::Decision::Driving(source),
                None => hud::Decision::Off(hud::Off::Unreadable),
            },
        };
        hud::render_derived(chr_id, &decision)
    }

    /// One frame of an active possession. Called from the FrameBegin task.
    fn tick_active(&mut self) {
        let Some(state) = self.active.as_mut() else {
            return;
        };
        // The LIVENESS check comes first. Everything below reads through the creature, and a
        // despawn between frames is ordinary rather than exceptional.
        if !state.creature.is_live() {
            possess_log(format_args!(
                "possession: the possessed character stopped resolving after {} frames",
                state.frames
            ));
            self.release_with(Reason::CreatureGone);
            return;
        }
        if state.release_on_death && state.creature.is_dead() {
            possess_log(format_args!(
                "possession: the possessed character died after {} frames",
                state.frames
            ));
            self.release_with(Reason::CreatureDied);
            return;
        }

        state.frames = state.frames.saturating_add(1);
        let first_frame = state.frames == 1;
        // A real clock rather than frames-times-sixteen: the combo window is specified in
        // milliseconds and the frame rate is not sixty. On a machine holding thirty, a
        // frame-counted window would be twice as long as the file says it is.
        state.elapsed_ms = u64::try_from(state.started.elapsed().as_millis()).unwrap_or(u64::MAX);

        // Cache before anything else. Death and despawn both make this unreadable, and the release
        // point has to come from a read taken while the creature was definitely alive.
        if let Some(position) = state.creature.position() {
            state.last_position = position;
        }
        if let Some(grounded) = state.creature.last_grounded_position() {
            state.last_grounded = Some(grounded);
        }
        if let Some(on_ground) = state.creature.standing_on_solid_ground() {
            state.last_on_ground = on_ground;
        }

        Self::neuter(state, first_frame);

        // Something else can null the camera override -- `RemoveChrIns` does it on despawn, which
        // the liveness check above has already ruled out for this frame. Re-asserting is one
        // compare in the common case and the difference between a working camera and a silently
        // detached one in every other.
        if !game::camera_override_is(state.creature) {
            game::set_camera_override(Some(state.creature));
        }
        // ...and the same for the team. One byte read, and a write only when something has put it
        // back: an aggro transition, a `TemporaryTeamType` slot arriving, or a respawn. Skipped
        // entirely when the original did not read at possession start, because then nothing was
        // written and there is nothing to hold.
        if state.original_team.is_some()
            && state.creature.team_type() != Some(chr_ins::TEAM_TYPE_CHARMED)
        {
            state.creature.set_team_type(chr_ins::TEAM_TYPE_CHARMED);
        }
        // ...and the same for the camera's size. Nothing in the game writes
        // `ChrExFollowCam+0x468`, but the constructor sets it to -1, so a camera rebuilt by a warp
        // or a map load mid-possession would quietly go back to framing a Tarnished.
        //
        // `refresh` first, because it may reinstall the whole patch: `[camera]` is live, so a
        // saved edit re-derives the row and re-points the override the same way a fresh possession
        // would. It costs one relaxed atomic load on the frames nothing has been edited.
        if state.camera.refresh() {
            possess_log(format_args!("{}", state.camera.log_line()));
            Self::write_derived(
                state.chr_id,
                &state.camera,
                state.creature,
                state.moveset.as_ref(),
            );
        }
        state.camera.reassert();

        // Point the HUD at the creature, every frame rather than once at possession start, so
        // that toggling `[hud] enabled` in the config file takes effect mid-possession like every
        // other live table. Two atomic loads and a store; the detour itself does the work.
        if crate::config::hud().enabled {
            hud::follow(state.creature.address());
        } else {
            hud::stop();
        }

        // Co-locate: the player's real body follows the creature, which is what keeps the net
        // position, the compass and the release point all correct with one mechanism.
        state
            .player
            .request_move(state.last_position, state.creature.yaw());

        let movement = crate::config::movement();
        let stick = game::read_move_stick(&movement);
        let write = intent::intent(
            state.last_position,
            state.creature.yaw().unwrap_or(0.0),
            stick,
            movement.speed_scale,
            movement.turn_deadzone_deg,
        );
        // Where the player is aiming, and the reason it has to be turned into a facing rather
        // than handed to the attack.
        //
        // A player's own attacks get their spawn orientation from `FUN_1403fb0b0`, which starts
        // from `ChrCtrl::GetPhysicsOrientation` and only bends it toward the camera when
        // `(*chr->vfptr->IsPlayerIns)(chr)` is true. That is a vtable predicate on the class, so
        // an `EnemyIns` can never take that branch and `camOverrideChrIns` cannot help -- the
        // override moves a pointer, not a type. Whatever the possessed creature fires therefore
        // leaves along its body facing, which is why the camera appeared to do nothing.
        //
        // So the bridge is the body: `wantToMoveTo` with `turnTarget = TARGET_SELF` is what
        // `UpdateMovement` turns into `aiIns+0xc3f0`, and `[vt+0x50]` differences that against the
        // live orientation outside the `walkType` gate -- so the body turns toward the aim even
        // standing still, and the movement staged at `ComManipulator+0x140` is untouched.
        let aim = intent::aim(
            state.last_position,
            camera::game::look_yaw(),
            state.creature.lock_on_target_pos(),
        );
        let write = match aim {
            Some(aim) => write.aiming_at(aim.point),
            None => write,
        };
        Self::tick_moveset(state, write.moving());

        // The net-damage watch, on its own cadence. Placed after the moveset tick so a sample
        // taken on a frame that fired something is taken after the fire, and outside every
        // input gate so it keeps reporting while the player stands still -- "neither HP bar goes
        // down" is a thing you notice by watching, not by pressing.
        if state.net.due(state.frames) {
            let creature_category = if state.creature.is_live() {
                // Both NPC categories route identically against a remote player, so the crate
                // never has to resolve `FUN_1403f3e30` to answer the question. See
                // `netdamage::Category::is_npc`.
                netdamage::Category::LocalNpc
            } else {
                netdamage::Category::None
            };
            for line in state.net.observe(
                creature_category,
                &game::players_in_world(),
                game::last_received_damage_packet(),
                state.frames,
            ) {
                possess_log(format_args!("{line}"));
            }
        }
        // The manual stage is gone, and its removal is the fix rather than a simplification.
        //
        // It was written when the vtable override starved the locomotion consumers: `[vt+0x50]`
        // published into the real manipulator and `FUN_1403cbff0` read the override's zeroes, so
        // nothing the engine computed ever reached the body and staging the vector ourselves was
        // the only way to move it. The swizzle removed that starvation -- one object, so the
        // publish and the consume are the same field -- and the workaround outlived its reason.
        //
        // Leaving it in is what made the movement unnatural, and the user named the symptom
        // exactly: "literally every animation causes the player to move the exact same way in the
        // same direction". A staged vector translates the body itself, so the clip playing over
        // the top is decorative and the body slides identically whatever it is. It also bypasses
        // the TURN: `[vt+0x50]` derives the facing inside `AiIns::UpdateMovement` ->
        // `FUN_1402c9410`, from `walkType` and `wantToMoveTo`, and a body moved by a staged vector
        // is never asked to rotate at all.
        //
        // So the AI write below is now the whole mechanism, which is what the native path was
        // always supposed to be: we fill the move request, and the engine computes the direction,
        // the gait scaling, the facing and the clip selection from it -- the same code that drives
        // every unpossessed creature in the game.
        if first_frame {
            // Once per possession, not sixty times a second. The read-back is taken immediately
            // after the AI write and is expected to disagree with it -- that disagreement is the
            // goal machine's `ClearMoveRequest`, and it is why `staged` is the value that moves
            // the body. A line that showed them agreeing would mean the goal churn had stopped.
            let (walk, target_x) = state
                .creature
                .read_move_intent()
                .unwrap_or((i32::MIN, f32::NAN));
            possess_log(format_args!(
                "movement: gait={} | AI read-back after our \
                 own write: walkType={walk} wantToMoveTo.x={target_x:.2} (ours was {:.2}). A \
                 disagreement here means the goal machine still zeroes walkType from one of its \
                 six lifecycle sites. It now MATTERS: with the manual stage removed, this request \
                 is the whole mechanism -- the engine derives the direction, the gait, the facing \
                 and the clip from it, the same way it does for every unpossessed creature",
                write.walk_type, write.target[0],
            ));
        }
        // The instrument, because three movement fixes have now shipped without one.
        //
        // The canary only answers "did the write land somewhere legitimate". It has never
        // answered the question the user keeps asking, which is "did pressing W produce
        // anything at all" -- and with no line between the key and the field, every attempt so
        // far has been a guess dressed as a diagnosis. This says, in order: whether an input
        // was read this frame, what gait and target came out of it, and what the field reads
        // back as after the write. A press that produces `stick=none` is an input bug; a
        // `stick=some` that reads back as zero is an engine bug; and those are different
        // repairs that were previously indistinguishable from the log.
        //
        // Rate-limited to one line a second and only while an input is actually being held, so
        // standing still is silent and a held key does not write sixty lines a second.
        // Throttled by frame count, not wall clock. `scripts/check-no-timeouts.py` bans an
        // `elapsed()` gate outright, and it is right to: the frame counter is the clock this
        // engine actually runs on, it cannot drift against the thing being measured, and a
        // stalled frame loop stops the log instead of flooding it.
        const MOVEMENT_LOG_EVERY_FRAMES: u64 = 60;
        let telemetry_due = state
            .last_movement_log
            .is_none_or(|at| state.frames.saturating_sub(at) >= MOVEMENT_LOG_EVERY_FRAMES);
        if stick.is_some() && telemetry_due {
            state.last_movement_log = Some(state.frames);
            possess_log(format_args!(
                "movement: input READ -- c{:04} gait={} target=({:.2}, {:.2}, {:.2}) \
                 turnTarget={} | \
                 creature at ({:.2}, {:.2}, {:.2}) yaw={:.3} | field read-back: walk_type={:?} \
                 want_to_move_to={:?} | staged={:?} published={:?} proxyFlags={:?} \
                 rootMotion2={:?} posNow={:?} anim={:?} gaitScale={:.2}. THE READ, and this \
                 crate now WRITES none of staged/published -- both are the engine's own work, \
                 derived from the walkType/wantToMoveTo request above. staged non-zero means \
                 [vt+0x50] computed a move vector of its own, which is the whole mechanism now \
                 that the manual stage is gone; staged ZERO with walk_type 2 means the request \
                 reached the field and the engine still declined it. Their FOURTH component is \
                 dot(direction, modelMatrix.row4) -- row4 is the TRANSLATION row, so w tracks the \
                 creature's position and is SUPPOSED to; [vt+0x50] computes the same value, and \
                 it was invisible until now only because this line printed three. rootMotion2 is \
                 the body's own displacement: NON-ZERO with posNow constant is a clip playing \
                 against something that holds the body. proxyFlags is a control, not a suspect: \
                 it reads 0",
                // The creature, first, because sliding is creature-dependent now: some move
                // correctly and at least one does not, so a line that describes the symptom
                // without naming the subject cannot be acted on.
                state.chr_id,
                write.walk_type,
                write.target[0],
                write.target[1],
                write.target[2],
                write.turn_target,
                state.last_position[0],
                state.last_position[1],
                state.last_position[2],
                state.creature.yaw().unwrap_or(f32::NAN),
                state.creature.read_walk_type(),
                state.creature.read_want_to_move_to(),
                state.creature.staged_move_vector(),
                state.creature.published_move_vector(),
                state.creature.chr_proxy_flags(),
                state.creature.root_motion_squared(),
                // Read here rather than reused from `state.last_position`, which was sampled
                // earlier in the frame: "position constant while rootMotion is non-zero" is the
                // whole distinction between a body that is held and a body that was never asked
                // to move, and it is only a distinction if both halves are one instant apart.
                state.creature.position(),
                state
                    .creature
                    .current_anim_frame()
                    .map(|frame| (frame.animation, frame.local_time)),
                write.gait_scale(),
            ));
        }
        // The aim instrument, and deliberately not gated on an input.
        //
        // The reported defect is "I move the camera and the cast vector does not follow", which is
        // a thing you observe standing still, so a line throttled behind `stick.is_some()` would
        // be silent on it. One line a second, always, while a possession is up.
        const AIM_LOG_EVERY_FRAMES: u64 = 60;
        let aim_due = state
            .last_aim_log
            .is_none_or(|at| state.frames.saturating_sub(at) >= AIM_LOG_EVERY_FRAMES);
        if aim_due {
            state.last_aim_log = Some(state.frames);
            possess_log(format_args!(
                "aim: source={} yaw={} point={:?} | body yaw={:.3} | ChrExFollowCam+0x154 \
                 camera yaw={:?} | \
                 ChrIns+0xd0 lockOnTargetPos={:?} | wantToMoveTo written={:?} teamType={:?} \
                 (worn={}). THE READ. source=camera with a non-None lockOnTargetPos means the \
                 lock point was REFUSED -- it is out of the 0.1..50 band or more than 60 degrees \
                 off the camera, which is the freshness test standing in for a lock-on state bit \
                 the override makes unreadable. source=none means the camera did not read at all \
                 and the body is being left facing wherever it walks, i.e. the OLD behaviour. \
                 `body yaw` converging on `yaw` over a few frames is the feature working; the \
                 convergence RATE is the creature's own NpcParam turn rate and is not ours",
                aim.map_or("none", |aim| aim.source.name()),
                aim.map_or_else(|| "n/a".to_owned(), |aim| format!("{:.3}", aim.yaw)),
                aim.map(|aim| aim.point),
                state.creature.yaw().unwrap_or(f32::NAN),
                camera::game::look_yaw(),
                state.creature.lock_on_target_pos(),
                write.aim,
                state.creature.team_type(),
                chr_ins::TEAM_TYPE_CHARMED,
            ));
        }
        // The bracket. The read in the telemetry block above happens before this write, so it
        // shows what survived the previous frame -- it can say the request is gone but not who
        // took it. This one is taken immediately after our own store, same frame, same thread. If
        // it reads back what we wrote and the next frame's line reads 0, the clearer runs between
        // this task and `[vt+0x50]`; if it disagrees here, the store never landed at all. That is
        // the fork the last round could not settle.
        let stored = state.creature.write_move_intent(write);
        if telemetry_due {
            let after = state.creature.read_move_intent();
            let agrees = after.is_some_and(|(walk, _)| walk == write.walk_type);
            if !stored || !agrees {
                possess_log(format_args!(
                    "movement: c{:04} THE WRITE DID NOT HOLD -- stored={stored}                      immediate-read-back={after:?} (we wrote walkType={}). This line is throttled,                      not once-per-possession: the previous version logged only on the first frame,                      so a request that started failing mid-possession was invisible and the                      creature simply stopped taking orders with nothing in the log. stored=false                      means our own store was refused -- the AiIns identity round trip failed.                      stored=true with a disagreeing read-back means something cleared it within                      the same frame",
                    state.chr_id, write.walk_type,
                ));
            }
        }
        if !stored && first_frame {
            // Said once, on the frame it is first known, rather than sixty times a second.
            possess_log(format_args!(
                "movement: the AiIns identity round trip did not close, so no movement intent is \
                 being written -- the character is possessed and camera-followed but will not \
                 walk. The check is exact (AiIns.comThinkOwner must BE the manipulator's inline \
                 member, which must point back at this ChrCtrl), so a failure means either a \
                 build whose ComManipulator/AiIns layout moved or an AiIns that is no longer \
                 live. Movement itself does not depend on this: it is staged at \
                 ComManipulator+0x140, one step downstream of the AI"
            ));
        }
    }
}

impl NpcPossessionEngine {
    /// Ask the game to create the creature `[spawn]` names, and start waiting for it.
    ///
    /// Returns `Accepted` the moment the `ChrIns` exists -- Not when it is drivable. The state
    /// machine above treats that as an active possession, which is right: the player pressed the
    /// key, something happened, and pressing again must cancel it rather than start a second one.
    fn begin_spawn(&mut self, request: &PossessionRequest, player: Chr) -> PossessionOutcome {
        let settings = request.target.spawn;
        let Some(position) = player.position() else {
            return PossessionOutcome::Refused("the player's position did not read".to_owned());
        };
        let yaw = player.yaw().unwrap_or(0.0);
        // In front of the player, at the player's own height, using the same basis the movement
        // target uses -- see `intent::ahead_of` for why that is one function and not two.
        let place_at = intent::ahead_of(position, yaw, settings.distance_m);
        let spec = SpawnSpec {
            chr_id: settings.chr_id,
            npc_param_id: settings.resolved_npc_param_id(),
            npc_think_id: settings.npc_think_id,
            position: place_at,
            yaw,
        };
        let spawned = match spawn_game::spawn(&spec) {
            Ok(spawned) => spawned,
            Err(reason) => {
                Self::write_spawn_refusal(settings.chr_id, &reason);
                return PossessionOutcome::Refused(reason);
            }
        };
        possess_log(format_args!(
            "spawn: created c{:04} (NpcParam {}) as ChrIns=0x{:x} in buddy roster slot {} -- \
             waiting up to {} ms for it to become drivable. Nothing is pumped while it loads: \
             EneDatManImp::Update walks every slot each frame from the game's own STEP_Update",
            settings.chr_id,
            spec.npc_param_id,
            spawned.creature.address(),
            spawned.slot,
            settings.readiness_ms,
        ));
        self.pending = Some(Pending {
            spawned,
            player,
            request: *request,
            readiness: Readiness::new(u64::from(settings.readiness_ms)),
            caps: layout::ene_dat_cap_offsets(game_file_version()),
            place_at,
            place_from: position,
            configured_distance_m: settings.distance_m,
            place_yaw: yaw,
            started: std::time::Instant::now(),
            reported: None,
            despawn_on_release: settings.despawn_on_release,
            chr_id: settings.chr_id,
        });
        PossessionOutcome::Accepted
    }

    /// One frame of waiting for a spawned creature to come up.
    fn tick_pending(&mut self) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };
        let elapsed = u64::try_from(pending.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let caps = pending.caps;
        let spawned = pending.spawned;
        let verdict = pending
            .readiness
            .observe(elapsed, |gate| spawn_game::evaluate(&spawned, caps, gate));
        match verdict {
            Poll::Waiting(gate) => {
                // Once per gate change. A per-frame line on a five-second wait is three hundred
                // identical lines, which is the same as no line at all.
                if pending.reported != Some(gate) {
                    pending.reported = Some(gate);
                    possess_log(format_args!(
                        "spawn: c{:04} is waiting on {} ({} ms elapsed)",
                        pending.chr_id,
                        gate.name(),
                        elapsed
                    ));
                }
            }
            Poll::Ready => {
                let pending = self.pending.take().expect("checked above");
                self.finish_spawn(pending, elapsed);
            }
            // The game took it away -- `EnemyIns::InitializeCharacterRendering` self-despawns a
            // character whose caps loaded but yielded no FLVER. The pointer is already dead, so
            // this drops it and must not call `RemoveChrIns` again.
            Poll::Vanished => {
                let pending = self.pending.take().expect("checked above");
                let reason = format!(
                    "the game removed c{:04} itself after {elapsed} ms, which is what it does to a \
                     character whose assets loaded but produced no model",
                    pending.chr_id
                );
                possess_log(format_args!("spawn: {reason}"));
                Self::write_spawn_refusal(pending.chr_id, &reason);
            }
            Poll::Expired(gate) => {
                let reached = pending.readiness.reached();
                let pending = self.pending.take().expect("checked above");
                // Both the gate it died on and how far it ever got. "waiting on chrres-loaded"
                // reads the same whether nothing at all happened or everything up to the assets
                // did, and those are different problems.
                let reason = format!(
                    "c{:04} was still waiting on {} after {elapsed} ms (furthest stage reached: \
                     {}): {}",
                    pending.chr_id,
                    gate.name(),
                    reached.map_or("none", Gate::name),
                    gate.stuck_means()
                );
                possess_log(format_args!("spawn: giving up -- {reason}"));
                Self::write_spawn_refusal(pending.chr_id, &reason);
                Self::cancel_pending(pending, Reason::SpawnTimedOut, true);
            }
        }
    }

    /// The creature is drivable. Put it where the press asked for it and wear it.
    fn finish_spawn(&mut self, pending: Pending, elapsed: u64) {
        // Placed here and not at spawn time, because the creature's position field is not read on
        // the creature path of `CreateCharacter` at all -- `InitEnemyChrBaseData` supplies the base
        // data and the request's vectors are consumed only by the PlayerIns branch. So the request
        // says where it should be and this is what actually puts it there, through the same proxy
        // drain co-location uses.
        //
        // ...AND it is also the first moment the creature'S own size can be read, which is why the
        // size-aware distance is derived here rather than at the press. `CSChrPhysicsModule` does
        // not exist until `InitForEnemy` has run; by this point it does, and `hit_radius` is the
        // real `NpcParam.hitRadius` rather than a guess. See `crate::spawn::placement` for why
        // `[spawn].distance_m` is now a floor.
        let placement = placement::place(
            pending.configured_distance_m,
            pending.spawned.creature.hit_radius(),
            pending.player.hit_radius(),
        );
        let place_at = if placement.creature_radius_m.is_some() {
            intent::ahead_of(pending.place_from, pending.place_yaw, placement.distance_m)
        } else {
            pending.place_at
        };
        let placed = pending
            .spawned
            .creature
            .request_move(place_at, Some(pending.place_yaw));
        possess_log(format_args!(
            "spawn: c{:04} became drivable after {elapsed} ms; {}",
            pending.chr_id,
            if placed {
                format!("placed in front of the player {}", placement.describe())
            } else {
                "COULD NOT BE PLACED, so it is wherever InitEnemyChrBaseData left it".to_owned()
            }
        ));
        let body = SpawnedBody {
            spawned: pending.spawned,
            despawn_on_release: pending.despawn_on_release,
        };
        let provenance = format!("mode=spawn slot={}", pending.spawned.slot);
        match self.enter(
            pending.spawned.creature,
            pending.player,
            &pending.request,
            &provenance,
            Some(body),
        ) {
            PossessionOutcome::Accepted => {}
            outcome => {
                // We made it and we cannot wear it, so we take it away -- an untracked creature is
                // one nothing will ever remove.
                let reason = format!(
                    "c{:04} loaded but could not be possessed: {}",
                    pending.chr_id,
                    outcome.describe()
                );
                possess_log(format_args!("spawn: {reason}"));
                Self::write_spawn_refusal(pending.chr_id, &reason);
                if pending.despawn_on_release {
                    spawn_game::despawn(pending.spawned.creature);
                }
            }
        }
    }

    /// `er-npc-possess.derived.toml`, for a press that produced no creature.
    ///
    /// Best-effort, like the moveset report it replaces: the game directory can be read-only, and a
    /// log line about failing to write a log-adjacent file is noise.
    fn write_spawn_refusal(chr_id: u32, reason: &str) {
        let _ = std::fs::write(
            crate::config::DERIVED_CONFIG_FILE_NAME,
            derived::render_spawn_refusal(chr_id, reason),
        );
    }

    /// Decide how big the player's body has to be for the lock-on reticle to land on the creature,
    /// apply it, and say what was decided.
    ///
    /// **Always logs, including when it does nothing.** "My reticle is still at my feet" needs an
    /// answer that is not "read the source", and the four ways this ends are four different
    /// problems: a height did not read, a height is not a plausible body, there was genuinely
    /// nothing to do, or the write did not land.
    ///
    /// The failed-write case still answers `Some`, and that is deliberate rather than sloppy: the
    /// record is what the release path restores from, so returning `None` after a write that
    /// partly landed would leave the body carrying a scale nothing will ever take off it.
    fn plan_body_scale(creature: Chr, player: Chr) -> Option<BodyScale> {
        let (Some(creature_height), Some(player_height)) =
            (creature.hit_height(), player.hit_height())
        else {
            possess_log(format_args!(
                "lock-on anchor: NOT adapted -- CSChrPhysicsModule+0x340 hitHeight did not read on \
                 the creature, the player, or both. The reticle stays on the body's own model, \
                 which on a large creature draws it near the feet"
            ));
            return None;
        };
        let Some(scale) = body_size::scale_for(creature_height, player_height) else {
            possess_log(format_args!(
                "lock-on anchor: NOT adapted -- hitHeight read {creature_height:.2} m (creature) \
                 and {player_height:.2} m (player), and at least one of those is not a plausible \
                 body. The reticle stays on the body's own model"
            ));
            return None;
        };
        if !body_size::worth_applying(scale) {
            possess_log(format_args!(
                "lock-on anchor: nothing to do -- the creature is {creature_height:.2} m against \
                 the body's {player_height:.2} m, so the reticle is already where it belongs"
            ));
            return None;
        }
        let Some(original) = player.body_scale() else {
            possess_log(format_args!(
                "lock-on anchor: NOT adapted -- ChrCtrl+0x2d4 did not read on the player's body, \
                 so there is nothing to put back afterwards and nothing is written"
            ));
            return None;
        };
        let worn = body_size::worn_scale(original, scale);
        if player.set_body_scale(worn) {
            possess_log(format_args!(
                "lock-on anchor: the body is stretched {scale:.2}x VERTICALLY ({creature_height:.2} \
                 m creature / {player_height:.2} m body), which lifts its lock-on dummy to the \
                 creature's own height. ChrCtrl+0x2d8 {:.2} -> {:.2}; X and Z are untouched. This \
                 is the RENDER transform only -- the body's hurtbox and collision do not move -- \
                 and it is what the reticle is drawn on, because the body is what your lock-on \
                 finds while the creature is the subject",
                original[1], worn[1],
            ));
        } else {
            possess_log(format_args!(
                "lock-on anchor: the {scale:.2}x vertical stretch did NOT fully land on \
                 ChrCtrl+0x2d8. The reticle may still be low, and release will put {original:?} \
                 back regardless"
            ));
        }
        Some(BodyScale { original, worn })
    }

    /// Wear `creature`: install the thunk, move the camera, neuter the player's body and build the
    /// moveset.
    ///
    /// Extracted so the found-a-target path and the spawned-one path are the same code. They
    /// differ only in where the creature came from -- and `spawned` is that difference, carried
    /// into the teardown so it knows whether it owes the world a despawn.
    fn enter(
        &mut self,
        creature: Chr,
        player: Chr,
        request: &PossessionRequest,
        provenance: &str,
        spawned: Option<SpawnedBody>,
    ) -> PossessionOutcome {
        let Some(real_com) = creature.real_manipulator() else {
            return PossessionOutcome::Refused("the target has no manipulator".to_owned());
        };
        let Some(thunk) = Thunk::build(real_com) else {
            return PossessionOutcome::Refused("could not reserve the thunk page".to_owned());
        };
        if !creature.install_manipulator_vtable(
            thunk.real_com(),
            thunk.original_vptr(),
            thunk.vtable_address(),
        ) {
            // Either the manipulator is no longer this creature's, or its vptr is not the one the
            // copy was taken from -- a second copy of this mod, or an assumption that has stopped
            // holding. Both are worse to force than to decline.
            return PossessionOutcome::Refused(
                "the creature's manipulator vtable was not the one we copied".to_owned(),
            );
        }
        if !game::set_camera_override(Some(creature)) {
            // Roll the one thing that is already installed back before giving up, or the creature
            // is left brainless with nobody driving it.
            game::Chr::restore_manipulator_vtable(
                thunk.real_com(),
                thunk.original_vptr(),
                thunk.vtable_address(),
            );
            return PossessionOutcome::Refused("could not move the camera".to_owned());
        }

        let debug_flags_offset = layout::debug_flags_offset(game_file_version());
        if debug_flags_offset.is_none() {
            possess_log(format_args!(
                "neuter: ChrIns.debugFlags has no known offset for {} -- the no-attack flag is \
                 being SKIPPED rather than written to a guessed address (it is +0x530 on 1.16.2 \
                 and +0x538 on 1.17, and the wrong one lands on a live neighbouring field)",
                describe_build()
            ));
        }
        let chr_id = creature.npc_param_id().unwrap_or(0);
        // The camera, last of the installs and after everything that can still refuse -- so there
        // is nothing to roll back if it fails, and nothing left behind if it succeeds and a later
        // step does not. `Session::begin` never panics and answers with a reason rather than an
        // error; see [`crate::camera`].
        let camera = camera::Session::begin(creature.address(), chr_id);
        possess_log(format_args!("{}", camera.log_line()));
        let moveset = Self::build_moveset(chr_id, request);
        // After everything that can still refuse, like the camera above: this one writes the
        // player's body rather than the creature's, so a later refusal would leave the body
        // wearing a size nothing is going to take off it.
        let body_scale = Self::plan_body_scale(creature, player);
        // The lock-on fix, and the smallest write in this whole function.
        //
        // Possession makes the creature the lock-on subject, so `CS::ChrIns::CanTargetTeamType`
        // is asked `relation[creature.team][candidate.team]` for every candidate in the world --
        // and an enemy creature is friend to every other enemy and enemy to the player. That is
        // one relation table lookup away from being the whole of what the player saw: the only
        // thing they could lock onto was their own co-located body, because it was the only
        // candidate on an opposing team. Putting the worn creature on `Charmed` inverts both
        // halves at once -- see [`chr_ins::TEAM_TYPE_CHARMED`] for the row read off the shipped
        // matrix. Read before the write, so the release has the real previous value and not an
        // assumption that every creature ships on the same team.
        let original_team = creature.team_type();
        let team_written =
            original_team.is_some_and(|_| creature.set_team_type(chr_ins::TEAM_TYPE_CHARMED));
        possess_log(format_args!(
            "lock-on: creature teamType {} -> {} ({}) -- this is what makes REAL ENEMIES lockable \
             and stops the player's own co-located body from being the only candidate. \
             CanTargetTeamType dispatches through CSTeamTypeRelation[subject][candidate] with \
             {{oppose, !friendly, !self}}, and possession made the CREATURE the subject; row 15 is \
             Rival to every hostile team and Friend to the player teams, so one byte flips both. \
             Nothing else in this mod makes the body untargetable -- IsLockOnDisabled still needs \
             a SpEffect -- but the body is now the WRONG TEAM to be a candidate at all",
            original_team.map_or_else(|| "unreadable".to_owned(), |team| team.to_string()),
            chr_ins::TEAM_TYPE_CHARMED,
            if team_written {
                "written"
            } else {
                "NOT WRITTEN -- lock-on is unchanged and the body is still the only candidate"
            },
        ));
        // So the first carried frame of every possession writes a co-location line, rather than
        // whichever frame the previous possession happened to leave the counter on.
        fall::reset_colocation_throttle();
        let state = Possessing {
            creature,
            player,
            spawned,
            body_scale,
            thunk,
            debug_flags_offset,
            release_on_death: request.target.release_on_death,
            last_position: creature.position().unwrap_or([0.0; 3]),
            last_grounded: creature.last_grounded_position(),
            // No line has been written yet, so the first frame carrying an input emits one.
            last_movement_log: None,
            last_on_ground: creature.standing_on_solid_ground().unwrap_or(false),
            frames: 0,
            camera,
            watchdog: Watchdog::new(
                // Seconds in the file, milliseconds in the engine. Clamped rather than cast bare:
                // a config value large enough to overflow would wrap to a tiny threshold and force
                // idle on every attack, which is the opposite of what the number asked for.
                (f64::from(request.mapping.watchdog_seconds) * 1000.0).clamp(1.0, 60_000.0) as u64,
                IDLE_ANIMATION,
            ),
            moveset,
            face: FaceEdges::default(),
            pages: FaceEdges::default(),
            elapsed_ms: 0,
            started: std::time::Instant::now(),
            chr_id,
            reported_dead_input: [false; 4],
            reported_availability: None,
            reported_movement_fire: false,
            reported_buffered_press: false,
            reported_one_page: [false; 2],
            original_team,
            last_aim_log: None,
            net: netdamage::Ledger::new(),
        };
        Self::write_derived(chr_id, &state.camera, creature, state.moveset.as_ref());
        match state.moveset.as_ref() {
            Some(dispatcher) => {
                // The panel goes up with the possession, not on the first page turn. A player who
                // never presses a page key still needs to know what r1/r2/l1/l2 fire on the
                // creature they just became -- and a panel that only appears once you have
                // already used the feature is no help to somebody trying to find out that the
                // feature exists. No flash: nothing has changed yet, this is the starting state.
                Self::publish_banner(chr_id, dispatcher, None);
                possess_log(format_args!(
                    "moveset: c{chr_id:04} {}",
                    dispatcher.summary()
                ));
                // The counts are not a sentence. `light=0 heavy=0 ranged=0 movement=12` is the
                // whole answer to "why do none of my attacks work", and a player reads it as
                // noise: they press the four attack buttons, get a step animation or nothing, and
                // conclude the mod is broken. 25 of the 408 creatures in the shipped table really
                // do have no attack animations -- a Balloon Dummy is one -- so this is a fact
                // about the creature that has to be said, not a failure to be hidden.
                if dispatcher.attack_count() == 0 {
                    possess_log(format_args!(
                        "moveset: c{chr_id:04} HAS NO ATTACK ANIMATIONS -- every move it has is \
                         locomotion, so r1/r2/l1 have nothing of their own to fire. With \
                         [mapping] unbound_inputs = \"promote\" they borrow a step from the \
                         movement list; with \"deny\" they do nothing. This is a property of the \
                         creature, not a failure: 25 of the 408 in the shipped table are like \
                         this. Movement, the camera, the HUD and release all still work."
                    ));
                }
                for hand in Hand::ALL {
                    let pages = dispatcher.pages(hand);
                    if pages > 1 {
                        possess_log(format_args!(
                            "moveset: {} attack set 1/{pages} -- the {} arrow pages {}/{} through \
                             the rest ([buttons] page_{}). er-npc-possess.derived.toml lists what \
                             each page leads with.",
                            hand.name(),
                            hand.name(),
                            hand.inputs()[0].name(),
                            hand.inputs()[1].name(),
                            hand.name(),
                        ));
                    }
                }
            }
            None => {
                // A panel that says "NOTHING" rather than no panel. The log line below is the
                // full explanation, but it is in a file; on screen, the difference between "this
                // creature has no attacks" and "the panel is broken" has to be visible, and only
                // a drawn panel can carry it.
                banner::publish(banner::Banner::absent(
                    chr_id,
                    crate::picker::catalog::name_of(chr_id),
                    request.buttons,
                ));
                possess_log(format_args!(
                    "moveset: c{chr_id:04} is not in the shipped table, so this character has no \
                     attacks. That is expected for a variant that owns a model but no animations \
                     of its own -- 20 of the 408 the offline sweep looked at are like that. \
                     Movement, camera, the HUD and release all still work."
                ));
            }
        }
        possess_log(format_args!(
            "possession: ChrIns=0x{:x} com=0x{real_com:x} thunk=0x{:x} camera=override player=0x{:x} \
             {provenance} spawned={} -- the body is still lock-on-ABLE in the IsLockOnDisabled \
             sense (that needs a SpEffect), but it is no longer a lock-on CANDIDATE: the team \
             write above put the worn creature on the other side of the relation table from it",
            creature.address(),
            state.thunk.vtable_address(),
            player.address(),
            match state.spawned {
                None => "no (the map placed this one)",
                Some(body) if body.despawn_on_release => "yes, and it is removed again on release",
                Some(_) => "yes, and [spawn].despawn_on_release leaves it standing",
            },
        ));
        debug_assert_eq!(state.thunk.real_com(), real_com);
        self.active = Some(state);
        PossessionOutcome::Accepted
    }
}

impl PossessionEngine for NpcPossessionEngine {
    fn possess(&mut self, request: &PossessionRequest) -> PossessionOutcome {
        if self.active.is_some() {
            return PossessionOutcome::Refused("already possessing".to_owned());
        }
        if self.pending.is_some() {
            return PossessionOutcome::Refused("a spawn is still coming up".to_owned());
        }
        let Some(player) = game::main_player() else {
            return PossessionOutcome::Refused("no local player".to_owned());
        };
        // The mode that creates rather than finds. It returns before any of the possession
        // machinery, because there is nothing to possess yet -- see `tick_pending`.
        if request.target.mode == TargetMode::Spawn {
            return self.begin_spawn(request, player);
        }
        let (target, candidates) = game::pick_target(request.target, player);
        let Some(creature) = target else {
            return PossessionOutcome::Refused(game::describe_no_target(
                request.target.mode,
                candidates,
            ));
        };
        let provenance = format!(
            "mode={} candidates={candidates}",
            request.target.mode.name()
        );
        self.enter(creature, player, request, &provenance, None)
    }

    fn release(&mut self) -> PossessionOutcome {
        self.release_with(Reason::Hotkey)
    }

    fn tick(&mut self) {
        if self.pending.is_some() {
            self.tick_pending();
            return;
        }
        self.tick_active();
    }

    /// A spawn in flight counts as active.
    ///
    /// Otherwise `tick_engine` would reconcile the toggle back to idle on the first frame of the
    /// wait, and the player's next press would start a second spawn while the first was still
    /// loading -- two creatures, one of them untracked and never removed.
    fn is_active(&self) -> bool {
        self.active.is_some() || self.pending.is_some()
    }

    /// Hold a config reload off while a spawn is coming up.
    ///
    /// The pending spawn carries the request it was started with, and it is replayed into the
    /// possession when the creature becomes drivable. Letting `[spawn]` move in between would mean
    /// the roster slot was created under one set of rules and worn under another -- and, if
    /// `despawn_on_release` moved, torn down under a third.
    fn accepts_reload(&self) -> bool {
        self.pending.is_none()
    }

    fn shutdown(&mut self) {
        // The one release that must happen even though nobody asked: an armed `ChrCtrl+0x3b0` in a
        // process that is about to unload this DLL is a DLPanic the next time that character is
        // torn down, with our code no longer present to explain it.
        //
        // A pending spawn goes through the same call, and this is why the check covers both: the
        // creature it created is real and registered even though nothing is possessed yet, so
        // "nothing to release" would be wrong. `release_with` declines the game call on this
        // thread and says the creature is being left; see `Step::DespawnCreature`.
        if self.active.is_some() || self.pending.is_some() {
            self.release_with(Reason::Shutdown);
        }
    }
}

/// A point, for a log line, or the word that says the read failed.
///
/// `None` is a real outcome on every one of these reads -- the module pointer chain can fail on a
/// character the game is tearing down -- and a line that says so is worth more than a line that
/// prints zeroes and lets the reader believe them.
fn describe_point(point: Option<[f32; 3]>) -> String {
    point.map_or_else(
        || "UNREADABLE".to_owned(),
        |point| format!("({:.2}, {:.2}, {:.2})", point[0], point[1], point[2]),
    )
}

/// A scalar in metres, likewise.
fn describe_metres(value: Option<f32>) -> String {
    value.map_or_else(|| "UNREADABLE".to_owned(), |value| format!("{value:.2}"))
}

/// A physics flag, likewise.
fn describe_flag(value: Option<bool>) -> String {
    value.map_or_else(|| "UNREADABLE".to_owned(), |value| format!("{value}"))
}
