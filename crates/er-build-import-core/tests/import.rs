//! End-to-end: a real planner payload becomes a complete, correct grant list.
//!
//! The fixture is the exact 6871-byte body returned by
//! `get https://er-inventory-api.nyasu.business/inventories/af97a9da874151`,
//! captured unauthenticated. Its catalog rows come from the planner's public
//! item database.

mod fixture_catalog;

use er_build_import_core::catalog::{Catalog, Kind};
use er_build_import_core::plan::{
    GEM_ITEM_CATEGORY, NO_SKILL, armament_item_id, equipped_armament_skills, plan,
};
use er_build_import_core::{model, share_id_from_url};

const BUILD: &str = include_str!("fixtures/build-af97a9da874151.json");

fn planned() -> (model::BuildDoc, er_build_import_core::Plan) {
    let doc = model::parse(BUILD).expect("fixture parses");
    let result = plan(&doc, &fixture_catalog::catalog());
    (doc, result)
}

#[test]
fn parses_the_real_payload() {
    let (doc, _) = planned();
    assert_eq!(doc.id, "af97a9da874151");
    assert_eq!(doc.name, "Occult Mage");
    assert_eq!(doc.character_class.as_deref(), Some("Bandit"));
    assert_eq!(doc.weapon_upgrade, 25);
    assert_eq!(doc.stats.get("rl"), Some(&150));
    assert_eq!(doc.inventory.slots.len(), 35);
    assert_eq!(doc.spells.slots.len(), 8);
    assert_eq!(doc.talismans.slots.len(), 4);
}

#[test]
fn every_referenced_item_resolves() {
    let (_, result) = planned();
    assert!(
        result.is_complete(),
        "unresolved items: {:?}",
        result
            .unresolved
            .iter()
            .map(|u| (u.kind.label(), &u.name))
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_accented_weapon_resolves() {
    // The build spells it `Miséricorde`; the catalog key is `Misericorde`.
    let (doc, result) = planned();
    assert!(
        doc.inventory
            .slots
            .iter()
            .any(|s| s.name.contains('\u{e9}')),
        "fixture should still exercise the accent path"
    );
    assert!(!result.grants.iter().any(|g| g.label.is_empty()));
    assert!(result.unresolved.is_empty());
}

#[test]
fn spells_are_memorised_in_slot_order() {
    let (doc, result) = planned();
    assert_eq!(result.equip_spells.len(), doc.spells.slots.len());
    // Great Oracular Bubble is slot 0 and has param id 5110.
    assert_eq!(result.equip_spells[0], 5110);
}

#[test]
fn grant_count_matches_the_build() {
    let (doc, result) = planned();
    let armour: usize = doc.protectors.values().map(|p| p.slots.len()).sum();
    let expected = doc.inventory.slots.len()
        + doc.spells.slots.len()
        + doc.talismans.slots.len()
        + armour
        + doc.items.tools.slots.len();
    assert_eq!(result.grants.len(), expected);
}

#[test]
fn every_grant_encodes_to_sixteen_bytes() {
    let (_, result) = planned();
    for grant in &result.grants {
        let record = grant.to_record();
        assert_eq!(&record[0..4], &grant.item_id.to_le_bytes());
        assert_eq!(
            &record[10..12],
            &[0, 0],
            "the reserved half-word stays zero"
        );
    }
}

#[test]
fn unenchanted_items_carry_no_skill_and_no_upgrade() {
    let (_, result) = planned();
    let talisman = result
        .grants
        .iter()
        .find(|g| g.label == "Radagon Icon")
        .expect("talisman");
    assert_eq!(talisman.item_id, 0x20000BFE);
    assert_eq!(talisman.reinforce_lv, 0);
    assert_eq!(talisman.weapon_skill, NO_SKILL);
}

#[test]
fn share_ids_are_extracted_and_the_offline_form_is_rejected() {
    assert_eq!(
        share_id_from_url("https://er-build-planner.nyasu.business/?b=af97a9da874151"),
        Some("af97a9da874151")
    );
    // `?i=` is the self-contained payload form and needs no fetch.
    assert_eq!(
        share_id_from_url("https://er-build-planner.nyasu.business/?i=uwuABC"),
        None
    );
    assert_eq!(
        share_id_from_url("https://er-build-planner.nyasu.business/"),
        None
    );
}

#[test]
fn name_collisions_across_categories_stay_distinct() {
    // `Golden Vow` is a spell, an ash of war and a consumable. A flat map would
    // grant whichever happened to be inserted last.
    let catalog = fixture_catalog::catalog();
    use er_build_import_core::Catalog;
    let spell = catalog.lookup(Kind::Spell, "Golden Vow");
    let ash = catalog.lookup(Kind::AshOfWar, "Golden Vow");
    if let (Some(spell), Some(ash)) = (spell, ash) {
        assert_ne!(spell.full_item_id, ash.full_item_id);
    }
}

// ---------------------------------------------------------------- equipping

use er_build_import_core::equip::{Capacity, PHYSICK_SLOTS, QUICKBAR_SLOTS, equip_plan};

fn equipped() -> er_build_import_core::EquipPlan {
    let doc = model::parse(BUILD).expect("fixture parses");
    equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default())
}

#[test]
fn armaments_land_in_the_slots_the_author_chose() {
    let plan = equipped();
    assert!(plan.is_complete(), "rejected: {:?}", plan.rejected);
    let names: Vec<_> = plan
        .armaments
        .iter()
        .map(|slot| slot.as_ref().map(|item| item.name.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            Some("Deadly Poison Perfume Bottle"),
            Some("Albinauric Staff"),
            Some("Mis\u{e9}ricorde"),
            Some("Poisoned Hand"),
            Some("Azur's Glintstone Staff"),
            None,
        ]
    );
}

#[test]
fn all_four_armour_pieces_are_worn() {
    let plan = equipped();
    assert_eq!(
        plan.head.as_ref().map(|i| i.name.as_str()),
        Some("Mushroom Crown")
    );
    assert_eq!(
        plan.body.as_ref().map(|i| i.name.as_str()),
        Some("Armor of Solitude")
    );
    assert_eq!(
        plan.arms.as_ref().map(|i| i.name.as_str()),
        Some("Young Lion's Gauntlets")
    );
    assert_eq!(
        plan.legs.as_ref().map(|i| i.name.as_str()),
        Some("Greaves of Solitude")
    );
}

#[test]
fn talismans_fill_every_slot_in_order() {
    let plan = equipped();
    let names: Vec<_> = plan
        .talismans
        .iter()
        .map(|s| s.as_ref().map(|i| i.name.as_str()))
        .collect();
    assert_eq!(
        names,
        vec![
            Some("Radagon Icon"),
            Some("Erdtree's Favor +2"),
            Some("Graven-Mass Talisman"),
            Some("Ritual Shield Talisman"),
        ]
    );
}

#[test]
fn spells_are_memorised_in_order_and_fit() {
    let plan = equipped();
    assert_eq!(plan.spells.len(), 8);
    assert_eq!(plan.spells[0].name, "Great Oracular Bubble");
    assert_eq!(plan.spells[7].name, "Scholar's Armament");
}

#[test]
fn spells_beyond_the_memory_slots_are_refused_not_dropped() {
    let doc = model::parse(BUILD).expect("fixture parses");
    let cramped = Capacity {
        memory_slots: 3,
        ..Capacity::default()
    };
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), cramped);
    assert_eq!(plan.spells.len(), 3);
    assert_eq!(plan.rejected.len(), 5);
    assert!(
        plan.rejected
            .iter()
            .all(|r| r.reason.contains("memory slots"))
    );
    assert!(
        !plan.is_complete(),
        "an overrun must be visible, never silent"
    );
}

#[test]
fn talisman_slots_shrink_with_capacity() {
    let doc = model::parse(BUILD).expect("fixture parses");
    let cramped = Capacity {
        talismans: 2,
        ..Capacity::default()
    };
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), cramped);
    assert_eq!(plan.talismans.len(), 2);
    assert_eq!(
        plan.rejected.len(),
        2,
        "the two overflow talismans must be reported"
    );
}

#[test]
fn empty_quickbar_pouch_and_physick_stay_empty() {
    // This build equips no tools and no tears; the plan must not invent any.
    let plan = equipped();
    assert_eq!(plan.quickbar.len(), QUICKBAR_SLOTS);
    assert_eq!(plan.physick.len(), PHYSICK_SLOTS);
    assert!(plan.quickbar.iter().all(Option::is_none));
    assert!(plan.pouch.iter().all(Option::is_none));
    assert!(plan.physick.iter().all(Option::is_none));
    assert!(plan.great_rune.is_none());
}

#[test]
fn quickbar_and_pouch_split_at_ten() {
    // Synthetic: the fixture build equips no tools, so the boundary the planner
    // renders (`equipIndex < 10` is quickbar, `>= 10` is pouch) is pinned here.
    let doc = model::parse(
        r#"{"items":{"tools":{"slots":[
             {"name":"Fingerprint Nostrum","equipIndex":0},
             {"name":"Fingerprint Nostrum","equipIndex":9},
             {"name":"Fingerprint Nostrum","equipIndex":10},
             {"name":"Fingerprint Nostrum","equipIndex":15}]}}}"#,
    )
    .expect("synthetic doc parses");
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert!(plan.is_complete(), "rejected: {:?}", plan.rejected);
    assert!(plan.quickbar[0].is_some() && plan.quickbar[9].is_some());
    assert!(plan.pouch[0].is_some() && plan.pouch[5].is_some());
    assert_eq!(plan.quickbar.iter().filter(|s| s.is_some()).count(), 2);
    assert_eq!(plan.pouch.iter().filter(|s| s.is_some()).count(), 2);
}

