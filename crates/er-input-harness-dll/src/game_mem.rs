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

use crate::input_scheduler::{DialogAcceptGate, WEAPON_UPGRADE_OPEN_MENU_ID};
use crate::win32::{GetModuleHandleA, read_f32, read_u32, read_usize};

// RVAs/offsets ported verbatim from the product's constant tree (image base 0x140000000):
//   GAME_DATA_MAN_GLOBAL_RVA / +0x08 PlayerGameData -- er-reload-trace-dll src/lib.rs
//   CS_MENU_MAN_GLOBAL_RVA / CS_MENU_MAN_MENU_DATA_OFFSET -- crates/er-effects-rs/src/constants/*
// They are plain integer literals (addresses the DLL reads), not shared statics.
const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize = 0x08;
const CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0;
const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;
/// Base `CS::MessageBoxDialog` vtable RVA. The top-window dialog accept reader uses this as a type
/// gate before reading dialog-layout offsets.
const MSGBOX_DIALOG_VTABLE_RVA: usize = 0x2b03550;
/// `CS::SaveRetryDialog` is a `MessageBoxDialog` subclass used by startup/offline title paths and
/// shares the same fade/settle accept fields.
const SAVE_RETRY_DIALOG_VTABLE_RVA: usize = 0x2aaabf8;
/// `CurrentOpenMenu` global written by the EventFlag/menu invoke open-menu helpers. In the active
/// Ghidra MCP 1.16.2 dump, invoke case `0x88` calls `FUN_140e9da60`, which writes `0x17` here for
/// weapon reinforcement/upgrade. This is diagnostic telemetry until a runtime trace correlates it
/// against the live process build.
const CURRENT_OPEN_MENU_ID_RVA: usize = 0x458baec;

/// Lowest plausible heap/image pointer -- filters null and small sentinel values out of walks.
const HEAP_LO: usize = 0x10000;

/// The game image base (`GetModuleHandleA(NULL)`), or `None` before the image is mapped.
pub fn game_base() -> Option<usize> {
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    (base != 0).then_some(base)
}

