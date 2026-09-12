//! Direct game-memory reads that RE-derive the coarse runtime state the self-drive gates on.
//!
//! Cross-DLL state (constraint #1): separate DLLs do not share Rust statics, so this harness cannot
//! read the product DLL's `SYSTEM_QUIT_INGAME_TOP_WINDOW` / `SYSTEM_QUIT_QUICKLOAD_PHASE` /
//! menu-window latches (those live in `er_quickload.dll`'s image). Those product statics are
//! themselves derived from game memory, so the harness re-derives what it needs the same way
//! `er-reload-trace` reads the game: `GetModuleHandleA(NULL)` for the image base, then
//! fault-safe `ReadProcessMemory` walks of the known singletons.
//!
//! Coarse vs precise (honest limit): the product's window latches are populated by native menu-window
//! ctor hooks (`menu_window_job_ctor_*`, the `SetState` trace). Standalone, a *precise* window
//! identity (IngameTop vs OptionSetting vs ProfileSelect) would require union-registering those same
//! ctor observers through the product's `er_effects_union_register` export and matching vtable RVAs.
//! This module intentionally re-derives only what a passive read can prove: image base, player
//! presence (in-world proxy), and top-menu-window presence -- enough to sequence the proven
//! keyboard-open + submenu edges, not enough to positively identify each pane.

use std::sync::atomic::{AtomicI64, AtomicU32, AtomicUsize, Ordering};

use crate::log::harness_log;
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
/// One past the highest address x64 Windows hands to user mode: the canonical-address split puts
/// every user pointer below `1 << 47`.
///
/// Added because `HEAP_LO` alone is a floor, and the value that broke `top_window` came in from
/// above it. `currentTopMenuJob+0x130` read `0x2f003a00320063` on 1.17.1 -- the UTF-16 for
/// `c2:/aet/aet050/A...` -- which is `1.3e16`, two orders of magnitude past the last address any
/// process on this target can map, and it sailed through a floor-only filter into every caller (bd
/// er-effects-rs-h09b). A ceiling costs one comparison and rejects text, floats and packed field
/// pairs that a floor cannot.
const HEAP_HI: usize = 1usize << 47;

/// Whether a qword read out of the game can be a live object pointer at all: inside the user-mode
/// address range and pointer-aligned.
///
/// This is a screen, not proof -- proof is [`crate::rtti::class_name`] resolving the object's own
/// `CompleteObjectLocator`. It exists so the cheap test runs first, and so no walk in this module
/// carries its own idea of what a pointer looks like.
fn plausible_ptr(value: usize) -> bool {
    (HEAP_LO..HEAP_HI).contains(&value) && value.is_multiple_of(core::mem::align_of::<usize>())
}

/// The game image base (`GetModuleHandleA(NULL)`), or `None` before the image is mapped.
pub fn game_base() -> Option<usize> {
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

/// True when the product DLL (`er_quickload.dll`) is loaded in this process -- a real runtime condition
/// (not a marker file): when the product is present the harness is a companion (the product owns the
/// drive), so the standalone boot/menu drive must stand down and not fight it.
pub fn product_dll_present() -> bool {
    let name = b"er_quickload.dll\0";
    (unsafe { GetModuleHandleA(name.as_ptr().cast()) } as usize) != 0
}

/// Dereference a game singleton pointer by RVA -- Resolved for the running build, never added raw.
///
/// This is the one chokepoint every singleton read in this DLL goes through, and its results feed
/// raw byte stores (`inject_vk` stamps `source+0x88`, the popup request byte at `+0x121`). Every
/// `.data` global moved on 1.17, so a raw `base + rva` reads whatever now occupies the old slot;
/// `plausible_ptr` screens the result but cannot prove it, so a garbage qword inside the user-mode
/// range still passes and the store lands somewhere arbitrary. Resolving here makes an unmapped global answer `None` instead,
/// which is a path every caller already has.
fn deref_singleton(base: usize, rva: usize, what: &'static str) -> Option<usize> {
    let address = er_game_base::mem::game_data_addr(base, rva, what);
    if address == 0 {
        return None;
    }
    let p = unsafe { read_usize(address) }?;
    plausible_ptr(p).then_some(p)
}

/// In-world PROXY: `GameDataMan.playerGameData` (+0x08) is non-null once a character's game data is
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
    unsafe { read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.is_some_and(plausible_ptr)
}

/// Top-menu-window PROXY: `CSMenuMan.menuData` (+0x8) non-null indicates a menu-data owner exists.
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
        .filter(|p| plausible_ptr(*p))
        .unwrap_or(0)
}

/// Cumulative play time (`GameDataMan+0xa0`, u32 ms), or -1 if unavailable. Rises only while the world
/// SIMULATES (frozen in menus / loading), which is why it is the reliable in-world gate -- unlike
/// `playerGameData+0x08`, which is non-null at the title and false-positives (observed 2026-07-22: the
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