#[test]
fn an_out_of_range_position_is_reported_rather_than_silently_clamped() {
    let doc = model::parse(
        r#"{"items":{"tools":{"slots":[{"name":"Fingerprint Nostrum","equipIndex":99}]}}}"#,
    )
    .expect("synthetic doc parses");
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert!(!plan.is_complete());
    assert!(plan.rejected[0].reason.contains("out of range"));
}

// ------------------------------------------------------------------ er-quickload.toml `build_url`

/// The exact block `er-quickload`'s `boilerplate_config` writes into a fresh `er-quickload.toml`,
/// minus the picker block. This is the file a player actually edits, so the scanner is held to it
/// rather than to a convenient shape: the commented example must not be read as a value, and the
/// key must survive being surrounded by the other keys' comments.
const PRODUCT_BOILERPLATE: &str = "\
# er-quickload runtime config (auto-created next to the game executable).
# All keys are optional; uncomment and edit as needed.
#
# save_file = 'C:\\path\\to\\ER0000.sl2'  # explicit read-only source
# slot = 0                               # character slot the autoload selects
# os_native_save_picker = false          # false=in-game browser, true=OS file dialog
# save_suppression_enabled = false
# build_url = 'https://er-build-planner.example/?b=af97a9da874151'
# The er-build-planner share link the System>Quit \"Load Build from URL\" row imports onto the
# character you are playing.
";

#[test]
fn the_untouched_product_config_configures_no_build() {
    // Every `build_url` in the shipped file is commented out, so a player who never edited it must
    // get "nothing to import" -- not the example link, which is not their build.
    assert_eq!(
        er_build_import_core::build_url_from_config(PRODUCT_BOILERPLATE),
        None
    );
}

#[test]
fn a_configured_build_url_is_read_in_every_spelling_the_file_allows() {
    for (line, expected) in [
        ("build_url = 'https://p/?b=abc123'", "https://p/?b=abc123"),
        ("build_url = \"https://p/?b=abc123\"", "https://p/?b=abc123"),
        ("build_url=https://p/?b=abc123", "https://p/?b=abc123"),
        (
            "   build_url   =   'https://p/?b=abc123'   ",
            "https://p/?b=abc123",
        ),
    ] {
        let contents = format!("{PRODUCT_BOILERPLATE}{line}\n");
        assert_eq!(
            er_build_import_core::build_url_from_config(&contents),
            Some(expected),
            "{line:?}"
        );
    }
}

#[test]
fn an_empty_or_absent_value_imports_nothing_rather_than_an_empty_build() {
    for contents in [
        "build_url =\n",
        "build_url = ''\n",
        "build_url = \"\"\n",
        "",
    ] {
        assert_eq!(
            er_build_import_core::build_url_from_config(contents),
            None,
            "{contents:?}"
        );
    }
}

/// The two halves have to agree: a value the scanner accepts still has to carry a share id, and the
/// self-contained `?i=` form -- which needs no network at all -- must be refused rather than fetched.
#[test]
fn the_configured_url_feeds_the_share_id_extractor() {
    let contents = "build_url = 'https://er-build-planner.example/?b=af97a9da874151'\n";
    let url = er_build_import_core::build_url_from_config(contents).expect("configured");
    assert_eq!(share_id_from_url(url), Some("af97a9da874151"));

    let self_contained = "build_url = 'https://er-build-planner.example/?i=eyJ2IjoxfQ'\n";
    let url = er_build_import_core::build_url_from_config(self_contained).expect("configured");
    assert_eq!(share_id_from_url(url), None);
}

// ------------------------------------------------------------------ in-game URL entry

use er_build_import_core::{BUILD_URL_PREFIX, UrlRejection, validate_build_url};

/// The editor opens pre-filled with the prefix, so the untouched field must refuse. If it did not,
/// pressing Accept without typing would start a fetch for a build id that is the empty string.
#[test]
fn the_untouched_prefill_is_refused() {
    assert_eq!(
        validate_build_url(BUILD_URL_PREFIX),
        Err(UrlRejection::Empty)
    );
    assert_eq!(validate_build_url(""), Err(UrlRejection::Empty));
    assert_eq!(validate_build_url("   "), Err(UrlRejection::Empty));
}

/// Completing the prefill -- the exact thing a player does -- must be accepted.
#[test]
fn typing_an_id_onto_the_prefix_is_accepted() {
    let typed = format!("{BUILD_URL_PREFIX}af97a9da874151");
    assert_eq!(validate_build_url(&typed), Ok("af97a9da874151"));
    // ...and a pasted-in link from any host, since the planner has moved domain before.
    assert_eq!(
        validate_build_url("https://somewhere.else/x?b=bc2a932db14675"),
        Ok("bc2a932db14675")
    );
}

/// A controller-typed field picks up stray whitespace; a trailing space is a slip, not a link.
#[test]
fn surrounding_whitespace_does_not_change_the_link() {
    let padded = format!("  {BUILD_URL_PREFIX}af97a9da874151\t");
    assert_eq!(validate_build_url(&padded), Ok("af97a9da874151"));
}

/// Every refusal names itself, and no two share a code -- the codes are the telemetry wire format.
#[test]
fn every_rejection_has_a_distinct_code_and_a_sentence() {
    let cases = [
        ("", UrlRejection::Empty),
        ("https://p/?i=eyJ2IjoxfQ", UrlRejection::SelfContained),
        ("https://p/?other=1", UrlRejection::NoShareId),
        ("https://p/no-query-at-all", UrlRejection::NoShareId),
        ("https://p/?b=", UrlRejection::MalformedShareId),
        ("https://p/?b=not an id", UrlRejection::MalformedShareId),
        ("https://p/?b=../../etc", UrlRejection::MalformedShareId),
    ];
    let mut codes = Vec::new();
    for (url, want) in cases {
        assert_eq!(validate_build_url(url), Err(want), "{url:?}");
        assert!(!want.indicator().is_empty(), "{want:?} has no sentence");
        assert_ne!(want.code(), 0, "0 is reserved for accepted");
        codes.push(want.code());
    }
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), 4, "rejection codes must be distinct");
}

/// The gate and the fetcher must never disagree: anything this accepts has to yield the same id to
/// `share_id_from_url`, which is what actually builds the request path. A link that validates but
/// does not resolve would be refused by nothing and then fetch nothing.
#[test]
fn acceptance_agrees_with_the_share_id_the_fetch_will_use() {
    for url in [
        "https://er-build-planner.nyasu.business/?b=af97a9da874151",
        "https://p/?x=1&b=abc123&y=2",
        "https://p/?b=abc123#fragment",
    ] {
        let accepted = validate_build_url(url).expect("valid");
        assert_eq!(Some(accepted), share_id_from_url(url), "{url:?}");
    }
    // ...and the converse: nothing this refuses may still resolve.
    for url in [
        "https://p/?b=",
        "https://p/?i=payload",
        "https://p/?b=bad id",
    ] {
        assert!(validate_build_url(url).is_err(), "{url:?}");
        assert_eq!(share_id_from_url(url), None, "{url:?}");
    }
}

// ---------------------------------------------------------------------------------------------
// The export direction. Every helper below is used by the Generate Build Link row to turn the live
// character back into a planner document, and every one of them is the inverse of something the
// importer does. An inverse that is subtly wrong does not fail loudly -- it produces a build that
// looks right and comes back a different weapon, or hand-swapped. So the pairs are tested as pairs.

#[test]
fn every_affinity_the_importer_adds_the_exporter_can_subtract() {
    use er_build_import_core::plan::{
        INFUSION_STEP, infusion_names, infusion_offset, split_armament_id,
    };

    // Misericorde, the worked example in the crate docs.
    const BASE: u32 = 1_070_000;
    for (index, name) in infusion_names().enumerate() {
        let offset = infusion_offset(Some(name))
            .unwrap_or_else(|| panic!("the importer must know the affinity {name:?}"));
        assert_eq!(offset, index as u32 * INFUSION_STEP, "{name} offset");

        let split = split_armament_id(BASE + offset);
        assert_eq!(split.row, BASE, "{name} base row");
        assert_eq!(
            split.row_with_affinity,
            BASE + offset,
            "{name} affinity row"
        );
        assert_eq!(split.level, 0, "{name} level");
        // Standard is index 0 and is spelled as an absent field, never as the word.
        let expected = (index != 0).then_some(name);
        assert_eq!(split.infusion, expected, "{name} round trip");

        // And the same ID at every upgrade level. The level lives in the id's last two digits, so
        // an exporter that does not take it off asks the message repository about a row that does
        // not exist -- which answers nothing, drops the slot, and empties the build.
        for level in 0..=25u16 {
            let split = split_armament_id(BASE + offset + u32::from(level));
            assert_eq!(split.row, BASE, "{name} +{level} base row");
            assert_eq!(split.infusion, expected, "{name} +{level} affinity");
            assert_eq!(split.level, level, "{name} +{level} level");
        }
    }
}

