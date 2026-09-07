//! Read the live `CS::ProfileSummary` -- the pointer, and what one record says about its slot.
//!
//! `system_quit_profile_summary_ptr` moved from
//! `experiments/startup_hooks/loading_cover/loading_cover_save_slot.rs` and
//! `profile_slot_has_character` with it; `profile_slot_fingerprint` moved from
//! `experiments/continue_load/slot_resolution.rs`. All three walked `GameDataMan` to the same
//! pointer and then read the same record table, from three different files.
//!
//! The pointer walk is the one route to the summary: `er-quit-menu-core`'s
//! `system_quit_profile_summary_ptr` host field is installed with this function, so the quit menu
//! and the autoload chain cannot disagree about where the records are.

use er_game_base::mem::safe_read_usize;
use er_game_base::pgd::{read_utf16_name_units, utf16_name_empty_like};
use er_game_base::profile_summary::{
    GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET as SLOT_MANAGER_CONTAINER_OFFSET,
    PROFILE_SUMMARY_LEVEL_OFFSET, PROFILE_SUMMARY_MAP_OFFSET,
    PROFILE_SUMMARY_SLOT_COUNT as TITLE_PROFILE_SLOT_COUNT, profile_summary_record_address,
};

use crate::host::game_data_man_ptr_or_null;
use crate::reassert_policy::{RecordIdentity, name_hash_utf16};
use crate::slot_identity::record_is_real_character;

/// A null game pointer. Named after the product constant the moved code read
/// (`TITLE_OWNER_SCAN_START_ADDRESS`, which is simply `usize::MIN`) so the bodies below stay
/// byte-for-byte the code that was moved.
const TITLE_OWNER_SCAN_START_ADDRESS: usize = usize::MIN;

/// The lowest valid ProfileSummary slot index.
///
/// The product spells this `OWN_STEPPER_SLOT_ZERO` and it lives in `er-title-flow`, which this
/// crate must not depend on: er-title-flow is this crate's natural consumer, so the edge would
/// close a cycle. Same value (0), stated here as what it means to a record table.
const PROFILE_SLOT_INDEX_ZERO: i32 = 0;

/// The live `CS::ProfileSummary`, or 0.
///
/// `GameDataMan+0x78` -- the single edge from the singleton to the ten records. Fault-guarded:
/// an unreadable pointer reads as 0 rather than faulting, so a caller that forgets the null check
/// writes nowhere.
///
/// # Safety
///
/// No precondition: the read goes through `ReadProcessMemory` and fails closed. The returned value
/// is a sample -- the game may free the allocation on another thread -- so callers must not hold it
/// across a frame boundary.
pub unsafe fn system_quit_profile_summary_ptr() -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return null;
    }
    unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
}

/// True if ProfileSummary slot `slot` holds a real character (non-empty saved name). Used to gate the
/// human-driven in-world Load-Profile pick so activating an empty slot never arms a switch (which
/// would tear the world down to a clean title and then fail the fresh deserialize, stranding the game
/// at a blank title). Reads the same save-record table the identity semaphore uses -- fault-guarded,
/// returns false on any unreadable pointer so an empty/unknown slot is treated as "no character".
///
/// # Safety
///
/// No precondition: every read goes through `ReadProcessMemory` and fails closed.
pub unsafe fn profile_slot_has_character(slot: i32) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return false;
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == null {
        return false;
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null);
    if profile_summary == null {
        return false;
    }
    let rec = profile_summary_record_address(profile_summary, slot as usize);
    let (name, len) = unsafe { read_utf16_name_units(rec) };
    !utf16_name_empty_like(&name, len)
}

/// What one live record says about its slot: `(is a real character, map, level, name length)`.
///
/// The realness verdict is [`record_is_real_character`]; everything else in the tuple is the raw
/// field, for the callers that log or compare it.
///
/// # Safety
///
/// No precondition: every read goes through `ReadProcessMemory` and fails closed, so an
/// unallocated summary, an out-of-range slot or a torn record produce
/// `(false, -1, 0, 0)` rather than a fault.
pub unsafe fn profile_slot_fingerprint(slot: i32) -> (bool, i32, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U32: u32 = 0;
    const NAME_LEN_NONE: usize = 0;
    if slot < PROFILE_SLOT_INDEX_ZERO {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == NULL {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    if profile_summary == NULL {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let rec = profile_summary_record_address(profile_summary, slot as usize);
    let profile_map = unsafe { safe_read_usize(rec + PROFILE_SUMMARY_MAP_OFFSET) }
        .map(|value| value as u32 as i32)
        .unwrap_or(BAD_I32);
    let profile_level = unsafe { safe_read_usize(rec + PROFILE_SUMMARY_LEVEL_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (profile_name, profile_name_len) = unsafe { read_utf16_name_units(rec) };
    let profile_name_empty = utf16_name_empty_like(&profile_name, profile_name_len);
    (
        record_is_real_character(profile_level, profile_name_empty),
        profile_map,
        profile_level,
        profile_name_len,
    )
}

/// The name + level identity of one live record, for the drift watch.
///
/// Separate from [`profile_slot_fingerprint`] because that answers "is this a character" and hands
/// back a name length -- two different names of the same length read as identical to it, which is
/// exactly the comparison this must not get wrong. The name is hashed rather than returned so the
/// caller can hold it in an atomic and compare it every tick without allocating.
///
/// # Safety
///
/// No precondition: every read is fault-guarded, so an unallocated summary or an out-of-range slot
/// returns the default (not-a-character) identity rather than faulting.
pub unsafe fn record_identity(slot: i32) -> RecordIdentity {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    if !(PROFILE_SLOT_INDEX_ZERO..TITLE_PROFILE_SLOT_COUNT as i32).contains(&slot) {
        return RecordIdentity::default();
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == NULL {
        return RecordIdentity::default();
    }
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    if summary == NULL {
        return RecordIdentity::default();
    }
    let rec = profile_summary_record_address(summary, slot as usize);
    let (units, len) = unsafe { read_utf16_name_units(rec) };
    let level = unsafe { safe_read_usize(rec + PROFILE_SUMMARY_LEVEL_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(0);
    RecordIdentity {
        name_hash: name_hash_utf16(&units[..len]),
        level,
    }
}
