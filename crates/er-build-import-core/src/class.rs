//! Starting classes, in the order the GAME stores them.
//!
//! `PlayerGameData::archetype` is one byte and `ArchetypeToInitParamId` is literally
//! `archetype + 3000`, so the byte is an index into `CharaInitParam` rows starting at 3000 and
//! the order is that table's, not any list's on any website.
//!
//! # The order was wrong, and it was wrong in a way that reads as plausible
//!
//! Both halves of this repository carried the planner's DISPLAY order, in which Samurai comes
//! sixth. In `CharaInitParam` it is Confessor. So a Confessor exported as `Samurai`, and a build
//! saying `Samurai` imported as a Confessor -- two names swapped in a list of ten, which no smoke
//! test notices and which is exactly what was reported.
//!
//! # Then the list stopped being ten
//!
//! ELDEN RING **1.17** added two classes, `CharaInitParam` 3010 and 3011. This table was a
//! `[&str; 10]`, so `class_for_archetype` answered `None` for both -- and every consumer treats
//! `None` as "no class", not as "this table is older than the game". Export dropped the class
//! silently; import silently never set one. The length now comes from the list itself
//! ([`STARTING_CLASS_COUNT`]), and `scripts/check-starting-classes.py` re-derives the whole table
//! from the installed `regulation.bin` so the next patch fails a gate instead of a character.
//!
//! # Proven from the game's own data, not from a wiki
//!
//! Read straight out of the installed `regulation.bin`: each row carries a starting level at row
//! offset 192 and the eight attributes at 194..=201 (vigour, mind, endurance, strength,
//! dexterity, intelligence, faith, arcane). The rows come out:
//!
//! ```text
//! 3000 Vagabond    lvl  9  vig 15      3006 Confessor    lvl 10  vig 10
//! 3001 Warrior     lvl  8  vig 11      3007 Samurai      lvl  9  vig 12
//! 3002 Hero        lvl  7  vig 14      3008 Prisoner     lvl  9  vig 11
//! 3003 Bandit      lvl  5  vig 10      3009 Wretch       lvl  1  vig 10
//! 3004 Astrologer  lvl  6  vig  9      3010 Idus Knight  lvl  7  vig 10   (1.17)
//! 3005 Prophet     lvl  7  vig 10      3011 Heavy Knight lvl 10  vig 14   (1.17)
//! ```
//!
//! **The `(level, vigour)` pair no longer identifies a class**, which it did on 1.16.2 and which
//! the previous revision of this file said outright: Prophet and Idus Knight are both level 7 with
//! 10 vigour. The full eight-attribute run still is unique, so that is what the tests below pin.
//!
//! # How each NAME is bound to its row
//!
//! `BaseChrSelectMenuParam` -- the class-select list -- links the two tables explicitly. Its class
//! rows (32-byte stride, s32 fields) carry the `CharaInitParam` row id in field 2 and the
//! `GR_MenuText` message id in field 4:
//!
//! ```text
//! row 2000: [1, 3100, 3000,  6, 288100, 0, 0, 0]     -> CharaInit 3000, text 288100 "Vagabond"
//! row 2008: [1, 3112, 3006, 12, 288106, 0, 0, 0]     -> CharaInit 3006, text 288106 "Confessor"
//! row 2010: [1, 3120, 3010, 16, 288110, 0, 0, 0]     -> CharaInit 3010, text 288110 "Idus Knight"
//! row 2011: [1, 3122, 3011, 17, 288111, 0, 0, 0]     -> CharaInit 3011, text 288111 "Heavy Knight"
//! ```
//!
//! Across all rows the message id is exactly `288100 + (charaInitRow - 3000)`, i.e.
//! [`FIRST_CLASS_NAME_MESSAGE_ID`]` + archetype` -- note rows 2006..2008 list Samurai and Prisoner
//! *before* Confessor, which is the display order, and is precisely the trap that produced the
//! original swap. The names themselves are the strings at those ids in
//! `msg/engus/menu.msgbnd.dcx` -> `GR_MenuText.fmg`.
//!
//! Reproduce all of it with `python3 scripts/check-starting-classes.py`, which reads the installed
//! regulation and (when an extracted message corpus is reachable) the FMG, and fails on any
//! disagreement with the table below.

