//! Which invasions are allowed to land, and where.
//!
//! # What this encodes
//!
//! Seamless decides the destination server-side and pushes it to the client in a
//! `ServerPushJoinData`; the block id is at `+0x00` of that struct, and it is written to
//! `GameMan+0xAC8` by `CS::SosSignMan::SetMultiplayJoinData` (`0x1406FB520`). Measured live
//! 2026-08-05: `+0x00` was the ONLY offset in all 128 bytes whose u32 equalled the destination, so
//! the field is identified by correlation, not by the reversed field name (`matchPlayerCount`,
//! which would have pointed elsewhere).
//!
//! That is the whole basis for a location filter: at the moment `SetMultiplayJoinData` runs, the
//! destination is decided and the player has NOT moved. A predicate there can accept or reject
//! with certainty. Rejecting means cancelling the match, which was measured non-destructive -- the
//! session walks `0x22 -> 0x00` and a fresh search works afterwards.
//!
//! It also encodes a NEGATIVE result, so nobody re-attempts it: the pre-match candidate table does
//! NOT predict the destination. Its block-shaped fields were the LOCAL character's own map
//! (`0x1c000000`, matching this save's `c30`) while the match that followed went to `0x3c353800`.
//! Filtering before connecting is therefore not available, and accept-then-reject is the shape
//! that works.
//!
//! # The anchor
//!
//! Every decision is relative to where the player IS. There is always a nearest invasion map point
//! for the player's current position -- that is what `nearest_place_name_text_id` resolves against
//! the shipped warp rows, same-area only. The anchor carries the block AND the place-name text ids
//! valid at that location, because a single warp destination can sit under more than one place
//! name: one name means one acceptable location, five names mean five.
//!
//! # Why the modes are shaped this way
//!
//! [`LocalInvasionMode::ExactOnly`] is the strictest useful thing -- the exact block the player
//! anchored to. [`LocalInvasionMode::PreferExactThenArea`] widens to the anchor's own place names
//! when the exact block is not on offer, which is the difference between "this tile" and
//! "somewhere in the Haligtree". [`LocalInvasionMode::NamedOnly`] ignores the anchor entirely and
//! honours a user-supplied list, for hunting a place you are not standing in.
//!
//! Disabled is the DEFAULT. This filter cancels real matches, and a mod that silently rejects
//! other players' invasions because a config file appeared is worse than one that does nothing
//! until asked.

use std::collections::BTreeSet;

/// A `PlaceName` FMG text id. `-1` is "no name", and it is not a wildcard.
pub type PlaceNameTextId = i32;

/// The "no name" sentinel. `nearest_place_name_text_id` returns this when nothing resolves, and it
/// must never be treated as matching anything -- an unnamed candidate is not "everywhere".
pub const PLACE_NAME_NONE: PlaceNameTextId = -1;

/// How a destination is judged against the player's anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LocalInvasionMode {
    /// Only the anchor's own block. The narrowest option.
    #[default]
    ExactOnly,
    /// The anchor's block if offered; otherwise any destination sharing one of the anchor's place
    /// names. "Prefer exact" is not a ranking this type can express on a single candidate -- each
    /// match is judged alone -- so it means: exact always passes, same-area also passes.
    PreferExactThenArea,
    /// Ignore the anchor; accept only destinations whose place name is in the configured list.
    NamedOnly,
}

impl LocalInvasionMode {
    /// Parse the TOML spelling. Returns `None` for anything unrecognised so the caller can report
    /// the bad value rather than silently falling back to a mode the user did not ask for.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "exact" | "exact_only" | "exact-only" => Some(Self::ExactOnly),
            "area" | "prefer_exact_then_area" | "prefer-exact-then-area" => {
                Some(Self::PreferExactThenArea)
            }
            "named" | "named_only" | "named-only" => Some(Self::NamedOnly),
            _ => None,
        }
    }

    /// The canonical TOML spelling, for round-tripping and for the generated default file.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExactOnly => "exact",
            Self::PreferExactThenArea => "area",
            Self::NamedOnly => "named",
        }
    }
}

/// Where the player is, and what that location is called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvasionAnchor {
    /// The player's current block id.
    pub block: u32,
    /// Place-name text ids valid at the anchor. Usually one; a location that sits under several
    /// names carries several, and each is an acceptable destination name in area mode.
    pub place_names: BTreeSet<PlaceNameTextId>,
}

impl InvasionAnchor {
    /// Build an anchor from a block and the place names resolved at it, dropping
    /// [`PLACE_NAME_NONE`] -- an unresolved name is not a name to match on.
    #[must_use]
    pub fn new(block: u32, place_names: impl IntoIterator<Item = PlaceNameTextId>) -> Self {
        Self {
            block,
            place_names: place_names
                .into_iter()
                .filter(|id| *id != PLACE_NAME_NONE)
                .collect(),
        }
    }

