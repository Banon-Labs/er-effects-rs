//! A DLL-owned `BonfireWarpParam` row for a synthetic invasion pin.
//!
//! # Why a whole fake param row
//!
//! `CS::WorldMapWarpPinData`'s constructor does not take an entity id, a name, an icon or a
//! category. It takes a `BonfireWarpParamLookupResult` and reads all of them out of the
//! `BonfireWarpParam*` inside it:
//!
//! | pin field | copied from | note |
//! |---|---|---|
//! | `+0x50` bonfire entity id | param `+0x08` | `-1` becomes `0` |
//! | `+0x54` cleared-event flag | param `+0x18` | `-1` becomes `0` |
//! | `+0x60` category mask bits | param `+0x1E` (u8) `& 7` | what the row filter's mask tests |
//! | `+0x248` icon id | param `+0x1C` (u16) | |
//! | `+0x68..` 8 labels | param `+0x30 + 12*i` text ids, kinds at `+0x90 + i` | |
//!
//! So there is no way to author a pin without a param row behind it. Rather than mutate the
//! live param table -- which would mean growing the file allocation, rewriting the `u16` row
//! count at `P+0x0a`, extending a format-dependent descriptor array and rebuilding the sorted
//! id index -- the DLL owns the bytes and a lookup detour hands them out for our private ids.
//!
//! # Lifetime
//!
//! These rows must outlive every pin that points at one, which means the whole session. They
//! are therefore leaked deliberately (`Box::leak`) rather than owned by anything droppable: a
//! freed param row behind a live pin is a use-after-free the engine would hit while rendering.
//!
//! # Why a miss is safe
//!
//! The param lookups are binary searches that yield a NULL row on a miss, and every caller
//! null-checks. A pin whose param row is missing renders with an empty name and a zero entity
//! id rather than crashing -- so a mistake here degrades, it does not fault.

/// Bytes of a `BonfireWarpParam` row this crate authors.
///
/// The engine reads up to `+0x97` (`kind` byte for label 7), so the buffer is rounded up to
/// `0x100`: over-allocating costs nothing and leaves headroom if a later field is discovered,
/// whereas under-allocating is an out-of-bounds read inside the engine.
pub const SYNTHETIC_PARAM_ROW_LEN: usize = 0x100;

/// `+0x08` -- bonfire entity id. Copied to pin `+0x50`.
pub const PARAM_ENTITY_ID_OFFSET: usize = 0x08;
/// `+0x14` -- subcategory row id, used to resolve the tab chain.
pub const PARAM_SUBCATEGORY_ID_OFFSET: usize = 0x14;
/// `+0x18` -- cleared-event flag id. Copied to pin `+0x54`.
pub const PARAM_CLEARED_EVENT_FLAG_OFFSET: usize = 0x18;
/// `+0x1C` -- icon id (u16). Copied to pin `+0x248`.
pub const PARAM_ICON_ID_OFFSET: usize = 0x1C;
/// `+0x1E` -- category bits (u8). Masked with `& 7` into pin `+0x60`.
pub const PARAM_CATEGORY_BITS_OFFSET: usize = 0x1E;
/// `+0x10` -- forbidden icon id (u16). Copied to pin `+0x288`.
pub const PARAM_FORBIDDEN_ICON_ID_OFFSET: usize = 0x10;
/// `+0xE8` -- ALTERNATE icon id (u16). Copied to pin `+0x2C8`.
///
/// `CS::WorldMapWarpPinData::GetIconId` (0x14088bb60) returns this one instead of `+0x1C` whenever
/// any enabled label carries kind `1` (`NpcName`). Every label this crate writes is kind `0`, so
/// the alternate is not reached today -- but leaving it zero means a single stray kind byte would
/// call `gotoAndStop(0)` on a 1-based clip and draw NOTHING, with the row still flagged visible and
/// every counter green. Mirroring the real icon here costs two bytes and deletes that whole silent
/// failure mode.
pub const PARAM_ALT_ICON_ID_OFFSET: usize = 0xE8;
/// `+0xEA` -- alternate forbidden icon id (u16). Copied to pin `+0x308`.
pub const PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET: usize = 0xEA;
/// `+0x30` -- first label text id; label `i` is at `+0x30 + 12*i`.
pub const PARAM_LABEL_TEXT_ID_BASE: usize = 0x30;
/// Stride between label text ids.
pub const PARAM_LABEL_TEXT_ID_STRIDE: usize = 12;
/// `+0x90` -- first label kind byte; label `i` is at `+0x90 + i`.
pub const PARAM_LABEL_KIND_BASE: usize = 0x90;
/// The row constructor pushes exactly this many labels.
pub const PARAM_LABEL_COUNT: usize = 8;