/// The `currentTopMenuJob` the cached window was resolved from, the window itself, and the vtable it
/// carried at that moment. The graph walk is bounded but not cheap, and several callers ask for the
/// window in one frame; the vtable is stored so a freed window cannot be re-validated by a pointer
/// comparison alone.
static CACHED_TOP_JOB: AtomicUsize = AtomicUsize::new(0);
static CACHED_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
static CACHED_TOP_WINDOW_VTABLE: AtomicUsize = AtomicUsize::new(0);
/// The last root job [`report_top_window`] wrote a line about, so a pane that stays up for a
/// thousand frames is described once.
static TOP_WINDOW_REPORTED_JOB: AtomicUsize = AtomicUsize::new(0);

static LAST_PLAY_TIME: AtomicI64 = AtomicI64::new(-1);
static WORLD_SIM_STREAK: AtomicU32 = AtomicU32::new(0);

/// True once play_time has risen for `RISING_STREAK` consecutive frames -> a loaded, UNPAUSED character
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

// SL-device-busy semaphores (ground truth from the product constant tree): `GameMan::saveState`
// at +0xb80 (0 idle -> non-0 busy) and the NowLoading latch. A driven Continue "took effect" once
// one of these trips within the frame budget -- else the harness is derailed (bd harness-drive-
// semaphore-gated-teardown-on-miss). GameMan singleton RVA 0x3d69918
// (profile_rows_system_quit_menu.rs), b80 = GAME_MAN_SAVE_STATE_B80_OFFSET; NowLoading singleton
// 0x3d60ec8, flag +0xED (CSNowLoadingHelperImp.load_done).
//
// This was called `GAME_MAN_LOAD_FSM_B80_OFFSET` / `load_fsm()` until 2026-08-31. The field is
// not load-only: the save lane stamps it too (see the value table on the declaration in
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

