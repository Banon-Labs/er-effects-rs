//! Which characters this DLL hides from lock-on.
//!
//! There is no switch and no config file: loading the DLL is the feature, so the rule is two sets
//! of constants and one predicate over the three integers the hook reads out of live memory. It
//! lives here, free of `cfg(windows)`, so the host test run covers it -- both ways it can be wrong
//! are silent. Hiding
//! nobody looks exactly like an invasion where nobody else turned up; hiding the host looks like
//! the lock-on button being broken. Neither is visible until someone is mid-invasion.
//!
//! # Both sets are read out of the game, not written from the wiki
//!
//! The first version of this file listed the three invasion items' `ChrType` values and their
//! three `SummonParamType` values, and that list was narrower than the game's own answer in a way
//! that cost the whole feature: a live Seamless Co-op session measured the local player at
//! `chrType 2` / `summonParamType -12`, and neither was a member, so the filter could never arm.
//! Both sets are now derived from the two tables the engine itself consults --
//! `CharacterTypeProperties` and `MultiplayProperties` -- which is why `Duelist` and the
//! map-guardian roles are members without anyone having had to think of them.
//! `scripts/er-character-type-tables.py` prints both tables and re-derives both sets.

use std::fmt::Write as _;

/// `ChrType::Duelist` -- a red summoned into the host's world, and an ally of the invaders
/// rather than of the host. The game classifies it as a hostile phantom; so does this crate.
pub(crate) const CHR_TYPE_DUELIST: i32 = 2;
/// `ChrType::BloodyFinger` -- an ordinary invader.
pub(crate) const CHR_TYPE_BLOODY_FINGER: i32 = 15;
/// `ChrType::Recusant` -- a Volcano Manor invader.
pub(crate) const CHR_TYPE_RECUSANT: i32 = 16;
/// `ChrType::FesteringBloodyFinger` -- the Taunter's Tongue / Festering finger invader.
pub(crate) const CHR_TYPE_FESTERING_BLOODY_FINGER: i32 = 18;

/// Highest `ChrType` a [`ChrTypeSet`] can hold. The enum names 0..=22, so a `u32` covers it with
/// room to spare, and a value outside the range is a character kind this build has never seen
/// rather than one to test against.
pub(crate) const MAX_CHR_TYPE: i32 = 31;

/// The `ChrType` values the game itself calls hostile phantoms, less the kinds it spawns.
///
/// Not a hand-written list of the invasion items. Elden Ring keeps a
/// `CharacterTypeProperties` table -- 23 records of 20 bytes at 1.16.2 `0x143b17c00`, and
/// byte-identical at 1.17.1 `0x143b1bc00` -- whose `isHostilePhantom` byte
/// `CS::CharacterTypeProperties::IsHostilePhantom` (1.16.2 `0x1404c7d10`) reads at record
/// `+0xa`. That table answers `true` for `2, 15, 16, 18, 20, 21, 22`, and
/// `scripts/er-character-type-tables.py --selftest` re-reads it rather than trusting this
/// sentence.
///
/// Two edits to that set, both deliberate:
///
/// * `20 BloodyFingerNpc`, `21 RecusantNpc` and `22` are dropped. They are characters the game
///   spawned, not other humans, and an invader may legitimately want to lock one.
/// * `2 Duelist` is kept, and this is the correction that matters. The previous list here had
///   three entries and excluded it on the reasoning that "a duelist was summoned by the host".
///   The game's own row disagrees, and so does the role table: `MultiplayProperties` role 2 is
///   `赤召喚`, a red summon, which fights beside the invaders. It is also the value the live
///   census has actually measured on the local player under Seamless Co-op.
pub(crate) const HOSTILE_PHANTOM_CHR_TYPES: [i32; 4] = [
    CHR_TYPE_DUELIST,
    CHR_TYPE_BLOODY_FINGER,
    CHR_TYPE_RECUSANT,
    CHR_TYPE_FESTERING_BLOODY_FINGER,
];

/// [`HOSTILE_PHANTOM_CHR_TYPES`] as the one word the hook consults.
pub(crate) const HOSTILE_PHANTOMS: ChrTypeSet = ChrTypeSet::of(&HOSTILE_PHANTOM_CHR_TYPES);