/// Label kind `0`: resolve the text id against the `PlaceName` FMG.
pub const LABEL_KIND_PLACE_NAME: u8 = 0;
/// Label kind `1`: resolve against the `NpcName` FMG.
pub const LABEL_KIND_NPC_NAME: u8 = 1;

/// A text id the engine treats as "no label" -- it renders an empty `MenuString`.
pub const LABEL_TEXT_ID_NONE: i32 = -1;

/// The icon id is a **1-based GFx frame number**, not a texture id.
///
/// `BonfireWarpParam+0x1C` is copied to pin `+0x248`, and `CS::WorldMapPinData::SetTo`
/// (`0x14087ae20`) hands it to Scaleform as `Icon_0.gotoAndStop(id)`. `Icon_0` is a 348-frame
/// MovieClip (sprite 171) inside `menu:/02_120_WorldMap.gfx`, and frame N places one
/// `MENU_MAP_*` bitmap. There is no lookup table, no atlas indirection and no clamp: an id
/// outside the populated frames lands on one of the 230 empty frames and draws NOTHING.
///
/// This is why the earlier value here was actively wrong. It was `2`, chosen as "some id that
/// is not the graces' `1`" -- but frame 2 is `MENU_MAP_01_Bonfire` *plus* an overlay, i.e. the
/// Site-of-Grace art with a decoration. The pins were not merely undifferentiated from graces;
/// they were drawn as graces on purpose by an id picked without knowing what it selected.
///
/// The frame the invasion pins use. [`RED_INVASION_PIN_FRAME`] when the world-map movie has
/// been given a red marker on a spare frame, [`FALLBACK_INVASION_PIN_ICON_ID`] otherwise -- so
/// a pin never points at an empty frame and vanishes.
#[must_use]
pub const fn invasion_pin_icon_id(red_frame_installed: bool) -> u16 {
    if red_frame_installed {
        RED_INVASION_PIN_FRAME
    } else {
        FALLBACK_INVASION_PIN_ICON_ID
    }
}

/// What the map should say about one invasion location, relative to the filter's current rules.
///
/// These are not three arbitrary looks -- they are the three answers
/// [`crate::local_invasion::LocalInvasionConfig::judge`] can give about a destination, so the map
/// shows what would actually happen if an invasion landed there rather than a separate notion of
/// "enabled" that could drift away from the filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PinAppearance {
    /// The user marked this exact location (Insert, or `allowed_blocks` by hand).
    Chosen,
    /// Not marked, but the current mode would accept it.
    #[default]
    Eligible,
    /// The current mode would reject it, so an invasion here would be cancelled.
    Rejected,
}

