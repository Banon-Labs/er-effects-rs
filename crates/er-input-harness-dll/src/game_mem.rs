//! Direct game-memory reads that RE-DERIVE the coarse runtime state the self-drive gates on.
//!
//! CROSS-DLL STATE (constraint #1): separate DLLs do NOT share Rust statics, so this harness cannot
//! read the product DLL's `SYSTEM_QUIT_INGAME_TOP_WINDOW` / `SYSTEM_QUIT_QUICKLOAD_PHASE` /
//! menu-window latches (those live in `er_effects_rs.dll`'s image). Those product statics are
//! themselves derived from GAME memory, so the harness re-derives what it needs the same way
//! `er-reload-trace-dll` reads the game: `GetModuleHandleA(NULL)` for the image base, then
//! fault-safe `ReadProcessMemory` walks of the known singletons.
//!
//! Coarse vs precise (honest limit): the product's window latches are populated by NATIVE menu-window
//! ctor hooks (`menu_window_job_ctor_*`, the `SetState` trace). Standalone, a *precise* window
//! identity (IngameTop vs OptionSetting vs ProfileSelect) would require union-registering those same
//! ctor observers through the product's `er_effects_union_register` export and matching vtable RVAs.
//! This module intentionally re-derives only what a passive read can prove: image base, player
//! presence (in-world proxy), and top-menu-window presence -- enough to sequence the proven
//! keyboard-open + submenu edges, not enough to positively identify each pane.

use crate::win32::{GetModuleHandleA, read_usize};

// RVAs/offsets ported verbatim from the product's constant tree (image base 0x140000000):
//   GAME_DATA_MAN_GLOBAL_RVA / +0x08 PlayerGameData -- er-reload-trace-dll src/lib.rs
//   CS_MENU_MAN_GLOBAL_RVA / CS_MENU_MAN_MENU_DATA_OFFSET -- crates/er-effects-rs/src/constants/*
// They are plain integer literals (addresses the DLL reads), not shared statics.
const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize = 0x08;
const CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0;
const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;

/// Lowest plausible heap/image pointer -- filters null and small sentinel values out of walks.
const HEAP_LO: usize = 0x10000;

/// The game image base (`GetModuleHandleA(NULL)`), or `None` before the image is mapped.
pub fn game_base() -> Option<usize> {
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

fn deref_singleton(base: usize, rva: usize) -> Option<usize> {
    let p = unsafe { read_usize(base + rva) }?;
    (p >= HEAP_LO).then_some(p)
}

/// IN-WORLD PROXY: `GameDataMan.playerGameData` (+0x08) is non-null once a character's game data is
/// resident. This replaces the product's `IN_WORLD_REACHED` static (which the product sets from its
/// own SetState trace) with a passive read the harness can make independently.
pub fn player_present() -> bool {
    let Some(base) = game_base() else {
        return false;
    };
    let Some(gdm) = deref_singleton(base, GAME_DATA_MAN_GLOBAL_RVA) else {
        return false;
    };
    unsafe { read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }
        .is_some_and(|pgd| pgd >= HEAP_LO)
}

/// TOP-MENU-WINDOW PROXY: `CSMenuMan.menuData` (+0x8) non-null indicates a menu-data owner exists.
/// Returns the pointer (for change-detection) or 0. This is the coarse re-derivation of the product's
/// `SYSTEM_QUIT_INGAME_TOP_WINDOW` latch -- it proves *a* menu is up, not *which* one (see module doc).
pub fn menu_data_ptr() -> usize {
    let Some(base) = game_base() else {
        return 0;
    };
    let Some(menu_man) = deref_singleton(base, CS_MENU_MAN_GLOBAL_RVA) else {
        return 0;
    };
    unsafe { read_usize(menu_man + CS_MENU_MAN_MENU_DATA_OFFSET) }
        .filter(|p| *p >= HEAP_LO)
        .unwrap_or(0)
}

/// Compact one-line state snapshot for the log (mirrors the trace DLL's `snapshot()` habit).
pub fn snapshot() -> String {
    let base = game_base().unwrap_or(0);
    let gdm = game_base()
        .and_then(|b| deref_singleton(b, GAME_DATA_MAN_GLOBAL_RVA))
        .unwrap_or(0);
    format!(
        "base=0x{base:x} gdm=0x{gdm:x} player_present={} menu_data=0x{:x}",
        player_present() as u8,
        menu_data_ptr()
    )
}
