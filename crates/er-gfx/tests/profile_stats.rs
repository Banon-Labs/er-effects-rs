//! Proof gates for the 05_010_ProfileSelect stats-panel transform.
//!
//! Same policy as `title_strip.rs`: no game-derived bytes are versioned;
//! ground truth is the recorded fingerprint of the generated asset
//! (`EDITED_LEN` + `EDITED_FNV1A64`, what the in-game runtime-serve telemetry
//! validates). Derivation tests read the real vanilla movie from the
//! extraction corpus and SKIP when it is absent; the failure-path garbage test
//! always runs. Regenerate the asset with
//! `cargo run -p er-gfx --example make_05_010_stats` for byte-level debugging.

mod common;

use er_gfx::title_05_000::fnv1a64;
use er_gfx::title_05_010::{
    EDITED_FNV1A64, EDITED_LEN, STATS_FIELD_NAME, StatsPanelError, VANILLA_FNV1A64, VANILLA_LEN,
    is_known_vanilla, stats_panel,
};
use er_gfx::{Movie, Tag};

fn read_vanilla_or_skip() -> Option<Vec<u8>> {
    let path = common::corpus_root().join("05_010_profileselect.gfx");
    if !path.exists() {
        eprintln!(
            "SKIP: vanilla movie {} not present; derivation test skipped",
            path.display()
        );
        return None;
    }
    let vanilla = std::fs::read(&path).expect("read vanilla movie");
    assert_eq!(vanilla.len(), VANILLA_LEN, "vanilla corpus file drifted");
    assert_eq!(
        fnv1a64(&vanilla),
        VANILLA_FNV1A64,
        "vanilla corpus file drifted"
    );
    assert!(is_known_vanilla(&vanilla));
    Some(vanilla)
}

#[test]
fn stats_panel_of_vanilla_matches_generated_fingerprint() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly to the known vanilla movie");
    assert_eq!(out.len(), EDITED_LEN);
    assert_eq!(fnv1a64(&out), EDITED_FNV1A64);
}

/// Structural gates on the edited movie: the face box placement is gone, and
/// the row template places a `DefineEditText` char as [`STATS_FIELD_NAME`]
/// (the exact child the DLL resolves for its native SetText push).
#[test]
fn stats_panel_output_places_stats_field_and_drops_face_box() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    let movie = Movie::parse(&out).expect("edited movie parses");
    let row = movie
        .tags
        .iter()
        .find_map(|t| match t {
            Tag::DefineSprite { id: 76, tags, .. } => Some(tags),
            _ => None,
        })
        .expect("edited movie keeps row template sprite 76");
    let names: Vec<&str> = row
        .iter()
        .filter_map(|t| match t {
            Tag::PlaceObject2 { name: Some(n), .. } => Some(n.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !names.contains(&"Icon_0"),
        "face box placement must be removed: {names:?}"
    );
    assert!(
        names.contains(&STATS_FIELD_NAME),
        "stats field placement missing: {names:?}"
    );
    let stats_char = row
        .iter()
        .find_map(|t| match t {
            Tag::PlaceObject2 {
                name: Some(n),
                character_id,
                ..
            } if n == STATS_FIELD_NAME => *character_id,
            _ => None,
        })
        .expect("stats placement carries a character id");
    let is_edit_text = movie.tags.iter().any(|t| {
        matches!(t, Tag::DefineEditText { character_id, font_class: Some(fc), .. }
            if *character_id == stats_char && fc == "MenuFont_01")
    });
    assert!(
        is_edit_text,
        "char {stats_char} must be a MenuFont_01 DefineEditText"
    );
    // Native fields the engine populates must all survive the transform.
    for native in [
        "PlayerName",
        "Level",
        "StaticText_110502",
        "Location",
        "PlayTime",
    ] {
        assert!(
            names.contains(&native),
            "lost native field {native}: {names:?}"
        );
    }
}

/// The edit set must NOT apply to a movie it wasn't derived for: applying it
/// twice has to fail all-or-nothing.
#[test]
fn stats_panel_of_already_edited_movie_fails_closed() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = stats_panel(&vanilla).expect("edits must apply cleanly");
    match stats_panel(&out) {
        Err(StatsPanelError::Edit(_)) => {}
        other => panic!("expected Edit error on already-edited input, got {other:?}"),
    }
}

#[test]
fn stats_panel_of_garbage_fails_closed() {
    assert!(matches!(
        stats_panel(b"not a gfx movie"),
        Err(StatsPanelError::Parse(_))
    ));
    assert!(matches!(stats_panel(&[]), Err(StatsPanelError::Parse(_))));
}
