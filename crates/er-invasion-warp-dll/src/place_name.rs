//! Turn a block's `PlaceName` id into the words the game itself uses for that area.
//!
//! # Why the banner said `m60_50_39_00`
//!
//! The filter judges by block, and a block id is what it had to hand. It is also unreadable: a
//! player who is told they were rejected from `m60_50_39_00` has to go and look that up, which is
//! the opposite of what a two-second auto-closing banner is for. The map pins have carried real
//! names all along -- they are named from the nearest shipped warp row -- so the name exists, it
//! simply was not being asked for.
//!
//! # The lookup, and why it needs no `MenuString`
//!
//! `CS::MsgRepository::GetAndFormat` (`0x1407633d0`) resolves a `PlaceName` in two steps: it reads
//! the `MsgRepositoryImp` singleton out of [`MSG_REPOSITORY_GLOBAL_RVA`], calls a getter with
//! `(repo, id)` to obtain a raw `wchar_t*`, and only THEN wraps that pointer in a `MenuString`.
//! The wrapper `0x1407611c0` is what binds the getter to `PlaceName`:
//!
//! ```text
//!   getter = FUN_140d10b60;
//!   CS::MsgRepository::GetAndFormat(out, &getter, id, L"PlaceName", L"PN");
//! ```
//!
//! Only the first step is wanted here, so [`PLACE_NAME_LOOKUP_RVA`] is called directly. That
//! matters for more than tidiness: constructing a `MenuString` means handing the engine a zeroed
//! `DLString` to assign into, and a zeroed `DLString` has a null allocator -- which is exactly the
//! failure that made the rejection banner render empty. Taking the pointer avoids re-entering that
//! trap rather than hoping it behaves differently here.
//!
//! The getter tries the base `PlaceName` FMG, then `PlaceName_dlc01`, then `PlaceName_dlc02`, and
//! returns the literal `?PlaceName?` when all three miss. That sentinel is treated as "no name"
//! rather than passed through, because putting `?PlaceName?` on a banner is worse than the block id
//! it replaced.

/// `GLOBAL_MsgRepository` -- the `MsgRepositoryImp*` singleton slot, read from
/// `MOV RCX, qword ptr [0x143d7d4f8]` inside `GetAndFormat`.
pub const MSG_REPOSITORY_GLOBAL_RVA: usize = 0x3d7_d4f8;

/// `FUN_140d10b60(MsgRepositoryImp*, id) -> wchar_t*` -- the `PlaceName` getter.
pub const PLACE_NAME_LOOKUP_RVA: usize = 0xd1_0b60;

/// Opening bytes of the getter, byte-checked against `eldenring-deobf.bin` at shift 0, so a game
/// update that moves it fails closed instead of calling into the middle of something else.
pub const PLACE_NAME_LOOKUP_PROLOGUE: &[u8] =
    &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20];

/// What the getter returns when every `PlaceName` FMG missed. Not a name.
pub const PLACE_NAME_MISSING_SENTINEL: &str = "?PlaceName?";

/// Longest name accepted. Elden Ring's longest place names are well under this; the bound exists so
/// a wild pointer cannot walk the heap looking for a NUL.
pub const MAX_PLACE_NAME_CHARS: usize = 64;

