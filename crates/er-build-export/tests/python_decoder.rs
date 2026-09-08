//! A second external check, against `scripts/decode-build-link.py`.
//!
//! `reference_decoder.rs` remains the authoritative gate -- it runs the code the planner
//! itself runs, and it is the only test that can assert the recovered JSON is *byte*
//! identical. But it depends on a 3.6 MB bundle extracted into a session scratchpad, so on a
//! machine without it the crate is left with nothing but its own transcription of the format
//! checking its own encoder.
//!
//! This closes that gap. `scripts/decode-build-link.py` is a pure-Python LZ-UTF8 and `?i=`
//! decoder that lives in the repository, was written independently of this crate, and was
//! validated against a payload produced by the planner's own `zc()`. Agreeing with it is a
//! real claim: a bug here would have to be the same bug in two separately written decoders.
//!
//! What it cannot claim is text identity -- the script pretty-prints with `indent=2` and
//! `ensure_ascii=False`, so its output is a re-serialisation, not the bytes we sent. So this
//! compares the *parsed* documents, which is the property the planner actually depends on
//! (`JSON.parse` reaching the same object). Escaping-level regressions are
//! `reference_decoder.rs`'s to catch.
//!
//! Skips, loudly, when `python3` or the script is missing.

mod common;

use common::representative_build;
use er_build_export::{BuildExportDoc, ascii_json, share_url};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Override naming the decoder script.
const DECODER_ENV: &str = "ER_BUILD_LINK_DECODER";

/// Where the script lives relative to this crate, when the environment says nothing.
const DECODER_RELATIVE_PATH: &str = "scripts/decode-build-link.py";

/// Locate the decoder script, or explain why the caller is being skipped.
fn decoder_script() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var(DECODER_ENV)
        && !path.trim().is_empty()
    {
        let path = PathBuf::from(path.trim());
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "{DECODER_ENV} names {}, which is not a file",
                path.display()
            ))
        };
    }

    // The crate sits at `<repo>/crates/er-build-export`, so the repository root is two up.
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| format!("cannot resolve the repository root: {error}"))?;
    let script = root.join(DECODER_RELATIVE_PATH);
    if script.is_file() {
        Ok(script)
    } else {
        Err(format!("{} not found; set {DECODER_ENV}", script.display()))
    }
}

/// Whether a usable `python3` is on path.
fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Decode `doc`'s share link with the Python decoder, or return `None` when it cannot run.
fn decode_with_python(doc: &BuildExportDoc) -> Option<serde_json::Value> {
    if !python_available() {
        eprintln!("SKIP: python3 is not on PATH; repository-decoder gate skipped");
        return None;
    }
    let script = match decoder_script() {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: {reason}");
            return None;
        }
    };

    let url = share_url(doc);
    let output = Command::new("python3")
        .arg(&script)
        .arg(&url)
        .stdin(Stdio::null())
        .output()
        .expect("python3 is on PATH, so spawning it works");

    // Strict rather than lossy: a decoder emitting non-UTF-8 is the finding, not a detail.
    let stdout = String::from_utf8(output.stdout).expect("the decoder writes utf-8");
    let stderr = String::from_utf8(output.stderr).expect("the decoder writes utf-8");
    assert!(
        output.status.success(),
        "{} could not decode our link.\nurl: {url}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        script.display()
    );

    let recovered: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("decoder output is not json ({error}):\n{stdout}"));
    println!("{} recovered {} bytes", script.display(), stdout.len());
    Some(recovered)
}

/// Assert the decoder recovers exactly the document we serialised.
fn assert_round_trip(doc: &BuildExportDoc) {
    let Some(recovered) = decode_with_python(doc) else {
        return;
    };
    let expected: serde_json::Value =
        serde_json::from_str(&ascii_json(doc)).expect("our own json parses");
    assert_eq!(recovered, expected);
}

#[test]
fn the_repository_decoder_recovers_a_representative_build() {
    assert_round_trip(&representative_build());
}

#[test]
fn the_repository_decoder_recovers_a_default_document() {
    assert_round_trip(&BuildExportDoc::default());
}

#[test]
fn the_repository_decoder_recovers_a_unicode_name() {
    // Proves the `\uXXXX` escaping survives a decoder that is not JavaScript's `JSON.parse`.
    let mut doc = representative_build();
    doc.name = "Dongerino \u{e9}\u{30c6}\u{30b9}\u{30c8} \u{1f525}".to_string();
    doc.description = "\u{5024}\u{6bb5}".to_string();
    assert_round_trip(&doc);
}

#[test]
fn the_repository_decoder_recovers_a_large_build() {
    // Long enough to force far matches and the three-byte header form.
    let mut doc = representative_build();
    doc.description = "The Erdtree governs all. ".repeat(2_000);
    doc.tags = (0..200).map(|index| format!("tag-{index}")).collect();
    assert_round_trip(&doc);
}