/// The icon id for one pin.
///
/// Falls back to the single vanilla frame for EVERY appearance when the markers are not installed.
/// Distinguishing the tiers requires the spare frames to have been populated; without them the
/// alternative ids point at empty frames and those pins would silently disappear, which is a worse
/// outcome than every pin looking the same.
///
/// # The `dimmed` axis
///
/// Orthogonal to the tier. `dimmed` means "an invasion attempt is in flight, so this pin is not
/// clickable right now" ([`crate::warp::invasion_attempt_in_flight`]); the tier means "here is what
/// the filter would do with an invasion landing here". Both have to be visible at once, which is
/// why there are two frames per tier rather than one dim frame shared by all three.
///
/// A colour transform is baked into a GFx frame, so it CANNOT be toggled on one icon id -- the
/// engine's only input is the frame number. Switching the id is the whole mechanism, and it is the
/// same one the tiers already use.
#[must_use]
pub const fn invasion_pin_icon_id_for(
    appearance: PinAppearance,
    markers_installed: bool,
    dimmed: bool,
) -> u16 {
    if !markers_installed {
        return FALLBACK_INVASION_PIN_ICON_ID;
    }
    if dimmed {
        return match appearance {
            PinAppearance::Chosen => CHOSEN_INVASION_PIN_FRAME_DIMMED,
            PinAppearance::Eligible => RED_INVASION_PIN_FRAME_DIMMED,
            PinAppearance::Rejected => REJECTED_INVASION_PIN_FRAME_DIMMED,
        };
    }
    match appearance {
        PinAppearance::Chosen => CHOSEN_INVASION_PIN_FRAME,
        // The DEFAULT tier keeps the frame invasion pins have always used, so an untouched config
        // renders exactly the map that existed before tiers. Putting this tier on a new frame is
        // what blanked a live map: every pin is `Eligible` until the user marks something.
        PinAppearance::Eligible => RED_INVASION_PIN_FRAME,
        PinAppearance::Rejected => REJECTED_INVASION_PIN_FRAME,
    }
}

/// Spare `Icon_0` frame carrying `MENU_MAP_Enemy_03` (188x190) -- the LARGEST of the family, for a
/// location the user chose.
///
/// The tiers are ranked by size: 188 > 146 > 68. Brightness is not monotonic across this family
/// (Enemy_03 is the dimmest), and size is the cue that survives at map scale anyway.
pub const CHOSEN_INVASION_PIN_FRAME: u16 = 303;

/// Spare `Icon_0` frame carrying `MENU_MAP_Enemy_00` (68x72, RGB `187/90/61`) -- the dimmest and
/// smallest of the hostile-marker family, for a location the filter would reject.
pub const REJECTED_INVASION_PIN_FRAME: u16 = 302;

/// The spare `Icon_0` frame the DLL installs a red marker on.
///
/// 300 is one of the 230 frames sprite 171 declares but never populates (the populated set is
/// 1..90-ish, 101..139, 208..218, 230..234, 240..261 and 348), so writing it collides with no
/// shipped icon. The bitmap placed there is `MENU_MAP_Enemy_02` -- the hostile-player marker,
/// which is the reddest art in the map atlas by measurement (alpha-weighted mean RGB
/// `234/99/48` against the grace's `170/144/81`) and at 146x146 declared / 73x73 packed is the
/// same scale as the 156x156 / 78x78 grace icon it replaces.
pub const RED_INVASION_PIN_FRAME: u16 = 300;

/// The DIMMED counterpart of each tier's frame: same bitmap, same size, same placement scale, plus
/// a `CXFORMWITHALPHA` that drops it to ~40% opacity.
///
/// # Why a second set of frames rather than a flag
///
/// The offset is a flat `+10` so the pairing reads at a glance in a log line
/// (`untouched=300`/`310`). All six sit inside the same unpopulated stretch: a census of vanilla
/// sprite 171 finds 348 declared frames, 138 populated, and a single empty run of 85 frames from
/// 263 to 347. None of the six collides with a shipped icon, and each is checked for emptiness
/// individually before anything is written.
///
/// The BRIGHT set is deliberately the original three, carrying their original placement bytes with
/// no colour transform at all. That makes the idle map -- the state a player is in almost all the
/// time -- byte-identical to the one already proven to render, and confines every new frame to the
/// dimmed set, where a failure is bounded to the duration of an invasion attempt and is obvious.
pub const DIMMED_FRAME_OFFSET: u16 = 10;
/// Dimmed `MENU_MAP_Enemy_02` -- the default tier while an attempt is in flight.
pub const RED_INVASION_PIN_FRAME_DIMMED: u16 = RED_INVASION_PIN_FRAME + DIMMED_FRAME_OFFSET;
/// Dimmed `MENU_MAP_Enemy_03` -- a location the user chose.
pub const CHOSEN_INVASION_PIN_FRAME_DIMMED: u16 = CHOSEN_INVASION_PIN_FRAME + DIMMED_FRAME_OFFSET;
/// Dimmed `MENU_MAP_Enemy_00` -- a location the filter would reject.
pub const REJECTED_INVASION_PIN_FRAME_DIMMED: u16 =
    REJECTED_INVASION_PIN_FRAME + DIMMED_FRAME_OFFSET;

