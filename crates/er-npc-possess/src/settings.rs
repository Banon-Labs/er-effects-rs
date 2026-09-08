//! The schema: every table in `er-npc-possess.toml` that is not a hotkey.
//!
//! Layer 1 shipped this whole schema before anything read it, on the reasoning that the file a
//! player tunes now has to be the file the later layers read -- a schema that appears a table at a
//! time makes every early config a migration. Layers 2, 3 and 4 have since consumed most of it;
//! what remains unread is named in the shipped file's own comments rather than here, so the two
//! cannot disagree.
//!
//! # The rule every field here follows
//!
//! A value that does not parse keeps the one already in force and says so. Not the built-in
//! default -- that silently drags someone back off a setting they had deliberately moved -- and
//! not zero. It is the same rule `er_hotkey_config::Binding` enforces for keys, applied to the
//! scalars, because a typo in `speed_scale` is exactly as invisible as a typo in `hotkey`.
//!
//! # The one table that is not live
//!
//! [`TargetSettings`] is the whole of `[target]`, and it is deliberately a struct of its own
//! rather than four fields on the config: changing who you are about to possess must not take
//! effect while you are possessing somebody. See `config::PossessConfig::stage_target`.

// Windows-only crate in practice; this module is pure parsing and stays ungated so its tests run
// on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use er_hotkey_config::keys::{Chord, chord_name, parse_chord};
use er_hotkey_config::pad::{PadChord, pad_chord_name, parse_pad_chord};

use crate::toml::Document;

/// Section names, spelled once.
pub(crate) const TARGET_SECTION: &str = "target";
pub(crate) const SPAWN_SECTION: &str = "spawn";
pub(crate) const MAPPING_SECTION: &str = "mapping";
pub(crate) const BUTTONS_SECTION: &str = "buttons";
pub(crate) const MOVEMENT_SECTION: &str = "movement";
pub(crate) const CAMERA_SECTION: &str = "camera";
pub(crate) const HUD_SECTION: &str = "hud";
pub(crate) const PICKER_SECTION: &str = "picker";
/// `[chr.c4500]`, `[chr.c2130]`, ... -- open-ended, one per character id.
pub(crate) const CHR_SECTION_PREFIX: &str = "chr.";

/// Collects `"<section>.<key>" = <value>` for every value that could not be read, so one log line
/// can name all of them and the player can find each in their file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Rejections(Vec<String>);

impl Rejections {
    /// Record one unreadable value. `path` is `<section>.<key>`, or a bare key at the top level.
    pub(crate) fn push(&mut self, path: &str, value: &str) {
        self.0.push(format!("{path}={value:?}"));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn summary(&self) -> String {
        self.0.join(" ")
    }
}

/// Read a value the caller knows how to parse, keeping `current` when it is absent or junk.
fn take<T>(
    current: &mut T,
    doc: &Document,
    section: &str,
    key: &str,
    rejections: &mut Rejections,
    parse: impl FnOnce(&str) -> Option<T>,
) {
    let Some(raw) = doc.scalar(section, key) else {
        return;
    };
    match parse(raw) {
        Some(value) => *current = value,
        None => rejections.push(&qualify(section, key), raw),
    }
}

fn qualify(section: &str, key: &str) -> String {
    if section.is_empty() {
        key.to_owned()
    } else {
        format!("{section}.{key}")
    }
}

/// `true` / `false`, any case. Anything else is a rejection rather than a silent `false`: a
/// misspelled boolean that reads as "off" is indistinguishable from the feature being broken.
pub(crate) fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn parse_f32(raw: &str) -> Option<f32> {
    raw.trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

fn parse_i32(raw: &str) -> Option<i32> {
    raw.trim().parse::<i32>().ok()
}

/// How the DLL decides which character to possess.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TargetMode {
    /// Whatever the player is locked on to.
    #[default]
    LockOn,
    /// The closest character the selection filter accepts.
    Nearest,
    /// Whatever the camera centre is pointed at.
    Crosshair,
    /// The literal [`TargetSettings::chr_id`], ignoring where the player is looking.
    ChrId,
    /// Create the creature `[spawn]` names and possess that, rather than finding one.
    ///
    /// The only mode that puts something in the world rather than borrowing something already
    /// there, and therefore the only one with a teardown that has to remove what it made. See
    /// [`SpawnSettings`] and `crate::spawn`.
    Spawn,
}

impl TargetMode {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "lock_on" | "lockon" => Some(Self::LockOn),
            "nearest" => Some(Self::Nearest),
            "crosshair" => Some(Self::Crosshair),
            "chr_id" | "chrid" => Some(Self::ChrId),
            "spawn" => Some(Self::Spawn),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::LockOn => "lock_on",
            Self::Nearest => "nearest",
            Self::Crosshair => "crosshair",
            Self::ChrId => "chr_id",
            Self::Spawn => "spawn",
        }
    }

    /// Does this mode create a character, and therefore owe the world a despawn?
    pub(crate) const fn creates(self) -> bool {
        matches!(self, Self::Spawn)
    }
}

/// `[target]`. The only table that is not live.
///
/// Every other setting here takes effect on the next reload, roughly a second after the file is
/// saved. This one cannot: it decides who you are, and swapping that out from under an in-flight
/// possession would mean the mapping, the camera and the moveset all belonging to a different
/// character than the body on screen. An edit is staged and adopted at the next possession.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TargetSettings {
    pub(crate) mode: TargetMode,
    /// Used only when `mode == ChrId`. An `NpcParam` row, matched against a loaded character's
    /// `npc_param_id` or `npc_id` -- so `45000000` for a Flying Dragon, not `4500` and not `45000`.
    /// `[spawn].chr_id` is the other kind of number and is deliberately a different field.
    pub(crate) chr_id: i32,
    pub(crate) release_on_death: bool,
    /// `[spawn]`. Carried inside `[target]` rather than beside it, because it decides who you
    /// become and therefore has to be staged with the rest of that decision -- an edit adopted
    /// mid-possession would mean the roster slot being torn down at release was chosen under
    /// different rules than the one that was created.
    pub(crate) spawn: SpawnSettings,
}

impl Default for TargetSettings {
    fn default() -> Self {
        Self {
            mode: TargetMode::LockOn,
            chr_id: 0,
            release_on_death: true,
            spawn: SpawnSettings::default(),
        }
    }
}

/// `[spawn]`. Read with `[target]` and staged with it; see [`TargetSettings::spawn`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SpawnSettings {
    /// The model number: `4500` becomes `c4500`. Used only when `mode = "spawn"`.
    pub(crate) chr_id: u32,
    /// The `NpcParam` row, or `0` to derive `chr_id * 10000` -- the row whose moveset the shipped
    /// table is keyed by, so the easy configuration and the moveset agree.
    pub(crate) npc_param_id: i32,
    /// The `NpcThinkParam` row. Rarely worth setting: the lookup pre-initialises its result and
    /// `LoadWait` treats the resulting NULL `LuaDat` caps as satisfied, so an invalid one is not a
    /// failure.
    pub(crate) npc_think_id: i32,
    /// How far in front of the player it appears, in metres.
    ///
    /// Not zero, and not configurable down to zero: a creature placed exactly on the player is one
    /// the player is standing inside for the frame before possession takes, and a large creature
    /// resolving that overlap can throw the body.
    pub(crate) distance_m: f32,
    /// How long to wait for it to become drivable before giving up and removing it.
    ///
    /// The only thing that ends a bad pick. There is no error edge anywhere in the `ChrRes` or
    /// `EneDat` state machines, so a chr id with no assets waits forever; this is the deadline that
    /// turns that into a message.
    pub(crate) readiness_ms: u32,
    /// Take the creature away again when the possession ends.
    ///
    /// On by default. Leaving it is not free: it is a live NPC that nothing else will ever remove,
    /// and the buddy roster has fourteen slots the game's own spawner shares.
    pub(crate) despawn_on_release: bool,
}

