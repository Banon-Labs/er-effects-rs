//! Loading [`LocalInvasionConfig`] from `er-invasion-warp.toml`, and RELOADING it while the game
//! runs.
//!
//! # Hot reload, and why it is mtime-polled rather than event-driven
//!
//! The user edits this file to change where they are hunting, mid-session, and expects the next
//! match to be judged by the new rules. A watcher API (ReadDirectoryChangesW, inotify) would be
//! more elegant and is the wrong tool here: it needs a thread and a handle owned by a DLL that can
//! be unloaded, and it fires on events we do not care about. The filter is consulted once per
//! incoming match -- at most a few times a minute -- so checking the file's mtime at that moment
//! costs one stat on a path we already know, and cannot drift out of sync with the decision it
//! feeds. Polling is not a compromise here; it is the cheaper correct thing.
//!
//! # Parsing without a TOML dependency
//!
//! This crate is `no-std`-adjacent in spirit and is compiled into a DLL that already avoids
//! pulling weight it does not need. The schema is a handful of scalars and one string array, so it
//! is parsed directly. The parser is deliberately STRICT about unknown keys and bad values --
//! reporting them rather than ignoring them -- because a typo in a filter that cancels other
//! players' matches should be visible, not silently inert.
//!
//! # Composition
//!
//! [`parse_local_invasion_config`] takes the file's text and nothing else, so the same schema can
//! be read out of a shared TOML when this crate is used as a library alongside others: hand it the
//! `[local_invasion]` section's body and it behaves identically. [`SECTION_NAME`] is exported for
//! that purpose.

use crate::local_invasion::{LocalInvasionConfig, LocalInvasionMode, PlaceNameTextId};

/// File name looked for next to the DLL.
pub const CONFIG_FILE_NAME: &str = "er-invasion-warp.toml";

/// Section name when this schema is embedded in a shared TOML rather than owning its own file.
pub const SECTION_NAME: &str = "local_invasion";

/// The default file written when none exists. Documented in place, because the person who needs
/// to understand these options is reading this file, not the source.
pub const DEFAULT_CONFIG_TOML: &str = r#"# er-invasion-warp -- local invasion filter.
#
# Seamless decides WHERE an invasion sends you, server-side, and tells the client. This filter
# reads that destination the moment it arrives -- before you have moved -- and cancels matches that
# are not where you want to be. Cancelling is safe: the session returns to idle and searching
# continues. Nothing here fakes an invasion or spoofs session state.
#
# Edits take effect immediately. The file is re-read when the next match arrives; no restart.

[local_invasion]

# Master switch. OFF by default -- this cancels real matches, so it has to be asked for.
enabled = false

# How a destination is judged:
#   "exact" -- only the exact location you are anchored to.
#   "area"  -- that exact location, or anywhere sharing one of its place names. If where you stand
#              carries one name, that is one place to look; if it carries five, five.
#   "named" -- ignore where you are; accept only the locations listed below.
#
# OPEN YOUR WORLD MAP ONCE per session if you use "area" or "named". Place names are read off the
# world map's own rows, so before you have opened it no location has a name, every name-based
# judgement fails closed, and both modes behave like "exact". The log says so the first time it
# happens. "exact" compares locations directly and never needs the map.
mode = "exact"

# HUNT MODE -- ask Steam for ONE location instead of rejecting what it sends.
#
# The filter above DECLINES matches: it sees every host and cancels the ones you do not want, which
# always works but can take many tries. Hunt instead narrows the QUERY, so the answer arrives right
# the first time.
#
# The cost, and why this is separate and off by default: it filters on a key only this DLL
# publishes, so while hunt is on you will NOT see hosts who are not running it. Your friend needs
# it too. Leave it off to keep meeting everybody.
#
# A Steam filter tests ONE value and has no OR, so hunt uses the single marked location if you have
# marked exactly one, or the map you are standing in if you have marked none. Several marked
# locations cannot be expressed and hunt will say so and stay out of the way.
hunt = false

