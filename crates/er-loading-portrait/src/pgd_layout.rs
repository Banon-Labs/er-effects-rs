//! `CS::PlayerGameData` / `CS::GameDataMan` typed offsets moved from
//! er-effects-rs constants/player_correctness.rs in the portrait crate split.
//! Bound to the upstream `eldenring` typed layout via `offset_of!` exactly as before.

use crate::prelude::*;

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

/// `character_name` is private upstream, so compute its start from the preceding public `chr_type`
/// field and its length from the following public `gender` field.
pub const PGD_NAME_9C_OFFSET: usize = core::mem::offset_of!(PlayerGameData, chr_type)
    + core::mem::size_of::<eldenring::cs::ChrType>();
pub const PGD_NAME_LEN_U16: usize =
    (PGD_GENDER_BE_OFFSET - PGD_NAME_9C_OFFSET) / core::mem::size_of::<u16>();

pub const PGD_STAT_BASE_3C_OFFSET: usize = core::mem::offset_of!(PlayerGameData, vigor);
