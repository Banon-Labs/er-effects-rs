//! Re-deriving one record from the character that is live right now.
//!
//! # The gap this closes
//!
//! [`crate::save_bytes_records`] rebuilds records from a save container's bytes. That is the right
//! source when the question is "what does this file hold", and the wrong one when the question is
//! "what is the player wearing this instant" -- a build import mutates `PlayerGameData` and writes
//! no save, so the container is behind and so is every record derived from it.
//!
//! The game has its own answer and this module calls it rather than re-implementing it.
//! [`PROFILE_SUMMARY_UPDATE_FROM_LIVE_PLAYER_RVA`] is
//! `CS::ProfileSummary::UpdateRecordFromLivePlayer(summary, slot)` in all but name, and its whole
//! body is the live character copied into one record:
//!
//! ```text
//! FUN_140262270(ProfileSummary *summary, uint slot)   // 1.16.2; refuses slot >= 10
//!   pgd = GameDataMan->mainPlayerGameData
//!   wcsncpy(record+0x00, FUN_14025f8e0(pgd), 0x10)    // name, terminated at +0x20
//!   record+0x24 = pgd->level
//!   record+0x2c = pgd->runeMemory
//!   record+0x28 = GetPlayTimeInSeconds()
//!   FUN_14025f900(pgd, record)                        // FaceData +0x38, ChrAsm +0x1a8, gender +0x290
//!   record+0x291 = pgd->archetype
//!   record+0x292 = pgd->startingGift
//!   record+0x293 = pgd->field_0xc4
//!   record+0x30  = GetCurrentMapId()
//!   record+0x34  = CSFeMan+0x653c                     // the PlaceName a row's Location shows
//! ```
//!
//! and the equipment step inside it is the one that matters here:
//!
//! ```text
//! FUN_14025f900(PlayerGameData *pgd, longlong record)
//!   FUN_1403bffa0(record+0x1a8)                       // release the 22 handles the record held
//!   FUN_140251580(record+0x38, &pgd->faceData)
//!   FUN_140245c00(record+0x1a8, &pgd->equipGameData.chrAsm)   // ChrAsm::Copy, live -> record
//!   record+0x290 = pgd->gender
//!   FUN_1403bffa0(record+0x1a8)                       // un-index the handles just copied in
//! ```
//!
//! # Why the record is what a portrait shows
//!
//! `FUN_1409aa680` -- `er_loading_portrait_core::PROFILE_RENDERER_REFRESH_RVA` -- is the only
//! caller of the profile renderer's set-ChrAsm (`FUN_140bbe1a0`; one call xref in the 1.16.2 dump,
//! the other two being vtable data), and the source it passes is `record + 0x1a8`. So a profile
//! portrait is dressed from a record and from nothing else, and a record that still describes the
//! pre-import loadout renders the pre-import loadout however many times the model is rebuilt.
//! Syncing first is what makes a rebuild worth asking for.
//!
//! # When the game does this by itself
//!
//! Only at a save. The two callers of the native in the 1.16.2 dump are `FUN_14067b750` and
//! `FUN_14067b940`, both `GameMan` save lanes, and each opens with
//! `MarkProfileIndexAsUsed(summary, slot)` then this function then the serialize. So an import that
//! writes no save leaves the record behind until something else saves -- which is the whole defect.

use er_game_base::mem::{game_rva_named, safe_read_i32};
use er_game_base::profile_summary::{
    PROFILE_SUMMARY_CHR_ASM_OFFSET, PROFILE_SUMMARY_LEVEL_OFFSET,
    PROFILE_SUMMARY_SLOT_COUNT as SLOT_COUNT, profile_summary_record_address,
};
use er_loading_portrait_core::{CHR_ASM_EQUIPMENT_ENTRY_COUNT, CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET};

use crate::equip_fingerprint::{LiveSync, RecordEquipment, equipment_fingerprint};
use crate::host::append_autoload_debug;
use crate::live_records::system_quit_profile_summary_ptr;

/// `CS::ProfileSummary::UpdateRecordFromLivePlayer(ProfileSummary*, uint slot)` -- the game's own
/// live-character-into-one-record writer, decompiled in this module's header.
///
/// Verified onto the installed build rather than assumed. The 1.16.2 entry byte-matches
/// `eldenring-deobf.bin` at shift zero (16 instructions identical,
/// `python3 scripts/check-dump-deobf-identity.py 0x140262270`), and
/// `docs/recon/rva-map-1162-to-1170.verified.tsv` carries `0x140262270 -> 0x140262280` as
/// `IDENTICAL-WHOLE` over 99 instructions, both entries declared, `PDATA:0x179/0x179`. The `+0x10`
/// step is the one its neighbour `MarkProfileIndexAsUsed` (`0x140262250 -> 0x140262260`) already
/// carries, and the pair sits below the `0xafefe9` boundary, so 1.17.1 does not move it again.
/// Every call goes through [`game_rva_named`], so a build with no row for it refuses rather than
/// transferring control into whatever now occupies the address.
pub const PROFILE_SUMMARY_UPDATE_FROM_LIVE_PLAYER_RVA: usize = 0x262270;