/// Used when the red frame is not installed: a populated, non-grace frame so the pins are still
/// visually distinct rather than invisible.
///
/// Frame 87 is `MENU_MAP_87`, the reddest of the icons the game itself already places on a
/// frame (mean RGB `202/144/106`) and 78x78, the same size as the grace icon.
pub const FALLBACK_INVASION_PIN_ICON_ID: u16 = 87;

/// The category bits, masked `& 7` into pin `+0x60`. These are **map-layer visibility bits**.
pub const CATEGORY_BITS_MASK: u8 = 0x7;

/// Layer bit for the Lands Between surface (map layer id `0`).
pub const LAYER_BIT_SURFACE: u8 = 0x1;
/// Layer bit for the underground (map layer id `1`) -- Siofra, Ainsel, Deeproot, Mohgwyn.
pub const LAYER_BIT_UNDERGROUND: u8 = 0x2;
/// Layer bit for the Shadow Lands (map layer id `10`).
pub const LAYER_BIT_SHADOW_LANDS: u8 = 0x4;

/// Area byte of a Shadow Lands (Shadow of the Erdtree) block.
pub const AREA_SHADOW_LANDS: u8 = 61;
/// Area byte the engine treats as the UNDERGROUND layer (Siofra, Ainsel, Deeproot, Mohgwyn).
///
/// It is `12`, not `60`. An earlier comment here asserted that area 60 covered both the surface
/// and the underground; the binary says otherwise (`0x140886c40` / `0x140887870` promote a
/// layer-0 block to layer 1 only when its area byte is `0x0C`). The shipped auto-invade-point
/// corpus holds no area-12 blocks at all, so the underground map legitimately has no invasion
/// pins -- an absence to report honestly, not to paper over by setting extra bits.
pub const AREA_UNDERGROUND: u8 = 12;

/// How `row+0x60` decides which map a pin is drawn on.
///
/// `+0x60` is a per-layer visibility BITMAP, not a category: `FUN_14087afa0` sets a row's drawn
/// flag (`row+0xc`) only when `(row+0x60 >> bitIdx) & 1`, where
/// `bitIdx = FUN_140887e90(currentLayerId)` maps layer `0` -> bit 0, `1` -> bit 1, `10` -> bit 2,
/// and anything else -> `-1` (invisible).
///
/// **A row carries exactly ONE coordinate** (`row+0x10`), produced by one area converter. Setting
/// several bits therefore does not put a pin on several maps -- it draws the SAME point on each
/// of them, and that point only means anything on the map whose converter produced it. Setting
/// all three was wrong in a way that was easy to see and easy to misdiagnose: a Shadow Lands pin,
/// correctly projected into Shadow Lands space, was also drawn on the Lands Between map at those
/// coordinates, i.e. out at sea -- while still warping correctly, because the warp resolves the
/// block id and never reads the map position.
///
/// So the bit must come from the converter that actually accepted the pin (the caller has that
/// index; see the DLL's `layer_bit_for_converter`), and exactly one bit may be set. "On every
/// map" is a property of the pin SET, not of any single row's mask.
pub const LAYER_BIT_DOC: () = ();

/// How a synthetic pin should be described and categorised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntheticParamSpec {
    /// Goes to `+0x08`, and thence to pin `+0x50`. This is the private-band id.
    pub entity_id: i32,
    /// Goes to `+0x14`; the subcategory whose `+0x08` names our tab.
    pub subcategory_id: i32,
    /// Goes to `+0x1C`.
    pub icon_id: u16,
    /// Goes to `+0x1E`; masked `& 7`.
    pub category_bits: u8,
    /// `PlaceName` text id for label 0, or [`LABEL_TEXT_ID_NONE`].
    pub place_name_text_id: i32,
}

