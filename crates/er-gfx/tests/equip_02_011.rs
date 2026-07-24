//! Verifies the Ash-of-War badge edit for `02_011_equip.gfx` applies cleanly and
//! round-trips. Reads the vanilla movie from the local extraction corpus
//! (`ER_GFX_CORPUS_ROOT`, e.g. `<ELDEN RING>/Game/menu`) and SKIPs when absent --
//! game-derived `.gfx` bytes are never versioned.

mod common;

use er_gfx::equip_02_011::{
    BADGE_CLIP_ID, BADGE_INSTANCE_NAME, VANILLA_FNV1A64, VANILLA_LEN, arts_badge, is_known_vanilla,
};
use er_gfx::title_05_000::fnv1a64;
use er_gfx::{Movie, Tag};

#[test]
fn arts_badge_edit_applies_and_roundtrips() {
    let Some(vanilla) = common::read_vanilla_or_skip(
        "02_011_equip.gfx",
        VANILLA_LEN,
        VANILLA_FNV1A64,
        fnv1a64,
        is_known_vanilla,
    ) else {
        return;
    };

    let out = arts_badge(&vanilla).expect("arts_badge edit applies to known vanilla");
    assert_ne!(out, vanilla, "edited movie must differ from vanilla");

    // The edited movie must re-parse and re-serialize byte-for-byte (codec identity).
    let movie = Movie::parse(&out).expect("edited movie re-parses");
    let rewritten = movie.write().expect("edited movie re-serializes");
    assert_eq!(rewritten, out, "edited movie round-trips");

    // The new single-frame badge clip must exist...
    let has_clip = movie
        .tags
        .iter()
        .any(|t| matches!(t, Tag::DefineSprite { id, .. } if *id == BADGE_CLIP_ID));
    assert!(has_clip, "badge clip {BADGE_CLIP_ID} present");

    // ...and the tile sprite 71 must place it under the ArtsBadge instance name.
    let tile = movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 71, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("tile sprite 71 present");
    let places_badge = tile.iter().any(|t| {
        matches!(
            t,
            Tag::PlaceObject2 { name: Some(n), character_id: Some(c), .. }
                if n == BADGE_INSTANCE_NAME && *c == BADGE_CLIP_ID
        )
    });
    assert!(
        places_badge,
        "tile 71 places {BADGE_INSTANCE_NAME} -> clip {BADGE_CLIP_ID}"
    );

    // Emit fingerprint so it can be baked into EDITED_LEN / EDITED_FNV1A64.
    eprintln!(
        "EQUIP_02_011 EDITED len={} fnv1a64=0x{:016x}",
        out.len(),
        fnv1a64(&out)
    );
}
