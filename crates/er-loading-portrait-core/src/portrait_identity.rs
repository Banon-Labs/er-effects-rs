//! Pure decision logic for the loading-screen portrait's character-identity semaphores.
//!
//! Split out of the DLL so the rules that decide "is the portrait showing the right character"
//! are host-`cargo test`-able instead of only reachable through a game launch. Nothing here
//! reads game memory; the callers pass in values they already read.
//!
//! Two independent questions live here:
//!
//! 1. **Is a packed map id worth comparing at all** ([`packed_map_is_plausible`])? The old gate
//!    was `map > 0`, which is meaningless for a packed `BlockId` — its high byte is the areaId,
//!    so any area >= 0x80 reads negative and silently switched the map comparison off.
//! 2. **Did the portrait we published match the character that actually loaded**
//!    ([`published_identity_verdict`])? This is the comparison the loading screen's pixels
//!    depend on, and nothing asserted it before (bd er-effects-rs-qoqc / er-effects-rs-91zb: a
//!    wrong face was on screen for 29.7s with every existing oracle reporting ok).

/// New-game / not-yet-resolved saved-map sentinel (`m10_01_00_00`). Excluded from every map
/// comparison so a transient `c30` during a loading screen cannot false-fire.
pub const DEFAULT_MAP_C30: i32 = 0x0a01_0000;

/// Lowest areaId seen on a real ER map. A packed `BlockId` is `{indexId, regionId, blockId,
/// areaId}` little-endian, so the areaId is the high byte of the dword.
pub const MAP_AREA_ID_MIN: u8 = 0x0a;
/// Highest areaId seen on a real ER map (DLC included).
pub const MAP_AREA_ID_MAX: u8 = 0x3d;

/// The areaId of a packed `BlockId` (its high byte).
#[must_use]
pub const fn packed_map_area_id(map: i32) -> u8 {
    ((map as u32) >> 24) as u8
}

/// True when `map` looks like a real packed `BlockId` worth comparing against another one.
///
/// Replaces the `map > 0` sign gate. A `BlockId`'s sign bit is just bit 7 of the areaId and
/// carries no meaning; treating it as a validity flag meant a garbage word with bit31 set turned
/// the map axis off instead of failing it. Empirically every one of 726 active corpus slots has
/// an areaId in `MAP_AREA_ID_MIN..=MAP_AREA_ID_MAX` (pinned by er-save-loader's corpus test),
/// while random noise clears it under 10% of the time.
#[must_use]
pub const fn packed_map_is_plausible(map: i32) -> bool {
    let area = packed_map_area_id(map);
    area >= MAP_AREA_ID_MIN && area <= MAP_AREA_ID_MAX && map != DEFAULT_MAP_C30
}

/// Whether the two map ids are comparable and disagree. Only a mismatch between two plausible
/// maps counts; an implausible value on either side means "no map evidence", never "mismatch".
#[must_use]
pub const fn packed_maps_disagree(ours: i32, live: i32) -> bool {
    packed_map_is_plausible(ours) && packed_map_is_plausible(live) && ours != live
}

/// Why a live `CS::ProfileSummary` record is, or is not, a character.
///
/// The loading-screen stats panel reads its name and Rune Level straight out of the slot's record,
/// and until 2026-08-30 it trusted whatever bytes were there. Those bytes are not always a
/// character: the in-game save picker deliberately writes its browse-row labels into the same
/// records (`save_picker_write_row_records` zeroes each 0x2a0-byte record and copies the row label
/// into the name field), and until the restore was fixed they could still be there when a loading
/// screen came up. The user saw the result on their own loading screens -- `[..] EldenRing` (the
/// picker's parent-directory row) and `[ new ]` (`PICKER_NEW_FILE_LABEL`) rendered as character
/// names beside `RL 0`.
///
/// So the panel asks this question first and draws nothing when the answer is no, exactly as the
/// portrait already refuses to publish a head it cannot attribute. A blank panel is a visible,
/// diagnosable absence; a confidently wrong one is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordCharacterVerdict {
    /// Name and level both present and self-consistent -- safe to render.
    Character,
    /// The name field is empty (or whitespace). A zeroed record reads exactly like this.
    EmptyName,
    /// `level <= 0`. The load-bearing TERM: every record the picker stages is memset to zero
    /// before the label is copied in, so its level is 0 and no browse-row label can pass here.
    NoLevel,
    /// The name has the shape of one of the picker's own row labels. Belt-and-braces only --
    /// [`Self::NoLevel`] already rejects every staged row -- but it costs one `starts_with` and it
    /// is the term that would still catch a label written over a record that kept its level.
    PickerRowLabel,
    /// The record's map word is populated but is not a packed `BlockId`, so the record is neither a
    /// character nor a zeroed slot: it is garbage. A brand-new character's `DEFAULT_MAP_C30` and a
    /// zero word are deliberately not refutations -- both are legitimate states of a real record,
    /// and rejecting the sentinel would blank the panel for a character who has not left the
    /// tutorial cave.
    ImplausibleMap,
}