/// Write `icon_id` into EVERY icon slot of a synthetic param row.
///
/// # Why all four, and why this is a function
///
/// A pin row does not hold one icon. `CS::WorldMapWarpPinData`'s constructor (`0x14088b7b0`) seeds
/// FOUR 0x40-byte icon descriptors from four separate fields of this param row -- byte-verified
/// against its own loads:
///
/// | param offset | row descriptor |
/// |--------------|----------------|
/// | `0x1c`       | `0x248`        |
/// | `0x10`       | `0x288`        |
/// | `0xe8`       | `0x2c8`        |
/// | `0xea`       | `0x308`        |
///
/// At draw time `SetTo` asks vtable slot `0xc` (`0x14088bb60`) which descriptor to use, and that
/// choice depends on an event-flag predicate over the param row -- state a synthetic row does not
/// model. So the only way to be sure the pin draws the intended icon is for all four to agree.
///
/// THIS IS A FUNCTION BECAUSE THE TWO CALLERS DRIFTED. `to_row_bytes` set all four; the
/// re-stamp that runs when the player marks a location set only `0x1c`. The result was the
/// reported bug: the log showed the tiers changing correctly on every pin, and the map on screen
/// never changed, because the descriptor the engine actually read still held the icon from the
/// FIRST build. One entry point makes that divergence impossible.
pub fn stamp_icon_id(row: &mut [u8; SYNTHETIC_PARAM_ROW_LEN], icon_id: u16) {
    for at in [
        PARAM_ICON_ID_OFFSET,
        PARAM_FORBIDDEN_ICON_ID_OFFSET,
        PARAM_ALT_ICON_ID_OFFSET,
        PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET,
    ] {
        row[at..at + 2].copy_from_slice(&icon_id.to_le_bytes());
    }
}

impl SyntheticParamSpec {
    /// Serialise into the byte layout the engine reads.
    ///
    /// Every unspecified byte stays zero. The cleared-event-flag field is written as `-1`
    /// on purpose: the row constructor maps `-1` to `0`, which is the "no flag" value, and
    /// leaving a real flag id there would tie a pin's state to an unrelated world flag.
    #[must_use]
    pub fn to_row_bytes(&self) -> [u8; SYNTHETIC_PARAM_ROW_LEN] {
        let mut row = [0_u8; SYNTHETIC_PARAM_ROW_LEN];
        row[PARAM_ENTITY_ID_OFFSET..PARAM_ENTITY_ID_OFFSET + 4]
            .copy_from_slice(&self.entity_id.to_le_bytes());
        row[PARAM_SUBCATEGORY_ID_OFFSET..PARAM_SUBCATEGORY_ID_OFFSET + 4]
            .copy_from_slice(&self.subcategory_id.to_le_bytes());
        row[PARAM_CLEARED_EVENT_FLAG_OFFSET..PARAM_CLEARED_EVENT_FLAG_OFFSET + 4]
            .copy_from_slice(&(-1_i32).to_le_bytes());
        stamp_icon_id(&mut row, self.icon_id);
        row[PARAM_CATEGORY_BITS_OFFSET] = self.category_bits & CATEGORY_BITS_MASK;

        // Label 0 carries the place name; labels 1..7 are explicitly "none" so the engine
        // renders empty strings rather than resolving whatever happened to be in memory.
        for index in 0..PARAM_LABEL_COUNT {
            let text_id = if index == 0 {
                self.place_name_text_id
            } else {
                LABEL_TEXT_ID_NONE
            };
            let at = PARAM_LABEL_TEXT_ID_BASE + index * PARAM_LABEL_TEXT_ID_STRIDE;
            row[at..at + 4].copy_from_slice(&text_id.to_le_bytes());
            row[PARAM_LABEL_KIND_BASE + index] = LABEL_KIND_PLACE_NAME;
        }
        row
    }

