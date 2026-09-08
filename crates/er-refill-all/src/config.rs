//! `er-refill-all.toml`, re-read while the game runs.
//!
//! Hand-parsed rather than pulled through a TOML crate: a handful of scalar settings do not
//! justify a dependency in a DLL cross-compiled into the game process. Same shape as
//! `er-enemynpc-effects`, for the same reason.
//!
//! # Why it reloads
//!
//! A hotkey read once at attach means restarting Elden Ring to change it -- a long way to go to
//! find out you picked a combination another mod already took, and finding that out is exactly
//! what makes someone want to change it. `er_hotkey_config::HotFile` re-reads about once a second
//! and compares the file's text, not its mtime, because mtime has one-second resolution on the
//! filesystems a Wine prefix sits on and a re-save moves it without changing anything.

// Windows-only in practice; kept portable so the parser and the reload decision are covered by
// `cargo test` on the host, where the windows-gated modules that consume them compile out.
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    fs,
    path::PathBuf,
    sync::{Mutex, MutexGuard, OnceLock},
};

use er_hotkey_config::{
    Binding, BindingUpdate, FileChange, HotFile, chord_name,
    keys::{Chord, parse_chord},
    pad::{PadChord, pad_chord_name, parse_pad_chord},
};

use crate::log::refill_log;

const CONFIG_FILE_NAME: &str = "er-refill-all.toml";

/// Select + Start, held together. The user-chosen default.
const DEFAULT_PAD_HOTKEY: &str = "select+start";

/// Empty on purpose: the pad chord is the shipped binding, and a keyboard default would be a
/// second way to fire the feature that nobody asked for and that could collide with another mod.
const DEFAULT_KEYBOARD_HOTKEY: &str = "";

const DEFAULT_CONFIG_TOML: &str = r#"# er-refill-all standalone DLL configuration.
#
# Press the combination while the STORAGE BOX is open and every item the game considers
# auto-refillable is marked to refill. Press it again and they are all marked not to refill.
# It cycles: on, off, on, off.
#
# IT ONLY DOES ANYTHING IN THE STORAGE BOX. The DLL runs from inside the storage box menu's own
# per-frame update, so with any other menu open -- or none -- the code simply never runs. This is
# not a check that could be got wrong; it is where the feature lives.
#
# EDITS TAKE EFFECT IMMEDIATELY. This file is re-read about once a second while the game runs;
# there is no need to restart. The log names the old binding and the new one each time one moves.

# Controller combination. Every named button must be held together; order does not matter, and
# holding other buttons as well is fine. Empty disables the controller binding.
#
#   select (back, share, view)   start (options, menu)
#   a/cross  b/circle  x/square  y/triangle
#   lb/l1  rb/r1  ls/l3  rs/r3
#   dpad_up  dpad_down  dpad_left  dpad_right
gamepad_hotkey = "select+start"

# Keyboard combination, if you would rather use one. Empty means no keyboard binding.
# Modifiers ctrl/alt/shift plus one key, e.g. "ctrl+shift+r". Same key names as the other
# er-* DLLs in this profile.
hotkey = ""

# After marking, call the game's own storage -> inventory refill immediately, instead of leaving
# it until the next site of grace or load. Set false to only change the marks.
refill_immediately = true
"#;

/// The settings in force, and the machinery that keeps them current.
///
/// Free of Windows types on purpose, so the whole reload decision is `cargo test`-able.
#[derive(Clone, Debug)]
pub(crate) struct RefillConfig {
    pub(crate) config_path: PathBuf,
    /// Not an `er_hotkey_config::Binding`: that type's parser must return `KeyParseError`, whose
    /// `Unknown` message tells the reader to pick a key -- and listing keyboard names at someone
    /// who mistyped a pad button is a worse answer than no message. The keep-the-last-working-value
    /// rule it exists to enforce is reimplemented below, which is the part that matters.
    pad: PadChord,
    pub(crate) pad_text: String,
    keyboard: Binding<Option<Chord>>,
    pub(crate) keyboard_text: String,
    pub(crate) refill_immediately: bool,
}

/// What re-reading the file did, in the terms the log line needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConfigUpdate {
    pub(crate) pad_moved: Option<(String, String)>,
    pub(crate) pad_rejected: Option<(String, String)>,
    pub(crate) keyboard_moved: Option<(String, String)>,
    pub(crate) keyboard_rejected: Option<(String, String)>,
    pub(crate) refill_immediately_moved: Option<(bool, bool)>,
}

impl ConfigUpdate {
    /// A poll that changed nothing must produce no log line at all.
    pub(crate) const fn is_quiet(&self) -> bool {
        self.pad_moved.is_none()
            && self.pad_rejected.is_none()
            && self.keyboard_moved.is_none()
            && self.keyboard_rejected.is_none()
            && self.refill_immediately_moved.is_none()
    }
}