/// `SummonParamType::RedInvasionA` -- the Bloody Finger role.
pub(crate) const SUMMON_PARAM_TYPE_RED_INVASION_A: i32 = -3;
/// `SummonParamType::RedInvasionALimited` -- the Festering finger role.
pub(crate) const SUMMON_PARAM_TYPE_RED_INVASION_A_LIMITED: i32 = -4;
/// `SummonParamType::RedInvasionB` -- the Recusant role.
pub(crate) const SUMMON_PARAM_TYPE_RED_INVASION_B: i32 = -5;
/// The role the live census measured on the local player in a Seamless Co-op session:
/// `MultiplayProperties` role 12, debug name `アノールマップ守護`, whose `CharacterType` is
/// `Duelist`. Named because it is the single value that decides whether this crate does
/// anything at all in the sessions the user actually plays.
pub(crate) const SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN: i32 = -12;

/// Every `SummonParamType` whose multiplayer role resolves to a hostile-phantom `CharacterType`.
///
/// Derived, like [`HOSTILE_PHANTOM_CHR_TYPES`], from a game table rather than from the item that
/// starts the invasion: `MultiplayProperties` (32 records of 64 bytes, 1.16.2 `0x143b11230`,
/// 1.17.1 `0x143b15230`, walked by `GetMultiplayPropertiesByMultiplayRole` at 1.16.2
/// `0x1401db340`) carries both the `SummonParamType` the engine matched the session on and the
/// `CharacterType` it derives from it. Every row landing on a hostile phantom is here, which is
/// what makes the map-guardian and red-sign roles members without anyone having to think of them
/// one at a time.
///
/// `Host` (0), `Summon` (-1) and the hunter roles are absent because their rows resolve to
/// `Local`, `WhitePhantom` or `BluePhantom`, not because a person judged them friendly.
pub(crate) const HOSTILE_PHANTOM_SUMMON_PARAM_TYPES: [i32; 19] = [
    -2,
    SUMMON_PARAM_TYPE_RED_INVASION_A,
    SUMMON_PARAM_TYPE_RED_INVASION_A_LIMITED,
    SUMMON_PARAM_TYPE_RED_INVASION_B,
    -8,
    -10,
    -11,
    SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN,
    -16,
    -17,
    -18,
    -19,
    -21,
    -22,
    -23,
    -25,
    -26,
    -29,
    -30,
];

/// The value the hook passes when it has no `GameMan` to read, so the summon-param half stands
/// down and the `ChrType` half decides alone. `Host` would do the same job and would be a lie
/// about what was read.
pub(crate) const SUMMON_PARAM_TYPE_UNKNOWN: i32 = i32::MIN;

/// A set of `ChrType` values, as one word.
///
/// A bitmask rather than a slice search because the hook consults it on the game thread, once per
/// lock-on point per frame, and because it lets the whole rule be a compile-time constant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChrTypeSet(u32);

impl ChrTypeSet {
    /// The set of `types`, dropping any that is outside the representable range.
    pub(crate) const fn of(types: &[i32]) -> Self {
        let mut bits = 0_u32;
        let mut index = 0;
        while index < types.len() {
            let chr_type = types[index];
            if chr_type >= 0 && chr_type <= MAX_CHR_TYPE {
                bits |= 1 << chr_type;
            }
            index += 1;
        }
        Self(bits)
    }

    /// Membership. A `chr_type` outside the representable range is never a member, which is what
    /// makes this safe to feed straight from live memory: `ChrIns::chr_type` is read as the raw
    /// `i32` the field holds, and a session that types a character in a way this build has never
    /// seen must answer "not in the set" rather than index anything.
    pub(crate) const fn contains(self, chr_type: i32) -> bool {
        chr_type >= 0 && chr_type <= MAX_CHR_TYPE && self.0 & (1 << chr_type) != 0
    }

    /// The members, low to high, for a log line.
    pub(crate) fn describe(self) -> String {
        let mut out = String::new();
        for chr_type in 0..=MAX_CHR_TYPE {
            if self.contains(chr_type) {
                if !out.is_empty() {
                    out.push_str(", ");
                }
                let _ = write!(out, "{chr_type}");
            }
        }
        out
    }
}

/// Is the local player invading, according to either of the two fields that can say so?
///
/// Either is enough. The two agree in vanilla -- one is derived from the other -- so the only
/// case where they differ is one where a session layer has written one and not the other, and
/// there the useful answer is the one that says yes. Widening this alone hides nobody: the
/// candidate still has to be typed as an invader, so a false yes here costs nothing while a false
/// no costs the whole feature.
pub(crate) fn local_player_is_invading(chr_type: i32, summon_param_type: i32) -> bool {
    HOSTILE_PHANTOMS.contains(chr_type)
        || HOSTILE_PHANTOM_SUMMON_PARAM_TYPES.contains(&summon_param_type)
}

