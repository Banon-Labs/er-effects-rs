//! Typed `CS::PlayerGameData` character-name layout and identity readers.
//!
//! This is Tier B: it is available only with the `game-types` feature on Windows, where the
//! offsets can stay bound to the upstream `eldenring` layout instead of becoming copied numbers.

use eldenring::cs::PlayerGameData;

use crate::mem::safe_read_usize;

/// `PlayerGameData::character_name` is private upstream, so derive its start from the preceding
/// public `chr_type` field.
pub const PGD_NAME_9C_OFFSET: usize = core::mem::offset_of!(PlayerGameData, chr_type)
    + core::mem::size_of::<eldenring::cs::ChrType>();

/// UTF-16 code-unit capacity between the private character name and the following public gender.
pub const PGD_NAME_LEN_U16: usize = (core::mem::offset_of!(PlayerGameData, gender)
    - PGD_NAME_9C_OFFSET)
    / core::mem::size_of::<u16>();

const _: () = assert!(PGD_NAME_9C_OFFSET == 0x9c);
const _: () = assert!(PGD_NAME_LEN_U16 == 17);

/// Treat the game's empty-name sentinels as empty character identities.
pub fn utf16_name_empty_like(units: &[u16], len: usize) -> bool {
    const NAME_LEN_NONE: usize = 0;
    const NAME_LEN_SINGLE: usize = 1;
    const NAME_UNDERSCORE: u16 = '_' as u16;
    const NAME_SPACE: u16 = ' ' as u16;
    if len == NAME_LEN_NONE {
        return true;
    }
    if len == NAME_LEN_SINGLE && units.first().copied() == Some(NAME_UNDERSCORE) {
        return true;
    }
    units.iter().take(len).all(|unit| *unit == NAME_SPACE)
}

/// Compare two UTF-16 name buffers over the caller-validated length.
pub fn utf16_names_equal(left: &[u16], right: &[u16], len: usize) -> bool {
    left.get(..len) == right.get(..len)
}

/// Fault-tolerantly read the fixed-size UTF-16 character-name buffer at `addr`.
///
/// # Safety
///
/// `addr` may be any process address. Reads are performed through `ReadProcessMemory`; unreadable
/// locations produce zero units instead of dereferencing the address directly.
pub unsafe fn read_utf16_name_units(addr: usize) -> ([u16; PGD_NAME_LEN_U16], usize) {
    const ZERO_U16: u16 = 0;
    const U16_STRIDE: usize = 2;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    let mut units = [ZERO_U16; PGD_NAME_LEN_U16];
    let mut len = IDX_START;
    while len < PGD_NAME_LEN_U16 {
        let unit = unsafe { safe_read_usize(addr + len * U16_STRIDE) }
            .map(|value| value as u16)
            .unwrap_or(ZERO_U16);
        units[len] = unit;
        if unit == ZERO_U16 {
            break;
        }
        len += IDX_STEP;
    }
    (units, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_like_matches_game_name_sentinels() {
        assert!(utf16_name_empty_like(&[], 0));
        assert!(utf16_name_empty_like(&[b'_' as u16], 1));
        assert!(utf16_name_empty_like(&[b' ' as u16, b' ' as u16], 2));
        assert!(!utf16_name_empty_like(&[b'R' as u16], 1));
    }

    #[test]
    fn equality_obeys_the_validated_length() {
        let left = ['R' as u16, 'a' as u16, 0];
        let right = ['R' as u16, 'a' as u16, 'n' as u16];
        assert!(utf16_names_equal(&left, &right, 2));
        assert!(!utf16_names_equal(&left, &right, 3));
    }
}
