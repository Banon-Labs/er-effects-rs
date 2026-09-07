//! The shipped moveset table: what each creature can be made to do, decided offline.
//!
//! # Why a table at all, rather than reading the graph at runtime
//!
//! Whether an animation can be fired is a property of the creature's Havok behaviour graph, and
//! the answer is not observable from inside the game. `CSChrEventModule::requestAnimationId`
//! formats the name `W_Event%04d`, hands it to the behaviour world, and if no
//! `hkbStateMachine::TransitionInfo` consumes that event id the call **returns cleanly and does
//! nothing at all** -- no error, no exception, no visible failure. A runtime "try it and see"
//! therefore cannot tell a wrong animation from a working one.
//!
//! So the gate happens offline, where the graph can be read: `scripts/er-moveset-table-gen.py`
//! walks every unpacked `<chr>.behbnd.dcx`, keeps only the ids some transition really consumes and
//! whose target state really has a clip, and writes the result here. Declared is not fireable --
//! the median creature declares 1366 event names and can fire 580 of them -- and that gap is
//! precisely what this file exists to have already resolved.
//!
//! ...and that is only half of it. The field write formats `W_Event%04d`, but `W_Event` is a broad
//! alias layer rather than a total one, so an id can be perfectly fireable under a different name
//! and unreachable by that field -- every dodge in the game is exactly that. The generator
//! therefore also resolves which spelling reaches each id on each creature, from that creature's
//! own event table; see [`Prefix`].
//!
//! # What is in the file
//!
//! Integers. Chr id, animation ids, a bucket, a rank, a reach band, a prefix index, a denial
//! reason. No game
//! bytes, no asset payloads, no strings taken out of the game -- animation ids are BND entry
//! filenames, and everything else is a decision the generator made about one. That is what keeps
//! it inside the repo's no-game-derived-binaries rule; see the generator's own header.
//!
//! # The one animation that lies about its own name
//!
//! `W_Event3110` plays clip `a000_003000`, not 3110 -- confirmed on every one of the eleven bosses
//! checked. So a row carries both ids: [`Move::fire`] is what gets written into the request field,
//! [`Move::played`] is what actually appears on screen. They are equal everywhere else, which is
//! why the format spells the second one only when it differs.
//!
//! # The grab, and the animation everybody mistakes for it
//!
//! A grab is not a 4000-band animation and it is not TimeAct event 304. It is an ordinary,
//! already-fireable attack whose `AtkParam_Npc` row has `throwTypeId != 0`: when that hit lands,
//! `ApplyDamage` hands it to the throw system before calculating any damage, and the throw system
//! -- not the event layer -- drives both parties into the 4000-band clips. That is exactly why no
//! event name in the 4000 band has a transition behind it on any of the 409 creatures swept: those
//! clips are reached by the bare names `W_ThrowAtk`/`W_ThrowDef` and are never addressed by id.
//! See [`Throw`] for the join and [`Denial::ThrowResultClip`] for how the clips are reported.

// Pure parsing; no game memory is touched here, so it stays ungated and `cargo test` proves it on
// the host.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::settings::Bucket;

/// The generated table, compiled in.
///
/// Regenerate with `scripts/er-moveset-table-gen.py --out crates/er-npc-possess/data/moveset.tbl`;
/// `moveset_table_regenerates_identically` in `tests` re-runs the generator and diffs when the
/// corpus is present.
pub(crate) const TABLE_TEXT: &str = include_str!("../../data/moveset.tbl");

/// Grammar version. The generator writes it and the parser refuses anything else, so a format
/// change cannot silently be read with the old column meanings.
pub(crate) const TABLE_VERSION: u32 = 4;

/// Which spelling of an animation ID this creature actually answers to.
///
/// `W_Event` is a broad alias layer, not a total one -- 88.4% num==anim-id across the corpus
/// against 100% for `W_Step` in 6000-6023 -- so an id can be perfectly fireable and still have no
/// `W_Event` name. c2120's dodges (6000/6001/6002/6003/6011) are exactly that: reachable under
/// `W_Step`, `W_RideStep`, `W_Ridden_Enemy_Step` and `W_Ride_Enemy_Step`, and under no `W_Event`
/// name at all.
///
/// **This is the one thing in the crate that costs a game function address**, and only for the
/// non-`Event` variants. [`Self::Event`] is fired by writing `CSChrEventModule+0x18`, which is a
/// field; everything else needs `PlayAnimationByBehaviorName` with a name built from the id. The
/// generator therefore prefers `Event` wherever it resolves, so the address is on the fallback
/// path only and the great majority of moves never touch it.
///
/// The variant is resolved per creature from that creature's own event table, never from the
/// animation-id band -- the two disagree constantly. c4500 declares `W_Step6000` through
/// `W_Step6023` and can fire none of them; c2120 declares the same range and fires five.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Prefix {
    /// The field write. No game address, and the default for everything that has one.
    Event,
    Attack,
    Step,
    GuardAttack,
    GoalAction,
    RideAttack,
    RideStep,
    RiddenEnemyAttack,
    RideEnemyAttack,
    RiddenEnemyStep,
    RideEnemyStep,
}

impl Prefix {
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Event),
            1 => Some(Self::Attack),
            2 => Some(Self::Step),
            3 => Some(Self::GuardAttack),
            4 => Some(Self::GoalAction),
            5 => Some(Self::RideAttack),
            6 => Some(Self::RideStep),
            7 => Some(Self::RiddenEnemyAttack),
            8 => Some(Self::RideEnemyAttack),
            9 => Some(Self::RiddenEnemyStep),
            10 => Some(Self::RideEnemyStep),
            _ => None,
        }
    }

    /// The literal the behaviour graph spells. Concatenated with the id at `%04d` to make the
    /// event name -- `W_Step6000`, `W_GoalAction3501`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Event => "W_Event",
            Self::Attack => "W_Attack",
            Self::Step => "W_Step",
            Self::GuardAttack => "W_GuardAttack",
            Self::GoalAction => "W_GoalAction",
            Self::RideAttack => "W_Ride_Attack_",
            Self::RideStep => "W_RideStep",
            Self::RiddenEnemyAttack => "W_Ridden_Enemy_Attack",
            Self::RideEnemyAttack => "W_Ride_Enemy_Attack",
            Self::RiddenEnemyStep => "W_Ridden_Enemy_Step",
            Self::RideEnemyStep => "W_Ride_Enemy_Step",
        }
    }

    /// Can this be fired without resolving a game function address?
    pub(crate) const fn is_field_write(self) -> bool {
        matches!(self, Self::Event)
    }
}

