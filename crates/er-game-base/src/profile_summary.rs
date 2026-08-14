//! Canonical in-memory `CS::ProfileSummary` binary layout for Elden Ring 1.16.2.
//!
//! This is a runtime RAM layout. It is deliberately separate from
//! `er_save_loader::profile_summary`, whose `USER_DATA010` records are packed differently on disk.
//! Cross-cutting consumers should derive offsets from these typed layouts instead of repeating
//! numeric record formulas in feature crates or the product DLL.

/// Number of character records held by `CS::ProfileSummary`.
pub const PROFILE_SUMMARY_SLOT_COUNT: usize = 10;

/// One in-memory character-summary record.
#[repr(C)]
pub struct ProfileSummaryRecord {
    pub character_name: [u8; 0x22],
    unknown_022: [u8; 0x02],
    pub level: i32,
    pub play_time: u32,
    pub rune_memory: i32,
    /// Saved `BlockId`, used as the record-side map identity.
    pub map: i32,
    /// `PlaceName` message id formatted into a profile row's Location field. Unlike `map`, the game
    /// writes this from `CSFeMan`, so a `PlayerGameData` body cannot reconstruct it by itself.
    pub place_name: i32,
    /// Native `FaceData` wrapper. Its inner `FaceDataBuffer` starts eight bytes into this field.
    pub face_data: [u8; 0x170],
    /// Native `ChrAsm` image consumed by the profile renderer's equipment path.
    pub chr_asm: [u8; 0xe8],
    pub gender: u8,
    pub archetype: u8,
    pub starting_gift: u8,
    pub field_c4: u8,
    unknown_294: [u8; 0x0c],
}

/// Header plus all ten in-memory character-summary records.
#[repr(C)]
pub struct ProfileSummaryLayout {
    unknown_000: [u8; 0x08],
    pub active_flags: [u8; PROFILE_SUMMARY_SLOT_COUNT],
    unknown_012: [u8; 0x06],
    pub records: [ProfileSummaryRecord; PROFILE_SUMMARY_SLOT_COUNT],
}

/// The private `GameDataMan` field that points to `CS::ProfileSummary`.
#[repr(C)]
pub struct GameDataManProfileSummaryLayout {
    unknown_000: [u8; 0x78],
    pub profile_summary: usize,
}

pub const GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET: usize =
    core::mem::offset_of!(GameDataManProfileSummaryLayout, profile_summary);
pub const PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryLayout, active_flags);
pub const PROFILE_SUMMARY_RECORD_BASE: usize = core::mem::offset_of!(ProfileSummaryLayout, records);
pub const PROFILE_SUMMARY_RECORD_STRIDE: usize = core::mem::size_of::<ProfileSummaryRecord>();
pub const PROFILE_SUMMARY_TOTAL_BYTES: usize = core::mem::size_of::<ProfileSummaryLayout>();

pub const PROFILE_SUMMARY_NAME_BYTES: usize = core::mem::size_of::<[u8; 0x22]>();
pub const PROFILE_SUMMARY_LEVEL_OFFSET: usize = core::mem::offset_of!(ProfileSummaryRecord, level);
pub const PROFILE_SUMMARY_PLAYTIME_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, play_time);
pub const PROFILE_SUMMARY_RUNE_MEMORY_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, rune_memory);
pub const PROFILE_SUMMARY_MAP_OFFSET: usize = core::mem::offset_of!(ProfileSummaryRecord, map);
pub const PROFILE_SUMMARY_PLACE_NAME_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, place_name);
pub const PROFILE_SUMMARY_FACE_DATA_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, face_data);
pub const PROFILE_SUMMARY_CHR_ASM_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, chr_asm);
pub const PROFILE_SUMMARY_GENDER_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, gender);
pub const PROFILE_SUMMARY_ARCHETYPE_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, archetype);
pub const PROFILE_SUMMARY_STARTING_GIFT_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, starting_gift);
pub const PROFILE_SUMMARY_FIELD_C4_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, field_c4);
pub const PROFILE_SUMMARY_FIELD_294_OFFSET: usize =
    core::mem::offset_of!(ProfileSummaryRecord, unknown_294);

/// Byte offset of one record within a `CS::ProfileSummary` allocation.
///
/// This deliberately preserves the old raw-address formula: callers that require an in-bounds
/// record validate `slot` against [`PROFILE_SUMMARY_SLOT_COUNT`] before dereferencing it.
pub const fn profile_summary_record_offset(slot: usize) -> usize {
    PROFILE_SUMMARY_RECORD_BASE.wrapping_add(slot.wrapping_mul(PROFILE_SUMMARY_RECORD_STRIDE))
}