#[test]
fn a_levelled_armament_id_names_the_row_the_game_actually_has() {
    use er_build_import_core::plan::{armament_item_id, split_armament_id};

    // The exact pair from a live import log: the plan placed Keen Cross-Naginata (16110200) and
    // the slot came back holding 16110217, the same armament at +17. `EquipParamWeapon` has a row
    // for the first and none for the second (verified offline against the installed regulation),
    // so the split has to hand back the first.
    let split = split_armament_id(16_110_217);
    assert_eq!(split.row, 16_110_000);
    assert_eq!(split.row_with_affinity, 16_110_200);
    assert_eq!(split.infusion, Some("Keen"));
    assert_eq!(split.level, 17);

    // ...and the importer's own id builder is its exact inverse.
    assert_eq!(
        armament_item_id(split.row_with_affinity, split.level),
        16_110_217,
    );
}

#[test]
fn the_somber_scale_matches_the_planners_own_table() {
    use er_build_import_core::plan::somber_level_for_regular;

    // Transcribed from the live bundle's `lr`, which is what `getWeaponUpgradeLevel` maps a
    // character's `weaponUpgrade` through for any armament taking Somber Smithing Stones. It maps
    // the character-wide number only: a per-slot `upgrade` is already on the game's scale, because
    // the planner's slot editor caps that input at `getWeaponUpgradeLevel(weapon)` -- 10 for a
    // somber armament -- and stores the typed number unchanged.
    const PLANNER_TABLE: [u16; 26] = [
        0, 0, 1, 1, 1, 2, 2, 3, 3, 3, 4, 4, 5, 5, 5, 6, 6, 7, 7, 7, 8, 8, 9, 9, 9, 10,
    ];
    for (regular, somber) in PLANNER_TABLE.into_iter().enumerate() {
        assert_eq!(
            somber_level_for_regular(regular as u16),
            somber,
            "+{regular}"
        );
    }
    // A maxed character puts a somber armament at its own maximum, not at 25.
    assert_eq!(somber_level_for_regular(25), 10);
    // Out of range asks for the most the armament can take rather than for nothing.
    assert_eq!(somber_level_for_regular(99), 10);
}

#[test]
fn a_per_slot_upgrade_and_the_character_default_are_told_apart() {
    use er_build_import_core::plan::plan;
    use er_build_import_core::{
        catalog::{Kind, MapCatalog, entry},
        model,
    };

    let catalog = MapCatalog::new().with(Kind::Weapon, "Nagakiba", entry(1_070_000));
    let doc = model::parse(
        r#"{"weaponUpgrade": 17, "inventory": {"slots": [
            {"name": "Nagakiba"},
            {"name": "Nagakiba", "upgrade": 8}
        ]}}"#,
    )
    .expect("valid build document");
    let result = plan(&doc, &catalog);

    // The slot with no `upgrade` carries the character's number and says so, because that one has
    // to be mapped down for a somber armament and the other one must not be.
    assert_eq!(result.grants[0].reinforce_lv, 17);
    assert!(result.grants[0].upgrade_is_character_default);
    assert_eq!(result.grants[1].reinforce_lv, 8);
    assert!(!result.grants[1].upgrade_is_character_default);
}

#[test]
fn an_id_carrying_no_recognisable_affinity_is_taken_whole() {
    use er_build_import_core::plan::split_armament_id;

    // Offset 1300 is past the last affinity (Occult, 1200). Subtracting an invented amount would
    // silently rename the weapon, so the id is left alone and reported as having no affinity.
    let split = split_armament_id(1_071_300);
    assert_eq!(split.row, 1_071_300);
    assert_eq!(split.infusion, None);
    assert_eq!(split.level, 0);
}

#[test]
fn the_armament_hand_map_is_a_bijection_in_both_directions() {
    use er_build_import_core::equip::{
        ARMAMENT_CHR_ASM_SLOTS, armament_planner_index, armament_slot,
    };

    // Six planner indices onto six distinct ChrAsm slots, and back again.
    let mut seen = ARMAMENT_CHR_ASM_SLOTS;
    seen.sort_unstable();
    assert_eq!(
        seen,
        [0, 1, 2, 3, 4, 5],
        "the six hand slots, each used once"
    );

    for planner_index in 0..6u32 {
        let slot = armament_slot(planner_index).expect("every planner index maps to a slot");
        assert_eq!(
            armament_planner_index(slot),
            Some(planner_index),
            "slot {slot} must map back to planner index {planner_index}"
        );
    }
    // Out of range in both directions is None, not a wrapped index.
    assert_eq!(armament_slot(6), None);
    assert_eq!(armament_planner_index(6), None);
    assert_eq!(armament_planner_index(-1), None);

    // The hand convention itself: planner 0..2 is the right hand (odd slots), 3..5 the left.
    // If imported builds ever come out hand-swapped, this assertion is the one to invert, together
    // with `ARMAMENT_CHR_ASM_SLOTS` -- and inverting the table alone will fail here first.
    assert!(
        ARMAMENT_CHR_ASM_SLOTS[0..3]
            .iter()
            .all(|slot| slot % 2 == 1)
    );
    assert!(
        ARMAMENT_CHR_ASM_SLOTS[3..6]
            .iter()
            .all(|slot| slot % 2 == 0)
    );
}

#[test]
fn the_armour_slots_are_consecutive_from_protector_head() {
    use er_build_import_core::equip::{CHR_ASM_SLOT_PROTECTOR_HEAD, PROTECTOR_PARTS};

    // `ProtectorIndexToChrAsmSlot` is literally `index + ProtectorHead`, so the four planner keys
    // must be in that order -- the exporter walks them by offset and would mislabel armour if the
    // order drifted.
    assert_eq!(PROTECTOR_PARTS, ["head", "body", "arms", "legs"]);
    assert_eq!(CHR_ASM_SLOT_PROTECTOR_HEAD, 12);
}

// ------------------------------------------------- the plan is the denominator

use er_build_import_core::equip::{EquipLedger, PositionKind, PositionResult};

/// The synthetic build behind the accounting tests: two quickbar tools and two pouch tools on
/// top of the fixture's gear, i.e. exactly the family that used to leave the denominator.
fn with_tools() -> er_build_import_core::EquipPlan {
    let doc = model::parse(
        r#"{"items":{"tools":{"slots":[
             {"name":"Fingerprint Nostrum","equipIndex":0},
             {"name":"Fingerprint Nostrum","equipIndex":3},
             {"name":"Fingerprint Nostrum","equipIndex":10},
             {"name":"Fingerprint Nostrum","equipIndex":15}]},
           "crystalTears":["Fingerprint Nostrum",null]}}"#,
    )
    .expect("synthetic doc parses");
    equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default())
}

#[test]
fn quickbar_and_pouch_positions_carry_their_native_slots() {
    let plan = with_tools();
    let quick: Vec<_> = plan
        .positions()
        .into_iter()
        .filter(|p| p.kind.is_quick_dispatch())
        .map(|p| (p.kind, p.index, p.slot))
        .collect();
    // ConvertChrAsmSlotToQuickItemOrPouchSlot: 0x16..0x1F is quickbar 0..9, 0x20..0x25 is pouch.
    assert_eq!(
        quick,
        vec![
            (PositionKind::Quickbar, 0, Some(0x16)),
            (PositionKind::Quickbar, 3, Some(0x19)),
            (PositionKind::Pouch, 0, Some(0x20)),
            (PositionKind::Pouch, 5, Some(0x25)),
        ]
    );
}

#[test]
fn occupied_is_exactly_the_position_count() {
    // Two counts of the same thing are two chances to disagree; this pins them together.
    let plan = with_tools();
    assert_eq!(plan.occupied(), plan.positions().len());
    assert_eq!(plan.occupied(), 5, "4 tools + 1 tear");
}

#[test]
fn a_position_nobody_visits_is_unaccounted_not_absent() {
    // The regression. A pass that writes only the gear it attempted must not be able to print a
    // perfect score: the four tools stay in the denominator with no result at all.
    let plan = with_tools();
    let mut ledger = EquipLedger::new(&plan);
    ledger.record_kind(PositionKind::Physick, 0, PositionResult::Verified);

    let counts = ledger.counts();
    assert_eq!(counts.planned, 5);
    assert_eq!(counts.verified, 1);
    assert_eq!(counts.unaccounted, 4);
    assert!(!counts.reconciles());

    let headline = ledger.headline();
    assert!(headline.contains("5 planned"), "{headline}");
    assert!(headline.contains("4 unaccounted"), "{headline}");
    assert!(headline.contains("NOT EQUIPPED"), "{headline}");
    assert!(headline.contains("quickbar 0 (slot 22)"), "{headline}");
    assert!(headline.contains("pouch 5 (slot 37)"), "{headline}");
    assert!(headline.contains("never attempted"), "{headline}");
}

#[test]
fn a_full_pass_reconciles_and_says_so_plainly() {
    let plan = with_tools();
    let mut ledger = EquipLedger::new(&plan);
    for (index, _) in plan.positions().iter().enumerate() {
        ledger.record(index, PositionResult::Verified);
    }
    let counts = ledger.counts();
    assert!(counts.reconciles());
    assert_eq!(counts.verified, 5);
    assert!(ledger.failures().is_empty());
    assert!(ledger.headline().contains("5 planned = 5 verified"));
    assert!(!ledger.headline().contains("NOT EQUIPPED"));
}