/// Should the character typed `candidate_chr_type` be hidden from the lock-on candidate set of
/// the local player?
///
/// The caller has already established that the two are different characters. This decides nothing
/// about the game's own targeting rules: a candidate it leaves alone still has to pass
/// `CS::ChrIns::CanTargetTeamType` and the distance and angle tests that follow it.
///
/// # Why a non-player candidate is never hidden
///
/// `candidate_is_player` is the runtime's answer to "is this a `CS::PlayerIns`", and it is a hard
/// precondition rather than one more term in the disjunction. The feature is about not locking on
/// to a fellow invader, so an `EnemyIns` is out of scope by definition -- but the sets this rule
/// tests are `chr_type` numbers, and nothing stops the game giving a non-player character a number
/// the hostile-phantom set holds. Without this term such a character would be taken off the lock-on
/// list, which is an ordinary enemy becoming untargetable: a far worse fault than the one the
/// filter exists to fix, and one that would show up in normal play rather than only during an
/// invasion.
pub(crate) fn hides(
    self_chr_type: i32,
    self_summon_param_type: i32,
    candidate_chr_type: i32,
    candidate_is_player: bool,
) -> bool {
    candidate_is_player
        && local_player_is_invading(self_chr_type, self_summon_param_type)
        && HOSTILE_PHANTOMS.contains(candidate_chr_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value the runtime passes for a candidate that is a `CS::PlayerIns`. Every test below
    /// that is about the invasion rule passes it, because the rule only ever applies to players;
    /// the tests that pass `false` are the ones asserting that.
    const PLAYER: bool = true;

    /// A `CS::EnemyIns` is out of scope whatever number it carries.
    ///
    /// This is the fault the precondition exists to refuse, and it is worth a test of its own
    /// because it fires in ordinary play rather than during an invasion: without the term, an
    /// invader walking past any non-player character the game happened to type 2, 15, 16 or 18
    /// would find it silently missing from the lock-on list.
    #[test]
    fn a_non_player_candidate_is_never_hidden() {
        const NOT_A_PLAYER: bool = false;
        for local in HOSTILE_PHANTOM_CHR_TYPES {
            for role in HOSTILE_PHANTOM_SUMMON_PARAM_TYPES {
                for candidate in HOSTILE_PHANTOM_CHR_TYPES {
                    assert!(
                        !hides(local, role, candidate, NOT_A_PLAYER),
                        "{local}/{role}/{candidate}"
                    );
                }
            }
        }
    }

    /// The precondition is a gate, not a reason on its own: being a player does not hide anyone
    /// the rest of the rule leaves alone.
    #[test]
    fn being_a_player_is_not_by_itself_grounds_to_hide() {
        assert!(!hides(CHR_TYPE_BLOODY_FINGER, HOST, HOST, PLAYER));
    }

    /// The value the runtime passes when `GameMan` answered, and said "not invading".
    const HOST: i32 = 0;

    #[test]
    fn a_hostile_phantom_stops_seeing_hostile_phantoms() {
        for local in HOSTILE_PHANTOM_CHR_TYPES {
            for candidate in HOSTILE_PHANTOM_CHR_TYPES {
                assert!(
                    hides(local, SUMMON_PARAM_TYPE_UNKNOWN, candidate, PLAYER),
                    "{local}/{candidate}"
                );
            }
        }
    }

    #[test]
    fn the_summon_param_type_alone_is_enough_to_be_invading() {
        // The case this second signal exists for: a session layer that leaves the derived
        // `ChrType` at `Local` while the role the engine matched on says otherwise.
        for role in HOSTILE_PHANTOM_SUMMON_PARAM_TYPES {
            assert!(hides(0, role, CHR_TYPE_BLOODY_FINGER, PLAYER), "{role}");
        }
    }

    #[test]
    fn the_chr_type_alone_is_enough_to_be_invading() {
        assert!(hides(
            CHR_TYPE_RECUSANT,
            HOST,
            CHR_TYPE_BLOODY_FINGER,
            PLAYER
        ));
    }

    /// The pairing a live Seamless Co-op session measured on the local player, and the one the
    /// previous three-entry lists could not express. Either field alone has to arm the gate.
    #[test]
    fn the_seamless_measured_pairing_reads_as_invading() {
        assert!(local_player_is_invading(
            CHR_TYPE_DUELIST,
            SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN
        ));
        assert!(local_player_is_invading(
            CHR_TYPE_DUELIST,
            SUMMON_PARAM_TYPE_UNKNOWN
        ));
        assert!(local_player_is_invading(
            0,
            SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN
        ));
        assert!(hides(
            CHR_TYPE_DUELIST,
            SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN,
            CHR_TYPE_DUELIST,
            PLAYER,
        ));
    }

    #[test]
    fn an_invader_still_sees_the_host_and_their_phantoms() {
        // 0 Local, 1 WhitePhantom, 8 GrayPhantom, 17 BluePhantom: the people an invader is there
        // to fight, and the ones a filter that hid them would look like a broken lock-on button.
        // The host reading 0 is measured, not assumed, which is what keeps the candidate half
        // strict no matter how wide the invading half gets.
        for candidate in [0, 1, 8, 17] {
            assert!(
                !hides(
                    CHR_TYPE_BLOODY_FINGER,
                    SUMMON_PARAM_TYPE_RED_INVASION_A,
                    candidate,
                    PLAYER,
                ),
                "{candidate}"
            );
        }
    }

    #[test]
    fn an_invader_still_sees_npc_invaders() {
        // 20 BloodyFingerNpc, 21 RecusantNpc, 22: the game's own table calls all three hostile
        // phantoms, and all three are dropped from the candidate set on purpose -- they are
        // characters the game spawned, not other humans.
        for candidate in [20, 21, 22] {
            assert!(
                !hides(
                    CHR_TYPE_BLOODY_FINGER,
                    SUMMON_PARAM_TYPE_RED_INVASION_A,
                    candidate,
                    PLAYER,
                ),
                "{candidate}"
            );
        }
    }

    #[test]
    fn anyone_who_is_not_a_hostile_phantom_is_unaffected() {
        // Every non-hostile kind, paired with the roles that resolve to one: host, an ordinary
        // summon, and no answer at all.
        for local in [0, 1, 5, 8, 13, 17] {
            for role in [HOST, -1, SUMMON_PARAM_TYPE_UNKNOWN] {
                assert!(
                    !hides(local, role, CHR_TYPE_BLOODY_FINGER, PLAYER),
                    "{local}/{role}"
                );
            }
        }
    }

    #[test]
    fn an_unknown_summon_param_type_is_not_an_invasion_by_itself() {
        assert!(!local_player_is_invading(0, SUMMON_PARAM_TYPE_UNKNOWN));
    }

    #[test]
    fn an_unrepresentable_chr_type_is_never_a_member() {
        for chr_type in [-1, -999, MAX_CHR_TYPE + 1, i32::MAX, i32::MIN] {
            assert!(!HOSTILE_PHANTOMS.contains(chr_type), "{chr_type}");
            assert!(
                !hides(chr_type, HOST, CHR_TYPE_BLOODY_FINGER, PLAYER),
                "{chr_type}"
            );
            assert!(
                !hides(
                    CHR_TYPE_BLOODY_FINGER,
                    SUMMON_PARAM_TYPE_RED_INVASION_A,
                    chr_type,
                    PLAYER,
                ),
                "{chr_type}"
            );
        }
    }

    /// The set the game's `CharacterTypeProperties` table gives, less the three kinds the game
    /// spawns. `scripts/er-character-type-tables.py --selftest` is the half of this that reads
    /// the image; this half pins what the crate did with the answer.
    #[test]
    fn the_set_is_the_games_hostile_phantoms_less_the_npc_kinds() {
        assert_eq!(HOSTILE_PHANTOMS.describe(), "2, 15, 16, 18");
    }

    /// The whole of the 2026-09-07 Seamless run, replayed: the gate now arms on the local
    /// player's measured pairing, and still hides nobody, because every candidate that session
    /// asked about was `Local` (0), `Npc` (5) or `Unk7` (7). Widening the invading half is only
    /// safe while that stays true, so the run is a test rather than a paragraph.
    #[test]
    fn the_measured_seamless_session_hides_nobody() {
        for (local, role) in [
            (0, 0),
            (2, SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN),
            (2, 0),
            (0, SUMMON_PARAM_TYPE_ANOR_MAP_GUARDIAN),
        ] {
            for candidate in [0, 5, 7] {
                assert!(
                    !hides(local, role, candidate, PLAYER),
                    "{local}/{role}/{candidate}"
                );
            }
        }
    }

    /// A role table row that resolves to a friendly or neutral kind must not arm the gate, or
    /// the crate would hide invaders from a blue hunter and from a co-op phantom.
    #[test]
    fn the_hunter_and_summon_roles_are_not_invading() {
        // -1 Summon (WhitePhantom), -9 red hunter (BluePhantom), -14 battle royale, -28 red
        // hunter 2 (BluePhantom), 0 an invalid sign.
        for role in [0, -1, -9, -14, -28] {
            assert!(!local_player_is_invading(0, role), "{role}");
        }
    }
}