/// `GameMan::savedMap` (+0xc30, i32): the `BlockId` of the map the mounted character is in.
/// Same field the product calls `GAME_MAN_SAVED_MAP_C30_OFFSET`.
const GAME_MAN_SAVED_MAP_C30_OFFSET: usize = 0xc30;
/// The map id `GameMan` holds when no character is mounted -- `m10_01_00_00`, the new-game default,
/// which is also what the title sits on. `er_title_flow::FULLREAD_C30_M10_DEFAULT` and
/// `er_quit_rows::orphan_title_window::C30_TITLE_DEFAULT` are the same number.
const GAME_MAN_SAVED_MAP_TITLE_DEFAULT: i32 = 0x0a01_0000;
/// `GameMan::savedMap` (+0xc30), or -1 when it cannot be read.
pub fn saved_map() -> i32 {
    let Some(base) = game_base() else {
        return -1;
    };
    let Some(gm) = deref_singleton(base, GAME_MAN_SINGLETON_RVA, "GAME_MAN_SINGLETON_RVA") else {
        return -1;
    };
    unsafe { read_usize(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as u32 as i32)
}

/// True once `GameMan::savedMap` names a real map, i.e. a character's world is being mounted.
///
/// The semaphore that replaced `save_state > 0` as the Continue effect check (bd er-effects-rs-9gxt).
/// `save_state` is the shared save/load device, and the title builds its own profile list by reading
/// the save through that device -- measured on the 2026-09-11 18:13 run, where `save_state` reached 2
/// at the title, `Phase::Continue` scored itself advanced 39 frames in, and the session then sat on
/// the title menu for 152 seconds with `world_sim=0`. This field moves only when a world mounts:
/// the product's own load log records the transition as `c30 0xa010000->0x1c000000`.
pub fn world_map_mounted() -> bool {
    let map = saved_map();
    map != GAME_MAN_SAVED_MAP_TITLE_DEFAULT && map != -1 && map != 0
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

// Flip-timing semaphore. CSFlipperImp singleton base+0x4589ad8; fixed_spf f32@+0x1c is the game's
// frame-time target (0.0167=60, 0.05=20, 0.0333=30, 0.0083=120), mode_current i32@+0xc.
// Correction (bd decisive-reload-20fps-is-render-bound-not-throttle-syncinterval1-refresh4-2026-07-22,
// build a38dccd): the reload 20fps is not a fixed_spf=0.05 cap. Measured across the full reload movable
// windows fixed_spf stays 0.0167 (60fps target) while task_delta(+0x268, actual)=0.05; the game passes
// SyncInterval=1 to Present yet GetFrameStatistics shows 4 refreshes/present -> the frame is render-bound
// (not ready within 1 vblank), not a loading-mode cap. Keep fixed_spf as a phase signal (target vs actual
// divergence) but do not treat 0.05 as the cap mechanism; the earlier fixedspf-0.05 memory is refuted.
const CS_FLIPPER_SINGLETON_RVA: usize = 0x4589ad8;
const CS_FLIPPER_FIXED_SPF_1C_OFFSET: usize = 0x1c;
const CS_FLIPPER_MODE_CURRENT_C_OFFSET: usize = 0xc;

/// CSFlipperImp fixed_spf (+0x1c, f32): the game's frame-time target. 0.05 = the 20fps loading cap,
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

// In-world menu-pane semaphores for the quit-to-menu flow (bd quit-to-menu-semaphores-2026-07-22).
// menuData (inputmgr+0x8) is non-null for the whole session -> useless as "menu open". The real open
// signal is the popupMenu's currentTopMenuJob (HasTopMenuJob 0x14080d810), and the pane identity is the
// top window's menu_id.
const CS_MENU_MAN_POPUP_MENU_80_OFFSET: usize = 0x80;
const CS_POPUP_CURRENT_TOP_JOB_B0_OFFSET: usize = 0xb0;
/// `CS::MenuWindowJob::owningMenuWindow`, the last field of that class and valid only on that class.
/// `CS::MenuWindowJob::Run` reads it at `0x1407ad63c`, `er-quit-rows` reads it every frame on the
/// job `Run` hands it, and `er_title_flow::MENU_WINDOW_JOB_OWNING_WINDOW_OFFSET` is the same number.
/// See [`resolve_top_window`] for why reading it off `currentTopMenuJob` was reading past the end of
/// a different object.
const TOP_JOB_WINDOW_130_OFFSET: usize = 0x130;
/// `CS::MenuWindow`'s menu id (u16). The engine bounds-checks it against `0x47` before indexing
/// `CSMenuMan+0x90` with it, in `MenuWindow::MenuWindow` (`0x140741960`), `FUN_140744dd0` and the
/// job teardown `FUN_1407ada40`; `0xffff` is its "ask the vtable" sentinel rather than a real id.
const TOP_WINDOW_MENU_ID_180_OFFSET: usize = 0x180;
/// OptionSetting SettingTabControl (window+0x1870) -> tab view ptr (+0x10, deref) -> selected index (+0xd4).
const OPTIONSETTING_TAB_CONTROL_1870_OFFSET: usize = 0x1870;
const OPTIONSETTING_TAB_VIEW_10_OFFSET: usize = 0x10;
const OPTIONSETTING_TAB_INDEX_D4_OFFSET: usize = 0xd4;
/// Return-title request byte within menuData (set when the quit-to-title functor fires) = quit started.
const MENU_DATA_RETURN_TITLE_5D_OFFSET: usize = 0x5d;

/// In-world menu pane ids read at `top_window+0x180` (u16).
///
/// Correction, 2026-09-12. This block used to say the offset had drifted on 1.17, on the evidence
/// that `top_menu_id()` answered 53724, 25445 and -1 while `pause_menu_open()` was correctly true.
/// The offset had not drifted: the static RE behind bd er-effects-rs-h09b shows `+0x180` is where
/// the engine itself reads a `CS::MenuWindow`'s menu id on both builds, and `er-quit-rows` reads it
/// there on 1.17.1 every frame. What was wrong was the pointer those reads were made through --
/// `top_window()` was handing them a UTF-16 asset path -- so `top_menu_id()` was reading two bytes
/// out of the middle of a string and reporting them as a pane.
///
/// Still a log field and still not an effect check, because the fix is static and the readings are
/// behavioural: nothing here has been seen answering `0x25` on a live 1.17.1 session yet. A phase
/// gated on `top_menu_id() != OPTIONSETTING_MENU_ID` advances on its first frame if the read is
/// wrong in any way, and reports success for a press it never issued -- which is what
/// `Phase::ActivateLoadFromFile` did before it moved to the `currentTopMenuJob` pointer-change
/// semaphore. Gate on these only once a run has shown them reading the ids below.
#[allow(dead_code)]
pub const INGAMETOP_MENU_ID: i32 = 0xffff;
#[allow(dead_code)]
pub const OPTIONSETTING_MENU_ID: i32 = 0x25;
///
/// Kept, and still not an effect check. `optionsetting_tab_index` walks `window+0x1870`, and every
/// `-1` it has ever returned was measured through the broken `top_window()` -- a chain rooted in a
/// string answers `-1` whatever its offsets are, so those runs are evidence about the root pointer
/// and not about `0x1870`. `Phase::TabToQuit` advances on the Quit tab's own rows being readable and
/// logs the index beside them, which is the line that would show this chain reading a real tab now
/// that it is rooted in a real window.
#[expect(
    dead_code,
    reason = "the diagnostic that would use it prints the raw index instead"
)]
pub const OPTIONSETTING_QUIT_TAB_INDEX: i32 = 8;