/// The starting classes, indexed by `PlayerGameData::archetype`.
///
/// A slice, not a fixed-size array, on purpose: the length is a fact about the installed game, and
/// writing it into the type is what let 1.17's two extra classes vanish into a `None`. Use
/// [`STARTING_CLASS_COUNT`] when a count is needed.
///
/// See the module docs: this is `CharaInitParam` row order, and the two classes that look
/// interchangeable in it -- Confessor at 6, Samurai at 7 -- are the whole reason it is stated once
/// here rather than transcribed wherever it is needed.
pub const STARTING_CLASSES: &[&str] = &[
    "Vagabond",
    "Warrior",
    "Hero",
    "Bandit",
    "Astrologer",
    "Prophet",
    "Confessor",
    "Samurai",
    "Prisoner",
    "Wretch",
    // 1.17 (2026-08-27). Names read out of the installed game's `GR_MenuText.fmg` at 288110 and
    // 288111, which `BaseChrSelectMenuParam` rows 2010/2011 bind to `CharaInitParam` 3010/3011.
    "Idus Knight",
    "Heavy Knight",
];

/// How many starting classes the game has, derived from [`STARTING_CLASSES`] rather than written
/// down a second time.
pub const STARTING_CLASS_COUNT: usize = STARTING_CLASSES.len();

/// `CharaInitParam` row id of the first starting class.
pub const FIRST_CHARA_INIT_PARAM_ROW: u32 = 3000;

/// `GR_MenuText` message id of the first starting class's NAME.
///
/// `BaseChrSelectMenuParam` pairs every class row's `CharaInitParam` id with a message id that is
/// this constant plus the archetype; see the module docs for the rows that show it.
pub const FIRST_CLASS_NAME_MESSAGE_ID: u32 = 288_100;

/// The class an archetype byte names.
///
/// `None` has two meanings and the caller almost always wants to distinguish them: the byte is
/// garbage, or **this table is older than the game the byte came from**. A build exported with
/// `None` here has silently lost its class, which is what 1.17 did before the two rows above were
/// added. [`chara_init_param_row`] and `scripts/check-starting-classes.py` are how that second
/// case gets caught.
///
/// ```
/// use er_build_import_core::class::class_for_archetype;
/// assert_eq!(class_for_archetype(6), Some("Confessor"));
/// assert_eq!(class_for_archetype(7), Some("Samurai"));
/// // 1.17 added these two; before that both answered None.
/// assert_eq!(class_for_archetype(10), Some("Idus Knight"));
/// assert_eq!(class_for_archetype(11), Some("Heavy Knight"));
/// assert_eq!(class_for_archetype(12), None);
/// ```
#[must_use]
pub fn class_for_archetype(archetype: u8) -> Option<&'static str> {
    STARTING_CLASSES.get(usize::from(archetype)).copied()
}

/// The archetype byte for a class name, however the build spelled it.
///
/// ```
/// use er_build_import_core::class::archetype_for_class;
/// assert_eq!(archetype_for_class("confessor"), Some(6));
/// assert_eq!(archetype_for_class("Samurai"), Some(7));
/// assert_eq!(archetype_for_class("idus knight"), Some(10));
/// assert_eq!(archetype_for_class("Heavy Knight"), Some(11));
/// assert_eq!(archetype_for_class("Tarnished"), None);
/// ```
#[must_use]
pub fn archetype_for_class(name: &str) -> Option<u8> {
    STARTING_CLASSES
        .iter()
        .position(|class| class.eq_ignore_ascii_case(name))
        .and_then(|index| u8::try_from(index).ok())
}

/// The `CharaInitParam` row an archetype byte indexes, whether or not this table knows its name.
///
/// The mapping is the game's `ArchetypeToInitParamId`, `archetype + 3000`, and it holds for rows
/// this build has never heard of -- which is what makes it the check that catches the next patch:
/// a row that exists in `regulation.bin` at an archetype past [`STARTING_CLASS_COUNT`] is a class
/// this table is missing.
///
/// ```
/// use er_build_import_core::class::chara_init_param_row;
/// assert_eq!(chara_init_param_row(0), 3000);
/// assert_eq!(chara_init_param_row(11), 3011);
/// ```
#[must_use]
pub fn chara_init_param_row(archetype: u8) -> u32 {
    FIRST_CHARA_INIT_PARAM_ROW + u32::from(archetype)
}

