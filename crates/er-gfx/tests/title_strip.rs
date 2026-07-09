//! Proof gates for the 05_000_title structured strip (er-effects-rs-h7x).
//!
//! No game-derived bytes are versioned in the repo (same policy as the
//! round-trip corpus): the stripped-v3 fingerprint is derived by applying the
//! edit table to the real vanilla corpus movie at test time, while runtime
//! validation uses structural invariants (`STRIPPED_LEN` plus black
//! SetBackgroundColor). The derivation tests read the real vanilla movie from
//! the extraction corpus (see [`common::corpus_root`]) and SKIP, like
//! `roundtrip.rs`, when it is absent; the failure-path garbage test always
//! runs. For byte-level debugging of a fingerprint mismatch, regenerate the
//! expected asset with `scripts/gfx_tag_diff.py --emit-rust` and compare
//! offline.

mod common;

use er_gfx::title_05_000::{
    STRIPPED_LEN, StripError, VANILLA_FNV1A64, VANILLA_LEN, fnv1a64, is_known_vanilla, strip,
    stripped_fnv1a64, stripped_output_is_valid,
};
use er_gfx::title_05_001::{
    TitleLogoEffectError, VANILLA_FNV1A64 as TITLE_LOGO_VANILLA_FNV1A64,
    VANILLA_LEN as TITLE_LOGO_VANILLA_LEN, is_known_vanilla as title_logo_is_known_vanilla,
    suppress_title_logo_effect, title_logo_effect_is_suppressed,
};

fn read_vanilla_or_skip() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "05_000_title.gfx",
        VANILLA_LEN,
        VANILLA_FNV1A64,
        fnv1a64,
        is_known_vanilla,
    )
}

#[test]
fn strip_of_vanilla_matches_validated_fingerprint() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = strip(&vanilla).expect("strip must apply cleanly to the known vanilla movie");
    // Derive the expected fingerprint from the vanilla fixture rather than
    // storing a second magic constant beside the edit table.
    let expected_fnv = stripped_fnv1a64(&vanilla).expect("derived fingerprint");
    assert_eq!(out.len(), STRIPPED_LEN);
    assert_eq!(fnv1a64(&out), expected_fnv);
    assert!(stripped_output_is_valid(&out));
}

/// The edit set must NOT apply to a movie it wasn't derived for: stripping an
/// already-stripped movie has to fail all-or-nothing (the removed placements
/// are gone, so the first missing match aborts the whole application).
#[test]
fn strip_of_already_stripped_movie_fails_closed() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let stripped = strip(&vanilla).expect("strip must apply cleanly to the known vanilla movie");
    match strip(&stripped) {
        Err(StripError::Edit(_)) => {}
        other => panic!("expected Edit error on already-stripped input, got {other:?}"),
    }
}

#[test]
fn strip_of_garbage_fails_closed() {
    assert!(matches!(
        strip(b"not a gfx movie"),
        Err(StripError::Parse(_))
    ));
    assert!(matches!(strip(&[]), Err(StripError::Parse(_))));
}

fn read_title_logo_vanilla_or_skip() -> Option<Vec<u8>> {
    common::read_vanilla_or_skip(
        "05_001_title_logo.gfx",
        TITLE_LOGO_VANILLA_LEN,
        TITLE_LOGO_VANILLA_FNV1A64,
        fnv1a64,
        title_logo_is_known_vanilla,
    )
}

#[test]
fn title_logo_effect_suppression_removes_animated_depth3_ramp() {
    let Some(vanilla) = read_title_logo_vanilla_or_skip() else {
        return;
    };
    assert!(!title_logo_effect_is_suppressed(&vanilla));
    let out = suppress_title_logo_effect(&vanilla)
        .expect("title-logo effect suppression must apply cleanly to known vanilla");
    assert!(
        title_logo_effect_is_suppressed(&out),
        "edited title-logo movie must not contain the animated top-level depth-3 effect"
    );
    assert!(out.len() < vanilla.len());
}

#[test]
fn title_logo_effect_suppression_fails_closed_when_already_suppressed() {
    let Some(vanilla) = read_title_logo_vanilla_or_skip() else {
        return;
    };
    let out = suppress_title_logo_effect(&vanilla)
        .expect("title-logo effect suppression must apply cleanly to known vanilla");
    match suppress_title_logo_effect(&out) {
        Err(TitleLogoEffectError::MissingAnimatedEffect) => {}
        other => {
            panic!("expected MissingAnimatedEffect on already-suppressed input, got {other:?}")
        }
    }
}
