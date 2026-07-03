//! Proof gates for the 05_000_title structured strip (er-effects-rs-h7x).
//!
//! No game-derived bytes are versioned in the repo (same policy as the
//! round-trip corpus): ground truth is the recorded fingerprint of the
//! validated v2 asset (`STRIPPED_LEN` + `STRIPPED_FNV1A64`, the exact bytes
//! that were runtime-validated and formerly embedded in the DLL as
//! `TITLE_05_000_TEXT_SUPPRESSED_GFX`; the same fingerprint is what the
//! in-game `oracle_title_05_000_runtime_strip_output_validated` telemetry
//! checks). The derivation tests read the real vanilla movie from the
//! extraction corpus (see [`common::corpus_root`]) and SKIP, like
//! `roundtrip.rs`, when it is absent; the failure-path garbage test always
//! runs. For byte-level debugging of a fingerprint mismatch, regenerate the
//! expected asset with `scripts/gfx_tag_diff.py --emit-rust` and compare
//! offline.

mod common;

use er_gfx::title_05_000::{
    STRIPPED_FNV1A64, STRIPPED_LEN, StripError, VANILLA_FNV1A64, VANILLA_LEN, fnv1a64,
    is_known_vanilla, strip,
};

fn read_vanilla_or_skip() -> Option<Vec<u8>> {
    let path = common::corpus_root().join("05_000_title.gfx");
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
fn strip_of_vanilla_matches_validated_fingerprint() {
    let Some(vanilla) = read_vanilla_or_skip() else {
        return;
    };
    let out = strip(&vanilla).expect("strip must apply cleanly to the known vanilla movie");
    // strip() itself enforces the fingerprint for known-vanilla input
    // (StripError::KnownInputBadOutput); assert it here too so this gate does
    // not silently depend on that internal check.
    assert_eq!(out.len(), STRIPPED_LEN);
    assert_eq!(fnv1a64(&out), STRIPPED_FNV1A64);
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
