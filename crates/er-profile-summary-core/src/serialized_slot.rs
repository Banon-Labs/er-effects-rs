//! Read one save container slot's serialized `PlayerGameData`, and write it into a live
//! `CS::ProfileSummary` record.
//!
//! Moved verbatim from er-quickload
//! `experiments/startup_hooks/loading_cover/loading_cover_save_slot.rs`. It lives in the
//! ProfileSummary crate because [`SerializedPlayerGameData::write_profile_summary_record`] is its
//! terminal operation: everything above it -- the section walk, the `FACE` locator, the runtime
//! `ChrAsm` reassembly, the plausibility score that picks the right `PlayerGameData` offset --
//! exists to fill one record. The purely container-shaped half of the read (`slot_body`,
//! `slot_saved_map`, the stat-block locator) already lives in `er-save-loader` and is delegated to
//! below rather than re-implemented, so the two cannot drift.

use core::sync::atomic::{AtomicUsize, Ordering};

use er_game_base::profile_summary::{
    PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET, PROFILE_SUMMARY_ARCHETYPE_OFFSET,
    PROFILE_SUMMARY_CHR_ASM_OFFSET, PROFILE_SUMMARY_FACE_DATA_OFFSET,
    PROFILE_SUMMARY_FIELD_C4_OFFSET, PROFILE_SUMMARY_GENDER_OFFSET, PROFILE_SUMMARY_LEVEL_OFFSET,
    PROFILE_SUMMARY_MAP_OFFSET, PROFILE_SUMMARY_NAME_BYTES, PROFILE_SUMMARY_PLACE_NAME_OFFSET,
    PROFILE_SUMMARY_PLAYTIME_OFFSET, PROFILE_SUMMARY_RECORD_STRIDE,
    PROFILE_SUMMARY_RUNE_MEMORY_OFFSET, PROFILE_SUMMARY_SLOT_COUNT as TITLE_PROFILE_SLOT_COUNT,
    PROFILE_SUMMARY_STARTING_GIFT_OFFSET, profile_summary_record_address,
};
use er_loading_portrait_core::chr_asm_layout::{
    CHR_ASM_EQUIPMENT_OFFSET, CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET, CHR_ASM_OVERRIDE_ABSENT,
    CHR_ASM_SIZE, CHR_ASM_UNK0_OFFSET, CHR_ASM_UNKD4_OFFSET, CHR_ASM_UNKD8_OFFSET,
};

use crate::face_data::{
    CHR_ASM_COPY_RVA, FACE_DATA_BUFFER_TOTAL_SIZE, FACE_DATA_COPY_FROM_BUFFER_RVA,
};
use crate::host::append_autoload_debug;

/// `base + rva`, resolved for the running build, or `None` when this build moved the function.
///
/// The two calls below copy a character's face data and ChrAsm out of a serialized slot. A
/// hand-built `base + rva` would call the 1.16.2 address on ELDEN RING 1.17 -- whatever now
/// occupies it -- and a mapped constant is no safer that way, because the map knows the new
/// address and `base + rva` never asks it.
#[cfg(windows)]
fn gated_summary_fn(rva: usize, what: &'static str) -> Option<usize> {
    er_game_base::mem::game_rva_named(rva as u32, what).ok()
}

pub const SAVE_FACE_MAGIC: &[u8; 4] = b"FACE";
#[allow(dead_code)] // Retained: Save-format fact beside the live SAVE_FACE_MAGIC.
pub const SAVE_FACE_DATA_BUFFER_SIZE: usize = 0x120;

/// Per-slot FNV-1a64 (truncated to usize) of the foreign character's inner `FaceDataBuffer` as written
/// into the RAM ProfileSummary record by the save-swap preview -- the expected portrait identity for
/// that slot. 0 = no foreign preview owns the slot. The build kick re-hashes the record at kick time
/// and trips `PORTRAIT_FACE_IDENTITY_MISMATCHES` on drift, so a wrong-face portrait can never again
/// pass a run silently (user directive 2026-07-06: the run must detect the wrong rendered character
/// itself instead of relying on human review of RT dumps).
pub static PROFILE_PREVIEW_FACE_HASH: [AtomicUsize; TITLE_PROFILE_SLOT_COUNT] =
    [const { AtomicUsize::new(0) }; TITLE_PROFILE_SLOT_COUNT];