impl RecordCharacterVerdict {
    /// True only for [`Self::Character`], so a caller can gate without matching every arm.
    #[must_use]
    pub const fn is_character(self) -> bool {
        matches!(self, Self::Character)
    }

    /// Short stable tag for logs and telemetry.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Character => "character",
            Self::EmptyName => "empty-name",
            Self::NoLevel => "no-level",
            Self::PickerRowLabel => "picker-row-label",
            Self::ImplausibleMap => "implausible-map",
        }
    }
}

/// Does this live `CS::ProfileSummary` record describe a real character?
///
/// `name` / `level` / `map` are the record's name field, `+0x24` level and `+0x30` packed map, read
/// by the caller. Nothing here touches game memory, so the rule is host-testable -- which is the
/// point: this is the rule a wrong-stats bug hides in.
#[must_use]
pub fn profile_record_character_verdict(
    name: &str,
    level: i32,
    map: i32,
) -> RecordCharacterVerdict {
    if name.trim().is_empty() {
        return RecordCharacterVerdict::EmptyName;
    }
    if level <= 0 {
        return RecordCharacterVerdict::NoLevel;
    }
    if name_looks_like_picker_row_label(name) {
        return RecordCharacterVerdict::PickerRowLabel;
    }
    if map != 0 && map != DEFAULT_MAP_C30 && !packed_map_is_plausible(map) {
        return RecordCharacterVerdict::ImplausibleMap;
    }
    RecordCharacterVerdict::Character
}

/// True when `name` has the shape of one of the save picker's browse-row labels rather than a
/// character name.
///
/// Matches what `SavePickerModel::row_label_utf16` can produce: every control row is bracketed
/// (`[..] <dir>`, `[ new ]`, `[ root ]`, `[ .. up ]`, `[ SCROLL ^ ]`, `[ SCROLL v ]`, and the drive
/// strip's `[C:]`-style label), and every directory row ends in a separator (`dir_display_name`
/// appends `/`, or renders a drive root such as `Z:\`). A file row's label is a bare filename and
/// is deliberately not matched here -- `ER0000.sl2` is indistinguishable in shape from a legal
/// character name, and inventing a rule that rejects names containing a dot would blank the panel
/// for real characters. The level term is what rejects file rows.
///
/// The predicate is intentionally shape-only and lives here rather than importing
/// `er-save-picker-core`: this crate must stay free of the picker's dependency graph, and a
/// duplicated 3-line shape check is cheaper than the coupling.
#[must_use]
pub fn name_looks_like_picker_row_label(name: &str) -> bool {
    let trimmed = name.trim();
    trimmed.starts_with('[') || trimmed.ends_with('/') || trimmed.ends_with('\\')
}

/// One live `CS::ProfileSummary` slot, as the scan reads it: the game's own occupancy byte
/// (`summary+0x8+slot`) and the three record fields [`profile_record_character_verdict`] judges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveRecordSample<'a> {
    /// `saveSlotsStates[slot] != 0` -- the game's answer to "does this slot produce a row".
    pub occupied: bool,
    /// The record's name field (`record+0x00`, UTF-16, decoded by the caller).
    pub name: &'a str,
    /// The record's Rune Level (`record+0x24`).
    pub level: i32,
    /// The record's packed saved map / `BlockId` (`record+0x30`).
    pub map: i32,
}

/// What one sweep of the live record table found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LiveRecordScan {
    /// Bit N = slot N is marked occupied while holding something that is not a character.
    pub orphaned_mask: u32,
    /// How many slots held a real character. Zero is not a clean table -- it is an unread one, or
    /// one whose every character record has been overwritten. The caller decides which, because
    /// only the caller knows whether the table was ever seen populated; see the note on
    /// [`LiveRecordScan::orphaned_mask`]'s consumer.
    pub characters: usize,
}