impl Default for SpawnSettings {
    fn default() -> Self {
        Self {
            // A real creature, not zero. `mode = "spawn"` with a default of `c0000` would ask for a
            // model that does not exist, and the game does not fail that request -- it waits, until
            // the deadline removes it. So the default id has to be one that works, and c4500 is in
            // the shipped moveset table, which makes the first press do something rather than
            // demonstrate the timeout.
            chr_id: 4500,
            npc_param_id: 0,
            npc_think_id: 0,
            distance_m: 3.0,
            readiness_ms: 5_000,
            despawn_on_release: true,
        }
    }
}

impl SpawnSettings {
    /// The `NpcParam` row in force: the configured one, or the id-derived default.
    ///
    /// The derivation is [`crate::spawn::request::SpawnSpec::default_npc_param_id`] rather than the
    /// same multiply written again -- the magnitude is the part that is easy to get wrong (c4500 is
    /// row 45,000,000, not 45,000) and it is asserted against the moveset table's own inverse over
    /// there.
    #[must_use]
    pub(crate) const fn resolved_npc_param_id(&self) -> i32 {
        if self.npc_param_id != 0 {
            return self.npc_param_id;
        }
        crate::spawn::request::SpawnSpec::default_npc_param_id(self.chr_id)
    }

    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = SPAWN_SECTION;
        // Bounded at parse time rather than at spawn time, so a five-digit id is reported next to
        // the line it was typed on instead of as a refused hotkey press minutes later.
        // `1..=9999`, bounded at parse time rather than at spawn time so a bad id is reported next
        // to the line it was typed on instead of as a refused hotkey press minutes later. Zero is
        // excluded with the rest: `c0000` is not a creature, and the game answers a request for a
        // model that does not exist by waiting rather than by failing.
        take(&mut self.chr_id, doc, s, "chr_id", rejections, |raw| {
            raw.trim()
                .parse::<u32>()
                .ok()
                .filter(|id| (1..=9999).contains(id))
        });
        take(
            &mut self.npc_param_id,
            doc,
            s,
            "npc_param_id",
            rejections,
            parse_i32,
        );
        take(
            &mut self.npc_think_id,
            doc,
            s,
            "npc_think_id",
            rejections,
            parse_i32,
        );
        // A metre is about the radius of the player's own capsule, and 50 is past anything the
        // camera will follow comfortably. Rejected rather than clamped: 0 would mean "inside me".
        take(
            &mut self.distance_m,
            doc,
            s,
            "distance_m",
            rejections,
            |raw| parse_f32(raw).filter(|v| (1.0..=50.0).contains(v)),
        );
        // A deadline under a second cannot be met by an asset load, and one over a minute is
        // indistinguishable from no deadline at all -- which is the state this setting exists to
        // leave.
        take(
            &mut self.readiness_ms,
            doc,
            s,
            "readiness_ms",
            rejections,
            |raw| {
                raw.trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|ms| (1_000..=60_000).contains(ms))
            },
        );
        take(
            &mut self.despawn_on_release,
            doc,
            s,
            "despawn_on_release",
            rejections,
            parse_bool,
        );
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "chr_id=c{:04} npc_param_id={} npc_think_id={} distance_m={} readiness_ms={} \
             despawn_on_release={}",
            self.chr_id,
            self.resolved_npc_param_id(),
            self.npc_think_id,
            self.distance_m,
            self.readiness_ms,
            self.despawn_on_release
        )
    }
}

impl TargetSettings {
    /// Read `[target]` out of a document. Named `apply_from` rather than `apply` because the
    /// caller does not apply the result to what is in force -- it stages it. See
    /// `crate::config::PossessConfig::adopt_staged_target`.
    pub(crate) fn apply_from(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = TARGET_SECTION;
        take(
            &mut self.mode,
            doc,
            s,
            "mode",
            rejections,
            TargetMode::parse,
        );
        take(&mut self.chr_id, doc, s, "chr_id", rejections, parse_i32);
        take(
            &mut self.release_on_death,
            doc,
            s,
            "release_on_death",
            rejections,
            parse_bool,
        );
        // `[spawn]` is read here, inside `[target]`'s reader, because it is staged with it. A
        // separate live table would let the roster slot be chosen under one set of rules and torn
        // down under another.
        self.spawn.apply(doc, rejections);
    }

    pub(crate) fn summary(&self) -> String {
        let spawn = if self.mode.creates() {
            format!(" spawn[{}]", self.spawn.summary())
        } else {
            String::new()
        };
        format!(
            "mode={} chr_id={} release_on_death={}{spawn}",
            self.mode.name(),
            self.chr_id,
            self.release_on_death
        )
    }
}

/// How a controller input turns into one of the possessed character's attacks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum MappingModel {
    /// Range band plus a combo rank picks the attack. The default, and the only one with a
    /// per-input meaning that survives a character with thirty attacks and four buttons.
    #[default]
    Context,
    /// The range band picks a contiguous third of the bucket's rank order rather than filtering
    /// by reach: close gets the low ranks, far the high ones. More predictable than
    /// [`Self::Context`] and less situationally right.
    Layered,
    /// No range filtering at all -- every press walks the whole bucket. For creatures whose
    /// attacks mostly came back with an unmeasured reach, where filtering removes moves for no
    /// good reason.
    Slots,
}

impl MappingModel {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "context" => Some(Self::Context),
            "layered" => Some(Self::Layered),
            "slots" => Some(Self::Slots),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Layered => "layered",
            Self::Slots => "slots",
        }
    }
}

/// What happens to an input the mapping has nothing for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum UnboundInputs {
    /// Give it the nearest attack in the same bucket. Fewer dead buttons, less predictable.
    #[default]
    Promote,
    /// Leave it dead. Predictable, and the honest choice for a character with few attacks.
    Deny,
}

impl UnboundInputs {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "promote" => Some(Self::Promote),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Promote => "promote",
            Self::Deny => "deny",
        }
    }
}

/// `[mapping]`. Live.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MappingSettings {
    pub(crate) model: MappingModel,
    /// Neutral time after which the attack rank falls back to 0.
    pub(crate) combo_window_ms: u32,
    /// `close < bands_m.0 < mid < bands_m.1 < far`, in metres.
    pub(crate) bands_m: (f32, f32),
    /// Offer the possessed creature's grabs.
    ///
    /// On by default, and this reversed once the corpus was actually measured. The old default
    /// was `false` on the reasoning that a grab is unfair; the reasoning was about the wrong
    /// direction of the interaction (you are the one grabbing) and the scope was wrong too --
    /// grabs are the signature move of most bosses, not a niche category.
    ///
    /// **it now gates 153 real moves across 78 creatures.** It used to gate nothing, and the
    /// reason it did is worth keeping written down, because the mistake was in the model rather
    /// than in the data. A grab is not a 4000-band animation and it is not TimeAct event 304: it
    /// is an ordinary, already-fireable attack whose `AtkParam_Npc` row has `throwTypeId != 0`.
    /// `ApplyDamage` reads that column off the hit that landed and calls
    /// `CSChrThrowModule::InitThrow` before calculating any damage; the throw system then drives
    /// both parties into the 4000-band clips through the bare behaviour names `W_ThrowAtk` and
    /// `W_ThrowDef`. So the old sweep's finding was true and pointed at the wrong half -- no event
    /// name in the 4000 band has a transition behind it because those clips are never addressed by
    /// id at all. Marking those as the grabs is what left this flag matching nothing.
    ///
    /// Every initiator is in the 3000 band, and every one of them was already being offered as a
    /// plain attack, so turning this off now genuinely removes something.
    pub(crate) allow_grabs: bool,
    pub(crate) unbound_inputs: UnboundInputs,
    /// How long a press that could not be honoured yet stays waiting before it is dropped.
    ///
    /// Not [`Self::combo_window_ms`], and the two are next to each other in the config file
    /// precisely because they read alike and mean different things. The combo window is about
    /// which attack a press gives you -- it decides when the rank cursor falls back to the first
    /// move of the bucket. This one is about when a press happens at all: a press that lands
    /// mid-swing is held rather than allowed to cancel the swing, and this is how long it is held
    /// for before the player is assumed to have given up on it. Reusing the combo window for both
    /// would silently redefine a number somebody may have tuned for the other meaning.
    ///
    /// See [`crate::moveset::chain`].
    pub(crate) input_buffer_ms: u32,
    /// How long the possessed creature may animate, go nowhere and be asked for nothing before
    /// the watchdog forces it back to idle. See [`crate::moveset::watchdog`].
    pub(crate) watchdog_seconds: f32,
}