/// Bit per slot: the preview rebuilt this slot's record but could not source a place name for it.
///
/// The rebuild copies a structural template from the original save's record before overwriting the
/// fields it can supply (see `write_profile_summary_record`), so a slot whose place name the previewed
/// save cannot supply keeps the template character's -- and the map field, which is written from the
/// body, then agrees with the body and makes the record look self-consistent. The map comparison that
/// catches a stale record on a normally-loaded save therefore cannot catch this one; this mask is the
/// record of the fact, set where the failure to source actually happens.
pub static PROFILE_PREVIEW_PLACE_NAME_UNSOURCED: AtomicUsize = AtomicUsize::new(0);
pub const SAVE_PGD_SCAN_LEADING_FACE_COUNT: usize = 4;
pub const SAVE_PGD_FACE_DELTA_WINDOW_LOW: usize = 0xa000;
pub const SAVE_PGD_FACE_DELTA_WINDOW_HIGH: usize = 0xa600;
pub const SAVE_PLAYER_GAME_DATA_MIN_SIZE: usize = 0x1b0;
pub const SAVE_PGD_HEALTH_OFFSET: usize = 0x08;
pub const SAVE_PGD_MAX_HEALTH_OFFSET: usize = 0x0c;
pub const SAVE_PGD_BASE_MAX_HEALTH_OFFSET: usize = 0x10;
pub const SAVE_PGD_STAT_BASE_OFFSET: usize = 0x34;
pub const SAVE_PGD_STAT_COUNT: usize = 8;
pub const SAVE_PGD_LEVEL_OFFSET: usize = 0x60;
pub const SAVE_PGD_RUNE_MEMORY_OFFSET: usize = 0x68;
pub const SAVE_PGD_CHARACTER_NAME_OFFSET: usize = 0x94;
pub const SAVE_PGD_CHARACTER_NAME_UNITS: usize = 0x10;
pub const SAVE_PGD_CHARACTER_NAME_BYTES: usize = SAVE_PGD_CHARACTER_NAME_UNITS * 2;
pub const SAVE_PGD_GENDER_OFFSET: usize = 0xb6;
// The serialized PGD tracks the runtime struct at (runtime - 8) for these early scalars (level
// 0x68->0x60, name 0x9c->0x94, gender 0xbe->0xb6): archetype/gift/c4 follow the same shift.
pub const SAVE_PGD_ARCHETYPE_OFFSET: usize = 0xb7;
pub const SAVE_PGD_STARTING_GIFT_OFFSET: usize = 0xbb;
pub const SAVE_PGD_FIELD_C4_OFFSET: usize = 0xbc;
pub const SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET: usize = 0xf9;
pub const SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET: usize = 0xfa;
pub const SAVE_SPEFFECT_COUNT: usize = 0x0d;
pub const SAVE_SPEFFECT_SIZE: usize = 0x10;
pub const SAVE_CHR_ASM_EQUIPMENT_SIZE: usize = 0x58;
pub const SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE: usize = 0x1c;
pub const SAVE_INVENTORY_HELD_SIZE: usize = 0x9010;
pub const SAVE_EQUIP_MAGIC_SIZE: usize = 0x74;
pub const SAVE_EQUIP_ITEM_SIZE: usize = 0x8c;
pub const SAVE_GESTURE_EQUIP_SIZE: usize = 0x18;
pub const SAVE_PROJECTILE_ENTRY_SIZE: usize = 0x08;
pub const SAVE_PROJECTILE_COUNT_MAX: u32 = 0x400;
pub const SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE: usize = 0x9c;
pub const SAVE_PHYSIC_EQUIP_SIZE: usize = 0x0c;
pub const SAVE_FACE_DATA_FULL_SIZE: usize = 0x12f;
pub const SAVE_INVENTORY_STORAGE_SIZE: usize = 0x6010;
pub const SAVE_GESTURE_GAME_DATA_SIZE: usize = 0x100;
pub const SAVE_REGION_COUNT_MAX: u32 = 0x400;
pub const SAVE_REGION_ID_SIZE: usize = 0x04;
pub const SAVE_RIDE_GAME_DATA_SIZE: usize = 0x28;
pub const SAVE_CONTROL_BYTE_SIZE: usize = 0x01;
pub const SAVE_BLOODSTAIN_DATA_SIZE: usize = 0x44;
pub const SAVE_MENU_PROFILE_SAVE_LOAD_SIZE: usize = 0x1008;
pub const SAVE_TROPHY_EQUIP_DATA_SIZE: usize = 0x34;
pub const SAVE_GAITEM_GAME_DATA_SIZE: usize = 0x1b588;
pub const SAVE_TUTORIAL_DATA_SIZE: usize = 0x408;
pub const SAVE_GLOBAL_GAME_MAN_FLAGS_SIZE: usize = 0x03;
pub const SAVE_TOTAL_DEATHS_SIZE: usize = 0x04;
pub const SAVE_CHARACTER_TYPE_SIZE: usize = 0x04;
pub const SAVE_ONLINE_SESSION_FLAG_SIZE: usize = 0x01;
pub const SAVE_ONLINE_CHARACTER_TYPE_FLAG_SIZE: usize = 0x04;
pub const SAVE_LAST_RESTED_GRACE_SIZE: usize = 0x04;
pub const SAVE_NOT_ALONE_FLAG_SIZE: usize = 0x01;
pub const SAVE_INGAME_TIMER_PADDING_AFTER_NOT_ALONE: usize = 0x04;
pub const SAVE_INGAME_TIMER_TICKS_MAX: u32 = 999 * 60 * 60 / 10 + 59 * 60 / 10 + 59 / 10 + 1;

#[derive(Clone, Copy)]
pub struct SerializedSaveSlot<'a> {
    pub body: &'a [u8],
}

#[derive(Clone, Copy)]
pub struct SerializedPlayerGameData<'a> {
    pub body: &'a [u8],
    pub offset: usize,
}

impl<'a> SerializedSaveSlot<'a> {
    pub fn new(body: &'a [u8]) -> Self {
        Self { body }
    }

    /// The saved `BlockId` / map id this slot deserializes into `GameMan+0xc30`. Delegates to
    /// `er_save_loader::bnd4::slot_saved_map`, which is the function the host corpus test asserts
    /// against, so `saved_map()` is the tested read rather than a parallel one that can drift.
    pub fn saved_map(self) -> Option<i32> {
        er_save_loader::bnd4::slot_saved_map(self.body)
    }