/// One way this attack can turn into a grab.
///
/// A grab is not an animation you play. `CS::ChrDamageModule::ApplyDamage` reads
/// `AtkParam.throwTypeId` off the hit that just landed and, before it calculates any damage,
/// calls `CSChrThrowModule::InitThrow(attackerThrowModule, victimChrIns, throwTypeId)`.
/// `CSThrowNode::ValidateAttemptAndReturnParamId` then walks `ThrowParam` looking for a row whose
/// `AtkChrId` is the attacker's `ChrIns::npcId`, whose `DefChrId` is the victim's, and whose
/// `throwTypeId` matches. On a match the throw system drives both parties into that row's
/// `atkAnimId`/`defAnimId` -- the 4000-band clips, reached through the bare behaviour names
/// `W_ThrowAtk`/`W_ThrowDef` and never by id.
///
/// So this type is the row's half of that match: who has to be on the receiving end, and how far
/// away they may be. It is what makes a grab refusable for a stated reason rather than a swing
/// that mysteriously never grabs anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Throw {
    /// `ThrowParam.DefChrId` -- the victim's `ChrIns::npcId`, matched exactly, not as a wildcard.
    /// `0` is the player, and it is `0` for 189 of the 190 creature rows in the shipped
    /// regulation; the single exception is c4280 grabbing c3300.
    pub(crate) victim_chr: u32,
    /// `ThrowParam.Dist` in DECIMETRES, because the table is integers-only. Divide by ten for
    /// metres. Measured across the shipped table: 50 to 1000, i.e. 5 m to 100 m -- most rows are
    /// 100 (10 m), and the 1000 is a single outlier. Comfortably inside a `u16`.
    pub(crate) range_dm: u16,
}

impl Throw {
    /// `ThrowParam.Dist` back in the metres the dispatcher measures distance in.
    pub(crate) fn range_m(self) -> f32 {
        f32::from(self.range_dm) / 10.0
    }

    /// Is the required victim the player's own body rather than another creature?
    ///
    /// It decides whether a live distance reading means anything for this throw. The possession
    /// co-locates the player's `PlayerIns` with the creature every frame, so a player-victim throw
    /// always has its victim in reach and a "nothing nearby" reading is about the wrong body.
    pub(crate) const fn victim_is_player(self) -> bool {
        self.victim_chr == 0
    }
}

/// Every `ThrowParam` row one attack can complete, inline and `Copy`.
///
/// Fixed-size rather than a `Vec` because [`Move`] is `Copy` and is copied per press. Four slots
/// against a measured maximum of TWO: swept over all 409 creatures, exactly one attack
/// (c4280's a3006) matches more than one row, and it matches two. The spare pair is headroom for
/// a regulation edit, not a guess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Throws {
    slots: [Option<Throw>; Self::CAPACITY],
}

impl Throws {
    pub(crate) const CAPACITY: usize = 4;

    /// No throw: an ordinary attack.
    pub(crate) const NONE: Self = Self {
        slots: [None; Self::CAPACITY],
    };

    /// Append, silently dropping past [`Self::CAPACITY`]. Dropping is right here: the slots hold
    /// alternative victims for the same attack, so an overflow costs one alternative rather than
    /// the move, and the move is still correctly marked a grab.
    fn push(&mut self, throw: Throw) {
        if let Some(slot) = self.slots.iter_mut().find(|slot| slot.is_none()) {
            *slot = Some(throw);
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = Throw> + '_ {
        self.slots.iter().filter_map(|slot| *slot)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Can this grab reach a victim, given the live distance to the nearest hostile?
    ///
    /// `None` for the distance means nothing is loaded nearby. The two cases are genuinely
    /// different and are kept apart on purpose:
    ///
    /// * a throw whose victim is the player is always reachable, because the possession keeps the
    ///   player's body co-located with the creature -- the hostile distance is measuring somebody
    ///   else entirely and must not be allowed to veto;
    /// * a throw whose victim is another creature needs that creature within `ThrowParam.Dist`,
    ///   and the nearest-hostile distance is the best reading the crate has of it.
    ///
    /// This is a necessary condition, not a sufficient one. It does not check the victim's chr id
    /// (the crate does not read `ChrIns::npcId`), nor the angle and vertical gates
    /// `ThrowPoseChecks` applies. The game re-checks all of it; this only stops the dispatcher
    /// from spending a press on a grab that provably cannot land.
    pub(crate) fn reachable(&self, distance_m: Option<f32>) -> bool {
        self.iter().any(|throw| {
            throw.victim_is_player()
                || distance_m.is_some_and(|distance| distance <= throw.range_m())
        })
    }
}

/// How far an attack reaches, in bands rather than metres.
///
/// Bands rather than a number because the dispatcher compares this against a live distance that is
/// itself noisy, and because the underlying source differs per move: a melee swing's reach is its
/// `AtkParam_Npc` hit-capsule radius, a projectile's is its `Bullet.dist` travel distance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Reach {
    /// The generator found a real behaviour row but no damaging hit capsule to measure -- so the
    /// move ships, and nothing is claimed about how far it goes.
    Unknown,
    Close,
    Mid,
    Far,
}

impl Reach {
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Unknown),
            1 => Some(Self::Close),
            2 => Some(Self::Mid),
            3 => Some(Self::Far),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Close => "close",
            Self::Mid => "mid",
            Self::Far => "far",
        }
    }
}

const fn bucket_from_code(code: u8) -> Option<Bucket> {
    match code {
        0 => Some(Bucket::Light),
        1 => Some(Bucket::Heavy),
        2 => Some(Bucket::Ranged),
        3 => Some(Bucket::Movement),
        _ => None,
    }
}

