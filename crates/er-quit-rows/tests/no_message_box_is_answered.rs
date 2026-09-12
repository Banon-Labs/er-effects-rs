//! This shell must never answer a `CS::MessageBoxDialog` for the player.
//!
//! `er-quit-rows` began as a copy of `er-quickload`, which answers boxes on purpose: its autoload
//! drives the front end past the connection-error and offline notices before a player exists. This
//! crate has no autoload. It puts rows in front of someone who is sitting at a menu, so every box
//! it can reach belongs to that person.
//!
//! The two were the same code until 2026-09-11, and the cost was measured rather than argued:
//! picking a character from the title's Load Game list built a confirmation box while `in_world`
//! was still false, `msgbox_builder_hook` captured it, and the next game-task tick called the
//! dialog's own first-button handler on it. The player saw the box appear and vanish.
//!
//! A deletion cannot be proven by a run -- nothing happens, and nothing happening is what every
//! broken build also looks like. So it is pinned here instead: the source may capture a dialog, and
//! may read its fields, and may not press anything on it. Comments are stripped before the scan, so
//! the notes left where the deleted code used to live do not re-arm the rule they describe.

use std::path::{Path, PathBuf};

/// Names that only ever appear in code that presses a button on a live dialog.
const DECIDE_CALLS: &[&str] = &[
    "MSGBOX_OK_HANDLER_RVA",
    "MSGBOX_ONDECIDE_RVA",
    "MsgBoxRva::OkHandler",
    "MsgBoxRva::OnDecide",
    "MsgBoxRva::ForceStop",
    "force_dismiss_startup_dialog",
];

/// Dialog fields whose meaning is "which button was chosen" or "how far along the decide is".
/// Reading them is how the save flow learns the player's answer; writing them is how the deleted
/// code manufactured one, so only a write is banned.
const DECIDE_FIELDS: &[&str] = &[
    "MSGBOX_BUTTON_COUNT_25E8_OFFSET",
    "MSGBOX_DEFAULT_CURSOR_25E0_OFFSET",
    "MSGBOX_CONFIRM_LATCH_1BC0_OFFSET",
    "MSGBOX_FADE_CURRENT_1278_OFFSET",
    "MSGBOX_JOB_RESULT_STATE_1E8_OFFSET",
    "0x25e0",
    "0x25e8",
    "0x1bc0",
];

fn source_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("the crate's own src directory is readable") {
        let path = entry.expect("a readable directory entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Drop `//` comments, so a note describing deleted code is not read as the code itself. Block
/// comments are left alone: this crate does not use them, and a half-written stripper that
/// mishandles one would fail open on the line that matters.
fn code_only(line: &str) -> &str {
    match line.find("//") {
        Some(at) => &line[..at],
        None => line,
    }
}

#[test]
fn no_source_file_presses_a_button_on_a_message_box() {
    let mut files = Vec::new();
    rust_sources(&source_root(), &mut files);
    assert!(
        files.len() > 50,
        "the scan found only {} source files, so it is looking in the wrong place",
        files.len()
    );
    let mut offences = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("a readable source file");
        for (number, line) in text.lines().enumerate() {
            let code = code_only(line);
            for name in DECIDE_CALLS {
                if code.contains(name) {
                    offences.push(format!("{}:{}: {name}", file.display(), number + 1));
                }
            }
            if !code.contains("*mut") {
                continue;
            }
            for field in DECIDE_FIELDS {
                if code.contains(field) {
                    offences.push(format!("{}:{}: writes {field}", file.display(), number + 1));
                }
            }
        }
    }
    assert!(
        offences.is_empty(),
        "this crate must never answer a message box for the player, but these lines do:\n  {}",
        offences.join("\n  ")
    );
}
