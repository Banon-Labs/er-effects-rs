//! `er-enemynpc-effects.toml`, re-read while the game runs.
//!
//! Hand-parsed rather than pulled through a TOML crate: four scalar settings do not justify a
//! dependency in a DLL that is cross-compiled into the game process.
//!
//! # Why it reloads
//!
//! The hotkey was read once, at attach, into a `OnceLock`. Changing it meant quitting Elden Ring,
//! which is a long way to go to find out you picked a key another mod had already taken -- and
//! finding that out is exactly what makes a player want to change it. The file is now re-read about
//! once a second (`er_hotkey_config::HotFile`, which compares the file's text so a fast edit cannot
//! fall inside one mtime tick) and the new key is live on the next keyboard poll.
//!
//! Two rules the reload has to get right, both of them ways a "working" reload looks broken:
//!
//! * A key that moved RESETS the edge detector. Without that, a key held at the instant of the swap
//!   is already latched as down, and releasing it -- or the very next poll -- reads as a fresh
//!   press the player never made.
//! * A key that did not move must not report a change, or every reformatting of the file produces
//!   that same phantom press.
//!
//! # Why it also writes
//!
//! `enabled` is the one setting that changes from inside the game: the hotkey toggles it. Holding
//! that in memory only means the feature is off again on every launch, so [`persist_enabled`]
//! writes it back here -- which makes this DLL both the writer and the watcher of one file, and
//! that is a feedback loop with three ways to go wrong. All three are handled where they arise
//! rather than discovered at runtime:
//!
//! * The write must not read back as somebody's edit. `HotFile` compares text, so the write is
//!   followed by a read-back and [`HotFile::adopt`] of the exact bytes that landed. A reload
//!   resets the key edge detector, so a self-write reported as an edit is a phantom keypress once
//!   a second.
//! * The write must not destroy the file. It is mostly comments, and those comments are this
//!   feature's only documentation. `er_hotkey_config::persist::set_scalar` rewrites the one
//!   `enabled` line where it already is and copies every other byte through.
//! * The player may be editing at the same time. The write re-reads the file immediately before
//!   rewriting it, so an edit made since the last poll is carried through rather than reverted,
//!   and the read-back is applied so that edit takes effect now instead of a second later. What is
//!   left is a microsecond-wide window between that read and the rename in which their save loses;
//!   nothing portable closes it, and only the `enabled` line is ever at stake.