fn input_mgr() -> usize {
    game_base()
        .and_then(|b| deref_singleton(b, CS_MENU_MAN_GLOBAL_RVA, "CS_MENU_MAN_GLOBAL_RVA"))
        .unwrap_or(0)
}

/// The gate every menu PAD read passes through (`FUN_140758050`, 1.16.2).
///
/// `CS::GridControl`'s pager (vtable slot 2, `FUN_1407392f0`) does not read a pad device directly.
/// Each direction it tests goes through `FUN_14075d970`, whose first act is to call this predicate
/// with no arguments of its own; when it answers false the lambda holding the menu code is never
/// invoked and the read returns "not pressed". So a shut gate makes the pause menu ignore every
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

/// `popupMenu->currentTopMenuJob` (inputmgr+0x80 -> +0xB0), or 0. Non-zero only when a popup/pause menu
/// is actually up -- the correct "pause menu open" signal (unlike menuData+0x8). It is a
/// FixOrderJobSequence (NOT a MenuWindowJob), and it is REPLACED when a submenu opens (old pushed to
/// popupMenu+0xD0), so a change in this pointer is the passive "entered a submenu" semaphore (bd
/// pane-ID-fix-currenttopjob-is-sequence-use-plusB0-ptr-change).
pub fn top_menu_job_ptr() -> usize {
    let im = input_mgr();
    if im == 0 {
        return 0;
    }
    let Some(popup) = (unsafe { read_usize(im + CS_MENU_MAN_POPUP_MENU_80_OFFSET) })
        .filter(|p| plausible_ptr(*p))
    else {
        return 0;
    };
    unsafe { read_usize(popup + CS_POPUP_CURRENT_TOP_JOB_B0_OFFSET) }
        .filter(|p| plausible_ptr(*p))
        .unwrap_or(0)
}

/// The mangled RTTI names this walk decides on. They are the whole of its build dependence: a class
/// name is stable across patches in a way a vtable address is not, which is why the walk carries
/// these two strings and no address at all.
const MENU_WINDOW_JOB_CLASS: &str = ".?AVMenuWindowJob@CS@@";
const MENU_WINDOW_CLASS: &str = ".?AVMenuWindow@CS@@";
/// How many distinct objects the job-graph walk will visit before giving up. The live graph from
/// `currentTopMenuJob` to the `MenuWindowJob` is four objects deep and holds single digits of jobs
/// at each level, so this is roughly an order of magnitude of headroom -- and it is a hard bound,
/// because every node costs `JOB_OBJECT_SCAN_QWORDS` fault-safe reads.
const JOB_GRAPH_MAX_NODES: usize = 64;
/// How deep the walk follows child pointers.
const JOB_GRAPH_MAX_DEPTH: u8 = 6;
/// How far into a job object to look for pointers to other jobs. `FixOrderJobSequence` keeps its
/// job vector at `+0x18..+0x58` with the count at `+0x60`, and the `FinalizeCallbackJob` that
/// `CSPopupMenu::StartTopMenuJob` installs keeps its inner job at `+0x10`, so `0x100` covers every
/// link in the chain with room to spare. It deliberately does not reach `MenuWindowJob`'s `+0x130`:
/// that field is read once the class is known, never scanned for.
const JOB_OBJECT_SCAN_QWORDS: usize = 32;
/// How many `MenuWindowJob`s the walk will name in its one-time log line.
const JOB_GRAPH_REPORT_LIMIT: usize = 4;