/// The `GR_MenuText` id holding an archetype's class NAME.
///
/// ```
/// use er_build_import_core::class::class_name_message_id;
/// assert_eq!(class_name_message_id(6), 288_106);
/// assert_eq!(class_name_message_id(10), 288_110); // "Idus Knight"
/// ```
#[must_use]
pub fn class_name_message_id(archetype: u8) -> u32 {
    FIRST_CLASS_NAME_MESSAGE_ID + u32::from(archetype)
}

/// Starting level and the eight base attributes, keyed by the archetype byte: bytes 192 and
/// 194..=201 of `CharaInitParam` rows 3000.. , read out of the installed `regulation.bin`.
///
/// # These are FLOORS, which is why this is no longer test-only
///
/// It sat inside `#[cfg(test)]` for as long as its only job was to pin the ORDER of
/// [`STARTING_CLASSES`] -- two independent sources agreeing about which row is Confessor and which
/// is Samurai. It is now also product data, because a class's base attributes are the **floor**
/// under its character: character creation deals you these numbers, and the level-up dialog reads
/// this same row (`0x1407c6560` -> `CS::PlayerLevelUpDialog` @ `0x14096daa0`) into a base-stats
/// simulator held beside the live one. No ordinary play session produces a Vagabond with strength
/// below 14.
///
/// The importer needs that fact, and it cannot borrow it from the engine: the native it calls to
/// write stats, `ApplyMainPlayerStats` @ `0x140788cf0`, does no clamping whatsoever. A planner
/// payload can carry an attribute below the base of the class it names, and the eight attributes
/// are what the level is derived from ([`crate::stats::normalise`]) -- so a sub-base attribute
/// silently costs the character levels it claimed to have, and nothing downstream objects. See
/// [`crate::stats`] for the reported case and the rest of the evidence.
///
/// Attribute order is the game's, and it is [`ATTRIBUTE_KEYS`] index for index: vigour, mind,
/// endurance, strength, dexterity, intelligence, faith, arcane. Byte 193 is zero on every row and
/// is not read here.
///
/// Ground truth, not transcription: `scripts/check-starting-classes.py` re-reads every one of
/// these numbers out of the installed regulation and fails on a disagreement, and the tests below
/// cross-check the same rows against figures the planner publishes.
pub const STARTING_STATS: &[(u8, [u8; 8])] = &[
    (9, [15, 10, 11, 14, 13, 9, 9, 7]),    // 0  Vagabond
    (8, [11, 12, 11, 10, 16, 10, 8, 9]),   // 1  Warrior
    (7, [14, 9, 12, 16, 9, 7, 8, 11]),     // 2  Hero
    (5, [10, 11, 10, 9, 13, 9, 8, 14]),    // 3  Bandit
    (6, [9, 15, 9, 8, 12, 16, 7, 9]),      // 4  Astrologer
    (7, [10, 14, 8, 11, 10, 7, 16, 10]),   // 5  Prophet
    (10, [10, 13, 10, 12, 12, 9, 14, 9]),  // 6  Confessor
    (9, [12, 11, 13, 12, 15, 9, 8, 8]),    // 7  Samurai
    (9, [11, 12, 11, 11, 14, 14, 6, 9]),   // 8  Prisoner
    (1, [10, 10, 10, 10, 10, 10, 10, 10]), // 9  Wretch
    (7, [10, 12, 11, 13, 15, 8, 11, 6]),   // 10 Idus Knight   (1.17)
    (10, [14, 8, 17, 15, 11, 7, 8, 9]),    // 11 Heavy Knight  (1.17)
];

/// The planner's key for each attribute, in the order [`STARTING_STATS`] holds them.
///
/// **One array, one order.** The clamp in [`crate::stats::normalise`] reads a payload key and the
/// class base at the SAME index, so a floor applied under the wrong name is a real and silent
/// failure mode -- Vagabond's strength base held against the build's faith. Writing the key list
/// beside the table it indexes, rather than in the consumer, is what makes that unrepresentable;
/// `the_planner_keys_line_up_with_the_param_row` states it as an assertion too.
///
/// Note `vit`: the planner spells **Endurance** that way, and it is the game's third attribute,
/// not its Vigor. Getting that pairing wrong is the other half of the same failure.
pub const ATTRIBUTE_KEYS: [&str; 8] = ["vig", "mnd", "vit", "str", "dex", "int", "fth", "arc"];