    /// How many distinct named locations this anchor makes acceptable in area mode. One name means
    /// one place to look, five names mean five.
    #[must_use]
    pub fn named_location_count(&self) -> usize {
        self.place_names.len()
    }
}

/// An incoming match, as known at `SetMultiplayJoinData`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvasionCandidate {
    /// Destination block from `ServerPushJoinData+0x00`.
    pub block: u32,
    /// Place names resolved for that destination. Empty when none resolved.
    ///
    /// # Why this is a SET and not one id
    ///
    /// It used to be a single `PlaceNameTextId`, taken as `.first()` of the resolved list. A block
    /// can sit under several place names -- [`InvasionAnchor`] has always modelled that correctly
    /// -- so keeping one of them threw the rest away, and `.first()` over a `BTreeSet` is the
    /// numerically smallest text id, which is arbitrary. The effect was silent and one-sided: a
    /// destination that shared the anchor's name via any name but its lowest-numbered one was
    /// rejected as `WrongPlaceName`, while the anchor side compared against all of its own. Two
    /// locations under the same name could therefore judge differently depending on which of them
    /// the player happened to be standing in.
    pub place_names: BTreeSet<PlaceNameTextId>,
}

impl InvasionCandidate {
    /// Build a candidate from a block and every place name resolved for it, dropping
    /// [`PLACE_NAME_NONE`] -- the same rule [`InvasionAnchor::new`] applies, so the two sides of a
    /// name comparison cannot disagree about what counts as a name.
    #[must_use]
    pub fn new(block: u32, place_names: impl IntoIterator<Item = PlaceNameTextId>) -> Self {
        Self {
            block,
            place_names: place_names
                .into_iter()
                .filter(|id| *id != PLACE_NAME_NONE)
                .collect(),
        }
    }

    /// A candidate carrying exactly one name.
    #[must_use]
    pub fn named(block: u32, place_name: PlaceNameTextId) -> Self {
        Self::new(block, [place_name])
    }

    /// A candidate whose name could not be resolved.
    ///
    /// Distinct from "resolved to nothing": both arrive here, but naming the constructor makes the
    /// test cases that mean "we could not look it up" read as such.
    #[must_use]
    pub fn unnamed(block: u32) -> Self {
        Self::new(block, [])
    }

    /// Whether any of this candidate's names appears in `names`.
    #[must_use]
    fn shares_a_name_with(&self, names: &BTreeSet<PlaceNameTextId>) -> bool {
        self.place_names.intersection(names).next().is_some()
    }

    /// How many names resolved for this destination.
    #[must_use]
    pub fn named_location_count(&self) -> usize {
        self.place_names.len()
    }
}

/// What to do with a candidate, and why. The reason is carried so a rejection can be logged with
/// its cause instead of appearing as an unexplained cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Let the match land.
    Keep(KeepReason),
    /// Cancel it.
    Reject(RejectReason),
}

/// Why a candidate was kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepReason {
    /// The filter is switched off; everything lands, exactly as unmodded play.
    FilterDisabled,
    /// Same block as the anchor.
    ExactBlock,
    /// Different block, but a place name the anchor also carries.
    SharedPlaceName,
    /// Place name is on the user's configured list.
    NamedLocation,
    /// The exact block is on the user's marked list (Insert, or `allowed_blocks` by hand).
    MarkedBlock,
}

/// Why a candidate was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// Not the anchor's block, in a mode that requires it.
    WrongBlock,
    /// Not one of the anchor's place names.
    WrongPlaceName,
    /// Not on the configured list.
    NotNamed,
    /// The candidate has no resolvable place name, in a mode that judges by name. Rejected rather
    /// than waved through: an unnamed destination is unknown, not universal.
    CandidateUnnamed,
    /// Name-based judging with nothing to judge against -- an anchor with no names, or an empty
    /// list. Fails CLOSED, because the alternative is accepting everything while appearing to
    /// filter.
    NothingToMatchAgainst,
    /// The user excluded this exact location with Delete.
    ExcludedByUser,
}

