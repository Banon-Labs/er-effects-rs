//! Machine-readable census output.
//!
//! Written event-driven -- once at install and again after each newly observed save
//! write -- rather than on a polling timer. Save writes are rare, so a poll loop
//! would be pure overhead, and this repo treats sleeps as a synchronization smell.
//!
//! The file is the run-stopping oracle for a save-suppression proof: a harness reads
//! `escaped_write_sites` and fails the run if it is non-empty. It is RAM-derived
//! in-process telemetry, never a screenshot.

use std::{fs, path::PathBuf};

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
/// The write goes through the same file APIs this DLL hooks. That is safe because
/// every caller reaches here from inside the witness reentrancy guard, so the
/// detours' record paths short-circuit; the telemetry path also fails the save-path
/// filter, giving a second, independent reason it cannot pollute its own census.
pub(crate) fn write_snapshot() {
    let mut body = String::new();
    body.push_str("{\n");
    body.push_str(&format!(
        "  \"phase\": \"{}\",\n",
        json_escape(crate::PHASE)
    ));
    body.push_str(&format!(
        "  \"census_hooks_installed\": {},\n",
        crate::hooks_installed()
    ));
    body.push_str("  \"census_hooks_expected\": 6,\n");

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
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, body).is_ok() {
        let _ = fs::rename(&tmp, &path);
    }
}

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