/// The top menu window, resolved through the job graph the engine itself builds, or 0.
///
/// What was wrong, and it was not the offset (bd er-effects-rs-h09b, static RE 2026-09-12).
/// `+0x130` is `CS::MenuWindowJob::owningMenuWindow` -- the engine reads it at `0x1407ad63c` inside
/// `CS::MenuWindowJob::Run`, and the product's own Quit-rows hook reads it every frame on the job
/// that `Run` hands it. But `popupMenu->currentTopMenuJob` is not that job.
/// `CSPopupMenu::StartTopMenuJob` (1.16.2 `FUN_1407f0b50`) chains the caller's job with a
/// wait-frame job and a functor job through `CS::MenuJob::ChainMenuJobs`, wraps the resulting
/// `FixOrderJobSequence` in a `0x58`-byte `CS::FinalizeCallbackJob` (`FUN_1407a8180` heap-allocates
/// exactly `0x58`), and assigns that to `+0xb0`. Reading `+0x130` off a `0x58`-byte object is a read
/// `0xd8` bytes past its end, which is why the live value on 1.17.1 was `0x2f003a00320063` -- the
/// UTF-16 for `c2:/aet/aet050/A...`, heap text sitting behind the job.
///
/// So the walk descends the graph instead: from `currentTopMenuJob`, follow pointers that resolve
/// to polymorphic objects of this image until one of them is a `CS::MenuWindowJob` by RTTI name,
/// then read `+0x130` from that. Every hop is validated by a `CompleteObjectLocator` self-RVA check,
/// so a qword of text cannot be followed, let alone accepted.
///
/// The route the brief suggested -- `CSMenuMan+0x90`'s per-menu-id table -- does not exist. That
/// field is a `byte[0x47]` of shown-flags, not window pointers: `MenuWindowJob::Run` sets
/// `field99_0x90[menu_id] |= 1`, `FUN_140744dd0` sets `|= 3`, and the teardown `FUN_1407ada40`
/// writes `0`. There is no live-window registry in `CSMenuManImp` to read.
///
/// Fails closed. When no `MenuWindowJob` is in the graph, or the window it owns is not a
/// `CS::MenuWindow`, this returns 0 and logs the root job's own class name once -- which is the
/// diagnostic that would name a new wrapper class on a future build, rather than handing a caller
/// something that merely reads like a pointer.
fn top_window() -> usize {
    let job = top_menu_job_ptr();
    if job == 0 {
        return 0;
    }
    let Some(base) = game_base() else {
        return 0;
    };
    // One BFS per distinct top job, not per call: `top_menu_id`, `pause_menu_grid`,
    // `optionsetting_current_pane`, `optionsetting_tab_index` and the phase telemetry all ask for
    // the window within a single frame, and the walk is up to 2048 fault-safe reads.
    if CACHED_TOP_JOB.load(Ordering::Relaxed) == job {
        let window = CACHED_TOP_WINDOW.load(Ordering::Relaxed);
        // A cached refusal is cached too, deliberately. The pane that has no reachable
        // `MenuWindowJob` is exactly the one whose walk costs the most, and re-running it on every
        // call -- five or six times a frame between `top_menu_id`, `pause_menu_grid`,
        // `optionsetting_current_pane`, the tab chain and the phase telemetry -- would turn a
        // refusal into a frame-rate defect.
        if window == 0 {
            return 0;
        }
        let vtable = CACHED_TOP_WINDOW_VTABLE.load(Ordering::Relaxed);
        if unsafe { read_usize(window) } == Some(vtable) {
            return window;
        }
    }
    let found = resolve_top_window(&crate::rtti::LiveMemory, base, job);
    report_top_window(&crate::rtti::LiveMemory, base, job, &found);
    let resolved = found.window;
    let vtable = if resolved == 0 {
        0
    } else {
        unsafe { read_usize(resolved) }.unwrap_or(0)
    };
    CACHED_TOP_JOB.store(job, Ordering::Relaxed);
    CACHED_TOP_WINDOW.store(resolved, Ordering::Relaxed);
    CACHED_TOP_WINDOW_VTABLE.store(vtable, Ordering::Relaxed);
    resolved
}

/// What [`resolve_top_window`] found: the window to use, and how many `CS::MenuWindowJob`s in the
/// graph owned one. The count is carried because more than one is the case nobody has observed yet
/// and the case that would make "the top window" ambiguous -- so the log says it rather than the
/// walk silently picking.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct TopWindow {
    pub(crate) window: usize,
    pub(crate) owners: usize,
}

/// The graph walk behind [`top_window`], parameterised over the reader and free of side effects so
/// it can be exercised against a fake address space on a host build.
pub(crate) fn resolve_top_window<M: crate::rtti::GameMemory>(
    mem: &M,
    base: usize,
    root_job: usize,
) -> TopWindow {
    let mut queue = [(0usize, 0u8); JOB_GRAPH_MAX_NODES];
    let mut windows = [0usize; JOB_GRAPH_REPORT_LIMIT];
    let mut window_count = 0usize;
    let mut head = 0usize;
    queue[0] = (root_job, 0);
    let mut seen = 1usize;

    while head < seen {
        let (node, depth) = queue[head];
        head += 1;
        if crate::rtti::is_class(mem, base, node, MENU_WINDOW_JOB_CLASS) {
            let window = mem
                .read_usize(node + TOP_JOB_WINDOW_130_OFFSET)
                .filter(|w| plausible_ptr(*w))
                .unwrap_or(0);
            if window != 0
                && crate::rtti::derives_from(mem, base, window, MENU_WINDOW_CLASS)
                && window_count < JOB_GRAPH_REPORT_LIMIT
            {
                windows[window_count] = window;
                window_count += 1;
            }
            // A `MenuWindowJob` owns a window, not more jobs -- descending into it would follow its
            // `std::function` members for nothing.
            continue;
        }
        if depth >= JOB_GRAPH_MAX_DEPTH {
            continue;
        }
        for slot in 0..JOB_OBJECT_SCAN_QWORDS {
            if seen >= JOB_GRAPH_MAX_NODES {
                break;
            }
            let Some(child) = mem.read_usize(node + slot * core::mem::size_of::<usize>()) else {
                continue;
            };
            // The cheap screen: a `CompleteObjectLocator` that points at itself, four reads, no
            // name. Reading the name of every qword in every object would be the same walk at
            // roughly forty times the cost, on the game thread.
            if !plausible_ptr(child) || crate::rtti::locator(mem, base, child).is_none() {
                continue;
            }
            if queue[..seen]
                .iter()
                .any(|(seen_node, _)| *seen_node == child)
            {
                continue;
            }
            queue[seen] = (child, depth + 1);
            seen += 1;
        }
    }

    TopWindow {
        window: windows[..window_count].first().copied().unwrap_or(0),
        owners: window_count,
    }
}