/// Sweep the live record table: which slots are marked occupied while holding something that is not
/// a character -- the durable RAM signature of `er-effects-rs-fmy6`.
///
/// The defect this names. The in-game save picker renders its browse rows by writing them into
/// these records: `save_picker_write_row_records` zeroes each `0x2a0`-byte record, copies the row
/// label into the name field, and sets the occupancy byte. Every exit is supposed to put the game's
/// own records back. When one did not (the sticky-`committed` defect, 2026-08-29), the labels sat
/// in a game-owned structure for the rest of the process, and the user's next loading screens
/// rendered `[..] EldenRing` and `[ new ]` as character names beside `RL 0`. That reached the user
/// as a visual observation while both halves of the evidence were already in telemetry and nothing
/// compared them -- which is the failure this function exists to make impossible to repeat.
///
/// Occupancy is the load-bearing term, and it is what keeps the mask a defect rather than a state.
/// A save with three characters leaves seven records zeroed, and a zeroed record is not a
/// character; without the occupancy term this would read `0b1111111000` on a perfectly healthy
/// boot. The game produces no row for an unoccupied slot (`FUN_140875590` appends only where
/// `ProfileSummary::saveSlotsStates[slot]` is set), so an unoccupied slot's bytes describe nothing
/// on screen and cannot be the defect.
///
/// `characters` is reported, not acted on, and that separation is deliberate. The allocation exists
/// well before `CS::ProfileSummary::Deserialize` fills it, so an unread table's bytes must not be
/// judged. The obvious rule -- "answer 0 unless this sample holds a character" -- is wrong, and
/// wrong in the exact case that matters: a picker staging four browse rows marks slots 4..9
/// unoccupied and zeroes all ten records, so during the defect no slot holds a character and that
/// rule would blind the oracle precisely when it fires. Whether the table has ever been read is
/// process-lifetime state; the caller latches it from `characters` and gates on the latch.
///
/// Slots past `u32::BITS` cannot be represented and are ignored; the table is ten.
#[must_use]
pub fn scan_live_records<'a>(
    records: impl IntoIterator<Item = LiveRecordSample<'a>>,
) -> LiveRecordScan {
    let mut scan = LiveRecordScan::default();
    for (index, sample) in records.into_iter().enumerate() {
        if index >= u32::BITS as usize {
            break;
        }
        let is_character =
            profile_record_character_verdict(sample.name, sample.level, sample.map).is_character();
        if is_character {
            scan.characters += 1;
        }
        if sample.occupied && !is_character {
            scan.orphaned_mask |= 1u32 << index;
        }
    }
    scan
}

/// What the published loading-screen portrait was compared against the character that loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishedIdentity {
    /// Nothing was published in this window — not a mismatch, just no evidence.
    NothingPublished,
    /// No load completed to compare against (no deserialize, no confirmed slot).
    NoLoadedSlot,
    /// Published portrait belongs to the slot that loaded.
    Match,
    /// Published portrait belongs to a different slot than the one that loaded. This is the
    /// 29.7s-wrong-face class.
    SlotMismatch { published: i32, loaded: i32 },
}

impl PublishedIdentity {
    /// True only for a genuine wrong-character publish, so callers can bump a fail counter
    /// without also counting "we had nothing to check".
    #[must_use]
    pub const fn is_mismatch(self) -> bool {
        matches!(self, Self::SlotMismatch { .. })
    }
}

/// Compare the published portrait's slot against the slot whose load actually completed.
///
/// `published_slot_tag` is the wire form stored in `LS_PORTRAIT_PUBLISHED_SLOT`: **slot + 1**, so
/// 0 means "never published". `loaded_slot` is `None` when no load has completed yet.
///
/// The +1 biasing is the whole reason this needs a test: the counter cannot distinguish "slot 0"
/// from "unset" without it, and an off-by-one here would either hide every mismatch or invent one
/// on every boot.
#[must_use]
pub fn published_identity_verdict(
    published_slot_tag: usize,
    loaded_slot: Option<i32>,
) -> PublishedIdentity {
    let Some(published) = published_slot_tag.checked_sub(1) else {
        return PublishedIdentity::NothingPublished;
    };
    let Some(loaded) = loaded_slot else {
        return PublishedIdentity::NoLoadedSlot;
    };
    let published = published as i32;
    if published == loaded {
        PublishedIdentity::Match
    } else {
        PublishedIdentity::SlotMismatch { published, loaded }
    }
}

