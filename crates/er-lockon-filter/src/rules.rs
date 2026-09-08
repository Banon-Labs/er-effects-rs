//! Which characters this DLL hides from lock-on.
//!
//! There is no switch and no config file: loading the DLL is the feature, so the rule is two lists
//! of constants and one predicate over the three integers the hook reads out of live memory. It
//! lives here, free of `cfg(windows)`, so the host test run covers it -- both ways it can be wrong
//! are silent. Hiding
//! nobody looks exactly like an invasion where nobody else turned up; hiding the host looks like
//! the lock-on button being broken. Neither is visible until someone is mid-invasion.

use std::fmt::Write as _;

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

/// The three player invader kinds. Both halves of the rule read this one set: it says who is
/// hidden, and it says who you have to be for anyone to be hidden from you.
///
/// `Duelist` (2), `BloodyFingerNpc` (20) and `RecusantNpc` (21) are absent on purpose. A duelist
/// was summoned by the host and an npc invader is a character the game spawned, so neither is a
/// fellow player invader -- and both are people an invader may legitimately want to lock.
pub(crate) const INVADER_CHR_TYPES: [i32; 3] = [
    CHR_TYPE_BLOODY_FINGER,
    CHR_TYPE_RECUSANT,
    CHR_TYPE_FESTERING_BLOODY_FINGER,
];

/// [`INVADER_CHR_TYPES`] as the one word the hook consults.
pub(crate) const INVADERS: ChrTypeSet = ChrTypeSet::of(&INVADER_CHR_TYPES);

/// `SummonParamType::RedInvasionA` -- the Bloody Finger role.
pub(crate) const SUMMON_PARAM_TYPE_RED_INVASION_A: i32 = -3;
/// `SummonParamType::RedInvasionALimited` -- the Festering finger role.
pub(crate) const SUMMON_PARAM_TYPE_RED_INVASION_A_LIMITED: i32 = -4;
/// `SummonParamType::RedInvasionB` -- the Recusant role.
pub(crate) const SUMMON_PARAM_TYPE_RED_INVASION_B: i32 = -5;

/// The multiplayer roles that mean the local player is invading, as `GameMan::summonParamType`
/// spells them.
///
/// A second, independent answer to "am I invading", and the reason it is worth reading a second
/// field: `ChrType` is what the engine derives, and `summonParamType` is what it derives it from
/// (`CS::GameMan::GetSummonParamType` -> `MultiplayProperties` -> `PlayerGameData::SetChrType`).
/// Under Seamless Co-op, which runs its own session layer, a roster walk has been measured typing
/// remote players `Local`, so the derived kind is the half more likely to be surprising.
///
/// `Host` (0), `Summon` (-1) and `RedSummon` (-2) are absent for the same reason `Duelist` is
/// absent from [`INVADER_CHR_TYPES`]: a summoned red is not invading.
pub(crate) const INVADER_SUMMON_PARAM_TYPES: [i32; 3] = [
    SUMMON_PARAM_TYPE_RED_INVASION_A,
    SUMMON_PARAM_TYPE_RED_INVASION_A_LIMITED,
    SUMMON_PARAM_TYPE_RED_INVASION_B,
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
    INVADERS.contains(chr_type) || INVADER_SUMMON_PARAM_TYPES.contains(&summon_param_type)
}

/// Should the character typed `candidate_chr_type` be hidden from the lock-on candidate set of
/// the local player?
///
/// The caller has already established that the two are different characters. This decides nothing
/// about the game's own targeting rules: a candidate it leaves alone still has to pass
/// `CS::ChrIns::CanTargetTeamType` and the distance and angle tests that follow it.
pub(crate) fn hides(
    self_chr_type: i32,
    self_summon_param_type: i32,
    candidate_chr_type: i32,
) -> bool {
    local_player_is_invading(self_chr_type, self_summon_param_type)
        && INVADERS.contains(candidate_chr_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The value the runtime passes when `GameMan` answered, and said "not invading".
    const HOST: i32 = 0;

    #[test]
    fn an_invader_stops_seeing_invaders() {
        for local in INVADER_CHR_TYPES {
            for candidate in INVADER_CHR_TYPES {
                assert!(
                    hides(local, SUMMON_PARAM_TYPE_UNKNOWN, candidate),
                    "{local}/{candidate}"
                );
            }
        }
    }

    #[test]
    fn the_summon_param_type_alone_is_enough_to_be_invading() {
        // The case this second signal exists for: a session layer that leaves the derived
        // `ChrType` at `Local` while the role the engine matched on says otherwise.
        for role in INVADER_SUMMON_PARAM_TYPES {
            assert!(hides(0, role, CHR_TYPE_BLOODY_FINGER), "{role}");
        }
    }

    #[test]
    fn the_chr_type_alone_is_enough_to_be_invading() {
        assert!(hides(CHR_TYPE_RECUSANT, HOST, CHR_TYPE_BLOODY_FINGER));
    }

    #[test]
    fn an_invader_still_sees_the_host_and_their_phantoms() {
        // 0 Local, 1 WhitePhantom, 8 GrayPhantom, 17 BluePhantom: the people an invader is there
        // to fight, and the ones a filter that hid them would look like a broken lock-on button.
        for candidate in [0, 1, 8, 17] {
            assert!(
                !hides(
                    CHR_TYPE_BLOODY_FINGER,
                    SUMMON_PARAM_TYPE_RED_INVASION_A,
                    candidate
                ),
                "{candidate}"
            );
        }
    }

    #[test]
    fn an_invader_still_sees_summoned_reds_and_npc_invaders() {
        // 2 Duelist, 20 BloodyFingerNpc, 21 RecusantNpc: reds, but not fellow player invaders.
        for candidate in [2, 20, 21] {
            assert!(
                !hides(
                    CHR_TYPE_BLOODY_FINGER,
                    SUMMON_PARAM_TYPE_RED_INVASION_A,
                    candidate
                ),
                "{candidate}"
            );
        }
    }

    #[test]
    fn anyone_who_is_not_invading_is_unaffected() {
        // Every non-invader kind, paired with the roles that are not an invasion: host, an
        // ordinary summon, a summoned red, and no answer at all.
        for local in [0, 1, 2, 5, 8, 13, 17] {
            for role in [HOST, -1, -2, SUMMON_PARAM_TYPE_UNKNOWN] {
                assert!(
                    !hides(local, role, CHR_TYPE_BLOODY_FINGER),
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
            assert!(!INVADERS.contains(chr_type), "{chr_type}");
            assert!(!hides(chr_type, HOST, CHR_TYPE_BLOODY_FINGER), "{chr_type}");
            assert!(
                !hides(
                    CHR_TYPE_BLOODY_FINGER,
                    SUMMON_PARAM_TYPE_RED_INVASION_A,
                    chr_type
                ),
                "{chr_type}"
            );
        }
    }

    #[test]
    fn the_set_holds_exactly_the_three_invader_kinds() {
        assert_eq!(INVADERS.describe(), "15, 16, 18");
    }
}
