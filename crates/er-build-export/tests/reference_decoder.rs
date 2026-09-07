//! The acceptance gate: the **site's own decoder** must recover our document.
//!
//! Everything else in this crate is a claim about the format. This is the only test that
//! checks the claim against the code the planner actually runs -- `Bc`, the exact inverse of
//! the `zc` that `serialise()` calls -- by handing it a payload this crate produced and
//! asserting `JSON.parse(atob(Bc(payload)))` deep-equals the document we started from. That
//! expression is not an approximation of the planner's import path; it is copied out of
//! `loadFromSerialised`.
//!
//! It needs two things this repository does not carry: a `node` on path, and an extracted
//! copy of the planner's LZ-UTF8 bundle. Neither is versioned here -- the bundle is
//! third-party minified JavaScript pulled from a live site, and the point of a reference
//! decoder is that it is *theirs*, so vendoring a copy that then drifts would defeat it. So
//! this test skips, loudly, when either is missing, exactly like the game-asset corpus tests
//! in `er-gfx`. Point `ER_LZUTF8_REFERENCE_JS` at the bundle to run it.

mod common;

use common::representative_build;
use er_build_export::{SHARE_URL_PREFIX, ascii_json, model::BuildExportDoc, share_url};
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Override naming the extracted bundle. It must export `{ zc, Bc, LZ }` as CommonJS.
const REFERENCE_JS_ENV: &str = "ER_LZUTF8_REFERENCE_JS";

/// Where the extraction currently lives, when the environment says nothing. Session-local by
/// nature -- set [`REFERENCE_JS_ENV`] rather than editing this.
const DEFAULT_REFERENCE_JS: &str = "/tmp/claude-1000/-home-banon-projects-er-mods-rs/8d5dd91e-259b-4e1d-a494-54b1e1472bfe/scratchpad/planner/lzutf8_extract.js";

/// Seconds the script gives itself before bailing out.
///
/// Belt to the `process.exit(0)` braces below. `cargo test` has no timeout of its own, so a
/// child that never exits hangs the whole suite -- which is exactly what happened here before
/// the exit was added, and what would happen again if a future bundle blocked on something
/// else. Comfortably inside this repo's 30s cap for non-game operations.
const SCRIPT_DEADLINE_SECONDS: u64 = 20;

/// Decode a `?i=` payload the way `loadFromSerialised` does, and compare against what we
/// meant to send.
///
/// Written as a script rather than a file so the gate has no fixture to keep in sync: it is
/// handed to `node -e`, and reads its two inputs as one JSON object on stdin.
///
/// **The explicit `process.exit(0)` is load-bearing, not tidiness.** Loading the bundle runs
/// its `initializeScheduler`, which -- because Node defines `MessageChannel` globally but not
/// `window` -- takes the `new MessageChannel()` branch and holds `port1.onmessage`. That is a
/// live libuv handle, so the event loop never drains and the process runs forever after the
/// script's last line. Measured: without the exit, every one of these tests sat at "has been
/// running for over 60 seconds" until `cargo test` was killed.
const DECODE_SCRIPT: &str = r#"
const assert = require('assert');

// Armed before the bundle is loaded, so a hang inside `require` is caught too. Not unref'd:
// its whole job is to fire on a loop that will not drain.
const deadline = Number(process.env.ER_LZUTF8_DEADLINE_SECONDS) * 1000;
setTimeout(() => {
  console.error('reference decoder did not finish within ' + deadline + 'ms');
  process.exit(3);
}, deadline);

const site = require(process.env.ER_LZUTF8_REFERENCE_JS);
const input = JSON.parse(require('fs').readFileSync(0, 'utf8'));

// Exactly `loadFromSerialised`: JSON.parse(atob(Bc(serialised))).
const recoveredJson = Buffer.from(site.Bc(input.payload), 'base64').toString('binary');
const recovered = JSON.parse(recoveredJson);
const expected = JSON.parse(input.expected);

assert.deepStrictEqual(recovered, expected);

console.log('json_bytes ' + recoveredJson.length);
console.log('text_identical ' + (recoveredJson === input.expected));
console.log('name ' + JSON.stringify(recovered.name));
console.log('OK');

// See the doc comment: the bundle's scheduler pins the event loop open.
process.exit(0);
"#;

/// Name of the env var carrying [`SCRIPT_DEADLINE_SECONDS`] into the script.
const DEADLINE_ENV: &str = "ER_LZUTF8_DEADLINE_SECONDS";