    /// The highest byte the engine reads for this layout, as a bounds assertion for the buffer.
    #[must_use]
    pub const fn highest_read_offset() -> usize {
        // The alternate forbidden icon is the last field, past the label kinds.
        PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// THE REGRESSION, and it shipped. A pin row carries FOUR icon descriptors and the engine picks
    /// one at draw time from state a synthetic param row does not model. A stamper that updates
    /// only the first leaves the map showing the icon from the first build forever -- the exact
    /// reported symptom: correct tiers in the log, no change on screen.
    #[test]
    fn stamping_an_icon_sets_every_slot_the_row_constructor_reads() {
        let mut row = [0_u8; SYNTHETIC_PARAM_ROW_LEN];
        stamp_icon_id(&mut row, 303);
        for at in [
            PARAM_ICON_ID_OFFSET,
            PARAM_FORBIDDEN_ICON_ID_OFFSET,
            PARAM_ALT_ICON_ID_OFFSET,
            PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET,
        ] {
            assert_eq!(
                u16::from_le_bytes([row[at], row[at + 1]]),
                303,
                "slot {at:#x} was not stamped; the engine may read this one"
            );
        }
    }

    /// Re-stamping must REPLACE every slot, not leave a stale one behind -- a mark changes the icon
    /// of a row that already has one.
    #[test]
    fn re_stamping_replaces_every_slot_rather_than_leaving_a_stale_one() {
        let mut row = [0_u8; SYNTHETIC_PARAM_ROW_LEN];
        stamp_icon_id(&mut row, 300);
        stamp_icon_id(&mut row, 302);
        for at in [
            PARAM_ICON_ID_OFFSET,
            PARAM_FORBIDDEN_ICON_ID_OFFSET,
            PARAM_ALT_ICON_ID_OFFSET,
            PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET,
        ] {
            assert_eq!(
                u16::from_le_bytes([row[at], row[at + 1]]),
                302,
                "slot {at:#x} is stale"
            );
        }
    }

    /// The four param offsets are byte-verified against the row constructor's own loads at
    /// 0x14088b7b0: `MOVZX EAX,word ptr [RAX + 0x1c]` -> `MOV [RSI + 0x248]`, and likewise
    /// 0x10 -> 0x288, 0xe8 -> 0x2c8, 0xea -> 0x308. If one of these constants is edited, the
    /// stamper writes somewhere the engine never reads and the map silently stops responding.
    #[test]
    fn the_icon_slot_offsets_are_the_ones_the_row_constructor_loads() {
        assert_eq!(PARAM_ICON_ID_OFFSET, 0x1c);
        assert_eq!(PARAM_FORBIDDEN_ICON_ID_OFFSET, 0x10);
        assert_eq!(PARAM_ALT_ICON_ID_OFFSET, 0xe8);
        assert_eq!(PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET, 0xea);
    }

    /// A row built from a spec must agree with the stamper -- they are two entry points to the same
    /// invariant and the bug was precisely that they disagreed.
    #[test]
    fn a_freshly_built_row_matches_what_the_stamper_would_write() {
        let built = SyntheticParamSpec {
            entity_id: 0x7f00_0000,
            subcategory_id: 0,
            icon_id: 303,
            category_bits: 1,
            place_name_text_id: 1_234,
        }
        .to_row_bytes();
        let mut stamped = [0_u8; SYNTHETIC_PARAM_ROW_LEN];
        stamp_icon_id(&mut stamped, 303);
        for at in [
            PARAM_ICON_ID_OFFSET,
            PARAM_FORBIDDEN_ICON_ID_OFFSET,
            PARAM_ALT_ICON_ID_OFFSET,
            PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET,
        ] {
            assert_eq!(
                built[at..at + 2],
                stamped[at..at + 2],
                "slot {at:#x} disagrees"
            );
        }
    }

    fn spec() -> SyntheticParamSpec {
        SyntheticParamSpec {
            entity_id: 0x7F00_0005,
            subcategory_id: 4242,
            icon_id: 7,
            category_bits: 0x1,
            place_name_text_id: 1234,
        }
    }

    fn read_i32(row: &[u8], at: usize) -> i32 {
        i32::from_le_bytes(row[at..at + 4].try_into().expect("4 bytes"))
    }

    #[test]
    fn the_buffer_covers_every_byte_the_engine_reads() {
        // Under-allocating here is an out-of-bounds read INSIDE the engine, not a Rust panic.
        assert!(SyntheticParamSpec::highest_read_offset() < SYNTHETIC_PARAM_ROW_LEN);
        // The last label text id must also fit.
        let last_text =
            PARAM_LABEL_TEXT_ID_BASE + (PARAM_LABEL_COUNT - 1) * PARAM_LABEL_TEXT_ID_STRIDE;
        assert!(last_text + 4 <= SYNTHETIC_PARAM_ROW_LEN);
    }

    #[test]
    fn the_entity_id_lands_where_the_row_ctor_reads_it() {
        let row = spec().to_row_bytes();
        assert_eq!(read_i32(&row, PARAM_ENTITY_ID_OFFSET), 0x7F00_0005);
    }

    #[test]
    fn the_cleared_event_flag_is_minus_one_so_the_ctor_maps_it_to_no_flag() {
        // -1 is the sentinel the ctor turns into 0; any other value ties the pin's state to a
        // real world flag.
        let row = spec().to_row_bytes();
        assert_eq!(read_i32(&row, PARAM_CLEARED_EVENT_FLAG_OFFSET), -1);
    }

    #[test]
    fn the_category_bits_are_masked_to_the_three_the_filter_tests() {
        let mut s = spec();
        s.category_bits = 0xFF;
        let row = s.to_row_bytes();
        assert_eq!(row[PARAM_CATEGORY_BITS_OFFSET], 0x7);
    }

    #[test]
    fn label_zero_carries_the_place_name_and_the_rest_are_explicitly_none() {
        let row = spec().to_row_bytes();
        assert_eq!(read_i32(&row, PARAM_LABEL_TEXT_ID_BASE), 1234);
        // Every icon slot carries the real id, so no GetIconId branch can select a zero.
        for at in [
            PARAM_ICON_ID_OFFSET,
            PARAM_FORBIDDEN_ICON_ID_OFFSET,
            PARAM_ALT_ICON_ID_OFFSET,
            PARAM_ALT_FORBIDDEN_ICON_ID_OFFSET,
        ] {
            assert_ne!(
                u16::from_le_bytes(row[at..at + 2].try_into().unwrap()),
                0,
                "icon slot {at:#x} left at zero would gotoAndStop(0) and draw nothing"
            );
        }
        for index in 1..PARAM_LABEL_COUNT {
            let at = PARAM_LABEL_TEXT_ID_BASE + index * PARAM_LABEL_TEXT_ID_STRIDE;
            assert_eq!(read_i32(&row, at), LABEL_TEXT_ID_NONE, "label {index}");
        }
    }

    #[test]
    fn every_label_kind_byte_is_written_rather_than_left_to_chance() {
        // The ctor pushes exactly 8 labels and reads a kind byte for each; an unwritten byte
        // would pick an FMG at random.
        let row = spec().to_row_bytes();
        for index in 0..PARAM_LABEL_COUNT {
            assert_eq!(
                row[PARAM_LABEL_KIND_BASE + index],
                LABEL_KIND_PLACE_NAME,
                "kind {index}"
            );
        }
    }

    #[test]
    fn the_three_tiers_map_to_three_distinct_frames_and_collapse_without_the_markers() {
        use std::collections::BTreeSet;
        let tiers = [
            PinAppearance::Chosen,
            PinAppearance::Eligible,
            PinAppearance::Rejected,
        ];
        // Distinct WITHIN each brightness, and the two brightnesses must not overlap either -- all
        // six ids are handed to the same `gotoAndStop`, so any collision renders two states or two
        // tiers identically.
        for dimmed in [false, true] {
            let installed: BTreeSet<u16> = tiers
                .iter()
                .map(|a| invasion_pin_icon_id_for(*a, true, dimmed))
                .collect();
            assert_eq!(
                installed.len(),
                tiers.len(),
                "two tiers sharing a frame would render identically (dimmed={dimmed})"
            );
        }
        let all: BTreeSet<u16> = tiers
            .iter()
            .flat_map(|a| {
                [
                    invasion_pin_icon_id_for(*a, true, false),
                    invasion_pin_icon_id_for(*a, true, true),
                ]
            })
            .collect();
        assert_eq!(
            all.len(),
            6,
            "bright and dimmed must occupy separate frames"
        );
        // Without the spare frames populated, every alternative id points at an EMPTY frame and
        // draws nothing. Falling back to one vanilla frame loses the distinction but keeps the
        // pins visible, which is the right way round -- and that has to hold in BOTH states, or an
        // invasion would blank a map that has no markers installed.
        for tier in tiers {
            for dimmed in [false, true] {
                assert_eq!(
                    invasion_pin_icon_id_for(tier, false, dimmed),
                    FALLBACK_INVASION_PIN_ICON_ID,
                    "{tier:?} must not point at an unpopulated frame (dimmed={dimmed})"
                );
            }
        }
        // The DEFAULT tier must be the frame the single-appearance helper already used. This is the
        // no-regression guarantee: an untouched config makes every pin `Eligible`, so the map has
        // to render exactly as it did before tiers existed. Pointing this tier at a NEW frame is
        // what blanked a live map -- 510 of 512 pins moved onto frames that had never been seen to
        // draw, all at once, by doing nothing at all.
        //
        // Note this pins the UNDIMMED id specifically. Idle is the state a player is in almost all
        // the time, so it is the one that must be historically exact; the dimmed twin is reached
        // only while an invasion attempt is running.
        assert_eq!(
            invasion_pin_icon_id_for(PinAppearance::Eligible, true, false),
            invasion_pin_icon_id(true),
            "the default tier must keep the historical frame while idle"
        );
        // The deviations are the only tiers allowed onto new frames.
        for tier in [PinAppearance::Chosen, PinAppearance::Rejected] {
            assert_ne!(
                invasion_pin_icon_id_for(tier, true, false),
                invasion_pin_icon_id(true)
            );
        }
    }

    #[test]
    fn dimming_changes_the_frame_without_changing_the_tier() {
        // The two axes are orthogonal, and the bug this guards is collapsing them: a dimmed map
        // that shows every pin the same way would still "dim correctly" while destroying the tier
        // information the filter exists to convey.
        for tier in [
            PinAppearance::Chosen,
            PinAppearance::Eligible,
            PinAppearance::Rejected,
        ] {
            let bright = invasion_pin_icon_id_for(tier, true, false);
            let dim = invasion_pin_icon_id_for(tier, true, true);
            assert_ne!(bright, dim, "{tier:?} must have a distinct dimmed frame");
            assert_eq!(
                dim,
                bright + DIMMED_FRAME_OFFSET,
                "{tier:?}'s pair must stay at the documented offset"
            );
        }
    }

    #[test]
    fn the_icon_id_is_written_as_sixteen_bits() {
        let mut s = spec();
        s.icon_id = 0xBEEF;
        let row = s.to_row_bytes();
        assert_eq!(
            u16::from_le_bytes(
                row[PARAM_ICON_ID_OFFSET..PARAM_ICON_ID_OFFSET + 2]
                    .try_into()
                    .expect("2 bytes")
            ),
            0xBEEF
        );
        // and must not have smeared into the category byte
        assert_eq!(row[PARAM_CATEGORY_BITS_OFFSET], 0x1);
    }

    #[test]
    fn the_subcategory_id_lands_where_the_tab_chain_reads_it() {
        let row = spec().to_row_bytes();
        assert_eq!(read_i32(&row, PARAM_SUBCATEGORY_ID_OFFSET), 4242);
    }

    #[test]
    fn unspecified_bytes_stay_zero() {
        let row = spec().to_row_bytes();
        // +0x00..+0x08 is untouched by the spec and must not carry stack junk.
        assert_eq!(&row[0..PARAM_ENTITY_ID_OFFSET], &[0_u8; 8]);
    }

    #[test]
    fn the_label_offsets_match_the_reverse_engineered_stride() {
        // textId at +0x30 + 12*i, kind at +0x90 + i.
        assert_eq!(PARAM_LABEL_TEXT_ID_BASE, 0x30);
        assert_eq!(PARAM_LABEL_TEXT_ID_STRIDE, 12);
        assert_eq!(PARAM_LABEL_KIND_BASE, 0x90);
        assert_eq!(PARAM_LABEL_COUNT, 8);
        // The text-id block must not overlap the kind block.
        let last_text_end =
            PARAM_LABEL_TEXT_ID_BASE + (PARAM_LABEL_COUNT - 1) * PARAM_LABEL_TEXT_ID_STRIDE + 4;
        assert!(last_text_end <= PARAM_LABEL_KIND_BASE);
    }
}