impl Default for MappingSettings {
    fn default() -> Self {
        Self {
            model: MappingModel::Context,
            combo_window_ms: 1200,
            bands_m: (4.0, 12.0),
            allow_grabs: true,
            unbound_inputs: UnboundInputs::Promote,
            // Long enough to bridge the recovery of an ordinary attack, so a press made during
            // one still arrives; short enough that a press you have already forgotten about does
            // not swing at nothing while you walk away.
            input_buffer_ms: 1000,
            watchdog_seconds: 4.0,
        }
    }
}

impl MappingSettings {
    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = MAPPING_SECTION;
        take(
            &mut self.model,
            doc,
            s,
            "model",
            rejections,
            MappingModel::parse,
        );
        take(
            &mut self.combo_window_ms,
            doc,
            s,
            "combo_window_ms",
            rejections,
            |raw| raw.trim().parse::<u32>().ok(),
        );
        take(
            &mut self.input_buffer_ms,
            doc,
            s,
            "input_buffer_ms",
            rejections,
            |raw| raw.trim().parse::<u32>().ok(),
        );
        take(
            &mut self.allow_grabs,
            doc,
            s,
            "allow_grabs",
            rejections,
            parse_bool,
        );
        take(
            &mut self.unbound_inputs,
            doc,
            s,
            "unbound_inputs",
            rejections,
            UnboundInputs::parse,
        );
        // A zero or negative watchdog would mean "force idle immediately", which cancels every
        // attack on the frame it starts. Rejected rather than clamped: somebody who typed 0 meant
        // something, and silently turning it into 4 would hide that they cannot have it.
        take(
            &mut self.watchdog_seconds,
            doc,
            s,
            "watchdog_seconds",
            rejections,
            |raw| parse_f32(raw).filter(|value| *value > 0.0),
        );

        // The bands are one setting in two numbers, so they move together or not at all: half an
        // edit -- a readable near band and a junk far one -- would leave the two inconsistent
        // with each other, which is worse than keeping both.
        if let Some(items) = doc.array(s, "bands_m") {
            match (
                items.len(),
                items.first().copied().and_then(parse_f32),
                items.get(1).copied().and_then(parse_f32),
            ) {
                (2, Some(near), Some(far)) if near > 0.0 && far > near => {
                    self.bands_m = (near, far);
                }
                _ => rejections.push(&qualify(s, "bands_m"), &format!("[{}]", items.join(", "))),
            }
        }
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "model={} combo_window_ms={} input_buffer_ms={} bands_m=[{},{}] allow_grabs={} \
             unbound_inputs={} watchdog_seconds={}",
            self.model.name(),
            self.combo_window_ms,
            self.input_buffer_ms,
            self.bands_m.0,
            self.bands_m.1,
            self.allow_grabs,
            self.unbound_inputs.name(),
            self.watchdog_seconds
        )
    }
}

/// The bucket an input draws its attack from. Identical on every character, which is the point:
/// the same button means the same kind of thing whoever you are wearing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bucket {
    Light,
    Heavy,
    Ranged,
    Movement,
}

impl Bucket {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "heavy" => Some(Self::Heavy),
            "ranged" => Some(Self::Ranged),
            "movement" => Some(Self::Movement),
            _ => None,
        }
    }

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Heavy => "heavy",
            Self::Ranged => "ranged",
            Self::Movement => "movement",
        }
    }
}

/// `[buttons]`. Live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ButtonSettings {
    pub(crate) r1: Bucket,
    pub(crate) r2: Bucket,
    pub(crate) l1: Bucket,
    pub(crate) l2: Bucket,
    /// Page the left hand's two buttons (`l1`/`l2`) onto the next move of their buckets.
    ///
    /// Left arrow by default because that is the key vanilla binds to the left-hand armament
    /// swap: a possessed creature has no armaments and no `PlayerGameData` to swap them in, so
    /// the two swap keys are free, and they are already the gesture a player reaches for when
    /// they want a button to do something else. `None` is unbound, which is a real setting.
    pub(crate) page_left: Option<Chord>,
    /// ...and the right hand's (`r1`/`r2`), on the right arrow.
    pub(crate) page_right: Option<Chord>,
    /// The pad spelling of the same two, defaulting to the d-pad for the same reason.
    pub(crate) pad_page_left: PadChord,
    pub(crate) pad_page_right: PadChord,
}

impl Default for ButtonSettings {
    fn default() -> Self {
        Self {
            r1: Bucket::Light,
            r2: Bucket::Heavy,
            l1: Bucket::Ranged,
            l2: Bucket::Movement,
            page_left: default_chord(PAGE_DEFAULT_LEFT),
            page_right: default_chord(PAGE_DEFAULT_RIGHT),
            pad_page_left: default_pad(PAGE_DEFAULT_PAD_LEFT),
            pad_page_right: default_pad(PAGE_DEFAULT_PAD_RIGHT),
        }
    }
}

impl ButtonSettings {
    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = BUTTONS_SECTION;
        take(&mut self.r1, doc, s, "r1", rejections, Bucket::parse);
        take(&mut self.r2, doc, s, "r2", rejections, Bucket::parse);
        take(&mut self.l1, doc, s, "l1", rejections, Bucket::parse);
        take(&mut self.l2, doc, s, "l2", rejections, Bucket::parse);
        take(
            &mut self.page_left,
            doc,
            s,
            "page_left",
            rejections,
            parse_optional_chord,
        );
        take(
            &mut self.page_right,
            doc,
            s,
            "page_right",
            rejections,
            parse_optional_chord,
        );
        take(
            &mut self.pad_page_left,
            doc,
            s,
            "pad_page_left",
            rejections,
            parse_pad,
        );
        take(
            &mut self.pad_page_right,
            doc,
            s,
            "pad_page_right",
            rejections,
            parse_pad,
        );
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "r1={} r2={} l1={} l2={} page_left={} page_right={}",
            self.r1.name(),
            self.r2.name(),
            self.l1.name(),
            self.l2.name(),
            binding_text(self.page_left, self.pad_page_left),
            binding_text(self.page_right, self.pad_page_right),
        )
    }
}

/// `[movement]`. Live.
///
/// # Two keys that used to be here and are not settings
///
/// `heading_converge` and `root_motion_only` shipped as `RESERVED` with a comment admitting there
/// was nothing for `false` to mean. There still is not, and now the mechanism is known well enough
/// to say why rather than to promise it later, so they are gone rather than reserved:
///
/// * The body converges on the requested heading at the rate its own `NpcParam` gives it, because
///   turning is the locomotion executor's job and this crate only names the direction. There is no
///   snap available to write.
/// * Root motion is the only mechanism there is. The engine's move vector is a normalised
///   direction handed to `CSChrActionRequestModule` -- the player's own request module -- and the
///   behaviour graph moves the body with locomotion clips. Nothing on the path takes a velocity.
///
/// An old config that still names them is unaffected: the parser only reads keys it asks for, so
/// an unknown one is ignored rather than rejected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MovementSettings {
    /// Stick deflection inside this cone of the body's own heading is treated as "no turn asked
    /// for", and straightened to exactly forward. Degrees, `0..=180`.
    pub(crate) turn_deadzone_deg: f32,
    pub(crate) speed_scale: f32,
    /// The keyboard fallback for the left stick. `None` is unbound, which is a real setting.
    ///
    /// A creature is driven by a stick -- a direction and a magnitude -- and a keyboard has
    /// neither, so these four keys are synthesised into one. They exist because the alternative
    /// measured worse than "no controller, no movement": with no pad attached `XInputGetState`
    /// returns nothing, the intent is empty every frame, and the creature stands still while
    /// every attack works, which reads as a broken mod rather than as a missing device.
    pub(crate) forward: Option<Chord>,
    pub(crate) back: Option<Chord>,
    pub(crate) left: Option<Chord>,
    pub(crate) right: Option<Chord>,
}