    fn read_u32(self, offset: usize) -> Option<u32> {
        self.body
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn add_offset(offset: &mut usize, len: usize) -> Option<()> {
        *offset = offset.checked_add(len)?;
        Some(())
    }

    fn add_counted_region(
        &self,
        offset: &mut usize,
        entry_size: usize,
        max_count: u32,
    ) -> Option<()> {
        let count = self.read_u32(*offset)?;
        if count > max_count {
            return None;
        }
        let bytes = (count as usize).checked_mul(entry_size)?.checked_add(4)?;
        Self::add_offset(offset, bytes)
    }

    /// Walk from the PGD start to the serialized ChrAsm sections. Section order (data-validated
    /// offline against the gold save, 2026-07-06: param ids read as armor/weapon param ranges,
    /// handles as 0x8xxxxxxx gaitem patterns):
    /// `[gaitem slot indices 0x58][ChrAsmEquipment 0x1c][equipment param ids 0x58][gaitem handles 0x58]`.
    fn walk_to_chr_asm_sections(self, pgd: SerializedPlayerGameData<'a>) -> Option<usize> {
        let mut offset = pgd.offset;
        Self::add_offset(&mut offset, SAVE_PLAYER_GAME_DATA_MIN_SIZE)?;
        Self::add_offset(&mut offset, SAVE_SPEFFECT_COUNT * SAVE_SPEFFECT_SIZE)?;
        Some(offset)
    }

    /// Continue past the ChrAsm + inventory/equip/gesture/projectile regions to the face section
    /// (`SAVE_FACE_DATA_FULL_SIZE` bytes; the `FACE` FaceDataBuffer sits a few bytes in).
    fn walk_to_face_section(self, pgd: SerializedPlayerGameData<'a>) -> Option<usize> {
        let mut offset = self.walk_to_chr_asm_sections(pgd)?;
        Self::add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        Self::add_offset(&mut offset, SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        Self::add_offset(&mut offset, SAVE_INVENTORY_HELD_SIZE)?;
        Self::add_offset(&mut offset, SAVE_EQUIP_MAGIC_SIZE)?;
        Self::add_offset(&mut offset, SAVE_EQUIP_ITEM_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GESTURE_EQUIP_SIZE)?;
        self.add_counted_region(
            &mut offset,
            SAVE_PROJECTILE_ENTRY_SIZE,
            SAVE_PROJECTILE_COUNT_MAX,
        )?;
        Self::add_offset(&mut offset, SAVE_EQUIPPED_ARMAMENTS_AND_ITEMS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_PHYSIC_EQUIP_SIZE)?;
        Some(offset)
    }

    /// The character's serialized `FaceDataBuffer` (starts at its `FACE` magic,
    /// `FACE_DATA_BUFFER_TOTAL_SIZE` bytes) -- the exact source `FaceData::CopyFromBuffer` expects.
    /// The face section has a small prefix before the magic (observed: 4 bytes of 0xff), so the magic
    /// is scanned within the section rather than assumed at +0. `None` when the walk or magic fails
    /// (caller keeps the fallback face and logs).
    pub fn face_data_buffer_bytes(self, pgd: SerializedPlayerGameData<'a>) -> Option<&'a [u8]> {
        let sect_off = self.walk_to_face_section(pgd)?;
        let sect = self
            .body
            .get(sect_off..sect_off + SAVE_FACE_DATA_FULL_SIZE)?;
        let rel = sect
            .windows(SAVE_FACE_MAGIC.len())
            .position(|w| w == SAVE_FACE_MAGIC)?;
        self.body
            .get(sect_off + rel..sect_off + rel + FACE_DATA_BUFFER_TOTAL_SIZE)
    }

    /// Assemble a runtime `ChrAsm` image from the serialized sections, so the native ChrAsm copy
    /// receives the layout it expects: runtime is `[hdr 8][ChrAsmEquipment][gaitem_handles]
    /// [equipment_param_ids][tail]` while the save serializes `[slot indices][ChrAsmEquipment]
    /// [param ids][handles]` -- a raw copy of the save bytes dresses the portrait from garbage.
    ///
    /// The three override SENTINELS must be -1, not zero (bd er-effects-rs-wncc -- the real
    /// entirely-nude root cause). `unk0` (+0x00), `unkd4` (+0xd4) and `unkd8` (+0xd8) are not padding:
    /// the model-resource request `FUN_1409e6fb0` tests them signed and treats a non-negative value in
    /// any of them as a forced whole-outfit override, so a zero-filled image resolves head/chest/hands/
    /// legs to param ids 0/100/200/300 -- rows that do not exist. Nothing renders, including the
    /// bare-body defaults the native feed equips into hands and legs, which is exactly the reported
    /// "nude, missing even the default underwear". A ctor-built `ChrAsm` holds -1 here (deobf
    /// 0x1403be1d0 and 0x1403be208), which is why boot was unaffected: its record is copied from the
    /// ctor-initialised `PlayerGameData.equipGameData.chrAsm`. See `CHR_ASM_OVERRIDE_ABSENT`.
    /// `unk4` (+0x04) and the +0xdc..+0xe8 tail stay zero -- that is also what the ctor does.
    ///
    /// The GAITEM handle array is left zero on purpose. A gaitem handle only has meaning against the
    /// `gaitemInsTable` of the process that minted it; the foreign save's serialized handles index a
    /// table this process never populated. Both consumers of this image (`CHR_ASM_COPY_RVA` into the
    /// ProfileSummary record, then the renderer's own `ChrAsm::Copy` inside the profile feed) run
    /// `GaitemHandle::copy` 22 times -- a REFCOUNTING assign -- so copying foreign handles would
    /// mutate live refcount state on entries this process owns.
    ///
    /// Dropping them costs nothing visually: the render path resolves armor from
    /// `equipment_param_ids` alone (`CS::ChrAsm::GetProtectorParamIdBySlot` at deobf 0x1403be950 is
    /// `mov 0x7c(%rcx,%rdx,4),%eax`, and `FUN_1409e6fb0` feeds that straight to
    /// `EquipParamProtector::GetEntry`). No gaitem handle is read anywhere on the render path.
    /// Handles matter only because `CS::ChrAsm::EquipItem` writes a param id from a handle lookup and
    /// stores -1 when the lookup fails -- i.e. a bad handle can only destroy a good param id.
    pub fn runtime_chr_asm_image(
        self,
        pgd: SerializedPlayerGameData<'a>,
    ) -> Option<[u8; CHR_ASM_SIZE]> {
        let mut off = self.walk_to_chr_asm_sections(pgd)?;
        off = off.checked_add(SAVE_CHR_ASM_EQUIPMENT_SIZE)?; // slot indices: no runtime home
        let equipment = self
            .body
            .get(off..off + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
        off = off.checked_add(SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE)?;
        let param_ids = self.body.get(off..off + SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        off = off.checked_add(SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        // Bounds only: a save truncated before the handle section is not a whole ChrAsm and must
        // still fail the walk exactly as it did before. The bytes themselves are dropped.
        let _foreign_handles = self.body.get(off..off + SAVE_CHR_ASM_EQUIPMENT_SIZE)?;
        let mut image = [0u8; CHR_ASM_SIZE];
        for offset in [
            CHR_ASM_UNK0_OFFSET,
            CHR_ASM_UNKD4_OFFSET,
            CHR_ASM_UNKD8_OFFSET,
        ] {
            image[offset..offset + core::mem::size_of::<i32>()]
                .copy_from_slice(&CHR_ASM_OVERRIDE_ABSENT.to_le_bytes());
        }
        image[CHR_ASM_EQUIPMENT_OFFSET..CHR_ASM_EQUIPMENT_OFFSET + equipment.len()]
            .copy_from_slice(equipment);
        // `image[CHR_ASM_GAITEM_HANDLES_OFFSET..]` is left at its zero-init value: see the header.
        image[CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET
            ..CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + param_ids.len()]
            .copy_from_slice(param_ids);
        Some(image)
    }

    pub fn in_game_timer_ticks(
        self,
        player_game_data: SerializedPlayerGameData<'a>,
    ) -> Option<u32> {
        let mut offset = self.walk_to_face_section(player_game_data)?;
        Self::add_offset(&mut offset, SAVE_FACE_DATA_FULL_SIZE)?;
        Self::add_offset(&mut offset, SAVE_INVENTORY_STORAGE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GESTURE_GAME_DATA_SIZE)?;
        self.add_counted_region(&mut offset, SAVE_REGION_ID_SIZE, SAVE_REGION_COUNT_MAX)?;
        Self::add_offset(&mut offset, SAVE_RIDE_GAME_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CONTROL_BYTE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_BLOODSTAIN_DATA_SIZE)?;
        Self::add_offset(&mut offset, 4)?;
        Self::add_offset(&mut offset, 4)?;
        Self::add_offset(&mut offset, SAVE_MENU_PROFILE_SAVE_LOAD_SIZE)?;
        Self::add_offset(&mut offset, SAVE_TROPHY_EQUIP_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GAITEM_GAME_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_TUTORIAL_DATA_SIZE)?;
        Self::add_offset(&mut offset, SAVE_GLOBAL_GAME_MAN_FLAGS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_TOTAL_DEATHS_SIZE)?;
        Self::add_offset(&mut offset, SAVE_CHARACTER_TYPE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_ONLINE_SESSION_FLAG_SIZE)?;
        Self::add_offset(&mut offset, SAVE_ONLINE_CHARACTER_TYPE_FLAG_SIZE)?;
        Self::add_offset(&mut offset, SAVE_LAST_RESTED_GRACE_SIZE)?;
        Self::add_offset(&mut offset, SAVE_NOT_ALONE_FLAG_SIZE)?;
        Self::add_offset(&mut offset, SAVE_INGAME_TIMER_PADDING_AFTER_NOT_ALONE)?;
        let timer = self.read_u32(offset)?;
        (timer <= SAVE_INGAME_TIMER_TICKS_MAX).then_some(timer)
    }

    fn face_magic_offsets(self) -> impl Iterator<Item = usize> + 'a {
        self.body
            .windows(SAVE_FACE_MAGIC.len())
            .enumerate()
            .filter_map(|(offset, window)| (window == SAVE_FACE_MAGIC).then_some(offset))
            .take(SAVE_PGD_SCAN_LEADING_FACE_COUNT)
    }

    /// Locate this slot body's serialized `PlayerGameData`.
    ///
    /// Two candidate sources, one acceptance test ([`SerializedPlayerGameData::is_plausible_core`]
    /// plus the best [`SerializedPlayerGameData::score`], both unchanged).
    ///
    /// The `0xa000..=0xa600` window before each leading `FACE` magic is an observation of one
    /// save's layout, and it is too narrow. Across the ten characters of one real container the
    /// true PGD->FaceData delta ran `0x9d14..=0xa05c`, and the live default container's single
    /// character sat at `0x959c`, so the window matched one of eleven -- every other character read
    /// as an empty slot. That is what made the "Load Character from File" preview offer one row of
    /// ten (`slot_mask=0x8`, 2026-08-25) and what made `save_bytes_have_any_character` call the live
    /// default save a `native empty container` while `scripts/dump-save-slots.py` and the game
    /// itself both read a character out of it. Reproduce with
    /// `scripts/er-save-active-slots.py --deep <save>`.
    ///
    /// So the Rune Level invariant is a candidate source too, borrowed from
    /// `er_save_loader::stats` rather than re-implemented -- the same delegation `saved_map()`
    /// makes, and for the same reason: that locator carries the host tests. It is additive and
    /// ordered last, so a body the window already resolved keeps the exact offset it had (an equal
    /// score does not displace the incumbent).
    pub fn player_game_data(self) -> Option<SerializedPlayerGameData<'a>> {
        let mut best: Option<SerializedPlayerGameData<'a>> = None;
        let mut best_score = 0usize;
        let consider = |offset: usize,
                        best: &mut Option<SerializedPlayerGameData<'a>>,
                        best_score: &mut usize| {
            let candidate = SerializedPlayerGameData {
                body: self.body,
                offset,
            };
            if !candidate.is_plausible_core() {
                return;
            }
            let score = candidate.score();
            if score > *best_score {
                *best_score = score;
                *best = Some(candidate);
            }
        };
        for face_offset in self.face_magic_offsets() {
            let start = face_offset.saturating_sub(SAVE_PGD_FACE_DELTA_WINDOW_HIGH);
            let stop = face_offset.saturating_sub(SAVE_PGD_FACE_DELTA_WINDOW_LOW);
            for offset in start..=stop {
                consider(offset, &mut best, &mut best_score);
            }
        }
        if let Some(offset) = er_save_loader::stats::located_stat_block_offset(self.body)
            .and_then(|stat_base| stat_base.checked_sub(SAVE_PGD_STAT_BASE_OFFSET))
        {
            consider(offset, &mut best, &mut best_score);
        }
        best
    }
}

/// True when any of the 10 save slots holds a readable character (a PlayerGameData block passing
/// the plausibility core: level/health/stat sanity). A no-save boot natively creates a full-size
/// empty `ER0000.{sl2,co2}` container, which must not satisfy default-save discovery: observed
/// 2026-07-07, the game rewrote ER0000.sl2 during a pending missing-save-picker run, and the next
/// launch silently entered default-user-save on that zero-character container instead of
/// re-arming the picker.
pub fn save_bytes_have_any_character(bytes: &[u8]) -> bool {
    (0..TITLE_PROFILE_SLOT_COUNT).any(|slot| {
        er_save_loader::bnd4::slot_body(bytes, slot)
            .ok()
            .and_then(|body| SerializedSaveSlot::new(body).player_game_data())
            .is_some()
    })
}

impl<'a> SerializedPlayerGameData<'a> {
    fn field(&self, offset: usize, len: usize) -> Option<&'a [u8]> {
        self.body
            .get(self.offset + offset..self.offset + offset + len)
    }

    fn read_u32(&self, offset: usize) -> Option<u32> {
        self.field(offset, 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i32(&self, offset: usize) -> Option<i32> {
        self.field(offset, 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u8(&self, offset: usize) -> Option<u8> {
        self.field(offset, 1).map(|b| b[0])
    }

    fn name_bytes(&self) -> Option<&'a [u8]> {
        self.field(
            SAVE_PGD_CHARACTER_NAME_OFFSET,
            SAVE_PGD_CHARACTER_NAME_BYTES,
        )
    }

    fn name_units(&self) -> Option<Vec<u16>> {
        let bytes = self.name_bytes()?;
        Some(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|u| u16::from_le_bytes([u[0], u[1]]))
                .take_while(|u| *u != 0)
                .collect(),
        )
    }

    fn has_real_name(&self) -> bool {
        self.name_units().is_some_and(|units| {
            !units.is_empty()
                && units.iter().any(|u| *u != b'_' as u16)
                && String::from_utf16(&units)
                    .ok()
                    .is_some_and(|s| s.chars().all(|c| !c.is_control()))
        })
    }

    fn stats(&self) -> Option<[u32; SAVE_PGD_STAT_COUNT]> {
        let mut stats = [0u32; SAVE_PGD_STAT_COUNT];
        for (index, stat) in stats.iter_mut().enumerate() {
            *stat = self.read_u32(SAVE_PGD_STAT_BASE_OFFSET + index * 4)?;
        }
        Some(stats)
    }

    fn is_plausible_core(&self) -> bool {
        if self.offset + SAVE_PLAYER_GAME_DATA_MIN_SIZE > self.body.len() || !self.has_real_name() {
            return false;
        }
        let Some(level) = self.read_u32(SAVE_PGD_LEVEL_OFFSET) else {
            return false;
        };
        let Some(health) = self.read_u32(SAVE_PGD_HEALTH_OFFSET) else {
            return false;
        };
        let Some(max_health) = self.read_u32(SAVE_PGD_MAX_HEALTH_OFFSET) else {
            return false;
        };
        let Some(base_max_health) = self.read_u32(SAVE_PGD_BASE_MAX_HEALTH_OFFSET) else {
            return false;
        };
        let Some(gender) = self.read_u8(SAVE_PGD_GENDER_OFFSET) else {
            return false;
        };
        let Some(max_crimson) = self.read_u8(SAVE_PGD_MAX_CRIMSON_FLASK_OFFSET) else {
            return false;
        };
        let Some(max_cerulean) = self.read_u8(SAVE_PGD_MAX_CERULEAN_FLASK_OFFSET) else {
            return false;
        };
        let Some(stats) = self.stats() else {
            return false;
        };
        (1..=713).contains(&level)
            && (1..=100_000).contains(&health)
            && (1..=100_000).contains(&max_health)
            && (1..=100_000).contains(&base_max_health)
            && health <= max_health
            && base_max_health <= max_health
            && gender <= 1
            && max_crimson <= 14
            && max_cerulean <= 14
            && stats.iter().all(|stat| (1..=99).contains(stat))
    }

    fn score(&self) -> usize {
        self.name_units().map_or(0, |units| units.len())
            + self
                .stats()
                .map_or(0, |stats| stats.iter().filter(|stat| **stat > 0).count())
            + usize::from(self.read_u32(SAVE_PGD_LEVEL_OFFSET).unwrap_or(0) > 0)
    }

    pub fn stats_text_utf16(&self) -> Option<Vec<u16>> {
        const LABELS: [&str; SAVE_PGD_STAT_COUNT] =
            ["VIG", "MND", "END", "STR", "DEX", "INT", "FAI", "ARC"];
        let stats = self.stats()?;
        let mut s = String::new();
        for (i, label) in LABELS.iter().enumerate() {
            if i > 0 {
                s.push(' ');
            }
            s.push_str(label);
            s.push(' ');
            s.push_str(&stats[i].to_string());
        }
        Some(s.encode_utf16().chain(core::iter::once(0)).collect())
    }

    /// # Safety
    ///
    /// `profile_summary` must be the live `CS::ProfileSummary` allocation and `slot` must be under
    /// [`TITLE_PROFILE_SLOT_COUNT`]: this writes `PROFILE_SUMMARY_RECORD_STRIDE` bytes at that
    /// slot's record and one occupancy byte at `summary+0x8+slot`, through raw pointers, with no
    /// fault guard. `base` must be the running game module base -- the two native helpers called
    /// through it (`FaceData::CopyFromBuffer`, `ChrAsm::Copy`) are transmuted from `base + RVA`.
    ///
    /// `fallback_record` (when present) must be at least `PROFILE_SUMMARY_RECORD_STRIDE` bytes; it
    /// is copied verbatim as a structural template before the fields below overwrite it.
    ///
    /// Must run on the game thread, and only while the summary is not being read by the profile
    /// renderer.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn write_profile_summary_record(
        &self,
        _base: usize,
        profile_summary: usize,
        slot: usize,
        saved_map: i32,
        place_name_id: Option<u32>,
        playtime_ticks: u32,
        fallback_record: Option<&[u8]>,
        face_bytes: Option<&[u8]>,
        chr_asm_image: Option<&[u8; CHR_ASM_SIZE]>,
    ) -> bool {
        let Some(name_bytes) = self.name_bytes() else {
            return false;
        };
        let slot_data = profile_summary_record_address(profile_summary, slot);
        unsafe {
            if let Some(record) = fallback_record {
                core::ptr::copy_nonoverlapping(
                    record.as_ptr(),
                    slot_data as *mut u8,
                    PROFILE_SUMMARY_RECORD_STRIDE,
                );
            } else {
                core::ptr::write_bytes(slot_data as *mut u8, 0, PROFILE_SUMMARY_RECORD_STRIDE);
            }
            core::ptr::write_bytes(slot_data as *mut u8, 0, PROFILE_SUMMARY_NAME_BYTES);
            core::ptr::copy_nonoverlapping(
                name_bytes.as_ptr(),
                slot_data as *mut u8,
                name_bytes.len().min(PROFILE_SUMMARY_NAME_BYTES),
            );
            *(slot_data.wrapping_add(PROFILE_SUMMARY_LEVEL_OFFSET) as *mut i32) =
                self.read_i32(SAVE_PGD_LEVEL_OFFSET).unwrap_or(0);
            *(slot_data.wrapping_add(PROFILE_SUMMARY_PLAYTIME_OFFSET) as *mut u32) = playtime_ticks;
            *(slot_data.wrapping_add(PROFILE_SUMMARY_RUNE_MEMORY_OFFSET) as *mut i32) =
                self.read_i32(SAVE_PGD_RUNE_MEMORY_OFFSET).unwrap_or(0);
            *(slot_data.wrapping_add(PROFILE_SUMMARY_MAP_OFFSET) as *mut i32) = saved_map;
            // Location. Without this the row keeps whichever place name the previous save left in
            // the record, so a swap updated the name, level, play time and stats while the location
            // stayed put -- user-reported 2026-08-07. `None` leaves the field alone rather than
            // writing a zero that would render as an empty Location, and records that the row must
            // not show it: the value still sitting there is the template character's, and the field
            // is unrecoverable otherwise (it is in no character body and the game cannot recompute
            // it from a map), so the row hides it rather than printing somebody else's place.
            let slot_bit = 1usize << slot;
            if let Some(place_name_id) = place_name_id {
                *(slot_data.wrapping_add(PROFILE_SUMMARY_PLACE_NAME_OFFSET) as *mut u32) =
                    place_name_id;
                PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.fetch_and(!slot_bit, Ordering::SeqCst);
            } else {
                PROFILE_PREVIEW_PLACE_NAME_UNSOURCED.fetch_or(slot_bit, Ordering::SeqCst);
            }
            // Visual identity (second-load wrong-head root fix, user-identified 2026-07-06: "Banon in
            // all three windows"). The fallback record above is a structural template cloned from the
            // original save's first active slot -- its FaceData (+0x38) and ChrAsm (+0x1a8) describe
            // that character, so every foreign row's portrait rendered the original character while
            // the overwritten name/level kept the stats text correct. Fill the real visual blocks from
            // the foreign character's save bytes through the game's own copy helpers: the section-walk
            // locators handle the save's variable-length layout (fixed runtime offsets false-negatived
            // on every slot, run portrait-faceid-switchqa-20260706-142552), and the saved FaceData
            // wrapper header does not match the live one, so CopyFromBuffer -- never a raw memcpy.
            if let (Some(face), Some(chr_asm_image)) = (face_bytes, chr_asm_image) {
                let copy_face_data_from_buffer: unsafe extern "system" fn(usize, usize) =
                    std::mem::transmute(
                        match gated_summary_fn(
                            FACE_DATA_COPY_FROM_BUFFER_RVA,
                            "FACE_DATA_COPY_FROM_BUFFER_RVA",
                        ) {
                            Some(address) => address,
                            None => return false,
                        },
                    );
                let copy_chr_asm: unsafe extern "system" fn(usize, usize) -> usize =
                    std::mem::transmute(
                        match gated_summary_fn(CHR_ASM_COPY_RVA, "CHR_ASM_COPY_RVA") {
                            Some(address) => address,
                            None => return false,
                        },
                    );
                copy_face_data_from_buffer(
                    slot_data.wrapping_add(PROFILE_SUMMARY_FACE_DATA_OFFSET),
                    face.as_ptr() as usize,
                );
                copy_chr_asm(
                    slot_data.wrapping_add(PROFILE_SUMMARY_CHR_ASM_OFFSET),
                    chr_asm_image.as_ptr() as usize,
                );
                for (record_off, save_pgd_off) in [
                    (PROFILE_SUMMARY_GENDER_OFFSET, SAVE_PGD_GENDER_OFFSET),
                    (PROFILE_SUMMARY_ARCHETYPE_OFFSET, SAVE_PGD_ARCHETYPE_OFFSET),
                    (
                        PROFILE_SUMMARY_STARTING_GIFT_OFFSET,
                        SAVE_PGD_STARTING_GIFT_OFFSET,
                    ),
                    (PROFILE_SUMMARY_FIELD_C4_OFFSET, SAVE_PGD_FIELD_C4_OFFSET),
                ] {
                    if let Some(v) = self.read_u8(save_pgd_off) {
                        *(slot_data.wrapping_add(record_off) as *mut u8) = v;
                    }
                }
                // `er_gfx::title_05_000::fnv1a64` in the product; that name is a `pub use` of
                // this function, so the hash is byte-identical and the crate keeps its dependency
                // on the shared primitive rather than on the GFx parser.
                PROFILE_PREVIEW_FACE_HASH[slot].store(
                    er_game_base::fnv1a::fnv1a64(face) as usize,
                    Ordering::SeqCst,
                );
            } else {
                PROFILE_PREVIEW_FACE_HASH[slot].store(0, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "system-quit-load-save-profiles: slot {slot} FOREIGN visual data unavailable (face_located={} chr_asm_located={}); record keeps the fallback character's face/equipment",
                    face_bytes.is_some(),
                    chr_asm_image.is_some()
                ));
            }
            *(profile_summary.wrapping_add(PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET + slot)
                as *mut u8) = 1;
        }
        true
    }
}

#[cfg(test)]
mod loading_cover_chr_asm_image_tests {
    // The two the module body does not need: the entry count and the handle-array offset are
    // read only to prove the assembled image's shape.
    use er_loading_portrait_core::chr_asm_layout::{
        CHR_ASM_EQUIPMENT_ENTRY_COUNT, CHR_ASM_GAITEM_HANDLES_OFFSET,
    };

    use super::*;

    /// Distinctive synthetic section payloads -- no game bytes are read or versioned here. Handles
    /// carry the real `0x8xxxxxxx` gaitem shape so a stray copy of them is unmistakable in the image.
    const TEST_EQUIPMENT_FILL: u8 = 0xa5;
    const TEST_PARAM_ID_BASE: i32 = 0x0001_0000;
    const TEST_HANDLE_BASE: u32 = 0x8000_0000;
    const CHR_ASM_ENTRY_COUNT: usize = SAVE_CHR_ASM_EQUIPMENT_SIZE / core::mem::size_of::<i32>();

    fn param_id_at(index: usize) -> i32 {
        TEST_PARAM_ID_BASE + index as i32
    }

    /// A save body whose ChrAsm region holds the four serialized sections in save order:
    /// `[slot indices][ChrAsmEquipment][param ids][gaitem handles]`.
    fn synthetic_save_body() -> Vec<u8> {
        let prefix = SAVE_PLAYER_GAME_DATA_MIN_SIZE + SAVE_SPEFFECT_COUNT * SAVE_SPEFFECT_SIZE;
        let mut body = vec![0u8; prefix];
        body.resize(body.len() + SAVE_CHR_ASM_EQUIPMENT_SIZE, 0u8); // slot indices
        body.resize(
            body.len() + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE,
            TEST_EQUIPMENT_FILL,
        );
        for index in 0..CHR_ASM_ENTRY_COUNT {
            body.extend(param_id_at(index).to_le_bytes());
        }
        for index in 0..CHR_ASM_ENTRY_COUNT {
            body.extend((TEST_HANDLE_BASE | index as u32).to_le_bytes());
        }
        body
    }

    fn image_from(body: &[u8]) -> Option<[u8; CHR_ASM_SIZE]> {
        SerializedSaveSlot::new(body)
            .runtime_chr_asm_image(SerializedPlayerGameData { body, offset: 0 })
    }

    fn image_i32_at(image: &[u8; CHR_ASM_SIZE], offset: usize) -> i32 {
        i32::from_le_bytes(
            image[offset..offset + core::mem::size_of::<i32>()]
                .try_into()
                .expect("4 bytes"),
        )
    }

    /// The fix (bd er-effects-rs-wncc). `FUN_1409e6fb0` tests these three signed and treats a
    /// non-negative value as a forced whole-outfit override, so a zero here resolves the four
    /// protector slots to 0/100/200/300 and the portrait renders entirely nude -- default underwear
    /// included. A ctor-built `ChrAsm` holds -1; our hand-built image must too.
    #[test]
    fn the_three_whole_outfit_override_sentinels_are_minus_one_not_zero() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        for (name, offset) in [
            ("unk0", CHR_ASM_UNK0_OFFSET),
            ("unkd4", CHR_ASM_UNKD4_OFFSET),
            ("unkd8", CHR_ASM_UNKD8_OFFSET),
        ] {
            assert_eq!(
                image_i32_at(&image, offset),
                CHR_ASM_OVERRIDE_ABSENT,
                "{name} (+{offset:#x}) must be the no-override sentinel"
            );
        }
    }

    /// The ctor writes `unk4` and the +0xdc..+0xe8 tail as zero, so the image must leave them zero --
    /// matching the ctor exactly, no wider.
    #[test]
    fn only_those_three_fields_are_seeded_the_rest_of_the_header_and_tail_stay_zero() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        let unk4 = CHR_ASM_UNK0_OFFSET + core::mem::size_of::<i32>();
        assert_eq!(image_i32_at(&image, unk4), 0, "unk4 must stay zero");
        let tail = CHR_ASM_UNKD8_OFFSET + core::mem::size_of::<i32>();
        assert!(
            image[tail..].iter().all(|byte| *byte == 0),
            "the +0xdc tail must stay zero, got {:02x?}",
            &image[tail..]
        );
    }

    /// The sentinel offsets must sit exactly one param-id array past the last public field and must
    /// stay inside the struct -- `unkd4`/`unkd8` are private in `fromsoftware-rs`, so this is what
    /// stands in for `offset_of!` if the typed layout ever moves.
    #[test]
    fn the_sentinel_offsets_agree_with_the_typed_chr_asm_layout() {
        assert_eq!(CHR_ASM_UNK0_OFFSET, 0);
        assert_eq!(CHR_ASM_UNKD4_OFFSET, 0xd4);
        assert_eq!(CHR_ASM_UNKD8_OFFSET, 0xd8);
        assert_eq!(
            CHR_ASM_EQUIPMENT_ENTRY_COUNT,
            SAVE_CHR_ASM_EQUIPMENT_SIZE / core::mem::size_of::<i32>(),
            "the save section and the runtime array must hold the same number of entries"
        );
        assert!(CHR_ASM_UNKD8_OFFSET + core::mem::size_of::<i32>() <= CHR_ASM_SIZE);
    }

    /// A foreign save's gaitem handles index a `gaitemInsTable` this process never populated, so they
    /// must never reach the refcounting `ChrAsm::Copy` -- not for the ProfileSummary record and not
    /// for the renderer. Costless visually: the render path never reads a handle.
    #[test]
    fn the_foreign_saves_gaitem_handles_are_never_copied_into_the_runtime_image() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        let handles = &image[CHR_ASM_GAITEM_HANDLES_OFFSET
            ..CHR_ASM_GAITEM_HANDLES_OFFSET + SAVE_CHR_ASM_EQUIPMENT_SIZE];
        assert!(
            handles.iter().all(|byte| *byte == 0),
            "gaitem handle array must be zeroed, got {handles:02x?}"
        );
    }

