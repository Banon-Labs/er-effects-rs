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

/// Icon id given to invasion pins so they are visually distinct from Sites of Grace.
///
/// The pin's icon is NOT a colour tint -- it is an index the engine resolves to a sprite, read
/// from the param row's `+0x1C` and copied to pin `+0x248`. Changing the sprite is the only
/// distinguishing lever this layer has; recolouring an existing sprite would mean touching the
/// renderer.
///
/// The shipped grace rows use icon `1`, which is why cloning a donor verbatim produced pins
/// indistinguishable from graces. This value is deliberately different. The injector logs the
/// distinct icon ids it sees across the shipped rows, so the choice can be made from what the
/// game actually has rather than from a guess.
pub const INVASION_PIN_ICON_ID: u16 = 2;

/// The category bits the row filter masks. Only the low three survive `& 7`.
pub const CATEGORY_BITS_MASK: u8 = 0x7;

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
        row[PARAM_ICON_ID_OFFSET..PARAM_ICON_ID_OFFSET + 2]
            .copy_from_slice(&self.icon_id.to_le_bytes());
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
        PARAM_LABEL_KIND_BASE + PARAM_LABEL_COUNT - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
