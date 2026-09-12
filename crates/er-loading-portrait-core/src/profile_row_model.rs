//! The `05_010_ProfileSelect` row model, and the two `CS::MenuString` fields a populate reads.
//!
//! One native function builds every character-summary row in the game, and two features write to
//! it: the loading-cover stats panel composes a merged `PlayerName` header, and the System>Quit
//! save picker repurposes `Location` for a browse file's last-saved time. Both do it the same way,
//! and the way matters more than the convenience.
//!
//! # Why the write is a staged pointer rather than a SetText
//!
//! The native populate reads the model's `rawString` pointer and SetTexts whatever it finds, so
//! the row's own draw writes the text. A row clip is recycled across different files and slots, so
//! text pushed out-of-band can survive onto a row it does not describe. Here there is nothing to
//! survive: the pointer exists only across the one populate call it belongs to, and a row that
//! stages nothing gets the game's own string rather than a stale one.
//!
//! So every staged field has to be put back the moment that call returns -- which is why each
//! `stage_*` hands back the pointer it displaced instead of remembering it. Two features staging
//! the same row in one chained call each hold their own displaced pointer, and unwinding in
//! reverse order restores the model exactly as the game built it.
//!
//! # Layout
//!
//! The model is `CS::MenuSaveDataSummary`, and each staged field is a `CS::MenuString`:
//! `{ wchar_t* rawString; DLString<wchar_t> dLString; }`, 0x38 bytes (Ghidra, 1.16.2). Every
//! reader takes `rawString` when it is non-null and falls back to the inline `DLString` buffer
//! otherwise -- the row populate `FUN_1408757e0` and the FMG static pass `FUN_14074c540` spell the
//! same accessor. `FUN_1408757e0` reads the raw pointer at `rowModel + 0x50` for `PlayerName`, and
//! the inline buffer at `rowModel + 0x60` only when the capacity at `rowModel + 0x78` says it is
//! heap-backed. Staging the raw pointer therefore wins over the fallback without touching it.
//!
//! Offsets measured on the row model `FUN_1408759e0` composes (`eldenring-deobf.bin`, 1.16.2;
//! unmoved on 1.17.1, which shifted only `.text`).

use std::sync::atomic::{AtomicUsize, Ordering};

/// Save slot index the row describes (0-9). The native populate reads `*(int*)(rowModel + 0x8) + 1`
/// as the `Icon_0` face-sprite frame, so this is the same field the game indexes by.
///
/// It is `-1` on a row with no slot behind it, and `0` for both save slot 0's row and the transient
/// current-player summary -- so this field alone cannot answer "whose character is this", and a
/// caller that needs to must ask the builder it is nested in.
pub const PROFILE_ROW_MODEL_SLOT_08_OFFSET: usize = 0x8;

/// `PlayerName`, the row's top-left field. The merged `<name> RL <level>` header is staged here.
pub const PROFILE_ROW_MODEL_PLAYER_NAME_MENUSTRING_50_OFFSET: usize = 0x50;

/// `Location`, the row's top-right field, on the same visual line as `PlayerName`. A browse
/// save-file row stages its last-saved timestamp here rather than into `PlayTime`, so filename and
/// time share one line.
pub const PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET: usize = 0x90;

/// Rows whose staged pointer could not be read back, summed across every field and feature. A
/// non-zero count means some row rendered the game's string where a feature meant to write one.
pub static PROFILE_ROW_MODEL_STAGE_REFUSALS: AtomicUsize = AtomicUsize::new(0);

/// Point `row_model + offset`'s `CS::MenuString` at `text`, returning the pointer it displaced.
///
/// `None` when the field is unreadable, in which case nothing is written and the row keeps the
/// game's own string.
///
/// # Safety
///
/// `row_model` must be a live row model inside its populate call, and `text` must be a
/// nul-terminated UTF-16 buffer that outlives that call.
pub unsafe fn stage_row_model_menu_string(
    row_model: usize,
    offset: usize,
    text: *const u16,
) -> Option<usize> {
    let field = row_model + offset;
    let Some(displaced) = (unsafe { er_game_base::mem::safe_read_usize(field) }) else {
        PROFILE_ROW_MODEL_STAGE_REFUSALS.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    unsafe { (field as *mut usize).write_volatile(text as usize) };
    Some(displaced)
}

/// Put back whatever [`stage_row_model_menu_string`] displaced at the same offset.
///
/// # Safety
///
/// Same call context as the `stage` that produced `displaced`, and `displaced` must be that call's
/// return rather than another field's.
pub unsafe fn restore_row_model_menu_string(row_model: usize, offset: usize, displaced: usize) {
    let field = row_model + offset;
    unsafe { (field as *mut usize).write_volatile(displaced) };
}

/// Stage the row's `PlayerName`. See [`stage_row_model_menu_string`].
///
/// # Safety
///
/// As [`stage_row_model_menu_string`].
pub unsafe fn stage_row_model_player_name(row_model: usize, text: *const u16) -> Option<usize> {
    unsafe {
        stage_row_model_menu_string(
            row_model,
            PROFILE_ROW_MODEL_PLAYER_NAME_MENUSTRING_50_OFFSET,
            text,
        )
    }
}

/// Put back whatever [`stage_row_model_player_name`] displaced.
///
/// # Safety
///
/// As [`restore_row_model_menu_string`].
pub unsafe fn restore_row_model_player_name(row_model: usize, displaced: usize) {
    unsafe {
        restore_row_model_menu_string(
            row_model,
            PROFILE_ROW_MODEL_PLAYER_NAME_MENUSTRING_50_OFFSET,
            displaced,
        )
    };
}

/// Stage the row's `Location`. See [`stage_row_model_menu_string`].
///
/// # Safety
///
/// As [`stage_row_model_menu_string`].
pub unsafe fn stage_row_model_location(row_model: usize, text: *const u16) -> Option<usize> {
    unsafe {
        stage_row_model_menu_string(
            row_model,
            PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET,
            text,
        )
    }
}

/// Put back whatever [`stage_row_model_location`] displaced.
///
/// # Safety
///
/// As [`restore_row_model_menu_string`].
pub unsafe fn restore_row_model_location(row_model: usize, displaced: usize) {
    unsafe {
        restore_row_model_menu_string(
            row_model,
            PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET,
            displaced,
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two staged fields must not share an offset. They are written in the same call and
    /// unwound separately, so an aliased pair would have the second restore put back the first
    /// field's pointer and leave a dangling one on screen.
    #[test]
    fn the_two_staged_fields_are_distinct_offsets() {
        assert_ne!(
            PROFILE_ROW_MODEL_PLAYER_NAME_MENUSTRING_50_OFFSET,
            PROFILE_ROW_MODEL_LOCATION_MENUSTRING_90_OFFSET
        );
    }

    /// A stage/restore round trip on a plain heap cell, which is the whole contract: the displaced
    /// pointer comes back and the cell ends where it started. No game memory is involved, so this
    /// runs on the host as well as under wine.
    #[test]
    fn a_round_trip_restores_the_pointer_the_game_had() {
        let mut cell: usize = 0xdead_beef;
        let model = (&raw mut cell) as usize;
        let text: Vec<u16> = "x\0".encode_utf16().collect();
        let displaced = unsafe { stage_row_model_menu_string(model, 0, text.as_ptr()) };
        assert_eq!(displaced, Some(0xdead_beef));
        assert_eq!(cell, text.as_ptr() as usize);
        unsafe { restore_row_model_menu_string(model, 0, displaced.unwrap()) };
        assert_eq!(cell, 0xdead_beef);
    }
}