// Windows-only in practice; kept portable so the parser and the reload decision below are covered
// by `cargo test` on the host, where the windows-gated modules that consume them are compiled out.
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    fs,
    path::PathBuf,
    sync::{
        Mutex, MutexGuard, OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use er_hotkey_config::{
    Binding, BindingUpdate, FileChange, HotFile, chord_name,
    persist::{set_scalar, write_atomic},
};

use crate::{
    keys::{Chord, parse_scancode_chord},
    log::charm_log,
};

const CONFIG_FILE_NAME: &str = "er-enemynpc-effects.toml";

/// SpEffect 20503350, `[Item] Charming Branch` in `SpEffectParam`.
///
/// Its `stateInfo` is 132 (`0x84`), and that number is the whole charm mechanism:
/// `CS::ChrIns::GetTeamType` (`0x1403f1a60`, 1.16.2) reports `TeamType::Charmed` for any character
/// whose `SpecialEffect` container holds an effect with that state, and reports the character's own
/// `teamType` otherwise. Nothing else gates it -- there is no `enableCharm` check on that path --
/// so putting this row on a `ChrIns` is exactly what the thrown item does to whatever it hits.
///
/// The base-game `Bewitching Branch` (503350) is the same 180-second `stateInfo` 132 effect and
/// works identically here.
pub(crate) const DEFAULT_EFFECT_ID: i32 = 20503350;

const DEFAULT_HOTKEY: &str = "ctrl+alt+c";

/// The one key this DLL writes back into the player's file.
const ENABLED_KEY: &str = "enabled";

/// Written above `enabled` when the key has to be APPENDED -- which is what happens to a config
/// created by a build that predates the toggle being persisted at all. A bare `enabled = true`
/// appearing in somebody's file with no explanation is worse than no line.
const ENABLED_COMMENT: &str = "\
# Whether the charm is ON right now. The hotkey writes this line each time it toggles, so the
# state survives a relaunch. Editing it by hand while the game runs works too. Absent, or
# anything other than true, is off.";

/// Toggle write-backs attempted, and the ones that failed. The status line carries both, so
/// "the toggle did not survive the relaunch" is answerable from the log instead of by eye.
static PERSIST_WRITES: AtomicUsize = AtomicUsize::new(0);
static PERSIST_FAILURES: AtomicUsize = AtomicUsize::new(0);

const DEFAULT_CONFIG_TOML: &str = r#"# er-enemynpc-effects standalone DLL configuration.
#
# Press the hotkey in-game to toggle "charm every loaded enemy" on and off. While it is on the
# DLL re-applies the effect to any loaded enemy that is not currently under it, so newly spawned
# enemies are charmed as they appear and the 180-second duration never lapses.
#
# EDITS TAKE EFFECT IMMEDIATELY. This file is re-read about once a second while the game runs;
# there is no need to restart. The log names the old key and the new one each time it moves.
#
# Modifiers: ctrl, alt, shift (either side of the keyboard counts). One trigger key, by name:
#
#   A..Z, 0..9, F1..F15
#   Insert Delete Home End PageUp PageDown Backspace Tab Enter Escape Space
#   Left Up Right Down PrintScreen ScrollLock NumLock Pause CapsLock
#   punctuation by symbol or name: - = [ ] \ ; ' , . / `  (Minus, Equals, LeftBracket,
#     RightBracket, Backslash, Semicolon, Quote, Comma, Period, Slash, Grave)
#   keypad: KP_0..KP_9, KP_Plus, KP_Minus, KP_Multiply, KP_Divide, KP_Period, KP_Enter
#
# Case and spacing do not matter. A key this file does not recognise is reported in the log and
# THE PREVIOUS KEY STAYS IN FORCE -- a typo never leaves you with no hotkey at all.
hotkey = "ctrl+alt+c"
# SpEffectParam row to apply. 20503350 is Charming Branch; 503350 is Bewitching Branch, the
# base-game item with the same charm state and duration.
#
# THOSE TWO ARE THE ONLY CHARM ROWS. Charmed is not a property of the item, it is `stateInfo` 132,
# and exactly 2 of SpEffectParam's 11,325 rows carry it -- the two above. Any other id here still
# gets applied to every loaded enemy, but it applies whatever that row does; it does not charm.
# (1653000, for instance, is `[Incantation] Darkness`: it clears their target, it does not turn
# them.)
effect_id = 20503350
# Strip the effect from every charmed enemy when the hotkey toggles the feature back off. With
# this false the charm instead lapses on its own, up to 180 seconds later.
remove_on_disable = true
# Whether the charm is ON right now. The hotkey WRITES this line each time it toggles, so the
# state survives a relaunch -- start the game with it true and enemies are charmed from the first
# frame the sweep finds them. Editing it by hand while the game runs works too, and takes effect
# on the next reload like every other setting here. Absent, or anything other than true, is off.
enabled = false
"#;

/// The settings in force, and the machinery that keeps them current.
///
/// Deliberately free of Windows types so the whole reload decision is `cargo test`-able: the game
/// side of this crate consumes [`CharmConfig::hotkey`] and nothing else.
#[derive(Clone, Debug)]
pub(crate) struct CharmConfig {
    pub(crate) config_path: PathBuf,
    hotkey: Binding<Chord>,
    pub(crate) hotkey_text: String,
    pub(crate) effect_id: i32,
    pub(crate) remove_on_disable: bool,
    /// Whether the charm is on. The only setting this DLL writes as well as reads.
    ///
    /// False when the key is absent, and deliberately so: a fresh install, or a player who
    /// deleted the line, must not find every enemy in the world charmed on their first launch
    /// because a file was missing. The fail-safe direction for a toggle is off.
    pub(crate) enabled: bool,
}

/// What re-reading the file did, in the terms the log line needs.
///
/// `Default` is "nothing moved", which is what almost every poll produces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConfigUpdate {
    /// `(old name, new name)` when the hotkey moved. This is the edge-reset signal.
    pub(crate) hotkey_moved: Option<(String, String)>,
    /// The value that could not be read, and what is still in force instead.
    pub(crate) hotkey_rejected: Option<(String, String)>,
    pub(crate) effect_id_moved: Option<(i32, i32)>,
    pub(crate) remove_on_disable_moved: Option<(bool, bool)>,
    /// The toggle moved because the file said so -- a hand edit, or the state restored at attach.
    /// The caller drives the sweep off this exactly as it does off a keypress.
    pub(crate) enabled_moved: Option<(bool, bool)>,
}