/// The slot the portrait pipeline should target while a load is in flight, given the three
/// sources that can name it. Higher precedence first:
///
/// 1. `picker_slot` — the user's explicit on-screen pick. Nothing the game infers outranks what
///    the user selected, and the pick is known before any of the others settle.
/// 2. `request_slot` — `GameMan+0xb78`, the native load-request register. When it names a slot
///    different from `save_slot`, a load for that slot is in flight, so `save_slot` is stale by
///    definition.
/// 3. `save_slot` — `GameMan.save_slot` (ac0). Last resort: both the game's own selector and our
///    own `set_save_slot` write it for reasons unrelated to "which character is loading", so it
///    is the weakest of the three (bd er-effects-rs-91zb).
///
/// `None` when no source names a valid slot — callers must not collapse that to slot 0.
#[must_use]
pub fn portrait_target_slot_from_sources(
    picker_slot: Option<i32>,
    request_slot: Option<i32>,
    save_slot: Option<i32>,
    slot_count: i32,
) -> Option<i32> {
    portrait_target_slot_attributed(picker_slot, request_slot, save_slot, slot_count)
        .map(|(slot, _)| slot)
}

/// Which source named a portrait target, ordered weakest evidence first.
///
/// The ordering is the whole point. [`portrait_target_slot_from_sources`] throws the winning
/// source away and returns a bare slot, so a target read off `save_slot` — a register both the
/// game's own selector and our loader write for reasons unrelated to "which character is
/// loading" — was indistinguishable from one the user explicitly clicked. The window latch then
/// defended both equally, and a stale `save_slot` outranked the load request that superseded it
/// for the whole window (bd `er-effects-rs-fmy6`: latched slot 0 from `ac0=0` at +56125ms, `b78`
/// and `ac0` both read the real slot 1 at +60296ms, retarget suppressed, wrong face on screen).
///
/// A configured hint is deliberately not a variant here. The autoload's `slot=` setting is not a
/// measurement of anything — it says what a config file asked for, not what the game is doing —
/// and giving it a rank would let it commit a window exactly the way the `.or_else` hint fallback
/// used to (run br-20260831-014208-b1d6: latched slot 0 at +1184ms with `picker=None b78=None
/// ac0=-1`, i.e. from the hint alone, then refused the real `ac0=2` 3371 times).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortraitSlotSource {
    /// `GameMan.save_slot` (ac0). Weakest: written for reasons unrelated to the question.
    SaveSlot,
    /// `GameMan+0xb78`, the native load-request register. A load for it is in flight, so
    /// `save_slot` is stale by definition.
    RequestSlot,
    /// The user's explicit on-screen pick. Nothing the game infers outranks it.
    UserPick,
}

impl PortraitSlotSource {
    /// Wire form for the `PORTRAIT_WINDOW_TARGET_SOURCE` atomic. `0` means "no latch"; the ranks
    /// are 1-based so the unset atomic cannot be mistaken for the weakest real source.
    #[must_use]
    pub const fn rank(self) -> usize {
        match self {
            Self::SaveSlot => 1,
            Self::RequestSlot => 2,
            Self::UserPick => 3,
        }
    }

    /// Inverse of [`Self::rank`]. `None` for `0` and for any value that is not a known rank.
    #[must_use]
    pub const fn from_rank(rank: usize) -> Option<Self> {
        match rank {
            1 => Some(Self::SaveSlot),
            2 => Some(Self::RequestSlot),
            3 => Some(Self::UserPick),
            _ => None,
        }
    }

    /// Short label for the debug/crash log lines. The provenance of a latch is what the
    /// br-20260831-014208-b1d6 diagnosis had to recover by hand from a line 12,000 lines away.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SaveSlot => "ac0",
            Self::RequestSlot => "b78",
            Self::UserPick => "pick",
        }
    }
}

/// [`portrait_target_slot_from_sources`], keeping the source that won.
///
/// Same precedence, same validity filter; the only difference is that the answer says where it
/// came from, so the window latch can tell a measured fact from a weaker measured fact.
#[must_use]
pub fn portrait_target_slot_attributed(
    picker_slot: Option<i32>,
    request_slot: Option<i32>,
    save_slot: Option<i32>,
    slot_count: i32,
) -> Option<(i32, PortraitSlotSource)> {
    let valid = |s: Option<i32>| s.filter(|v| (0..slot_count).contains(v));
    valid(picker_slot)
        .map(|slot| (slot, PortraitSlotSource::UserPick))
        .or_else(|| valid(request_slot).map(|slot| (slot, PortraitSlotSource::RequestSlot)))
        .or_else(|| valid(save_slot).map(|slot| (slot, PortraitSlotSource::SaveSlot)))
}

