//! `CS::PlayerGameData` / `CS::GameDataMan` typed offsets moved from
//! er-effects-rs constants/player_correctness.rs in the portrait crate split.
//! Bound to the upstream `eldenring` typed layout via `offset_of!` exactly as before.

use eldenring::cs::{GameDataMan, PlayerGameData};

/// `[base+this]` -> CS::GameDataMan* (the singleton at 0x144588268). The all-player save data
/// GameDataMan singleton slot: `GameDataMan* = *(base + 0x3d5df38)`; PlayerGameData hangs off it
/// at +0x08. CORRECTED 2026-06-17: the prior value 0x4588268 was the WRONG global (read garbage:
/// level=805829232, name="翿"). The real GameDataMan is 0x3d5df38 -- confirmed by fromsoftware-rs
/// (`rva::game_data_man = 0x3d5df38`, `GameDataMan::main_player_game_data` at struct +0x08) and the
/// on-disk binary (dozens of `mov reg,[rip->0x143d5df38]; mov reg,[rax+0x8]; test; je` accessor
/// sites). Validated against the live char "a" (level 9, runes 0, stats [15,10,11,14,13,9,9,7]).
/// GameDataMan -> PlayerGameData (the active/main player's save data) sub-object pointer.
/// Offsets are bound to the upstream `eldenring` typed layout via `offset_of!` so they
/// track `fromsoftware-rs` automatically and fail the build if the struct layout drifts
/// (compile-time accuracy guarantee, replacing the hand-decoded hex constants).
pub const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize =
    core::mem::offset_of!(GameDataMan, main_player_game_data);

pub const PGD_CURRENT_MAX_HP_14_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_max_hp);

pub const PGD_CURRENT_MAX_FP_20_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_max_fp);

pub const PGD_CURRENT_MAX_STAMINA_30_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_max_stamina);

pub const PGD_LEVEL_68_OFFSET: usize = core::mem::offset_of!(PlayerGameData, level);

pub const PGD_GENDER_BE_OFFSET: usize = core::mem::offset_of!(PlayerGameData, gender);

// Character-name layout belongs to the lower er-game-base tier because the product and portrait
// crate both consume it. Re-export the historical names so existing portrait callers stay stable.
pub use er_game_base::pgd::{PGD_NAME_9C_OFFSET, PGD_NAME_LEN_U16};

pub const PGD_STAT_BASE_3C_OFFSET: usize = core::mem::offset_of!(PlayerGameData, vigor);

/// `matching_weapon_level` -- the character's HIGHEST weapon upgrade level, maintained by the game
/// for multiplayer matchmaking. Raw `+0..=+25`, NOT a matchmaking bucket: `CS::ChrIns::
/// CheckWeaponLevelMismatch` (1.16.2 `0x14068fd30`) guards it with `< 0x1a` and clamps to `0x19`,
/// then feeds it to the `GetMatchingWeaponLevelUpper*` param lookups that do the bucketing.
///
/// Taking this instead of walking equipment or inventory means no item-record stride, no reliance
/// on the `paramId % 100` reinforcement convention, and no equipped-only blind spot.
pub const PGD_MATCHING_WEAPON_LEVEL_E2_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, matching_weapon_level);

/// The offset is bound through `offset_of!`, so this only guards against an upstream layout change
/// silently moving it: the Ghidra 1.16.2 dump has `matchmakingWeaponLevel` at `PlayerGameData+0xe2`.
const _: () = assert!(PGD_MATCHING_WEAPON_LEVEL_E2_OFFSET == 0xe2);

/// Highest value the field can legitimately hold: standard armaments reinforce `+0..=+25` (somber
/// `+0..=+10`), and `CS::ChrIns::CheckWeaponLevelMismatch` itself guards the byte with `< 0x1a`
/// before using it. A byte above this means we are not looking at a live `PlayerGameData`, so the
/// value is reported as unknown rather than rendered -- a confident wrong number on the loading
/// screen is worse than a placeholder. `er_save_loader::stats` applies the identical bound to the
/// serialized copy of the same field, so both sources reject the same values.
pub const PGD_MATCHING_WEAPON_LEVEL_MAX: u8 = 25;
