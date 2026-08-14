//! Centre the notice text in the REAL announcement movie.
//!
//! The unit tests build a synthetic field, so they prove the edit's shape but not that the shipped
//! movie has that shape. This reads the vanilla `01_080_emergencynotice.gfx` out of the local
//! extraction corpus, applies the edit and re-parses the result.
//!
//! The movie's bytes are never committed -- only its length and FNV-1a-64 -- and the test SKIPs
//! when the corpus is absent, like the rest of the er-gfx corpus tests.

mod common;

use er_game_base::fnv1a::fnv1a64;
use er_gfx::announce_notice::{
    ALIGN_CENTER, ALIGN_LEFT, EDIT_TEXT_AUTO_SIZE, EDIT_TEXT_HAS_LAYOUT, NOTICE_FIELD_WIDTH_PX,
    NOTICE_TEXT_CHARACTER_ID, with_centered_notice_text,
};
use er_gfx::{Movie, Tag};

/// Vanilla `01_080_emergencynotice.gfx` fingerprint. The DLL gates its swap on exactly this pair,
/// so a drift here and a drift there are the same fact and must be updated together.
const NOTICE_LEN: usize = 3_205;
const NOTICE_FNV1A64: u64 = 0x6973_a088_b693_13f8;

fn vanilla_or_skip() -> Option<Vec<u8>> {
    let path = common::corpus_root().join("01_080_emergencynotice.gfx");
    if !path.exists() {
        eprintln!("SKIP: {} not present", path.display());
        return None;
    }
    let bytes = std::fs::read(&path).expect("read vanilla notice movie");
    assert_eq!(bytes.len(), NOTICE_LEN, "vanilla corpus file drifted");
    assert_eq!(
        fnv1a64(&bytes),
        NOTICE_FNV1A64,
        "vanilla corpus file drifted"
    );
    Some(bytes)
}

fn notice_field(movie: &Movie) -> &Tag {
    movie
        .tags
        .iter()
        .find(
            |tag| matches!(tag, Tag::DefineEditText { character_id, .. } if *character_id == NOTICE_TEXT_CHARACTER_ID),
        )
        .expect("the notice text field is present")
}

#[test]
fn the_vanilla_field_has_the_shape_the_edit_assumes() {
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let movie = Movie::parse(&vanilla).expect("vanilla notice movie parses");
    let Tag::DefineEditText {
        flags2,
        layout: Some(layout),
        bounds,
        ..
    } = notice_field(&movie)
    else {
        panic!("the notice field must carry a layout block");
    };
    assert_eq!(layout.align, ALIGN_LEFT, "vanilla is left-aligned");
    assert_ne!(flags2 & EDIT_TEXT_HAS_LAYOUT, 0, "HasLayout must be set");
    assert_eq!(
        flags2 & EDIT_TEXT_AUTO_SIZE,
        0,
        "AutoSize must be CLEAR -- an auto-sized box has no spare width, and centring inside it \
         would change nothing on screen while every assertion here still passed"
    );
    // The box has to be substantially wider than a line of text, or "centred" and "left" would
    // look the same and this edit would be pointless.
    let width_px = (bounds.x_max - bounds.x_min) / 20;
    assert!(
        width_px > 800,
        "the field is only {width_px}px wide; centring needs spare width to be visible"
    );
    // And it must match the constant the DLL's blank-banner oracle subtracts. The engine truncates
    // each EDGE to int before subtracting, so this is reproduced edge-wise rather than as a plain
    // width -- `(int)(x_max*0.05) - (int)(x_min*0.05)`, which is not the same as `(x_max-x_min)/20`
    // when either edge is negative, and this field's left edge is.
    let engine_width =
        ((f64::from(bounds.x_max) * 0.05) as i32) - ((f64::from(bounds.x_min) * 0.05) as i32);
    assert_eq!(
        engine_width, NOTICE_FIELD_WIDTH_PX,
        "the oracle's baseline must equal what the engine measures for this field"
    );
}

#[test]
fn the_edit_centres_the_real_movie_and_it_re_parses() {
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let edited = with_centered_notice_text(&vanilla).expect("centres");
    // Re-parsing is the real assertion: a malformed tag stream is handed to Scaleform at menu-load
    // time, which is a crash rather than a misaligned line.
    let movie = Movie::parse(&edited).expect("edited notice movie re-parses");
    let Tag::DefineEditText {
        layout: Some(layout),
        ..
    } = notice_field(&movie)
    else {
        panic!("edit text");
    };
    assert_eq!(layout.align, ALIGN_CENTER);
}

#[test]
fn the_edit_changes_exactly_one_byte_worth_of_meaning() {
    // The alignment is a 2-bit field inside the layout block, so a correct edit cannot change the
    // movie's LENGTH. A length change means something else moved -- a re-encoded string, a
    // re-packed tag -- and on a menu movie that is how a crash gets introduced by accident.
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let edited = with_centered_notice_text(&vanilla).expect("centres");
    assert_eq!(
        edited.len(),
        vanilla.len(),
        "an alignment change must not resize the movie"
    );
    let differing: Vec<usize> = vanilla
        .iter()
        .zip(edited.iter())
        .enumerate()
        .filter_map(|(index, (a, b))| (a != b).then_some(index))
        .collect();
    assert_eq!(
        differing.len(),
        1,
        "exactly one byte should differ (the align field), got {differing:?}"
    );
}

#[test]
fn every_other_tag_survives_untouched() {
    let Some(vanilla) = vanilla_or_skip() else {
        return;
    };
    let before = Movie::parse(&vanilla).expect("parse");
    let edited = with_centered_notice_text(&vanilla).expect("centres");
    let after = Movie::parse(&edited).expect("parse");
    assert_eq!(before.header, after.header, "the header must not move");
    assert_eq!(before.tags.len(), after.tags.len());
    for (index, (a, b)) in before.tags.iter().zip(after.tags.iter()).enumerate() {
        let ours = matches!(
            a,
            Tag::DefineEditText { character_id, .. } if *character_id == NOTICE_TEXT_CHARACTER_ID
        );
        if !ours {
            assert_eq!(a, b, "tag {index} changed and it is not ours");
        }
    }
}