/// One log line per distinct top job, naming what the walk found or what it refused.
///
/// The point is the refusal case. Before this, a `top_window` that resolved to garbage was
/// indistinguishable in the log from one that resolved correctly -- the callers just started
/// answering `-1` and `no GridControl found`, with nothing saying why. Now a graph with no
/// `CS::MenuWindowJob` in it prints the root job's own class name, which is exactly what a future
/// build's new wrapper class would need to be named.
fn report_top_window<M: crate::rtti::GameMemory>(
    mem: &M,
    base: usize,
    root_job: usize,
    found: &TopWindow,
) {
    if TOP_WINDOW_REPORTED_JOB.swap(root_job, Ordering::Relaxed) == root_job {
        return;
    }
    let root_class = crate::rtti::class_name(mem, base, root_job).map_or_else(
        || "<not a polymorphic object>".to_string(),
        |c| c.to_string(),
    );
    if found.window == 0 {
        harness_log!(
            "top-window: REFUSED -- no CS::MenuWindowJob reachable from currentTopMenuJob \
             0x{root_job:x} (class {root_class}), so there is no owningMenuWindow to read and every \
             caller gets 0 rather than whatever +0x130 happens to hold"
        );
        return;
    }
    let window = found.window;
    let window_class = crate::rtti::class_name(mem, base, window)
        .map_or_else(|| "<unnamed>".to_string(), |c| c.to_string());
    harness_log!(
        "top-window: resolved 0x{window:x} (class {window_class}) from currentTopMenuJob \
         0x{root_job:x} (class {root_class}); {} MenuWindowJob(s) in that graph own a window",
        found.owners
    );
}

/// True only when the in-world pause menu (a popup top-job) is up. Replaces the false-positive
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
/// `FUN_140242b00` selects a pair out of this row by mode -- mode 0 takes `[0]`, mode 1 takes
/// `[1]`/`[2]`, mode 2 takes `[3]`/`[4]` (the pad pair the menu path asks for). Reading the whole row
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
/// Why read it instead of GUESSING: menu navigation reads the FD4 pad device through
/// `CS::CSEzMenuViewerPad`, and a menu code is an index into this table, not a device id. Sweeping
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

/// OptionSetting composite (`window+0x1768`) and, within it, the current pane dialog (`+0xb8`) -- the
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
    .filter(|p| plausible_ptr(*p))
    .unwrap_or(0)
}

/// `CS::GridControl` selected-cell index. Read off the pager FUN_1407392f0, which compares
/// `*(int*)(this+0xd4)` against the extents at `+0xd0`/`+0xd8`/`+0xdc`. It is the same field
/// `optionsetting_tab_index` already reads through the tab strip -- because the tab strip is a
/// GridControl, and so is the pause-menu grid.
const GRID_CONTROL_SELECTED_D4_OFFSET: usize = 0xd4;
/// How far into a menu window to look for an embedded GridControl pointer. The OptionSetting one
/// sits at +0x1870; this covers that and the pause menu's own, without running off the object.
const MENU_WINDOW_SCAN_QWORDS: usize = 0x400;

/// `CS::GridControl`'s vtable on the installed 1.17 build, measured rather than translated.
///
/// Recovered by `scripts/er-rtti-map.py`, which walks MSVC RTTI in `eldenring-deobf-1.17.bin`:
/// TypeDescriptor (`.?AVGridControl@CS@@`, name at `+0x10`) -> CompleteObjectLocator (validated by
/// its own self-RVA and signature 1) -> the qword pointing at that COL, whose `+8` is the vtable.
/// The 1.16.2 value was `0x142a913b8`; nothing translates between them and nothing needs to.
///
/// This replaces a circular runtime derivation. The previous version read GridControl's vtable off
/// the OptionSetting tab strip (`window+0x1870 -> +0x10`) because `map-data-rvas` rated the 1.16.2
/// vtable weak on 1.17 -- one reference, one vote. But the tab strip only exists once OptionSetting
/// is open, and opening OptionSetting is what this scan exists to enable: during the pause menu the
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
        let Some(candidate) =
            (unsafe { read_usize(window + offset) }).filter(|c| plausible_ptr(*c))
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