fn default_pad() -> PadChord {
    parse_pad_chord(DEFAULT_PAD_HOTKEY).expect("the built-in default pad chord parses")
}

/// An empty value is a real setting -- "no keyboard binding" -- not a parse failure.
fn parse_optional_chord(raw: &str) -> Result<Option<Chord>, er_hotkey_config::keys::KeyParseError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    parse_chord(raw).map(Some)
}

fn keyboard_name(chord: Option<Chord>) -> String {
    chord.map_or_else(|| "(none)".to_owned(), chord_name)
}

impl RefillConfig {
    fn new(path: PathBuf) -> Self {
        Self {
            config_path: path,
            pad: default_pad(),
            pad_text: DEFAULT_PAD_HOTKEY.to_owned(),
            keyboard: Binding::new(None),
            keyboard_text: DEFAULT_KEYBOARD_HOTKEY.to_owned(),
            refill_immediately: true,
        }
    }

    pub(crate) const fn pad(&self) -> PadChord {
        self.pad
    }

    pub(crate) fn keyboard(&self) -> Option<Chord> {
        self.keyboard.code()
    }

    /// Apply one file's text to the settings in force.
    ///
    /// An absent key keeps what is already in force -- the built-in default on the first load, the
    /// last good value on a reload. That is what makes deleting a line the same as never having
    /// written it.
    pub(crate) fn apply(&mut self, text: &str) -> ConfigUpdate {
        let mut update = ConfigUpdate::default();

        if let Some(raw) = setting(text, "gamepad_hotkey") {
            // An empty value is a real setting -- "no controller binding" -- not a typo.
            let parsed = if raw.trim().is_empty() {
                Ok(PadChord::default())
            } else {
                parse_pad_chord(raw)
            };
            match parsed {
                Ok(chord) if chord == self.pad => {
                    // Record the spelling actually used, so the status line echoes their file
                    // rather than the last spelling of the same chord. Not a change.
                    self.pad_text = raw.to_owned();
                }
                Ok(chord) => {
                    let before = pad_chord_name(self.pad);
                    self.pad = chord;
                    self.pad_text = raw.to_owned();
                    update.pad_moved = Some((before, pad_chord_name(chord)));
                }
                // A rejection is not a change, and the last working chord stays in force. Not the
                // shipped default -- that would drag someone back onto a collision they had just
                // escaped -- and not nothing. Counting a rejection as a change would make a config
                // with a permanent typo re-report, and re-prime the edge, on every single reload.
                Err(error) => {
                    update.pad_rejected =
                        Some((format!("{raw:?}: {error}"), pad_chord_name(self.pad)));
                }
            }
        }

        if let Some(raw) = setting(text, "hotkey") {
            let before = keyboard_name(self.keyboard.code());
            match self.keyboard.apply(raw, parse_optional_chord) {
                BindingUpdate::Unchanged => self.keyboard_text = raw.to_owned(),
                BindingUpdate::Changed { .. } => {
                    self.keyboard_text = raw.to_owned();
                    update.keyboard_moved = Some((before, keyboard_name(self.keyboard.code())));
                }
                BindingUpdate::Rejected { value, error, kept } => {
                    update.keyboard_rejected =
                        Some((format!("{value:?}: {error}"), keyboard_name(kept)));
                }
            }
        }

        if let Some(raw) = setting(text, "refill_immediately") {
            let refill = raw != "false";
            if refill != self.refill_immediately {
                update.refill_immediately_moved = Some((self.refill_immediately, refill));
                self.refill_immediately = refill;
            }
        }

        update
    }
}

struct ConfigState {
    config: RefillConfig,
    hot: HotFile,
}

static CONFIG: OnceLock<Mutex<ConfigState>> = OnceLock::new();

fn config_path() -> PathBuf {
    PathBuf::from(CONFIG_FILE_NAME)
}

/// Strip an inline `#` comment and surrounding whitespace/quotes from a value.
fn scalar(raw: &str) -> &str {
    match raw.split_once('#') {
        Some((before, _)) => before,
        None => raw,
    }
    .trim()
    .trim_matches('"')
}

fn setting<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .find_map(|line| {
            let (name, value) = line.split_once('=')?;
            (name.trim() == key).then(|| scalar(value))
        })
}

fn parse_config(text: &str, path: PathBuf) -> RefillConfig {
    let mut config = RefillConfig::new(path);
    let update = config.apply(text);
    if let Some((rejected, kept)) = update.pad_rejected {
        refill_log(format_args!(
            "config: gamepad_hotkey {rejected}; staying on {kept}"
        ));
    }
    if let Some((rejected, kept)) = update.keyboard_rejected {
        refill_log(format_args!("config: hotkey {rejected}; staying on {kept}"));
    }
    config
}

