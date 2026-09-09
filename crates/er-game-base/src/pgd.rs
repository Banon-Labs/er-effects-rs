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

// ---------------------------------------------------------------------------------------------
// CS::PlayerGameData layout pins (2026-08-30)
//
// Why these exist. 44 sites across this workspace compute a PlayerGameData offset with
// `offset_of!`, and every one of them gets its answer from `../fromsoftware-rs`'s `#[repr(C)]`
// mirror. That mirror is provably a 1.16.2 MODEL: its filler fields are named `unka54`, `unka78`,
// `unka91`, `unkab4` -- which are exactly the 1.16.2 offsets measured as moved on 1.17
// (0xa54->0xa58, 0xa78->0xa80, 0xa91->0xa99, 0xab4->0xabc) -- and it ends at 0xae8, eight bytes
// short of the 1.17 object.
//
// Nothing is wrong today: every field this workspace actually references sits at or below
// face_data 0x760, and the first measured 1.17 move is at 0x960. The hazard is the shape of the
// mechanism. A
// compiler-computed, `offset_of!`-bound constant looks maximally trustworthy -- it is the thing
// this repo reaches for instead of a hand-decoded hex number -- and it is one added field
// reference away from silently reading a neighbouring field. A wrong struct offset is the only
// 1.17 failure class with no refusal, no fault and no log line: it returns a plausible number of
// the right width, forever.
//
// So each referenced field's offset is frozen to a literal that was verified against the two
// de-ARXAN'D images, not copied out of the binding. The moment the sibling binding drifts -- an
// inserted field, a resized enum, a corrected `unk` filler -- these stop compiling and a human
// looks, instead of the DLL reading `base_max_hp` and calling it `current_max_hp`.
//
// How each literal was verified. Every offset below is read as a PlayerGameData field by both
// images, established by three routes that each prove the base register holds a PGD without any
// cross-image function pairing (the sets are built per image and then intersected):
//
//   A. the singleton chain -- `mov r64,[rip+GameDataMan]; mov r64,[r64+0x8]` and then
//      `[reg+0xNN]`. `GameDataMan+0x8` is `main_player_game_data`, the route 20+ live sites in
//      this workspace already take. Reproduce with:
//        scripts/check-singleton-field-offsets.py --pgd-offsets
//   B. functions called with that chain's PGD pointer in an argument register, one level deep.
//   C. `CS::PlayerGameData`'s constructor (1.16.2 0x14025d580 -> 1.17 0x14025d550, paired by
//      scripts/map-rvas-1162-to-1170.py) and the methods of the vtable that constructor stores at
//      `[this+0]` (1.16.2 0x1429e15f8 / 0x1429e5fa8, 1.17 0x1429e45f8 / 0x1429e8fa8).
//
// Together those witness 109 PGD field offsets in both images, and all 25 values pinned below are
// among them. That is the whole claim each pin rests on: 1.17 code demonstrably reads this offset
// as a PlayerGameData field, so the number is not merely "what the binding says".
//
// The two images' witness sets also differ at 20 offsets, and those are not all moves. A witness
// set is "offsets somebody's code happens to read inside the scanned window", not the object's
// field list, so a one-sided witness proves nothing by itself. Below 0xa58 the divergences at
// 0x18, 0x28, 0x38, 0x60 and 0x66 are one-sided coverage gaps.
//
// Corrected 2026-08-31 by aligning the CONSTRUCTOR'S two bodies rather than intersecting two
// witness sets (scripts/check-object-field-offsets-1170.py, whose 18 frozen rows re-measure this
// on every run; scripts/pair-object-field-drift.py is the matcher). The earlier reading of "+4
// from 0x9c8" and "0x964 read by 1.17 alone" was a coverage artifact of the census: 0x9c8 was
// merely the lowest moved offset that scan happened to witness, and 0x964 is not a new 1.17
// field at all. The real shape is one 4-byte insertion at 0x960:
//
//   1.16.2 ctor: mov [rbx+0x958],..  lea rcx,[rbx+0x960]  call <stat sub-object ctor>
//   1.17   ctor: mov [rbx+0x958],..  mov byte [rbx+0x960],0  lea rcx,[rbx+0x964]  call <same>
//
// 254 of 254 instructions align; the only structural difference is that inserted store. The
// 0x118-byte stat sub-object is otherwise byte-identical (its own ctor aligns with zero moved
// offsets), so it now ends at 0xa7c instead of 0xa78, and the 8-byte-aligned pointer that
// follows lands at 0xa80. Hence two bands, not one:
//
//   [0x000, 0x960)  held   -- everything this workspace references, up to face_data 0x760
//   [0x960, 0xa78)  +4     -- e.g. resistance_gauges 0x9c8 -> 0x9cc (leaf accessor, independent)
//   [0xa78, 0xae8)  +8     -- e.g. the scadutree override 0xab4 -> 0xabc
//
// A mechanical "+8 above the insertion" fix would therefore put resistance_gauges at 0x9d0 --
// four bytes into the array rather than at its start, reading a neighbouring element with no
// fault and no log line. That is the whole reason the bands are written out here.
//
// Eight referenced fields are deliberately not pinned, because the 1.17 image never witnesses
// their offset and a pin asserting a value nobody verified is worse than no pin. They are, with
// the offset the binding computes and what the images actually say about it:
//
//   base_max_hp                    0x18   read by 1.16.2 twice, by 1.17 never in this scan;
//                                         bracketed by both-witnessed 0x14 and 0x1c
//   base_max_fp                    0x24   no witness in either image; bracketed by 0x20 and 0x2c
//   base_max_stamina               0x34   no witness in either; bracketed by 0x30 and 0x3c
//   intelligence                   0x50   no witness in either; bracketed by 0x4c and 0x68
//   faith                          0x54   no witness in either; bracketed by 0x4c and 0x68
//   arcane                         0x58   no witness in either; bracketed by 0x4c and 0x68
//   base_hero_point                0x5c   no witness in either; bracketed by 0x4c and 0x68
//   matchmaking_spirit_ashes_level 0xc7   no witness in either; bracketed by 0xc6 and 0xc8
//
// A bracket is not a proof. A compensating insertion-plus-removal inside one bracket moves the
// fields between its ends while leaving both ends exactly where they were -- which is precisely
// what the audit measured happening to `CS::PlayerIns`: an 8-byte insertion in (0x398,0x3a8] and
// an 8-byte removal in (0x560,0x580] (narrowed 2026-08-31 from the earlier (0x38c,0x400] /
// (0x538,0x580] by aligning the constructor, the destructor and 183 vtable slots), net object
// size unchanged, so a "+8 above the insertion" rule would have corrupted
// PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET = 0x6b8, which is witnessed held by both the
// constructor and `~PlayerIns`. Seven of these eight sit in brackets one or two slots wide
// where that is implausible, but implausible is not measured, so they stay unpinned and named
// here rather than argued into place.
// ---------------------------------------------------------------------------------------------