impl Default for MovementSettings {
    fn default() -> Self {
        Self {
            turn_deadzone_deg: 20.0,
            speed_scale: 1.0,
            forward: default_chord(MOVE_DEFAULT_FORWARD),
            back: default_chord(MOVE_DEFAULT_BACK),
            left: default_chord(MOVE_DEFAULT_LEFT),
            right: default_chord(MOVE_DEFAULT_RIGHT),
        }
    }
}

impl MovementSettings {
    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = MOVEMENT_SECTION;
        take(
            &mut self.turn_deadzone_deg,
            doc,
            s,
            "turn_deadzone_deg",
            rejections,
            |raw| parse_f32(raw).filter(|v| (0.0..=180.0).contains(v)),
        );
        take(
            &mut self.speed_scale,
            doc,
            s,
            "speed_scale",
            rejections,
            |raw| parse_f32(raw).filter(|v| *v > 0.0),
        );
        for (field, key) in [
            (&mut self.forward, "forward"),
            (&mut self.back, "back"),
            (&mut self.left, "left"),
            (&mut self.right, "right"),
        ] {
            take(field, doc, s, key, rejections, parse_optional_chord);
        }
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "turn_deadzone_deg={} speed_scale={}",
            self.turn_deadzone_deg, self.speed_scale
        )
    }
}

/// `[camera]`. Live -- but only read at possession start, because the row patch and the
/// `ChrExFollowCam+0x468` write both happen once and are undone once.
///
/// See [`crate::camera`] for what each of these actually moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CameraSettings {
    /// The off switch. `false` leaves the follow camera exactly as vanilla frames it, which on
    /// anything large means inside the model.
    pub(crate) enabled: bool,
    /// Which `LockCamParam` row to patch in memory. It must exist and nothing in the regulation
    /// may reference it; both are checked against the live param tables at possession start.
    pub(crate) param_row: u32,
    /// A taste KNOB over the framing law, not the law: `3.8 * (H / 1.5) ^ exponent`. `1.0` is
    /// the law -- distance scaling with height, which is what holds the player's framing at every
    /// size -- and is the default for that reason rather than as a preference. `0.0` pins the
    /// distance at the player's own; anything below `1.0` is a deliberately tighter shot on big
    /// creatures. Whatever it is set to, the pivot is solved against the distance it produces, so
    /// a tight shot loses headroom rather than cropping the head.
    pub(crate) distance_exponent: f32,
    /// Ceiling on the framing distance, in metres, for anyone who wants their camera closer than
    /// the framing asks. The clearance floor over the creature's own physics radius still wins
    /// over this, and the pivot follows whatever distance comes out -- see
    /// [`crate::camera::geometry`].
    pub(crate) distance_max: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            // 1000. Free in the shipped regulation -- 73 of the 166 `LockCamParam` rows are, and
            // all of 1000-1099 are among them -- and re-checked live so a regulation mod that
            // uses it gets a refusal rather than a stolen camera.
            param_row: 1000,
            // 1.0, not 0.7, and the difference is the whole of "the big ones get clipped".
            //
            // The camera's job is to hold a framing, and with a fixed vertical FOV holding a
            // framing means distance scales LINEARLY with subject height. Any exponent below 1
            // makes the shot tighter the bigger the creature -- the framing degrades in exactly
            // the direction the size increases, which is the worst possible place to be sublinear.
            //
            // Measured in the live game 2026-09-02, and reported by the user before the number was
            // looked at: a 6.00 m creature framed at 10.03 m, where the player's own 3.8 m at
            // 1.5 m tall wants 15.2 m for the same shot. The user's words were that a normal
            // character has "at least a full character's height between my head and the top of the
            // screen" while big avatars "get clipped by the camera because it doesn't travel far
            // enough away from the target".
            distance_exponent: 1.0,
            // Derived, not picked -- see `camera::geometry::MAX_FRAMING_DISTANCE`. Every ceiling
            // this setting has had was chosen first and then cropped a real creature: 40 m cropped
            // everything above 3.8 m tall, and the 120 m that replaced it still cropped the 59 m
            // Walking Mausoleum (`c4450`), whose framing distance is 149.5 m. A nonsense height
            // from a modded `NpcParam` is already caught one step earlier and better, by clamping
            // the height -- which keeps the distance and the pivot consistent with each other,
            // where clamping only the distance breaks the composition. So the default is the
            // distance the law asks for at that height clamp, and cannot fire on its own. It is
            // still a real knob for anyone who wants their camera closer than the framing wants.
            distance_max: crate::camera::geometry::MAX_FRAMING_DISTANCE,
        }
    }
}

/// `[picker]`. Live -- the in-game creature list and the keys that drive it.
///
/// # Why the bindings live in a table instead of beside `hotkey`
///
/// The three top-level bindings (`hotkey`, `gamepad_hotkey`, `radial`) go through
/// `config::PossessConfig`'s keep-the-last-working-value machinery, which exists so a typo in the
/// key you possess with does not leave you unable to possess. These six are not load-bearing in
/// that way: a typo in `next_group` costs you one direction of a list you can still step through,
/// and the `[picker]` table is live, so the fix is to correct the line and save. Routing them
/// through six more `Binding` fields plus six more `ConfigUpdate` variants would have doubled the
/// config's moving parts for a strictly smaller failure.
///
/// # Why every pad binding ships empty
///
/// This crate claims no game prologue -- specifically not the DirectInput `GetDeviceState` one
/// that three other shells in this profile already share -- so it can see a press but cannot
/// take it away from the game. See the `er-npc-possess` entry in
/// `scripts/me3-dll-conflicts.toml`. A controller has few buttons and Elden Ring uses them, so a
/// shipped pad default for "move the picker cursor down" would very likely also swap the
/// player's spell every time it was pressed -- and a default that misbehaves out of the box is
/// worse than no default.
///
/// That is not a measurement. Elden Ring's own binding table was not read; it is a judgement
/// about where a collision is likely, and every one of these keys is rebindable while the game
/// runs precisely because the judgement may be wrong for a given setup.
///
/// # Why the keyboard defaults are the arrows and not the keypad
///
/// The keypad was the first choice, for being the cluster least likely to be in the way. It was
/// wrong for a reason that has nothing to do with collisions: `KP_8` parses to `VK_NUMPAD8`, and
/// with NUMLOCK off the numpad's 8 key does not produce `VK_NUMPAD8` at all -- it produces
/// `VK_UP`. [`crate::input::chord_held`] reads the virtual key, so all four navigation defaults
/// would have been silently dead for anyone with NumLock off or no numpad at all, on a feature
/// with no runtime proof. A key that does nothing and says nothing is a worse default than a key
/// that collides, because a collision is visible and this is not.
///
/// `Up`/`Down`/`Left`/`Right` produce `VK_UP`/`VK_DOWN`/`VK_LEFT`/`VK_RIGHT` from the arrow
/// cluster and from the numpad with NumLock off, so both work. `KP_*` spellings still parse and
/// are still a fine thing to bind -- they just need NumLock on, which the shipped config says.
///
/// Binding a pad button here is supported and is a real choice a player may want; it is just not
/// one this file makes on their behalf. When the DirectInput claim the conflicts table already
/// anticipates lands, suppression becomes possible and these defaults can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PickerSettings {
    pub(crate) enabled: bool,
    /// Rows drawn at once. A viewport, not a capacity: the list is always the whole catalogue.
    pub(crate) visible_rows: u32,
    /// Opens and closes the list. `None` when the value is empty, which is a real setting.
    pub(crate) toggle: Option<Chord>,
    pub(crate) up: Option<Chord>,
    pub(crate) down: Option<Chord>,
    pub(crate) prev_group: Option<Chord>,
    pub(crate) next_group: Option<Chord>,
    /// Pad equivalents. `PadChord::default()` is the empty mask, i.e. unbound.
    pub(crate) pad_toggle: PadChord,
    pub(crate) pad_up: PadChord,
    pub(crate) pad_down: PadChord,
    pub(crate) pad_prev_group: PadChord,
    pub(crate) pad_next_group: PadChord,
}

