//! The gate on the categories that used to fall out of a share link.
//!
//! Every other test in this crate asks whether the ENCODER is faithful: does the payload decode
//! back to the JSON we handed it. That question was already green while the quickbar, the pouch
//! and the ammunition were missing from every link the game produced, because they were missing
//! from the document before the encoder ever saw it -- a faithful encoding of an empty field.
//!
//! So this one asks the other question. It writes a document, encodes it, decodes it with the
//! independent decoder in `tests/common`, parses the result with **the importer's own reader**,
//! and runs **the importer's own equip planner** over it -- then asserts each item comes back at
//! the native slot it started from. That chain has no shared code with the writer at any step, so
//! a field written under a key the reader does not know fails here rather than on a website.

mod common;

use er_build_export::model::{
    Ammo, BuildExportDoc, Items, POUCH_POSITIONS_TOTAL, QUICKBAR_POSITIONS, Slot, SlotList,
};
use er_build_export::share_payload;
use er_build_import_core::catalog::{Kind, MapCatalog, entry};
use er_build_import_core::equip::{
    CHR_ASM_SLOT_AMMO_1, CHR_ASM_SLOT_QUICK_BASE, Capacity, POUCH_SLOTS, PositionKind,
    QUICKBAR_SLOTS, equip_plan,
};
use er_build_import_core::model;

/// A quickbar position near each end of the run, so an off-by-one in either direction shows up.
const QUICKBAR_FIRST: u32 = 0;
const QUICKBAR_LAST: u32 = 7;
/// A pouch position near each end of its run, expressed the way the payload does -- past the
/// quickbar.
const POUCH_FIRST: u32 = QUICKBAR_SLOTS as u32;
const POUCH_LAST: u32 = QUICKBAR_SLOTS as u32 + POUCH_SLOTS as u32 - 1;

/// The four ammunition names, in the order the engine's slots run: `Arrow1, Bolt1, Arrow2, Bolt2`.
/// Deliberately two arrows and two bolts with distinguishable names, because the failure this
/// guards is an interleave that puts a bolt in an arrow slot -- which four identical names, or
/// four arrows, would not reveal.
const AMMO_IN_SLOT_ORDER: [&str; 4] = ["Bone Arrow", "Bolt", "Great Arrow", "Ballista Bolt"];

/// A document carrying something in every category this test is about.
fn document() -> BuildExportDoc {
    let mut ammo = Ammo::default();
    assert!(ammo.set("arrow1", AMMO_IN_SLOT_ORDER[0]));
    assert!(ammo.set("bolt1", AMMO_IN_SLOT_ORDER[1]));
    assert!(ammo.set("arrow2", AMMO_IN_SLOT_ORDER[2]));
    assert!(ammo.set("bolt2", AMMO_IN_SLOT_ORDER[3]));

    let tools = SlotList::new(vec![
        Slot::carried("Flask of Crimson Tears", 0).equipped_without_set(QUICKBAR_FIRST),
        Slot::carried("Boiled Prawn", 1).equipped_without_set(QUICKBAR_LAST),
        Slot::carried("Blessing of Marika", 2).equipped_without_set(POUCH_FIRST),
        Slot::carried("Crafting Kit", 3).equipped_without_set(POUCH_LAST),
        // A carried tool with no position at all, which must reach the far side unequipped
        // rather than landing on position 0.
        Slot::carried("Fingerprint Nostrum", 4),
    ]);

    BuildExportDoc {
        name: "Quickbar Round Trip".to_string(),
        items: Items {
            ammo,
            tools,
            ..Items::default()
        },
        ..BuildExportDoc::with_level(150, false)
    }
}

/// Decode a payload this crate produced with the importer's reader.
fn round_trip(doc: &BuildExportDoc) -> model::BuildDoc {
    let json = common::decode_payload(&share_payload(doc));
    model::parse(&json).expect("the importer parses a document this crate wrote")
}