/// Address of one record within a `CS::ProfileSummary` allocation.
///
/// Like [`profile_summary_record_offset`], this owns address arithmetic without adding a new runtime
/// validity policy to existing fault-guarded readers.
pub const fn profile_summary_record_address(summary: usize, slot: usize) -> usize {
    summary.wrapping_add(profile_summary_record_offset(slot))
}

// Compile-time guards pin the reverse-engineered 1.16.2 ABI independently of the tests.
const _: () = assert!(GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET == 0x78);
const _: () = assert!(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET == 0x08);
const _: () = assert!(PROFILE_SUMMARY_RECORD_BASE == 0x18);
const _: () = assert!(PROFILE_SUMMARY_RECORD_STRIDE == 0x2a0);
const _: () = assert!(PROFILE_SUMMARY_TOTAL_BYTES == 0x1a58);
const _: () = assert!(PROFILE_SUMMARY_LEVEL_OFFSET == 0x24);
const _: () = assert!(PROFILE_SUMMARY_PLAYTIME_OFFSET == 0x28);
const _: () = assert!(PROFILE_SUMMARY_RUNE_MEMORY_OFFSET == 0x2c);
const _: () = assert!(PROFILE_SUMMARY_MAP_OFFSET == 0x30);
const _: () = assert!(PROFILE_SUMMARY_PLACE_NAME_OFFSET == 0x34);
const _: () = assert!(PROFILE_SUMMARY_FACE_DATA_OFFSET == 0x38);
const _: () = assert!(PROFILE_SUMMARY_CHR_ASM_OFFSET == 0x1a8);
const _: () = assert!(PROFILE_SUMMARY_GENDER_OFFSET == 0x290);
const _: () = assert!(PROFILE_SUMMARY_ARCHETYPE_OFFSET == 0x291);
const _: () = assert!(PROFILE_SUMMARY_STARTING_GIFT_OFFSET == 0x292);
const _: () = assert!(PROFILE_SUMMARY_FIELD_C4_OFFSET == 0x293);
const _: () = assert!(PROFILE_SUMMARY_FIELD_294_OFFSET == 0x294);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_layout_matches_the_legacy_1162_binary_contract() {
        assert_eq!(GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET, 0x78);
        assert_eq!(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET, 0x08);
        assert_eq!(PROFILE_SUMMARY_RECORD_BASE, 0x18);
        assert_eq!(PROFILE_SUMMARY_RECORD_STRIDE, 0x2a0);
        assert_eq!(PROFILE_SUMMARY_TOTAL_BYTES, 0x18 + 10 * 0x2a0);
        assert_eq!(PROFILE_SUMMARY_NAME_BYTES, 0x22);
        assert_eq!(PROFILE_SUMMARY_LEVEL_OFFSET, 0x24);
        assert_eq!(PROFILE_SUMMARY_PLAYTIME_OFFSET, 0x28);
        assert_eq!(PROFILE_SUMMARY_RUNE_MEMORY_OFFSET, 0x2c);
        assert_eq!(PROFILE_SUMMARY_MAP_OFFSET, 0x30);
        assert_eq!(PROFILE_SUMMARY_PLACE_NAME_OFFSET, 0x34);
        assert_eq!(PROFILE_SUMMARY_FACE_DATA_OFFSET, 0x38);
        assert_eq!(PROFILE_SUMMARY_CHR_ASM_OFFSET, 0x1a8);
        assert_eq!(PROFILE_SUMMARY_GENDER_OFFSET, 0x290);
        assert_eq!(PROFILE_SUMMARY_ARCHETYPE_OFFSET, 0x291);
        assert_eq!(PROFILE_SUMMARY_STARTING_GIFT_OFFSET, 0x292);
        assert_eq!(PROFILE_SUMMARY_FIELD_C4_OFFSET, 0x293);
        assert_eq!(PROFILE_SUMMARY_FIELD_294_OFFSET, 0x294);
    }

    #[test]
    fn typed_record_offsets_match_the_legacy_formula_for_every_slot() {
        for slot in 0..PROFILE_SUMMARY_SLOT_COUNT {
            assert_eq!(profile_summary_record_offset(slot), 0x18 + slot * 0x2a0);
        }
        assert_eq!(
            profile_summary_record_address(0x1000, PROFILE_SUMMARY_SLOT_COUNT - 1),
            0x1000 + 0x18 + 9 * 0x2a0
        );
        assert_eq!(
            profile_summary_record_offset(PROFILE_SUMMARY_SLOT_COUNT),
            0x18 + 10 * 0x2a0,
            "the helper preserves the legacy unchecked formula"
        );
    }
}