impl ConfigUpdate {
    /// Did anything move? A poll that changed nothing must produce no log line at all.
    pub(crate) const fn is_quiet(&self) -> bool {
        self.hotkey_moved.is_none()
            && self.hotkey_rejected.is_none()
            && self.effect_id_moved.is_none()
            && self.remove_on_disable_moved.is_none()
            && self.enabled_moved.is_none()
    }
}

fn default_hotkey() -> Chord {
    parse_scancode_chord(DEFAULT_HOTKEY).expect("the built-in default hotkey parses")
}

impl CharmConfig {
    fn new(path: PathBuf) -> Self {
        Self {
            config_path: path,
            hotkey: Binding::new(default_hotkey()),
            hotkey_text: DEFAULT_HOTKEY.to_owned(),
            effect_id: DEFAULT_EFFECT_ID,
            remove_on_disable: true,
            enabled: false,
        }
    }

    pub(crate) fn hotkey(&self) -> Chord {
        self.hotkey.code()
    }

    /// Apply one file's text to the settings in force.
    ///
    /// Absent keys keep what is already in force -- which for the first load is the built-in
    /// default, and for a reload is whatever was last read successfully. That is what makes
    /// deleting a line the same as never having written it.
    pub(crate) fn apply(&mut self, text: &str) -> ConfigUpdate {
        let mut update = ConfigUpdate::default();

        if let Some(raw) = setting(text, "hotkey").filter(|value| !value.is_empty()) {
            let before = chord_name(self.hotkey.code());
            match self.hotkey.apply(raw, parse_scancode_chord) {
                BindingUpdate::Unchanged => {
                    // Still record the spelling the player used, so the status line echoes their
                    // file rather than the last spelling of the same key.
                    self.hotkey_text = raw.to_owned();
                }
                BindingUpdate::Changed { .. } => {
                    self.hotkey_text = raw.to_owned();
                    update.hotkey_moved = Some((before, chord_name(self.hotkey.code())));
                }
                BindingUpdate::Rejected { value, error, kept } => {
                    update.hotkey_rejected =
                        Some((format!("{value:?}: {error}"), chord_name(kept)));
                }
            }
        }

        if let Some(effect_id) = setting(text, "effect_id").and_then(|v| v.parse::<i32>().ok())
            && effect_id != self.effect_id
        {
            update.effect_id_moved = Some((self.effect_id, effect_id));
            self.effect_id = effect_id;
        }

        if let Some(raw) = setting(text, "remove_on_disable") {
            let remove = raw != "false";
            if remove != self.remove_on_disable {
                update.remove_on_disable_moved = Some((self.remove_on_disable, remove));
                self.remove_on_disable = remove;
            }
        }

        // Anything that is not `true` is off -- the opposite convention to `remove_on_disable`
        // above, and on purpose: that one defaults on so an unreadable value should keep it on,
        // this one defaults off so an unreadable value must not charm the world.
        if let Some(raw) = setting(text, ENABLED_KEY) {
            let enabled = raw.eq_ignore_ascii_case("true");
            if enabled != self.enabled {
                update.enabled_moved = Some((self.enabled, enabled));
                self.enabled = enabled;
            }
        }

        update
    }
}

/// The live settings plus the file watcher that keeps them current.
struct ConfigState {
    config: CharmConfig,
    hot: HotFile,
}

static CONFIG: OnceLock<Mutex<ConfigState>> = OnceLock::new();

fn config_path() -> PathBuf {
    PathBuf::from(CONFIG_FILE_NAME)
}

