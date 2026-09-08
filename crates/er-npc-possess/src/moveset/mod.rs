//! The MOVESET layer -- stack layer 3, the seam layer 2 left open.
//!
//! Layer 2 could wear a creature, move it and point a camera at it, and said in its own module
//! docs that attacks were left out because the animation-name question was unresolved. It is
//! resolved, and this is the answer.
//!
//! # Firing an attack is a field write
//!
//! `CS::CSChrEventModule::RequestAnimation` has a two-line body: a renderer-visibility call whose
//! effect the `ChrIns` update overwrites every frame, and one `int` store into
//! `CSChrEventModule+0x18 requestAnimationId`. So the whole firing mechanism is
//! `chr.request_animation(id)` writing an `i32`, and **this layer resolves no game function
//! address either** -- the property that makes the crate survive a game patch is intact through
//! all three layers.
//!
//! `CSChrEventModule::Update` consumes the request once per frame, formats
//! `DLString::FormatW(L"%s%04d", L"W_Event", animId)`, and resolves it against the same behaviour
//! world `PlayAnimationByBehaviorName` uses. The graph picks the clip, the clip carries its
//! TimeAct binding, and the hitbox, VFX, sound and root motion all arrive with it. None of that
//! had to be reimplemented, which is the entire reason this design drives animation ids rather
//! than behaviour ids.
//!
//! ## What is established about that root motion, and what is not
//!
//! The sentence above used to assert the displacement without anyone having watched it, so here
//! is the line between the two:
//!
//! * **This crate cannot be eating it.** The only thing possession writes anywhere near a
//!   character's own displacement is the co-location request, and that is written to the PLAYER:
//!   `driver.rs` calls `state.player.request_move(...)`, `state.player` is `game::main_player()`,
//!   and `request_move` resolves `self.chr_ctrl()` -- the player's. The creature's `ChrCtrl` proxy
//!   fields and flags are never touched, so the drain that snaps a body and zeroes its delta only
//!   ever reaches the one that is supposed to be dragged along.
//! * **It is the same mechanism walking rides on.** `crate::possess::intent` writes an AI move
//!   request; `[vt+0x50]` turns that into a normalised direction and hands it to
//!   `CSChrActionRequestModule`, the player's own request module, and the behaviour graph answers
//!   with locomotion clips. Nothing on that path takes a velocity. So attack root motion and walk
//!   displacement are one mechanism, not two, and they stand or fall together.
//! * **Nobody has measured a lunge.** `CSChrBehaviorModule+0x30` is the field that would say, and
//!   it is byte-verified on 1.17, so a reader would be reading the right field on the installed
//!   build. What it reports for a known-displacing attack has still not been captured. The
//!   watchdog no longer consults it at all -- see [`watchdog`], which now asks whether the
//!   PLAYHEAD advanced rather than whether the body moved -- so answering this needs a live run
//!   and a reader written for the purpose.
//!
//! # The one thing that cannot be checked at runtime
//!
//! An animation id the creature's behaviour graph has no transition for resolves cleanly and then
//! **does nothing**. No error, no exception, nothing on screen. The median creature declares 1366
//! event names and can fire 580 of them, so "just try it" would be wrong more than half the time
//! and would fail invisibly every time.
//!
//! That is why [`table`] exists: the fireability gate runs offline, over the unpacked corpus,
//! where the graph can be read -- and what ships is the answer. See
//! `scripts/er-moveset-table-gen.py`.
//!
//! # Grabs, and the animation this layer first mistook for one
//!
//! Layer 3 shipped saying no creature's grab could be fired, having swept all 408 creatures and
//! found that no event name in the 4000 band has a transition behind it under any prefix. That
//! sweep result is correct. The conclusion drawn from it was not, because the 4000-band clip is
//! not the grab.
//!
//! `CS::ChrDamageModule::ApplyDamage` looks up the `AtkParam` row of the hit that just landed and
//! reads `throwTypeId`. If it is non-zero it calls
//! `CSChrThrowModule::InitThrow(attackerThrowModule, victimChrIns, throwTypeId)` **before any
//! damage is calculated**, and returns early when the throw is accepted.
//! `CSThrowNode::ValidateAttemptAndReturnParamId` scans `ThrowParam` for a row matching (attacker
//! `ChrIns::npcId`, victim `ChrIns::npcId`, `throwTypeId`); on a match `CSChrThrowModule` sets
//! both nodes' roles and `PlayThrowAnim` plays `W_ThrowAtk` on the attacker and `W_ThrowDef` on
//! the victim. Those are bare names with no id in them -- which is exactly why nothing in the 4000
//! band is reachable by an event name, and why the fireability sweep was right and the label was
//! wrong.
//!
//! So the thing to fire is the INITIATOR, and it was already in the table as a plain attack: 153
//! of them across 78 creatures, every one in the 3000 band, all already fireable. Marking them
//! costs **no game address, no offset and no hook** -- the whole change is an extra param join in
//! the generator. What the runtime gained is a reason: [`table::Throw`] carries the victim chr id
//! and range the throw system will demand, `allow_grabs` finally withholds something, and a grab
//! that needs a creature victim is not offered when there is none in range.
//!
//! # the CHR-ID join that cost a third of the roster its attacks
//!
//! The generator joins five offline sources per creature, and for two of them it used the chr id as
//! the key on both sides: the behaviour graph (what can be fired) out of `<chr>.behbnd.dcx`, and
//! the TimeAct (what the animation does, and when it may be cancelled) out of `<chr>.anibnd.dcx`.
//! The first key is right. The second is wrong for 133 of the 408 creatures, because **a creature's
//! animations do not have to live under its own id**.
//!
//! c4351 Godrick Knight is the worked example. Its behaviour graph declares 72 in-band event names
//! and can fire all 72. Its `.anibnd` contains `skeleton.hkx` and nothing else -- no clips, no
//! `.tae` -- because the whole 435x knight family plays out of `c4350.anibnd.dcx`, whose
//! `INTERROOT_win64/chr/c4350/tae/c4350.tae` is a megabyte of TimeAct covering all six of them. So
//! c4351 reached [`table`]'s classifier with an empty TimeAct: no damage window on any of those 72,
//! every one denied `no-damage-window`, and then the in-span denial trim -- which keeps only
//! denials sitting among ids that did make it -- threw the whole list away, because the only
//! survivors were the twelve `W_Step` walk clips.
//!
//! The shipped row was therefore `4351 6000:3:0:0:2 6001:3:1:0:2 ...`: twelve moves, `denied=0`,
//! and not one attack. Identical rows for Battlemage, Cemetery Shade, Wolf, Troll Knight, Decaying
//! Ekzykes, every Godrick/Leyndell/Radahn soldier and knight, Lichdragon Fortissax. Nothing went
//! red anywhere, because "this creature has no attacks" is a legitimate answer -- a Balloon Dummy
//! really has none -- and the table could not tell the two cases apart.
//!
//! What names the owner is `NpcParam.behaviorVariationId`, which is `<family> * 100 + <variant>`:
//! 43500 is family 435 and ships under `c4350`; 41600 is family 416 and ships under `c4160`, which
//! is itself, which is why 270 creatures joined correctly and nobody noticed. The generator now
//! falls back to the family base's TimeAct when a creature has none of its own
//! (`tae_paths_for_chr`), and the shipped table went from 4,669 attacks to 7,174 across 128 changed
//! rows. Five creatures are still stranded -- c3020, c4250, c5194, c5261, c5560 have no `.tae`
//! under either id in the current extraction, which is a corpus gap rather than a join failure.
//!
//! Two gates hold it: `scripts/er-moveset-coverage.py --check` opens the TimeAct behind every
//! attackless creature and fails if it describes attacks the table is not offering, and
//! `only_a_handful_of_creatures_in_the_shipped_table_have_no_attacks_at_all` in [`table`] caps the
//! attackless count from inside `cargo test`.
//!
//! # The four parts
//!
//! * [`table`] -- what each creature can do, decided offline. Integers only.
//! * [`dispatch`] -- four fixed buttons onto 8-60 attacks, by distance band and rank cycling, and
//!   the per-hand attack-set page the left and right arrow keys move.
//! * [`chain`] -- When a press is allowed to happen: chain, wait, or fire. Never interrupt.
//! * [`watchdog`] -- forces idle when an animation strands the creature, and denies it afterwards.
//! * [`derived`] -- writes every decision, and every denial's reason, where the player can read it.
//!
//! # Cancel discipline, which is the fifth thing and arrived last
//!
//! Firing was a field write from the first commit, and the field write is honoured whenever it
//! lands -- `CSChrEventModule::Update` gates it on death, a throw, an HKS-driven flag and five
//! item-pickup ids, and on nothing about the animation currently playing. So for as long as this
//! layer only decided which move, a second press mid-swing cancelled the first, and the mod both
//! chained and interrupted depending on when you pressed.
//!
//! [`chain`] is the answer, and the rule it applies is the game's own: TAE event type 0 with
//! `FlagType` 86 sets the `taeCancels` bit that `CS::CSAiFunc::IsEnableCancelAttack` reads to let
//! a creature's AI chain out of a swing. Reading that bit is the whole oracle; the shipped table
//! carries the same window measured offline, as the fallback and as something the derived report
//! can print per move.
//!
//! # What is still a seam after this layer
//!
//! * **Nothing here is runtime-proven.** Every claim above is static: read out of the 1.16.2 dump,
//!   the unpacked corpus and `regulation.bin`. The game has not been launched against this code.
//! * **The cancel window is read one frame late.** The possession ticks in
//!   `CSTaskGroupIndex::FrameBegin`; `CS::ChrIns::PreBehaviorSafe` clears the transient
//!   `taeCancels` bits and the TimeAct events re-set them later in the same frame. 16 ms on a
//!   window whose median width is 800 ms, and not something this layer can fix without a second
//!   task in a later group.
//! * **one game function address, on the fallback path only.** `requestAnimationId` spells
//!   `W_Event%04d` and nothing else, and `W_Event` is a broad alias layer rather than a total one
//!   -- 88.4% num==anim-id across the corpus against 100% for `W_Step` in 6000-6023. So dodges,
//!   goal actions and the ride sets have no `W_Event` name at all and the field write cannot ask
//!   for them, however fireable they are. Those ids are fired through
//!   `PlayAnimationByBehaviorName` with a name built from the prefix the generator resolved from
//!   that creature's own event table, which costs the crate's zero-address property for those
//!   moves only. `W_Event` is preferred wherever it resolves, so the great majority never touch
//!   it. Losing every dodge in the game to keep a property was not a trade worth making.
//! * **The distance the dispatcher bands on is to the nearest hostile, not to a lock-on target.**
//!   `CSLockTgtMan` is not modelled by this crate, and reaching it would need either an address or
//!   an offset nobody here has verified on 1.17.
//! * **Reach for a move whose only damage row is a zero-damage marker is [`table::Reach::Unknown`],**
//!   so it is offered in every band rather than the right one.
//! * **Root-motion travel never made it into the table.** It is the documented fallback for reach
//!   when there is no hit capsule, and getting it means deserialising the animation itself, which
//!   the offline toolchain does not do.
//! * **A grab whose victim is the player is refused while the possession is running.** Settled
//!   statically on both builds, and not a limitation of this layer -- see
//!   [`crate::possess::layout::chr_ins::INVINCIBLE`] for the trace and
//!   `NpcPossessionEngine::neuter` for why it is kept. In short: `ApplyDamage` is the only caller
//!   of `InitThrow`, `HitChr` the only caller of `ApplyDamage`, and both hit-resolution routines
//!   that reach `HitChr` with a real attacker clear the victim through `IsImmuneToAttack` first --
//!   which reads the `chrFlags1c5 & 0x10` the neuter sets. `IsImmuneToThrow`, the adjacent vtable
//!   slot (`+0x1E0` against `+0x1D8`), does not read that bit, but it only guards the throw's own
//!   downstream pre-check; it never gets reached, because the hit that would start the throw is
//!   dropped first. The escape hatch in the predicate is `AtkParam.isDisableNoDamage`, which 188
//!   of the 206 rows with a non-zero `throwTypeId` leave at 0. The initiator plays either way; a
//!   grab that is refused is a swing that misses, and the derived file says so per move.
//! * **What is still unproven about grabs** is the co-op case: `ThrowParam.DefChrId` 0 matches any
//!   `ChrIns::npcId` 0, which a second player in the session also has, and this crate makes only
//!   the possessing player's own body immune. Nothing here has been runtime-tested.

