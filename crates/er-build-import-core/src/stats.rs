//! The build's stat block: the eight attributes, the class floor under them, and the level they
//! imply.
//!
//! # The eight attributes are the build
//!
//! The payload's `rl` is not. Nothing downstream reads it, every stat the importer applies comes
//! from the attributes themselves, and for every starting class the eight sum to `level + 79` --
//! so the level is derived here rather than trusted, and a planner that disagrees with its own
//! numbers is reported, not obeyed.
//!
//! That derivation used to be a hard refusal, and it rejected a real build over one point: a
//! payload carrying `rl: 150` beside attributes summing to 228 (= level 149) failed with
//! "internally inconsistent" and imported nothing at all. The observation was correct and the
//! response was not -- the gear, spells, talismans and the attributes were all perfectly
//! well-formed.
//!
//! # And the claim is overwritten, not merely reported
//!
//! `character::apply_stats` fills `stats[STAT_LEVEL]` from `rl`. So a planner link claiming
//! `rl: 150` beside attributes summing to 226 stamped a character with `level == 150` and a stat
//! block implying 147. That is not cosmetic: `er_save_loader::stats` locates a save slot's
//! serialized `PlayerGameData` by the identity `level == sum(attrs) - 79`, so a character minted
//! with a contradictory level was one the mod's own save reader could no longer find -- on the
//! live default container it decoded 9 of 10 slots, and the Load Character row for the missing one
//! rendered a name with an empty attribute line and no `WL` (user-reported 2026-09-01).
//!
//! Normalising here rather than in the applier keeps one derivation: everything downstream -- the
//! applier, the read-back check, the report -- reads a [`BuildDoc`] whose `rl` and attributes
//! agree, and no second copy of `- 79` can drift from this one.
//!
//! # The class floor (user-reported 2026-09-06)
//!
//! > *"Its possible for nyasu builds to have stats set below their level, or below their base
//! > class. Vagabond has base 14 str, a build with 12 str saved, even at level 150 will set the Str
//! > to 12 and the level to 148, making the base class invalid."*
//!
//! Vagabond's strength base really is 14. [`class::STARTING_STATS`] is `CharaInitParam` row
//! `3000 + archetype`, re-read out of the installed `regulation.bin` rather than transcribed.
//!
//! A payload naming Vagabond with `str: 12` therefore describes a character no ordinary play
//! session could produce. Taken literally its eight attributes summed two short, the level came
//! out 148 instead of the 150 the payload claimed, and the importer stamped a character two levels
//! poorer than the build asked for, holding a strength its own class contradicts.
//!
//! Raising each attribute to its class base before the sum fixes both at once, and the arithmetic
//! is the evidence that this is the planner's own intent rather than a guess: putting the two
//! missing points back restores the total to 229, and `229 - 79` is exactly the `rl: 150` the
//! payload claimed. The planner computes its level with the floor applied while exporting the raw
//! stat; the two agree again the moment the importer applies the same floor. All three archived
//! payloads in `tests/fixtures/` corroborate it from the other side -- every attribute the author
//! never spent sits *exactly* on its class base (the Vagabond build's `int 9, fth 9, arc 7, mnd
//! 10` are Vagabond's bases to the point), which is what a planner enforcing a floor produces.
//!
//! ## What the binary shows, and what it does not
//!
//! Measured against the 1.16.2 named Ghidra image (shift zero, so the addresses are also the
//! runtime's):
//!
//! * `ArchetypeToInitParamId` @ `0x140d34bf0` is literally `return archetype + 3000`, so the row
//!   this table indexes is the row the game indexes.
//! * `0x1407c6560` reads `PlayerGameData::archetype`, calls `ArchetypeToInitParamId` and
//!   `GetCharaInitParam`, and fills a ten-slot array from the row -- `soulLv`, `baseVit`,
//!   `baseEnd`, `baseWil`, `baseStr`, `baseDex`, `baseDurability`, `baseMag`, `baseFai`,
//!   `baseLuc`. That is the same ten-slot layout `GetMainPlayerStats`/`ApplyMainPlayerStats` use.
//! * `CS::PlayerLevelUpDialog::PlayerLevelUpDialog` @ `0x14096daa0` keeps that array and builds a
//!   **second** `CS::LevelPlayerStatusSimulator` from it, held separately from the live-stats
//!   simulator that `0x1407c6440` populates from `GetMainPlayerStats`. A dedicated base-stats
//!   object beside the live one is the shape a floor has.
//!
//! **Not confirmed**: the literal compare-and-reject instruction. It is presumably in a virtual
//! Update/input method of that dialog class, which was not reached. So the floor is evidenced
//! structurally and by the planner's arithmetic, not by a decompiled clamp.
//!
//! **Confirmed, and it is why this module has to do the work**: `ApplyMainPlayerStats` @
//! `0x140788cf0` -- the native the importer calls -- does **not** clamp. It writes all ten ints
//! straight into `PlayerGameData` with no comparison against `CharaInitParam` or anything else;
//! its only clamps are on the derived HP/FP/stamina deltas. Whatever the level-up UI enforces, it
//! is a UI-layer restriction that this code path goes around. Nothing downstream will catch a
//! sub-base attribute, so the floor is applied here or not at all.
//!
//! # When the build names no class
//!
//! Then there is no floor, and none is invented. [`Floor::Unnamed`] is reported and the attributes
//! are taken exactly as given. Substituting a default class would be a guess that silently adds
//! levels to a build, and the failure it causes -- a Wretch build quietly raised to Vagabond
//! bases -- is worse and harder to see than the one it would paper over. The same applies to a
//! class name this table does not recognise ([`Floor::Unrecognised`]), which on past evidence
//! means the game grew a class and `class::STARTING_CLASSES` has not caught up.