/// A catalog holding every name the fixture uses, under the kind the planner files it as.
///
/// Ids are arbitrary and distinct -- nothing here asserts on an id, only on which position each
/// name reached, and distinct ids are what makes a swapped pair visible.
fn catalog() -> MapCatalog {
    let mut catalog = MapCatalog::new();
    for (index, name) in AMMO_IN_SLOT_ORDER.iter().enumerate() {
        catalog.insert(Kind::Ammo, name, entry(0x0000_1000 + index as u32));
    }
    for (index, name) in [
        "Flask of Crimson Tears",
        "Boiled Prawn",
        "Blessing of Marika",
        "Crafting Kit",
        "Fingerprint Nostrum",
    ]
    .into_iter()
    .enumerate()
    {
        catalog.insert(Kind::Tool, name, entry(0x4000_2000 + index as u32));
    }
    catalog
}

#[test]
fn the_writers_quickbar_split_matches_the_readers() {
    // Two independent facts that must be equal, so their agreement is a test rather than an
    // assumption: `QUICKBAR_POSITIONS` is the planner's `QUICKBAR` constant, and `QUICKBAR_SLOTS`
    // is the length of `ChrAsmEquipEntries::quickItem1..10` in the game. If a patch ever moved
    // one, the export would write positions the import could not place -- and nothing else in
    // either crate would notice, because each side is self-consistent.
    assert_eq!(QUICKBAR_POSITIONS, QUICKBAR_SLOTS);
    assert_eq!(POUCH_POSITIONS_TOTAL, QUICKBAR_SLOTS + POUCH_SLOTS);
}

#[test]
fn the_documents_own_category_counts_match_what_it_carries() {
    let counts = document().written_categories();
    assert_eq!(counts.quickbar, 2);
    assert_eq!(counts.pouch, 2);
    assert_eq!(counts.tools_unassigned, 1);
    assert_eq!(counts.ammo, 4);
}

#[test]
fn the_quickbar_and_pouch_survive_the_round_trip_as_tool_equip_indices() {
    let doc = round_trip(&document());
    let placed: Vec<(&str, Option<u32>)> = doc
        .items
        .tools
        .slots
        .iter()
        .map(|slot| (slot.name.as_str(), slot.equip_index))
        .collect();
    assert_eq!(
        placed,
        vec![
            ("Flask of Crimson Tears", Some(QUICKBAR_FIRST)),
            ("Boiled Prawn", Some(QUICKBAR_LAST)),
            ("Blessing of Marika", Some(POUCH_FIRST)),
            ("Crafting Kit", Some(POUCH_LAST)),
            ("Fingerprint Nostrum", None),
        ]
    );
}

#[test]
fn a_tool_row_claims_no_equip_set() {
    // `equip_index_in_set` prefers `equipSet` when it is present, so a spurious one would be the
    // field the importer reads. There is no `sets.tools`, so there is nothing for it to mean.
    let json = common::decode_payload(&share_payload(&document()));
    let raw: serde_json::Value = serde_json::from_str(&json).expect("the payload is JSON");
    for slot in raw["items"]["tools"]["slots"]
        .as_array()
        .expect("tools.slots is an array")
    {
        assert!(
            slot.get("equipSet").is_none(),
            "a tool row must not carry equipSet: {slot}"
        );
    }
}

#[test]
fn ammunition_survives_the_round_trip_in_chr_asm_slot_order() {
    let doc = round_trip(&document());
    let positions = doc.items.ammo.positions();
    let names: Vec<Option<&str>> = positions.iter().map(|(_, name)| *name).collect();
    assert_eq!(
        names,
        AMMO_IN_SLOT_ORDER.map(Some).to_vec(),
        "the interleave is the engine's: Arrow1, Bolt1, Arrow2, Bolt2"
    );
    // And the keys the reader used are the ones the writer wrote.
    let keys: Vec<&str> = positions.iter().map(|(key, _)| *key).collect();
    assert_eq!(keys, vec!["arrow1", "bolt1", "arrow2", "bolt2"]);
}

