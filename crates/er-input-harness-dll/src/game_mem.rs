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

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

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

/// Cumulative play time (`GameDataMan+0xa0`, u32 ms), or -1 if unavailable. Rises ONLY while the world
/// SIMULATES (frozen in menus / loading), which is why it is the reliable in-world gate -- unlike
/// `playerGameData+0x08`, which is non-null AT THE TITLE and false-positives (observed 2026-07-22: the
/// harness marched through every reload step because player_present() returned true at the title menu).
const GAME_DATA_MAN_PLAY_TIME_A0_OFFSET: usize = 0xa0;

pub fn play_time_ms() -> i64 {
    let Some(base) = game_base() else {
        return -1;
    };
    let Some(gdm) = deref_singleton(base, GAME_DATA_MAN_GLOBAL_RVA) else {
        return -1;
    };
    unsafe { read_usize(gdm + GAME_DATA_MAN_PLAY_TIME_A0_OFFSET) }
        .map_or(-1, |v| i64::from((v & 0xffff_ffff) as u32))
}

static LAST_PLAY_TIME: AtomicI64 = AtomicI64::new(-1);
static WORLD_SIM_STREAK: AtomicU32 = AtomicU32::new(0);

/// True once play_time has RISEN for `RISING_STREAK` consecutive frames -> a loaded, UNPAUSED character
/// genuinely simulating. Call once per frame from the in-world wait phase. This is the real "reached
/// world" gate (replaces the false-positive `player_present`). Resets the streak on any non-rise.
pub fn world_simulating() -> bool {
    const RISING_STREAK: u32 = 4;
    let pt = play_time_ms();
    let last = LAST_PLAY_TIME.swap(pt, Ordering::SeqCst);
    let rose = pt >= 0 && last >= 0 && pt > last;
    let streak = if rose {
        WORLD_SIM_STREAK.fetch_add(1, Ordering::SeqCst) + 1
    } else {
        WORLD_SIM_STREAK.store(0, Ordering::SeqCst);
        0
    };
    streak >= RISING_STREAK
}

// LOAD-STARTED semaphores (ground truth from the product constant tree): the load FSM GameMan+0xb80
// (0 IDLE -> non-0 loading/resident) and the NowLoading latch. A driven Continue "took effect" once one
// of these trips within the frame budget -- else the harness is derailed (bd HARNESS-drive-semaphore-
// gated-teardown-on-miss). GameMan singleton RVA 0x3d69918 (profile_rows_system_quit_menu.rs), b80 =
// GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET; NowLoading singleton 0x3d60ec8, flag +0xED (CSNowLoadingHelperImp.load_done).
const GAME_MAN_SINGLETON_RVA: usize = 0x3d69918;
const GAME_MAN_LOAD_FSM_B80_OFFSET: usize = 0xb80;
const NOW_LOADING_SINGLETON_RVA: usize = 0x3d60ec8;
const NOW_LOADING_FLAG_ED_OFFSET: usize = 0xed;

/// Load FSM byte (GameMan+0xb80): 0 = idle, non-zero = a load is opening/reading/resident.
pub fn load_fsm() -> i32 {
    let Some(base) = game_base() else {
        return -1;
    };
    let Some(gm) = deref_singleton(base, GAME_MAN_SINGLETON_RVA) else {
        return -1;
    };
    unsafe { read_usize(gm + GAME_MAN_LOAD_FSM_B80_OFFSET) }.map_or(-1, |v| (v & 0xff) as i32)
}

/// NowLoading latch (deref base+0x3d60ec8 -> +0xED): set while/after a load screen; a load-activity
/// signal (lingers). Non-zero = loading activity seen.
pub fn now_loading() -> bool {
    let Some(base) = game_base() else {
        return false;
    };
    let Some(helper) = deref_singleton(base, NOW_LOADING_SINGLETON_RVA) else {
        return false;
    };
    unsafe { read_usize(helper + NOW_LOADING_FLAG_ED_OFFSET) }.is_some_and(|v| (v & 0xff) != 0)
}

/// Read the optional drive-mode flag file (CWD-relative, same dir as the log): one of `boot`,
/// `reload`, `full` (default `full`). Lets a run switch the drive PATTERN without a rebuild.
pub fn read_drive_mode_flag() -> String {
    std::fs::read_to_string("er-harness-drive-mode.txt")
        .map(|s| s.trim().to_ascii_lowercase())
        .unwrap_or_default()
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