/// The shipped keyboard defaults, spelled once so the parser, the config comment and the tests
/// cannot drift apart.
/// The keyboard movement fallback, spelled once so the parser, the shipped config comment and the
/// tests cannot drift apart. WASD because that is what the game itself binds movement to.
pub(crate) const MOVE_DEFAULT_FORWARD: &str = "W";
pub(crate) const MOVE_DEFAULT_BACK: &str = "S";
pub(crate) const MOVE_DEFAULT_LEFT: &str = "A";
pub(crate) const MOVE_DEFAULT_RIGHT: &str = "D";

/// The attack-set page keys, spelled once for the parser, the shipped config comment and the
/// tests.
///
/// Left and right arrow because vanilla binds those to the left-hand and right-hand armament swap
/// -- the gesture a player already has in their fingers for "make this button do something else".
/// A possessed creature has no armaments and no `PlayerGameData` to swap them in, so both keys are
/// dead weight during a possession and there is nothing to collide with. The pad spelling is the
/// d-pad for the same reason.
pub(crate) const PAGE_DEFAULT_LEFT: &str = "Left";
pub(crate) const PAGE_DEFAULT_RIGHT: &str = "Right";
pub(crate) const PAGE_DEFAULT_PAD_LEFT: &str = "DPad_Left";
pub(crate) const PAGE_DEFAULT_PAD_RIGHT: &str = "DPad_Right";

pub(crate) const PICKER_DEFAULT_TOGGLE: &str = "F10";
pub(crate) const PICKER_DEFAULT_UP: &str = "Up";
pub(crate) const PICKER_DEFAULT_DOWN: &str = "Down";
pub(crate) const PICKER_DEFAULT_PREV_GROUP: &str = "Left";
pub(crate) const PICKER_DEFAULT_NEXT_GROUP: &str = "Right";

/// Rows on screen at once when the file says nothing.
pub(crate) const PICKER_DEFAULT_VISIBLE_ROWS: u32 = 15;

/// Bounds on `visible_rows`. One row is a picker you cannot see the shape of; past forty the
/// panel is taller than the screen it is drawn on.
const PICKER_MIN_VISIBLE_ROWS: u32 = 5;
const PICKER_MAX_VISIBLE_ROWS: u32 = 40;

impl Default for PickerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            visible_rows: PICKER_DEFAULT_VISIBLE_ROWS,
            toggle: default_chord(PICKER_DEFAULT_TOGGLE),
            up: default_chord(PICKER_DEFAULT_UP),
            down: default_chord(PICKER_DEFAULT_DOWN),
            prev_group: default_chord(PICKER_DEFAULT_PREV_GROUP),
            next_group: default_chord(PICKER_DEFAULT_NEXT_GROUP),
            pad_toggle: PadChord::default(),
            pad_up: PadChord::default(),
            pad_down: PadChord::default(),
            pad_prev_group: PadChord::default(),
            pad_next_group: PadChord::default(),
        }
    }
}

impl CameraSettings {
    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = CAMERA_SECTION;
        take(&mut self.enabled, doc, s, "enabled", rejections, parse_bool);
        take(
            &mut self.param_row,
            doc,
            s,
            "param_row",
            rejections,
            |raw| parse_i32(raw).and_then(|v| u32::try_from(v).ok()),
        );
        take(
            &mut self.distance_exponent,
            doc,
            s,
            "distance_exponent",
            rejections,
            // Negative would make bigger creatures closer, which is the one shape this setting
            // must not be able to express.
            |raw| parse_f32(raw).filter(|v| (0.0..=2.0).contains(v)),
        );
        take(
            &mut self.distance_max,
            doc,
            s,
            "distance_max",
            rejections,
            |raw| parse_f32(raw).filter(|v| *v > 0.0),
        );
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "enabled={} param_row={} distance_exponent={} distance_max={}",
            self.enabled, self.param_row, self.distance_exponent, self.distance_max
        )
    }
}

/// A built-in default must parse; a spelling this crate ships and cannot read is a build bug, and
/// the test below is what turns it into a red gate rather than a silently unbound key.
fn default_chord(spelling: &str) -> Option<Chord> {
    parse_chord(spelling).ok()
}

/// An empty value is a real setting -- "unbound" -- not a parse failure. Anything else that does
/// not parse is a rejection, so it reaches the log instead of silently unbinding the key.
fn parse_optional_chord(raw: &str) -> Option<Option<Chord>> {
    if raw.trim().is_empty() {
        return Some(None);
    }
    parse_chord(raw).ok().map(Some)
}

fn parse_pad(raw: &str) -> Option<PadChord> {
    if raw.trim().is_empty() {
        return Some(PadChord::default());
    }
    parse_pad_chord(raw).ok()
}

impl PickerSettings {
    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        let s = PICKER_SECTION;
        take(&mut self.enabled, doc, s, "enabled", rejections, parse_bool);
        take(
            &mut self.visible_rows,
            doc,
            s,
            "visible_rows",
            rejections,
            |raw| {
                raw.trim()
                    .parse::<u32>()
                    .ok()
                    .filter(|v| (PICKER_MIN_VISIBLE_ROWS..=PICKER_MAX_VISIBLE_ROWS).contains(v))
            },
        );
        for (field, key) in [
            (&mut self.toggle, "hotkey"),
            (&mut self.up, "up"),
            (&mut self.down, "down"),
            (&mut self.prev_group, "prev_group"),
            (&mut self.next_group, "next_group"),
        ] {
            take(field, doc, s, key, rejections, parse_optional_chord);
        }
        for (field, key) in [
            (&mut self.pad_toggle, "gamepad_hotkey"),
            (&mut self.pad_up, "pad_up"),
            (&mut self.pad_down, "pad_down"),
            (&mut self.pad_prev_group, "pad_prev_group"),
            (&mut self.pad_next_group, "pad_next_group"),
        ] {
            take(field, doc, s, key, rejections, parse_pad);
        }
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "enabled={} visible_rows={} hotkey={} up={} down={} prev_group={} next_group={} \
             gamepad_hotkey={}",
            self.enabled,
            self.visible_rows,
            chord_text(self.toggle),
            chord_text(self.up),
            chord_text(self.down),
            chord_text(self.prev_group),
            chord_text(self.next_group),
            pad_chord_name(self.pad_toggle),
        )
    }
}

/// `[hud]`. Live.
///
/// Two switches, and they are two rather than one because they are two surfaces, not two tastes
/// about one. [`Self::enabled`] retargets bars the game already draws; [`Self::pages`] adds a
/// panel the game does not have. Neither implies the other -- a player who wants the creature's
/// HP but not a second panel in the corner, or the panel but their own bars, is asking for a
/// coherent thing in both directions. Within each surface there is still deliberately no
/// per-element knob, because "show the creature's HP but the player's stamina" is not a thing
/// anyone wants and every extra knob is another state nobody tests. What the FP and stamina bars
/// do when the creature has no such pool is decided by evidence rather than by configuration --
/// see [`crate::hud::vitals`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HudSettings {
    /// Point the HP / FP / stamina bars at the possessed creature.
    ///
    /// Defaults ON: it is the behaviour the feature exists for, and turning it off is the
    /// unusual request. `false` installs nothing -- the detour is skipped entirely rather than
    /// installed and left idle, so a player who does not want it does not carry five patched
    /// bytes in the game image for the whole session.
    pub(crate) enabled: bool,
    /// Draw the attack-set panel while something is possessed.
    ///
    /// Defaults on, and that default is not a preference: the attack-set page is a mode with no
    /// other indicator anywhere on screen, and a mode nobody can see is a mode nobody can use.
    /// Shipping it off would recreate the exact defect it was written to close -- a live session
    /// on 2026-09-02 paged the right hand through seventeen sets, logged every one of them, and
    /// the player watching the screen reported "I still don't see any abilities to switch
    /// between".
    ///
    /// `false` costs nothing at runtime beyond one atomic read per frame: the panel is skipped
    /// before any lock is taken, and a session that possesses nothing never installs the overlay
    /// at all.
    pub(crate) pages: bool,
}