/// `PlayerGameData -> EquipGameData` and `EquipGameData -> the carried EquipInventoryData`.
///
/// The same two hops `er-build-import-runtime` walks, repeated here because this DLL cannot call
/// into that one and the harness needs its own answer.
const PLAYER_GAME_DATA_EQUIP_2B0_OFFSET: usize = 0x2b0;
const EQUIP_GAME_DATA_INVENTORY_158_OFFSET: usize = 0x158;
/// `EquipInventoryData.nextSortId`, the monotonic acquisition counter.
const EQUIP_INVENTORY_NEXT_SORT_ID_84_OFFSET: usize = 0x84;

/// The carried inventory's acquisition counter, or -1 when it cannot be read.
///
/// The effect oracle for [`crate::drive`]'s build-import phase, and the reason that phase can
/// prove anything at all. Every other row on the Quit tab opens a pane, so a changed
/// `currentTopMenuJob` is evidence the press landed; **Load Build from URL** opens nothing -- it
/// grants, equips and re-orders the character in place -- so the top job never moves and a
/// job-pointer check would report the press as never having happened.
///
/// This counter is what the import moves, and it moves it a lot: `CS::EquipInventoryData::InsertItem`
/// stamps `entry.sortId` from it and increments on every insert, and the importer's reorder pass
/// deposits and retrieves every item the build names. Measured on the live 1.17.1 process at
/// pid 790212 on 2026-09-10: 13393, with 2225 carried entries holding 2225 distinct sort ids and
/// the largest at 13392.
///
/// It is not a general-purpose "did anything happen" flag: it also rises when the player picks
/// something up. During a driven run nothing else adds items, which is what makes it usable here.
pub fn carried_next_sort_id() -> i64 {
    let Some(base) = game_base() else {
        return -1;
    };
    let Some(gdm) = deref_singleton(base, GAME_DATA_MAN_GLOBAL_RVA, "GAME_DATA_MAN_GLOBAL_RVA")
    else {
        return -1;
    };
    let Some(pgd) = (unsafe { read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) })
        .filter(|p| plausible_ptr(*p))
    else {
        return -1;
    };
    let inventory = pgd + PLAYER_GAME_DATA_EQUIP_2B0_OFFSET + EQUIP_GAME_DATA_INVENTORY_158_OFFSET;
    unsafe { read_usize(inventory + EQUIP_INVENTORY_NEXT_SORT_ID_84_OFFSET) }
        .map_or(-1, |v| i64::from((v & 0xffff_ffff) as u32))
}

/// Row index of **Load Build from URL** on the currently displayed Quit-tab pane, or -1.
///
/// Read for the same reason [`optionsetting_load_from_file_row`] is read: the Quit tab carries
/// *Return to Desktop*, and a guessed row order quits the game instead of importing a build.
#[cfg(windows)]
pub fn optionsetting_load_build_url_row() -> i32 {
    optionsetting_row_of(er_quit_menu_core::rows::QuitRow::LoadBuildFromUrl)
}

/// Row index of **Load Character from File** on the currently displayed Quit-tab pane, or -1.
///
/// The drive needs this before it presses Confirm, because the Quit tab also carries *Return to
/// Desktop* -- pressing blind and counting on a row order is how a repro quits the game instead of
/// loading a character. `system_quit_row_label_at` classifies each row by its label, matching the
/// pointer when it can and falling back to an ASCII prefix compare (longest-first, so
/// "Load Character from File" is never mistaken for "Load Character"), which is what makes it usable
/// from this DLL even though the label arrays live in `er_quickload.dll`'s image.
#[cfg(windows)]
pub fn optionsetting_load_from_file_row() -> i32 {
    optionsetting_row_of(er_quit_menu_core::rows::QuitRow::LoadSaveProfiles)
}