    /// The param ids are the only armor source the render path reads, so zeroing the handles must not
    /// take them with it.
    #[test]
    fn the_equipment_param_ids_survive_verbatim() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        for index in 0..CHR_ASM_ENTRY_COUNT {
            let at = CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + index * core::mem::size_of::<i32>();
            let got = i32::from_le_bytes(image[at..at + 4].try_into().expect("4 bytes"));
            assert_eq!(got, param_id_at(index), "param id {index} drifted");
        }
    }

    #[test]
    fn the_chr_asm_equipment_block_still_lands_at_its_runtime_offset() {
        let body = synthetic_save_body();
        let image = image_from(&body).expect("synthetic body walks to the ChrAsm sections");
        let equipment = &image[CHR_ASM_EQUIPMENT_OFFSET
            ..CHR_ASM_EQUIPMENT_OFFSET + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE];
        assert!(equipment.iter().all(|byte| *byte == TEST_EQUIPMENT_FILL));
    }

    /// Dropping the handle bytes must not drop the bounds check they carried: a body truncated
    /// inside the handle section is not a whole serialized ChrAsm and must still be rejected.
    #[test]
    fn a_body_truncated_inside_the_handle_section_is_still_rejected() {
        let mut body = synthetic_save_body();
        body.truncate(body.len() - 1);
        assert!(image_from(&body).is_none());
    }
}