# Announce a rejection on the game's own message banner ("Rejected invasion: m60_42_36_00").
#
# Only a CHANGE of wrong destination is announced. Seamless retries roughly every 20 seconds and
# the same wrong place comes back around constantly, so announcing every one would be wallpaper
# within a minute. Silence after the first means "still being sent to the same wrong place".
reject_notice = false

# Match ONLY other players running this DLL with this option turned on.
#
# Seamless finds worlds with a `lobby_key` that is a fingerprint of your game's params and Seamless
# build -- NOT your co-op password. This rewrites that key into a pool of our own, and because one
# value drives both the search and the advertisement, the separation is symmetric: vanilla players
# cannot see you and you cannot see them.
#
# It is ABSOLUTE, not a preference. While this is on, the entire vanilla population is invisible to
# you for hosting AND for invading -- you will only ever meet other people running this DLL with
# this same option on. Turn it on for a session with friends, not permanently.
dll_users_only = false

# THE TWO USEFUL COMBINATIONS, since these switches are independent:
#
#   enabled = true,  dll_users_only = false   filter by LOCATION, meet everybody (the default use)
#   enabled = false, dll_users_only = true    invade ANYWHERE as normal, but only ever meet other
#                                             DLL users -- a private global community
#
# Both together works too: only DLL users, and only at the place you are standing.

# NOT IMPLEMENTED YET -- anything listed here is parsed and then ignored, and the log says so on
# every load. Turning a typed place name into the FMG text id the game matches on has not been
# reversed, so there is nothing to compare a string against.
#
# Use Shift+Insert instead: stand somewhere, press it, and every location sharing that place's name
# is accepted from then on. That writes `named_location_text_ids` below, which IS consulted.
#
# Be careful with mode = "named": if this list is the only thing you filled in, no ids exist and
# EVERY match is rejected.
named_locations = []

# Locations you marked, and the two lists the in-game keys write to. Both WIDEN whatever `mode`
# allows -- a marked place is always accepted, in every mode -- so you can leave mode = "exact"
# and just collect places as you visit them.
#
#   mark_key         "invade here"      -> allowed_blocks   (and clears any exclusion)
#   unmark_key       "not here"         -> blocked_blocks   (and clears any mark)
#   Shift+mark_key   mark every place sharing this one's name -> named_location_text_ids
#   Shift+unmark_key un-mark those
#
# The world map colours its invasion pins by these two lists, and by nothing else: chosen is the
# brightest marker, excluded the dimmest, and anything in neither list keeps the middle one. That
# is a property of the LOCATION, so the map reads the same wherever you are standing.
#
# The keys rewrite this file, so marks survive a restart. Hand-edits are equally fine; the file is
# re-read when the next match arrives.
allowed_blocks = []

# The two keys, by NAME. Insert and Delete are the defaults, but a 60% keyboard has neither -- so
# name a key you actually have instead. Recognised:
#
#   Insert Delete Home End PageUp PageDown Backspace Tab Enter Escape Space
#   Left Up Right Down PrintScreen ScrollLock Pause CapsLock
#   F1..F24, any single letter or digit ("K", "7")
#   punctuation by symbol or name: - = [ ] \ ; ' , . / `  (Minus, Equals, LeftBracket,
#     RightBracket, Backslash, Semicolon, Quote, Comma, Period, Slash, Grave)
#   keypad: KP_0..KP_9, KP_Plus, KP_Minus, KP_Multiply, KP_Divide, KP_Period
#
# Case and spacing do not matter. A raw virtual-key code works too ("0x2d"). A name this file does
# not recognise is reported in the log and the previous key stays in force -- it is never silently
# ignored, because a key that does nothing is indistinguishable from a broken feature.
mark_key = "Insert"
unmark_key = "Delete"

# Locations you excluded. An exclusion beats everything, including a mode that would accept it.
blocked_blocks = []

# Raw PlaceName text ids. Shift+Insert appends here; you can also add ids by hand.
named_location_text_ids = []
"#;