/// Row index of one of our cloned rows on the currently displayed Quit-tab pane, or -1.
///
/// One walk for every caller, so a second row can be driven without a second copy of the scan --
/// and so the two callers cannot drift into disagreeing about which pane they read.
///
/// Windows-only because `er_quit_menu_core::row_identity` is: the label walk it performs reads the
/// game's own row objects, so the module has nothing to compile on a host build and the import is an
/// error there rather than dead code.
#[cfg(windows)]
fn optionsetting_row_of(wanted: er_quit_menu_core::rows::QuitRow) -> i32 {
    use er_quit_menu_core::row_identity::system_quit_row_label_at;
    use er_quit_menu_core::rows::QuitRowLabel;
    let dialog = optionsetting_current_pane();
    if dialog == 0 {
        return -1;
    }
    for index in 0..16i32 {
        if let Some(QuitRowLabel::Ours(row)) = unsafe { system_quit_row_label_at(dialog, index) }
            && row == wanted
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
    .filter(|p| plausible_ptr(*p)) else {
        return -1;
    };
    unsafe { read_usize(view + OPTIONSETTING_TAB_INDEX_D4_OFFSET) }
        .map_or(-1, |v| (v & 0xffff_ffff) as i32)
}

/// getShownMenuFlags result word (CSMenuManImp+0x1c, u32): the native "which menu input fired this
/// frame" bits -- the passive verification that an injected pad button reached the menu layer (bd
/// PAD-button-OFFSETS): 0x100=confirm(0x3d), 0x10=cancel(0x1c), 0x1000=tab-left(0x30),
/// 0x80000=tab-right(0x31), 0x8000=OptionSetting up. (Up/Down 0x00/0x45 are not in this word.)
const CS_MENU_MAN_FLAGS_1C_OFFSET: usize = 0x1c;

pub fn menu_flags() -> u32 {
    let im = input_mgr();
    if im == 0 {
        return 0;
    }
    unsafe { read_usize(im + CS_MENU_MAN_FLAGS_1C_OFFSET) }.map_or(0, |v| (v & 0xffff_ffff) as u32)
}

/// Return-title request byte (menuData+0x5d == 1): the quit-to-title functor fired = quit started.
pub fn return_title_requested() -> bool {
    let im = input_mgr();
    if im == 0 {
        return false;
    }
    let Some(md) =
        (unsafe { read_usize(im + CS_MENU_MAN_MENU_DATA_OFFSET) }).filter(|p| plausible_ptr(*p))
    else {
        return false;
    };
    unsafe { read_usize(md + MENU_DATA_RETURN_TITLE_5D_OFFSET) }.is_some_and(|v| (v & 0xff) == 1)
}

/// Read the optional drive-mode flag file: one of `boot`, `reload`, `reload2`, `full`. An absent
/// or unreadable file yields `""`, which `DriveMode::from_flag` maps to `passive`.
///
/// Resolved the same way as the log, which it was documented to sit beside but did not (fixed
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

/// Probe hold-ID (CWD file `er-harness-probe-hold-id.txt` containing a decimal vk-id 1000..1080): in
/// `probe` drive mode, hold that single vk-id (instead of sweeping the whole range) so one index's
/// in-world menu action can be isolated -- e.g. confirm which index drives return-to-title. 0/absent =
/// normal sweep. Diagnostic only (bd next-inworld-menu-idmap-recovery-plan).
/// OS-input test mode (CWD file `er-harness-os-input.txt`): in `probe` drive mode, instead of RAM
/// injection, send focus-gated OS keyboard taps (VK_DOWN) to the pause menu -- the game's real input path
/// that reaches Scaleform (bd synthesis-pause-menu-is-scaleform). Tests whether OS input drives the menu.
pub fn os_input_enabled() -> bool {
    std::path::Path::new("er-harness-os-input.txt").exists()
}

/// Native-quit test mode (CWD file `er-harness-native-quit.txt`): drive System->Quit by the direct native
/// request instead of menu input (acceptance §3a: native input cannot reach the Scaleform menu, so the
/// action is reproduced by a direct native state write). See `request_return_to_title`.
pub fn native_quit_enabled() -> bool {
    std::path::Path::new("er-harness-native-quit.txt").exists()
}

/// Direct native return-to-title: write `menuData+0x5d = 1`, the return-to-title request byte the game's
/// own quit-functor / idle-timeout sets (proven: `return_title_requested()` reads exactly this and latches
/// on the game's idle timeout). This reproduces the System->Quit result without any menu input. Returns
/// true if the byte was written (fault-safe via WriteProcessMemory).
pub fn request_return_to_title() -> bool {
    let im = input_mgr();
    if im == 0 {
        return false;
    }
    let Some(md) =
        (unsafe { read_usize(im + CS_MENU_MAN_MENU_DATA_OFFSET) }).filter(|p| plausible_ptr(*p))
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

/// Force-drive override (env `ER_HARNESS_FORCE_DRIVE=1` or CWD file `er-harness-force-drive.txt`):
/// make the harness honor its drive-mode flag even when the product DLL is loaded. Default off, so the
/// samechar-3x product run keeps the companion/Passive stand-down (the product owns the drive there).
/// The vanilla agent-driven baseline needs this: it loads the product for its telemetry (autoload
/// disarmed via telemetry-only) but the harness must drive the native Continue -> Quit -> Continue.
pub fn force_drive_requested() -> bool {
    // The marker resolves beside the log, not against the CWD -- same fix, same reason, as
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

/// Companion-AUTOLOAD (bd STEP4-fix-direction-proven): when the product DLL is loaded, drive the boot
/// menu-Continue as the AUTOLOAD (DriveMode::BootContinueOnly) instead of standing down Passive -- so the
/// initial load goes through the menu path (run49 parity) rather than the product's menu-free
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