#[test]
fn a_written_position_that_reads_back_wrong_is_named_on_the_headline() {
    let plan = with_tools();
    let mut ledger = EquipLedger::new(&plan);
    for (index, position) in plan.positions().iter().enumerate() {
        let result = if position.kind == PositionKind::Quickbar && position.index == 3 {
            PositionResult::Mismatch {
                expected: 0x400008AE,
                actual: -1,
            }
        } else {
            PositionResult::Verified
        };
        ledger.record(index, result);
    }
    let counts = ledger.counts();
    assert_eq!(counts.failed, 1);
    assert_eq!(counts.unaccounted, 0);
    assert!(!counts.reconciles());
    let headline = ledger.headline();
    assert!(
        headline.contains("1 POSITION(S) NOT EQUIPPED"),
        "{headline}"
    );
    assert!(headline.contains("quickbar 3 (slot 25)"), "{headline}");
    assert!(headline.contains("holds -1"), "{headline}");
}

#[test]
fn recording_a_position_the_plan_never_asked_for_is_refused() {
    // The other direction of the same fault: a write nobody planned must not be able to pad the
    // numerator.
    let plan = with_tools();
    let mut ledger = EquipLedger::new(&plan);
    assert!(ledger.record_kind(PositionKind::Quickbar, 0, PositionResult::Verified));
    assert!(!ledger.record_kind(PositionKind::Quickbar, 7, PositionResult::Verified));
    assert_eq!(ledger.counts().verified, 1);
}

#[test]
fn the_users_build_plans_twelve_positions_and_none_are_tools() {
    // The report that started this: "none of the heavy iron balls got equipped, but are in my
    // inventory". The planner document marks only two armaments equipped, and carries no tools
    // at all, so the twelve planned positions are 2 armaments + 4 armour + 4 talismans + 2
    // tears -- the two that used to fall out of the denominator were the PHYSICK, written by a
    // different call, not a dropped quickbar.
    let plan = equipped();
    let doc = model::parse(BUILD).expect("fixture parses");
    let carried = doc
        .inventory
        .slots
        .iter()
        .filter(|slot| slot.equip_index.is_none())
        .count();
    assert!(
        carried > 0,
        "the fixture must still carry unequipped armaments"
    );
    let kinds: Vec<_> = plan.positions().iter().map(|p| p.kind).collect();
    assert!(
        !kinds.iter().any(|k| k.is_quick_dispatch()),
        "a build with no tools plans no quick positions"
    );
}

#[test]
fn an_ash_of_war_is_encoded_as_a_gem_item_id() {
    // The bug this pins: `weaponSkill` must carry an `EquipParamGem` row under the game's gem
    // category nibble (8). The planner's own database tags ashes with nibble 2, and a runtime
    // catalog that resolved the ash name to its `SwordArtsParam` row instead produced a
    // well-formed value naming a row that does not exist -- every ash "resolved" and no weapon
    // came out carrying one.
    let (_, result) = planned();
    let catalog = fixture_catalog::catalog();
    let ash = catalog
        .lookup(Kind::AshOfWar, "Bloodhound's Step")
        .expect("fixture catalog has the ash");
    assert_eq!(
        ash.full_item_id & 0xF000_0000,
        0x2000_0000,
        "the planner tags ashes with nibble 2, which is not the game's gem category"
    );
    // Bloodhound's Step is EquipParamGem row 80100.
    assert_eq!(ash.full_item_id & 0x0FFF_FFFF, 80_100);

    let armament = result
        .grants
        .iter()
        .find(|grant| grant.label == "Miséricorde")
        .expect("the fixture's Miséricorde is granted");
    assert_eq!(armament.weapon_skill, GEM_ITEM_CATEGORY | 80_100);
    assert_eq!(armament.weapon_skill & 0xF000_0000, GEM_ITEM_CATEGORY);
    assert_eq!(
        armament.weapon_skill & 0x0FFF_FFFF,
        ash.full_item_id & 0x0FFF_FFFF
    );
}

#[test]
fn armaments_are_granted_in_payload_order_even_when_a_later_one_is_worn() {
    // The order the player sees. A build lists its armaments in `order`, and that is the order the
    // planner page shows and the only one they can check their inventory against.
    //
    // This test replaces `the_worn_copy_of_a_duplicated_armament_is_granted_first`, which asserted
    // the OPPOSITE: worn copies were hoisted ahead of everything else so the equip's item-id lookup
    // -- which the game answers with the lowest inventory index -- would land on the right twin.
    // That mitigation died with the gaitem-handle threading, which names one instance outright, and
    // it was costing the user a visibly-scrambled inventory (reported 2026-08-23 against build
    // 94252a868b4f2a: the two worn armaments were granted first while the payload puts them at
    // `order` 2 and 8). Twins are still indistinguishable by item id here -- that has not changed,
    // it is simply no longer the equip's question.
    let mut doc = model::BuildDoc {
        weapon_upgrade: 25,
        ..model::BuildDoc::default()
    };
    doc.inventory.slots = vec![
        model::Slot {
            name: "Miséricorde".into(),
            order: 0,
            infusion: Some("Magic".into()),
            weapon_art: Some("Carian Retaliation".into()),
            ..model::Slot::default()
        },
        model::Slot {
            name: "Miséricorde".into(),
            order: 1,
            infusion: Some("Magic".into()),
            weapon_art: Some("Bloodhound's Step".into()),
            equip_index: Some(0),
            ..model::Slot::default()
        },
    ];
    let result = plan(&doc, &fixture_catalog::catalog());
    assert_eq!(result.grants.len(), 2);
    assert_eq!(
        result.grants[0].item_id, result.grants[1].item_id,
        "the twins are indistinguishable by item id -- still true, just no longer load-bearing"
    );
    assert_eq!(
        result.grants[0].weapon_skill,
        GEM_ITEM_CATEGORY | 30_500,
        "payload order 0 is Carian Retaliation, and being unworn does not push it down the list"
    );
    assert_eq!(
        result.grants[1].weapon_skill,
        GEM_ITEM_CATEGORY | 80_100,
        "the WORN copy stays at payload order 1 rather than being hoisted to the front"
    );
}

#[test]
fn the_read_back_target_names_the_slot_and_the_gem() {
    // What the post-import read-back compares against: the fixture wears Miséricorde in planner
    // armament index 2, which is `ChrAsmSlot` 5.
    let (doc, _) = planned();
    let wants = equipped_armament_skills(&doc, &fixture_catalog::catalog());
    let worn = wants
        .iter()
        .find(|want| want.weapon == "Miséricorde")
        .expect("the fixture wears it");
    assert_eq!(worn.slot, 5);
    assert_eq!(worn.art.as_deref(), Some("Bloodhound's Step"));
    assert_eq!(worn.weapon_skill, GEM_ITEM_CATEGORY | 80_100);

    // A slot the build leaves without a skill is still listed, with the sentinel, so the
    // read-back reports it instead of skipping it.
    let bare = wants
        .iter()
        .find(|want| want.weapon == "Poisoned Hand")
        .expect("the fixture wears it");
    assert_eq!(bare.art, None);
    assert_eq!(bare.weapon_skill, NO_SKILL);
}

// ------------------------------------------- one item name is several item rows

use er_build_import_core::catalog::{MapCatalog, entry};

/// The flask rows exactly as the live catalog enumerated them, printed by the import's own
/// `COLLIDING NAME [tool]` lines on 2026-08-31: each upgrade level is its own `EquipParamGoods`
/// row pair under its own `+N` name.
fn flask_catalog() -> MapCatalog {
    let mut catalog = MapCatalog::new();
    for (name, ids) in [
        ("Flask of Crimson Tears", [0x4000_03E8u32, 0x4000_03E9]),
        ("Flask of Crimson Tears +1", [0x4000_03EA, 0x4000_03EB]),
        ("Flask of Crimson Tears +9", [0x4000_03FA, 0x4000_03FB]),
        ("Flask of Wondrous Physick", [0x4000_00FA, 0x4000_00FB]),
    ] {
        for id in ids {
            catalog.insert(Kind::Tool, name, entry(id));
        }
    }
    catalog
}

#[test]
fn an_upgraded_flask_is_the_same_item_under_a_row_the_name_does_not_reach() {
    let catalog = flask_catalog();
    // What `alternates` can see: the second row of the identical name, and nothing else. This is
    // the whole answer the grant path was given, and it is why the equip kept missing.
    assert_eq!(
        Catalog::alternates(&catalog, Kind::Tool, "Flask of Crimson Tears"),
        vec![0x4000_03E9]
    );
    // What the equip needs: every row that is this item at some upgrade level.
    assert_eq!(
        catalog.upgrade_variants(Kind::Tool, "Flask of Crimson Tears"),
        vec![
            0x4000_03E8,
            0x4000_03E9,
            0x4000_03EA,
            0x4000_03EB,
            0x4000_03FA,
            0x4000_03FB
        ]
    );
    // An item with no `+N` rows is not widened by a neighbour: the physick keeps its own two.
    assert_eq!(
        catalog.upgrade_variants(Kind::Tool, "Flask of Wondrous Physick"),
        vec![0x4000_00FA, 0x4000_00FB]
    );
}

#[test]
fn a_pouch_position_carries_every_row_of_its_item() {
    // The regression, in the shape the live run had it: pouch position 2 (ChrAsmSlot 34) asking
    // for a flask. Two consecutive imports recorded it not-in-inventory -- once for each row of
    // the unupgraded name -- while the character's upgraded flask sat in the pouch.
    let doc = model::parse(
        r#"{"items":{"tools":{"slots":[
             {"name":"Flask of Crimson Tears","equipIndex":12}]}}}"#,
    )
    .expect("synthetic doc parses");
    let plan = equip_plan(&doc, &flask_catalog(), Capacity::default());
    let pouch = plan.pouch[2].as_ref().expect("pouch position 2 is filled");
    assert_eq!(pouch.item_id, 0x4000_03E8, "the build names the base row");
    assert!(
        pouch.also_known_as.contains(&0x4000_03FA),
        "a character who has drunk Sacred Tears holds only the +N row: {:?}",
        pouch.also_known_as
    );
    assert!(
        !pouch.also_known_as.contains(&pouch.item_id),
        "the primary is not its own alternate: {:?}",
        pouch.also_known_as
    );
}