#[test]
fn ammunition_is_written_as_bare_names_under_position_keys() {
    let json = common::decode_payload(&share_payload(&document()));
    let raw: serde_json::Value = serde_json::from_str(&json).expect("the payload is JSON");
    assert_eq!(
        raw["items"]["ammo"],
        serde_json::json!({
            "arrow1": "Bone Arrow",
            "bolt1": "Bolt",
            "arrow2": "Great Arrow",
            "bolt2": "Ballista Bolt",
        }),
        "no order, no equipIndex, no quantity -- the key IS the position"
    );
}

#[test]
fn the_planner_default_document_still_carries_an_empty_ammo_object() {
    // `{}` and not a missing key: `makeDefault` writes it, and a build that equips nothing is not
    // a build whose ammo failed to encode.
    let json = common::decode_payload(&share_payload(&BuildExportDoc::default()));
    let raw: serde_json::Value = serde_json::from_str(&json).expect("the payload is JSON");
    assert_eq!(raw["items"]["ammo"], serde_json::json!({}));
    assert_eq!(
        raw["items"]["tools"]["slots"],
        serde_json::json!([]),
        "an empty tool list is still a list"
    );
}

#[test]
fn the_importer_plans_every_position_back_onto_its_native_slot() {
    // The end-to-end claim: what the game read, through the URL, through the reader, lands on the
    // `ChrAsmSlot` it came from. Anything short of this can be true while the build is wrong.
    let doc = round_trip(&document());
    let plan = equip_plan(&doc, &catalog(), Capacity::default());
    assert!(
        plan.rejected.is_empty(),
        "nothing should be rejected: {:?}",
        plan.rejected
    );
    assert!(
        plan.contested.is_empty(),
        "no two rows claim one position: {:?}",
        plan.contested
    );

    let mut placed: Vec<(PositionKind, Option<i32>, String)> = plan
        .positions()
        .into_iter()
        .filter(|position| {
            matches!(
                position.kind,
                PositionKind::Quickbar | PositionKind::Pouch | PositionKind::Ammo
            )
        })
        .map(|position| (position.kind, position.slot, position.item.name))
        .collect();
    placed.sort_by_key(|(_, slot, _)| *slot);

    assert_eq!(
        placed,
        vec![
            (
                PositionKind::Ammo,
                Some(CHR_ASM_SLOT_AMMO_1),
                "Bone Arrow".to_string()
            ),
            (
                PositionKind::Ammo,
                Some(CHR_ASM_SLOT_AMMO_1 + 1),
                "Bolt".to_string()
            ),
            (
                PositionKind::Ammo,
                Some(CHR_ASM_SLOT_AMMO_1 + 2),
                "Great Arrow".to_string()
            ),
            (
                PositionKind::Ammo,
                Some(CHR_ASM_SLOT_AMMO_1 + 3),
                "Ballista Bolt".to_string()
            ),
            (
                PositionKind::Quickbar,
                Some(CHR_ASM_SLOT_QUICK_BASE + QUICKBAR_FIRST as i32),
                "Flask of Crimson Tears".to_string()
            ),
            (
                PositionKind::Quickbar,
                Some(CHR_ASM_SLOT_QUICK_BASE + QUICKBAR_LAST as i32),
                "Boiled Prawn".to_string()
            ),
            (
                PositionKind::Pouch,
                Some(CHR_ASM_SLOT_QUICK_BASE + POUCH_FIRST as i32),
                "Blessing of Marika".to_string()
            ),
            (
                PositionKind::Pouch,
                Some(CHR_ASM_SLOT_QUICK_BASE + POUCH_LAST as i32),
                "Crafting Kit".to_string()
            ),
        ]
    );
}