pub(crate) mod banner;
pub(crate) mod chain;
pub(crate) mod derived;
pub(crate) mod dispatch;
pub(crate) mod table;
pub(crate) mod watchdog;

#[cfg(test)]
mod tests {
    /// The generator and this crate must agree on what the numbers in the table mean. They are
    /// written in two languages and there is no shared header, so the codes are asserted against
    /// the generator's source text -- if somebody renumbers a bucket on one side, this goes red
    /// instead of the moveset quietly turning every heavy attack into a ranged one.
    #[test]
    fn the_generator_and_the_parser_agree_on_every_code() {
        let generator = include_str!("../../../../scripts/er-moveset-table-gen.py");
        for expected in [
            "BUCKET_LIGHT, BUCKET_HEAVY, BUCKET_RANGED, BUCKET_MOVEMENT = 0, 1, 2, 3",
            "REACH_UNKNOWN, REACH_CLOSE, REACH_MID, REACH_FAR = 0, 1, 2, 3",
            "DENY_NOT_FIREABLE = 1",
            "DENY_NO_CLIP = 2",
            "DENY_NO_DAMAGE_WINDOW = 3",
            "DENY_MISSING_ATK_ROW = 4",
            "DENY_SPEFFECT_ONLY = 5",
            "DENY_UNRESOLVED_BEHAVIOR = 6",
            // The prefix table is a second shared vocabulary: the generator writes an index into
            // it and this crate turns that index back into the literal it will build a name from.
            // Getting them out of step would fire `W_Step` names at `W_GoalAction` ids.
            "'W_Event',",
            "'W_Attack',",
            "'W_Step',",
            "'W_GuardAttack',",
            "'W_GoalAction',",
            "'W_Ride_Attack_',",
            "'W_RideStep',",
            "'W_Ridden_Enemy_Attack',",
            "'W_Ride_Enemy_Attack',",
            "'W_Ridden_Enemy_Step',",
            "'W_Ride_Enemy_Step',",
            "DENY_UNUSABLE_AT_RUNTIME = 9",
            // The grab join. The column name is the whole mechanism: an attack is a grab because
            // its AtkParam row says so, not because its animation id is in a band.
            "DENY_THROW_RESULT_CLIP = 10",
            "ATK_THROW_FIELD = 'throwTypeId'",
            "'AtkChrId', 'DefChrId', 'throwTypeId', 'Dist'",
            // The chain window. 86 is the creature cancel-into-attack flag and 4 is the player
            // one; swapping them silently would produce a table with almost no windows in it,
            // which reads as "this game has no combos" rather than as a bug.
            "CANCEL_ATTACK_FLAG_TYPES = (86, 4)",
            "CHR_ACTION_FLAG_ARG = 0",
            "TABLE_VERSION = 4",
        ] {
            assert!(
                generator.contains(expected),
                "the generator no longer says `{expected}` -- the shipped table's codes and this \
                 crate's enums have drifted apart"
            );
        }
    }

    /// The generator's attack band is still the band the table is built from.
    ///
    /// It used to also assert a runtime NEUTRAL_ANIMATION_CEILING equal to it. That constant is
    /// gone: the band describes what the generator scans offline, and it was never a safe
    /// description of what the engine reports at runtime -- a possessed Battlemage idles in
    /// 43000. Nothing at runtime compares an id against a threshold any more.
    #[test]
    fn the_generator_still_builds_the_table_from_the_attack_band() {
        let generator = include_str!("../../../../scripts/er-moveset-table-gen.py");
        assert!(generator.contains("ATTACK_BAND = (3000, 3999)"));
    }
}