/// The filter's configuration, as loaded from TOML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalInvasionConfig {
    /// Master switch. Off by default.
    pub enabled: bool,
    /// HUNT MODE: narrow the outgoing lobby query to ONE location instead of rejecting answers.
    ///
    /// Off by default and separate from `enabled` because the two do opposite things to your reach.
    /// The reject filter declines matches and still sees every host; hunt asks Steam for a key only
    /// this DLL's users publish, so while it is on a host without the DLL is invisible to you. That
    /// is a trade the user must choose, never a default.
    pub hunt: bool,
    /// Match ONLY other players running this DLL with this option on.
    ///
    /// Rewrites Seamless's `lobby_key` into a pool of our own (see
    /// [`crate::lobby_pool`]). One value drives both the search filter and the publish, so the
    /// separation is symmetric: vanilla players cannot see you and you cannot see them, for
    /// hosting as much as for invading.
    ///
    /// Absolute, not a preference. While it is on the entire vanilla population is invisible in
    /// both directions, which is why it is off by default and wants to be a per-session choice.
    pub dll_users_only: bool,
    /// Announce a rejection on the game's own system-message banner.
    ///
    /// Off by default: it is a notification, and a notification nobody asked for is spam. When on,
    /// only a CHANGE of wrong destination is announced -- consecutive rejections at the same place
    /// stay silent, because Seamless retries roughly every 20 seconds and the same wrong place
    /// recurs constantly.
    pub reject_notice: bool,
    /// Inject invasion pins into the world map.
    ///
    /// ON by default -- the pins are the feature. It is configurable because the map path is the
    /// crate's largest interaction with live engine memory (it appends 467 rows into the
    /// `WorldMapViewModel`'s row buffer from that object's own constructor), and until this key
    /// existed there was no way to run the DLL WITHOUT that surgery. That made a whole class of
    /// question unanswerable: a fault that follows opening the map could not be attributed,
    /// because the only A/B available was "this DLL or no DLL", which changes nine other things
    /// at the same time.
    ///
    /// Turning it off withholds the `WorldMapViewModel` constructor observer and the world-map
    /// GFx hook. Everything else -- the local-invasion filter, the warp keys, the lobby pool --
    /// is untouched, so the two halves of the A/B differ by the map path and nothing else.
    pub map_pins: bool,
    /// Install the three Steam-matchmaking detours.
    ///
    /// ON by default -- location publishing, hunt mode and the pool filter all need them. It is
    /// configurable for the same reason as [`Self::map_pins`]: these are the only detours this
    /// crate installs at addresses it did not derive statically. Each is read out of a LIVE
    /// `ISteamMatchmaking` vtable slot at runtime and handed straight to MinHook, and the
    /// installers retry every tick until a read succeeds, so whatever the slot happens to hold at
    /// that instant becomes a five-byte patch target inside `steamclient64.dll`.
    ///
    /// Turning it off withholds `install_advertisement_observer`, `install_hunt_hook` and
    /// `install_pool_filter_hook` and nothing else, which makes that patching isolable from
    /// everything else the DLL does.
    pub steam_hooks: bool,
    /// Install the two read-only observers on Seamless Co-op's own code.
    ///
    /// ON by default -- the `show` observer is the only way this DLL learns the Seamless menu
    /// object's address, and the lobby-key observer reports the one string that decides whether
    /// two Seamless players can see each other at all.
    ///
    /// It is configurable because these two are the ONLY always-on detours this crate places
    /// inside `ersc.dll`, and they were the only always-on hooks on the stack of the
    /// `0x140010043` illegal-instruction crash: the fault's frames read
    /// `ersc.dll -> er_invasion_warp -> ersc.dll -> lsteamclient.dll`, and it did not reproduce at
    /// all in a run with this DLL excluded while the player completed a whole invasion. `map_pins`
    /// and `steam_hooks` have each already been A/B'd off with the crash still present, so this is
    /// what is left to isolate.
    ///
    /// Worth knowing while that A/B is open: both are installed through
    /// `er_hook::register_union_hook`, the entry point for a 1.16.2 GAME constant, even though
    /// their addresses are derived from the running `ersc.dll`. That path audits the destination
    /// by translating it through a table keyed by eldenring.exe RVAs, which has nothing to say
    /// about a module based at 0x180000000 -- so unlike every runtime-derived hook in this crate,
    /// these two never face `detour_site::write_site_is_sound`. The only thing standing behind
    /// them is `prologue_matches` against the pinned build's recorded bytes.
    ///
    /// Turning it off withholds `install_show_observer` and `install_lobby_key_observer` and
    /// nothing else; the local-invasion filter still judges matches, and the warp keys still work.
    pub ersc_observers: bool,
    /// Install the `show` observer specifically, when [`Self::ersc_observers`] is on.
    ///
    /// The master switch proved the PAIR is what crashes; these two split that pair so the next
    /// run names which one. Both default ON, so turning the master on restores the previous
    /// behaviour exactly. Both patch sites were checked statically against
    /// `vendor-archive/seamless/ersc-2.0.1.dll` and are mechanically sound -- MinHook's five bytes
    /// land exactly on `push rbp; push r15; push r14`, a clean instruction boundary with nothing
    /// to relocate -- so whatever goes wrong is in the detour's semantics, not the patch.
    pub ersc_show_observer: bool,
    /// Install the lobby-key observer specifically, when [`Self::ersc_observers`] is on.
    ///
    /// See [`Self::ersc_show_observer`]. This is the one whose target Seamless documents as
    /// `SHA256_hex(AES_decrypt(ctx[0xB8]) ++ ...)` -- worth noting only because the crash lands
    /// inside AES-NI code, which is suggestive and not evidence.
    pub ersc_lobby_key_observer: bool,
    /// How destinations are judged.
    pub mode: LocalInvasionMode,
    /// Place-name text ids accepted in [`LocalInvasionMode::NamedOnly`], and -- see [`Self::judge`]
    /// -- accepted in every other mode too, because this is the list Shift+Insert appends to.
    pub named_location_text_ids: BTreeSet<PlaceNameTextId>,
    /// Human-readable names the user typed. Kept alongside the ids so the DLL can resolve them
    /// against the game's FMG at runtime and report which ones did not resolve -- a name that
    /// silently matches nothing is the failure mode this exists to make visible.
    pub named_locations: Vec<String>,
    /// Exact blocks the user marked with Insert. Widens whatever the mode allows.
    /// Virtual-key code that marks the location the player is standing in.
    ///
    /// Configurable because the historical default `VK_INSERT` does not exist on a 60% keyboard,
    /// which locked the whole marking feature out for anyone using one.
    pub mark_key: crate::keybind::VirtualKey,
    /// Virtual-key code that un-marks it.
    pub unmark_key: crate::keybind::VirtualKey,
    /// Virtual-key code for "the nearest invasion point that is not the one under our feet".
    ///
    /// Configurable for a sharper reason than the mark keys. This was hard-coded to `VK_F7`, and
    /// so was a feature in another mod loaded in the same me3 profile -- one keypress reached both,
    /// and a live session warped when the player meant the other thing. Neither side had a config
    /// key to move, so the only fix available to the player was unloading a mod.
    pub warp_nearest_key: crate::keybind::VirtualKey,
    /// Virtual-key code for "the next point in the catalog's stable order".
    pub warp_next_key: crate::keybind::VirtualKey,
    /// Virtual-key code for "the first point in a DIFFERENT area".
    pub warp_other_area_key: crate::keybind::VirtualKey,
    pub allowed_blocks: BTreeSet<u32>,
    /// Exact blocks the user excluded with Delete. Overrides everything, including a mode that
    /// would otherwise accept them.
    ///
    /// This is the third state the world map needs. `allowed` and "not allowed" are only two, and
    /// deriving the third from [`Self::judge`] against the player's current position made the map
    /// answer a question it should not: standing in a hub, every destination is a `WrongPlaceName`
    /// reject, so the whole map rendered one uniform tier and conveyed nothing. Chosen / untouched
    /// / excluded are properties OF A LOCATION, so they read the same from anywhere.
    pub blocked_blocks: BTreeSet<u32>,
}