use crate::class;
use crate::model::BuildDoc;

/// Every starting class satisfies `stat_sum - level == 79`; it is a property of the game, not of
/// any one class, so the level follows from the attributes alone.
///
/// `class::tests::every_starting_level_is_the_attribute_total_less_seventy_nine` checks that
/// against all twelve `CharaInitParam` rows, and this is the constant it checks.
pub const CLASS_INVARIANT: i64 = 79;

/// All eight attributes at 99.
pub const MAX_LEVEL: i64 = 8 * 99 - CLASS_INVARIANT;

/// Which floor was available, and why, when a build's attributes were normalised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Floor {
    /// The build names a class this build of the table knows. Its `CharaInitParam` base
    /// attributes were applied as minimums.
    Class {
        /// The class name, as [`class::STARTING_CLASSES`] spells it.
        name: &'static str,
        /// Its `PlayerGameData::archetype` byte.
        archetype: u8,
    },
    /// The build names no class at all, so there is no floor to apply. The attributes stand as
    /// the payload gave them; see the module docs for why no default is substituted.
    Unnamed,
    /// The build names a class, and it is not one this table lists -- which has historically meant
    /// the game grew a class rather than that the payload is junk. No floor is applied.
    Unrecognised(String),
}

impl Floor {
    /// The base attributes to hold the build to, if any.
    #[must_use]
    pub fn attributes(&self) -> Option<[u8; 8]> {
        match self {
            Self::Class { archetype, .. } => class::starting_attributes(*archetype),
            Self::Unnamed | Self::Unrecognised(_) => None,
        }
    }
}

/// One attribute that sat below its class base and was raised to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raised {
    /// The planner's key, one of [`class::ATTRIBUTE_KEYS`].
    pub key: &'static str,
    /// What the payload carried.
    pub was: i64,
    /// The class base it was raised to.
    pub now: i64,
}