/// A parse problem worth telling the user about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigIssue {
    /// 1-based line number in the source text.
    pub line: usize,
    /// What was wrong.
    pub message: String,
}

/// Parse result: the config plus everything questionable about it.
///
/// Issues do NOT prevent a config from being returned. A single bad line should not disable a
/// filter the user asked for, but it must be visible -- so the good keys apply and the bad ones
/// are reported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedConfig {
    /// The configuration, with defaults for anything absent.
    pub config: LocalInvasionConfig,
    /// Problems found while parsing.
    pub issues: Vec<ConfigIssue>,
}

/// Parse `er-invasion-warp.toml` text.
///
/// Accepts the schema at top level OR inside a `[local_invasion]` section, so the same function
/// serves the standalone file and the embedded-in-a-shared-TOML case. Sections other than
/// `[local_invasion]` are skipped entirely rather than misread -- when this schema lives in a
/// shared file, another crate's `[section]` keys are none of our business, and warning about them
/// would make a correct shared config look broken.
#[must_use]
pub fn parse_local_invasion_config(text: &str) -> ParsedConfig {
    let mut config = LocalInvasionConfig::default();
    let mut issues = Vec::new();
    // None = top level (ours), Some(true) = inside [local_invasion], Some(false) = someone else's.
    let mut in_our_section: Option<bool> = None;

    for (index, raw_line) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            in_our_section = Some(section.trim() == SECTION_NAME);
            continue;
        }
        if in_our_section == Some(false) {
            continue; // another crate's section in a shared file
        }
        let Some((key, value)) = line.split_once('=') else {
            issues.push(ConfigIssue {
                line: line_no,
                message: format!("not a key = value line: {line:?}"),
            });
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "enabled" => match parse_bool(value) {
                Some(v) => config.enabled = v,
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("enabled must be true or false, got {value:?}"),
                }),
            },
            "dll_users_only" => match parse_bool(value) {
                Some(v) => config.dll_users_only = v,
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("dll_users_only must be true or false, got {value:?}"),
                }),
            },
            "reject_notice" => match parse_bool(value) {
                Some(v) => config.reject_notice = v,
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("reject_notice must be true or false, got {value:?}"),
                }),
            },
            "hunt" => match parse_bool(value) {
                Some(v) => config.hunt = v,
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("enabled must be true or false, got {value:?}"),
                }),
            },
            "mode" => match LocalInvasionMode::parse(&unquote(value)) {
                Some(v) => config.mode = v,
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!(
                        "mode must be \"exact\", \"area\" or \"named\", got {value:?} -- \
                         keeping {:?}",
                        config.mode
                    ),
                }),
            },
            "named_locations" => match parse_string_array(value) {
                Some(v) => config.named_locations = v,
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("named_locations must be an array of strings, got {value:?}"),
                }),
            },
            "mark_key" => match crate::keybind::parse_key(&unquote(value)) {
                Ok(key) => config.mark_key = key,
                // Surfaced as an issue rather than silently kept at the default: a player who
                // mistypes their key otherwise presses it, gets nothing, and has no way to tell
                // that apart from the feature being broken.
                Err(error) => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("mark_key: {error}"),
                }),
            },
            "unmark_key" => match crate::keybind::parse_key(&unquote(value)) {
                Ok(key) => config.unmark_key = key,
                Err(error) => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!("unmark_key: {error}"),
                }),
            },
            "named_location_text_ids" => match parse_int_array(value) {
                Some(v) => config.named_location_text_ids = v.into_iter().collect(),
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!(
                        "named_location_text_ids must be an array of integers, got {value:?}"
                    ),
                }),
            },
            "blocked_blocks" => match parse_block_array(value) {
                Some(v) => config.blocked_blocks = v.into_iter().collect(),
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!(
                        "blocked_blocks must be an array of block ids (0x... or decimal), got \
                         {value:?}"
                    ),
                }),
            },
            "allowed_blocks" => match parse_block_array(value) {
                Some(v) => config.allowed_blocks = v.into_iter().collect(),
                None => issues.push(ConfigIssue {
                    line: line_no,
                    message: format!(
                        "allowed_blocks must be an array of block ids (0x... or decimal), got \
                         {value:?}"
                    ),
                }),
            },
            other => issues.push(ConfigIssue {
                line: line_no,
                message: format!("unknown key {other:?} -- ignored"),
            }),
        }
    }

    ParsedConfig { config, issues }
}