impl Default for LocalInvasionConfig {
    fn default() -> Self {
        Self {
            // OFF. See the module docs: this cancels real matches, so it must be asked for.
            enabled: false,
            // OFF, and for a different reason than `enabled`. Hunt narrows the QUERY to a key only
            // this DLL's users publish, so while it is on a host without the DLL cannot be seen at
            // all. Losing reach is not something to inherit from a default.
            hunt: false,
            // OFF: it hides the entire vanilla population in both directions.
            dll_users_only: false,
            // OFF: a notification nobody asked for is spam.
            reject_notice: false,
            // ON: the pins are the point of the world-map half of this DLL.
            map_pins: true,
            // ON: location publishing and hunt mode both need these detours.
            steam_hooks: true,
            // OFF BY DEFAULT, on measured evidence rather than caution. These two detours are the
            // ONLY thing this DLL writes into `ersc.dll`, and arming them kills the game in ~25s.
            // Measured 2026-09-04 on the autoload route with everything else already ON: with them
            // off the process ran 251s and 110s with zero fault records across two arms; with them
            // on and NOTHING ELSE CHANGED it died at 24.9s with a fault at 0x140010043, no input
            // given. Five earlier faults at that address land in a 43.8-56.1s window, so this is
            // the same bug rather than a new one.
            //
            // Turning them off costs the local-invasion filter its session lookup and the lobby-key
            // line, both of which are inert anyway while `enabled` is false. It does NOT cost the
            // warp keys, the catalog or the map pins, which is what a user of this DLL is here for.
            // Set it back to true only once the mechanism is identified -- it is still UNKNOWN, and
            // a switch is a mitigation, not a fix.
            ersc_observers: false,
            // ON: both halves of the pair, so the master switch alone reproduces the old behaviour.
            ersc_show_observer: true,
            ersc_lobby_key_observer: true,
            mode: LocalInvasionMode::ExactOnly,
            named_location_text_ids: BTreeSet::new(),
            named_locations: Vec::new(),
            // The historical keys stay the default, so an existing config and an existing player's
            // muscle memory both keep working without touching the file.
            mark_key: crate::keybind::VK_INSERT,
            unmark_key: crate::keybind::VK_DELETE,
            warp_nearest_key: crate::keybind::VK_F7,
            warp_next_key: crate::keybind::VK_F8,
            warp_other_area_key: crate::keybind::VK_F9,
            allowed_blocks: BTreeSet::new(),
            blocked_blocks: BTreeSet::new(),
        }
    }
}