/// Which source names the slot whose character the loading-screen stats panel describes.
///
/// Extracted from `read_loading_screen_stats` so the answer is testable without game memory: the
/// panel rendering the wrong character is a decision bug, and it was one (2026-08-26 -- picked slot
/// 1, panel showed slot 0's `angrE RL 100`). `BestActiveFallback` is a distinct outcome rather than
/// a resolved slot because computing it is an unsafe scan the caller must not pay for when a
/// stronger source already answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsSlotSource {
    /// A System->Quit->Load switch selection, known at the confirm press -- before the deserialize
    /// flips `save_slot`, so it outranks everything the game infers.
    SwitchSelection(i32),
    /// This loading window's committed portrait target, i.e. the character on screen. Includes the
    /// user's boot pick, which reaches it through the window latch.
    PortraitWindow(i32),
    /// The slot this boot is configured to autoload, before anything has observed a load.
    ///
    /// Deliberately ranked below the window and deliberately not a portrait source. A configured
    /// hint is not an observation, and the portrait window pays for that distinction by rendering
    /// nothing until a register names a slot (see `portrait_loaded_slot_confirmed`). The stats
    /// panel's alternative is not "nothing" though -- it is `BestActiveFallback`, a scan that
    /// returns the highest-level slot and has no relationship to the load at all. Measured
    /// 2026-09-03 (run br-20260903-204517-82d2): slot 4 was configured and loading, and the panel
    /// rendered slot 0's `angrE RL 100` for the 1.2s before the window latched, because angrE is
    /// level 100. Between two guesses, the one that names the slot the loader was told to use is
    /// the one that turns out right.
    ConfiguredAutoload(i32),
    /// Nothing named a slot: the caller must scan for the best active one.
    BestActiveFallback,
}

/// Resolve the stats panel's slot source. `switch_selected_wire` is the raw
/// `SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT` word, whose unset form is `usize::MAX`.
///
/// The panel and the portrait must never disagree, so the second term is the window target rather
/// than a fresh precedence evaluation -- otherwise the face and the stats under it could name two
/// different characters within one loading screen.
///
/// `configured_autoload` is the third term and applies only while the first two are silent, which
/// on a boot autoload is the window between the config being read and a register naming the slot.
#[must_use]
pub fn loading_screen_stats_slot_source(
    switch_selected_wire: usize,
    window_target: Option<i32>,
    configured_autoload: Option<i32>,
    slot_count: i32,
) -> StatsSlotSource {
    let valid = |slot: i32| (0..slot_count).contains(&slot);
    if switch_selected_wire <= i32::MAX as usize {
        let selected = switch_selected_wire as i32;
        if valid(selected) {
            return StatsSlotSource::SwitchSelection(selected);
        }
    }
    if let Some(slot) = window_target.filter(|&slot| valid(slot)) {
        return StatsSlotSource::PortraitWindow(slot);
    }
    match configured_autoload.filter(|&slot| valid(slot)) {
        Some(slot) => StatsSlotSource::ConfiguredAutoload(slot),
        None => StatsSlotSource::BestActiveFallback,
    }
}

/// Stability over freshness, for the duration of one loading-screen window.
///
/// [`portrait_target_slot_from_sources`] is a precedence ordering evaluated fresh on every kick,
/// so its answer can change while a single loading screen is on screen — and when it does, the
/// face the user is looking at is replaced by a different character's mid-load. Measured
/// 2026-08-02 21:05: the user picked slot 0, the pipeline built and published slot 0 at
/// +17775ms, then the picker term expired (it is spent on `IN_WORLD_REACHED`, i.e. *a* world
/// existing, not *that slot's* world), precedence fell through `request_slot = -1` to
/// `save_slot = 9`, and kick #2 retargeted the same window to slot 9 at +20998ms. The window did
/// not close until +29989ms, so the user watched the portrait change out from under the character
/// they clicked.
///
/// This is the fix for that, and it is deliberately the smallest one that can work: once a window
/// has committed to a target, that target is what the window keeps. A newly-resolved slot is
/// adopted only when the window has no target yet.
///
/// `latched` is the target this window already committed to (`None` before the first resolution).
/// Returns the slot the window should use, plus whether it is newly latching — callers reset
/// `latched` to `None` on window close, which is what allows the next load to pick a new target.
///
/// It intentionally does not try to decide which slot is *correct*: that question belongs to the
/// load path, and a portrait that stays wrong for one window is strictly better than one that
/// changes identity while a user is looking at it.
#[must_use]
pub fn portrait_window_target_slot(
    latched: Option<i32>,
    resolved: Option<i32>,
) -> (Option<i32>, bool) {
    portrait_window_target_slot_authoritative(latched, false, resolved, false).into_pair()
}

