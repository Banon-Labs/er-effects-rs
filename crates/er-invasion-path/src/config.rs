//! `er-invasion-path.toml`, re-read while the game runs.
//!
//! Hand-parsed rather than pulled through a TOML crate, matching every other standalone shell in
//! this workspace: a handful of scalars does not justify a dependency in a DLL that is
//! cross-compiled into the game process.
//!
//! The file is live. Editing it -- the toggle key most of all -- takes effect within about a
//! second, without restarting the game, which is the difference between finding a free key in one
//! sitting and finding one across four launches.

// Windows-only in practice; kept portable so the parser below is covered by `cargo test` on the
// host, where the windows-gated modules that consume it are compiled out.
#![cfg_attr(not(windows), allow(dead_code))]

use std::{
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use er_invasion_warp_core::keybind::{VirtualKey, key_name, parse_key};

use crate::{geometry, log::path_log};

const CONFIG_FILE_NAME: &str = "er-invasion-path.toml";

/// Default toggle key.
///
/// It was `F7`, chosen because Elden Ring itself binds nothing to it. That reasoning was
/// incomplete and the omission was caught live on 2026-08-25: `er-invasion-warp` polls `VK_F7`
/// every frame in the same process, so in a profile holding both, pressing the key warped the
/// player instead of drawing anything -- and nothing anywhere said why. The question is never
/// "does the GAME use this key", it is "does any DLL sharing this process use it", and the mods
/// loaded beside you are not something a default can know.
///
/// `;` is free of every binding this workspace's shells poll (`F1`, `F7`-`F9`, `Insert`,
/// `Delete`, `]`, numpad-multiply, the arrows, `ctrl+alt+c`), exists on every layout including
/// 60%, and is not a key anyone hits by accident. That makes it a better default, not a safe
/// one -- a third-party mod can still claim it, which is why the durable answer is a detector
/// that reports collisions at runtime rather than a cleverer guess here.
pub(crate) const DEFAULT_TOGGLE_KEY_NAME: &str = "semicolon";

/// Below this separation, in metres, a target gets no path at all.
///
/// `30.0` is the game's own number: `MenuCommonParam` row 0's `compassEnemyHostInnerDistance` at
/// `+0xa4`, the distance inside which an invader's compass marker for the host disappears. The
/// engine squares it at runtime in `FUN_140775f30`.
///
/// Two deliberate departures from that rule:
///
/// * **This compares in 3D; the compass compares in 2D.** Its `dx`/`dy` are map-plane components
///   with no vertical term, so a player five metres away and forty metres straight down reads as
///   five metres and loses their marker. That is exactly when a route is worth drawing.
/// * **No line-of-sight test.** The compass path has none, and the one this crate used was its
///   own stricter invention.
pub(crate) const DEFAULT_NEAR_SUPPRESS_METERS: f32 = 30.0;

/// How many remote players get a path. Six covers a full Seamless session's worth of phantoms;
/// past that the screen is more line than game.
pub(crate) const DEFAULT_MAX_TARGETS: usize = 6;

/// Length of the "no walkable route" arrow, in metres of world space.
pub(crate) const DEFAULT_ARROW_METERS: f32 = 3.0;

/// Metres between markers along a route.
pub(crate) const DEFAULT_MARKER_SPACING_METERS: f32 = 2.7;

/// Most markers a single route's trail may hold.
pub(crate) const DEFAULT_MAX_MARKERS: usize = 144;

/// The value `params+0x20` takes for "no range limit" -- the engine's own initialiser writes
/// `0xbf800000`, which is `-1.0f`.
pub(crate) const UNLIMITED_SEARCH_RANGE: f32 = -1.0;

/// `params+0x24`: iterations the engine may spend. The engine's own default.
pub(crate) const DEFAULT_SEARCH_BUDGET: i32 = 100_000;

/// Metres of already-walked trail kept behind you before the stones are torn down.
///
/// Not zero: a trail that ends exactly at your feet reads as broken rather than as followed.
pub(crate) const DEFAULT_MARKER_KEEP_BEHIND_METERS: f32 = 12.0;

/// Markers spawned per roster pass, i.e. per ~sixth of a second.
///
/// This is the "small delay as it goes". Three per pass lays a full 48-marker trail over about
/// two and a half seconds, from your feet outwards -- fast enough to read as a trail appearing,
/// slow enough that a route to somebody who is moving stops being laid long before it reaches
/// where they used to be.
pub(crate) const DEFAULT_MARKERS_PER_PASS: usize = 3;

/// Item whose use toggles the overlay, when `trigger_item_id` is set.
///
/// Zero means "no item trigger, hotkey only". There is no default item: silently binding a real
/// consumable would make that item behave differently for anyone who installed the DLL without
/// reading this file.
pub(crate) const DEFAULT_TRIGGER_ITEM_ID: i32 = 0;

const DEFAULT_CONFIG_TOML: &str = r#"# er-invasion-path standalone DLL configuration.
#
# Draws a walkable route from you to every other player in the session, on the ground, following
# the terrain. The route is the engine's own Havok-AI navmesh path -- the same graph the game's
# characters walk -- so it goes round cliffs and through doorways rather than through them.
#
# One path per player, each its own colour. The closer a player is, the bolder their path.
# A player the navmesh cannot reach from where you stand gets a glowing arrow out of your body
# pointing at them instead of a path.

# Key that toggles the overlay on and off. A NAME, not a number: ";", "]", "KP_Plus", "Insert".
# A raw virtual-key code such as 0xba is accepted too.
#
# Pick one no OTHER mod in your profile already reads. The default was F7 until a live run found
# er-invasion-warp polling F7 every frame -- the key warped the player instead of drawing a path,
# and nothing warned about it. The game's own bindings are not the only thing to avoid.
toggle_key = "semicolon"

# Item whose use ALSO toggles the overlay, by EquipParamGoods row id. 0 disables the item trigger
# and leaves the key as the only way in. Example: 2110 is Furlcalling Finger Remedy.
trigger_item_id = 0

# Players closer than this many metres get no path drawn. 30 is the game's own figure -- it is
# MenuCommonParam's compassEnemyHostInnerDistance, the range inside which an invader's compass
# marker for the host disappears.
#
# Measured in 3D, unlike the compass, which compares on the map plane only: someone five metres
# away and forty metres below you reads as five metres to the compass and loses their marker,
# which is the exact case where the walk down is the thing you needed drawing.
near_suppress_meters = 30.0

# A path at or inside this distance is drawn at full width and opacity.
bold_at_meters = 20.0
# A path at this distance is drawn at its faintest. Further than this stays at that faintest
# weight rather than vanishing, so "far away" and "no route" never look the same.
faint_at_meters = 150.0

# Most players to draw at once.
max_targets = 6

# Length in metres of the arrow drawn when no walkable route exists.
arrow_meters = 3.0

# Start with the overlay already on, instead of waiting for the first toggle.
start_enabled = false

# ---------------------------------------------------------------------------
# World markers: the game's OWN effects, spawned along the route on the ground.
#
# The imgui line above is drawn over the finished frame, so it does not go behind a hill and it
# never looks like part of the game. These are real engine effects: correct depth, correct light.
#
# 0 = off, and off is the default. This is the ONLY setting here that changes the game rather
# than just reading it, and it is unknown whether other players in a Seamless session can see
# the effects you spawn -- if they can, a trail pointing at an invader points back at you.
#
# Rainbow Stone effect ids, if you want a trail of the coloured stones:
#   302022  lingering coloured stone   <- the one you want; it stays on the ground
#   302020  held in hand
#   302021  projectile in flight
#   302023  burst on impact            (the last three are momentary: they flash and vanish)
#
# This file is re-read about once a second, so you can change the id with the game running and
# watch the difference immediately.
marker_fxr_id = 0

# One effect per player instead of one for everybody, assigned in the order players are tracked so
# a given player keeps the same stones for as long as they are in your session. Overrides
# marker_fxr_id when present. An FXR carries no tint -- SpawnFfxInstance's spare arguments feed
# time-of-day and weather, not colour -- so telling two players apart means two different effects,
# not two shades of one.
#
# EVERY id must be an effect that LINGERS. 302020 (held), 302021 (projectile) and 302023 (burst)
# are momentary Rainbow Stone stages: they flash once and vanish, so a player assigned one gets a
# trail that is not there -- which looks like the colours changing rather than like a broken
# marker. 302022, the lingering coloured stone, is the only one of the four that marks anything.
#
# THE GAME DOES NOT COLOUR ITS MULTIPLAYER ROLES WITH N EFFECTS. It tints a shared asset through a
# param: RoleParam.roleNameColor has exactly two values across all 49 rows, and the phantom body
# colour is PhantomParam RGB -- gold (200,120,80), red (255,40,40), blue (112,118,255). The Hunter
# is blue and still uses the GOLD sign effect. Looking for "N coloured role effects" finds nothing
# because there are none.
#
# What does exist, in rough order of how likely each is to work as a ground marker:
#
#   PROVEN free-standing and lingering, but only two colours. The whole visible summon sign IS the
#   effect -- CreateSignSfx builds a world matrix and spawns once, no mesh, no bone:
#     30001 / 30000 / 30003   gold-white co-op sign
#     30011 / 30010 / 30013   red invader sign
#   (30020..30061 are vow-rank variants pointing at PhantomParam rows that DO NOT EXIST -- DS3-era
#   dead data. Do not ship one untested.)
#
#   Full-body buff auras -- authored to render on a whole body rather than a weapon, so not bound
#   to weapon geometry:
#     301202 green (Greenburst Crystal Tear)   301252 silver (Silver-Pickled Fowl Foot)
#     301262 gold (Gold-Pickled Fowl Foot)     302152 Baldachin's Blessing
#
#   Weapon-grease enchant auras -- thirteen element colours, all looping by definition
#   (SpEffectVfxParam.midstSfxId) and naming no dummypoly. The catch: they are authored to coat a
#   WEAPON MESH, so whether they render at a bare world transform is unproven:
#     303001 fire     303011 lightning  303061 poison   303071 magic    303091 bleed
#     303101 sleep    303111 frost      303131 holy     303191 rot
#
# The log says which of these the engine even accepts: a spawn logs `bound=false` when the id is
# rejected or not resident, which is a different failure from an effect that spawns and is
# invisible. Sweep candidates and read the log rather than squinting at the ground.
#
# CANDIDATES FROM FILE SHAPE, untested on screen but promising on disk. 302464,
# 302465 and 302466 are 71056 / 71040 / 71056 bytes -- three near-identical files sharing 302022's
# s84005 resource family, and 302464 vs 302466 differ only at the id and then on a regular 64-byte
# stride. That is what a colour-table swap looks like: one effect authored in several colours.
# They are in sfxbnd_commoneffects, so they are already resident and spawnable with no patching.
#
# 302022 is listed FIRST on purpose: it is the one known to linger as a ground stone, so the first
# player always gets a working trail even if the other three turn out to be something else.
#
#   marker_fxr_ids = 302022, 302464, 302465, 302466

# The three spare arguments the spawn passes to the engine's effect-parameter builder. -1 is
# "unset", which is what the game itself passes for a one-shot. NOTHING here is known to change an
# effect's appearance -- they are exposed because they are the only per-instance inputs the spawn
# has, so they are the place to look for a solid colour instead of the Rainbow Stone's own cycling.
# Sweep them one at a time; the file is re-read every second.
marker_variant_a = -1
marker_variant_b = -1
marker_variant_c = -1

# Metres between markers along the route. Markers are spaced evenly along the PATH, not placed at
# navmesh corners, or a doorway would get six of them and open ground none.
marker_spacing_meters = 2.7

# Most markers one route's trail may hold.
max_markers = 144

# Metres of already-walked trail kept behind you before those stones are torn down. The markers
# you have passed are clutter; a few are kept so the trail does not appear to end at your feet.
marker_keep_behind_meters = 12.0

# Markers placed per pass (about six passes a second). The trail is laid from your feet outwards
# a few at a time rather than all at once, and laying STOPS the moment the route changes -- the
# far end of a route to somebody who is moving was never going to be where they are by the time
# you got there. Raise it for a trail that appears faster, lower it for one that creeps.
markers_per_pass = 3

# ---------------------------------------------------------------------------
# How hard the engine looks for a route. Both of these WERE copied from CS::CSAiFunc -- an NPC
# working out how to walk at something it can already see -- and that was the wrong place to copy
# from. A downward corkscrew has an enormous path length inside a tiny footprint, so an NPC-sized
# iteration budget is spent going round and round before it reaches the bottom, and the search
# reports "no route", which looks exactly like there not being one.
#
# These are the engine's OWN defaults now.

# Metres the search may range. 0 or negative = unlimited, which is what the engine itself sets.
search_range_meters = 0.0

# Iterations the search may spend. The engine's own default is 100000; CSAiFunc uses 800.
search_budget = 100000
"#;

impl PathConfig {
    /// The effect for a target holding palette slot `slot`, or `None` when markers are off.
    ///
    /// Wraps rather than running out: with more players than effects two of them share a look,
    /// which is worse than telling them apart and far better than one of them getting no trail.
    pub(crate) fn marker_fxr_for(&self, slot: usize) -> Option<u32> {
        if self.marker_fxr_ids.is_empty() {
            return None;
        }
        Some(self.marker_fxr_ids[slot % self.marker_fxr_ids.len()])
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PathConfig {
    pub(crate) config_path: PathBuf,
    pub(crate) toggle_key: VirtualKey,
    pub(crate) toggle_key_text: String,
    pub(crate) trigger_item_id: i32,
    pub(crate) near_suppress_meters: f32,
    pub(crate) bold_at_meters: f32,
    pub(crate) faint_at_meters: f32,
    pub(crate) max_targets: usize,
    pub(crate) arrow_meters: f32,
    pub(crate) start_enabled: bool,
    pub(crate) marker_spacing_meters: f32,
    pub(crate) max_markers: usize,
    pub(crate) markers_per_pass: usize,
    pub(crate) search_range_meters: f32,
    pub(crate) search_budget: i32,
    pub(crate) marker_keep_behind_meters: f32,
    /// Effects by palette slot. Empty means markers are off.
    pub(crate) marker_fxr_ids: Vec<u32>,
    /// The three spare spawn arguments, a lead on effect variants. See `sfx::SpawnVariant`.
    pub(crate) marker_variant: (i16, i16, i32),
}

/// The parsed config, plus the exact text it was parsed from.
///
/// The text is kept so a reload can decide whether anything actually changed by comparing
/// content rather than by trusting a timestamp. `mtime` has one-second granularity on several
/// filesystems, so an edit saved within the same second as the previous read is invisible to it
/// -- and "I changed the key and nothing happened" is exactly the bug this is meant to prevent.
/// The file is a kilobyte; reading it once a second is cheaper than being wrong about it.
struct Loaded {
    config: Arc<PathConfig>,
    text: String,
}

static CONFIG: RwLock<Option<Loaded>> = RwLock::new(None);

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

/// A positive, finite float setting, or the default.
///
/// Rejecting non-finite and negative values here rather than at the draw is what keeps a
/// fat-fingered config from producing a NaN distance that silently suppresses every path.
fn positive_float(text: &str, key: &str, default: f32) -> f32 {
    setting(text, key)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .unwrap_or(default)
}

/// The configured search range, in the form the engine's parameter block wants.
///
/// The engine reads `params+0x20` as a limit where **negative means unlimited**, which is not a
/// number anybody would type on purpose, so the config spells it `0` and this converts. Anything
/// non-finite is a typo, and a typo must not silently become a tighter search than the player
/// asked for -- a search that gives up reports "no route", which reads as "you cannot walk
/// there" rather than as a broken setting.
fn search_range(text: &str) -> f32 {
    match setting(text, "search_range_meters").and_then(|value| value.parse::<f32>().ok()) {
        Some(range) if range.is_finite() && range > 0.0 => range,
        // Both "0" (the documented spelling) and a malformed value land on unlimited.
        _ => UNLIMITED_SEARCH_RANGE,
    }
}

/// The configured iteration budget, never zero or negative.
fn search_budget(text: &str) -> i32 {
    setting(text, "search_budget")
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|budget| *budget > 0)
        .unwrap_or(DEFAULT_SEARCH_BUDGET)
}

/// The per-slot effect list.
///
/// `marker_fxr_ids` wins when present; otherwise the single `marker_fxr_id` is used for every
/// target, which is what a config written before this setting existed will do. Zero anywhere in
/// the list is dropped rather than spawned -- `0` is the "no markers" value and spawning it would
/// ask the engine for effect zero.
fn marker_fxr_ids(text: &str) -> Vec<u32> {
    if let Some(list) = setting(text, "marker_fxr_ids") {
        let ids: Vec<u32> = list
            .split(',')
            .filter_map(|entry| entry.trim().parse::<u32>().ok())
            .filter(|id| *id > 0)
            .collect();
        if !ids.is_empty() {
            return ids;
        }
    }
    match setting(text, "marker_fxr_id").and_then(|value| value.parse::<u32>().ok()) {
        Some(id) if id > 0 => vec![id],
        _ => Vec::new(),
    }
}

fn parse_config(text: &str, path: PathBuf) -> PathConfig {
    let toggle_key_text = setting(text, "toggle_key")
        .filter(|value| !value.is_empty())
        .unwrap_or(DEFAULT_TOGGLE_KEY_NAME)
        .to_owned();
    let toggle_key = match parse_key(&toggle_key_text) {
        Ok(key) => key,
        Err(error) => {
            // Naming the fallback matters: a player who mistyped a key would otherwise press
            // something that does nothing, with no way to tell that from a broken feature.
            let fallback = parse_key(DEFAULT_TOGGLE_KEY_NAME).expect("the built-in default parses");
            path_log(format_args!(
                "config: toggle_key {error}; falling back to {} ({DEFAULT_TOGGLE_KEY_NAME})",
                key_name(fallback)
            ));
            fallback
        }
    };
    let bold_at_meters = positive_float(text, "bold_at_meters", geometry::DEFAULT_BOLD_AT_METERS);
    let mut faint_at_meters =
        positive_float(text, "faint_at_meters", geometry::DEFAULT_FAINT_AT_METERS);
    if faint_at_meters <= bold_at_meters {
        // `boldness` treats an inverted pair as "always faintest", which is a legal but useless
        // overlay. Repairing it here, loudly, beats shipping a config that draws every path at
        // minimum weight and looks like a rendering bug.
        path_log(format_args!(
            "config: faint_at_meters ({faint_at_meters}) must exceed bold_at_meters \
             ({bold_at_meters}); using {}",
            geometry::DEFAULT_FAINT_AT_METERS
        ));
        faint_at_meters = bold_at_meters + geometry::DEFAULT_FAINT_AT_METERS;
    }
    PathConfig {
        config_path: path,
        toggle_key,
        toggle_key_text,
        trigger_item_id: setting(text, "trigger_item_id")
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(DEFAULT_TRIGGER_ITEM_ID),
        near_suppress_meters: positive_float(
            text,
            "near_suppress_meters",
            DEFAULT_NEAR_SUPPRESS_METERS,
        ),
        bold_at_meters,
        faint_at_meters,
        max_targets: setting(text, "max_targets")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|count| *count > 0)
            .unwrap_or(DEFAULT_MAX_TARGETS),
        arrow_meters: positive_float(text, "arrow_meters", DEFAULT_ARROW_METERS),
        start_enabled: setting(text, "start_enabled").is_some_and(|value| value == "true"),
        // A spacing of zero would ask for a marker every zero metres, so it is floored rather
        // than accepted: the resampler refuses it anyway, and a silent no-markers is worse than
        // a sane one.
        marker_spacing_meters: positive_float(
            text,
            "marker_spacing_meters",
            DEFAULT_MARKER_SPACING_METERS,
        )
        .max(0.5),
        max_markers: setting(text, "max_markers")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|count| *count > 0)
            .unwrap_or(DEFAULT_MAX_MARKERS),
        markers_per_pass: setting(text, "markers_per_pass")
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|count| *count > 0)
            .unwrap_or(DEFAULT_MARKERS_PER_PASS),
        search_range_meters: search_range(text),
        search_budget: search_budget(text),
        marker_fxr_ids: marker_fxr_ids(text),
        marker_variant: (
            setting(text, "marker_variant_a")
                .and_then(|v| v.parse::<i16>().ok())
                .unwrap_or(-1),
            setting(text, "marker_variant_b")
                .and_then(|v| v.parse::<i16>().ok())
                .unwrap_or(-1),
            setting(text, "marker_variant_c")
                .and_then(|v| v.parse::<i32>().ok())
                .unwrap_or(-1),
        ),
        marker_keep_behind_meters: positive_float(
            text,
            "marker_keep_behind_meters",
            DEFAULT_MARKER_KEEP_BEHIND_METERS,
        ),
    }
}