/// Strip a `#` comment, respecting quotes so a `#` inside a place name survives.
fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_quotes = false;
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'"' => in_quotes = !in_quotes,
            b'#' if !in_quotes => return &line[..index],
            _ => {}
        }
    }
    line
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').to_owned()
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn array_body(value: &str) -> Option<&str> {
    value.trim().strip_prefix('[')?.strip_suffix(']')
}

fn parse_string_array(value: &str) -> Option<Vec<String>> {
    let body = array_body(value)?;
    if body.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        // Require quotes: a bare word here is a typo, and accepting it would let `named_locations
        // = [Haligtree]` look valid while matching a name nobody wrote.
        let unquoted = item.strip_prefix('"')?.strip_suffix('"')?;
        out.push(unquoted.to_owned());
    }
    Some(out)
}

fn parse_int_array(value: &str) -> Option<Vec<PlaceNameTextId>> {
    let body = array_body(value)?;
    if body.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        out.push(item.parse::<PlaceNameTextId>().ok()?);
    }
    Some(out)
}

/// Block ids, accepted as `0x3c353800` or plain decimal.
///
/// Hex is the form every log line, every RE note and every telemetry document in this repo prints
/// a block id in, so a user copying one out of the log must be able to paste it straight in. The
/// writer below emits hex for the same reason.
fn parse_block_array(value: &str) -> Option<Vec<u32>> {
    let body = array_body(value)?;
    if body.trim().is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    for item in body.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        let parsed = item.strip_prefix("0x").map_or_else(
            || item.parse::<u32>().ok(),
            |hex| u32::from_str_radix(hex, 16).ok(),
        )?;
        out.push(parsed);
    }
    Some(out)
}