/// Strip an inline `#` comment and surrounding whitespace/quotes from a value.
fn scalar(raw: &str) -> &str {
    let value = match raw.split_once('#') {
        Some((before, _)) => before,
        None => raw,
    }
    .trim();
    value.trim_matches('"')
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

fn parse_config(text: &str, path: PathBuf) -> CharmConfig {
    let mut config = CharmConfig::new(path);
    let update = config.apply(text);
    if let Some((rejected, kept)) = update.hotkey_rejected {
        charm_log(format_args!("config: hotkey {rejected}; staying on {kept}"));
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
        // Adopt what we just read, so the first poll a second from now is not a spurious reload
        // of text nothing has touched.
        hot.adopt(text.clone());
        Mutex::new(ConfigState {
            config: parse_config(&text, path),
            hot,
        })
    });
    // A poisoned lock here means a previous holder panicked; the settings inside are still a valid
    // config, and refusing to read them would disable the feature over a fault that already
    // happened.
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Read the config, writing the commented default file when there is not one yet.
pub(crate) fn init_config() -> CharmConfig {
    state().config.clone()
}

/// The settings in force right now.
pub(crate) fn config() -> CharmConfig {
    state().config.clone()
}

/// The hotkey in force, without cloning the paths and strings around it.
///
/// This is on the per-frame path, so it takes the lock and copies 8 bytes rather than allocating.
pub(crate) fn live_hotkey() -> Chord {
    state().config.hotkey()
}

/// Re-read the file if it changed, and report what moved.
///
/// Returns `None` when nothing happened -- which is the overwhelmingly common case, and the one
/// that must stay silent in the log. `Some` carries a report the caller logs and acts on: a moved
/// hotkey is the signal to reset the key edge state.
pub(crate) fn poll_reload() -> Option<ConfigUpdate> {
    let mut guard = state();
    let ConfigState { config, hot } = &mut *guard;
    match hot.poll()? {
        FileChange::Text(text) => {
            let update = config.apply(&text);
            (!update.is_quiet()).then_some(update)
        }
        FileChange::Missing => {
            // The file was deleted. Keep the settings that were working -- a deleted config is
            // not an instruction to unbind the hotkey, and re-writing the default file here would
            // fight a player who is mid-edit.
            charm_log(format_args!(
                "config: {} disappeared; keeping the settings already in force",
                config.config_path.display()
            ));
            None
        }
    }
}

/// What writing the toggle state back to disk did.
#[derive(Clone, Debug, Default)]
pub(crate) struct PersistOutcome {
    /// Anything the file turned out to say that the settings in force did not.
    ///
    /// The write re-reads the file first, so an edit the player made since the last poll is in
    /// this text. Applying it here rather than leaving it for the next poll is what stops the
    /// write-back from swallowing their edit: it goes into force now, and the caller logs and acts
    /// on it exactly as it does for a reload.
    pub(crate) update: ConfigUpdate,
    /// `None` when the file now holds the new state. `Some` is the reason it does not -- a
    /// read-only game directory, a full disk -- and the state is live but will not survive a
    /// relaunch.
    pub(crate) error: Option<String>,
}

/// The text a write-back must be built on, or the reason there must not be one.
///
/// `Ok(None)` is the only case that licenses writing the shipped default: the file is not there,
/// which is how it gets created in the first place. Every other read failure means the file exists
/// and we could not see it -- a permission bit, a lock, a bad sector -- and the one thing that must
/// not happen then is writing [`DEFAULT_CONFIG_TOML`] over a config nobody has read. That turns an
/// unreadable file into a destroyed one, and it would take the player's whole config with it.
fn readable_base(path: &std::path::Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!(
            "could not re-read '{}' ({err}); refusing to write the default over a file that is \
             there but unreadable",
            path.display()
        )),
    }
}