/// Read the file, writing the commented default when there is not one yet.
///
/// A file that exists but cannot be read (locked, mid-save by the editor) returns `None` rather
/// than the default text: overwriting a config the player is editing, or silently reverting them
/// to defaults for one unlucky second, are both worse than skipping this reload.
fn read_source(path: &PathBuf) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let _ = fs::write(path, DEFAULT_CONFIG_TOML);
            Some(DEFAULT_CONFIG_TOML.to_owned())
        }
        Err(_) => None,
    }
}

/// Read the config, writing the commented default file when there is not one yet.
pub(crate) fn init_config() -> Arc<PathConfig> {
    config()
}

/// The config as it stands right now.
///
/// Returns an `Arc` rather than a reference because the config is no longer immortal: a frame
/// takes one snapshot and uses it consistently, while a reload on another tick swaps in a new one
/// without disturbing anything already holding the old.
pub(crate) fn config() -> Arc<PathConfig> {
    if let Some(loaded) = CONFIG
        .read()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
    {
        return Arc::clone(&loaded.config);
    }
    let path = config_path();
    let text = read_source(&path).unwrap_or_else(|| DEFAULT_CONFIG_TOML.to_owned());
    let config = Arc::new(parse_config(&text, path));
    let mut slot = CONFIG.write().unwrap_or_else(|error| error.into_inner());
    // Another thread may have won the race; keep whatever is already there so two callers in the
    // same instant cannot end up holding configs that disagree.
    let loaded = slot.get_or_insert(Loaded {
        config: Arc::clone(&config),
        text,
    });
    Arc::clone(&loaded.config)
}

