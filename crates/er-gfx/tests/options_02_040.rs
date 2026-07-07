//! Proof gates for the runtime-derived 4-button System->Quit OptionSetting movie.
//!
//! No game-derived `.gfx` is versioned in the repo. These tests read the real
//! vanilla Windows `02_040_optionsetting.gfx` from the local extraction corpus and
//! skip when it is absent; the DLL uses the same transform at runtime against the
//! game's own Scaleform MemoryFile.

mod common;

use er_gfx::options_02_040::{
    QUIT4_WIN_FNV1A64, QUIT4_WIN_LEN, Quit4Error, VANILLA_WIN_FNV1A64, VANILLA_WIN_LEN,
    is_known_vanilla_win, quit4,
};
use er_gfx::title_05_000::fnv1a64;

fn read_vanilla_or_skip() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "win/02_040_optionsetting.gfx",
        VANILLA_WIN_LEN,
        VANILLA_WIN_FNV1A64,
        fnv1a64,
        is_known_vanilla_win,
    )
}

#[test]
fn quit4_of_vanilla_matches_validated_fingerprint() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = quit4(&vanilla).expect("quit4 edit must apply cleanly to the known vanilla movie");
    assert_eq!(out.len(), QUIT4_WIN_LEN);
    assert_eq!(fnv1a64(&out), QUIT4_WIN_FNV1A64);
}

#[test]
fn quit4_of_already_edited_movie_fails_closed() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let edited = quit4(&vanilla).expect("quit4 edit must apply cleanly to the known vanilla movie");
    match quit4(&edited) {
        Err(Quit4Error::Edit(_)) => {}
        other => panic!("expected Edit error on already-edited input, got {other:?}"),
    }
}

#[test]
fn quit4_of_garbage_fails_closed() {
    assert!(matches!(
        quit4(b"not a gfx movie"),
        Err(Quit4Error::Parse(_))
    ));
    assert!(matches!(quit4(&[]), Err(Quit4Error::Parse(_))));
}