/// Write `enabled` back into the player's config, so the toggle survives a relaunch.
///
/// Read-modify-write, deliberately, and re-reading here rather than reusing the text the last poll
/// saw: an edit made in the last second is on disk and not in memory, and rewriting from the stale
/// copy would silently undo it.
///
/// Never panics and never blocks on anything but one small file write -- this runs on the game
/// thread, from a keypress. A failure is reported and dropped: the toggle stays live for this
/// session and is simply not persisted.
pub(crate) fn persist_enabled(enabled: bool) -> PersistOutcome {
    PERSIST_WRITES.fetch_add(1, Ordering::Relaxed);
    let mut guard = state();
    let ConfigState { config, hot } = &mut *guard;
    let path = config.config_path.clone();

    let disk = match readable_base(&path) {
        Ok(disk) => disk,
        Err(error) => {
            PERSIST_FAILURES.fetch_add(1, Ordering::Relaxed);
            config.enabled = enabled;
            return PersistOutcome {
                update: ConfigUpdate::default(),
                error: Some(error),
            };
        }
    };
    let base = disk.as_deref().unwrap_or(DEFAULT_CONFIG_TOML);
    let updated = set_scalar(
        base,
        ENABLED_KEY,
        if enabled { "true" } else { "false" },
        ENABLED_COMMENT,
    );

    // In force before the write is checked, because the keypress has already taken effect. What
    // the write decides is only whether the next launch knows about it.
    config.enabled = enabled;

    if let Err(error) = write_atomic(&path, &updated) {
        PERSIST_FAILURES.fetch_add(1, Ordering::Relaxed);
        // Baseline the watcher on what is on disk, not on what we wanted to be there. Without
        // this the failed write comes back a second later as an "edit" carrying the old value,
        // and the toggle the player just pressed flips itself back.
        if let Some(disk) = disk {
            hot.adopt(disk);
        }
        return PersistOutcome {
            update: ConfigUpdate::default(),
            error: Some(error.to_string()),
        };
    }

    // Read back what actually landed rather than trusting `updated`: if a hand edit slipped in
    // between the rename and here, this is the text the next poll would have seen, so adopting
    // and applying it now is both the churn suppression and the pickup of their edit.
    let read_back = fs::read_to_string(&path).unwrap_or(updated);
    hot.adopt(read_back.clone());
    let update = config.apply(&read_back);
    PersistOutcome {
        update,
        error: None,
    }
}

/// Write-backs attempted and write-backs that failed, for the status line.
pub(crate) fn persist_tallies() -> (usize, usize) {
    (
        PERSIST_WRITES.load(Ordering::Relaxed),
        PERSIST_FAILURES.load(Ordering::Relaxed),
    )
}