impl Default for HudSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            pages: true,
        }
    }
}

impl HudSettings {
    fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        take(
            &mut self.enabled,
            doc,
            HUD_SECTION,
            "enabled",
            rejections,
            parse_bool,
        );
        take(
            &mut self.pages,
            doc,
            HUD_SECTION,
            "pages",
            rejections,
            parse_bool,
        );
    }

    pub(crate) fn summary(&self) -> String {
        format!("enabled={} pages={}", self.enabled, self.pages)
    }
}

fn chord_text(chord: Option<Chord>) -> String {
    chord.map_or_else(|| "(none)".to_owned(), chord_name)
}

/// A built-in pad default must parse, for the same reason [`default_chord`] exists.
fn default_pad(spelling: &str) -> PadChord {
    parse_pad_chord(spelling).unwrap_or_default()
}

/// One binding as the log prints it: both spellings, because a player who bound only the pad and a
/// player who unbound both need to tell their situations apart.
pub(crate) fn binding_text(chord: Option<Chord>, pad: PadChord) -> String {
    format!("{}/{}", chord_text(chord), pad_chord_name(pad))
}

/// One `[chr.cXXXX]` table. Live, and open-ended: the parser reports the ids the file contains
/// rather than looking up a list this build was compiled knowing.
///
/// Every field is optional because an override is a delta. `speed_scale` absent here means "use
/// `[movement].speed_scale`", which is not the same as "1.0".
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ChrOverride {
    /// The section suffix as written, e.g. `c4500`.
    pub(crate) chr: String,
    pub(crate) turn_rate_deg_per_sec: Option<f32>,
    pub(crate) camera_distance_scale: Option<f32>,
    pub(crate) speed_scale: Option<f32>,
    /// `pin = { r2 = 3046 }` -- force one input onto one animation id.
    pub(crate) pin: Vec<(String, i32)>,
    /// Animation ids this character must never be given.
    pub(crate) unusable: Vec<i32>,
    /// Animation ids to force back in, overriding the auto-classifier's verdict.
    pub(crate) usable: Vec<i32>,
}

impl ChrOverride {
    fn read(doc: &Document, chr: &str, rejections: &mut Rejections) -> Self {
        let section = format!("{CHR_SECTION_PREFIX}{chr}");
        let mut over = Self {
            chr: chr.to_owned(),
            ..Self::default()
        };
        let positive = |key: &str, slot: &mut Option<f32>, rejections: &mut Rejections| {
            if let Some(raw) = doc.scalar(&section, key) {
                match parse_f32(raw).filter(|v| *v > 0.0) {
                    Some(value) => *slot = Some(value),
                    None => rejections.push(&qualify(&section, key), raw),
                }
            }
        };
        positive(
            "turn_rate_deg_per_sec",
            &mut over.turn_rate_deg_per_sec,
            rejections,
        );
        positive(
            "camera_distance_scale",
            &mut over.camera_distance_scale,
            rejections,
        );
        positive("speed_scale", &mut over.speed_scale, rejections);

        if let Some(pairs) = doc.inline_table(&section, "pin") {
            for (input, value) in pairs {
                match parse_i32(value) {
                    Some(id) => over.pin.push((input.to_owned(), id)),
                    None => rejections.push(&qualify(&section, &format!("pin.{input}")), value),
                }
            }
        }
        for (key, slot) in [
            ("unusable", &mut over.unusable),
            ("usable", &mut over.usable),
        ] {
            let Some(items) = doc.array(&section, key) else {
                continue;
            };
            for item in items {
                match parse_i32(item) {
                    Some(id) => slot.push(id),
                    None => rejections.push(&qualify(&section, key), item),
                }
            }
        }
        over
    }
}

/// Everything in the file that is not a hotkey and not the master switch.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Tables {
    pub(crate) mapping: MappingSettings,
    pub(crate) buttons: ButtonSettings,
    pub(crate) movement: MovementSettings,
    pub(crate) camera: CameraSettings,
    pub(crate) hud: HudSettings,
    pub(crate) picker: PickerSettings,
    pub(crate) chr_overrides: Vec<ChrOverride>,
}

impl Tables {
    /// Apply a whole document. `[target]` is deliberately not here -- it is staged separately
    /// because it is the one table that must not move mid-possession.
    pub(crate) fn apply(&mut self, doc: &Document, rejections: &mut Rejections) {
        self.mapping.apply(doc, rejections);
        self.buttons.apply(doc, rejections);
        self.movement.apply(doc, rejections);
        self.camera.apply(doc, rejections);
        self.hud.apply(doc, rejections);
        self.picker.apply(doc, rejections);

        // Replaced, not merged. A `[chr.cXXXX]` table the player deleted has to stop applying,
        // and merging into what is already in force would make deletion a no-op -- the one edit
        // that is impossible to debug, because the file no longer mentions the setting that is
        // still in effect.
        self.chr_overrides = doc
            .sections_under(CHR_SECTION_PREFIX)
            .into_iter()
            .map(|chr| ChrOverride::read(doc, chr, rejections))
            .collect();
    }