/// Render a config back to TOML.
///
/// This exists because the in-game keys edit the config, and an edit that only lived in memory
/// would evaporate on the next reload -- the file is the source of truth, so a mark has to be
/// written into it. The output is the documented default file with the four value lines replaced,
/// so a user who marks a place still gets the comments explaining what they just did.
#[must_use]
pub fn render_local_invasion_config(config: &LocalInvasionConfig) -> String {
    let mut out = String::new();
    for line in DEFAULT_CONFIG_TOML.lines() {
        let key = line.split_once('=').map(|(k, _)| k.trim()).unwrap_or("");
        match key {
            "enabled" => out.push_str(&format!("enabled = {}\n", config.enabled)),
            "hunt" => out.push_str(&format!("hunt = {}\n", config.hunt)),
            // THESE TWO WERE MISSING, AND THE DEFAULT ARM BELOW COPIES THE SHIPPED FILE'S LINE
            // VERBATIM -- so every write silently reset them to `false`, on disk AND in memory
            // (`save` adopts the re-parsed round-trip). Marking a location with Insert was enough
            // to switch the player's own banner off and drop them out of the DLL-users pool, with
            // nothing said. Any key the writer does not name is a key the writer destroys.
            "reject_notice" => {
                out.push_str(&format!("reject_notice = {}\n", config.reject_notice));
            }
            "dll_users_only" => {
                out.push_str(&format!("dll_users_only = {}\n", config.dll_users_only));
            }
            "mode" => out.push_str(&format!("mode = \"{}\"\n", config.mode.as_str())),
            "named_locations" => {
                let names = config
                    .named_locations
                    .iter()
                    // A quote inside a name would produce a file this parser cannot read back, so
                    // drop it rather than write a config that breaks on the next reload.
                    .map(|name| format!("\"{}\"", name.replace('"', "")))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("named_locations = [{names}]\n"));
            }
            "mark_key" => out.push_str(&format!(
                "mark_key = \"{}\"\n",
                crate::keybind::key_name(config.mark_key)
            )),
            "unmark_key" => out.push_str(&format!(
                "unmark_key = \"{}\"\n",
                crate::keybind::key_name(config.unmark_key)
            )),
            "named_location_text_ids" => {
                let ids = config
                    .named_location_text_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("named_location_text_ids = [{ids}]\n"));
            }
            "blocked_blocks" => {
                let blocks = config
                    .blocked_blocks
                    .iter()
                    .map(|block| format!("{block:#010x}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("blocked_blocks = [{blocks}]\n"));
            }
            "allowed_blocks" => {
                let blocks = config
                    .allowed_blocks
                    .iter()
                    .map(|block| format!("{block:#010x}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!("allowed_blocks = [{blocks}]\n"));
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out
}

/// Tracks the config file so edits during play take effect on the next match.
///
/// Holds the last modification time and reloads when it changes. `reload_if_changed` returns
/// `Some` only when a reload actually happened, so the caller can log the transition once instead
/// of every time it asks.
#[derive(Debug, Default)]
pub struct HotConfig {
    config: LocalInvasionConfig,
    /// Last mtime seen, as whatever the platform reports. `None` until the file is first read, and
    /// also when the file is missing -- so a file that appears mid-session is picked up.
    last_stamp: Option<std::time::SystemTime>,
    loaded_once: bool,
}

/// What a reload produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadOutcome {
    /// The configuration now in force.
    pub config: LocalInvasionConfig,
    /// Issues from this parse.
    pub issues: Vec<ConfigIssue>,
    /// True when the file went missing and defaults were restored.
    pub reverted_to_defaults: bool,
}

impl HotConfig {
    /// The configuration currently in force.
    #[must_use]
    pub fn current(&self) -> &LocalInvasionConfig {
        &self.config
    }

    /// Write a config to disk and adopt it, returning whether it read back identically.
    ///
    /// This is what the in-game mark keys call. It re-reads and re-parses what it just wrote
    /// instead of trusting the write, because the failure it guards against is silent: a renderer
    /// that emits something the parser drops would lose the user's mark while the key press looked
    /// like it worked. A `false` return means exactly that happened and is worth logging loudly.
    ///
    /// The mtime stamp is updated from the file we just wrote, so our own write is not re-reported
    /// as somebody editing the file.
    pub fn save(
        &mut self,
        path: &std::path::Path,
        config: &LocalInvasionConfig,
    ) -> std::io::Result<bool> {
        std::fs::write(path, render_local_invasion_config(config))?;
        self.last_stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        self.loaded_once = true;
        let round_tripped = parse_local_invasion_config(&std::fs::read_to_string(path)?).config;
        let matched = round_tripped == *config;
        self.config = round_tripped;
        Ok(matched)
    }

    /// Re-read the file if its mtime moved since the last look.
    ///
    /// Returns `Some` only on an actual change. A missing file reverts to defaults -- which means
    /// the filter switches OFF, because `enabled` defaults to false. Deleting the config is
    /// therefore a safe way to stop filtering mid-session, and it fails in the direction that
    /// stops cancelling other people's matches.
    pub fn reload_if_changed(&mut self, path: &std::path::Path) -> Option<ReloadOutcome> {
        let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if self.loaded_once && stamp == self.last_stamp {
            return None;
        }
        self.last_stamp = stamp;
        self.loaded_once = true;
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let parsed = parse_local_invasion_config(&text);
                self.config = parsed.config.clone();
                Some(ReloadOutcome {
                    config: parsed.config,
                    issues: parsed.issues,
                    reverted_to_defaults: false,
                })
            }
            Err(_) => {
                self.config = LocalInvasionConfig::default();
                Some(ReloadOutcome {
                    config: LocalInvasionConfig::default(),
                    issues: Vec::new(),
                    reverted_to_defaults: true,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_invasion::{InvasionAnchor, InvasionCandidate, KeepReason, Verdict};

    #[test]
    fn a_marked_config_round_trips_through_the_writer() {
        // The mark keys rewrite the file; if the writer emitted anything the parser drops, a mark
        // would vanish on the next reload while the key press looked like it worked.
        let mut config = LocalInvasionConfig {
            enabled: true,
            mode: crate::local_invasion::LocalInvasionMode::PreferExactThenArea,
            named_locations: vec!["Miquella's Haligtree".to_owned()],
            ..Default::default()
        };
        config.mark_block(0x3c35_3800);
        config.mark_block(0x0f00_0000);
        config
            .named_location_text_ids
            .extend([1_400_100, 1_400_200]);
        let rendered = render_local_invasion_config(&config);
        let parsed = parse_local_invasion_config(&rendered);
        assert_eq!(parsed.issues, Vec::new(), "our own output must parse clean");
        assert_eq!(parsed.config, config);
        // And the comments survive, so a user who pressed Insert still has the file that explains
        // what the key did.
        assert!(rendered.contains("Shift+Insert"), "{rendered}");
        assert!(
            rendered.contains("allowed_blocks = [0x0f000000, 0x3c353800]"),
            "{rendered}"
        );
    }

    #[test]
    fn every_boolean_survives_the_writer_including_the_two_it_used_to_eat() {
        // THE BUG THIS PINS. `reject_notice` and `dll_users_only` had no arm in the writer, so the
        // default arm copied the shipped file's `= false` over them. Pressing Insert to mark a
        // location therefore switched the player's banner off and dropped them out of the DLL-users
        // pool, on disk and in memory, silently. The old round-trip test could not see it because
        // its fixture left both at their `false` defaults -- a fixture that only exercises defaults
        // cannot detect a writer that emits defaults.
        let config = LocalInvasionConfig {
            enabled: true,
            hunt: true,
            reject_notice: true,
            dll_users_only: true,
            ..Default::default()
        };
        let rendered = render_local_invasion_config(&config);
        let parsed = parse_local_invasion_config(&rendered);
        assert_eq!(parsed.issues, Vec::new(), "our own output must parse clean");
        assert!(
            parsed.config.reject_notice,
            "reject_notice was eaten: {rendered}"
        );
        assert!(
            parsed.config.dll_users_only,
            "dll_users_only was eaten: {rendered}"
        );
        assert_eq!(parsed.config, config);
    }

    #[test]
    fn the_writer_names_every_key_the_shipped_file_declares() {
        // Structural, so the NEXT field added cannot repeat this. Any assignment in the shipped
        // default file that the writer does not match on is a setting the writer overwrites with
        // the shipped literal the moment anything calls it.
        let source = include_str!("local_invasion_config.rs");
        let writer = source
            .split_once("fn render_local_invasion_config")
            .expect("the writer is in this file")
            .1;
        let writer = writer.split_once("\n}").expect("the writer ends").0;
        for line in DEFAULT_CONFIG_TOML.lines() {
            let Some((key, _)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() || key.starts_with('#') || key.starts_with('[') {
                continue;
            }
            assert!(
                writer.contains(&format!("\"{key}\" =>")),
                "the shipped config declares `{key}` but the writer has no arm for it, so every \
                 write resets it to the shipped default"
            );
        }
    }

    #[test]
    fn block_ids_read_back_in_hex_or_decimal() {
        let parsed = parse_local_invasion_config(
            "[local_invasion]\nallowed_blocks = [0x3c353800, 251658240]\n",
        );
        assert_eq!(parsed.issues, Vec::new());
        assert_eq!(
            parsed.config.allowed_blocks,
            [0x3c35_3800, 0x0f00_0000].into_iter().collect()
        );
    }

    #[test]
    fn a_marked_block_survives_a_save_and_reload_cycle() {
        let dir = std::env::temp_dir().join("er-invasion-warp-config-save-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("er-invasion-warp.toml");
        let _ = std::fs::remove_file(&path);
        let mut hot = HotConfig::default();
        let mut config = LocalInvasionConfig {
            enabled: true,
            ..Default::default()
        };
        config.mark_block(0x3c35_3800);
        assert!(hot.save(&path, &config).expect("write"), "round trip");
        assert_eq!(
            hot.current().allowed_blocks,
            [0x3c35_3800].into_iter().collect()
        );
        // Our own write must not read as somebody else editing the file.
        assert_eq!(hot.reload_if_changed(&path), None);
        let _ = std::fs::remove_file(&path);
    }

    /// A 60% keyboard names a key it has; that must survive the writer, since the in-game keys
    /// rewrite this file and would otherwise erase the player's choice on the first mark.
    #[test]
    fn a_renamed_key_survives_a_write_and_reparse() {
        let config = LocalInvasionConfig {
            mark_key: crate::keybind::parse_key("F7").expect("F7 is a key"),
            unmark_key: crate::keybind::parse_key("]").expect("] is a key"),
            ..Default::default()
        };
        let rendered = render_local_invasion_config(&config);
        assert!(rendered.contains("mark_key = \"F7\""), "{rendered}");
        // Rendered as the SYMBOL: it is the first name in the table and is what a player typing a
        // config sees printed on the key itself.
        assert!(rendered.contains("unmark_key = \"]\""), "{rendered}");
        let parsed = parse_local_invasion_config(&rendered);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert_eq!(parsed.config.mark_key, config.mark_key);
        assert_eq!(parsed.config.unmark_key, config.unmark_key);
    }

    /// A typo must be REPORTED and must leave the previous key in force -- never silently swallowed
    /// and never a crash.
    #[test]
    fn an_unknown_key_name_is_reported_and_leaves_the_default_in_force() {
        let parsed = parse_local_invasion_config(
            "[local_invasion]\nmark_key = \"Winkey\"\nunmark_key = \"Delete\"\n",
        );
        assert_eq!(parsed.config.mark_key, crate::keybind::VK_INSERT);
        assert_eq!(parsed.config.unmark_key, crate::keybind::VK_DELETE);
        assert_eq!(parsed.issues.len(), 1, "{:?}", parsed.issues);
        assert!(
            parsed.issues[0].message.contains("Winkey"),
            "{:?}",
            parsed.issues
        );
    }

    /// The shipped file must name the defaults, or the writer has no line to replace and a marked
    /// location would silently drop the key settings.
    #[test]
    fn the_shipped_file_names_both_keys() {
        let parsed = parse_local_invasion_config(DEFAULT_CONFIG_TOML);
        assert!(parsed.issues.is_empty(), "{:?}", parsed.issues);
        assert_eq!(parsed.config.mark_key, crate::keybind::VK_INSERT);
        assert_eq!(parsed.config.unmark_key, crate::keybind::VK_DELETE);
        assert!(
            DEFAULT_CONFIG_TOML.contains("mark_key = "),
            "no mark_key line to rewrite"
        );
        assert!(
            DEFAULT_CONFIG_TOML.contains("unmark_key = "),
            "no unmark_key line to rewrite"
        );
    }

    #[test]
    fn the_shipped_default_file_parses_and_is_off() {
        let parsed = parse_local_invasion_config(DEFAULT_CONFIG_TOML);
        assert_eq!(
            parsed.issues,
            Vec::new(),
            "the file we generate must not warn about itself"
        );
        assert!(!parsed.config.enabled);
        assert_eq!(parsed.config.mode, LocalInvasionMode::ExactOnly);
    }

    #[test]
    fn parses_a_full_config() {
        let parsed = parse_local_invasion_config(
            r#"
            [local_invasion]
            enabled = true
            mode = "area"
            named_locations = ["Miquella's Haligtree", "Leyndell"]
            named_location_text_ids = [1000, 2000]
            "#,
        );
        assert_eq!(parsed.issues, Vec::new());
        assert!(parsed.config.enabled);
        assert_eq!(parsed.config.mode, LocalInvasionMode::PreferExactThenArea);
        assert_eq!(parsed.config.named_locations.len(), 2);
        assert!(parsed.config.named_location_text_ids.contains(&2000));
    }

    #[test]
    fn another_crates_section_in_a_shared_toml_is_skipped_not_warned_about() {
        let parsed = parse_local_invasion_config(
            r#"
            [some_other_crate]
            unrelated_key = 42
            another = "value"

            [local_invasion]
            enabled = true
            "#,
        );
        assert_eq!(
            parsed.issues,
            Vec::new(),
            "sharing a TOML must not make a correct config look broken"
        );
        assert!(parsed.config.enabled);
    }

    #[test]
    fn a_bad_value_is_reported_and_the_rest_still_applies() {
        let parsed = parse_local_invasion_config(
            r#"
            enabled = true
            mode = "somewhere-nice"
            "#,
        );
        assert!(parsed.config.enabled, "the good key still took effect");
        assert_eq!(parsed.config.mode, LocalInvasionMode::ExactOnly);
        assert_eq!(parsed.issues.len(), 1);
        assert!(parsed.issues[0].message.contains("mode must be"));
    }

    #[test]
    fn an_unknown_key_is_surfaced_rather_than_silently_dropped() {
        let parsed = parse_local_invasion_config("enabbled = true\n");
        assert!(!parsed.config.enabled);
        assert_eq!(parsed.issues.len(), 1);
        assert!(parsed.issues[0].message.contains("unknown key"));
    }

    #[test]
    fn a_bare_word_in_named_locations_is_a_typo_not_a_name() {
        let parsed = parse_local_invasion_config("named_locations = [Haligtree]\n");
        assert_eq!(parsed.issues.len(), 1);
        assert!(parsed.config.named_locations.is_empty());
    }

    #[test]
    fn a_hash_inside_a_quoted_name_is_not_a_comment() {
        let parsed = parse_local_invasion_config(r#"named_locations = ["Grace #2"]"#);
        assert_eq!(parsed.issues, Vec::new());
        assert_eq!(parsed.config.named_locations, vec!["Grace #2".to_owned()]);
    }

    #[test]
    fn hot_reload_picks_up_an_edit_and_a_deletion() {
        let dir = std::env::temp_dir().join(format!("er-invasion-warp-cfg-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("er-invasion-warp.toml");
        std::fs::write(&path, "enabled = false\nmode = \"exact\"\n").unwrap();

        let mut hot = HotConfig::default();
        let first = hot
            .reload_if_changed(&path)
            .expect("first look always loads");
        assert!(!first.config.enabled);
        assert!(
            hot.reload_if_changed(&path).is_none(),
            "an unchanged file must not report a reload"
        );

        // Edit it the way a user would, mid-session.
        std::fs::write(&path, "enabled = true\nmode = \"area\"\n").unwrap();
        // mtime granularity can be coarse; force a distinct stamp rather than sleeping.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        let _ = filetime_set(&path, later);
        let second = hot
            .reload_if_changed(&path)
            .expect("an edited file must be picked up");
        assert!(second.config.enabled);
        assert_eq!(second.config.mode, LocalInvasionMode::PreferExactThenArea);
        assert!(hot.current().enabled);

        // Deleting the file must switch the filter OFF, not leave the last config latched.
        std::fs::remove_file(&path).unwrap();
        let third = hot
            .reload_if_changed(&path)
            .expect("a deleted file is a change");
        assert!(third.reverted_to_defaults);
        assert!(
            !hot.current().enabled,
            "losing the config must stop cancelling matches, not keep filtering blind"
        );
        let anchor = InvasionAnchor::new(0x0f00_0000, [100]);
        assert_eq!(
            hot.current()
                .judge(&anchor, &InvasionCandidate::named(0x3c35_3800, 999)),
            Verdict::Keep(KeepReason::FilterDisabled)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Set an mtime without pulling in a dependency. Falls back to a rewrite when the platform
    /// call is unavailable; the test only needs the stamp to CHANGE.
    fn filetime_set(path: &std::path::Path, _when: std::time::SystemTime) -> std::io::Result<()> {
        let text = std::fs::read_to_string(path)?;
        std::fs::write(path, format!("{text}\n"))?;
        Ok(())
    }
}