/// Which of the three persistent states a location is in, independent of where the player stands.
///
/// This is what the world map colours by, and it is deliberately NOT derived from [`Verdict`]:
/// a verdict answers "would this match land right now, from here", which changes as the player
/// walks. A map needs an answer that does not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum LocationChoice {
    /// The user marked it with Insert.
    Chosen,
    /// Neither marked nor excluded -- an invasion location the user has no opinion about.
    #[default]
    Untouched,
    /// The user excluded it with Delete.
    Excluded,
}

impl LocalInvasionConfig {
    /// Judge one incoming match.
    ///
    /// Disabled short-circuits to [`KeepReason::FilterDisabled`] before anything else is
    /// considered, so a misconfigured filter cannot reject matches while switched off.
    ///
    /// # Why marks are checked before the mode
    ///
    /// The Insert / Shift+Insert hotkeys exist so a player can say "and here too" without editing
    /// a file or thinking about which mode they are in. So a mark WIDENS: it can only ever turn a
    /// reject into a keep, never the reverse. If marks were folded into the mode instead, marking
    /// a place while in `exact` mode would do nothing visible and the keys would appear broken.
    ///
    /// Marks never reject on their own. An empty mark list therefore leaves the mode entirely in
    /// charge, and `named` mode with an empty list still fails closed further down.
    #[must_use]
    pub fn judge(&self, anchor: &InvasionAnchor, candidate: &InvasionCandidate) -> Verdict {
        if !self.enabled {
            return Verdict::Keep(KeepReason::FilterDisabled);
        }
        // Excluded beats chosen. They cannot both hold via the hotkeys -- each key clears the
        // other list -- but a hand-edited file can say both, and refusing is the fail-closed
        // reading of a contradiction.
        if self.blocked_blocks.contains(&candidate.block) {
            return Verdict::Reject(RejectReason::ExcludedByUser);
        }
        if self.allowed_blocks.contains(&candidate.block) {
            return Verdict::Keep(KeepReason::MarkedBlock);
        }
        if candidate.shares_a_name_with(&self.named_location_text_ids) {
            return Verdict::Keep(KeepReason::NamedLocation);
        }
        match self.mode {
            LocalInvasionMode::ExactOnly => {
                if candidate.block == anchor.block {
                    Verdict::Keep(KeepReason::ExactBlock)
                } else {
                    Verdict::Reject(RejectReason::WrongBlock)
                }
            }
            LocalInvasionMode::PreferExactThenArea => {
                if candidate.block == anchor.block {
                    return Verdict::Keep(KeepReason::ExactBlock);
                }
                if anchor.place_names.is_empty() {
                    return Verdict::Reject(RejectReason::NothingToMatchAgainst);
                }
                if candidate.place_names.is_empty() {
                    return Verdict::Reject(RejectReason::CandidateUnnamed);
                }
                if candidate.shares_a_name_with(&anchor.place_names) {
                    Verdict::Keep(KeepReason::SharedPlaceName)
                } else {
                    Verdict::Reject(RejectReason::WrongPlaceName)
                }
            }
            LocalInvasionMode::NamedOnly => {
                if self.named_location_text_ids.is_empty() {
                    return Verdict::Reject(RejectReason::NothingToMatchAgainst);
                }
                if candidate.place_names.is_empty() {
                    return Verdict::Reject(RejectReason::CandidateUnnamed);
                }
                // A listed name was already accepted above; reaching here means it was not listed.
                Verdict::Reject(RejectReason::NotNamed)
            }
        }
    }

    /// What the map should say about a location, with no reference to where the player is.
    #[must_use]
    pub fn choice_for(&self, block: u32) -> LocationChoice {
        if self.blocked_blocks.contains(&block) {
            LocationChoice::Excluded
        } else if self.allowed_blocks.contains(&block) {
            LocationChoice::Chosen
        } else {
            LocationChoice::Untouched
        }
    }

    /// Insert: "I want to invade here." Clears any exclusion, so the two keys are inverses rather
    /// than two ways of reaching the same stuck state. Returns whether anything changed, so a
    /// hotkey can stay silent on a repeat press instead of rewriting the file every frame.
    pub fn mark_block(&mut self, block: u32) -> bool {
        let unblocked = self.blocked_blocks.remove(&block);
        self.allowed_blocks.insert(block) || unblocked
    }

