//! Which characters count as players, and a tally of what a roster walk actually saw.
//!
//! Deliberately not inside [`crate::game`], which is `#[cfg(windows)]` and therefore never
//! compiled by `cargo test`. The classification below is a list of integers that decides whether
//! anything is drawn at all, and a wrong entry fails in the two worst ways available: silently
//! drawing nothing, or drawing a line to a bloodstain. Putting it here is what lets the host test
//! it without a game.

/// `ChrType` values that are emphatically not another player.
///
/// Everything the game spawns from a map is `Npc`; the ghost kinds are the translucent replays of
/// bloodstains, messages and other people's bonfire animations, which have positions and no
/// bodies.
///
/// The list is what gets excluded rather than what gets included, on purpose. An allow-list of the
/// phantom types this workspace knows about would silently draw nothing at all if Seamless typed
/// its remote players as something not on it, and "nothing drawn" is indistinguishable from "the
/// navmesh found no route". Failing towards drawing an extra character is a visible, correctable
/// mistake; failing towards drawing nobody is the one that wastes an invasion.
///
/// `Local` (0) is absent for the same reason. The local player is rejected by address, which is
/// exact; excluding the type as well would drop a remote player in any session that types them
/// `Local`, which is precisely the possibility the wide sweep exists to survive.
const NON_PLAYER_CHR_TYPES: [i32; 7] = [
    -1, // None
    3,  // Ghost
    4,  // Ghost1
    5,  // Npc
    10, // BloodstainGhost
    11, // BonfireGhost
    14, // MessageGhost
];

/// `ChrType::Npc`, the ordinary map character.
pub(crate) const NPC_CHR_TYPE: i32 = 5;

/// Is this character kind one a route should be drawn to, given that it was found in
/// `player_chr_set`?
///
/// Membership in the player set is itself the evidence. The engine puts players there, so a type
/// this build does not recognise, found in that set, is far more likely to be a session kind
/// nobody here has seen than a mistake -- hence the exclusion list, which fails towards drawing.
pub(crate) fn is_player_kind(chr_type: i32) -> bool {
    !NON_PLAYER_CHR_TYPES.contains(&chr_type)
}

/// `ChrType` values the engine names as a real person: the local kind, the phantom kinds, the
/// invader kinds, the arena kind.
///
/// `WhiteSummonNpc` (19), `BloodyFingerNpc` (20) and `RecusantNpc` (21) are absent on purpose --
/// they are the NPC invaders, which are characters the game spawns, not people in the session.
const NAMED_PLAYER_CHR_TYPES: [i32; 9] = [
    0,  // Local
    1,  // WhitePhantom
    2,  // Duelist
    8,  // GrayPhantom
    13, // Arena
    15, // BloodyFinger
    16, // Recusant
    17, // BluePhantom
    18, // FesteringBloodyFinger
];

/// Is this character kind one a route should be drawn to, given that it was found by sweeping
/// every ChrSet in the world?
///
/// Strictly an allow-list, and the asymmetry with [`is_player_kind`] is the whole point. The wide
/// sweep exists only as a fallback for a session that puts players somewhere unexpected, and it
/// walks the map's own character sets -- hundreds of them -- where an unfamiliar type is evidence
/// of nothing except that the map contains something this build has not catalogued.
///
/// Measured live on 2026-08-25, in a Seamless session, outside any invasion: the sweep saw
/// `types=[0:1 5:582 7:1]` every tick. `ChrType 7` is `Unk7`, one of the enum's unnamed values;
/// exactly one exists, it sits among the map's NPCs rather than in `player_chr_set`, and it
/// persists whether or not anyone else is in the session. Under the exclusion list it read as a
/// player and was drawn a "no walkable route" arrow out of the player's body -- permanently,
/// outside any invasion. That same run also showed the real people arriving exactly where the
/// engine documents (`sets=1 widened=false types=[2:1 0:3]`, a Duelist and three Locals in
/// `player_chr_set`), which is what makes fail-closed affordable here: the fallback keeps its
/// reason to exist, and stops inventing players out of map furniture.
pub(crate) fn is_named_player_kind(chr_type: i32) -> bool {
    NAMED_PLAYER_CHR_TYPES.contains(&chr_type)
}

/// Distinct `chr_type` values the census will name before it stops adding new ones.
const MAX_CENSUS_TYPES: usize = 16;

/// What a roster walk saw, so a frame that draws nothing can say why.
///
/// Without this, an invasion that produces no line has three indistinguishable explanations: the
/// roster found nobody, the navmesh refused every request, or the projection dropped every
/// segment. Only the first is answered here, and it is answered by counting rather than by
/// inferring afterwards.
#[derive(Default)]
pub(crate) struct Census {
    /// ChrSets walked, including the inline player set.
    pub(crate) sets: usize,
    /// Characters seen across all of them, live or not.
    pub(crate) characters: usize,
    /// `(chr_type, count)` for every distinct type seen, so an unexpected typing shows up as a
    /// number rather than as silence.
    pub(crate) by_chr_type: Vec<(i32, usize)>,
    /// True when `player_chr_set` yielded nobody and the wider sweep had to run.
    pub(crate) widened: bool,
}

