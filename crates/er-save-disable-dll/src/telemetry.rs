//! Machine-readable census output -- the semaphore surface.
//!
//! This file, not the text log, is what a harness reads. It is RAM-derived in-process
//! telemetry, never a screenshot, and it is the run-stopping oracle for a
//! save-suppression proof: `escaped_write_sites` must be empty.
//!
//! Written event-driven rather than on a polling timer (this repo treats sleeps as a
//! synchronization smell), on a schedule chosen so the file is always current *where a
//! verdict is drawn from it*:
//!
//! - at install, so an absent file means "the DLL never loaded" rather than "nothing
//!   happened";
//! - on each newly discovered write site, the census's only interesting event;
//! - on the first swallowed save and at exponentially spaced milestones after it;
//! - on every quit-to-title settle, the moment the acceptance test is about.
//!
//! Publishing once per swallowed save was the earlier behaviour and was wrong: saves
//! are requested on every rune-total change, so it re-serialized and rewrote this file
//! on the save worker thread continuously. Nothing was gained, because every counter a
//! harness gates on is a *threshold*, and a threshold is crossed at a first occurrence
//! that is always published.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use er_game_base::log::game_directory_path;

use crate::witness;

const TELEMETRY_FILE_NAME: &str = "er-save-disable-telemetry.json";

fn telemetry_path() -> PathBuf {
    game_directory_path()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join(TELEMETRY_FILE_NAME)
}

/// Serialize the current census and replace the telemetry file atomically.
///
/// The write goes through the same file APIs this DLL hooks. That is safe because every
/// caller reaches here from inside the witness reentrancy guard, so the detours' record
/// paths short-circuit -- census callers arrive already guarded, and the suppression
/// hooks publish through the `er_save_suppress` publish sink, which this DLL wires to
/// take the guard for them (`spawn_census_task`). The telemetry path also fails the
/// save-path filter, giving a second, independent reason it cannot pollute its own
/// census.
pub(crate) fn write_snapshot() {
    let mut body = String::new();
    body.push_str("{\n");
    body.push_str(&format!(
        "  \"phase\": \"{}\",\n",
        json_escape(crate::phase())
    ));
    body.push_str(&format!(
        "  \"census_hooks_installed\": {},\n",
        crate::hooks_installed()
    ));
    #[cfg(windows)]
    let expected = crate::hooks::EXPECTED_HOOKS;
    #[cfg(not(windows))]
    let expected = 0;
    body.push_str(&format!("  \"census_hooks_expected\": {expected},\n"));

    body.push_str(&format!(
        "  \"suppression_armed\": {},\n",
        er_save_suppress::is_armed()
    ));
    body.push_str(&format!(
        "  \"suppression_hooks_installed\": {},\n",
        er_save_suppress::installed_hooks()
    ));
    body.push_str(&format!(
        "  \"suppression_hooks_expected\": {},\n",
        er_save_suppress::SUPPRESSOR_HOOKS
    ));
    // Reported separately from the suppressor count: a missing observer means
    // `quit_phase_settle_events` is structurally 0, which must not be read as a deadlock.
    body.push_str(&format!(
        "  \"quit_settle_observer_installed\": {},\n",
        er_save_suppress::settle_observer_installed()
    ));
    body.push_str(&format!(
        "  \"publish_failures\": {},\n",
        PUBLISH_FAILURES.load(Ordering::Relaxed)
    ));
    for (name, value) in er_save_suppress::counters() {
        body.push_str(&format!("  \"{name}\": {value},\n"));
    }

    let (game_base, text_start, text_size) = witness::attribution_context();
    body.push_str(&format!("  \"game_base\": \"0x{game_base:x}\",\n"));
    body.push_str(&format!("  \"text_start\": \"0x{text_start:x}\",\n"));
    body.push_str(&format!("  \"text_size\": \"0x{text_size:x}\",\n"));

    for (name, value) in witness::counters() {
        body.push_str(&format!("  \"{name}\": {value},\n"));
    }

    let escaped = witness::escaped_write_sites();
    body.push_str(&format!(
        "  \"escaped_write_site_count\": {},\n",
        escaped.len()
    ));
    body.push_str("  \"escaped_write_sites\": [\n");
    for (index, site) in escaped.iter().enumerate() {
        let rvas = hex_list(&site.game_rvas);
        let foreign = hex_list(&site.foreign_frames);
        body.push_str(&format!(
            "    {{\"api\": \"{}\", \"path\": \"{}\", \"hits\": {}, \"bytes\": {}, \"game_rvas\": [{}], \"foreign_frames\": [{}]}}{}\n",
            json_escape(site.api),
            json_escape(&site.path),
            site.hits,
            site.bytes,
            rvas,
            foreign,
            if index + 1 == escaped.len() { "" } else { "," }
        ));
    }
    body.push_str("  ]\n}\n");

    let path = telemetry_path();
    // Unique tmp name per publish. There are publishers on three threads -- the install
    // thread, the game thread (`enqueue_save_job_hook`, `quit_phase_settle_hook`) and the
    // SL worker (`record`) -- and a shared fixed tmp name lets one `fs::write` truncate
    // the file another is about to rename. The result is a torn JSON that the checker
    // cannot parse, which it reports as the hard and completely wrong verdict "the DLL
    // never installed". A counter costs nothing and removes the race without a lock held
    // across file I/O inside a detour.
    let serial = PUBLISH_SERIAL.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.tmp.{serial}"));
    if fs::write(&tmp, body).is_ok() && fs::rename(&tmp, &path).is_err() {
        // The rename is what makes the snapshot visible; a failure here means the
        // on-disk file is stale, which must not pass silently.
        PUBLISH_FAILURES.fetch_add(1, Ordering::Relaxed);
        let _ = fs::remove_file(&tmp);
    }
}

static PUBLISH_SERIAL: AtomicU64 = AtomicU64::new(0);
static PUBLISH_FAILURES: AtomicU64 = AtomicU64::new(0);

fn hex_list(values: &[usize]) -> String {
    values
        .iter()
        .map(|value| format!("\"0x{value:x}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}