/// What a re-read of the file means, given what was already loaded.
///
/// Split out from [`reload_if_changed`] so it can be tested without a filesystem or a global: the
/// lock handling around it is mechanical, and this is the part that can be wrong.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReloadOutcome {
    /// Byte-identical to what is loaded. The overwhelmingly common case.
    Unchanged,
    /// Something in the file changed. `previous_key_text` is `Some` only when the binding is what
    /// changed, because that is the one change with state attached to it -- the edge detector has
    /// to forget whichever key was held.
    Changed { previous_key_text: Option<String> },
}

fn classify(
    previous_text: Option<&str>,
    previous_key_text: Option<&str>,
    text: &str,
    new_key_text: &str,
) -> ReloadOutcome {
    if previous_text == Some(text) {
        return ReloadOutcome::Unchanged;
    }
    ReloadOutcome::Changed {
        previous_key_text: previous_key_text
            .filter(|previous| *previous != new_key_text)
            .map(str::to_owned),
    }
}

/// What a reload did, so the caller can log it and react to a changed binding.
pub(crate) struct Reloaded {
    pub(crate) config: Arc<PathConfig>,
    /// The key that was bound, when the binding is what changed.
    pub(crate) previous_key_text: Option<String>,
}

/// Re-read the config file and swap it in if its contents changed.
///
/// This is what makes the settings editable while the game is running: change the key in the toml,
/// save, and the next call picks it up. Returns `None` when nothing changed, which is the case on
/// almost every call.
///
/// A file that is unreadable this instant, or whose text is byte-identical to what is already
/// loaded, is not a change. A file whose `toggle_key` is malformed is a change -- to a config that
/// keeps working -- because [`parse_config`] falls back to the built-in default and says so rather
/// than leaving the feature unbindable.
pub(crate) fn reload_if_changed() -> Option<Reloaded> {
    let path = config_path();
    let text = read_source(&path)?;
    // Cheap path first: hold the read lock only long enough to compare, and parse nothing at all
    // when the file has not been touched -- which is every call but the one that matters.
    {
        let slot = CONFIG.read().unwrap_or_else(|error| error.into_inner());
        if slot.as_ref().is_some_and(|loaded| loaded.text == text) {
            return None;
        }
    }
    let config = Arc::new(parse_config(&text, path));
    let mut slot = CONFIG.write().unwrap_or_else(|error| error.into_inner());
    let outcome = classify(
        slot.as_ref().map(|loaded| loaded.text.as_str()),
        slot.as_ref()
            .map(|loaded| loaded.config.toggle_key_text.as_str()),
        &text,
        &config.toggle_key_text,
    );
    let ReloadOutcome::Changed { previous_key_text } = outcome else {
        // Another thread reloaded the identical text between the two locks.
        return None;
    };
    *slot = Some(Loaded {
        config: Arc::clone(&config),
        text,
    });
    Some(Reloaded {
        config,
        previous_key_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FXR spawned at each point along a route when world markers are switched on.
    ///
    /// `302022` is the Rainbow Stone's lingering coloured stone -- the one that stays on the ground
    /// after the throw, which is the whole reason it is the right effect for a trail. Its siblings
    /// are `302020` (held in hand), `302021` (the projectile in flight) and `302023` (the burst on
    /// impact); all three are momentary, so a trail built from them would flash once and vanish.
    ///
    /// Zero means "no world markers", and that is the shipped default. Spawning these is the only
    /// thing this DLL does that changes the game, so it is opt-in rather than opt-out.
    const DEFAULT_MARKER_FXR_ID: u32 = 0;

    /// Effects assigned to targets in palette-slot order, so each player's trail looks different.
    ///
    /// The original ask was N players, N colours -- which the imgui line does by generating a hue per
    /// slot. An FXR is not tintable that way: `SpawnFfxInstance`'s trailing arguments feed an external
    /// parameter table that carries time-of-day and weather, not a colour, so the only honest way to
    /// give two players visibly different stones is two different effects.
    ///
    /// The four Rainbow Stone stages are visually distinct from each other even though they are
    /// stages rather than colours, so they make a usable default set. Any list of ids works.
    const DEFAULT_MARKER_FXR_IDS: [u32; 1] = [302_022];

    /// Rainbow Stone stages that flash and VANISH: held, projectile, burst. Shipping one of these
    /// as a per-player effect gives that player a trail that is not there, which is how it
    /// shipped once already -- the file documented them as momentary and then used three of them
    /// as defaults anyway.
    const MOMENTARY_STAGES: [u32; 3] = [302_020, 302_021, 302_023];

    fn parse(text: &str) -> PathConfig {
        parse_config(text, PathBuf::from("test.toml"))
    }

    /// The common case, and the one that must not churn: the file is re-read every second, and a
    /// reload that re-parses and re-logs on every one of those reads would fill the log with
    /// nothing and reset the edge detector sixty times a minute.
    #[test]
    fn an_untouched_file_is_not_a_reload() {
        assert_eq!(
            classify(
                Some(DEFAULT_CONFIG_TOML),
                Some("semicolon"),
                DEFAULT_CONFIG_TOML,
                "semicolon"
            ),
            ReloadOutcome::Unchanged
        );
    }

    #[test]
    fn a_changed_binding_reports_what_it_was_bound_to_before() {
        let edited = DEFAULT_CONFIG_TOML.replace("semicolon", "F9");
        assert_eq!(
            classify(Some(DEFAULT_CONFIG_TOML), Some("semicolon"), &edited, "F9"),
            ReloadOutcome::Changed {
                previous_key_text: Some("semicolon".to_owned())
            }
        );
    }

    /// Editing some other setting is still a reload -- the new value has to take effect -- but it
    /// is not a binding change, so the edge detector must be left alone.
    #[test]
    fn changing_a_setting_other_than_the_key_is_not_a_binding_change() {
        let edited = DEFAULT_CONFIG_TOML.replace("max_targets = 6", "max_targets = 3");
        assert_ne!(
            edited, DEFAULT_CONFIG_TOML,
            "the fixture must actually differ"
        );
        assert_eq!(
            classify(
                Some(DEFAULT_CONFIG_TOML),
                Some("semicolon"),
                &edited,
                "semicolon"
            ),
            ReloadOutcome::Changed {
                previous_key_text: None
            }
        );
    }

    /// The first read has nothing to compare against and must not be mistaken for "unchanged",
    /// which would leave the config empty.
    #[test]
    fn the_very_first_read_counts_as_a_change() {
        assert_eq!(
            classify(None, None, DEFAULT_CONFIG_TOML, "semicolon"),
            ReloadOutcome::Changed {
                previous_key_text: None
            }
        );
    }

    /// A mistyped key must leave the feature bound to something the player can press, not
    /// unbound. Reloading is when this matters most: the player is editing live, and a typo that
    /// silently killed the toggle would look exactly like the DLL crashing.
    #[test]
    fn a_malformed_key_reloads_to_the_default_rather_than_to_nothing() {
        let edited = DEFAULT_CONFIG_TOML.replace(
            "toggle_key = \"semicolon\"",
            "toggle_key = \"not-a-key-at-all\"",
        );
        assert_ne!(
            edited, DEFAULT_CONFIG_TOML,
            "the fixture must actually differ"
        );
        let parsed = parse(&edited);
        assert_eq!(
            parsed.toggle_key,
            parse_key(DEFAULT_TOGGLE_KEY_NAME).expect("the built-in default parses")
        );
    }

    /// The Rainbow Stone's lingering coloured stone -- the id the generated file tells the
    /// player to try. It lives here rather than beside the other defaults because nothing in the
    /// shipping build consumes it: its whole job is to keep the comment honest.
    const RAINBOW_STONE_LINGERING_FXR_ID: u32 = 302_022;

    /// The bug behind "it fails on downward corkscrews": both limits were copied from
    /// `CS::CSAiFunc`, an NPC walking at something it can already see. A spiral descent has an
    /// enormous path length in a tiny footprint, so an NPC-sized budget is spent before it
    /// reaches the bottom and the search reports no route.
    #[test]
    fn the_shipped_limits_are_the_engines_own_not_the_npc_ones() {
        let parsed = parse(DEFAULT_CONFIG_TOML);
        assert_eq!(parsed.search_budget, DEFAULT_SEARCH_BUDGET);
        assert_eq!(parsed.search_budget, 100_000, "CSAiFunc's 800 is the bug");
        assert_eq!(parsed.search_range_meters, UNLIMITED_SEARCH_RANGE);
    }

    /// `0` is how the config spells "unlimited"; the engine spells it `-1.0`.
    #[test]
    fn zero_range_becomes_the_engines_unlimited_sentinel() {
        let edited =
            DEFAULT_CONFIG_TOML.replace("search_range_meters = 0.0", "search_range_meters = 0");
        assert_eq!(parse(&edited).search_range_meters, UNLIMITED_SEARCH_RANGE);
    }

    #[test]
    fn a_real_range_is_passed_through() {
        let edited =
            DEFAULT_CONFIG_TOML.replace("search_range_meters = 0.0", "search_range_meters = 250.0");
        assert_eq!(parse(&edited).search_range_meters, 250.0);
    }

    /// A broken value must not become a tighter search than was asked for: that would report no
    /// route, which reads as "you cannot walk there" rather than as a typo.
    #[test]
    fn a_broken_limit_falls_back_to_unlimited_never_to_a_smaller_search() {
        for bad in ["nonsense", "-5", "0.0"] {
            let edited = DEFAULT_CONFIG_TOML.replace(
                "search_range_meters = 0.0",
                &format!("search_range_meters = {bad}"),
            );
            assert_eq!(
                parse(&edited).search_range_meters,
                UNLIMITED_SEARCH_RANGE,
                "{bad}"
            );
        }
        for bad in ["nonsense", "0", "-1"] {
            let edited = DEFAULT_CONFIG_TOML
                .replace("search_budget = 100000", &format!("search_budget = {bad}"));
            assert_eq!(parse(&edited).search_budget, DEFAULT_SEARCH_BUDGET, "{bad}");
        }
    }

    /// No default may be a momentary effect. A player assigned one sees no trail at all, and
    /// that reads as the colours changing rather than as a marker that never persisted.
    #[test]
    fn no_default_effect_is_one_of_the_momentary_stages() {
        for id in DEFAULT_MARKER_FXR_IDS {
            assert!(
                !MOMENTARY_STAGES.contains(&id),
                "{id} flashes and vanishes; it cannot mark a trail"
            );
        }
    }

    /// The list the generated file suggests must be the list the code names, or a player pastes
    /// a line that does something other than what the comment beside it promised.
    #[test]
    fn the_shipped_file_quotes_the_effect_list_the_code_names() {
        let suggested = DEFAULT_MARKER_FXR_IDS
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        assert!(
            DEFAULT_CONFIG_TOML.contains(&suggested),
            "the generated config must name {suggested}"
        );
    }

    #[test]
    fn a_single_id_still_applies_to_every_target() {
        let edited = DEFAULT_CONFIG_TOML.replace("marker_fxr_id = 0", "marker_fxr_id = 302022");
        let parsed = parse(&edited);
        assert_eq!(parsed.marker_fxr_for(0), Some(302_022));
        assert_eq!(parsed.marker_fxr_for(3), Some(302_022));
    }

    /// The original ask: N players, N looks -- and a given player keeps theirs.
    #[test]
    fn each_palette_slot_gets_its_own_effect() {
        let edited = DEFAULT_CONFIG_TOML.replace(
            "marker_fxr_id = 0",
            // Momentary ids are fine as a PARSING fixture -- this asserts slot assignment, not
            // that the effects are usable markers.
            "marker_fxr_ids = 302022, 302020, 302021",
        );
        let parsed = parse(&edited);
        assert_eq!(parsed.marker_fxr_for(0), Some(302_022));
        assert_eq!(parsed.marker_fxr_for(1), Some(302_020));
        assert_eq!(parsed.marker_fxr_for(2), Some(302_021));
    }

    /// More players than effects must share a look, never lose a trail.
    #[test]
    fn the_list_wraps_rather_than_running_out() {
        let edited =
            DEFAULT_CONFIG_TOML.replace("marker_fxr_id = 0", "marker_fxr_ids = 302022, 302020");
        let parsed = parse(&edited);
        assert_eq!(parsed.marker_fxr_for(2), parsed.marker_fxr_for(0));
        assert_eq!(parsed.marker_fxr_for(5), parsed.marker_fxr_for(1));
    }

    /// Zero is the "no markers" value; it must never reach the engine as an effect id.
    #[test]
    fn zeroes_are_dropped_from_the_list_rather_than_spawned() {
        let edited =
            DEFAULT_CONFIG_TOML.replace("marker_fxr_id = 0", "marker_fxr_ids = 0, 302022, 0");
        assert_eq!(parse(&edited).marker_fxr_ids, vec![302_022]);
    }

    #[test]
    fn markers_stay_off_when_neither_setting_names_an_effect() {
        assert!(parse(DEFAULT_CONFIG_TOML).marker_fxr_ids.is_empty());
        assert_eq!(parse(DEFAULT_CONFIG_TOML).marker_fxr_for(0), None);
    }

    /// The generated file tells the player which id to try, and a comment that drifts from the
    /// constant is worse than no comment: they would paste a number that spawns nothing and have
    /// no way to tell that from the feature being broken.
    #[test]
    fn the_shipped_file_quotes_the_lingering_stone_id_the_code_names() {
        assert!(
            DEFAULT_CONFIG_TOML.contains(&RAINBOW_STONE_LINGERING_FXR_ID.to_string()),
            "the generated config must name {RAINBOW_STONE_LINGERING_FXR_ID}"
        );
    }

    /// World markers change the game, so they must be off until asked for.
    #[test]
    fn world_markers_are_off_until_the_player_asks_for_them() {
        assert_eq!(DEFAULT_MARKER_FXR_ID, 0);
        assert!(parse(DEFAULT_CONFIG_TOML).marker_fxr_ids.is_empty());
    }

    /// A zero spacing would ask the resampler for a marker every zero metres.
    #[test]
    fn a_zero_spacing_is_floored_rather_than_obeyed() {
        // Keyed off the constant rather than a literal: a fixture that names the old default
        // silently stops testing anything the moment the default moves.
        let edited = DEFAULT_CONFIG_TOML.replace(
            &format!("marker_spacing_meters = {DEFAULT_MARKER_SPACING_METERS}"),
            "marker_spacing_meters = 0.0",
        );
        assert_ne!(
            edited, DEFAULT_CONFIG_TOML,
            "the fixture must actually differ"
        );
        assert!(parse(&edited).marker_spacing_meters >= 0.5);
    }

    #[test]
    fn the_shipped_default_file_parses_to_the_documented_defaults() {
        let parsed = parse(DEFAULT_CONFIG_TOML);
        assert_eq!(parsed.toggle_key_text, DEFAULT_TOGGLE_KEY_NAME);
        assert_eq!(parsed.trigger_item_id, DEFAULT_TRIGGER_ITEM_ID);
        assert_eq!(parsed.near_suppress_meters, DEFAULT_NEAR_SUPPRESS_METERS);
        assert_eq!(parsed.bold_at_meters, geometry::DEFAULT_BOLD_AT_METERS);
        assert_eq!(parsed.faint_at_meters, geometry::DEFAULT_FAINT_AT_METERS);
        assert_eq!(parsed.max_targets, DEFAULT_MAX_TARGETS);
        assert_eq!(parsed.arrow_meters, DEFAULT_ARROW_METERS);
        assert!(!parsed.start_enabled);
    }

    #[test]
    fn the_shipped_defaults_match_the_prose_in_the_shipped_file() {
        // Keyed off the constants, not literals: a fixture naming the old default stops testing
        // anything the moment the default moves, which is how this test survived a 10 -> 30 change
        // by failing rather than by checking.
        assert!(DEFAULT_CONFIG_TOML.contains(&format!(
            "near_suppress_meters = {DEFAULT_NEAR_SUPPRESS_METERS}"
        )));
        assert!(
            DEFAULT_CONFIG_TOML.contains(&format!("toggle_key = \"{DEFAULT_TOGGLE_KEY_NAME}\""))
        );
    }

    #[test]
    fn the_default_key_is_none_of_the_ones_this_workspace_already_polls() {
        // F7 shipped as the default and collided with er-invasion-warp, which polls it every
        // frame -- the key warped the player rather than toggling this overlay. These are the
        // virtual keys the sibling shells read; the default must not be among them.
        const CLAIMED_BY_SIBLING_SHELLS: &[(&str, VirtualKey)] = &[
            ("er-invasion-warp F1", 0x70),
            ("er-invasion-warp F7", 0x76),
            ("er-invasion-warp F8", 0x77),
            ("er-invasion-warp F9", 0x78),
            ("er-invasion-warp Insert", 0x2d),
            ("er-invasion-warp Delete", 0x2e),
            ("er-invasion-warp ]", 0xdd),
            ("er-net-effects numpad *", 0x6a),
        ];
        let default = parse_key(DEFAULT_TOGGLE_KEY_NAME).expect("the default parses");
        for (owner, key) in CLAIMED_BY_SIBLING_SHELLS {
            assert_ne!(
                default, *key,
                "the default toggle key is already read by {owner}"
            );
        }
    }

    #[test]
    fn an_empty_file_still_yields_working_defaults() {
        let parsed = parse("");
        assert_eq!(parsed.toggle_key_text, DEFAULT_TOGGLE_KEY_NAME);
        assert_eq!(parsed.max_targets, DEFAULT_MAX_TARGETS);
        assert!(parsed.faint_at_meters > parsed.bold_at_meters);
    }

    #[test]
    fn settings_override_the_defaults() {
        let parsed = parse(
            "toggle_key = \"]\"\ntrigger_item_id = 2110\nnear_suppress_meters = 4.5\n\
             bold_at_meters = 8\nfaint_at_meters = 60\nmax_targets = 2\narrow_meters = 1.5\n\
             start_enabled = true\n",
        );
        assert_eq!(parsed.toggle_key, parse_key("]").expect("] is a key"));
        assert_eq!(parsed.trigger_item_id, 2110);
        assert_eq!(parsed.near_suppress_meters, 4.5);
        assert_eq!(parsed.bold_at_meters, 8.0);
        assert_eq!(parsed.faint_at_meters, 60.0);
        assert_eq!(parsed.max_targets, 2);
        assert_eq!(parsed.arrow_meters, 1.5);
        assert!(parsed.start_enabled);
    }

    #[test]
    fn an_unparseable_key_falls_back_instead_of_disabling_the_feature() {
        let parsed = parse("toggle_key = \"nonsense\"\n");
        assert_eq!(
            parsed.toggle_key,
            parse_key(DEFAULT_TOGGLE_KEY_NAME).expect("F7 is a key")
        );
    }

    #[test]
    fn a_commented_out_setting_is_not_a_setting() {
        let parsed = parse("# max_targets = 1\nmax_targets = 3 # trailing comment\n");
        assert_eq!(parsed.max_targets, 3);
    }

    #[test]
    fn nonsense_distances_do_not_reach_the_draw() {
        let parsed = parse("near_suppress_meters = -5\nbold_at_meters = nan\nmax_targets = 0\n");
        assert_eq!(parsed.near_suppress_meters, DEFAULT_NEAR_SUPPRESS_METERS);
        assert!(parsed.bold_at_meters.is_finite());
        assert_eq!(parsed.max_targets, DEFAULT_MAX_TARGETS);
    }

    #[test]
    fn an_inverted_distance_pair_is_repaired_rather_than_drawn() {
        let parsed = parse("bold_at_meters = 100\nfaint_at_meters = 10\n");
        assert!(
            parsed.faint_at_meters > parsed.bold_at_meters,
            "inverted thresholds would make every path draw at minimum weight"
        );
        // And the repaired pair must still produce a real ramp rather than a constant.
        let near = geometry::boldness(0.0, parsed.bold_at_meters, parsed.faint_at_meters);
        let far = geometry::boldness(1_000.0, parsed.bold_at_meters, parsed.faint_at_meters);
        assert!(near > far);
    }
}