/// What normalising a build's stat block established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Normalised {
    /// The level the attributes imply, and the value now in `stats["rl"]`.
    pub level: i64,
    /// The level the payload claimed, if it carried one at all. Captured before the overwrite.
    pub claimed: Option<i64>,
    /// The eight attributes' total, after the floor.
    pub total: i64,
    /// Which floor applied.
    pub floor: Floor,
    /// Every attribute the floor raised, in [`class::ATTRIBUTE_KEYS`] order. Empty is the normal
    /// case.
    pub raised: Vec<Raised>,
}

impl Normalised {
    /// Whether the payload's own `rl` agrees with the level its attributes imply.
    ///
    /// A payload with no `rl` at all counts as agreeing: it claimed nothing to disagree with.
    #[must_use]
    pub fn claim_agrees(&self) -> bool {
        self.claimed.is_none_or(|claimed| claimed == self.level)
    }
}

/// Why a stat block could not be read as a character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatError {
    /// The payload is missing one of the eight attributes.
    ///
    /// This is the failure that actually matters, and it used to be invisible: a `filter_map`
    /// skipped absent keys, so a payload short one attribute summed low, derived a lower level,
    /// and imported a character quietly missing points nobody would notice.
    MissingAttribute(&'static str),
    /// The attributes sum to something no character could hold.
    LevelOutOfRange {
        /// The eight attributes' total.
        total: i64,
        /// The level it implies.
        level: i64,
    },
}

impl core::fmt::Display for StatError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MissingAttribute(key) => write!(
                formatter,
                "the payload has no `{key}` attribute; refusing to import a build whose stats \
                 cannot be read in full"
            ),
            Self::LevelOutOfRange { total, level } => write!(
                formatter,
                "attributes sum to {total}, which is level {level} -- outside 1..={MAX_LEVEL}, so \
                 the stat block is not a real character"
            ),
        }
    }
}

impl core::error::Error for StatError {}

