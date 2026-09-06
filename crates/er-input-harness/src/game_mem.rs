//! Direct game-memory reads that RE-DERIVE the coarse runtime state the self-drive gates on.
//!
//! CROSS-DLL STATE (constraint #1): separate DLLs do NOT share Rust statics, so this harness cannot
//! read the product DLL's `SYSTEM_QUIT_INGAME_TOP_WINDOW` / `SYSTEM_QUIT_QUICKLOAD_PHASE` /
//! menu-window latches (those live in `er_quickload.dll`'s image). Those product statics are
//! themselves derived from GAME memory, so the harness re-derives what it needs the same way
//! `er-reload-trace` reads the game: `GetModuleHandleA(NULL)` for the image base, then
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
//   GAME_DATA_MAN_GLOBAL_RVA / +0x08 PlayerGameData -- er-reload-trace src/lib.rs
//   CS_MENU_MAN_GLOBAL_RVA / CS_MENU_MAN_MENU_DATA_OFFSET -- crates/er-quickload/src/constants/*
// They are plain integer literals (addresses the DLL reads), not shared statics.
const GAME_DATA_MAN_GLOBAL_RVA: usize = er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA;
const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize = 0x08;
const CS_MENU_MAN_GLOBAL_RVA: usize = er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;
const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;

/// Lowest plausible heap/image pointer -- filters null and small sentinel values out of walks.
const HEAP_LO: usize = 0x10000;