/// Why an animation the generator looked at is not offered.
///
/// Every one of these reaches the player: [`crate::moveset::derived`] writes the whole list, with
/// the reason spelled out, into `er-npc-possess.derived.toml` on every possession. Withholding a
/// move silently would be indistinguishable from the mod being broken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Denial {
    /// The event name exists in the graph's string table, but no transition consumes it. Firing it
    /// would resolve and then do nothing.
    NotFireable,
    /// A transition consumes it and lands on a real state, but that state plays no clip.
    NoClip,
    /// Fireable, with a clip, but the TimeAct carries no ability event -- so it is an animation
    /// with no damage window: a flourish, a turn, or an unused take.
    NoDamageWindow,
    /// Its behaviour row says `AtkParam_Npc`, and the row it names is not in the param. Malenia's
    /// 3025 is the worked example: judge 350 resolves to `2120350`, which does not exist.
    MissingAtkRow,
    /// The only thing its ability events do is apply a SpEffect. Not an attack.
    SpEffectOnly,
    /// The TimeAct names a behaviour id that `BehaviorParam` does not have a row for.
    UnresolvedBehavior,
    /// It is the animation the throw system plays once a grab has been accepted, not one anybody
    /// can fire. `CSChrThrowModule::PlayThrowAnim` reaches these through the two bare, un-numbered
    /// behaviour names `W_ThrowAtk` and `W_ThrowDef`; which clip that lands on is decided by the
    /// `ThrowParam` row, so there is no id to ask for. 108 animations across the corpus carry
    /// TimeAct event 304 `ThrowAttackBehavior`, all in the 4000 band, and not one of them is
    /// fireable under any prefix -- which is the whole of the "grabs are unreachable" finding,
    /// pointed at the wrong half of the mechanism. The grab itself is a 3000-band attack; see
    /// [`Throw`].
    ThrowResultClip,
    // RETIRED: reason 7, `PrefixUnreachable`. It meant "the graph can play this but the
    // `W_Event%04d` field write cannot spell it", and it covered every dodge in the game. The
    // class no longer exists: those ids are fired now, through `PlayAnimationByBehaviorName`
    // with the prefix the generator resolved -- see [`Prefix`]. An id no prefix reaches is
    // `NotFireable`, which is what it always was. The code is left unused rather than recycled,
    // so a table written before the fallback cannot be misread by this parser.
    /// Not a generator verdict: the runtime watchdog saw this animation softlock the creature and
    /// wrote it back. See [`crate::moveset::watchdog`].
    UnusableAtRuntime,
}

impl Denial {
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::NotFireable),
            2 => Some(Self::NoClip),
            3 => Some(Self::NoDamageWindow),
            4 => Some(Self::MissingAtkRow),
            5 => Some(Self::SpEffectOnly),
            6 => Some(Self::UnresolvedBehavior),
            9 => Some(Self::UnusableAtRuntime),
            10 => Some(Self::ThrowResultClip),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::NotFireable => "not-fireable",
            Self::NoClip => "no-clip",
            Self::NoDamageWindow => "no-damage-window",
            Self::MissingAtkRow => "missing-atk-row",
            Self::SpEffectOnly => "speffect-only",
            Self::UnresolvedBehavior => "unresolved-behavior",
            Self::UnusableAtRuntime => "unusable-at-runtime",
            Self::ThrowResultClip => "throw-result-clip",
        }
    }
}

/// One thing a possessed creature can be told to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Move {
    /// The id written into `CSChrEventModule::requestAnimationId`. The request field formats
    /// `W_Event%04d` from it, so this is the number that has to be fireable -- not the one that
    /// plays.
    pub(crate) fire: i32,
    /// The animation that actually appears. Equal to [`Self::fire`] except for `W_Event3110`.
    pub(crate) played: i32,
    pub(crate) bucket: Bucket,
    /// Position within the bucket, ascending by the generator's `duration x damage` score. Stable
    /// across regenerations, which is what makes rank-cycling reproducible.
    pub(crate) rank: u8,
    pub(crate) reach: Reach,
    /// The `ThrowParam` rows this attack can complete, empty for an ordinary one.
    ///
    /// Non-empty means this is a grab INITIATOR: an ordinary, already-fireable attack whose
    /// `AtkParam_Npc` row carries a non-zero `throwTypeId` and for which a `ThrowParam` row pairs
    /// this creature with a victim. Landing it is what starts a grab. See [`Throw`] for the
    /// mechanism and [`Denial::ThrowResultClip`] for the 4000-band clips that are not this.
    ///
    /// 153 of the 9426 shipped moves across 78 creatures, every one of them in the 3000 band.
    /// (169 counting `(animation, throwTypeId)` pairs -- a few attacks carry two.)
    pub(crate) throws: Throws,
    /// Which event-name spelling reaches this animation on this creature. See [`Prefix`]; it
    /// decides whether firing is a field write or a call.
    pub(crate) prefix: Prefix,
    /// When this move stops being worth protecting, in centiseconds from the start of the clip.
    ///
    /// The moment the last damage window closes: before it, a press that fires something else
    /// cancels a hit that has not landed; after it, the swing has done everything it is going to
    /// do and a follow-up is a combo rather than an interruption. Compared against
    /// `CSChrTimeActModule::animQueue[readIdx].localTime` by [`crate::moveset::chain`].
    ///
    /// `None` means the generator could not measure one -- a move with no resolvable ability
    /// event, which after the fireability gate is almost always a step or a dodge. Those are
    /// treated as committed for their whole length: a press during one waits. Centiseconds rather
    /// than a float because this table is integers only, and 10 ms is finer than a 60 Hz frame.
    pub(crate) chain_from_cs: Option<u16>,
}

impl Move {
    /// Is this a grab -- an attack that starts a throw when it lands?
    pub(crate) fn grab(&self) -> bool {
        !self.throws.is_empty()
    }

    /// [`Self::chain_from_cs`] in the seconds the game's own animation clock counts in.
    pub(crate) fn chain_from_s(&self) -> Option<f32> {
        self.chain_from_cs.map(|cs| f32::from(cs) / 100.0)
    }
}

/// Everything the table knows about one creature.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Moveset {
    pub(crate) moves: Vec<Move>,
    /// Animations considered and withheld, with the reason. Sorted by animation id.
    pub(crate) denials: Vec<(i32, Denial)>,
}

impl Moveset {
    pub(crate) fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// The moves in one bucket, in rank order.
    pub(crate) fn bucket(&self, bucket: Bucket) -> impl Iterator<Item = &Move> {
        self.moves.iter().filter(move |m| m.bucket == bucket)
    }

    pub(crate) fn find(&self, fire: i32) -> Option<&Move> {
        self.moves.iter().find(|m| m.fire == fire)
    }