/// What one window's portrait target should be, and whether it is newly latching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortraitWindowTarget {
    /// The slot this window should use, or `None` while nothing has named one.
    pub slot: Option<i32>,
    /// True when this call is what commits the window to `slot`.
    pub latching: bool,
    /// True when the commit replaced a latch that had been adopted from a guess, because the user
    /// has now explicitly picked. Counted as `PORTRAIT_WINDOW_TARGET_PICK_PROMOTIONS`.
    pub promoted_by_pick: bool,
    /// The evidence the window's target now rests on. The caller must store this every call, not
    /// only when `latching`: a resolution that agrees with the held slot but comes from a stronger
    /// source tightens the latch without changing the face, and forgetting that upgrade would let
    /// an already-confirmed target be moved again later.
    pub source: Option<PortraitSlotSource>,
}

impl PortraitWindowTarget {
    /// The legacy `(slot, latching)` pair, for callers that do not track latch authority.
    #[must_use]
    pub const fn into_pair(self) -> (Option<i32>, bool) {
        (self.slot, self.latching)
    }
}

/// [`portrait_window_target_slot`], plus the one exception the stability rule needs.
///
/// **the latch must not OUTRANK the user, and that is the bug this fixes.** "Stability over
/// freshness" is right when it stops a user-confirmed target sliding to a game-inferred one. It is
/// wrong when the thing being defended is itself a guess. The boot loading window opens long before
/// the missing-save picker is answered, so it commits to whatever the autoload hint happens to say
/// -- and then rejects the user's actual pick as a "mid-window retarget".
///
/// Measured, run br-20260826-190532-55e2:
///
/// ```text
/// [+1061ms]     loading-portrait: window LATCHED portrait target slot 0
///               (picker=None b78=None ac0=-1) -- held until this loading screen closes
/// [+1084597ms]  loading-portrait: SUPPRESSED a mid-window retarget 0 -> 1
///               (picker=Some(1) b78=Some(-1) ac0=-1)
/// ```
///
/// Every source was invalid at latch time -- `picker=None b78=None ac0=-1` -- so slot 0 came from
/// the autoload's default hint, i.e. from nothing. Eighteen minutes later the user picked slot 1
/// and the latch refused it, so the loading screen rendered slot 0's character (`angrE RL 100`)
/// while slot 1 (level 90) was the one being loaded. A row index is not a slot and a guess is not a
/// choice; this is the second shape of the same confusion (bd
/// `profileselect-cursor-is-a-row-index-not-a-slot-2026-08-25`).
///
/// The exception is deliberately the narrowest one that can work: a latch adopted from a guess
/// yields exactly once, and only to the user's own pick. A latch that came from the pick never
/// yields -- so the 2026-08-02 repro this whole mechanism exists for (picked slot 0, precedence
/// later fell through to `save_slot = 9`, window retargeted mid-load) still cannot happen.
///
/// Superseded by [`portrait_window_target_slot_by_evidence`], which this now delegates to. The
/// two-boolean form can only say "pick" or "not pick", and that turned out to be too coarse: it
/// files a target read off `save_slot` and a target read off the load-request register under the
/// same word, so the yield could not fire when the stronger of two measured sources arrived late.
/// Kept because its behaviour is what the existing callers and repro tests are written against.
#[must_use]
pub fn portrait_window_target_slot_authoritative(
    latched: Option<i32>,
    latched_from_pick: bool,
    resolved: Option<i32>,
    resolved_from_pick: bool,
) -> PortraitWindowTarget {
    let as_source = |from_pick: bool| {
        if from_pick {
            PortraitSlotSource::UserPick
        } else {
            PortraitSlotSource::SaveSlot
        }
    };
    portrait_window_target_slot_by_evidence(
        latched,
        latched.map(|_| as_source(latched_from_pick)),
        resolved.map(|slot| (slot, as_source(resolved_from_pick))),
    )
}