/// Raise every attribute to its class base, derive the level from the result, and write both back.
///
/// The single derivation of a character's level from its attributes. On success `doc.stats` holds
/// eight attributes at or above the named class's base and an `rl` that is exactly their sum less
/// [`CLASS_INVARIANT`] -- the identity `er_save_loader::stats` uses to find a save slot, so a
/// character this produces is one the mod's own reader can locate.
///
/// # Errors
///
/// [`StatError::MissingAttribute`] when the payload does not carry all eight, and
/// [`StatError::LevelOutOfRange`] when they sum to a level outside `1..=`[`MAX_LEVEL`]. On either,
/// `doc` is left exactly as it was: a rejected stat block must not leave a half-normalised
/// document behind for a caller that logs the error and carries on.
///
/// ```
/// use er_build_import_core::{model, stats};
///
/// // The reported case: a Vagabond whose strength sits two below the class base of 14.
/// let mut doc = model::parse(
///     r#"{"characterClass":"Vagabond","stats":{
///          "rl":150,"vig":60,"mnd":10,"vit":45,"str":12,"dex":75,"int":9,"fth":9,"arc":7}}"#,
/// )
/// .expect("parses");
///
/// let normalised = stats::normalise(&mut doc).expect("a real stat block");
///
/// assert_eq!(normalised.level, 150); // not the 148 the raw attributes implied
/// assert!(normalised.claim_agrees()); // and it is the RL the payload claimed
/// assert_eq!(doc.stats["str"], 14); // raised to Vagabond's base
/// assert_eq!(doc.stats["rl"], 150);
/// ```
pub fn normalise(doc: &mut BuildDoc) -> Result<Normalised, StatError> {
    let floor = match doc.character_class.as_deref() {
        None => Floor::Unnamed,
        Some(named) => match class::archetype_for_class(named) {
            // The name as the game spells it, not as the payload did: `archetype_for_class` is
            // case-insensitive, so a build saying "vagabond" should not make every log line say
            // "vagabond" either.
            Some(archetype) => match class::class_for_archetype(archetype) {
                Some(name) => Floor::Class { name, archetype },
                None => Floor::Unrecognised(named.to_owned()),
            },
            None => Floor::Unrecognised(named.to_owned()),
        },
    };
    let base = floor.attributes();

    // Summed from the CLAMPED values, so the level below is the level of the character that
    // actually gets applied -- which is the whole point. Reading all eight before writing any of
    // them is what lets a missing attribute leave `doc` untouched.
    let mut total: i64 = 0;
    let mut raised: Vec<Raised> = Vec::new();
    for (index, key) in class::ATTRIBUTE_KEYS.into_iter().enumerate() {
        let Some(was) = doc.stats.get(key).copied() else {
            return Err(StatError::MissingAttribute(key));
        };
        let now = match base {
            Some(base) => was.max(i64::from(base[index])),
            None => was,
        };
        if now != was {
            raised.push(Raised { key, was, now });
        }
        total += now;
    }

    // The one `- 79`. Every other level in this importer is read from `stats["rl"]`, which is the
    // line below it.
    let level = total - CLASS_INVARIANT;
    if !(1..=MAX_LEVEL).contains(&level) {
        return Err(StatError::LevelOutOfRange { total, level });
    }

    for entry in &raised {
        doc.stats.insert(entry.key.to_owned(), entry.now);
    }
    let claimed = doc.stats.insert("rl".to_owned(), level);

    Ok(Normalised {
        level,
        claimed,
        total,
        floor,
        raised,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model;

    /// A stat block as a payload would carry it, in ATTRIBUTE_KEYS order.
    fn doc_with(class: Option<&str>, claimed: Option<i64>, attributes: [i64; 8]) -> BuildDoc {
        let mut doc = BuildDoc {
            character_class: class.map(str::to_owned),
            ..BuildDoc::default()
        };
        if let Some(claimed) = claimed {
            doc.stats.insert("rl".to_owned(), claimed);
        }
        for (key, value) in class::ATTRIBUTE_KEYS.into_iter().zip(attributes) {
            doc.stats.insert(key.to_owned(), value);
        }
        doc
    }

    /// The reported build, in ATTRIBUTE_KEYS order: vig, mnd, vit, str, dex, int, fth, arc.
    ///
    /// A Vagabond claiming RL 150 whose strength sits at 12, two below the class base of 14 -- so
    /// the eight sum to 227 and imply 148, which is the number the user reported seeing. Every
    /// other attribute is at or above its Vagabond base, so strength is the only thing the floor
    /// has to move, and it moves the total by exactly the two points that separate 148 from 150.
    const VAGABOND_STR_12: [i64; 8] = [60, 10, 45, 12, 75, 9, 9, 7];

    #[test]
    fn the_reported_vagabond_keeps_the_level_it_claimed() {
        // User-reported 2026-09-06, verbatim: "Vagabond has base 14 str, a build with 12 str
        // saved, even at level 150 will set the Str to 12 and the level to 148".
        //
        // Un-clamped these eight sum to 227, which is level 148 -- the exact number reported.
        // Vagabond's strength base is 14, so the floor puts back the two missing points, and 229
        // less 79 is the 150 the payload claimed all along.
        let raw: i64 = VAGABOND_STR_12.iter().sum();
        assert_eq!(raw - CLASS_INVARIANT, 148, "the bug, before the floor");

        let mut doc = doc_with(Some("Vagabond"), Some(150), VAGABOND_STR_12);
        let normalised = normalise(&mut doc).expect("a real stat block");

        assert_eq!(normalised.level, 150);
        assert_eq!(normalised.total, 229);
        assert_eq!(normalised.claimed, Some(150));
        assert!(
            normalised.claim_agrees(),
            "the payload's own RL is the evidence the floor is what the planner meant"
        );
        assert_eq!(
            normalised.raised,
            vec![Raised {
                key: "str",
                was: 12,
                now: 14
            }]
        );
        assert_eq!(
            normalised.floor,
            Floor::Class {
                name: "Vagabond",
                archetype: 0
            }
        );

        // And the document the applier reads agrees with itself.
        assert_eq!(doc.stats["str"], 14);
        assert_eq!(doc.stats["rl"], 150);
        let applied: i64 = class::ATTRIBUTE_KEYS
            .into_iter()
            .map(|key| doc.stats[key])
            .sum();
        assert_eq!(doc.stats["rl"], applied - CLASS_INVARIANT);
    }

    #[test]
    fn a_build_that_names_no_class_gets_no_floor_and_no_invented_one() {
        // There is no class, so there is no base to raise to, and guessing one would silently add
        // levels to the build. The attributes stand and the level follows them -- 148, not 150.
        let mut doc = doc_with(None, Some(150), VAGABOND_STR_12);
        let normalised = normalise(&mut doc).expect("a real stat block");

        assert_eq!(normalised.floor, Floor::Unnamed);
        assert_eq!(normalised.raised, vec![]);
        assert_eq!(normalised.level, 148);
        assert_eq!(normalised.total, 227);
        assert_eq!(doc.stats["str"], 12, "nothing was raised");
        // The disagreement is reported rather than obeyed, exactly as before the floor existed.
        assert_eq!(normalised.claimed, Some(150));
        assert!(!normalised.claim_agrees());
        assert_eq!(doc.stats["rl"], 148);
    }

    #[test]
    fn an_unrecognised_class_gets_no_floor_either() {
        // Historically this has meant the game grew a class, not that the payload is junk. Either
        // way there is no row to read a base from, so nothing is raised and the caller is told
        // which name it was.
        let mut doc = doc_with(Some("Ronin"), Some(150), VAGABOND_STR_12);
        let normalised = normalise(&mut doc).expect("a real stat block");

        assert_eq!(normalised.floor, Floor::Unrecognised("Ronin".to_owned()));
        assert_eq!(normalised.raised, vec![]);
        assert_eq!(normalised.level, 148);
    }

    #[test]
    fn the_class_name_is_reported_the_way_the_game_spells_it() {
        let mut doc = doc_with(Some("vAgAbOnD"), None, VAGABOND_STR_12);
        let normalised = normalise(&mut doc).expect("a real stat block");
        assert_eq!(
            normalised.floor,
            Floor::Class {
                name: "Vagabond",
                archetype: 0
            }
        );
    }

    #[test]
    fn a_build_already_above_its_class_base_is_left_alone() {
        // The archived Vagabond fixture, unmodified: str 17, and every unspent attribute sitting
        // exactly on its class base. Nothing to raise, and the level it derives is the one it
        // claims -- which is what says the floor does not disturb a well-formed build.
        let fixture = [60, 10, 45, 17, 72, 9, 9, 7];
        let mut doc = doc_with(Some("Vagabond"), Some(150), fixture);
        let normalised = normalise(&mut doc).expect("a real stat block");

        assert_eq!(normalised.raised, vec![]);
        assert_eq!(normalised.level, 150);
        assert!(normalised.claim_agrees());
    }

    #[test]
    fn a_floor_can_raise_more_than_one_attribute() {
        // Wretch is the useful case: every base is 10, so an under-spent build trips several at
        // once and the report has to name each.
        let mut doc = doc_with(Some("Wretch"), None, [8, 10, 9, 10, 10, 10, 10, 7]);
        let normalised = normalise(&mut doc).expect("a real stat block");

        assert_eq!(
            normalised.raised,
            vec![
                Raised {
                    key: "vig",
                    was: 8,
                    now: 10
                },
                Raised {
                    key: "vit",
                    was: 9,
                    now: 10
                },
                Raised {
                    key: "arc",
                    was: 7,
                    now: 10
                },
            ]
        );
        assert_eq!(normalised.total, 80);
        assert_eq!(normalised.level, 1, "a Wretch at its floor is level 1");
    }

    #[test]
    fn every_class_at_its_own_base_derives_its_own_starting_level() {
        // The floor and the level agree for all twelve classes, so the clamp can never produce a
        // character below the level the game would have dealt them.
        for (archetype, (level, base)) in class::STARTING_STATS.iter().copied().enumerate() {
            let archetype = u8::try_from(archetype).expect("the classes fit in a byte");
            let name = class::class_for_archetype(archetype).expect("a listed class");
            // One point below the base in every attribute: the floor has to put all eight back.
            let starved = base.map(|value| i64::from(value) - 1);
            let mut doc = doc_with(Some(name), None, starved);
            let normalised = normalise(&mut doc).expect("a real stat block");

            assert_eq!(normalised.raised.len(), 8, "{name}");
            assert_eq!(i64::from(level), normalised.level, "{name}");
            for (key, want) in class::ATTRIBUTE_KEYS.into_iter().zip(base) {
                assert_eq!(doc.stats[key], i64::from(want), "{name} {key}");
            }
        }
    }

    #[test]
    fn a_missing_attribute_is_refused_and_changes_nothing() {
        let mut doc = doc_with(Some("Vagabond"), Some(150), VAGABOND_STR_12);
        doc.stats.remove("fth");
        let before = doc.stats.clone();

        assert_eq!(normalise(&mut doc), Err(StatError::MissingAttribute("fth")));
        assert_eq!(
            doc.stats, before,
            "a refused stat block must not leave a half-normalised document behind"
        );
    }

    #[test]
    fn an_impossible_stat_block_is_refused_before_anything_is_written() {
        // Below the floor of the whole game: eight attributes at 1 is level -71.
        let mut doc = doc_with(None, Some(150), [1; 8]);
        let before = doc.stats.clone();
        assert_eq!(
            normalise(&mut doc),
            Err(StatError::LevelOutOfRange {
                total: 8,
                level: 8 - CLASS_INVARIANT
            })
        );
        assert_eq!(doc.stats, before);

        let mut doc = doc_with(None, None, [1000; 8]);
        assert!(matches!(
            normalise(&mut doc),
            Err(StatError::LevelOutOfRange { .. })
        ));
    }

    #[test]
    fn the_refusal_messages_say_what_went_wrong() {
        assert!(
            StatError::MissingAttribute("fth")
                .to_string()
                .contains("has no `fth` attribute")
        );
        assert!(
            StatError::LevelOutOfRange {
                total: 8,
                level: -71
            }
            .to_string()
            .contains("outside 1..=713")
        );
    }

    #[test]
    fn a_payload_with_no_rl_at_all_gets_the_derived_one() {
        let mut doc = doc_with(Some("Vagabond"), None, VAGABOND_STR_12);
        let normalised = normalise(&mut doc).expect("a real stat block");
        assert_eq!(normalised.claimed, None);
        assert!(
            normalised.claim_agrees(),
            "it claimed nothing to disagree with"
        );
        assert_eq!(doc.stats["rl"], 150);
    }

    #[test]
    fn the_archived_payloads_still_normalise_to_the_level_they_claim() {
        // The three real planner payloads in tests/fixtures/. None of them trips the floor -- each
        // already sits at or above its class base -- and that is the point: the floor is not
        // allowed to move a build the planner exported cleanly.
        for raw in [
            include_str!("../tests/fixtures/build-82086df03c4b8e.json"),
            include_str!("../tests/fixtures/build-94252a868b4f2a.json"),
            include_str!("../tests/fixtures/build-af97a9da874151.json"),
        ] {
            let mut doc = model::parse(raw).expect("an archived payload parses");
            let class = doc
                .character_class
                .clone()
                .expect("the fixtures name a class");
            let normalised = normalise(&mut doc).expect("a real stat block");

            assert_eq!(normalised.raised, vec![], "{class} was moved by the floor");
            assert_eq!(normalised.level, 150, "{class}");
            assert!(normalised.claim_agrees(), "{class}");
        }
    }
}