    pub(crate) fn summary(&self) -> String {
        format!(
            "[mapping] {} | [buttons] {} | [movement] {} | [camera] {} | [hud] {} | \
             [picker] {} | chr_overrides={}",
            self.mapping.summary(),
            self.buttons.summary(),
            self.movement.summary(),
            self.camera.summary(),
            self.hud.summary(),
            self.picker.summary(),
            self.chr_overrides.len()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> (Tables, TargetSettings, Rejections) {
        let doc = Document::parse(text);
        let mut tables = Tables::default();
        let mut target = TargetSettings::default();
        let mut rejections = Rejections::default();
        tables.apply(&doc, &mut rejections);
        target.apply_from(&doc, &mut rejections);
        (tables, target, rejections)
    }

    #[test]
    fn an_empty_file_yields_the_built_in_defaults() {
        let (tables, target, rejections) = read("");
        assert_eq!(tables, Tables::default());
        assert_eq!(target, TargetSettings::default());
        assert!(rejections.is_empty());
        assert_eq!(target.mode, TargetMode::LockOn);
        assert_eq!(tables.mapping.bands_m, (4.0, 12.0));
        assert_eq!(tables.buttons.r1, Bucket::Light);
    }

    #[test]
    fn every_table_reads_back_what_the_file_says() {
        let (tables, target, rejections) = read(
            r#"
[target]
mode = "chr_id"
chr_id = 45000
release_on_death = false

[mapping]
model = "slots"
combo_window_ms = 800
bands_m = [2.5, 20.0]
allow_grabs = true
unbound_inputs = "deny"

[buttons]
r1 = "heavy"; r2 = "light"; l1 = "movement"; l2 = "ranged"

[movement]
turn_deadzone_deg = 5.0
speed_scale = 1.25

[camera]
enabled = false
param_row = 1099
distance_exponent = 1.0
distance_max = 25.0
[hud]
enabled = false
"#,
        );
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert_eq!(target.mode, TargetMode::ChrId);
        assert_eq!(target.chr_id, 45000);
        assert!(!target.release_on_death);
        assert_eq!(tables.mapping.model, MappingModel::Slots);
        assert_eq!(tables.mapping.combo_window_ms, 800);
        assert_eq!(tables.mapping.bands_m, (2.5, 20.0));
        assert!(tables.mapping.allow_grabs);
        assert_eq!(tables.mapping.unbound_inputs, UnboundInputs::Deny);
        assert_eq!(tables.buttons.r1, Bucket::Heavy);
        assert_eq!(tables.buttons.l2, Bucket::Ranged);
        assert_eq!(tables.movement.turn_deadzone_deg, 5.0);
        assert_eq!(tables.movement.speed_scale, 1.25);
        assert!(!tables.camera.enabled);
        assert_eq!(tables.camera.param_row, 1099);
        assert_eq!(tables.camera.distance_exponent, 1.0);
        assert_eq!(tables.camera.distance_max, 25.0);
        assert!(!tables.hud.enabled);
    }

    /// `[camera]` values that would break the size law are rejected rather than taken.
    ///
    /// A negative exponent would make a dragon's camera closer than a rat's, and a negative row id
    /// cannot be a param row at all -- both are the kind of value that produces a camera nobody
    /// can explain, so both keep the working value and name themselves.
    #[test]
    fn the_camera_table_refuses_values_that_invert_the_size_law() {
        let (tables, _, rejections) =
            read("[camera]\nparam_row = -3\ndistance_exponent = -1.0\ndistance_max = 0\n");
        assert_eq!(tables.camera, CameraSettings::default());
        let summary = rejections.summary();
        assert!(summary.contains("camera.param_row=\"-3\""), "{summary}");
        assert!(
            summary.contains("camera.distance_exponent=\"-1.0\""),
            "{summary}"
        );
        assert!(summary.contains("camera.distance_max=\"0\""), "{summary}");
    }

    /// The shipped default is the row this crate proved free offline, and the exponent that holds
    /// a framing rather than the one that fitted under a ceiling.
    ///
    /// The old default was `0.7`, and this comment used to justify it as "the exponent that keeps
    /// the biggest shipped creature inside the ceiling" -- which is the reasoning backwards. The
    /// ceiling was picked first, the exponent was bent until the giants fitted under it, and what
    /// the bending actually did was crop them: below 1.0 the shot gets tighter as the subject gets
    /// bigger, because distance grows slower than the thing it is framing.
    ///
    /// Reported by the user from a live run 2026-09-02, before anyone looked at the number: a
    /// normal character has "at least a full character's height between my head and the top of the
    /// screen", while the big avatars "get clipped by the camera because it doesn't travel far
    /// enough away from the target, and not high enough". The log from that same run shows a
    /// 6.00 m creature framed at 10.03 m; linear framing from the player's 3.8 m at 1.5 m wants
    /// 15.2 m.
    ///
    /// So the exponent is pinned at 1.0 by the geometry, not by taste, and the second assertion
    /// below states that geometry directly -- a future edit that moves the constant back has to
    /// break a claim about framing, not merely a number.
    #[test]
    fn the_camera_defaults_hold_a_framing_rather_than_fit_under_a_ceiling() {
        let camera = CameraSettings::default();
        assert!(camera.enabled);
        assert_eq!(camera.param_row, 1000);
        assert_eq!(camera.distance_exponent, 1.0);
        assert_eq!(
            camera.distance_max,
            crate::camera::geometry::MAX_FRAMING_DISTANCE
        );

        // The property the number is for. Doubling the subject's height must double the framing
        // distance, or the headroom above its head shrinks as it grows. `powf` is the law in
        // `camera::geometry::shape`; this asserts the exponent the law is fed keeps it linear.
        let at = |scale: f32| scale.powf(camera.distance_exponent);
        assert!(
            (at(2.0) / at(1.0) - 2.0).abs() < 1e-6,
            "2x taller wants 2x the distance"
        );
        assert!(
            (at(8.0) / at(4.0) - 2.0).abs() < 1e-6,
            "and it must not decay with size"
        );

        // The ceiling must not be what decides the shot for a real creature. The tallest
        // possessable subject is not the 29 m Fire Giant -- it is `c4450`, the Walking Mausoleum,
        // at 59 m, which linear framing puts 149.5 m out. The 120 m ceiling that first replaced
        // the 40 m one still cropped it, which is why this asserts against the real maximum rather
        // than against the biggest creature anyone happened to name.
        let tallest_real_m: f32 = 59.0;
        let wanted = 3.8 * at(tallest_real_m / 1.5);
        assert!(
            wanted < camera.distance_max,
            "{wanted} m would be cropped by the ceiling"
        );
    }

    /// `[spawn]` reads back as written, and it reads back through `[target]`'s reader -- which is
    /// what makes it staged rather than live. A `[spawn]` that took effect immediately could change
    /// `despawn_on_release` under an in-flight possession and decide the fate of a roster slot that
    /// was created under the other answer.
    #[test]
    fn the_spawn_table_reads_back_what_the_file_says_and_arrives_through_target() {
        let (_, target, rejections) = read(
            r#"
[target]
mode = "spawn"

[spawn]
chr_id = 3200
npc_param_id = 32000100
npc_think_id = 32000000
distance_m = 7.5
readiness_ms = 12000
despawn_on_release = false
"#,
        );
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert_eq!(target.mode, TargetMode::Spawn);
        assert!(target.mode.creates(), "spawn is the mode that creates");
        assert_eq!(target.spawn.chr_id, 3200);
        assert_eq!(target.spawn.npc_param_id, 32_000_100);
        assert_eq!(target.spawn.resolved_npc_param_id(), 32_000_100);
        assert_eq!(target.spawn.npc_think_id, 32_000_000);
        assert_eq!(target.spawn.distance_m, 7.5);
        assert_eq!(target.spawn.readiness_ms, 12_000);
        assert!(!target.spawn.despawn_on_release);
        // ...and it is named in the summary only for the mode that uses it, so the log line for a
        // lock-on possession does not carry six irrelevant numbers.
        assert!(target.summary().contains("spawn["), "{}", target.summary());
        let borrowed = TargetSettings::default();
        assert!(!borrowed.mode.creates());
        assert!(
            !borrowed.summary().contains("spawn["),
            "{}",
            borrowed.summary()
        );
    }

    /// `npc_param_id = 0` means "derive it", and the derivation has to land on the row the shipped
    /// moveset table is keyed by -- c4500 is row 45,000,000, not 45,000.
    #[test]
    fn a_zero_param_row_is_derived_from_the_chr_id_at_the_right_magnitude() {
        let (_, target, _) = read("[spawn]\nchr_id = 4500\nnpc_param_id = 0\n");
        assert_eq!(target.spawn.resolved_npc_param_id(), 45_000_000);
        assert_eq!(target.spawn.resolved_npc_param_id() / 10_000, 4500);
    }

    /// Every `[spawn]` bound is enforced at PARSE time, so a bad number is reported next to the
    /// line it was typed on rather than as a hotkey press that quietly does nothing. Zero is
    /// rejected with the out-of-range ids: `c0000` is not a creature, and the game answers a
    /// request for a model that does not exist by waiting rather than by failing -- so accepting it
    /// would buy a five-second timeout instead of a message.
    #[test]
    fn out_of_range_spawn_values_are_rejected_and_the_working_ones_stay() {
        let (_, target, rejections) = read(
            r#"
[spawn]
chr_id = 10000
npc_think_id = 5
distance_m = 0.0
readiness_ms = 500
"#,
        );
        for key in ["spawn.chr_id", "spawn.distance_m", "spawn.readiness_ms"] {
            assert!(
                rejections.summary().contains(key),
                "{key} in {}",
                rejections.summary()
            );
        }
        // The rejected ones kept the value in force; the readable one landed.
        assert_eq!(target.spawn.chr_id, SpawnSettings::default().chr_id);
        assert_eq!(target.spawn.distance_m, SpawnSettings::default().distance_m);
        assert_eq!(
            target.spawn.readiness_ms,
            SpawnSettings::default().readiness_ms
        );
        assert_eq!(target.spawn.npc_think_id, 5);
        // Zero is out of range for the same reason 10000 is.
        let (_, zeroed, rejected) = read("[spawn]\nchr_id = 0\n");
        assert!(
            rejected.summary().contains("spawn.chr_id"),
            "{}",
            rejected.summary()
        );
        assert_eq!(zeroed.spawn.chr_id, SpawnSettings::default().chr_id);
    }

    /// The HUD retarget is on unless the file says otherwise. It is the behaviour the layer
    /// exists for, so defaulting it off would ship a feature nobody sees without editing a file
    /// they have no reason to open.
    #[test]
    fn the_hud_retarget_defaults_on_and_only_the_file_turns_it_off() {
        let (tables, _, rejections) = read("");
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert!(tables.hud.enabled, "default ON");

        let (tables, _, rejections) = read("[hud]\nenabled = false\n");
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert!(!tables.hud.enabled);

        // ...and a misspelling is a rejection, not an off switch -- the same rule as every other
        // boolean here. "enabled = flase" reading as false is indistinguishable, from the
        // player's chair, from the retarget being broken.
        let doc = Document::parse("[hud]\nenabled = \"flase\"\n");
        let mut hud = HudSettings::default();
        let mut rejections = Rejections::default();
        hud.apply(&doc, &mut rejections);
        assert!(hud.enabled, "the value in force is kept");
        assert_eq!(rejections.summary(), "hud.enabled=\"flase\"");
    }

    /// The attack-set panel obeys the same three rules as the bar retarget beside it, and -- the
    /// part worth a test rather than a reading -- the two keys are independent.
    ///
    /// One `[hud]` table holding two switches is exactly the shape where a copy-pasted `take`
    /// silently points both at one field, and the symptom would be that turning off the bars also
    /// removes the panel: two features lost to one edit, with nothing in the file to explain it.
    #[test]
    fn the_attack_set_panel_defaults_on_and_is_independent_of_the_bar_retarget() {
        let (tables, _, rejections) = read("");
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert!(tables.hud.pages, "default ON");

        let (tables, _, rejections) = read("[hud]\npages = false\n");
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert!(!tables.hud.pages);
        assert!(
            tables.hud.enabled,
            "turning the panel off must not take the bar retarget with it"
        );

        let (tables, _, rejections) = read("[hud]\nenabled = false\n");
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert!(
            tables.hud.pages,
            "...and turning the bar retarget off must not take the panel with it"
        );

        // A misspelling is a rejection, not an off switch. Same rule as every other boolean here.
        let doc = Document::parse("[hud]\npages = \"of\"\n");
        let mut hud = HudSettings::default();
        let mut rejections = Rejections::default();
        hud.apply(&doc, &mut rejections);
        assert!(hud.pages, "the value in force is kept");
        assert_eq!(rejections.summary(), "hud.pages=\"of\"");
    }

    /// The rule. Junk keeps the value that was working and names itself, so the player can find
    /// the line. It does not reset to the built-in default and it does not read as zero/false.
    #[test]
    fn a_junk_value_keeps_the_one_in_force_and_names_itself() {
        let doc = Document::parse("[movement]\nspeed_scale = \"fast\"\n");
        let mut movement = MovementSettings {
            speed_scale: 1.4,
            ..MovementSettings::default()
        };
        let mut rejections = Rejections::default();
        movement.apply(&doc, &mut rejections);
        assert_eq!(movement.speed_scale, 1.4, "not 1.0, and certainly not 0.0");
        assert_eq!(rejections.summary(), "movement.speed_scale=\"fast\"");
    }

    /// A misspelled boolean must not read as `false`. "off" looks exactly like the feature not
    /// working, and the player has no way to tell which they are looking at.
    #[test]
    fn a_misspelled_boolean_is_a_rejection_not_an_off_switch() {
        assert_eq!(parse_bool("TRUE"), Some(true));
        assert_eq!(parse_bool("false"), Some(false));
        assert_eq!(parse_bool("yes"), None);
        assert_eq!(parse_bool("1"), None);

        let (_, target, rejections) = read("[target]\nrelease_on_death = yes\n");
        assert!(target.release_on_death, "kept the default, which is true");
        assert_eq!(rejections.summary(), "target.release_on_death=\"yes\"");
    }

    /// Half an edit to the range bands would leave the two numbers inconsistent, so they move
    /// together or not at all.
    #[test]
    fn the_range_bands_are_all_or_nothing() {
        for junk in [
            "[mapping]\nbands_m = [4.0]\n",
            "[mapping]\nbands_m = [12.0, 4.0]\n",
            "[mapping]\nbands_m = [4.0, near]\n",
            "[mapping]\nbands_m = [0.0, 12.0]\n",
            "[mapping]\nbands_m = [4.0, 12.0, 30.0]\n",
        ] {
            let (tables, _, rejections) = read(junk);
            assert_eq!(tables.mapping.bands_m, (4.0, 12.0), "{junk}");
            assert!(!rejections.is_empty(), "{junk}");
        }
    }

    #[test]
    fn per_chr_overrides_are_read_from_whatever_ids_the_file_names() {
        let (tables, _, rejections) = read(
            r#"
[chr.c4500]
turn_rate_deg_per_sec = 45.0
camera_distance_scale = 2.4
speed_scale = 1.15
pin = { r2 = 3046 }
unusable = [3300, 3301]
usable = [3120]

[chr.c9999]
speed_scale = 0.5
"#,
        );
        assert!(rejections.is_empty(), "{}", rejections.summary());
        assert_eq!(tables.chr_overrides.len(), 2);
        let first = &tables.chr_overrides[0];
        assert_eq!(first.chr, "c4500");
        assert_eq!(first.turn_rate_deg_per_sec, Some(45.0));
        assert_eq!(first.camera_distance_scale, Some(2.4));
        assert_eq!(first.pin, vec![("r2".to_owned(), 3046)]);
        assert_eq!(first.unusable, vec![3300, 3301]);
        assert_eq!(first.usable, vec![3120]);
        assert_eq!(tables.chr_overrides[1].chr, "c9999");
        // An absent override is None, not a default -- the difference between "leave it alone"
        // and "force it to 1.0".
        assert_eq!(tables.chr_overrides[1].turn_rate_deg_per_sec, None);
    }

    /// Deleting a `[chr.*]` table has to stop it applying. Merging would make deletion the one
    /// edit with no effect, and a setting still in force that the file no longer mentions is
    /// undebuggable.
    #[test]
    fn deleting_a_chr_table_removes_its_override() {
        let mut tables = Tables::default();
        let mut rejections = Rejections::default();
        tables.apply(
            &Document::parse("[chr.c4500]\nspeed_scale = 1.15\n[chr.c2130]\nspeed_scale = 0.8\n"),
            &mut rejections,
        );
        assert_eq!(tables.chr_overrides.len(), 2);
        tables.apply(
            &Document::parse("[chr.c2130]\nspeed_scale = 0.8\n"),
            &mut rejections,
        );
        assert_eq!(tables.chr_overrides.len(), 1);
        assert_eq!(tables.chr_overrides[0].chr, "c2130");
    }

    /// Every enum spelling in the shipped default file resolves, in either case.
    #[test]
    fn every_shipped_enum_spelling_resolves() {
        for (raw, expected) in [
            ("lock_on", TargetMode::LockOn),
            ("NEAREST", TargetMode::Nearest),
            ("crosshair", TargetMode::Crosshair),
            ("chr_id", TargetMode::ChrId),
        ] {
            assert_eq!(TargetMode::parse(raw), Some(expected), "{raw}");
        }
        assert_eq!(TargetMode::parse("locked"), None);
        for (raw, expected) in [
            ("context", MappingModel::Context),
            ("Layered", MappingModel::Layered),
            ("slots", MappingModel::Slots),
        ] {
            assert_eq!(MappingModel::parse(raw), Some(expected), "{raw}");
        }
        assert_eq!(MappingModel::parse("contextual"), None);
        assert_eq!(UnboundInputs::parse("deny"), Some(UnboundInputs::Deny));
        assert_eq!(UnboundInputs::parse("drop"), None);
        assert_eq!(Bucket::parse("Ranged"), Some(Bucket::Ranged));
        assert_eq!(Bucket::parse("spell"), None);
    }

    /// The summaries are what the log line is made of, so they have to name every field: a
    /// setting missing from here is one that silently moved.
    #[test]
    fn the_summaries_name_every_field() {
        let summary = Tables::default().summary();
        for field in [
            "model=",
            "combo_window_ms=",
            "bands_m=",
            "allow_grabs=",
            "unbound_inputs=",
            "r1=",
            "r2=",
            "l1=",
            "l2=",
            "turn_deadzone_deg=",
            "speed_scale=",
            "param_row=",
            "distance_exponent=",
            "distance_max=",
            "chr_overrides=",
        ] {
            assert!(summary.contains(field), "{field} missing from {summary}");
        }
        let target = TargetSettings::default().summary();
        for field in ["mode=", "chr_id=", "release_on_death="] {
            assert!(target.contains(field), "{field} missing from {target}");
        }
    }
}