#[test]
fn the_balance_line_reconciles_or_names_its_own_casualties() {
    // Every number that would have caught the last defect was already in the log, in separate
    // lines, and nothing subtracted them. This is that subtraction.
    let plan = with_tools();
    let mut ledger = EquipLedger::new(&plan);
    ledger.record(0, PositionResult::Verified);
    ledger.record(1, PositionResult::Already);
    ledger.record(2, PositionResult::NotInInventory);
    ledger.record(
        3,
        PositionResult::Mismatch {
            expected: 1_030_600,
            actual: 1_030_625,
        },
    );
    // Position 4 is never visited at all, and must not leave the denominator with itself.

    let counts = ledger.counts();
    assert_eq!(counts.planned, 5);
    assert_eq!(counts.reached(), 3);
    assert_eq!(counts.never_written(), 2);
    assert_eq!(
        counts.failed,
        counts.mismatched + counts.not_in_inventory + counts.not_attempted,
        "the one failure number is the sum of the three ways to fail"
    );
    assert_eq!(
        counts.planned,
        counts.reached() + counts.never_written(),
        "the balance balances"
    );

    let balance = ledger.balance(1);
    assert!(
        balance.contains("5 planned = 3 reached + 2 never written"),
        "{balance}"
    );
    assert!(balance.contains("2 hold the build's item"), "{balance}");
    assert!(balance.contains("and 1 do not"), "{balance}");
    assert!(
        balance.contains("1 of those were verified first and stripped afterwards"),
        "{balance}"
    );
}

// ---------------------------------------------------------------------------
// Ammunition -- the category whose shape is not a slot list.
// ---------------------------------------------------------------------------

/// A build that equips ammunition, in the exact shape the planner writes.
///
/// Hand-written rather than captured, because none of the three captured fixtures equips any: two
/// carry `"ammo": {}` (the planner's own default for a new character) and the third predates the
/// feature entirely. The shape below is transcribed from the planner's own code, not guessed --
/// its picker does `character.items.ammo[slot] = ammo.name` for `slot` in `['arrow1','arrow2']`
/// and `['bolt1','bolt2']`, and its equip view reads each back as `e.arrow1 ? {name: e.arrow1} : null`.
/// So the value is a bare name and the key is the equip position: no `order`, no `equipIndex`, no
/// `upgrade`, no nesting.
const AMMO_BUILD: &str = r#"{
    "id": "ammotest",
    "name": "Archer",
    "items": {
        "ammo": {
            "arrow1": "Bone Arrow",
            "arrow2": "Great Arrow",
            "bolt1": "Bolt",
            "bolt2": "Ballista Bolt"
        }
    }
}"#;

/// Every captured fixture parses with no ammunition, whether it says `{}` or says nothing.
///
/// Both spellings are real and they must not be distinguishable: the planner writes `{}` into a
/// new character, DELETES the whole object when the last slot is emptied, and only grew the key at
/// version 3.9 -- so the 3.7.7 fixture has no `ammo` at all. A model that required the key would
/// fail on one third of the corpus.
#[test]
fn the_captured_builds_equip_no_ammunition() {
    for (label, body) in [("af97a9da874151", BUILD), ("82086df03c4b8e", SETS_BUILD)] {
        let doc = model::parse(body).unwrap_or_else(|e| panic!("{label} parses: {e}"));
        assert!(doc.items.ammo.is_empty(), "{label} equips ammunition");
        assert_eq!(
            doc.items.ammo.positions(),
            [
                ("arrow1", None),
                ("bolt1", None),
                ("arrow2", None),
                ("bolt2", None)
            ],
            "{label}"
        );
    }
    // And the raw JSON agrees about which of the two spellings each one uses, so a future capture
    // that changes the shape cannot pass this by being parsed leniently.
    let raw: serde_json::Value = serde_json::from_str(SETS_BUILD).expect("raw JSON");
    assert_eq!(
        raw["items"]["ammo"],
        serde_json::json!({}),
        "82086df03c4b8e should carry the empty object"
    );
    let raw: serde_json::Value = serde_json::from_str(BUILD).expect("raw JSON");
    assert!(
        raw["items"].get("ammo").is_none(),
        "af97a9da874151 predates the feature and should carry no ammo key at all"
    );
}

/// `items.ammo` carries names keyed by position, and nothing that looks like a quantity.
///
/// The counterpart to `the_payload_states_no_quantities` for the one category whose shape differs
/// from every other. If the planner ever gives an ammo entry a body -- a count, a `slots` array,
/// an object -- this fails, and the importer should honour what it says instead of the item's own
/// quiver limit.
#[test]
fn an_ammo_entry_is_a_bare_name_and_states_no_quantity() {
    let raw: serde_json::Value = serde_json::from_str(AMMO_BUILD).expect("fixture parses");
    let ammo = raw["items"]["ammo"].as_object().expect("ammo is an object");
    let keys: Vec<&str> = ammo.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["arrow1", "arrow2", "bolt1", "bolt2"]);
    for (key, value) in ammo {
        assert!(
            value.is_string(),
            "ammo.{key} should be a bare name, not {value}"
        );
    }
}

/// The four positions come out interleaved, because `ChrAsmSlot` is.
///
/// The planner groups them (`arrow1, arrow2` then `bolt1, bolt2`); the engine does not
/// (`Arrow1 = 6, Bolt1 = 7, Arrow2 = 8, Bolt2 = 9`). Reading them in the planner's order and
/// adding the base slot would put every bolt in an arrow slot, so the interleave is asserted at
/// the point it is decided rather than left to the equip plan to get right.
#[test]
fn the_ammo_positions_are_in_chr_asm_slot_order() {
    let doc = model::parse(AMMO_BUILD).expect("fixture parses");
    assert_eq!(
        doc.items.ammo.positions(),
        [
            ("arrow1", Some("Bone Arrow")),
            ("bolt1", Some("Bolt")),
            ("arrow2", Some("Great Arrow")),
            ("bolt2", Some("Ballista Bolt")),
        ]
    );
}

/// Ammunition is granted at the engine's own quiver limit, and is not an armament.
///
/// The quantity is `EquipParamWeapon.maxArrowQuantity`, which is what `::GetMaxItemQuantity`
/// returns for `weaponCategory` 13 and 14 -- a different field in a different table from the
/// `maxNum` behind every consumable. The numbers below are the installed 1.17 regulation's:
/// ordinary arrows and bolts 99, Great Arrows 30, Ballista Bolts 20.
///
/// `armament: false` is the load-bearing half. Ammunition carries the weapon category nibble, so
/// the runtime's old `item_id & 0xF0000000 == 0` test would send a quiver of arrows down the
/// armament mint path -- one `GaItemHandle`, an upgrade level, a gem mount -- none of which an
/// arrow has.
#[test]
fn ammunition_is_granted_by_the_quiver_limit_and_never_as_an_armament() {
    let doc = model::parse(AMMO_BUILD).expect("fixture parses");
    let plan = plan(&doc, &fixture_catalog::catalog());
    assert!(plan.unresolved.is_empty(), "{:?}", plan.unresolved);

    let by_name: std::collections::BTreeMap<&str, &er_build_import_core::plan::Grant> =
        plan.grants.iter().map(|g| (g.label.as_str(), g)).collect();
    assert_eq!(by_name["Bone Arrow"].quantity, 99);
    assert_eq!(by_name["Bolt"].quantity, 99);
    assert_eq!(by_name["Great Arrow"].quantity, 30);
    assert_eq!(by_name["Ballista Bolt"].quantity, 20);

    for grant in &plan.grants {
        assert!(
            !grant.armament,
            "{:?} was flagged an armament; the mint path would give it a level and a gem",
            grant.label
        );
        // Ammunition still carries the weapon category nibble -- which is exactly why the flag
        // above cannot be derived from the id.
        assert_eq!(grant.item_id & 0xF000_0000, 0, "{:?}", grant.label);
        assert_eq!(grant.weapon_skill, NO_SKILL);
        assert_eq!(grant.reinforce_lv, 0);
    }
}