/// Locate the extracted bundle, or explain why the caller is being skipped.
fn reference_bundle() -> Result<PathBuf, String> {
    let candidates: Vec<PathBuf> = match std::env::var(REFERENCE_JS_ENV) {
        Ok(path) if !path.trim().is_empty() => vec![PathBuf::from(path.trim())],
        _ => vec![
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/reference/lzutf8_extract.js"),
            PathBuf::from(DEFAULT_REFERENCE_JS),
        ],
    };

    candidates
        .iter()
        .find(|path| path.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "planner LZ-UTF8 bundle not found (looked in {}); set {REFERENCE_JS_ENV}",
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// Whether `node` can actually `require` the shim, i.e. whether `lzutf8` is installed.
///
/// A separate, tiny `node -e` rather than a flag on the main run: the gate's assertion has to stay
/// unconditional once it decides to run, or a missing library and a decoder disagreement would
/// come back through the same channel and the second one would be reported as the first.
fn bundle_loads(bundle: &std::path::Path) -> Result<(), String> {
    let output = Command::new("node")
        .arg("-e")
        .arg("require(process.env[process.argv[1]]); process.exit(0)")
        .arg(REFERENCE_JS_ENV)
        .env(REFERENCE_JS_ENV, bundle)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    // Strict, like the main run's reads: node writes UTF-8, and a stderr that is not UTF-8 is a
    // finding rather than something to smooth over -- so it falls back to a fixed sentence
    // instead of being displayed with replacement characters.
    let reason = String::from_utf8(output.stderr).unwrap_or_default();
    Err(reason
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("node could not load the bundle")
        .to_owned())
}

/// Whether a usable `node` is on path.
fn node_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Run the gate for one document, or return `None` when the environment cannot host it.
fn decode_through_the_site(doc: &BuildExportDoc) -> Option<String> {
    if !node_available() {
        eprintln!("SKIP: node is not on PATH; reference-decoder gate skipped");
        return None;
    }
    let bundle = match reference_bundle() {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: {reason}");
            return None;
        }
    };
    // Finding the SHIM is not the same as being able to load it. `tests/reference/lzutf8_extract.js`
    // is versioned here, so the existence check above always passes -- but the shim `require`s
    // `lzutf8` from npm, which is not, and a checkout that never ran the install got four hard
    // failures from a gate whose whole design is to skip loudly when its environment is absent.
    // The library is third-party minified JavaScript pulled from a live site and deliberately not
    // vendored (see the module header), so "absent" is the ordinary state on a fresh checkout and
    // on CI, and it has to read as a skip.
    if let Err(reason) = bundle_loads(&bundle) {
        eprintln!(
            "SKIP: {} is present but cannot be loaded -- {reason}\n      \
             npm --prefix crates/er-build-export/tests/reference install lzutf8@0.6.3",
            bundle.display()
        );
        return None;
    }

    let url = share_url(doc);
    let payload = url
        .strip_prefix(SHARE_URL_PREFIX)
        .expect("share_url always writes the prefix");
    let stdin_json = serde_json::json!({
        "payload": payload,
        "expected": ascii_json(doc),
    })
    .to_string();

    let mut child = Command::new("node")
        .arg("-e")
        .arg(DECODE_SCRIPT)
        .env(REFERENCE_JS_ENV, &bundle)
        .env(DEADLINE_ENV, SCRIPT_DEADLINE_SECONDS.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("node is on PATH, so spawning it works");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(stdin_json.as_bytes())
        .expect("node reads its whole input");
    let output = child.wait_with_output().expect("node terminates");

    // Deliberately strict rather than lossy: if the site's decoder emitted something that is
    // not UTF-8, that is the finding, not a detail to paper over.
    let stdout = String::from_utf8(output.stdout).expect("node writes utf-8");
    let stderr = String::from_utf8(output.stderr).expect("node writes utf-8");
    assert!(
        output.status.success(),
        "the site's decoder did not recover the document.\nurl: {url}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("OK"),
        "gate did not reach its end:\n{stdout}"
    );

    println!("reference decoder ({}):", bundle.display());
    for line in stdout.lines() {
        println!("  {line}");
    }
    println!("  url_length {}", url.len());
    Some(url)
}

#[test]
fn the_site_decoder_recovers_a_representative_build() {
    let Some(url) = decode_through_the_site(&representative_build()) else {
        return;
    };
    assert!(url.starts_with(SHARE_URL_PREFIX));
}

#[test]
fn the_site_decoder_recovers_a_default_document() {
    decode_through_the_site(&BuildExportDoc::default());
}

#[test]
fn the_site_decoder_recovers_a_unicode_name() {
    // The case the site itself cannot produce: `btoa` throws on these code units, so this
    // link can only have been written by something that escapes them first.
    let mut doc = representative_build();
    doc.name = "Dongerino \u{e9}\u{30c6}\u{30b9}\u{30c8} \u{1f525}".to_string();
    doc.description = "\u{5024}\u{6bb5}".to_string();
    decode_through_the_site(&doc);
}

#[test]
fn the_site_decoder_recovers_a_large_build() {
    // Long enough to force far matches and a three-byte header form.
    let mut doc = representative_build();
    doc.description = "The Erdtree governs all. ".repeat(2_000);
    doc.tags = (0..200).map(|index| format!("tag-{index}")).collect();
    decode_through_the_site(&doc);
}