/// The base attributes an archetype starts with -- the game's per-attribute floor for that class.
///
/// Indexed by [`ATTRIBUTE_KEYS`]. `None` means the same thing it means in
/// [`class_for_archetype`]: either the byte is garbage, or this table is older than the game.
///
/// ```
/// use er_build_import_core::class::{starting_attributes, ATTRIBUTE_KEYS};
/// let vagabond = starting_attributes(0).expect("archetype 0 is Vagabond");
/// let strength = ATTRIBUTE_KEYS.iter().position(|k| *k == "str").expect("str is an attribute");
/// assert_eq!(vagabond[strength], 14);
/// assert_eq!(starting_attributes(12), None);
/// ```
#[must_use]
pub fn starting_attributes(archetype: u8) -> Option<[u8; 8]> {
    STARTING_STATS
        .get(usize::from(archetype))
        .map(|(_, attributes)| *attributes)
}

/// The base attributes for a class name, however the build spelled it.
///
/// `None` covers both "the build named no class this table knows" and "the name is not a class at
/// all". The caller has to tell those apart from an ABSENT name, which is why this takes a `&str`
/// and not an `Option<&str>` -- see [`crate::stats::Floor`].
///
/// ```
/// use er_build_import_core::class::starting_attributes_for_class;
/// assert_eq!(starting_attributes_for_class("vagabond"), starting_attributes_for_class("Vagabond"));
/// assert_eq!(starting_attributes_for_class("Tarnished"), None);
/// ```
#[must_use]
pub fn starting_attributes_for_class(name: &str) -> Option<[u8; 8]> {
    archetype_for_class(name).and_then(starting_attributes)
}