/// Corpus diagnostics for the serialized `ChrAsmEquipment` block, run against a real save named by
/// `ER_ARM_STYLE_SAVE` (`<path>:<slot>`). Skips when unset, so it costs nothing in CI.
///
/// Why it exists. The loading portrait picks its idle animation from `armStyle`, and a user
/// observed the portrait one-handed while the same save loaded into the world two-handing -- so the
/// grip is in the save and our reading of it disagrees with the game's. The runtime layout is
/// `ChrAsmEquipment { armStyle @ +0, selectedSlots @ +4 }` (Ghidra 1.16.2, 28 bytes); whether the
/// save serializes it in that order is exactly what this dumps rather than assumes.
#[cfg(test)]
mod arm_style_corpus {
    use super::*;

    /// Legal `ChrAsmArmStyle` values. `CS::ChrIns::IsTwoHanding` (deobf 0x1403f4930) is
    /// `ADD EAX,-0x2 ; CMP EAX,0x1 ; SETBE`, so 2 and 3 are the two-handed pair and the enum has at
    /// least four members.
    const ARM_STYLE_MAX: u32 = 3;

    #[test]
    fn the_serialized_equipment_block_holds_a_legal_arm_style_somewhere() {
        let Ok(spec) = std::env::var("ER_ARM_STYLE_SAVE") else {
            eprintln!("ER_ARM_STYLE_SAVE unset -- skipping");
            return;
        };
        let (path, slot) = spec.rsplit_once(':').expect("<path>:<slot>");
        let slot: usize = slot.parse().expect("slot index");
        let data = std::fs::read(path).expect("save readable");
        let body = er_save_loader::bnd4::slot_body(&data, slot).expect("slot body");
        let pgd = SerializedSaveSlot::new(body)
            .player_game_data()
            .expect("player game data");
        let mut off = SerializedSaveSlot::new(body)
            .walk_to_chr_asm_sections(pgd)
            .expect("chr asm sections");
        off += SAVE_CHR_ASM_EQUIPMENT_SIZE;
        let equipment = &body[off..off + SAVE_ARM_STYLE_ACTIVE_WEAPON_SLOTS_SIZE];
        let dwords: Vec<u32> = equipment
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect();
        eprintln!("serialized ChrAsmEquipment dwords for {path} slot {slot}: {dwords:?}");
        assert!(
            dwords.iter().any(|d| *d <= ARM_STYLE_MAX),
            "no dword in the block is a legal arm style: {dwords:?}"
        );
    }
}