fn state() -> MutexGuard<'static, ConfigState> {
    let state = CONFIG.get_or_init(|| {
        let path = config_path();
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(_) => {
                let _ = fs::write(&path, DEFAULT_CONFIG_TOML);
                DEFAULT_CONFIG_TOML.to_owned()
            }
        };
        let mut hot = HotFile::new(path.clone());
        // Adopt what we just read, so the first poll a second from now is not a spurious reload of
        // text nothing has touched -- and a spurious reload re-primes the edge detectors.
        hot.adopt(text.clone());
        Mutex::new(ConfigState {
            config: parse_config(&text, path),
            hot,
        })
    });
    // A poisoned lock means a previous holder panicked; the settings inside are still a valid
    // config, and refusing to read them would disable the feature over a fault that already
    // happened.
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Read the config, writing the commented default file when there is not one yet.
pub(crate) fn init_config() -> RefillConfig {
    state().config.clone()
}

/// The settings in force right now.
pub(crate) fn config() -> RefillConfig {
    state().config.clone()
}

/// Re-read the file if it changed, and report what moved. `None` when nothing happened.
pub(crate) fn poll_reload() -> Option<ConfigUpdate> {
    let mut guard = state();
    let ConfigState { config, hot } = &mut *guard;
    match hot.poll()? {
        FileChange::Text(text) => {
            let update = config.apply(&text);
            (!update.is_quiet()).then_some(update)
        }
        FileChange::Missing => {
            // A deleted config is not an instruction to unbind the hotkey, and re-writing the
            // default here would fight someone who is mid-edit.
            refill_log(format_args!(
                "config: {} disappeared; keeping the settings already in force",
                config.config_path.display()
            ));
            None
        }
    }
}

/// Log what a reload changed. Split from [`poll_reload`] so the decision stays host-testable.
pub(crate) fn log_update(update: &ConfigUpdate) {
    if let Some((from, to)) = &update.pad_moved {
        refill_log(format_args!("config: gamepad_hotkey {from} -> {to}"));
    }
    if let Some((rejected, kept)) = &update.pad_rejected {
        refill_log(format_args!(
            "config: gamepad_hotkey {rejected}; staying on {kept}"
        ));
    }
    if let Some((from, to)) = &update.keyboard_moved {
        refill_log(format_args!("config: hotkey {from} -> {to}"));
    }
    if let Some((rejected, kept)) = &update.keyboard_rejected {
        refill_log(format_args!("config: hotkey {rejected}; staying on {kept}"));
    }
    if let Some((from, to)) = &update.refill_immediately_moved {
        refill_log(format_args!("config: refill_immediately {from} -> {to}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> RefillConfig {
        RefillConfig::new(PathBuf::from("er-refill-all.toml"))
    }

    #[test]
    fn the_shipped_default_file_reproduces_the_built_in_defaults() {
        let mut config = cfg();
        let update = config.apply(DEFAULT_CONFIG_TOML);
        assert!(
            update.is_quiet(),
            "the shipped file must parse to the built-in defaults, moved nothing: {update:?}"
        );
        assert_eq!(pad_chord_name(config.pad()), "Select+Start");
        assert_eq!(config.keyboard(), None);
        assert!(config.refill_immediately);
    }

    #[test]
    fn an_empty_keyboard_value_is_no_binding_not_a_rejection() {
        let mut config = cfg();
        let update = config.apply("hotkey = \"\"\n");
        assert_eq!(update.keyboard_rejected, None);
        assert_eq!(config.keyboard(), None);
    }

    #[test]
    fn a_malformed_pad_chord_keeps_the_last_working_one() {
        let mut config = cfg();
        config.apply("gamepad_hotkey = \"lb+rb\"\n");
        let kept = config.pad();
        let update = config.apply("gamepad_hotkey = \"lb+turbo\"\n");
        assert!(update.pad_rejected.is_some(), "the typo is reported");
        assert_eq!(update.pad_moved, None, "a rejection is NOT a change");
        assert_eq!(config.pad(), kept, "the working chord stays in force");
    }

    /// A rejection counted as a change would fire one phantom press per reload, forever, for a
    /// config with a permanent typo in it.
    #[test]
    fn re_reading_an_unchanged_file_moves_nothing() {
        let mut config = cfg();
        config.apply(DEFAULT_CONFIG_TOML);
        for _ in 0..3 {
            assert!(config.apply(DEFAULT_CONFIG_TOML).is_quiet());
        }
    }

    #[test]
    fn a_deleted_line_keeps_what_was_already_in_force() {
        let mut config = cfg();
        config.apply("gamepad_hotkey = \"lb+rb\"\nrefill_immediately = false\n");
        let kept = config.pad();
        let update = config.apply("# everything commented out\n");
        assert!(update.is_quiet());
        assert_eq!(config.pad(), kept);
        assert!(!config.refill_immediately);
    }

    #[test]
    fn inline_comments_and_spacing_do_not_reach_the_parser() {
        let mut config = cfg();
        config.apply("  gamepad_hotkey  =  \"lb+rb\"   # the old binding\n");
        assert_eq!(pad_chord_name(config.pad()), "LB+RB");
    }
}
