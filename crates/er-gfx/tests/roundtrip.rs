//! Byte-identity round-trip over the real Elden Ring menu `.gfx` corpus.
//!
//! For every `.gfx` under the corpus root (recursively, including any `win/`
//! subdir): read -> [`Movie::parse`] -> [`Movie::write`] -> assert the output is
//! byte-for-byte identical to the input. The gate is byte-identity; the first
//! differing offset is reported on mismatch.
//!
//! If the corpus root does not exist (CI without assets), the test SKIPS with an
//! `eprintln!` rather than failing.

mod common;

use er_gfx::Movie;
use std::fs;
use std::path::{Path, PathBuf};

const MIN_EXPECTED_FILES: usize = 100;

/// Recursively collect every `*.gfx` path under `dir`.
fn collect_gfx(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_gfx(&path, out);
        } else if path.extension().and_then(|s| s.to_str()) == Some("gfx") {
            out.push(path);
        }
    }
}

/// Index of the first differing byte, or `None` if equal.
fn first_diff(a: &[u8], b: &[u8]) -> Option<usize> {
    if a.len() != b.len() {
        let n = a.len().min(b.len());
        for i in 0..n {
            if a[i] != b[i] {
                return Some(i);
            }
        }
        return Some(n);
    }
    a.iter().zip(b.iter()).position(|(x, y)| x != y)
}

#[test]
fn corpus_round_trips_byte_identical() {
    let root = common::corpus_root();
    if !root.exists() {
        eprintln!(
            "SKIP: corpus root {} not present; round-trip test skipped (no assets)",
            root.display()
        );
        return;
    }

    let mut files = Vec::new();
    collect_gfx(&root, &mut files);
    files.sort();

    assert!(
        files.len() >= MIN_EXPECTED_FILES,
        "corpus shrank: found {} .gfx files under {}, expected at least {MIN_EXPECTED_FILES}",
        files.len(),
        root.display()
    );

    let mut failures: Vec<String> = Vec::new();
    let mut ok = 0usize;

    for path in &files {
        let input = match fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{}: read error: {e}", path.display()));
                continue;
            }
        };
        let movie = match Movie::parse(&input) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{}: parse error: {e}", path.display()));
                continue;
            }
        };
        let output = match movie.write() {
            Ok(o) => o,
            Err(e) => {
                failures.push(format!("{}: write error: {e}", path.display()));
                continue;
            }
        };
        match first_diff(&input, &output) {
            None => ok += 1,
            Some(off) => {
                let a = input.get(off).copied();
                let b = output.get(off).copied();
                failures.push(format!(
                    "{}: byte mismatch at offset {off} (in_len={}, out_len={}, in={a:02x?}, out={b:02x?})",
                    path.display(),
                    input.len(),
                    output.len(),
                ));
            }
        }
    }

    eprintln!(
        "er-gfx round-trip: {ok}/{} files byte-identical ({} failure(s))",
        files.len(),
        failures.len()
    );

    assert!(
        failures.is_empty(),
        "round-trip failures ({}):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