/// The game image base (`GetModuleHandleA(NULL)`), or `None` before the image is mapped.
pub fn game_base() -> Option<usize> {
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

/// True when the PRODUCT DLL (`er_quickload.dll`) is loaded in this process -- a REAL runtime condition
/// (not a marker file): when the product is present the harness is a COMPANION (the product owns the
/// drive), so the standalone boot/menu drive must stand down and not fight it.
pub fn product_dll_present() -> bool {
    let name = b"er_quickload.dll\0";
    (unsafe { GetModuleHandleA(name.as_ptr().cast()) } as usize) != 0
}

/// Dereference a game singleton pointer by RVA -- RESOLVED for the running build, never added raw.
///
/// This is the one chokepoint every singleton read in this DLL goes through, and its results feed
/// raw byte STORES (`inject_vk` stamps `source+0x88`, the popup request byte at `+0x121`). Every
/// `.data` global moved on 1.17, so a raw `base + rva` reads whatever now occupies the old slot;
/// `HEAP_LO` only rejects the low 64 KiB, so a plausible-looking garbage qword passes it and the
/// store lands somewhere arbitrary. Resolving here makes an unmapped global answer `None` instead,
/// which is a path every caller already has.
fn deref_singleton(base: usize, rva: usize, what: &'static str) -> Option<usize> {
    let address = er_game_base::mem::game_data_addr(base, rva, what);
    if address == 0 {
        return None;
    }
    let p = unsafe { read_usize(address) }?;
    (p >= HEAP_LO).then_some(p)
}

/// IN-WORLD PROXY: `GameDataMan.playerGameData` (+0x08) is non-null once a character's game data is
/// resident. This replaces the product's `IN_WORLD_REACHED` static (which the product sets from its
/// own SetState trace) with a passive read the harness can make independently.
pub fn player_present() -> bool {
    let Some(base) = game_base() else {
        return false;
    };
    let Some(gdm) = deref_singleton(base, GAME_DATA_MAN_GLOBAL_RVA, "GAME_DATA_MAN_GLOBAL_RVA")
    else {
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
    let Some(menu_man) = deref_singleton(base, CS_MENU_MAN_GLOBAL_RVA, "CS_MENU_MAN_GLOBAL_RVA")
    else {
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
    let Some(gdm) = deref_singleton(base, GAME_DATA_MAN_GLOBAL_RVA, "GAME_DATA_MAN_GLOBAL_RVA")
    else {
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

// SL-DEVICE-BUSY semaphores (ground truth from the product constant tree): `GameMan::saveState`
// at +0xb80 (0 IDLE -> non-0 busy) and the NowLoading latch. A driven Continue "took effect" once
// one of these trips within the frame budget -- else the harness is derailed (bd HARNESS-drive-
// semaphore-gated-teardown-on-miss). GameMan singleton RVA 0x3d69918
// (profile_rows_system_quit_menu.rs), b80 = GAME_MAN_SAVE_STATE_B80_OFFSET; NowLoading singleton
// 0x3d60ec8, flag +0xED (CSNowLoadingHelperImp.load_done).
//
// This was called `GAME_MAN_LOAD_FSM_B80_OFFSET` / `load_fsm()` until 2026-08-31. The field is
// NOT load-only: the SAVE lane stamps it too (see the value table on the declaration in
// er-title-flow's `constants_moved.rs`), so `> 0` here means "the SL device is busy", which is
// what both call sites in `drive.rs` actually want.
const GAME_MAN_SINGLETON_RVA: usize = er_game_base::rva::GAME_MAN_SINGLETON_RVA;
const GAME_MAN_SAVE_STATE_B80_OFFSET: usize = 0xb80;
const NOW_LOADING_SINGLETON_RVA: usize = 0x3d60ec8;
const NOW_LOADING_FLAG_ED_OFFSET: usize = 0xed;

/// `GameMan::saveState` (+0xb80), low byte: 0 = the SL device is idle, non-zero = a save, a
/// preview read or a load owns it. Named by the game's own predicates `IsSaveState1` (1.16.2
/// `0x14067a010`) and `IsSaveState2` (`0x140679ff0`), each a two-instruction
/// `cmp dword ptr [rax+0xb80], N` off the GameMan singleton.
pub fn save_state() -> i32 {
    let Some(base) = game_base() else {
        return -1;
    };
    let Some(gm) = deref_singleton(base, GAME_MAN_SINGLETON_RVA, "GAME_MAN_SINGLETON_RVA") else {
        return -1;
    };
    unsafe { read_usize(gm + GAME_MAN_SAVE_STATE_B80_OFFSET) }.map_or(-1, |v| (v & 0xff) as i32)
}

/// NowLoading latch (deref base+0x3d60ec8 -> +0xED): set while/after a load screen; a load-activity
/// signal (lingers). Non-zero = loading activity seen.
pub fn now_loading() -> bool {
    let Some(base) = game_base() else {
        return false;
    };
    let Some(helper) =
        deref_singleton(base, NOW_LOADING_SINGLETON_RVA, "NOW_LOADING_SINGLETON_RVA")
    else {
        return false;
    };
    unsafe { read_usize(helper + NOW_LOADING_FLAG_ED_OFFSET) }.is_some_and(|v| (v & 0xff) != 0)
}

// FLIP-TIMING semaphore. CSFlipperImp singleton base+0x4589ad8; fixed_spf f32@+0x1c is the game's
// frame-time TARGET (0.0167=60, 0.05=20, 0.0333=30, 0.0083=120), mode_current i32@+0xc.
// CORRECTION (bd DECISIVE-reload-20fps-is-render-bound-not-throttle-syncinterval1-refresh4-2026-07-22,
// build a38dccd): the reload 20fps is NOT a fixed_spf=0.05 cap. Measured across the full reload movable
// windows fixed_spf stays 0.0167 (60fps TARGET) while task_delta(+0x268, actual)=0.05; the game passes
// SyncInterval=1 to Present yet GetFrameStatistics shows 4 refreshes/present -> the frame is RENDER-BOUND
// (not ready within 1 vblank), not a loading-mode cap. Keep fixed_spf as a phase signal (target vs actual
// divergence) but do NOT treat 0.05 as the cap mechanism; the earlier fixedspf-0.05 memory is refuted.
const CS_FLIPPER_SINGLETON_RVA: usize = 0x4589ad8;
const CS_FLIPPER_FIXED_SPF_1C_OFFSET: usize = 0x1c;
const CS_FLIPPER_MODE_CURRENT_C_OFFSET: usize = 0xc;

/// CSFlipperImp fixed_spf (+0x1c, f32): the game's frame-time TARGET. 0.05 = the 20fps loading cap,
/// 0.0167 = 60fps. -1.0 if unavailable. The decisive load-completion / fps-cap semaphore.
pub fn flip_fixed_spf() -> f32 {
    let Some(base) = game_base() else {
        return -1.0;
    };
    let Some(flipper) = deref_singleton(base, CS_FLIPPER_SINGLETON_RVA, "CS_FLIPPER_SINGLETON_RVA")
    else {
        return -1.0;
    };
    unsafe { read_usize(flipper + CS_FLIPPER_FIXED_SPF_1C_OFFSET) }
        .map_or(-1.0, |v| f32::from_bits((v & 0xffff_ffff) as u32))
}

/// CSFlipperImp flip mode_current (+0xc, i32): which flip mode is engaged (FLIP_20FPS_ADAPTIVE forces
/// the 0.05 cap; FLIP_60FPS_VSYNC_ON is the default). -1 if unavailable.
pub fn flip_mode_current() -> i32 {
    let Some(base) = game_base() else {
        return -1;
    };
    let Some(flipper) = deref_singleton(base, CS_FLIPPER_SINGLETON_RVA, "CS_FLIPPER_SINGLETON_RVA")
    else {
        return -1;
    };
    unsafe { read_usize(flipper + CS_FLIPPER_MODE_CURRENT_C_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as i32)
}

// IN-WORLD MENU-PANE semaphores for the quit-to-menu flow (bd QUIT-TO-MENU-semaphores-2026-07-22).
// menuData (inputmgr+0x8) is non-null for the whole SESSION -> useless as "menu open". The real open
// signal is the popupMenu's currentTopMenuJob (HasTopMenuJob 0x14080d810), and the pane identity is the
// top window's menu_id.
const CS_MENU_MAN_POPUP_MENU_80_OFFSET: usize = 0x80;
const CS_POPUP_CURRENT_TOP_JOB_B0_OFFSET: usize = 0xb0;
const TOP_JOB_WINDOW_130_OFFSET: usize = 0x130;
const TOP_WINDOW_MENU_ID_180_OFFSET: usize = 0x180;
/// OptionSetting SettingTabControl (window+0x1870) -> tab view ptr (+0x10, deref) -> selected index (+0xd4).
const OPTIONSETTING_TAB_CONTROL_1870_OFFSET: usize = 0x1870;
const OPTIONSETTING_TAB_VIEW_10_OFFSET: usize = 0x10;
const OPTIONSETTING_TAB_INDEX_D4_OFFSET: usize = 0xd4;
/// Return-title request byte within menuData (set when the quit-to-title functor fires) = quit STARTED.
const MENU_DATA_RETURN_TITLE_5D_OFFSET: usize = 0x5d;

/// In-world menu pane ids read at top_window+0x180 (u16).
///
/// RETAINED RE FACTS, AND NOTHING DECIDES ON THEM ANY MORE (2026-09-05). Both are real values off a
/// reversed 1.16.2 menu-id table, which is why they are kept rather than deleted. But the OFFSET
/// they are read through, `TOP_WINDOW_MENU_ID_180_OFFSET`, has drifted on 1.17: `top_menu_id()`
/// returns -1 or garbage there (53724, 25445, -1 measured across br-20260905-041435-e8d0 and
/// -041731-2bc4) while `pause_menu_open()` was correctly true. Garbage compares `!=` to anything, so
/// a phase gated on `top_menu_id() != OPTIONSETTING_MENU_ID` advances on its first frame and reports
/// success for a press it never issued -- which is exactly what `Phase::ActivateLoadFromFile` did
/// until it moved to the `currentTopMenuJob` pointer-change semaphore. `top_menu_id()` survives as a
/// LOG field only. Do not gate anything on either constant until the 1.17 offset is re-measured.
#[allow(dead_code)]
pub const INGAMETOP_MENU_ID: i32 = 0xffff;
#[allow(dead_code)]
pub const OPTIONSETTING_MENU_ID: i32 = 0x25;
pub const OPTIONSETTING_QUIT_TAB_INDEX: i32 = 8;

fn input_mgr() -> usize {
    game_base()
        .and_then(|b| deref_singleton(b, CS_MENU_MAN_GLOBAL_RVA, "CS_MENU_MAN_GLOBAL_RVA"))
        .unwrap_or(0)
}

/// THE GATE EVERY MENU PAD READ PASSES THROUGH (`FUN_140758050`, 1.16.2).
///
/// `CS::GridControl`'s pager (vtable slot 2, `FUN_1407392f0`) does not read a pad device directly.
/// Each direction it tests goes through `FUN_14075d970`, whose FIRST act is to call this predicate
/// with no arguments of its own; when it answers false the lambda holding the menu code is never
/// invoked and the read returns "not pressed". So a shut gate makes the pause menu ignore EVERY
/// input, whatever is written into the pad device -- which is the exact shape of every derailed
/// `nav_to_optionsetting` phase this drive has produced.
///
/// The predicate is a conjunction:
///
/// ```text
///   *caller_flag != 0
///   && CSMenuManImp + 0x798 == 0
///   && CSMenuManImp + 0x19  != 0
///   && (disableMouseCursor == false || CSFadeImp::FadePlateTimerHasEnded(fade, 2))
/// ```
///
/// `disableMouseCursor` is the named field at `+0x1a`; the other two are unnamed in the dump, so
/// they are read here by offset and reported raw rather than interpreted. Reading them is the
/// difference between "the key never arrived" and "the key arrived at a menu that was refusing
/// input", which no amount of pressing harder can distinguish.
const CS_MENU_MAN_INPUT_GATE_19_OFFSET: usize = 0x19;
const CS_MENU_MAN_DISABLE_MOUSE_CURSOR_1A_OFFSET: usize = 0x1a;
const CS_MENU_MAN_INPUT_GATE_798_OFFSET: usize = 0x798;

/// `(gate_19, disable_mouse_cursor, gate_798)` straight out of `CSMenuManImp`, or `None` when the
/// singleton is not up.
pub fn menu_input_gate() -> Option<(u8, u8, usize)> {
    let im = input_mgr();
    if im == 0 {
        return None;
    }
    let gate_19 = unsafe { crate::win32::read_u8(im + CS_MENU_MAN_INPUT_GATE_19_OFFSET) }?;
    let disable_cursor =
        unsafe { crate::win32::read_u8(im + CS_MENU_MAN_DISABLE_MOUSE_CURSOR_1A_OFFSET) }?;
    let gate_798 = unsafe { read_usize(im + CS_MENU_MAN_INPUT_GATE_798_OFFSET) }?;
    Some((gate_19, disable_cursor, gate_798))
}

/// `popupMenu->currentTopMenuJob` (inputmgr+0x80 -> +0xB0), or 0. Non-zero ONLY when a popup/pause menu
/// is actually up -- the correct "pause menu open" signal (unlike menuData+0x8). It is a
/// FixOrderJobSequence (NOT a MenuWindowJob), and it is REPLACED when a submenu opens (old pushed to
/// popupMenu+0xD0), so a CHANGE in this pointer is the passive "entered a submenu" semaphore (bd
/// PANE-ID-FIX-currenttopjob-is-sequence-use-plusB0-ptr-change).
pub fn top_menu_job_ptr() -> usize {
    let im = input_mgr();
    if im == 0 {
        return 0;
    }
    let Some(popup) =
        (unsafe { read_usize(im + CS_MENU_MAN_POPUP_MENU_80_OFFSET) }).filter(|p| *p >= HEAP_LO)
    else {
        return 0;
    };
    unsafe { read_usize(popup + CS_POPUP_CURRENT_TOP_JOB_B0_OFFSET) }
        .filter(|p| *p >= HEAP_LO)
        .unwrap_or(0)
}

/// The top menu window (`currentTopMenuJob+0x130`), or 0.
fn top_window() -> usize {
    let job = top_menu_job_ptr();
    if job == 0 {
        return 0;
    }
    unsafe { read_usize(job + TOP_JOB_WINDOW_130_OFFSET) }
        .filter(|p| *p >= HEAP_LO)
        .unwrap_or(0)
}

/// TRUE only when the in-world pause menu (a popup top-job) is up. Replaces the false-positive
/// menu_data_ptr check.
pub fn pause_menu_open() -> bool {
    top_menu_job_ptr() != 0
}

/// The topmost pane's menu id (top_window+0x180, u16), or -1: `INGAMETOP_MENU_ID`=0xffff,
/// `OPTIONSETTING_MENU_ID`=0x25.
pub fn top_menu_id() -> i32 {
    let w = top_window();
    if w == 0 {
        return -1;
    }
    unsafe { read_usize(w + TOP_WINDOW_MENU_ID_180_OFFSET) }.map_or(-1, |v| (v & 0xffff) as i32)
}

/// `GLOBAL_CSPcKeyConfig` (1.16.2 RVA; `er_game_base::mem::game_data_addr` maps it to 0x3d61f08 on
/// 1.17, agreed by 82 references). Resolved from `mov rcx, [rip+0x3607e98]` at 0x140756009, inside
/// the function that turns a menu code into a device binding.
const CS_PC_KEY_CONFIG_GLOBAL_RVA: usize = er_game_base::rva::CS_PC_KEY_CONFIG_SINGLETON_RVA;
/// The binding table inside CSPcKeyConfig: `config + 0x440 + code * 0x14`, valid for `code < 0x36`.
/// Each 0x14-byte entry is five dwords and `FUN_140242b00` picks by mode -- mode 2, which the menu
/// path uses, reads the PAD pair at `+0x0c` and `+0x10`.
const KEY_CONFIG_BINDING_TABLE_OFFSET: usize = 0x440;
const KEY_CONFIG_BINDING_STRIDE: usize = 0x14;
const KEY_CONFIG_BINDING_PAD_PRIMARY_OFFSET: usize = 0x0c;
const KEY_CONFIG_BINDING_PAD_SECONDARY_OFFSET: usize = 0x10;
/// Highest valid menu code -- `FUN_140242ab0` returns an empty binding for anything `>= 0x36`.
pub const KEY_CONFIG_MAX_MENU_CODE: u32 = 0x36;

/// Every device binding a menu code carries: the five dwords of its `0x14`-byte entry, in order.
///
/// `FUN_140242b00` selects a PAIR out of this row by mode -- mode 0 takes `[0]`, mode 1 takes
/// `[1]`/`[2]`, mode 2 takes `[3]`/`[4]` (the pad pair the menu path asks for). Reading the WHOLE row
/// is what turns the table from "the pad id for a code I already identified" into "which code is
/// menu-down": the keyboard half is dword `[0]`, and a DIK scancode is recognisable on sight
/// (`0xd0` down-arrow, `0x1f` S, `0xc8` up-arrow, `0x11` W), so dumping all `0x36` rows names the
/// codes instead of sweeping them.
///
/// The dwords are read as four separate byte-quads rather than through `read_usize`, which would
/// pack two dwords into one value and silently truncate -- how the existing pad reader gets `[3]`
/// right and would get `[4]` wrong if it ever read at `+0x10` with a `usize` that ran off the entry.
pub fn menu_code_binding_row(code: u32) -> Option<[u32; 5]> {
    if code >= KEY_CONFIG_MAX_MENU_CODE {
        return None;
    }
    let base = game_base()?;
    let config = deref_singleton(
        base,
        CS_PC_KEY_CONFIG_GLOBAL_RVA,
        "CS_PC_KEY_CONFIG_GLOBAL_RVA",
    )?;
    let entry =
        config + KEY_CONFIG_BINDING_TABLE_OFFSET + KEY_CONFIG_BINDING_STRIDE * code as usize;
    let mut row = [0u32; 5];
    for (index, slot) in row.iter_mut().enumerate() {
        *slot = unsafe { crate::win32::read_u32(entry + index * 4) }?;
    }
    Some(row)
}

/// The pad binding a menu code resolves to: `(primary, secondary)` from the mode-2 pair, or `None`
/// when the config is not up or the code is out of range.
///
/// WHY READ IT INSTEAD OF GUESSING: menu navigation reads the FD4 pad device through
/// `CS::CSEzMenuViewerPad`, and a menu code is an INDEX into this table, not a device id. Sweeping
/// pad ids to find the one that moves a cursor is how the previous drive ended up injecting into
/// `inputmgr+0x90`, which is a shown-menu-window bitmap and not input at all. This table says which
/// pad input the game itself has bound to each menu action.
pub fn menu_code_pad_binding(code: u32) -> Option<(u32, u32)> {
    if code >= KEY_CONFIG_MAX_MENU_CODE {
        return None;
    }
    let base = game_base()?;
    let config = deref_singleton(
        base,
        CS_PC_KEY_CONFIG_GLOBAL_RVA,
        "CS_PC_KEY_CONFIG_GLOBAL_RVA",
    )?;
    let entry =
        config + KEY_CONFIG_BINDING_TABLE_OFFSET + KEY_CONFIG_BINDING_STRIDE * code as usize;
    let primary = unsafe { read_usize(entry + KEY_CONFIG_BINDING_PAD_PRIMARY_OFFSET) }? as u32;
    let secondary = unsafe { read_usize(entry + KEY_CONFIG_BINDING_PAD_SECONDARY_OFFSET) }? as u32;
    Some((primary, secondary))
}

/// OptionSetting composite (`window+0x1768`) and, within it, the CURRENT pane dialog (`+0xb8`) -- the
/// pane the game's own tab-select writes, so it follows a TabLeft the drive injected rather than a
/// cached guess. Same offsets the product walks in `profile_rows_system_quit_menu.rs`.
const OPTIONSETTING_COMPOSITE_1768_OFFSET: usize = 0x1768;
const OPTIONSETTING_COMPOSITE_CURRENT_PANE_B8_OFFSET: usize = 0xb8;

/// The Quit tab's currently displayed pane dialog, or 0.
pub fn optionsetting_current_pane() -> usize {
    let w = top_window();
    if w == 0 {
        return 0;
    }
    unsafe {
        read_usize(
            w + OPTIONSETTING_COMPOSITE_1768_OFFSET
                + OPTIONSETTING_COMPOSITE_CURRENT_PANE_B8_OFFSET,
        )
    }
    .filter(|p| *p >= HEAP_LO)
    .unwrap_or(0)
}

/// `CS::GridControl` selected-cell index. Read off the pager FUN_1407392f0, which compares
/// `*(int*)(this+0xd4)` against the extents at `+0xd0`/`+0xd8`/`+0xdc`. It is the same field
/// `optionsetting_tab_index` already reads through the tab strip -- because the tab strip IS a
/// GridControl, and so is the pause-menu grid.
const GRID_CONTROL_SELECTED_D4_OFFSET: usize = 0xd4;
/// How far into a menu window to look for an embedded GridControl pointer. The OptionSetting one
/// sits at +0x1870; this covers that and the pause menu's own, without running off the object.
const MENU_WINDOW_SCAN_QWORDS: usize = 0x400;

/// `CS::GridControl`'s vtable ON THE INSTALLED 1.17 BUILD, measured rather than translated.
///
/// Recovered by `scripts/er-rtti-map.py`, which walks MSVC RTTI in `eldenring-deobf-1.17.bin`:
/// TypeDescriptor (`.?AVGridControl@CS@@`, name at `+0x10`) -> CompleteObjectLocator (validated by
/// its own self-RVA and signature 1) -> the qword pointing at that COL, whose `+8` is the vtable.
/// The 1.16.2 value was `0x142a913b8`; nothing translates between them and nothing needs to.
///
/// THIS REPLACES A CIRCULAR RUNTIME DERIVATION. The previous version read GridControl's vtable off
/// the OptionSetting tab strip (`window+0x1870 -> +0x10`) because `map-data-rvas` rated the 1.16.2
/// vtable WEAK on 1.17 -- one reference, one vote. But the tab strip only exists once OptionSetting
/// is OPEN, and opening OptionSetting is what this scan exists to enable: during the pause menu the
/// top window is IngameTop, the `+0x1870` read yields nothing usable, and the function returned
/// `None` before scanning a single slot. Measured on br-20260905-170715-2300, which logged
/// "no GridControl found in the top menu window" at nav frames 0, 120 and 240.
pub(crate) const GRID_CONTROL_VTABLE_RVA_1170: usize = 0x2a94438;

/// Find a `CS::GridControl` inside the top menu window and report `(offset_in_window, selected_cell)`.
pub fn pause_menu_grid() -> Option<(usize, i32)> {
    let window = top_window();
    if window == 0 {
        return None;
    }
    let grid_vtable = game_base()? + GRID_CONTROL_VTABLE_RVA_1170;
    for slot in 0..MENU_WINDOW_SCAN_QWORDS {
        let offset = slot * 8;
        let Some(candidate) = (unsafe { read_usize(window + offset) }).filter(|c| *c >= HEAP_LO)
        else {
            continue;
        };
        if unsafe { read_usize(candidate) } != Some(grid_vtable) {
            continue;
        }
        let selected = unsafe { read_usize(candidate + GRID_CONTROL_SELECTED_D4_OFFSET) }
            .map_or(-1, |v| (v & 0xffff_ffff) as i32);
        return Some((offset, selected));
    }
    None
}

/// Row index of **Load Character from File** on the currently displayed Quit-tab pane, or -1.
///
/// The drive needs this BEFORE it presses Confirm, because the Quit tab also carries *Return to
/// Desktop* -- pressing blind and counting on a row order is how a repro quits the game instead of
/// loading a character. `system_quit_row_label_at` classifies each row by its label, matching the
/// pointer when it can and falling back to an ASCII prefix compare (longest-first, so
/// "Load Character from File" is never mistaken for "Load Character"), which is what makes it usable
/// from THIS DLL even though the label arrays live in `er_quickload.dll`'s image.
pub fn optionsetting_load_from_file_row() -> i32 {
    use er_quit_menu_core::row_identity::system_quit_row_label_at;
    use er_quit_menu_core::rows::{QuitRow, QuitRowLabel};
    let dialog = optionsetting_current_pane();
    if dialog == 0 {
        return -1;
    }
    for index in 0..16i32 {
        if let Some(QuitRowLabel::Ours(QuitRow::LoadSaveProfiles)) =
            unsafe { system_quit_row_label_at(dialog, index) }
        {
            return index;
        }
    }
    -1
}

/// OptionSetting selected tab index (window+0x1870+0x10[deref]+0xd4, i32), or -1. Quit tab = 8.
pub fn optionsetting_tab_index() -> i32 {
    let w = top_window();
    if w == 0 {
        return -1;
    }
    let Some(view) = (unsafe {
        read_usize(w + OPTIONSETTING_TAB_CONTROL_1870_OFFSET + OPTIONSETTING_TAB_VIEW_10_OFFSET)
    })
    .filter(|p| *p >= HEAP_LO) else {
        return -1;
    };
    unsafe { read_usize(view + OPTIONSETTING_TAB_INDEX_D4_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as i32)
}

/// getShownMenuFlags result word (CSMenuManImp+0x1c, u32): the native "which menu input fired this
/// frame" bits -- the passive VERIFICATION that an injected pad button reached the menu layer (bd
/// PAD-BUTTON-OFFSETS): 0x100=confirm(0x3d), 0x10=cancel(0x1c), 0x1000=tab-left(0x30),
/// 0x80000=tab-right(0x31), 0x8000=OptionSetting up. (Up/Down 0x00/0x45 are NOT in this word.)
const CS_MENU_MAN_FLAGS_1C_OFFSET: usize = 0x1c;

pub fn menu_flags() -> u32 {
    let im = input_mgr();
    if im == 0 {
        return 0;
    }
    unsafe { read_usize(im + CS_MENU_MAN_FLAGS_1C_OFFSET) }.map_or(0, |v| (v & 0xffff_ffff) as u32)
}

/// Return-title request byte (menuData+0x5d == 1): the quit-to-title functor fired = quit STARTED.
pub fn return_title_requested() -> bool {
    let im = input_mgr();
    if im == 0 {
        return false;
    }
    let Some(md) =
        (unsafe { read_usize(im + CS_MENU_MAN_MENU_DATA_OFFSET) }).filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    unsafe { read_usize(md + MENU_DATA_RETURN_TITLE_5D_OFFSET) }.is_some_and(|v| (v & 0xff) == 1)
}

/// Read the optional drive-mode flag file: one of `boot`, `reload`, `reload2`, `full`. An absent
/// or unreadable file yields `""`, which `DriveMode::from_flag` maps to `passive`.
///
/// RESOLVED THE SAME WAY AS THE LOG, which it was documented to sit beside but did not (fixed
/// 2026-09-04). It used to be a bare CWD-relative `read_to_string`, and the harness's log had since
/// moved onto `redirected_artifact_path`, so a per-run artifact directory took the log with it and
/// left this file behind. The cost is silent and total: the flag simply reads absent, the harness
/// logs `drive: mode='passive'`, and a run staged to drive itself sits there driving nothing --
/// observed on run br-20260905-023540-8b07, where the flag had been written into the game
/// directory and the process CWD was elsewhere. There is no "flag not found" error to notice,
/// because an absent flag is a legitimate state.
pub fn read_drive_mode_flag() -> String {
    std::fs::read_to_string(er_game_base::log::redirected_artifact_path(
        "ER_HARNESS_DRIVE_MODE_PATH",
        "er-harness-drive-mode.txt",
    ))
    .map(|s| s.trim().to_ascii_lowercase())
    .unwrap_or_default()
}

/// PROBE HOLD-ID (CWD file `er-harness-probe-hold-id.txt` containing a decimal vk-id 1000..1080): in
/// `probe` drive mode, HOLD that single vk-id (instead of sweeping the whole range) so one index's
/// in-world menu action can be isolated -- e.g. confirm which index drives return-to-title. 0/absent =
/// normal sweep. Diagnostic only (bd NEXT-inworld-menu-idmap-recovery-plan).
/// OS-INPUT test mode (CWD file `er-harness-os-input.txt`): in `probe` drive mode, instead of RAM
/// injection, send focus-gated OS keyboard taps (VK_DOWN) to the pause menu -- the game's REAL input path
/// that reaches Scaleform (bd SYNTHESIS-pause-menu-is-scaleform). Tests whether OS input drives the menu.
pub fn os_input_enabled() -> bool {
    std::path::Path::new("er-harness-os-input.txt").exists()
}

/// NATIVE-QUIT test mode (CWD file `er-harness-native-quit.txt`): drive System->Quit by the DIRECT NATIVE
/// request instead of menu input (acceptance §3a: native input cannot reach the Scaleform menu, so the
/// action is reproduced by a direct native state write). See `request_return_to_title`.
pub fn native_quit_enabled() -> bool {
    std::path::Path::new("er-harness-native-quit.txt").exists()
}

/// DIRECT NATIVE return-to-title: write `menuData+0x5d = 1`, the return-to-title request byte the game's
/// own quit-functor / idle-timeout sets (proven: `return_title_requested()` reads exactly this and latches
/// on the game's idle timeout). This reproduces the System->Quit result without any menu input. Returns
/// true if the byte was written (fault-safe via WriteProcessMemory).
pub fn request_return_to_title() -> bool {
    let im = input_mgr();
    if im == 0 {
        return false;
    }
    let Some(md) =
        (unsafe { read_usize(im + CS_MENU_MAN_MENU_DATA_OFFSET) }).filter(|p| *p >= HEAP_LO)
    else {
        return false;
    };
    unsafe { crate::win32::write_u8(md + MENU_DATA_RETURN_TITLE_5D_OFFSET, 1) }
}

pub fn probe_hold_id() -> u32 {
    std::fs::read_to_string(er_game_base::log::redirected_artifact_path(
        "ER_HARNESS_PROBE_HOLD_ID_PATH",
        "er-harness-probe-hold-id.txt",
    ))
    .ok()
    .and_then(|s| s.trim().parse::<u32>().ok())
    .unwrap_or(0)
}

/// FORCE-DRIVE override (env `ER_HARNESS_FORCE_DRIVE=1` OR CWD file `er-harness-force-drive.txt`):
/// make the harness honor its drive-mode flag EVEN when the product DLL is loaded. Default off, so the
/// samechar-3x product run keeps the companion/Passive stand-down (the product owns the drive there).
/// The VANILLA agent-driven baseline needs this: it loads the product for its telemetry (autoload
/// disarmed via telemetry-only) but the HARNESS must drive the native Continue -> Quit -> Continue.
pub fn force_drive_requested() -> bool {
    // The marker resolves beside the LOG, not against the CWD -- same fix, same reason, as
    // `read_drive_mode_flag` (2026-09-04). me3 launches the game with an arbitrary CWD, so a bare
    // relative `exists()` silently answered false for a file sitting in the game directory, and the
    // harness stood down Passive on a run staged to drive itself.
    matches!(std::env::var("ER_HARNESS_FORCE_DRIVE").as_deref(), Ok("1"))
        || er_game_base::log::redirected_artifact_path(
            "ER_HARNESS_FORCE_DRIVE_PATH",
            "er-harness-force-drive.txt",
        )
        .exists()
}

/// COMPANION-AUTOLOAD (bd STEP4-FIX-DIRECTION-PROVEN): when the product DLL is loaded, drive the boot
/// menu-Continue as the AUTOLOAD (DriveMode::BootContinueOnly) instead of standing down Passive -- so the
/// initial load goes through the menu path (run49 PARITY) rather than the product's menu-free
/// `own_load_continue` (which leaves the ~4-6fps epoch1 render residual). Opt-in marker while validating;
/// intended to become the product default once the pure-default smoke reaches parity. The product's own
/// autoload must stand down (er-quickload-diag-no-autoload.txt) so the two do not compete for the boot load.
pub fn companion_autoload_requested() -> bool {
    matches!(
        std::env::var("ER_HARNESS_COMPANION_AUTOLOAD").as_deref(),
        Ok("1")
    ) || std::path::Path::new("er-harness-companion-autoload.txt").exists()
}

/// Compact one-line state snapshot for the log (mirrors the trace DLL's `snapshot()` habit).
pub fn snapshot() -> String {
    let base = game_base().unwrap_or(0);
    let gdm = game_base()
        .and_then(|b| deref_singleton(b, GAME_DATA_MAN_GLOBAL_RVA, "GAME_DATA_MAN_GLOBAL_RVA"))
        .unwrap_or(0);
    format!(
        "base=0x{base:x} gdm=0x{gdm:x} player_present={} menu_data=0x{:x}",
        player_present() as u8,
        menu_data_ptr()
    )
}