/// Stability over freshness, **except against strictly better evidence**.
///
/// This is [`portrait_window_target_slot_authoritative`] with the pick/not-pick boolean replaced
/// by the actual [`PortraitSlotSource`], and the rule generalised to the thing that boolean was
/// approximating: **a committed target yields only to a source that outranks the one it was
/// committed from, and only when the slot actually differs.** Equal or weaker evidence never
/// moves a window, so the mid-load face change the latch exists to prevent (2026-08-02: picked
/// slot 0, `save_slot` later read 9, window retargeted at +20998ms) is still impossible — a pick
/// is the top rank and nothing outranks it.
///
/// What the boolean form could not express, and what this fixes (bd `er-effects-rs-fmy6`): on a
/// System->Quit->Load switch across save files the redirect swap leaves the slot registers
/// momentarily stale, so the window latched slot 0 off `ac0=0` — a real read, just an obsolete
/// one. 4.2s later the load-request register named the real slot 1 and the retarget was refused,
/// because "latched from ac0" and "latched from the pick" were the same value. Under the rank
/// rule `RequestSlot > SaveSlot`, so the window follows the request exactly once and the face
/// matches the character that loads.
///
/// Rank is monotonic within a window (it only ever increases, and resets on window close), so
/// there are at most two retargets per window and each one is a strict improvement in evidence.
///
/// `latched_source` is what the caller stored from the previous call's [`PortraitWindowTarget`];
/// `None` alongside `Some(slot)` is read as the weakest rank, so a caller that has not recorded a
/// source yet cannot accidentally make its latch unassailable.
#[must_use]
pub fn portrait_window_target_slot_by_evidence(
    latched: Option<i32>,
    latched_source: Option<PortraitSlotSource>,
    resolved: Option<(i32, PortraitSlotSource)>,
) -> PortraitWindowTarget {
    // The rule, in one LINE: a window defends its commitment only against evidence of equal or
    // lower rank; strictly better evidence always wins.
    //
    // That single sentence is what collapses two separately-filed defects into one. b1d6 latched a
    // CONFIG hint (rank: none at all -- it is not evidence, which is why the caller no longer
    // offers it) and refused the first real `ac0`. fmy6 latched a real-but-stale `ac0` and refused
    // the `b78` that superseded it. Both were "a weak commitment outranking a stronger fact", and
    // the old pick/not-pick boolean could express neither, because it filed an obsolete register
    // read and a deliberate human click under the same word.
    match (latched, resolved) {
        (Some(held), Some((fresh, fresh_source))) => {
            let held_source = latched_source.unwrap_or(PortraitSlotSource::SaveSlot);
            if held != fresh && fresh_source > held_source {
                // Strictly better evidence, and it names a different character: follow it.
                PortraitWindowTarget {
                    slot: Some(fresh),
                    latching: true,
                    promoted_by_pick: fresh_source == PortraitSlotSource::UserPick,
                    source: Some(fresh_source),
                }
            } else {
                // Same slot, or evidence that is no better: the window keeps what it committed
                // to. The recorded source still rises to the best that has agreed with it, so a
                // target the user has now confirmed stops being movable.
                PortraitWindowTarget {
                    slot: Some(held),
                    latching: false,
                    promoted_by_pick: false,
                    source: Some(if held == fresh {
                        held_source.max(fresh_source)
                    } else {
                        held_source
                    }),
                }
            }
        }
        // Window already committed and nothing names a slot right now: keep it, and keep its
        // provenance -- a momentarily-invalid register must not demote an established latch.
        (Some(held), None) => PortraitWindowTarget {
            slot: Some(held),
            latching: false,
            promoted_by_pick: false,
            source: latched_source,
        },
        // First resolution of this window: adopt it.
        (None, Some((fresh, fresh_source))) => PortraitWindowTarget {
            slot: Some(fresh),
            latching: true,
            promoted_by_pick: false,
            source: Some(fresh_source),
        },
        // Nothing named a slot yet; stay uncommitted rather than inventing one.
        (None, None) => PortraitWindowTarget {
            slot: None,
            latching: false,
            promoted_by_pick: false,
            source: None,
        },
    }
}