    /// Delete: "I do not want to invade here." Clears any mark and records the exclusion.
    ///
    /// This used to merely un-mark, which made Delete a no-op on anything the user had not already
    /// chosen -- and left no way at all to say "not here". Excluding is the useful meaning, and it
    /// is what gives the world map its third tier.
    pub fn unmark_block(&mut self, block: u32) -> bool {
        let unmarked = self.allowed_blocks.remove(&block);
        self.blocked_blocks.insert(block) || unmarked
    }

    /// Add every place name the anchor carries -- "this place and everywhere sharing its name".
    ///
    /// Returns how many were newly added. An anchor with no resolved names adds nothing and
    /// reports 0, which is what lets the caller say "nothing to mark here" rather than claiming a
    /// mark that would match nothing.
    pub fn mark_place_names(&mut self, anchor: &InvasionAnchor) -> usize {
        anchor
            .place_names
            .iter()
            .filter(|id| self.named_location_text_ids.insert(**id))
            .count()
    }

    /// Remove every place name the anchor carries. Returns how many were actually removed.
    pub fn unmark_place_names(&mut self, anchor: &InvasionAnchor) -> usize {
        anchor
            .place_names
            .iter()
            .filter(|id| self.named_location_text_ids.remove(*id))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_destination_matches_on_any_of_its_names_not_just_the_lowest_numbered_one() {
        // THE TRUNCATION BUG, fixed 2026-08-06. The candidate used to carry ONE name, taken as
        // `.first()` of the resolved list -- over a `BTreeSet` that is the numerically smallest id,
        // which has no meaning. A destination sitting under two names, only the higher of which the
        // anchor shares, was rejected as `WrongPlaceName` while the anchor side compared against
        // every name it had. The comparison was one-sided, so the same pair of locations judged
        // differently depending on which one the player stood in.
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..LocalInvasionConfig::default()
        };
        let anchor = InvasionAnchor::new(0x0f00_0000, [900]);
        // 100 is the lower id and does NOT match; 900 does. Truncation would have kept only 100.
        let two_names = InvasionCandidate::new(0x2000_0000, [100, 900]);
        assert_eq!(
            config.judge(&anchor, &two_names),
            Verdict::Keep(KeepReason::SharedPlaceName),
            "a shared name must match whichever position it holds"
        );
    }