    /// The move behind an animation the creature is observed to be playing.
    ///
    /// [`Move::played`] first, [`Move::fire`] second, and the order is the whole reason this is
    /// not [`Self::find`]. What comes back from `CSChrTimeActModule` is what is on screen, and
    /// for `W_Event3110` that is 3000 while the id the table is keyed on is 3110 -- so looking up
    /// by `fire` would miss exactly the one animation this crate already knows lies about its own
    /// name, and miss it silently.
    pub(crate) fn playing(&self, animation: i32) -> Option<&Move> {
        // The runtime ID is raw and this table'S is collapsed, so the lookup has to try both.
        //
        // `TAE_Callback` hands `CSChrTimeActModule::animQueue[].animId` the id Havok is running,
        // ungrouped -- and a creature's TimeAct numbers the same animation once per group, as
        // `<group> * 1000000 + <id>`. `scripts/er-moveset-table-gen.py` collapses that with
        // `% 1000000` before it writes a row, so a c3700 clip the engine reports as 3009000 is
        // 9000 in here. Measured on the 2026-09-02 Battlemage run: 3009000 and 3009500 are both
        // literal raw ids in c3700's own TimeAct, and matching only the raw value would have
        // missed every one of them.
        let collapsed = animation.rem_euclid(1_000_000);
        self.moves
            .iter()
            .find(|m| m.played == animation || m.played == collapsed)
            .or_else(|| {
                self.moves
                    .iter()
                    .find(|m| m.fire == animation || m.fire == collapsed)
            })
    }

    /// Move an animation out of the offered set and record why.
    ///
    /// Used by the `[chr.*] unusable` override and by the watchdog. Idempotent: denying something
    /// already denied does not duplicate the entry.
    pub(crate) fn deny(&mut self, fire: i32, reason: Denial) -> bool {
        let removed = self.moves.iter().position(|m| m.fire == fire);
        if let Some(index) = removed {
            self.moves.remove(index);
        }
        if self.denials.iter().any(|(id, _)| *id == fire) {
            return removed.is_some();
        }
        let at = self.denials.partition_point(|(id, _)| *id < fire);
        self.denials.insert(at, (fire, reason));
        removed.is_some()
    }

    /// Re-admit a denied animation, for the `[chr.*] usable` override.
    ///
    /// It comes back in [`Bucket::Light`] at the end of the rank order with [`Reach::Unknown`],
    /// because the generator declined to classify it and this crate has no way to do better: the
    /// numbers that would decide bucket and reach live in `regulation.bin`, not in the process.
    /// The player asked for it by animation id, so it is offered by animation id.
    pub(crate) fn admit(&mut self, fire: i32) -> bool {
        if self.moves.iter().any(|m| m.fire == fire) {
            return false;
        }
        self.denials.retain(|(id, _)| *id != fire);
        let rank = self
            .moves
            .iter()
            .filter(|m| m.bucket == Bucket::Light)
            .map(|m| u16::from(m.rank) + 1)
            .max()
            .unwrap_or(0);
        self.moves.push(Move {
            fire,
            played: fire,
            bucket: Bucket::Light,
            rank: u8::try_from(rank).unwrap_or(u8::MAX),
            reach: Reach::Unknown,
            // Re-admitted by animation id, and a `ThrowParam` join cannot be redone in the
            // process -- so it comes back as a plain attack. If it really is a grab initiator the
            // game still starts the throw when it lands; only this crate's label is missing, and
            // with it the `allow_grabs` veto the player just overrode by name anyway.
            throws: Throws::NONE,
            // Re-admitted by animation id, so the only spelling that can be assumed is the one
            // the field write can ask for. If it needed a different prefix the generator would
            // have offered it already.
            prefix: Prefix::Event,
            // Unmeasured, which for a chain window means committed for the whole clip: a press
            // during a move the player forced back on waits for it to finish rather than
            // cancelling it on a number nobody measured.
            chain_from_cs: None,
        });
        true
    }
}

/// The version the compiled-in table declares, read at compile time.
///
/// A `const fn` rather than a runtime check so that regenerating the table with a newer generator
/// and forgetting to update the parser is a build failure, not a silently mis-columned moveset.
const fn declared_version(text: &str) -> u32 {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'#' {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
            index += 1;
            continue;
        }
        if bytes[index] == b'\n' || bytes[index] == b'\r' {
            index += 1;
            continue;
        }
        if bytes[index] != b'v' {
            return 0;
        }
        let mut version = 0;
        let mut digit = index + 1;
        while digit < bytes.len() && bytes[digit] >= b'0' && bytes[digit] <= b'9' {
            version = version * 10 + (bytes[digit] - b'0') as u32;
            digit += 1;
        }
        return version;
    }
    0
}

const _: () = assert!(
    declared_version(TABLE_TEXT) == TABLE_VERSION,
    "data/moveset.tbl declares a grammar version this parser was not written for -- regenerate \
     with scripts/er-moveset-table-gen.py and update moveset::table"
);

/// Parse the grab suffix: `g<victim>,<rangeDm>[+<victim>,<rangeDm>]...`.
///
/// `None` when the suffix is present but malformed, so a mangled grab spec costs the whole entry
/// rather than silently downgrading a grab into an ordinary attack -- `parse_line` then skips the
/// field and says so by omission. Silently dropping the spec would leave a grab on offer that
/// `allow_grabs = false` no longer withholds.
fn parse_throws(spec: &str) -> Option<Throws> {
    let mut throws = Throws::NONE;
    for row in spec.split('+') {
        let (victim, range) = row.split_once(',')?;
        throws.push(Throw {
            victim_chr: victim.parse().ok()?,
            range_dm: range.parse().ok()?,
        });
    }
    (!throws.is_empty()).then_some(throws)
}