// Vitals. 0x10/0x1c/0x2c are the live values, 0x14/0x20/0x30 the current maxima.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, current_hp) == 0x10);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, current_max_hp) == 0x14);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, current_fp) == 0x1c);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, current_max_fp) == 0x20);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, current_stamina) == 0x2c);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, current_max_stamina) == 0x30);

// The eight-stat block. Its base is `vigor`; the four that follow it are witnessed individually.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, vigor) == 0x3c);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, mind) == 0x40);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, endurance) == 0x44);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, strength) == 0x48);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, dexterity) == 0x4c);

// Level and runes.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, level) == 0x68);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, rune_count) == 0x6c);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, rune_memory) == 0x70);

// Identity. `chr_type` + its size is where PGD_NAME_9C_OFFSET above comes from, and `gender` is
// where PGD_NAME_LEN_U16 stops -- so these two pin the character-name window from both ends.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, chr_type) == 0x98);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, gender) == 0xbe);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, archetype) == 0xbf);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, voice_type) == 0xc2);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, starting_gift) == 0xc3);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, unlocked_talisman_slots) == 0xc6);

// Matchmaking weapon level. er_loading_portrait_core::pgd_layout already pins this same value
// from the other side; both must hold, so the two derivations cannot drift apart silently.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, matching_weapon_level) == 0xe2);

// Flasks.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, max_hp_flask) == 0x101);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, max_fp_flask) == 0x102);

// The two large sub-objects. `equipment` carries the whole ChrAsm/inventory tree and `face_data`
// the 288-byte appearance buffer, so a shift in either moves every offset the build importer and
// the loading-cover portrait compute on top of it. `face_data` is 0x760, not the 0x5e0 an offset
// census invites you to guess: 0x5e0 is witnessed as a PGD field read too, but it is inside
// `equipment`. The compiler was asked, and it answered 1888.
const _: () = assert!(core::mem::offset_of!(PlayerGameData, equipment) == 0x2b0);
const _: () = assert!(core::mem::offset_of!(PlayerGameData, face_data) == 0x760);

/// Lowest `CS::PlayerGameData` offset that ELDEN RING 1.17 moved, measured from the two images
/// (see the band table above). Everything below it is the same field in both builds; a field at
/// or above it would need a version-aware offset, and this workspace has none.
pub const PGD_FIRST_MOVED_OFFSET_1170: usize = 0x960;

// The pins above protect individual fields. This protects the shape, and it has now fired once,
// which is what it was written for.
//
// The sibling binding was a 1.16.2 model. It was refreshed to the 1.17 layout by
// `fromsoftware-rs` a5f0e1f ("More of quickmatch stuff", merged as #308): `unkaa4: i32` became
// `quick_match_spawn_slot_fraction: f32`, `quick_match_desired_team` became an enum, two bools
// were named out of `unkaa9`/`unkb2`, and the tail filler `unkab4` gave up two bytes to
// `host_scadutree_blessing` and `host_scaling_applied`. Every one of those edits is at or above
// 0xa90, so no `offset_of!` answer this workspace consumes moved -- and that is not an argument,
// it is the compiler's: all 25 pins above were the only other asserts in this file and they
// compiled clean against the refreshed binding while this one failed alone.
//
// The object's size is the one number the refresh does change, 0xae8 -> 0xaf0, exactly as this
// comment predicted before it happened. The number is still measured rather than trusted: the way
// to read it is to replace this assertion with `const _: [(); 0] = [(); size_of::<PlayerGameData>()];`
// and take `found one with a size of 2800` out of the type error. Do that on the next refresh
// instead of editing the constant until the build passes -- a guess that happens to be right and a
// measurement look identical afterwards. So the literal moves to 0xaf0 and the tripwire is armed
// again for the next binding edit. The highest offset actually referenced stays asserted under
// `PGD_FIRST_MOVED_OFFSET_1170` so such an edit cannot walk a field across the boundary unnoticed.
const _: () = assert!(core::mem::size_of::<PlayerGameData>() == 0xaf0);
const _: () =
    assert!(core::mem::offset_of!(PlayerGameData, face_data) < PGD_FIRST_MOVED_OFFSET_1170);

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