    #[test]
    fn the_name_comparison_is_symmetric_between_anchor_and_destination() {
        // Same two locations, roles swapped. Both directions must agree, which is the property
        // truncating one side silently broke.
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..LocalInvasionConfig::default()
        };
        let here = (0x0f00_0000u32, [100, 900]);
        let there = (0x2000_0000u32, [900, 4200]);
        let forward = config.judge(
            &InvasionAnchor::new(here.0, here.1),
            &InvasionCandidate::new(there.0, there.1),
        );
        let backward = config.judge(
            &InvasionAnchor::new(there.0, there.1),
            &InvasionCandidate::new(here.0, here.1),
        );
        assert_eq!(
            forward, backward,
            "judging must not depend on which end you stand at"
        );
        assert_eq!(forward, Verdict::Keep(KeepReason::SharedPlaceName));
    }

    #[test]
    fn a_configured_named_location_matches_any_of_the_destinations_names() {
        // The `named_location_text_ids` path had the same truncation: it tested one id against the
        // list, so a listed name in any other position was missed.
        let mut config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::NamedOnly,
            ..LocalInvasionConfig::default()
        };
        config.named_location_text_ids.insert(900);
        assert_eq!(
            config.judge(
                &InvasionAnchor::new(0x0f00_0000, []),
                &InvasionCandidate::new(0x2000_0000, [100, 900]),
            ),
            Verdict::Keep(KeepReason::NamedLocation)
        );
    }

    #[test]
    fn an_unresolvable_name_is_still_refused_rather_than_treated_as_a_wildcard() {
        // The fix must not turn "we could not look it up" into "it matches". An unnamed
        // destination is unknown, not universal -- accepting it would land the player somewhere
        // they explicitly filtered against, which is worse than the rejection it replaces.
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..LocalInvasionConfig::default()
        };
        assert_eq!(
            config.judge(
                &InvasionAnchor::new(0x0f00_0000, [900]),
                &InvasionCandidate::unnamed(0x2000_0000),
            ),
            Verdict::Reject(RejectReason::CandidateUnnamed)
        );
    }

    fn anchor() -> InvasionAnchor {
        InvasionAnchor::new(0x0f00_0000, [100, 200])
    }

    #[test]
    fn disabled_keeps_everything_even_with_a_mode_that_would_reject() {
        let config = LocalInvasionConfig {
            enabled: false,
            mode: LocalInvasionMode::ExactOnly,
            ..Default::default()
        };
        let far = InvasionCandidate::named(0x3c35_3800, 999);
        assert_eq!(
            config.judge(&anchor(), &far),
            Verdict::Keep(KeepReason::FilterDisabled),
            "a switched-off filter must never cancel someone's match"
        );
    }

    #[test]
    fn the_default_is_off() {
        assert!(
            !LocalInvasionConfig::default().enabled,
            "this cancels real matches; it has to be asked for, not inherited"
        );
    }

    #[test]
    fn exact_mode_takes_the_block_and_nothing_else() {
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::ExactOnly,
            ..Default::default()
        };
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x0f00_0000, 100)),
            Verdict::Keep(KeepReason::ExactBlock)
        );
        // Same NAME, different block: exact mode still refuses.
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x0f01_0000, 100)),
            Verdict::Reject(RejectReason::WrongBlock)
        );
    }

    #[test]
    fn area_mode_takes_exact_first_then_a_shared_name() {
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..Default::default()
        };
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x0f00_0000, 777)),
            Verdict::Keep(KeepReason::ExactBlock),
            "the exact block passes regardless of its name"
        );
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x0f02_0000, 200)),
            Verdict::Keep(KeepReason::SharedPlaceName)
        );
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x0f02_0000, 300)),
            Verdict::Reject(RejectReason::WrongPlaceName)
        );
    }

    #[test]
    fn an_anchor_reports_how_many_places_it_makes_acceptable() {
        assert_eq!(InvasionAnchor::new(1, [100]).named_location_count(), 1);
        assert_eq!(
            InvasionAnchor::new(1, [100, 200, 300, 400, 500]).named_location_count(),
            5,
            "five names means five places to look"
        );
    }

    #[test]
    fn place_name_none_is_never_stored_as_an_anchor_name() {
        let anchor = InvasionAnchor::new(1, [PLACE_NAME_NONE, 100, PLACE_NAME_NONE]);
        assert_eq!(anchor.named_location_count(), 1);
        assert!(!anchor.place_names.contains(&PLACE_NAME_NONE));
    }

    #[test]
    fn an_unnamed_candidate_is_rejected_not_waved_through() {
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..Default::default()
        };
        assert_eq!(
            config.judge(
                &anchor(),
                &InvasionCandidate::named(0x2000_0000, PLACE_NAME_NONE)
            ),
            Verdict::Reject(RejectReason::CandidateUnnamed),
            "no resolvable name means unknown, not universal"
        );
    }

    #[test]
    fn name_matching_with_nothing_to_match_against_fails_closed() {
        let no_names = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..Default::default()
        };
        assert_eq!(
            no_names.judge(
                &InvasionAnchor::new(0x0f00_0000, []),
                &InvasionCandidate::named(0x2000_0000, 100)
            ),
            Verdict::Reject(RejectReason::NothingToMatchAgainst),
        );
        let empty_list = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::NamedOnly,
            ..Default::default()
        };
        assert_eq!(
            empty_list.judge(&anchor(), &InvasionCandidate::named(0x0f00_0000, 100)),
            Verdict::Reject(RejectReason::NothingToMatchAgainst),
            "named mode with an empty list must not silently accept everything"
        );
    }

    #[test]
    fn a_marked_block_is_accepted_whatever_the_mode_says() {
        // Insert marks where you stand. It has to work in the strictest mode too, or the key
        // appears dead to anyone who has not changed `mode`.
        let mut config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::ExactOnly,
            ..Default::default()
        };
        let far = InvasionCandidate::named(0x3c35_3800, 999);
        assert_eq!(
            config.judge(&anchor(), &far),
            Verdict::Reject(RejectReason::WrongBlock)
        );
        assert!(
            config.mark_block(0x3c35_3800),
            "first mark changes something"
        );
        assert_eq!(
            config.judge(&anchor(), &far),
            Verdict::Keep(KeepReason::MarkedBlock)
        );
        assert!(
            !config.mark_block(0x3c35_3800),
            "a repeat press changes nothing"
        );
        assert!(config.unmark_block(0x3c35_3800));
        assert_eq!(
            config.judge(&anchor(), &far),
            Verdict::Reject(RejectReason::ExcludedByUser),
            "Delete now EXCLUDES rather than reverting to neutral -- it used to be a no-op on \
             anything not already chosen, which left no way to say \"not here\""
        );
        // The two keys are inverses: Insert on an excluded location takes it back.
        assert!(config.mark_block(0x3c35_3800));
        assert_eq!(
            config.judge(&anchor(), &far),
            Verdict::Keep(KeepReason::MarkedBlock)
        );
        assert!(
            config.blocked_blocks.is_empty(),
            "Insert clears the exclusion"
        );
    }

    #[test]
    fn the_three_location_states_do_not_depend_on_where_the_player_is() {
        // The whole reason this exists. Deriving the map's tiers from `judge` made them a function
        // of the player's position, so standing in a hub painted every pin the same tier.
        let mut config = LocalInvasionConfig {
            enabled: true,
            ..Default::default()
        };
        config.mark_block(0x0f00_0000);
        config.unmark_block(0x3c35_3800);
        assert_eq!(config.choice_for(0x0f00_0000), LocationChoice::Chosen);
        assert_eq!(config.choice_for(0x3c35_3800), LocationChoice::Excluded);
        assert_eq!(config.choice_for(0x1234_0000), LocationChoice::Untouched);
        // No anchor is involved at all -- there is no parameter for one.
        assert_eq!(
            LocalInvasionConfig::default().choice_for(0x0f00_0000),
            LocationChoice::Untouched
        );
    }

    #[test]
    fn an_exclusion_beats_a_mode_that_would_otherwise_accept() {
        let mut config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::PreferExactThenArea,
            ..Default::default()
        };
        let same_name = InvasionCandidate::named(0x0f02_0000, 200);
        assert_eq!(
            config.judge(&anchor(), &same_name),
            Verdict::Keep(KeepReason::SharedPlaceName)
        );
        config.unmark_block(0x0f02_0000);
        assert_eq!(
            config.judge(&anchor(), &same_name),
            Verdict::Reject(RejectReason::ExcludedByUser),
            "an explicit \"not here\" has to outrank the mode, or Delete would be advisory"
        );
    }

    #[test]
    fn shift_insert_marks_every_name_the_anchor_carries() {
        let mut config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::ExactOnly,
            ..Default::default()
        };
        // anchor() carries 100 and 200: "two names, two places to look".
        assert_eq!(config.mark_place_names(&anchor()), 2);
        assert_eq!(config.mark_place_names(&anchor()), 0, "already marked");
        let elsewhere_same_name = InvasionCandidate::named(0x9999_0000, 200);
        assert_eq!(
            config.judge(&anchor(), &elsewhere_same_name),
            Verdict::Keep(KeepReason::NamedLocation),
            "a marked name has to widen exact mode, not sit inert until the mode changes"
        );
        assert_eq!(config.unmark_place_names(&anchor()), 2);
        assert_eq!(
            config.judge(&anchor(), &elsewhere_same_name),
            Verdict::Reject(RejectReason::WrongBlock)
        );
    }

    #[test]
    fn an_anchor_with_no_resolved_names_marks_nothing_and_says_so() {
        let mut config = LocalInvasionConfig {
            enabled: true,
            ..Default::default()
        };
        let nameless = InvasionAnchor::new(0x0f00_0000, [PLACE_NAME_NONE]);
        assert_eq!(
            config.mark_place_names(&nameless),
            0,
            "marking nothing must report 0, not claim a mark that would match nothing"
        );
        assert!(config.named_location_text_ids.is_empty());
    }

    #[test]
    fn marking_does_not_override_the_master_switch() {
        let mut config = LocalInvasionConfig::default();
        config.mark_block(0x3c35_3800);
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x1234_0000, 7)),
            Verdict::Keep(KeepReason::FilterDisabled),
            "marks widen a filter that is ON; they must not switch one on"
        );
    }

    #[test]
    fn named_mode_ignores_the_anchor_entirely() {
        let config = LocalInvasionConfig {
            enabled: true,
            mode: LocalInvasionMode::NamedOnly,
            named_location_text_ids: [500].into_iter().collect(),
            named_locations: vec!["Somewhere Else".to_owned()],
            allowed_blocks: BTreeSet::new(),
            blocked_blocks: BTreeSet::new(),
            ..LocalInvasionConfig::default()
        };
        // The anchor's own block, but not a listed name -> rejected.
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x0f00_0000, 100)),
            Verdict::Reject(RejectReason::NotNamed)
        );
        // Far from the anchor, but listed -> kept.
        assert_eq!(
            config.judge(&anchor(), &InvasionCandidate::named(0x3c35_3800, 500)),
            Verdict::Keep(KeepReason::NamedLocation)
        );
    }

    #[test]
    fn mode_spellings_round_trip_and_unknown_is_reported_not_defaulted() {
        for mode in [
            LocalInvasionMode::ExactOnly,
            LocalInvasionMode::PreferExactThenArea,
            LocalInvasionMode::NamedOnly,
        ] {
            assert_eq!(LocalInvasionMode::parse(mode.as_str()), Some(mode));
        }
        assert_eq!(
            LocalInvasionMode::parse("EXACT"),
            Some(LocalInvasionMode::ExactOnly)
        );
        assert_eq!(
            LocalInvasionMode::parse("prefer-exact-then-area"),
            Some(LocalInvasionMode::PreferExactThenArea)
        );
        assert_eq!(
            LocalInvasionMode::parse("whatever"),
            None,
            "an unknown mode must surface as an error, not become a mode the user did not choose"
        );
    }
}