/// Parse `<fired>[=<played>][w<chainFromCs>][g<victim>,<rangeDm>[+...]]:<bucket>:<rank>:<reach>[:<prefix>]`.
fn parse_move(field: &str) -> Option<Move> {
    let mut parts = field.split(':');
    let head = parts.next()?;
    let bucket = bucket_from_code(parts.next()?.parse().ok()?)?;
    let rank = parts.next()?.parse().ok()?;
    let reach = Reach::from_code(parts.next()?.parse().ok()?)?;
    // The prefix column is omitted for `W_Event`, which is both the common case and the one that
    // costs no game address -- so a four-field entry reads as "fired by the field write" and a
    // five-field one as "fired by name".
    let prefix = match parts.next() {
        Some(code) => Prefix::from_code(code.parse().ok()?)?,
        None => Prefix::Event,
    };
    if parts.next().is_some() {
        return None;
    }
    // The grab suffix is `g` and everything after it. `split_once` rather than a search from the
    // end, because the spec itself contains no `g` -- only digits, commas and `+`.
    let (head, throws) = match head.split_once('g') {
        Some((rest, spec)) => (rest, parse_throws(spec)?),
        None => (head, Throws::NONE),
    };
    // ...and the chain window is `w` and the digits after it, stripped before the grab spec has
    // been removed above, so `3006w45g0,100` reads as (3006, window 0.45s, grab). It is a suffix
    // rather than a seventh colon-separated column because the prefix column is already optional:
    // a positional window would force every `W_Event` move to spell a prefix it does not have.
    // Absent means unmeasured, not zero -- zero would say "chainable from the first frame", which
    // is the opposite of what an unmeasured move should be treated as.
    let (head, chain_from_cs) = match head.split_once('w') {
        Some((rest, window)) => (rest, Some(window.parse().ok()?)),
        None => (head, None),
    };
    let (fire, played) = match head.split_once('=') {
        Some((fire, played)) => (fire.parse().ok()?, played.parse().ok()?),
        None => {
            let fire = head.parse().ok()?;
            (fire, fire)
        }
    };
    Some(Move {
        fire,
        played,
        bucket,
        rank,
        reach,
        throws,
        prefix,
        chain_from_cs,
    })
}

/// Parse `!<fired>:<reason>`.
fn parse_denial(field: &str) -> Option<(i32, Denial)> {
    let (fire, reason) = field.strip_prefix('!')?.split_once(':')?;
    Some((fire.parse().ok()?, Denial::from_code(reason.parse().ok()?)?))
}

/// Parse one creature's line. `None` for a comment, the version marker, or a blank.
///
/// A field that does not parse is skipped rather than failing the line: one malformed entry
/// costing one move is better than costing the creature its whole moveset.
pub(crate) fn parse_line(line: &str) -> Option<(u32, Moveset)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('v') {
        return None;
    }
    let (chr, rest) = line.split_once(' ')?;
    let chr = chr.parse().ok()?;
    let mut moveset = Moveset::default();
    for field in rest.split_whitespace() {
        if field.starts_with('!') {
            if let Some(denial) = parse_denial(field) {
                moveset.denials.push(denial);
            }
        } else if let Some(entry) = parse_move(field) {
            moveset.moves.push(entry);
        }
    }
    Some((chr, moveset))
}

/// The moveset for one creature, by numeric chr id (`c4500` is `4500`).
///
/// Scans the compiled-in text rather than building a map: it is called once per possession, the
/// table is a few hundred lines, and a lazily-initialised static would cost a lock on a path that
/// runs once.
pub(crate) fn lookup(chr_id: u32) -> Option<Moveset> {
    let wanted = chr_id.to_string();
    for line in TABLE_TEXT.lines() {
        let Some(rest) = line.strip_prefix(&wanted) else {
            continue;
        };
        if !rest.starts_with(' ') {
            continue;
        }
        return parse_line(line).map(|(_, moveset)| moveset);
    }
    None
}