/// The same-identity bridge hold: may the head published for the previous loading window keep
/// displaying across a switch rearm, instead of being cleared as a possible wrong character?
///
/// **this PREDICATE cannot fail on a same-slot repeat load, and that is the point of writing it
/// down here.** `incoming_name_hash` is hashed from slot N's ProfileSummary record at rearm time.
/// `published_name_hash` was hashed from slot N's ProfileSummary record at the previous window's
/// build kick and carried to the bridge at publish. Both operands are the same record read twice,
/// so the comparison answers "did this record's name change between the two reads" -- never "does
/// this record describe the character that is about to load". Re-select the same slot and it
/// matches trivially, which is exactly what happened on 2026-08-22 (run br-20260822-040913-f0f4):
/// slot 0's record said `Maddened Bean` while the character actually resident was `Ordinary Bean`,
/// and the hold matched anyway and kept the previous head for the whole window.
///
/// It is kept as-is, because as a cheap first filter it is still right: a changed name hash proves
/// a different character and must clear. What changed is its status -- a match is now provisional,
/// not a decision, and the caller must arrange for something independent to confirm or revoke it
/// (see [`bridge_hold_face_verdict`]).
///
/// `*_slot_tag` is the wire form used by `LS_PORTRAIT_PUBLISHED_SLOT`: **slot + 1**, so 0 means
/// "no slot". A 0 hash means "unknown name", which is never treated as agreement.
#[must_use]
pub const fn same_identity_bridge_hold(
    have_head: bool,
    incoming_slot_tag: usize,
    incoming_name_hash: usize,
    published_slot_tag: usize,
    published_name_hash: usize,
) -> bool {
    have_head
        && incoming_slot_tag != 0
        && incoming_slot_tag == published_slot_tag
        && incoming_name_hash != 0
        && incoming_name_hash == published_name_hash
}

/// What an independent identity signal says about an outstanding provisional bridge hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeHoldVerdict {
    /// No provisional hold is outstanding -- either none was taken, or this window already
    /// published its own head and superseded it. Nothing to say.
    NoHold,
    /// The signal is about a different slot than the held head. Not evidence either way.
    OtherSlot,
    /// No fingerprint exists for this slot, so the record cannot be checked against anything
    /// outside itself. The hold stays up, still unproven. Fails closed in the sense that matters:
    /// absence of evidence is never reported as agreement.
    NoEvidence,
    /// The record agrees with its fingerprint. Deliberately not called "confirmed": it proves the
    /// record was not rewritten under the slot, not that the held head is the loading character
    /// (the 2026-08-22 record was intact against nothing and still named the wrong person). Only
    /// this window publishing its own frame proves that.
    Unrefuted,
    /// The record the portrait is about to build from is a different character than the one whose
    /// fingerprint was taken for this slot. The held head cannot be right; drop it.
    Revoke,
}

/// Judge an outstanding provisional hold against the record-vs-preview face fingerprint taken at
/// the build kick.
///
/// This is the only portrait identity signal that compares the ProfileSummary record against a
/// source outside that record: `preview_face_hash` is hashed from the picked save's own bytes when
/// the foreign-save preview writes the slot, `record_face_hash` is re-hashed off the live record at
/// the kick. Every other signal in the pipeline (the hold's name hashes, the published-vs-target
/// name hashes, the loadwin `identity=` tag) reads the record on both sides and therefore agrees
/// with itself no matter how wrong the record is -- which is why the 2026-08-22 window closed
/// `identity=ok` while this fingerprint had already disagreed twice.
///
/// It is not available when the hold is taken. The hold is decided at the switch rearm; the
/// fingerprint arrives at the first build kick, measured ~1.4s later (`+107006ms` rearm vs
/// `+108385ms` first mismatch). That timing is the whole reason the hold is provisional-then-
/// revocable rather than simply being given a better predicate up front.
///
/// `held_slot_tag` is slot+1 of the outstanding hold (0 = none); `kick_slot` is the raw slot index
/// the kick is building. A 0 `preview_face_hash` means the slot has no fingerprint.
#[must_use]
pub const fn bridge_hold_face_verdict(
    held_slot_tag: usize,
    kick_slot: i32,
    record_face_hash: usize,
    preview_face_hash: usize,
) -> BridgeHoldVerdict {
    if held_slot_tag == 0 {
        return BridgeHoldVerdict::NoHold;
    }
    if kick_slot < 0 || held_slot_tag != (kick_slot as usize) + 1 {
        return BridgeHoldVerdict::OtherSlot;
    }
    if preview_face_hash == 0 {
        return BridgeHoldVerdict::NoEvidence;
    }
    if record_face_hash == preview_face_hash {
        BridgeHoldVerdict::Unrefuted
    } else {
        BridgeHoldVerdict::Revoke
    }
}

impl BridgeHoldVerdict {
    /// True only when the held head must be dropped, so a caller can act without matching on
    /// every arm.
    #[must_use]
    pub const fn revokes(self) -> bool {
        matches!(self, Self::Revoke)
    }
}

#[cfg(test)]
mod tests;