/// Log what a reload changed. Split from [`poll_reload`] so the decision stays host-testable.
pub(crate) fn log_update(update: &ConfigUpdate) {
    if let Some((from, to)) = &update.hotkey_moved {
        charm_log(format_args!("config: hotkey {from} -> {to}"));
    }
    if let Some((rejected, kept)) = &update.hotkey_rejected {
        charm_log(format_args!("config: hotkey {rejected}; staying on {kept}"));
    }
    if let Some((from, to)) = update.effect_id_moved {
        charm_log(format_args!("config: effect_id {from} -> {to}"));
    }
    if let Some((from, to)) = update.remove_on_disable_moved {
        charm_log(format_args!("config: remove_on_disable {from} -> {to}"));
    }
    if let Some((from, to)) = update.enabled_moved {
        charm_log(format_args!(
            "config: enabled {from} -> {to} (from the file)"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use er_hotkey_config::keys::{MODIFIER_ALT, MODIFIER_CTRL};

    fn parse(text: &str) -> CharmConfig {
        let mut config = CharmConfig::new(PathBuf::from("test.toml"));
        config.apply(text);
        config
    }

    #[test]
    fn the_shipped_default_file_parses_to_the_documented_defaults() {
        let parsed = parse(DEFAULT_CONFIG_TOML);
        assert_eq!(parsed.hotkey_text, DEFAULT_HOTKEY);
        assert_eq!(parsed.hotkey().modifiers, MODIFIER_CTRL | MODIFIER_ALT);
        assert_eq!(parsed.effect_id, DEFAULT_EFFECT_ID);
        assert!(parsed.remove_on_disable);
    }

    #[test]
    fn an_empty_file_still_yields_working_defaults() {
        let parsed = parse("");
        assert_eq!(parsed.effect_id, DEFAULT_EFFECT_ID);
        assert_eq!(parsed.hotkey_text, DEFAULT_HOTKEY);
        assert!(parsed.remove_on_disable);
    }

    #[test]
    fn settings_override_the_defaults() {
        let parsed =
            parse("hotkey = \"shift+f9\"\neffect_id = 503350\nremove_on_disable = false\n");
        assert_eq!(parsed.hotkey_text, "shift+f9");
        assert_eq!(parsed.effect_id, 503350);
        assert!(!parsed.remove_on_disable);
    }

    #[test]
    fn a_commented_out_setting_is_not_a_setting() {
        let parsed = parse("# effect_id = 1\neffect_id = 503350 # trailing comment\n");
        assert_eq!(parsed.effect_id, 503350);
    }

    /// Reload, the whole point: a changed key is picked up, and reported so the caller can reset
    /// its edge state and the log can name both ends.
    #[test]
    fn a_changed_hotkey_is_picked_up_and_names_both_ends() {
        let mut config = parse(DEFAULT_CONFIG_TOML);
        let update = config.apply("hotkey = \"ctrl+alt+v\"\n");
        assert_eq!(
            update.hotkey_moved,
            Some(("Ctrl+Alt+C".to_owned(), "Ctrl+Alt+V".to_owned()))
        );
        assert_eq!(config.hotkey().dik, Some(0x2f), "DIK_V");
        assert!(!update.is_quiet());
    }

    /// RELOAD: an unchanged file must not churn. A reported change resets the key edge detector,
    /// and a reset while the key is held reads as a press the player never made -- so re-applying
    /// identical text, or the same key spelled differently, must stay silent.
    #[test]
    fn an_unchanged_file_does_not_churn() {
        let mut config = parse(DEFAULT_CONFIG_TOML);
        for text in [
            DEFAULT_CONFIG_TOML,
            "hotkey = \"ctrl+alt+c\"\n",
            "hotkey = \"CTRL + ALT + C\"\n",
            "hotkey = \"Ctrl+Alt+C\"\neffect_id = 20503350\nremove_on_disable = true\n",
        ] {
            let update = config.apply(text);
            assert!(update.is_quiet(), "{text:?} reported {update:?}");
        }
    }

    /// RELOAD: a malformed value falls back to the previous working key, not to the built-in
    /// default and not to nothing. A typo must leave the player with a hotkey that still works and
    /// a log line naming the typo.
    #[test]
    fn a_malformed_hotkey_falls_back_to_the_previous_value_not_to_nothing() {
        let mut config = parse(DEFAULT_CONFIG_TOML);
        config.apply("hotkey = \"shift+f9\"\n");
        let working = config.hotkey();
        assert_eq!(working.dik, Some(0x43), "DIK_F9");

        let update = config.apply("hotkey = \"ctrl+alt+nonsense\"\n");
        assert_eq!(config.hotkey(), working, "the working key stays in force");
        assert!(update.hotkey_moved.is_none(), "a rejection is not a move");
        let (rejected, kept) = update.hotkey_rejected.expect("the typo is reported");
        assert!(rejected.contains("nonsense"), "{rejected}");
        assert_eq!(kept, "Shift+F9");
    }

    /// A key with no DirectInput scancode can never appear in the buffer this DLL reads, so it is
    /// refused the same way a typo is rather than becoming a hotkey that silently never fires.
    #[test]
    fn a_key_this_dll_could_never_see_is_refused_like_a_typo() {
        let mut config = parse(DEFAULT_CONFIG_TOML);
        let update = config.apply("hotkey = \"F16\"\n");
        assert!(update.hotkey_rejected.is_some());
        assert_eq!(config.hotkey(), default_hotkey());
    }

    /// A line the file no longer carries keeps whatever was in force. Deleting a setting is the
    /// same as never having written it, not an instruction to unbind.
    #[test]
    fn a_removed_line_keeps_the_setting_in_force() {
        let mut config = parse("hotkey = \"f9\"\neffect_id = 503350\n");
        let update = config.apply("# everything commented out\n");
        assert!(update.is_quiet());
        assert_eq!(config.hotkey().dik, Some(0x43));
        assert_eq!(config.effect_id, 503350);
    }

    /// The config the user actually has in the game directory, as of 2026-09-01: written by a
    /// build that predates `enabled`, hand-edited since, carrying a commented-out alternative and
    /// a trailing note on the live value. Every persistence test below runs against this rather
    /// than against the shipped default, because the shipped default is the easy case.
    const USER_CONFIG: &str = r#"# er-enemynpc-effects standalone DLL configuration.
#
# Press the hotkey in-game to toggle "charm every loaded enemy" on and off. While it is on the
# DLL re-applies the effect to any loaded enemy that is not currently under it, so newly spawned
# enemies are charmed as they appear and the 180-second duration never lapses.
#
# Modifiers: ctrl, alt, shift (either side of the keyboard counts). One trigger key.
hotkey = "ctrl+alt+c"
# SpEffectParam row to apply. 20503350 is Charming Branch; 503350 is Bewitching Branch, the
# base-game item with the same charm state and duration.
# effect_id = 20503350 # bewitching
effect_id = 1653000 # darkness
# Strip the effect from every charmed enemy when the hotkey toggles the feature back off. With
# this false the charm instead lapses on its own, up to 180 seconds later.
remove_on_disable = true
"#;

    /// Rewrite `enabled` the way [`persist_enabled`] does, without the filesystem.
    fn write_enabled(text: &str, enabled: bool) -> String {
        set_scalar(
            text,
            ENABLED_KEY,
            if enabled { "true" } else { "false" },
            ENABLED_COMMENT,
        )
    }

    /// A toggle nobody has ever pressed is off. Not "keep whatever was there", not "on because
    /// the file is missing" -- a fresh install must not charm every enemy in the world.
    #[test]
    fn an_absent_enabled_key_means_off() {
        assert!(!parse("").enabled);
        assert!(!parse(USER_CONFIG).enabled);
        assert!(!parse(DEFAULT_CONFIG_TOML).enabled);
    }

    /// The state reloads from the file like everything else, and names both ends so the log can
    /// say the toggle moved because somebody edited the file rather than pressed the key.
    #[test]
    fn enabled_reloads_from_the_file_and_reports_both_ends() {
        let mut config = parse(DEFAULT_CONFIG_TOML);
        let update = config.apply("enabled = true\n");
        assert_eq!(update.enabled_moved, Some((false, true)));
        assert!(config.enabled);
        assert!(!update.is_quiet());

        let update = config.apply("enabled = true\n");
        assert!(update.is_quiet(), "the same value is not a change");

        let update = config.apply("enabled = false\n");
        assert_eq!(update.enabled_moved, Some((true, false)));
        assert!(!config.enabled);
    }

    /// A value that is not `true` is off -- the fail-safe direction for this particular setting.
    /// `remove_on_disable` goes the other way on purpose; both defaults are the harmless one.
    #[test]
    fn only_true_turns_the_toggle_on() {
        assert!(parse("enabled = true\n").enabled);
        assert!(parse("enabled = TRUE\n").enabled, "case does not matter");
        assert!(parse("enabled = true # left it on\n").enabled);
        for off in [
            "enabled = false\n",
            "enabled = 1\n",
            "enabled = yes\n",
            "enabled =\n",
        ] {
            assert!(!parse(off).enabled, "{off:?}");
        }
    }

    /// The destructive-write test. Persisting the toggle into the user's real file changes one
    /// line and copies every other byte through. The comments are this feature's only
    /// documentation and the commented-out `effect_id` is a choice they mean to come back to; a
    /// writer that renders the four settings it knows about deletes all of it on the first
    /// keypress.
    #[test]
    fn persisting_the_toggle_preserves_every_comment_and_unknown_key() {
        // First press: the key is not in their file at all, so it is appended with its comment.
        let with_key = write_enabled(USER_CONFIG, true);
        assert!(
            with_key.starts_with(USER_CONFIG),
            "appending must not touch a single byte that was already there"
        );
        assert!(with_key.ends_with("enabled = true\n"));

        // Every subsequent press rewrites that one line, in place.
        let off = write_enabled(&with_key, false);
        let changed: Vec<_> = with_key
            .lines()
            .zip(off.lines())
            .filter(|(a, b)| a != b)
            .collect();
        assert_eq!(
            changed,
            vec![("enabled = true", "enabled = false")],
            "exactly one line may differ"
        );
        assert_eq!(with_key.lines().count(), off.lines().count());

        // The specifics, named rather than implied by the diff above.
        for survivor in [
            "# effect_id = 20503350 # bewitching",
            "effect_id = 1653000 # darkness",
            "hotkey = \"ctrl+alt+c\"",
            "# base-game item with the same charm state and duration.",
        ] {
            assert!(off.contains(survivor), "lost: {survivor}");
        }
    }

    /// A key this build has never heard of survives the write. Somebody's config may be newer than
    /// the DLL reading it -- a downgrade, a shared profile -- and eating the settings of the build
    /// they are about to switch back to is the same destruction as eating the comments.
    #[test]
    fn a_setting_this_build_does_not_know_survives_the_write() {
        let text = format!("{USER_CONFIG}charm_radius = 42\nenabled = false\n");
        let out = write_enabled(&text, true);
        assert!(out.contains("charm_radius = 42"));
        assert_eq!(
            out.matches("enabled").count(),
            text.matches("enabled").count()
        );
    }

    /// Round TRIP: parse, toggle, write, re-parse. The value comes back and nothing else moved,
    /// which is the assertion that the writer and the reader agree about the same file.
    #[test]
    fn the_toggle_round_trips_through_the_file() {
        let before = parse(USER_CONFIG);
        assert!(!before.enabled);

        let on_disk = write_enabled(USER_CONFIG, true);
        let after = parse(&on_disk);
        assert!(after.enabled, "the write is readable by the reader");
        assert_eq!(after.effect_id, before.effect_id);
        assert_eq!(after.hotkey_text, before.hotkey_text);
        assert_eq!(after.remove_on_disable, before.remove_on_disable);

        // ...and back, byte for byte: writing the value it already had is a no-op file.
        let back = write_enabled(&on_disk, false);
        assert_eq!(back, write_enabled(&back, false), "the write is idempotent");
        assert!(!parse(&back).enabled);
    }

    /// A file that is missing is the one case that licenses writing the shipped default -- that is
    /// how the config gets created. A file that is there and unreadable must not be: writing the
    /// default over it would turn "could not read your config" into "destroyed your config".
    #[test]
    fn an_unreadable_file_refuses_the_write_instead_of_defaulting_over_it() {
        let dir = std::env::temp_dir().join(format!(
            "er-enemynpc-effects-readable-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch dir");

        let missing = dir.join("absent.toml");
        assert_eq!(
            readable_base(&missing),
            Ok(None),
            "absent is writable from default"
        );

        let present = dir.join("present.toml");
        fs::write(&present, "enabled = true\n").expect("seed");
        assert_eq!(
            readable_base(&present),
            Ok(Some("enabled = true\n".to_owned()))
        );

        // A directory in the file's place: present, and unreadable as text on every platform.
        let blocked = dir.join("blocked.toml");
        fs::create_dir(&blocked).expect("blocker");
        let refused = readable_base(&blocked).expect_err("a directory is not a config");
        assert!(
            refused.contains("refusing to write the default"),
            "{refused}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    /// The self-write must not read as an edit. Applying the text we just wrote to the config that
    /// wrote it reports nothing -- because a reported change resets the key edge detector, and
    /// doing that once a second turns a held key into a keypress the player never made.
    #[test]
    fn re_reading_our_own_write_reports_nothing() {
        let mut config = parse(USER_CONFIG);
        config.enabled = true;
        let written = write_enabled(USER_CONFIG, true);
        let update = config.apply(&written);
        assert!(update.is_quiet(), "our own write reported {update:?}");
    }

    /// A hand edit that lands between the last poll and the write is carried through, not
    /// reverted. The write re-reads the file first, so their new hotkey is in the text we rewrite
    /// -- and applying the read-back is what puts it into force a second early instead of losing
    /// it. This is the whole reason `persist_enabled` re-reads rather than reusing the last poll's
    /// text.
    #[test]
    fn an_edit_made_just_before_the_write_survives_and_takes_effect() {
        let mut config = parse(USER_CONFIG);

        // The player saves a new hotkey; the DLL has not polled yet.
        let edited = USER_CONFIG.replace("ctrl+alt+c", "shift+f9");
        // The keypress reads the file as it is now and rewrites one line of it.
        let written = write_enabled(&edited, true);
        assert!(
            written.contains("shift+f9"),
            "their edit is in what we wrote"
        );

        config.enabled = true;
        let update = config.apply(&written);
        assert_eq!(
            update.hotkey_moved,
            Some(("Ctrl+Alt+C".to_owned(), "Shift+F9".to_owned())),
            "their edit takes effect on the write, not a second later"
        );
        assert!(
            update.enabled_moved.is_none(),
            "our own value is not a move"
        );
    }

    /// The other settings reload too, and each names both ends.
    #[test]
    fn the_other_settings_reload_and_report_both_ends() {
        let mut config = parse(DEFAULT_CONFIG_TOML);
        let update = config.apply("effect_id = 503350\nremove_on_disable = false\n");
        assert_eq!(update.effect_id_moved, Some((DEFAULT_EFFECT_ID, 503350)));
        assert_eq!(update.remove_on_disable_moved, Some((true, false)));
    }
}