impl Census {
    /// Record one character of `chr_type`.
    pub(crate) fn count(&mut self, chr_type: i32) {
        self.characters += 1;
        if let Some((_, count)) = self
            .by_chr_type
            .iter_mut()
            .find(|(seen, _)| *seen == chr_type)
        {
            *count += 1;
        } else if self.by_chr_type.len() < MAX_CENSUS_TYPES {
            // Bounded: a distinct-value list built from live memory is only as short as that
            // memory is sane, and a log line is not worth an unbounded allocation.
            self.by_chr_type.push((chr_type, 1));
        }
    }
}

impl std::fmt::Display for Census {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "sets={} characters={} widened={} types=[",
            self.sets, self.characters, self.widened
        )?;
        for (index, (chr_type, count)) in self.by_chr_type.iter().enumerate() {
            if index > 0 {
                write!(f, " ")?;
            }
            write!(f, "{chr_type}:{count}")?;
        }
        write!(f, "]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The named phantom and invader kinds from `ChrType`, every one of which is a real person.
    const PLAYER_KINDS: [i32; 9] = [
        1,  // WhitePhantom
        2,  // Duelist
        8,  // GrayPhantom
        13, // Arena
        15, // BloodyFinger
        16, // Recusant
        17, // BluePhantom
        18, // FesteringBloodyFinger
        0,  // Local -- a remote player typed as one must still be reachable
    ];

    #[test]
    fn every_named_player_kind_is_drawn_to() {
        for kind in PLAYER_KINDS {
            assert!(is_player_kind(kind), "chr_type {kind} should be a target");
        }
    }

    #[test]
    fn map_characters_and_ghosts_are_not_drawn_to() {
        for kind in [NPC_CHR_TYPE, -1, 3, 4, 10, 11, 14] {
            assert!(
                !is_player_kind(kind),
                "chr_type {kind} should not be a target"
            );
        }
    }

    /// The whole point of an exclusion list rather than an allow-list: a value this build has
    /// never seen is a player, not a silent nothing. If Seamless types its remote players 23, the
    /// line still draws.
    #[test]
    fn an_unrecognised_kind_is_treated_as_a_player_rather_than_dropped() {
        for kind in [19, 20, 21, 22, 23, 99, i32::MAX] {
            assert!(
                is_player_kind(kind),
                "an unknown chr_type {kind} must fail towards drawing"
            );
        }
    }

    #[test]
    fn the_local_type_is_not_excluded_because_the_local_player_is_rejected_by_address() {
        const LOCAL: i32 = 0;
        assert!(is_player_kind(LOCAL));
    }

    /// The live regression, named after what it did: outside any invasion, the wide sweep saw
    /// `types=[0:1 5:582 7:1]` and drew a permanent arrow at the one `Unk7` character sitting
    /// among the map's NPCs.
    #[test]
    fn the_unnamed_chr_types_are_not_players_when_found_by_the_wide_sweep() {
        for kind in [6, 7, 9, 12, 22, 23, 99, i32::MAX] {
            assert!(
                !is_named_player_kind(kind),
                "wide-sweep chr_type {kind} must not be drawn to"
            );
        }
    }

    #[test]
    fn every_named_player_kind_survives_the_wide_sweep() {
        for kind in PLAYER_KINDS {
            assert!(
                is_named_player_kind(kind),
                "wide-sweep chr_type {kind} should be a target"
            );
        }
    }

    /// The NPC invaders wear invader-shaped types but are characters the game spawns, not people.
    #[test]
    fn npc_invaders_are_not_swept_up_as_players() {
        for kind in [19, 20, 21] {
            assert!(!is_named_player_kind(kind));
        }
    }

    /// The asymmetry is the fix. An unknown type is trusted where the engine keeps players and
    /// refused where the map keeps its own characters; collapsing the two either re-opens the
    /// false arrow or throws away the fallback's reason to exist.
    #[test]
    fn an_unknown_type_is_trusted_in_the_player_set_and_refused_in_the_wide_sweep() {
        const UNKNOWN: i32 = 7;
        assert!(is_player_kind(UNKNOWN));
        assert!(!is_named_player_kind(UNKNOWN));
    }

    #[test]
    fn a_census_tallies_each_kind_once_and_totals_them_all() {
        let mut census = Census::default();
        for kind in [5, 5, 5, 1, 16, 1] {
            census.count(kind);
        }
        assert_eq!(census.characters, 6);
        assert_eq!(census.by_chr_type.len(), 3);
        let npcs = census
            .by_chr_type
            .iter()
            .find(|(kind, _)| *kind == 5)
            .map(|(_, count)| *count);
        assert_eq!(npcs, Some(3));
    }

    /// Garbage memory must not turn a diagnostic into an allocation loop.
    #[test]
    fn a_census_stops_naming_new_kinds_once_it_has_enough() {
        let mut census = Census::default();
        for kind in 0..1000 {
            census.count(kind);
        }
        assert_eq!(census.characters, 1000);
        assert_eq!(census.by_chr_type.len(), MAX_CENSUS_TYPES);
    }

    #[test]
    fn a_census_reads_as_one_line() {
        let mut census = Census {
            sets: 2,
            widened: true,
            ..Census::default()
        };
        census.count(5);
        census.count(1);
        assert_eq!(
            census.to_string(),
            "sets=2 characters=2 widened=true types=[5:1 1:1]"
        );
    }
}