/// True when the PRODUCT DLL (`er_effects_rs.dll`) is loaded in this process -- a REAL runtime condition
/// (not a marker file): when the product is present the harness is a COMPANION (the product owns the
/// drive), so the standalone boot/menu drive must stand down and not fight it.
pub fn product_dll_present() -> bool {
    let name = b"er_effects_rs.dll\0";
    (unsafe { GetModuleHandleA(name.as_ptr().cast()) } as usize) != 0
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

/// Current high-level open-menu id, if readable.
pub fn current_open_menu_id() -> Option<u32> {
    game_base().and_then(|b| unsafe { read_u32(b + CURRENT_OPEN_MENU_ID_RVA) })
}

/// TRUE once the native weapon-reinforcement open path has selected the weapon-upgrade menu family.
pub fn weapon_upgrade_open_menu_active() -> bool {
    current_open_menu_id() == Some(WEAPON_UPGRADE_OPEN_MENU_ID)
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
    let Some(flipper) = deref_singleton(base, CS_FLIPPER_SINGLETON_RVA) else {
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
    let Some(flipper) = deref_singleton(base, CS_FLIPPER_SINGLETON_RVA) else {
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
/// Generic `CS::MessageBoxDialog` accept gate: OK handler commits only when `+0x2300 >= +0x1278`.
/// These offsets are used only after a caller has independently established that the top window is a
/// message-box/dialog object.
const MSGBOX_FADE_REQUIRED_1278_OFFSET: usize = 0x1278;
const MSGBOX_FADE_ELAPSED_2300_OFFSET: usize = 0x2300;
/// OptionSetting SettingTabControl (window+0x1870) -> tab view ptr (+0x10, deref) -> selected index (+0xd4).
const OPTIONSETTING_TAB_CONTROL_1870_OFFSET: usize = 0x1870;
const OPTIONSETTING_TAB_VIEW_10_OFFSET: usize = 0x10;
const OPTIONSETTING_TAB_INDEX_D4_OFFSET: usize = 0xd4;
/// Return-title request byte within menuData (set when the quit-to-title functor fires) = quit STARTED.
const MENU_DATA_RETURN_TITLE_5D_OFFSET: usize = 0x5d;

/// In-world menu pane ids read at top_window+0x180 (u16).
pub const INGAMETOP_MENU_ID: i32 = 0xffff;
pub const OPTIONSETTING_MENU_ID: i32 = 0x25;
pub const OPTIONSETTING_QUIT_TAB_INDEX: i32 = 8;

fn input_mgr() -> usize {
    game_base()
        .and_then(|b| deref_singleton(b, CS_MENU_MAN_GLOBAL_RVA))
        .unwrap_or(0)
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

/// Current top popup window pointer, if any.
pub fn top_window_ptr() -> usize {
    top_window()
}

/// Current top popup window vtable pointer, if readable.
pub fn top_window_vtable() -> usize {
    let window = top_window();
    if window == 0 {
        return 0;
    }
    unsafe { read_usize(window) }.unwrap_or(0)
}

/// Read the generic `CS::MessageBoxDialog` fade/settle accept gate from a known dialog pointer.
pub fn dialog_accept_gate(dialog: usize) -> Option<DialogAcceptGate> {
    if dialog < HEAP_LO {
        return None;
    }
    let required_elapsed = unsafe { read_f32(dialog + MSGBOX_FADE_REQUIRED_1278_OFFSET) }?;
    let elapsed = unsafe { read_f32(dialog + MSGBOX_FADE_ELAPSED_2300_OFFSET) }?;
    Some(DialogAcceptGate::new(required_elapsed, elapsed))
}

fn known_message_box_dialog(window: usize) -> bool {
    let Some(base) = game_base() else {
        return false;
    };
    let Some(vtable) = (unsafe { read_usize(window) }).filter(|p| *p >= base) else {
        return false;
    };
    vtable == base + MSGBOX_DIALOG_VTABLE_RVA || vtable == base + SAVE_RETRY_DIALOG_VTABLE_RVA
}

/// Candidate accept gate for the current top popup window, valid only when the top window has a
/// known `CS::MessageBoxDialog`-family vtable.
pub fn top_window_dialog_accept_gate() -> Option<DialogAcceptGate> {
    let window = top_window();
    known_message_box_dialog(window).then(|| dialog_accept_gate(window))?
}

/// TRUE when the top popup window is a readable dialog whose native fade/settle gate is ready.
pub fn top_window_dialog_accept_ready() -> bool {
    top_window_dialog_accept_gate().is_some_and(DialogAcceptGate::is_ready)
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

/// Read the optional drive-mode flag file (CWD-relative, same dir as the log): one of `boot`,
/// `reload`, `full` (default `full`). Lets a run switch the drive PATTERN without a rebuild.
pub fn read_drive_mode_flag() -> String {
    std::fs::read_to_string("er-harness-drive-mode.txt")
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
    std::fs::read_to_string("er-harness-probe-hold-id.txt")
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
    matches!(std::env::var("ER_HARNESS_FORCE_DRIVE").as_deref(), Ok("1"))
        || std::path::Path::new("er-harness-force-drive.txt").exists()
}

/// COMPANION-AUTOLOAD (bd STEP4-FIX-DIRECTION-PROVEN): when the product DLL is loaded, drive the boot
/// menu-Continue as the AUTOLOAD (DriveMode::BootContinueOnly) instead of standing down Passive -- so the
/// initial load goes through the menu path (run49 PARITY) rather than the product's menu-free
/// `own_load_continue` (which leaves the ~4-6fps epoch1 render residual). Opt-in marker while validating;
/// intended to become the product default once the pure-default smoke reaches parity. The product's own
/// autoload must stand down (er-effects-diag-no-autoload.txt) so the two do not compete for the boot load.
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
        .and_then(|b| deref_singleton(b, GAME_DATA_MAN_GLOBAL_RVA))
        .unwrap_or(0);
    format!(
        "base=0x{base:x} gdm=0x{gdm:x} player_present={} menu_data=0x{:x} open_menu={}",
        player_present() as u8,
        menu_data_ptr(),
        current_open_menu_id()
            .map(|id| format!("0x{id:x}"))
            .unwrap_or_else(|| "unreadable".to_string())
    )
}