/// The area name for a `PlaceName` text id, or `None` when it cannot be resolved.
///
/// `None` covers every "no answer" case identically on purpose -- the singleton not up yet, the
/// getter moved, a negative id, the engine's own miss sentinel -- because the caller's response to
/// all of them is the same: fall back to the block id rather than print something misleading.
///
/// # Safety
///
/// Game thread. Calls a byte-verified game function and reads the string it returns.
#[cfg(windows)]
#[must_use]
pub fn place_name_for_text_id(text_id: i32) -> Option<String> {
    // The engine's own guard: `GetAndFormat` treats anything below 1 as "no message" before it
    // touches the singleton, and -1 is this crate's own "unnamed" sentinel.
    if text_id < 1 {
        return None;
    }
    let base = er_game_base::mem::game_module_base().ok()?;
    let lookup = base + PLACE_NAME_LOOKUP_RVA;
    for (index, expected) in PLACE_NAME_LOOKUP_PROLOGUE.iter().enumerate() {
        if unsafe { er_game_base::mem::safe_read_u8(lookup + index) }? != *expected {
            return None;
        }
    }
    let repository =
        unsafe { er_game_base::mem::safe_read_usize(base + MSG_REPOSITORY_GLOBAL_RVA) }?;
    if repository == 0 {
        // Before the singleton exists the engine DLPanics on this path; we simply decline.
        return None;
    }
    type LookupFn = unsafe extern "system" fn(usize, u32) -> usize;
    let lookup: LookupFn = unsafe { core::mem::transmute(lookup) };
    let text = unsafe { lookup(repository, text_id as u32) };
    if text == 0 {
        return None;
    }
    let mut units: Vec<u16> = Vec::new();
    for index in 0..MAX_PLACE_NAME_CHARS {
        let unit = unsafe { er_game_base::mem::safe_read_u16(text + index * 2) }?;
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    if units.is_empty() {
        return None;
    }
    let name = String::from_utf16(&units).ok()?;
    if name == PLACE_NAME_MISSING_SENTINEL {
        return None;
    }
    Some(name)
}

#[cfg(not(windows))]
#[must_use]
pub fn place_name_for_text_id(_text_id: i32) -> Option<String> {
    None
}

/// The area name for a BLOCK, using the names the world map recorded as it built the pins.
///
/// Returns `None` until the map has been opened at least once this session -- the same condition
/// that makes `mode = "area"` fail closed, and for the same reason: nothing has a name before then.
///
/// A block can carry several names (that is the "five names, five places to look" rule). The lowest
/// id is taken so repeated rejections at one block always print the SAME name; picking arbitrarily
/// from a set would make the banner appear to flicker between names for one place.
#[must_use]
pub fn place_name_for_block(block: u32) -> Option<String> {
    let mut ids = crate::map_hooks::registry_place_names_for_block(block);
    ids.sort_unstable();
    ids.into_iter().find_map(place_name_for_text_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RVAs are byte-checked facts about 1.16.2, not tunables.
    #[test]
    fn the_rvas_are_the_byte_checked_ones() {
        assert_eq!(MSG_REPOSITORY_GLOBAL_RVA, 0x3d7_d4f8);
        assert_eq!(PLACE_NAME_LOOKUP_RVA, 0xd1_0b60);
        // `48 89 5C 24 08` is `MOV [RSP+8],RBX` -- long enough to discriminate, and confirmed
        // against eldenring-deobf.bin at shift 0.
        assert_eq!(
            PLACE_NAME_LOOKUP_PROLOGUE,
            &[0x48, 0x89, 0x5c, 0x24, 0x08, 0x57, 0x48, 0x83, 0xec, 0x20]
        );
        assert!(PLACE_NAME_LOOKUP_PROLOGUE.len() >= 8);
    }

    /// The engine's miss sentinel must never reach a player.
    #[test]
    fn the_missing_sentinel_is_the_engines_own_string() {
        assert_eq!(PLACE_NAME_MISSING_SENTINEL, "?PlaceName?");
    }

    /// A bound, so a wild pointer cannot walk the heap looking for a terminator.
    #[test]
    fn the_name_length_is_bounded_but_fits_real_place_names() {
        const {
            assert!(
                MAX_PLACE_NAME_CHARS >= 40,
                "must fit the longest area names"
            )
        };
        const { assert!(MAX_PLACE_NAME_CHARS <= 256) };
    }

    /// Host builds cannot call the game, and must say so by declining rather than inventing a name.
    #[cfg(not(windows))]
    #[test]
    fn the_host_build_resolves_nothing() {
        assert_eq!(place_name_for_text_id(1), None);
    }
}