/// The host-side test in [`crate::equip_fingerprint`] spells the array length itself, because the
/// typed constant is behind the Windows-only game bindings. This is the assertion that keeps the
/// two from drifting apart.
const _: () = assert!(CHR_ASM_EQUIPMENT_ENTRY_COUNT == 22);

/// Read the `equipment_param_ids` out of a `ChrAsm`-shaped block.
///
/// Works on a record's `+0x1a8` block and on a live `CSMenuProfModelRend` stage alike, because both
/// are `CS::ChrAsm` images and the array sits at the same offset in each. That is the point: one
/// function reads both sides of the record-against-renderer comparison.
///
/// # Safety
///
/// `chr_asm` must address a `CS::ChrAsm`-shaped block. Every dword is read through
/// `ReadProcessMemory`, so a stale or unmapped pointer yields `None` rather than faulting -- but a
/// pointer to a different live object would be read as though it were a `ChrAsm`.
pub unsafe fn read_equipment_ids(chr_asm: usize) -> Option<[i32; CHR_ASM_EQUIPMENT_ENTRY_COUNT]> {
    let mut ids = [0i32; CHR_ASM_EQUIPMENT_ENTRY_COUNT];
    for (index, id) in ids.iter_mut().enumerate() {
        let at = chr_asm + CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET + index * core::mem::size_of::<i32>();
        *id = unsafe { safe_read_i32(at) }?;
    }
    Some(ids)
}

/// Fingerprint a `ChrAsm`-shaped block in one call. `None` when it could not be read whole, which
/// the comparison treats as no measurement rather than as a mismatch.
///
/// # Safety
///
/// As [`read_equipment_ids`].
pub unsafe fn chr_asm_equipment_fingerprint(chr_asm: usize) -> Option<u64> {
    unsafe { read_equipment_ids(chr_asm) }.map(|ids| equipment_fingerprint(&ids))
}

/// What one record says about the gear it will dress a portrait in. `None` when it could not be
/// read whole.
///
/// # Safety
///
/// `record` must address a `ProfileSummaryRecord`. Fault-guarded throughout.
unsafe fn read_record_equipment(record: usize) -> Option<RecordEquipment> {
    let level = unsafe { safe_read_i32(record + PROFILE_SUMMARY_LEVEL_OFFSET) }?;
    let fingerprint =
        unsafe { chr_asm_equipment_fingerprint(record + PROFILE_SUMMARY_CHR_ASM_OFFSET) }?;
    Some(RecordEquipment { level, fingerprint })
}

/// Re-derive `slot`'s record from the character that is live right now, through the game's own
/// writer.
///
/// Idempotent in the sense that matters: it writes whatever `PlayerGameData` currently says, so
/// calling it twice with nothing changed in between leaves the same bytes. It is not free -- the
/// native releases and re-acquires 22 gaitem handles each time -- so it belongs at an edge (a build
/// was just imported), never on a per-frame cadence.
///
/// `slot` must be the slot the live character occupies. Writing the live character into another
/// character's record would overwrite that character's summary, which is why the caller resolves
/// the slot from the game's own answer rather than from a portrait's binding.
///
/// # Safety
///
/// Game thread, character in the world. The native reads `GameDataMan->mainPlayerGameData`,
/// `CSPlaygo`, `CSFeMan` and `CSLuaEventMan`, and it derives and writes through the record pointer
/// itself, so a caller must not hold a record pointer of its own across the call.
pub unsafe fn sync_record_from_live_player(slot: i32) -> LiveSync {
    if !(0..SLOT_COUNT as i32).contains(&slot) {
        return LiveSync::SlotOutOfRange(slot);
    }
    let summary = unsafe { system_quit_profile_summary_ptr() };
    if summary == 0 {
        return LiveSync::NoSummary;
    }
    let Ok(address) = game_rva_named(
        PROFILE_SUMMARY_UPDATE_FROM_LIVE_PLAYER_RVA as u32,
        "PROFILE_SUMMARY_UPDATE_FROM_LIVE_PLAYER_RVA",
    ) else {
        return LiveSync::NativeUnmapped;
    };
    let record = profile_summary_record_address(summary, slot as usize);
    let before = unsafe { read_record_equipment(record) };
    // Safety: resolved for the running build immediately above. The game thread and a live
    // character are this function's own contract, and they are exactly what the two native save
    // lanes hold when they call it.
    let update: unsafe extern "system" fn(usize, u32) =
        unsafe { core::mem::transmute::<usize, unsafe extern "system" fn(usize, u32)>(address) };
    unsafe { update(summary, slot as u32) };
    let after = unsafe { read_record_equipment(record) };
    append_autoload_debug(format_args!(
        "profile-summary: re-derived record slot {slot} from the live character -- level {} -> {}, equipment fingerprint 0x{:016x} -> 0x{:016x}",
        before.map_or(0, |e| e.level),
        after.map_or(0, |e| e.level),
        before.map_or(0, |e| e.fingerprint),
        after.map_or(0, |e| e.fingerprint),
    ));
    LiveSync::Synced {
        slot,
        before,
        after,
    }
}
