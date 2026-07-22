//! Tier A: stable singleton RVA / offset table (game 1.16.x, image base
//! 0x140000000). These are version-anchored facts shared by all three DLLs;
//! they were previously re-declared under ~4 different aliases in the product
//! `constants/*` tree and hand-copied verbatim into the two mini-DLLs. This is
//! the single source of truth.
//!
//! Feature-specific / experiment-local offsets do NOT belong here — only the
//! cross-cutting singleton globals + their generic field offsets.

/// `GameDataMan` singleton global (aliased as GAME_DATA_MAN_GLOBAL_RVA /
/// CONTINUE_MANAGER_GLOBAL_RVA in the product tree).
pub const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
/// `CSMenuMan` singleton global (aliased GLOBAL_CSMENUMAN_RVA /
/// CS_MENU_MAN_GLOBAL_RVA / SELECTBOT_INPUT_MANAGER_GLOBAL_RVA /
/// TITLE_INPUT_MANAGER_RVA).
pub const CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0;
/// `GameMan` singleton global (save-slot owner).
pub const GAME_MAN_SINGLETON_RVA: usize = 0x3d69918;
/// Save-data subsystem gate global (submit path guard).
pub const SAVE_DATA_SUBSYSTEM_GATE_RVA: usize = 0x3d68078;

/// `GameDataMan` -> `PlayerGameData` pointer field offset.
pub const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize = 0x08;
/// `CSMenuMan` -> `menuData` pointer field offset.
pub const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;