/// The level an archetype's character starts at, straight out of `CharaInitParam` byte 192.
///
/// ```
/// use er_build_import_core::class::starting_level;
/// assert_eq!(starting_level(0), Some(9)); // Vagabond
/// assert_eq!(starting_level(9), Some(1)); // Wretch
/// ```
#[must_use]
pub fn starting_level(archetype: u8) -> Option<u8> {
    STARTING_STATS
        .get(usize::from(archetype))
        .map(|(level, _)| *level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::CLASS_INVARIANT;

    /// Starting level and vigour as the PLANNER publishes them, keyed by NAME.
    ///
    /// This table's whole value is that it does NOT come from `regulation.bin`. Two sources
    /// agreeing is the order being right; two copies of one source agreeing is nothing. So it
    /// stays exactly as it was -- (level, vigour), the figures the planner states -- rather than
    /// being widened with attribute runs copied out of the param, which would have quietly turned
    /// the cross-check into a self-comparison.
    ///
    /// The 1.17 pair is absent because the planner has no entry for either. Their independent
    /// binding is the `BaseChrSelectMenuParam` -> `GR_MenuText` link (288110 / 288111), which
    /// `scripts/check-starting-classes.py` checks against the installed game -- not here.
    const PUBLISHED: &[(&str, u8, u8)] = &[
        ("Vagabond", 9, 15),
        ("Warrior", 8, 11),
        ("Hero", 7, 14),
        ("Bandit", 5, 10),
        ("Astrologer", 6, 9),
        ("Prophet", 7, 10),
        ("Samurai", 9, 12),
        ("Prisoner", 9, 11),
        ("Confessor", 10, 10),
        ("Wretch", 1, 10),
    ];

    #[test]
    fn the_stat_table_covers_exactly_the_classes_the_name_table_lists() {
        // The failure this whole module exists to prevent is one table growing without the other.
        assert_eq!(STARTING_STATS.len(), STARTING_CLASS_COUNT);
    }

    #[test]
    fn each_archetype_names_the_class_whose_starting_stats_that_row_holds() {
        // The param says what the ROW holds; the planner says what the NAME holds. Walking the
        // archetypes and requiring the two to meet is what pins the order -- and it is why the
        // planner side must stay planner-sourced.
        //
        // On 1.17 the (level, vigour) pair alone can no longer separate every class: Prophet and
        // Idus Knight share (7, 10). That costs this test nothing, because it looks the class up
        // by NAME rather than by pair, and Idus Knight is not in the planner table at all. The
        // property that a class is identifiable at all now rests on the full attribute run, which
        // `every_class_has_a_distinct_attribute_run` states and checks explicitly.
        for (archetype, (level, attributes)) in STARTING_STATS.iter().copied().enumerate() {
            let archetype = u8::try_from(archetype).expect("the classes fit in a byte");
            let name = class_for_archetype(archetype).expect("every archetype names a class");
            let Some(published) = PUBLISHED.iter().find(|(class, _, _)| *class == name) else {
                // A 1.17 class. Its binding is checked against the game's own FMG by
                // `scripts/check-starting-classes.py`, not by a table in this file.
                continue;
            };
            let vigour = attributes[0];
            assert_eq!(
                (published.1, published.2),
                (level, vigour),
                "archetype {archetype} says {name}, whom the planner starts at level {} with {} \
                 vigour, but CharaInitParam row {} holds level {level} vigour {vigour}",
                published.1,
                published.2,
                chara_init_param_row(archetype),
            );
        }
    }

    #[test]
    fn every_class_has_a_distinct_attribute_run() {
        // On 1.16.2 the (level, vigour) PAIR was unique and the old tests leaned on that. 1.17
        // broke it -- Prophet and Idus Knight are both level 7 with 10 vigour -- so the property
        // the tables actually rely on is stated and checked rather than assumed.
        for (a, (_, left)) in STARTING_STATS.iter().enumerate() {
            for (b, (_, right)) in STARTING_STATS.iter().enumerate().skip(a + 1) {
                assert_ne!(
                    left,
                    right,
                    "archetypes {a} ({:?}) and {b} ({:?}) share an attribute run, so the run no \
                     longer identifies a class",
                    class_for_archetype(u8::try_from(a).unwrap()),
                    class_for_archetype(u8::try_from(b).unwrap()),
                );
            }
        }
    }

    #[test]
    fn every_starting_level_is_the_attribute_total_less_seventy_nine() {
        // An arithmetic invariant of ER character creation, independent of any table here: a
        // starting class's level is the sum of its eight attributes minus 79. It holds for all ten
        // 1.16.2 classes and for both 1.17 additions, which is the cheapest evidence that the two
        // new rows are real starting-stat blocks rather than padding that happens to be non-zero.
        //
        // The 79 is `stats::CLASS_INVARIANT` rather than a literal, so this test and the importer's
        // one derivation of a level cannot disagree about the number they both rest on.
        for (archetype, (level, attributes)) in STARTING_STATS.iter().copied().enumerate() {
            let total: i64 = attributes.iter().map(|&a| i64::from(a)).sum();
            assert_eq!(
                i64::from(level),
                total - CLASS_INVARIANT,
                "archetype {archetype} ({:?}) has attributes totalling {total}, which is level {}, \
                 not {level}",
                class_for_archetype(u8::try_from(archetype).unwrap()),
                total - CLASS_INVARIANT,
            );
        }
    }

    #[test]
    fn the_planner_keys_line_up_with_the_param_row() {
        // The pairing the clamp depends on, stated rather than assumed: ATTRIBUTE_KEYS[i] names
        // the attribute STARTING_STATS holds at [i]. Checked against the one class whose base
        // attributes are all distinct, so every index is pinned by a different number and a
        // transposition cannot survive.
        //
        // Vagabond, CharaInitParam 3000: vig 15, mnd 10, end 11, str 14, dex 13, int 9, fth 9,
        // arc 7. Vigour and strength are the pair that matters -- 15 against 14, adjacent numbers
        // in different positions, which is exactly the mistake a reordering would make.
        let vagabond = starting_attributes(0).expect("archetype 0 is Vagabond");
        let at = |key: &str| {
            let index = ATTRIBUTE_KEYS
                .iter()
                .position(|candidate| *candidate == key)
                .unwrap_or_else(|| panic!("{key} is not one of the eight attribute keys"));
            vagabond[index]
        };
        assert_eq!(at("vig"), 15);
        assert_eq!(at("mnd"), 10);
        assert_eq!(at("vit"), 11, "the planner spells Endurance `vit`");
        assert_eq!(at("str"), 14);
        assert_eq!(at("dex"), 13);
        assert_eq!(at("int"), 9);
        assert_eq!(at("fth"), 9);
        assert_eq!(at("arc"), 7);
    }

    #[test]
    fn the_eight_keys_are_eight_distinct_keys() {
        // A duplicate key would make one attribute's floor unreachable and apply another's twice,
        // and the table above would still look right.
        for (index, key) in ATTRIBUTE_KEYS.iter().enumerate() {
            assert!(
                !ATTRIBUTE_KEYS[..index].contains(key),
                "{key} appears twice in ATTRIBUTE_KEYS"
            );
        }
        assert_eq!(ATTRIBUTE_KEYS.len(), STARTING_STATS[0].1.len());
    }

    #[test]
    fn a_class_name_resolves_to_the_base_attributes_of_its_row() {
        // The by-name accessor is the one the importer actually calls, so it gets its own check
        // rather than riding on the by-archetype one.
        assert_eq!(
            starting_attributes_for_class("Vagabond"),
            starting_attributes(0)
        );
        assert_eq!(
            starting_attributes_for_class("wretch"),
            starting_attributes(9)
        );
        assert_eq!(
            starting_attributes_for_class("Heavy Knight"),
            starting_attributes(11)
        );
        assert_eq!(starting_attributes_for_class(""), None);
        assert_eq!(starting_attributes_for_class("Tarnished"), None);
    }

    #[test]
    fn every_class_starts_at_the_level_its_row_states() {
        for (archetype, (level, _)) in STARTING_STATS.iter().copied().enumerate() {
            let archetype = u8::try_from(archetype).expect("the classes fit in a byte");
            assert_eq!(starting_level(archetype), Some(level));
        }
        let past_the_end = u8::try_from(STARTING_CLASS_COUNT).expect("the classes fit in a byte");
        assert_eq!(starting_level(past_the_end), None);
        assert_eq!(starting_attributes(past_the_end), None);
    }

    #[test]
    fn the_two_that_were_swapped_are_the_way_the_game_has_them() {
        // The reported bug: a Confessor exported as `Samurai`.
        assert_eq!(class_for_archetype(6), Some("Confessor"));
        assert_eq!(class_for_archetype(7), Some("Samurai"));
        assert_eq!(archetype_for_class("Confessor"), Some(6));
        assert_eq!(archetype_for_class("Samurai"), Some(7));
    }

    #[test]
    fn the_two_that_1_17_added_are_where_the_class_select_param_puts_them() {
        // BaseChrSelectMenuParam row 2010 = [1, 3120, 3010, 16, 288110, ..], row 2011 likewise.
        assert_eq!(class_for_archetype(10), Some("Idus Knight"));
        assert_eq!(class_for_archetype(11), Some("Heavy Knight"));
        assert_eq!(archetype_for_class("Idus Knight"), Some(10));
        assert_eq!(archetype_for_class("Heavy Knight"), Some(11));
        assert_eq!(chara_init_param_row(10), 3010);
        assert_eq!(chara_init_param_row(11), 3011);
        assert_eq!(class_name_message_id(10), 288_110);
        assert_eq!(class_name_message_id(11), 288_111);
    }

    #[test]
    fn every_name_round_trips() {
        for (index, name) in STARTING_CLASSES.iter().copied().enumerate() {
            let archetype = archetype_for_class(name).expect("a listed class resolves");
            assert_eq!(usize::from(archetype), index);
            assert_eq!(class_for_archetype(archetype), Some(name));
        }
    }

    #[test]
    fn the_first_archetype_past_the_table_has_no_class() {
        // Deliberately phrased against the derived count. The assertion it replaces was written as
        // `class_for_archetype(10) == None`, and 1.17 made that literal false without making it
        // fail to compile.
        let past_the_end = u8::try_from(STARTING_CLASS_COUNT).expect("the classes fit in a byte");
        assert_eq!(class_for_archetype(past_the_end), None);
        assert_eq!(class_for_archetype(u8::MAX), None);
    }
}