/// Every creature the table covers, for the status line and for tests.
pub(crate) fn chr_ids() -> impl Iterator<Item = u32> {
    TABLE_TEXT
        .lines()
        .filter_map(|line| parse_line(line).map(|(chr, _)| chr))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_compiled_in_table_declares_the_version_this_parser_reads() {
        assert_eq!(declared_version(TABLE_TEXT), TABLE_VERSION);
    }

    #[test]
    fn every_line_of_the_shipped_table_parses() {
        let mut chrs = 0;
        let mut moves = 0;
        let mut denials = 0;
        for line in TABLE_TEXT.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('v') {
                continue;
            }
            let (chr, moveset) =
                parse_line(line).unwrap_or_else(|| panic!("unparsable line: {line}"));
            assert!(chr > 0, "chr id 0 in: {line}");
            // Field-level skipping is deliberate, so a line that parsed but dropped everything
            // would be invisible. Count instead, and assert the totals below.
            chrs += 1;
            moves += moveset.moves.len();
            denials += moveset.denials.len();
        }
        assert!(chrs > 200, "only {chrs} creatures in the table");
        assert!(moves > 2000, "only {moves} moves in the table");
        assert!(
            denials > 0,
            "no denials at all -- the reasons are not being shipped"
        );
    }

    /// The systematic exception, asserted against the shipped data rather than trusted.
    #[test]
    fn w_event_3110_is_recorded_as_playing_3000() {
        let mut seen = 0;
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            let Some(entry) = moveset.find(3110) else {
                continue;
            };
            assert_eq!(
                entry.played, 3000,
                "c{chr} W_Event3110 should play a000_003000, not {}",
                entry.played
            );
            assert_ne!(entry.fire, entry.played);
            seen += 1;
        }
        assert!(seen >= 10, "only {seen} creatures carry the 3110 exception");
    }

    /// Nothing in the offered set may also be denied, or the derived file would tell the player
    /// an animation is both available and withheld.
    #[test]
    fn no_animation_is_both_offered_and_denied() {
        for chr in chr_ids() {
            let moveset = lookup(chr).expect("listed chr must look up");
            for entry in &moveset.moves {
                assert!(
                    !moveset.denials.iter().any(|(id, _)| *id == entry.fire),
                    "c{chr} {} is both offered and denied",
                    entry.fire
                );
            }
        }
    }

    /// Ranks inside a bucket must be a dense 0..n. The dispatcher cycles by index, so a gap
    /// would be a press that fires nothing.
    #[test]
    fn ranks_are_dense_within_every_bucket() {
        for chr in chr_ids() {
            let moveset = lookup(chr).expect("listed chr must look up");
            for bucket in [
                Bucket::Light,
                Bucket::Heavy,
                Bucket::Ranged,
                Bucket::Movement,
            ] {
                let mut ranks: Vec<u8> = moveset.bucket(bucket).map(|m| m.rank).collect();
                ranks.sort_unstable();
                for (index, rank) in ranks.iter().enumerate() {
                    assert_eq!(
                        usize::from(*rank),
                        index,
                        "c{chr} {:?} ranks are not dense: {ranks:?}",
                        bucket
                    );
                }
            }
        }
    }

    #[test]
    fn a_move_field_round_trips() {
        let parsed = parse_move("3110=3000g0,100:2:4:3").expect("well-formed");
        assert_eq!(parsed.fire, 3110);
        assert_eq!(parsed.played, 3000);
        assert_eq!(parsed.bucket, Bucket::Ranged);
        assert_eq!(parsed.rank, 4);
        assert_eq!(parsed.reach, Reach::Far);
        assert_eq!(parsed.prefix, Prefix::Event);
        assert!(parsed.grab());
        assert_eq!(
            parsed.throws.iter().collect::<Vec<_>>(),
            vec![Throw {
                victim_chr: 0,
                range_dm: 100
            }]
        );
        let plain = parse_move("3001:0:0:1").expect("well-formed");
        assert_eq!(plain.fire, plain.played);
        assert!(!plain.grab());
        assert_eq!(plain.reach, Reach::Close);
        assert_eq!(plain.chain_from_cs, None);
    }

    /// The window sits between the played id and the grab spec, and all four parts of a head have
    /// to survive being spelled together -- this is the entry shape c4280's a3006 would take if
    /// it ever gained one.
    #[test]
    fn a_chain_window_parses_beside_a_played_id_and_a_grab_spec() {
        let full = parse_move("3110=3000w947g0,100:2:4:3").expect("well-formed");
        assert_eq!(full.fire, 3110);
        assert_eq!(full.played, 3000);
        assert_eq!(full.chain_from_cs, Some(947));
        assert!(full.grab());
        let bare = parse_move("3000w947:0:0:1").expect("well-formed");
        assert_eq!(bare.chain_from_cs, Some(947));
        assert_eq!(bare.fire, 3000);
    }

    /// Centiseconds in, seconds out, because the game's animation clock counts in seconds and
    /// the table counts in integers. A tenth of a frame of drift here would be invisible; a
    /// factor of ten would put every window past the end of its animation.
    #[test]
    fn the_window_converts_from_centiseconds_to_the_seconds_the_game_counts_in() {
        let entry = parse_move("3000w947:0:0:1").expect("well-formed");
        assert_eq!(entry.chain_from_s(), Some(9.47));
        assert_eq!(parse_move("3000:0:0:1").unwrap().chain_from_s(), None);
    }

    /// A window that will not parse costs the whole entry rather than being silently dropped:
    /// keeping the move with no window would turn a typo into "this attack can never be chained",
    /// which is indistinguishable from the mod working.
    #[test]
    fn a_malformed_window_rejects_the_entry_rather_than_dropping_the_window() {
        assert!(parse_move("3000wxyz:0:0:1").is_none());
        assert!(parse_move("3000w:0:0:1").is_none());
    }

    /// `Moveset::playing` is keyed on what is on screen, and `W_Event3110` is the one animation
    /// where that differs from the id the table is keyed on. Looking the window up by `fire`
    /// would miss it, and miss it quietly.
    #[test]
    fn the_played_animation_finds_the_move_even_when_it_is_not_the_fired_id() {
        let (_, moveset) = parse_line("4500 3110=3000w947:2:0:3 3001w853:2:1:3").expect("parses");
        assert_eq!(moveset.playing(3000).map(|m| m.fire), Some(3110));
        assert_eq!(moveset.playing(3001).map(|m| m.fire), Some(3001));
        assert!(moveset.playing(9999).is_none());
    }

    /// c4280's a3006 is the only attack in the game that matches two `ThrowParam` rows, and the
    /// only creature-victim grab there is. It is the reason [`Throws`] is a list.
    #[test]
    fn a_grab_can_name_more_than_one_victim() {
        let parsed = parse_move("3006g0,100+3300,100:0:0:1").expect("well-formed");
        assert_eq!(
            parsed.throws.iter().collect::<Vec<_>>(),
            vec![
                Throw {
                    victim_chr: 0,
                    range_dm: 100
                },
                Throw {
                    victim_chr: 3300,
                    range_dm: 100
                },
            ]
        );
        assert!(
            parsed
                .throws
                .iter()
                .next()
                .expect("first")
                .victim_is_player()
        );
        assert!(
            !parsed
                .throws
                .iter()
                .nth(1)
                .expect("second")
                .victim_is_player()
        );
    }

    /// A mangled grab spec must cost the whole entry, never quietly downgrade a grab into an
    /// ordinary attack that `allow_grabs = false` would then fail to withhold.
    #[test]
    fn a_malformed_grab_spec_rejects_the_entry_rather_than_dropping_the_grab() {
        assert!(parse_move("3006g:0:0:1").is_none());
        assert!(parse_move("3006gnonsense:0:0:1").is_none());
        assert!(parse_move("3006g0:0:0:1").is_none());
        assert!(parse_move("3006g0,:0:0:1").is_none());
    }

    /// The reach gate, which is what makes `allow_grabs` refuse for a stated reason.
    #[test]
    fn a_player_victim_throw_is_always_reachable_and_a_creature_one_is_not() {
        let player = parse_move("3016g0,100:0:0:1").expect("well-formed").throws;
        // The player's body is co-located with the creature, so a hostile distance -- or the
        // absence of one -- says nothing about whether this throw has a victim.
        assert!(player.reachable(None));
        assert!(player.reachable(Some(200.0)));

        let creature = parse_move("3006g3300,100:0:0:1")
            .expect("well-formed")
            .throws;
        assert!(creature.reachable(Some(9.9)), "9.9 m is inside a 10 m Dist");
        assert!(!creature.reachable(Some(10.1)));
        assert!(!creature.reachable(None), "nothing loaded is not a victim");
        // Exactly at the range is inclusive here and exclusive in the game -- `ThrowPoseChecks`
        // is `if (distance < GetDist(row))`. Deliberate: this gate is a filter in front of the
        // game's own check, and being a hair generous costs a swing that misses rather than a
        // grab silently withheld. The reading it compares is coarser than that anyway: the game
        // measures between each party's physics position, swapped for a dummy-poly position when
        // the row names one in `judgeRangeBasePosDmyId1/2` (`GetPlayerThrowPositions`).
        assert!(creature.reachable(Some(10.0)));

        // The two-victim case falls back to the player half, which always holds.
        let both = parse_move("3006g0,100+3300,100:0:0:1")
            .expect("well-formed")
            .throws;
        assert!(both.reachable(None));

        assert!(
            !Throws::NONE.reachable(None),
            "an empty set reaches nothing"
        );
        assert!(!Throws::NONE.reachable(Some(0.0)));
    }

    /// Past capacity the move stays a grab and loses only an alternative victim -- never the
    /// other way round, which would put an ungated grab on offer.
    #[test]
    fn more_victims_than_there_are_slots_costs_a_victim_and_not_the_grab() {
        let spec: String = (0..Throws::CAPACITY + 3)
            .map(|index| format!("{}{index},50", if index == 0 { "" } else { "+" }))
            .collect();
        let parsed = parse_move(&format!("3006g{spec}:0:0:1")).expect("well-formed");
        assert!(parsed.grab());
        assert_eq!(parsed.throws.iter().count(), Throws::CAPACITY);
    }

    #[test]
    fn a_malformed_field_is_skipped_and_the_rest_of_the_line_survives() {
        let (chr, moveset) =
            parse_line("4500 3000:0:0:1 nonsense 3001:1:0:2 !3013:3 !bad").expect("parses");
        assert_eq!(chr, 4500);
        assert_eq!(moveset.moves.len(), 2);
        assert_eq!(moveset.denials, vec![(3013, Denial::NoDamageWindow)]);
    }

    #[test]
    fn comments_and_the_version_marker_are_not_creatures() {
        assert!(parse_line("# a comment").is_none());
        assert!(parse_line("v1").is_none());
        assert!(parse_line("").is_none());
    }

    #[test]
    fn denying_moves_an_entry_across_and_admitting_brings_it_back() {
        let (_, mut moveset) = parse_line("4500 3000:0:0:1 3001:0:1:1").expect("parses");
        assert!(moveset.deny(3001, Denial::UnusableAtRuntime));
        assert!(moveset.find(3001).is_none());
        assert_eq!(moveset.denials, vec![(3001, Denial::UnusableAtRuntime)]);
        // Idempotent.
        assert!(!moveset.deny(3001, Denial::UnusableAtRuntime));
        assert_eq!(moveset.denials.len(), 1);
        assert!(moveset.admit(3001));
        assert!(moveset.find(3001).is_some());
        assert!(moveset.denials.is_empty());
    }

    #[test]
    fn a_known_boss_looks_up_with_a_usable_moveset() {
        // c4500, the Flying Dragon: the creature every offline finding in this layer was checked
        // against. If the table ever stops carrying it, the corpus join broke.
        let dragon = lookup(4500).expect("c4500 must be in the table");
        assert!(dragon.moves.len() >= 15, "{}", dragon.moves.len());
        assert!(dragon.bucket(Bucket::Ranged).count() >= 1);
        assert!(dragon.bucket(Bucket::Light).count() >= 1);
        assert!(dragon.bucket(Bucket::Heavy).count() >= 1);
        // Its two declared-but-stateless attack events, from the corpus sweep.
        for id in [3014, 3019] {
            assert_eq!(
                dragon
                    .denials
                    .iter()
                    .find(|(a, _)| *a == id)
                    .map(|(_, r)| *r),
                Some(Denial::NotFireable),
                "c4500 {id} should be declared-but-not-fireable"
            );
        }
    }

    /// The grabs, asserted against the shipped data rather than against the finding that produced
    /// it. Before the `ThrowParam` join the table carried zero grab-marked moves, so `allow_grabs`
    /// gated nothing and said so in its own doc comment. If this goes back to zero the join broke
    /// and the setting is dead again.
    #[test]
    fn the_shipped_table_carries_real_grabs_and_every_one_is_a_fireable_attack() {
        let mut grabs = 0;
        let mut creatures = 0;
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            let here = moveset.moves.iter().filter(|entry| entry.grab()).count();
            if here > 0 {
                creatures += 1;
            }
            grabs += here;
            for entry in moveset.moves.iter().filter(|entry| entry.grab()) {
                // The whole point. A grab initiator is an ordinary attack; the 4000-band clip it
                // leads to is the throw system's, and is never on offer.
                assert!(
                    (3000..4000).contains(&entry.fire),
                    "c{chr} {} is marked a grab but is not in the attack band",
                    entry.fire
                );
                assert!(
                    entry.throws.iter().count() <= Throws::CAPACITY,
                    "c{chr} {} overflowed its victim list",
                    entry.fire
                );
                for throw in entry.throws.iter() {
                    assert!(
                        throw.range_dm > 0,
                        "c{chr} {} has a zero-range ThrowParam row",
                        entry.fire
                    );
                }
            }
        }
        assert!(
            grabs >= 150 && creatures >= 70,
            "only {grabs} grabs across {creatures} creatures -- the ThrowParam join is not \
             producing what the corpus sweep measured (153 across 78)"
        );
    }

    /// The other half: the 4000-band clips are reported, with a reason that is true of them,
    /// instead of vanishing or being mislabelled `not-fireable`.
    #[test]
    fn the_throw_result_clips_are_denied_with_their_own_reason() {
        let mut clips = 0;
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            for (animation, reason) in &moveset.denials {
                if *reason != Denial::ThrowResultClip {
                    continue;
                }
                assert!(
                    (4000..5000).contains(animation),
                    "c{chr} {animation} is a throw-result clip outside the 4000 band"
                );
                clips += 1;
            }
        }
        assert!(
            clips >= 50,
            "only {clips} throw-result clips reported -- 88 fall inside a creature's span"
        );
    }

    /// Malenia, the creature whose grab the config file names by animation id. Both halves of the
    /// mechanism have to show up on her line, or the documentation is lying to the player.
    #[test]
    fn malenias_grab_is_an_attack_and_her_4100_is_a_throw_result_clip() {
        let malenia = lookup(2120).expect("c2120 must be in the table");
        let grab = malenia
            .moves
            .iter()
            .find(|entry| entry.grab())
            .expect("c2120 has a grab initiator");
        assert!((3000..4000).contains(&grab.fire), "{}", grab.fire);
        assert!(
            grab.throws.iter().all(|throw| throw.victim_is_player()),
            "c2120 grabs the player"
        );
        for clip in [4100, 4101] {
            assert_eq!(
                malenia
                    .denials
                    .iter()
                    .find(|(a, _)| *a == clip)
                    .map(|(_, r)| *r),
                Some(Denial::ThrowResultClip),
                "c2120 a{clip} should be reported as the throw system's clip"
            );
        }
    }

    /// The FALLBACK, asserted against the shipped data. Dodges have no `W_Event` name, so if any
    /// step in the table claimed to be a field write the runtime would write an id the request
    /// field cannot spell and the button would silently do nothing.
    #[test]
    fn every_step_ships_with_a_by_name_prefix_and_never_the_field_write() {
        let mut steps = 0;
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            for entry in moveset.bucket(Bucket::Movement) {
                assert!(
                    !entry.prefix.is_field_write(),
                    "c{chr} {} is a step claiming the W_Event field write, which cannot reach it",
                    entry.fire
                );
                assert_eq!(entry.prefix, Prefix::Step);
                steps += 1;
            }
        }
        assert!(
            steps > 1000,
            "only {steps} dodges in the whole table -- the by-name fallback is not doing its job"
        );
    }

    /// ...and the converse: the address is confined to the minority that needs it. If this ratio
    /// inverts, the crate started paying a resolved game address for moves a field write reaches.
    #[test]
    fn most_moves_still_cost_no_game_address() {
        let (mut field_write, mut by_name) = (0, 0);
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            for entry in &moveset.moves {
                if entry.prefix.is_field_write() {
                    field_write += 1;
                } else {
                    by_name += 1;
                }
            }
        }
        assert!(
            field_write > by_name,
            "{by_name} moves need PlayAnimationByBehaviorName against {field_write} that do not"
        );
    }

    #[test]
    fn only_the_event_prefix_is_a_field_write() {
        assert!(Prefix::Event.is_field_write());
        for other in [
            Prefix::Attack,
            Prefix::Step,
            Prefix::GuardAttack,
            Prefix::GoalAction,
            Prefix::RideAttack,
            Prefix::RideStep,
            Prefix::RiddenEnemyAttack,
            Prefix::RideEnemyAttack,
            Prefix::RiddenEnemyStep,
            Prefix::RideEnemyStep,
        ] {
            assert!(!other.is_field_write(), "{other:?}");
            assert!(other.name().starts_with("W_"));
        }
    }

    /// The omitted column means `W_Event`, which is the difference between a field write and a
    /// call -- so a four-field entry must never come back as anything else.
    #[test]
    fn an_absent_prefix_column_reads_as_the_field_write() {
        assert_eq!(parse_move("3000:0:0:1").unwrap().prefix, Prefix::Event);
        assert_eq!(parse_move("6000:3:0:0:2").unwrap().prefix, Prefix::Step);
        assert!(
            parse_move("6000:3:0:0:99").is_none(),
            "an unknown prefix code must be refused"
        );
    }

    /// How much of the shipped table has a real chain window, asserted rather than described.
    ///
    /// Measured on the corpus: 4,669 attacks, 4,576 of them (98.0%) carrying a TAE type-0
    /// `ChrActionFlag` FlagType-86 window, against 2,252 movement moves of which only 510 do.
    /// That split is the shape of the data and not a defect -- a dodge is not an attack, so it
    /// authors no attack-cancel window -- and it is exactly what the runtime falls back on: a
    /// press during a windowless move waits for the animation to end.
    ///
    /// The floor is what makes this a gate. If the generator ever reads FlagType 4 (the player
    /// combo flag, 0.3% of creature attack animations) instead of 86, or the wrong param index,
    /// the table still parses and every attack silently becomes uncancellable.
    /// A creature with no attacks is the failure this table is most likely to ship silently.
    ///
    /// It shipped exactly that way once. The generator joined the behaviour graph (what a creature
    /// can fire) against `<chr>.tae` (what the animation does) by chr id on both sides, and a
    /// third of the roster does not own its TimeAct: c4351 Godrick Knight's `.anibnd` is a
    /// skeleton and nothing else, because the whole 435x knight family plays out of `c4350.tae`.
    /// Those 133 creatures reached the classifier with an empty TimeAct, had every attack denied
    /// `no-damage-window`, and shipped as twelve `W_Step` walk clips with `denied=0` -- which
    /// reads, in the derived report and in the log, as a creature that simply has no attacks.
    /// Nothing went red. See `scripts/er-moveset-tae-owner.py` for the join that fixed it.
    ///
    /// So the counts are asserted. 25 creatures genuinely have no attack animations (Balloon
    /// Dummy, Talk Dummy, Ranni, the Two Fingers, an owl), 20 more have nothing fireable at all,
    /// and the numbers below leave room for a handful either way rather than pinning an exact
    /// figure a legitimate corpus change would break. What they will not survive is the join
    /// regressing: that puts a hundred and thirty creatures back in the attackless column at once.
    #[test]
    fn only_a_handful_of_creatures_in_the_shipped_table_have_no_attacks_at_all() {
        let mut attackless = Vec::new();
        let mut attacks = 0;
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            let mine = moveset
                .moves
                .iter()
                .filter(|entry| entry.bucket != Bucket::Movement)
                .count();
            attacks += mine;
            if mine == 0 {
                attackless.push(chr);
            }
        }
        assert!(
            attackless.len() <= 45,
            "{} creatures in the shipped table classify zero attacks: {attackless:?}. Above about \
             forty-five this is the TimeAct join failing, not the roster -- run \
             `scripts/er-moveset-coverage.py --check`, which names the cause per creature.",
            attackless.len()
        );
        assert!(
            attacks > 7000,
            "only {attacks} attacks in the whole table; the chr-id TimeAct join used to give 4669 \
             and the family-base join gives 7174"
        );
    }

    #[test]
    fn nearly_every_attack_in_the_shipped_table_carries_a_real_chain_window() {
        let mut attacks = 0;
        let mut windowed = 0;
        let mut movement = 0;
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            for entry in &moveset.moves {
                if entry.bucket == Bucket::Movement {
                    movement += 1;
                    continue;
                }
                attacks += 1;
                windowed += usize::from(entry.chain_from_cs.is_some());
            }
        }
        assert!(attacks > 4000, "only {attacks} attacks in the table");
        assert!(
            movement > 2000,
            "only {movement} movement moves in the table"
        );
        assert!(
            windowed * 100 >= attacks * 95,
            "only {windowed} of {attacks} attacks carry a chain window -- the generator is \
             reading the wrong ChrActionFlag FlagType (86 is the creature one, 4 is the player \
             one) or the wrong param index"
        );
    }

    /// A window that is longer than any animation in the game would mean the units are wrong --
    /// seconds written where centiseconds were meant, or the other way round. 25.53 s is the
    /// longest the corpus has; anything past a minute is a bug, not a boss.
    #[test]
    fn no_chain_window_is_longer_than_an_animation_could_plausibly_be() {
        for chr in chr_ids() {
            let Some(moveset) = lookup(chr) else { continue };
            for entry in &moveset.moves {
                let Some(from) = entry.chain_from_s() else {
                    continue;
                };
                assert!(
                    (0.0..60.0).contains(&from),
                    "c{chr} {} opens its chain window at {from} s",
                    entry.fire
                );
            }
        }
    }

    #[test]
    fn an_unknown_creature_has_no_moveset_rather_than_an_empty_one() {
        assert!(lookup(999_999).is_none());
    }
}