/// A row that declares no quiver limit is granted one, not the engine's fallback.
///
/// Same rule as a consumable whose `maxNum` is zero: handing a player a pile of something the
/// table has no opinion about is the over-grant this avoids.
#[test]
fn ammunition_with_no_declared_limit_is_granted_one() {
    let mut catalog = fixture_catalog::catalog();
    catalog.insert(
        Kind::Ammo,
        "Unmeasured Arrow",
        er_build_import_core::catalog::entry(0x02FB65B0),
    );
    let doc = model::parse(r#"{"items":{"ammo":{"arrow1":"Unmeasured Arrow"}}}"#).expect("parses");
    let plan = plan(&doc, &catalog);
    assert_eq!(plan.grants.len(), 1);
    assert_eq!(plan.grants[0].quantity, 1);
}

/// The four ammunition positions reach `ChrAsmSlot` 6, 7, 8 and 9.
#[test]
fn the_ammo_equip_positions_land_on_the_native_ammo_slots() {
    let doc = model::parse(AMMO_BUILD).expect("fixture parses");
    let equips = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert!(equips.rejected.is_empty(), "{:?}", equips.rejected);

    let placed: Vec<(i32, String)> = equips
        .positions()
        .into_iter()
        .filter(|position| position.kind == PositionKind::Ammo)
        .map(|position| {
            (
                position.slot.expect("an ammo position has a native slot"),
                position.item.name.clone(),
            )
        })
        .collect();
    assert_eq!(
        placed,
        vec![
            (6, "Bone Arrow".to_owned()),    // ChrAsmSlot::Arrow1
            (7, "Bolt".to_owned()),          // ChrAsmSlot::Bolt1
            (8, "Great Arrow".to_owned()),   // ChrAsmSlot::Arrow2
            (9, "Ballista Bolt".to_owned()), // ChrAsmSlot::Bolt2
        ]
    );
    // And an empty position leaves a hole rather than shifting the ones after it.
    let doc = model::parse(r#"{"items":{"ammo":{"bolt2":"Ballista Bolt"}}}"#).expect("parses");
    let equips = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    let slots: Vec<i32> = equips
        .positions()
        .into_iter()
        .filter(|position| position.kind == PositionKind::Ammo)
        .filter_map(|position| position.slot)
        .collect();
    assert_eq!(slots, vec![9]);
}

/// An unresolvable ammo name is reported, and named by the position the author sees.
#[test]
fn unresolvable_ammunition_is_rejected_by_its_planner_key() {
    let doc = model::parse(r#"{"items":{"ammo":{"bolt1":"Nonexistent Bolt"}}}"#).expect("parses");
    let equips = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert_eq!(equips.rejected.len(), 1);
    assert_eq!(equips.rejected[0].name, "Nonexistent Bolt");
    assert!(
        equips.rejected[0].reason.contains("bolt1"),
        "{}",
        equips.rejected[0].reason
    );
}

// ------------------------------------------------------------------ loadout sets

/// The 22634-byte body of `GET /inventories/82086df03c4b8e`, a build carrying six
/// armament sets, six talisman sets and three armour sets. Kept whole rather than
/// trimmed: the collisions this exercises live in rows that a "tidy" fixture would
/// have deleted as noise.
const SETS_BUILD: &str = include_str!("fixtures/build-82086df03c4b8e.json");

fn sets_build() -> model::BuildDoc {
    model::parse(SETS_BUILD).expect("sets fixture parses")
}

fn armament_names(plan: &er_build_import_core::EquipPlan) -> Vec<Option<&str>> {
    plan.armaments
        .iter()
        .map(|slot| slot.as_ref().map(|item| item.name.as_str()))
        .collect()
}

#[test]
fn the_payload_carries_named_sets_with_one_active_per_category() {
    let doc = sets_build();
    let names: Vec<_> = doc.sets.weapons.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Default", "PSGKAT", "2H", "PS Katana", "Urumi", "Bows"]
    );
    // Three categories, each with its own switch -- not one build-wide index.
    assert_eq!(doc.sets.active_weapons(), 0);
    assert_eq!(doc.sets.active_talismans(), 0);
    assert_eq!(doc.sets.active_protectors(), 0);
    assert_eq!(doc.sets.talismans.len(), 6);
    assert_eq!(doc.sets.protectors.len(), 3);
}

#[test]
fn equip_index_is_only_a_cache_of_the_active_sets_entry() {
    // The invariant `setSlotEquipIndex` maintains, checked against every row of a
    // real six-set build: `equipIndex == equipSet[active]`, always. That is why
    // reading `equipSet` selects the same rows -- and why it is safe to stop
    // trusting `equipIndex` and derive the selection instead.
    let doc = sets_build();
    let lists = [&doc.inventory, &doc.talismans];
    let mut checked = 0;
    for list in lists.into_iter().chain(doc.protectors.values()) {
        for slot in &list.slots {
            if slot.equip_set.is_none() && slot.equip_index.is_none() {
                continue;
            }
            assert_eq!(
                slot.equip_index,
                slot.equip_index_in_set(0),
                "{} disagrees with its own equipSet",
                slot.name
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked, 43,
        "19 armament rows, 14 talismans and 10 armour pieces carry a set assignment"
    );
}

#[test]
fn only_the_active_armament_set_is_equipped() {
    let plan = equip_plan(
        &sets_build(),
        &fixture_catalog::catalog(),
        Capacity::default(),
    );
    assert_eq!(
        armament_names(&plan),
        vec![
            Some("Shamshir"),
            Some("Mis\u{e9}ricorde"),
            None,
            Some("Mis\u{e9}ricorde"),
            Some("Frenzied Flame Seal"),
            Some("Twinbird Kite Shield"),
        ]
    );
    // Rows that belong to PSGKAT / 2H / PS Katana / Urumi / Bows must not appear
    // anywhere -- not placed, and not rejected either, because they were never
    // candidates for this loadout.
    for name in [
        "Nagakiba",
        "Guardian's Swordspear",
        "Rakshasa's Great Katana",
    ] {
        assert!(
            !plan.armaments.iter().flatten().any(|i| i.name == name),
            "{name} belongs to another set"
        );
        assert!(
            !plan.rejected.iter().any(|r| r.name == name),
            "{name} should not even be considered"
        );
    }
}

#[test]
fn the_contested_armament_positions_go_the_way_the_planner_shows_them() {
    // Three rows claim armament position 0 and three claim position 4, all inside
    // the active set. The planner's own `WeaponEquipSlots` folds them with a
    // `reduce` that overwrites, so the last row in the list is what its author
    // sees, and that is what gets equipped.
    let plan = equip_plan(
        &sets_build(),
        &fixture_catalog::catalog(),
        Capacity::default(),
    );
    let contested: Vec<_> = plan
        .contested
        .iter()
        .map(|c| {
            (
                c.position.as_str(),
                c.winner.as_str(),
                c.losers.iter().map(String::as_str).collect::<Vec<_>>(),
            )
        })
        .collect();
    assert_eq!(
        contested,
        vec![
            (
                "armament slot 0",
                "Shamshir",
                vec!["Mis\u{e9}ricorde", "Shamshir"]
            ),
            (
                "armament slot 1",
                "Mis\u{e9}ricorde",
                vec!["Mis\u{e9}ricorde"]
            ),
            (
                "armament slot 4",
                "Frenzied Flame Seal",
                vec!["Mis\u{e9}ricorde", "Frenzied Flame Seal"]
            ),
        ]
    );
}

#[test]
fn a_losing_claimant_is_rejected_so_no_caller_can_miss_the_collision() {
    // The whole point: a plan that quietly drops five armaments is
    // indistinguishable from a plan that placed them. `rejected` is what the
    // runtime already logs, so the losers go there too rather than only into the
    // structured list.
    let plan = equip_plan(
        &sets_build(),
        &fixture_catalog::catalog(),
        Capacity::default(),
    );
    assert!(
        !plan.is_complete(),
        "a contested build is not a clean import"
    );
    let losses: Vec<_> = plan
        .rejected
        .iter()
        .map(|r| (r.name.as_str(), r.reason.as_str()))
        .collect();
    assert_eq!(
        losses,
        vec![
            (
                "Mis\u{e9}ricorde",
                "armament slot 0 already claimed by \"Shamshir\""
            ),
            (
                "Shamshir",
                "armament slot 0 already claimed by \"Shamshir\""
            ),
            (
                "Mis\u{e9}ricorde",
                "armament slot 1 already claimed by \"Mis\u{e9}ricorde\""
            ),
            (
                "Mis\u{e9}ricorde",
                "armament slot 4 already claimed by \"Frenzied Flame Seal\""
            ),
            (
                "Frenzied Flame Seal",
                "armament slot 4 already claimed by \"Frenzied Flame Seal\""
            ),
        ]
    );
    assert_eq!(
        plan.contested.len(),
        3,
        "five losses across three positions"
    );
}

#[test]
fn the_contest_winner_keeps_its_own_affinity() {
    // Position 1 is claimed by two Miséricordes that differ only by affinity:
    // a Keen one earlier in the list and a Lightning one later. Asserting on the
    // name alone would pass either way, so pin the id: last-wins must bring the
    // Lightning row's `+600`, not the Keen row's `+200`.
    let plan = equip_plan(
        &sets_build(),
        &fixture_catalog::catalog(),
        Capacity::default(),
    );
    const MISERICORDE: u32 = 0x000F_B770;
    let slot = plan.armaments[1].as_ref().expect("position 1 is filled");
    assert_eq!(slot.item_id, MISERICORDE + 600, "Lightning, not Keen");
    let slot = plan.armaments[0].as_ref().expect("position 0 is filled");
    assert_eq!(slot.item_id, 0x006B_44F0 + 200, "the Keen Shamshir");
}

#[test]
fn talismans_and_armour_resolve_against_their_own_sets() {
    let plan = equip_plan(
        &sets_build(),
        &fixture_catalog::catalog(),
        Capacity::default(),
    );
    let talismans: Vec<_> = plan
        .talismans
        .iter()
        .map(|s| s.as_ref().map(|i| i.name.as_str()))
        .collect();
    assert_eq!(
        talismans,
        vec![
            Some("Blue-Feathered Branchsword"),
            Some("Crimson Amber Medallion +3"),
            Some("Erdtree's Favor +2"),
            Some("Bull-Goat's Talisman"),
        ]
    );
    assert_eq!(
        plan.head.as_ref().map(|i| i.name.as_str()),
        Some("Divine Beast Helm")
    );
    assert_eq!(
        plan.body.as_ref().map(|i| i.name.as_str()),
        Some("Armor of Solitude")
    );
    assert_eq!(
        plan.arms.as_ref().map(|i| i.name.as_str()),
        Some("Gauntlets of Solitude")
    );
    assert_eq!(
        plan.legs.as_ref().map(|i| i.name.as_str()),
        Some("Greaves of Solitude")
    );
    // Rakshasa is the armour set 2 wears; set 0 must not pick any of it up.
    for worn in [&plan.head, &plan.body, &plan.arms, &plan.legs] {
        let name = worn.as_ref().map(|i| i.name.as_str()).unwrap_or_default();
        assert!(!name.starts_with("Rakshasa"), "{name} is set 2's armour");
    }
}

#[test]
fn switching_the_active_armament_set_switches_only_the_armaments() {
    // `sets` is per category, so activating the "PS Katana" armament set must not
    // disturb the talismans or armour -- which is exactly the mistake a single
    // build-wide active index would make.
    let mut doc = sets_build();
    doc.sets.weapons[0].active = false;
    doc.sets.weapons[3].active = true;
    assert_eq!(doc.sets.weapons[3].name, "PS Katana");
    assert_eq!(doc.sets.active_weapons(), 3);

    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert_eq!(
        armament_names(&plan),
        vec![
            Some("Nagakiba"),
            None,
            None,
            Some("Nagakiba"),
            Some("Frenzied Flame Seal"),
            None,
        ]
    );
    // Not one Miséricorde survives: every one of them is a set-0 row.
    assert!(
        !plan
            .armaments
            .iter()
            .flatten()
            .any(|i| i.name.contains('\u{e9}')),
        "set 3 has no daggers"
    );
    assert_eq!(
        plan.talismans[0].as_ref().map(|i| i.name.as_str()),
        Some("Blue-Feathered Branchsword"),
        "the talisman set did not move"
    );
    assert_eq!(
        plan.head.as_ref().map(|i| i.name.as_str()),
        Some("Divine Beast Helm"),
        "the armour set did not move"
    );
}

#[test]
fn a_row_carrying_only_equip_index_still_equips() {
    // Builds authored before the planner grew sets have no `sets` key and no
    // `equipSet`. The planner migrates such a row to `equipSet = [equipIndex]`,
    // i.e. set 0, and so must the importer -- otherwise every older share link
    // imports a naked character.
    let doc = model::parse(
        r#"{"inventory":{"slots":[{"name":"Shamshir","order":0,"equipIndex":2}]},
            "talismans":{"slots":[{"name":"Radagon Icon","order":0,"equipIndex":1}]}}"#,
    )
    .expect("legacy doc parses");
    assert!(doc.sets.weapons.is_empty());
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert!(plan.is_complete(), "rejected: {:?}", plan.rejected);
    assert_eq!(armament_names(&plan)[2], Some("Shamshir"));
    assert_eq!(
        plan.talismans[1].as_ref().map(|i| i.name.as_str()),
        Some("Radagon Icon")
    );
}

#[test]
fn a_row_equipped_only_in_an_inactive_set_is_left_carried() {
    // The `null` holes are the whole encoding: this row is position 0 of set 1,
    // and nothing at all in the active set 0.
    let doc = model::parse(
        r#"{"sets":{"weapons":[{"name":"Default","active":true},{"name":"Bows"}]},
            "inventory":{"slots":[{"name":"Shamshir","order":0,"equipSet":[null,0]}]}}"#,
    )
    .expect("doc parses");
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert!(plan.armaments.iter().all(Option::is_none));
    assert!(plan.rejected.is_empty(), "not equipped is not a failure");
    assert!(plan.contested.is_empty());
}

#[test]
fn armour_takes_the_first_claimant_the_way_the_planner_does() {
    // `ProtectorEquipSlots` resolves with `find`, not a fold, so armour breaks a
    // tie the opposite way to armaments. And the value is meaningless: the planner
    // writes a constant `1` for every worn piece, so it is membership that counts.
    let doc = model::parse(
        r#"{"sets":{"protectors":[{"name":"Default","active":true}]},
            "protectors":{"head":{"slots":[
                {"name":"Divine Beast Helm","order":0,"equipSet":[1],"equipIndex":1},
                {"name":"Mushroom Crown","order":1,"equipSet":[1],"equipIndex":1}]}}}"#,
    )
    .expect("doc parses");
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert_eq!(
        plan.head.as_ref().map(|i| i.name.as_str()),
        Some("Divine Beast Helm")
    );
    assert_eq!(plan.contested.len(), 1);
    assert_eq!(plan.contested[0].position, "head armour");
    assert_eq!(plan.contested[0].losers, vec!["Mushroom Crown"]);
}

#[test]
fn the_rest_of_the_real_build_still_imports_whole() {
    // Everything the sets machinery does not touch: tools carry no `equipSet` at
    // all, and spells, tears and the great rune are set-free too.
    let plan = equip_plan(
        &sets_build(),
        &fixture_catalog::catalog(),
        Capacity::default(),
    );
    assert_eq!(
        plan.quickbar[5].as_ref().map(|i| i.name.as_str()),
        Some("Clarifying Boluses")
    );
    assert_eq!(
        plan.quickbar[7].as_ref().map(|i| i.name.as_str()),
        Some("Neutralizing Boluses")
    );
    assert_eq!(
        plan.pouch[0].as_ref().map(|i| i.name.as_str()),
        Some("Blessing of Marika")
    );
    assert_eq!(
        plan.pouch[2].as_ref().map(|i| i.name.as_str()),
        Some("Flask of Cerulean Tears")
    );
    assert_eq!(
        plan.physick[0].as_ref().map(|i| i.name.as_str()),
        Some("Crimsonwhorl Bubbletear")
    );
    assert_eq!(plan.spells.len(), 1);
    assert_eq!(plan.spells[0].name, "Bestial Vitality");
    assert_eq!(
        plan.great_rune.as_ref().map(|i| i.name.as_str()),
        Some("Mohg's Great Rune")
    );
    assert!(plan.two_handing);
}

/// The +0 bug, 2026-08-23. A merged import handed the player thirty armaments at +0 while its own
/// read-back reported `+25 -> +25`, because the level was written only to
/// `CSWepGaitemIns::reinforcement` and never folded into the item id.
///
/// The id is the half the player sees. `CSGaitemImp::GetGaItemHandleWeapon` stores whatever id it
/// is handed verbatim (`CSGaitemIns::SetItemIdWithWeaponCategory`), and the character's own
/// weapons come back from the same getter carrying their level: `12531125`, `30190825`. A weapon
/// minted from a bare `base + affinity` id is a +0 weapon no matter what the instance field says.
#[test]
fn the_upgrade_level_rides_in_the_item_id() {
    // base + affinity, the id the plan names -- and the id that shipped, at +0.
    let great_stars_blood = 12181200;
    assert_eq!(armament_item_id(great_stars_blood, 25), 12181225);
    assert_eq!(armament_item_id(great_stars_blood, 0), great_stars_blood);

    // Somber armaments stop at +10; the caller clamps, this only carries the number.
    assert_eq!(armament_item_id(11500000, 10), 11500010);

    // The level never bleeds into the affinity digits: +25 on Standard must not become Heavy.
    for level in 0u16..=25 {
        let id = armament_item_id(2020100, level);
        assert_eq!(id / 100 * 100, 2020100, "affinity moved at +{level}");
        assert_eq!(u16::try_from(id % 100).unwrap(), level);
    }
}

/// The shape the reference exporter emits, and the reason the id half was missed: the record
/// carries the level twice, and porting only the `reinforceLv` field looks complete.
#[test]
fn a_planned_armament_keeps_its_level_in_the_separate_field_too() {
    let (_doc, plan) = planned();
    let armament = plan
        .grants
        .iter()
        .find(|grant| grant.reinforce_lv > 0)
        .expect("the fixture build upgrades at least one armament");
    // The plan still names the unlevelled id; folding happens at the mint, where the clamp
    // against the armament's real `ReinforceParamWeapon` rows is known.
    assert_eq!(armament.item_id % 100, 0);
    assert!(armament.reinforce_lv > 0);
    assert_eq!(
        armament_item_id(armament.item_id, armament.reinforce_lv) % 100,
        u32::from(armament.reinforce_lv)
    );
}

// ---------------------------------------------------------------------------
// How many of each -- the half of a grant the payload does not carry.
// ---------------------------------------------------------------------------

/// The payload has no count field, on tools or on anything else.
///
/// This is the fact the whole quantity decision rests on, so it is asserted rather than assumed:
/// if the planner ever grows a per-slot count, this test fails and the importer should honour the
/// stated number instead of the item's own limit. Checked against the raw JSON, not the model,
/// because `serde` drops unknown keys silently and a model-shaped check could never see one
/// arrive.
#[test]
fn the_payload_states_no_quantities() {
    for body in [BUILD, SETS_BUILD] {
        let doc: serde_json::Value = serde_json::from_str(body).expect("fixture parses as JSON");
        let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let mut stack = vec![doc];
        while let Some(node) = stack.pop() {
            match node {
                serde_json::Value::Object(map) => {
                    for (key, value) in map {
                        keys.insert(key);
                        stack.push(value);
                    }
                }
                serde_json::Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }
        for forbidden in ["quantity", "count", "amount", "qty", "stack", "num"] {
            assert!(
                !keys.contains(forbidden),
                "the payload grew a `{forbidden}` key -- honour it instead of `max_stored`"
            );
        }
    }
}

/// A consumable is granted the number the game declares, not one.
///
/// Fingerprint Nostrum's `EquipParamGoods.maxNum` is 10 in the installed regulation, and ten is
/// what a build listing it is asking for. One was the bug: `max_stored` was declared and never
/// populated, so `unwrap_or(1)` made every consumable a single item.
#[test]
fn a_consumable_is_granted_the_games_own_hold_limit() {
    let (doc, plan) = planned();
    let tool = doc
        .items
        .tools
        .slots
        .first()
        .expect("the fixture build carries a tool");
    assert_eq!(tool.name, "Fingerprint Nostrum");
    let grant = plan
        .grants
        .iter()
        .find(|grant| grant.label == tool.name)
        .expect("the tool is granted");
    assert_eq!(
        grant.quantity, 10,
        "a consumable is granted its own EquipParamGoods.maxNum"
    );
}

/// Everything that is not a consumable is granted exactly one, because one is what it means.
#[test]
fn only_consumables_are_granted_in_numbers() {
    let (doc, plan) = planned();
    let consumables: std::collections::BTreeSet<&str> = doc
        .items
        .tools
        .slots
        .iter()
        .map(|slot| slot.name.as_str())
        .collect();
    for grant in &plan.grants {
        if consumables.contains(grant.label.as_str()) {
            continue;
        }
        assert_eq!(
            grant.quantity, 1,
            "{:?} is not a consumable and must be granted once",
            grant.label
        );
    }
}

/// A build asking for the same tools twice plans the same numbers, and the runtime reconciles to
/// them -- so the target being high does not make the import cumulative.
#[test]
fn planning_the_same_build_twice_asks_for_the_same_number() {
    let (_, first) = planned();
    let (_, second) = planned();
    assert_eq!(first.grants, second.grants);
}

/// The absurd tail of `maxNum` is clamped, and the clamp is the only thing that moves.
///
/// Furlcalling Finger Remedy, Ruin Fragment and Roundrock declare 999 in the installed 1.17
/// regulation. A build that merely lists one is not asking for nine hundred of it.
#[test]
fn a_thousand_wide_hold_limit_is_capped_at_the_engines_own_ceiling() {
    use er_build_import_core::catalog::{Entry, Kind, MapCatalog};

    let table = |max_stored| {
        MapCatalog::new().with(
            Kind::Tool,
            "Ruin Fragment",
            Entry {
                full_item_id: 0x400006E0,
                max_stored,
                somber: false,
                pot_group: None,
            },
        )
    };
    let quantity_for = |max_stored| {
        let doc = model::parse(r#"{"items":{"tools":{"slots":[{"name":"Ruin Fragment"}]}}}"#)
            .expect("parses");
        plan(&doc, &table(max_stored)).grants[0].quantity
    };

    assert_eq!(
        quantity_for(Some(999)),
        99,
        "clamped to the engine's own 99"
    );
    assert_eq!(quantity_for(Some(99)), 99, "at the ceiling, untouched");
    assert_eq!(quantity_for(Some(10)), 10, "under the ceiling, untouched");
    // A row that declares nothing gets one, not the 99 the engine would hand out for it. Handing
    // a player ninety-nine of something the game itself has no opinion about is the over-grant
    // this path exists to avoid.
    assert_eq!(quantity_for(None), 1, "no declared limit means one");
    assert_eq!(quantity_for(Some(0)), 1, "a zero limit still means one");
}

/// A crystal tear is looked up as a tool and still granted once -- the physick holds two, and
/// every tear row declares `maxNum = 1`.
#[test]
fn a_crystal_tear_is_granted_once() {
    use er_build_import_core::catalog::{Entry, Kind, MapCatalog};

    let catalog = MapCatalog::new().with(
        Kind::Tool,
        "Opaline Hardtear",
        Entry {
            full_item_id: 0x40002B03,
            // Deliberately a number the tear does not have: if the tear path ever started
            // reading `max_stored`, this test would catch it handing out fifty.
            max_stored: Some(50),
            somber: false,
            pot_group: None,
        },
    );
    let doc =
        model::parse(r#"{"items":{"crystalTears":["Opaline Hardtear",null]}}"#).expect("parses");
    let plan = plan(&doc, &catalog);
    assert_eq!(plan.grants.len(), 1);
    assert_eq!(plan.grants[0].quantity, 1);
}

/// A pot-capped consumable asks for its own `maxNum` and carries its group forward, which is what
/// makes the storage-box ladder reachable: without a request above one, a pot conflict could only
/// ever surface as `1 requested, 0 delivered`.
#[test]
fn a_pot_capped_consumable_asks_for_more_than_one_and_names_its_group() {
    use er_build_import_core::catalog::{Entry, Kind, MapCatalog};

    let catalog = MapCatalog::new().with(
        Kind::Tool,
        "Fire Pot",
        Entry {
            // Goods row 600, `potGroupId` 1, `maxNum` 10 in the installed regulation.
            full_item_id: 0x40000258,
            max_stored: Some(10),
            somber: false,
            pot_group: Some(1),
        },
    );
    let doc =
        model::parse(r#"{"items":{"tools":{"slots":[{"name":"Fire Pot"}]}}}"#).expect("parses");
    let plan = plan(&doc, &catalog);
    assert_eq!(plan.grants[0].quantity, 10);
    assert_eq!(plan.grants[0].pot_group, Some(1));
    assert_eq!(
        u32::from_le_bytes(plan.grants[0].to_record()[4..8].try_into().unwrap()),
        10,
        "the quantity reaches the ItemGib record"
    );
}

// ---------------------------------------------------- positions left empty

use er_build_import_core::equip::{
    CHR_ASM_SLOT_ACCESSORY_1, CHR_ASM_SLOT_GREAT_RUNE, CHR_ASM_SLOT_QUICK_BASE, POUCH_SLOTS,
    armament_slot,
};

#[test]
fn a_position_is_either_filled_or_vacated_never_both_and_never_neither() {
    // The property the pair has to have: `positions` and `vacancies` partition every position the
    // plan models. A position in neither is the gap this feature closed -- it is what let the
    // previous build's talisman stay on a character importing a build that wears none.
    let plan = equipped();
    let filled: Vec<(PositionKind, i32)> = plan
        .positions()
        .into_iter()
        .filter(|position| position.kind != PositionKind::Physick)
        .map(|position| {
            (
                position.kind,
                position.slot.expect("every non-physick has a slot"),
            )
        })
        .collect();
    let empty: Vec<(PositionKind, i32)> = plan
        .vacancies()
        .into_iter()
        .map(|vacancy| (vacancy.kind, vacancy.slot))
        .collect();

    for position in &filled {
        assert!(
            !empty.contains(position),
            "{position:?} is both filled and vacated"
        );
    }
    let expected = plan.armaments.len()
        + plan.ammo.len()
        + 4
        + plan.talismans.len()
        + QUICKBAR_SLOTS
        + POUCH_SLOTS
        + 1;
    assert_eq!(
        filled.len() + empty.len(),
        expected,
        "every position the plan models has to be in exactly one of the two lists"
    );
}

#[test]
fn the_armament_slot_this_build_leaves_bare_is_vacated() {
    // The fixture fills five of six armament positions; planner index 5 is empty, and its
    // `ChrAsmSlot` is what the vacate pass has to clear.
    let vacancies = equipped().vacancies();
    let bare = armament_slot(5).expect("planner index 5 is a real position");
    assert!(
        vacancies
            .iter()
            .any(|vacancy| vacancy.kind == PositionKind::Armament && vacancy.slot == bare),
        "the sixth armament position is empty in this build and is not being cleared: {vacancies:?}"
    );
}

#[test]
fn a_build_that_equips_nothing_vacates_every_position_it_models() {
    let doc = model::parse("{}").expect("an empty document parses");
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert!(plan.positions().is_empty());
    let vacancies = plan.vacancies();
    // The great rune is one position and is not in any of the counted lists.
    assert!(
        vacancies
            .iter()
            .any(|vacancy| vacancy.slot == CHR_ASM_SLOT_GREAT_RUNE
                && vacancy.kind == PositionKind::GreatRune)
    );
    // The talisman slots are contiguous from Accessory1, and the quickbar from its own base.
    for offset in 0..plan.talismans.len() {
        let slot = CHR_ASM_SLOT_ACCESSORY_1 + i32::try_from(offset).expect("small");
        assert!(vacancies.iter().any(|vacancy| vacancy.slot == slot));
    }
    for offset in 0..QUICKBAR_SLOTS + POUCH_SLOTS {
        let slot = CHR_ASM_SLOT_QUICK_BASE + i32::try_from(offset).expect("small");
        assert!(vacancies.iter().any(|vacancy| vacancy.slot == slot));
    }
}

#[test]
fn the_physick_is_never_vacated() {
    // Deliberate asymmetry, and the one place `vacancies` is not the complement of `positions`.
    // A build that names no tear is far more often one authored before the planner modelled the
    // flask than a deliberate instruction to empty it.
    let doc = model::parse("{}").expect("an empty document parses");
    let plan = equip_plan(&doc, &fixture_catalog::catalog(), Capacity::default());
    assert_eq!(plan.physick.len(), PHYSICK_SLOTS);
    assert!(
        plan.vacancies()
            .iter()
            .all(|vacancy| vacancy.kind != PositionKind::Physick)
    );
}
