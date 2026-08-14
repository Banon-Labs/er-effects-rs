//! Live `CS::PlayerGameData` identity checks shared by portrait and load-flow callers.

use crate::prelude::*;

/// Return `(is_real, level, name_len)` for the currently mounted PlayerGameData character.
///
/// A real character has level >= 1 and a non-empty-like UTF-16 name. Every memory access is routed
/// through the fault-tolerant er-game-base readers, so missing or unreadable state fails closed.
///
/// # Safety
///
/// Reads process memory through the installed GameDataMan host pointer. The reads themselves are
/// guarded by `ReadProcessMemory`; callers must still treat a false result as an unavailable sample.
pub unsafe fn char_fingerprint(_base: usize) -> (bool, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ZERO_U32: u32 = 0;
    const MIN_REAL_LEVEL: u32 = 1;
    const NAME_LEN_NONE: usize = 0;
    let gdm = game_data_man_ptr_or_null();
    let pgd = if gdm != NULL {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if pgd == NULL {
        return (false, ZERO_U32, NAME_LEN_NONE);
    }
    let level = unsafe { safe_read_usize(pgd + PGD_LEVEL_68_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (name_units, name_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let is_real = level >= MIN_REAL_LEVEL && !utf16_name_empty_like(&name_units, name_len);
    (is_real, level, name_len)
}
