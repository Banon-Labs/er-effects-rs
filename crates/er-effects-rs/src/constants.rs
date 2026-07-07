//! Runtime constants, static state, and reverse-engineered layout facts.
//!
//! This is intentionally broad for the first lib.rs slimming pass. Split into
//! narrower constants submodules once stable clusters emerge.

#![allow(unused_imports)]

use std::sync::{
    Once,
    atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, AtomicUsize},
};

use crate::input_blocker::InputBlocker;
use eldenring::{
    cs::{ChrAsm, EquipGameData, FaceData, FaceDataBuffer, GameDataMan, GameMan, PlayerGameData},
    dlkr::DLAllocator,
    fd4::FD4TaskData,
};
use fromsoftware_shared::{F32Vector4, FromStatic};
use windows::Win32::Foundation::HWND;

pub(crate) const DLL_MAIN_SUCCESS: i32 = 1;
pub(crate) const DIRECTINPUT_FORWARD_UNRESOLVED: usize = 0;
pub(crate) const DIRECTINPUT_FORWARD_ERROR_MOD_NOT_FOUND: i32 = 0x8007_007e_u32 as i32;
pub(crate) const DINPUT8_SYSTEM_DLL: &[u8] = b"C:\\windows\\system32\\dinput8.dll\0";
pub(crate) const DIRECTINPUT8_CREATE_SYMBOL: &[u8] = b"DirectInput8Create\0";
pub(crate) const APPEAR_ANIMATION_ID: i32 = 63010;
pub(crate) const OVERLAY_INITIAL_POSITION: [f32; 2] = [24.0, 24.0];
pub(crate) const OVERLAY_INITIAL_SIZE: [f32; 2] = [420.0, 420.0];
/// TimeAct animation IDs at or below this value mark unused/cleared queue
/// slots rather than a real animation.
pub(crate) const INVALID_ANIMATION_ID_FLOOR: i32 = 0;
/// Current local-player TimeAct animation id, or 0 when none/player unavailable. This is the product
/// semaphore for "player animations are going" and is later than bare world/player-present readiness.
pub(crate) static PLAYER_CURRENT_ANIMATION_ID: AtomicI32 = AtomicI32::new(0);
pub(crate) const ANIM_QUEUE_SLOT_STEP: u32 = 1;
pub(crate) const ANIM_QUEUE_SCAN_FLOOR: u32 = 0;
pub(crate) const CUSTOM_CALL_DEFAULT_ID: i32 = 0;
pub(crate) const NEXT_INDEX_OFFSET: usize = 1;
pub(crate) const TITLE_HANDOFF_INCOMPLETE: usize = 0;
pub(crate) const TITLE_HANDOFF_COMPLETE_VALUE: usize = 1;
pub(crate) const STACK_TRACE_FRAME_COUNT: usize = 8;
pub(crate) const STACK_TRACE_FRAMES_TO_SKIP: u32 = 0;
pub(crate) const NULL_MODULE_BASE: usize = 0;
pub(crate) const HOOK_ORIGINAL_UNSET: usize = 0;
pub(crate) const HOOK_FALSE_RETURN: u8 = 0;

#[repr(usize)]
pub(crate) enum RuntimeGlobalRva {
    NowLoadingSingleton = 0x3d60ec8,
    FakeLoadingScreenSingleton = 0x3d74868,
    CsGraphicsSingleton = 0x3d71c48,
    RendManSingleton = 0x3d7b0c0,
    CsScaleformSingleton = 0x3d83148,
    Fd4IoPool = 0x4853048,
    Fd4IoWorkerManager = 0x4852f88,
    IoDeviceSingleton = 0x4589390,
    DluidInputManager = 0x485dc18,
}

/// Access-violation NTSTATUS (0xC0000005) as the i32 the OS passes to a VEH.
pub(crate) const EXCEPTION_ACCESS_VIOLATION_CODE: u32 = 0xC000_0005;
/// VEH disposition: leave the exception for the game's own handlers.
pub(crate) const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// Run our VEH first so it logs before Arxan's handlers consume the exception.
pub(crate) const VECTORED_FIRST_HANDLER: u32 = 1;
/// Cap access-violation log lines so an Arxan exception storm cannot fill disk.
pub(crate) const MAX_AV_LOG_LINES: usize = 32;
pub(crate) const AV_LOG_LINE_INCREMENT: usize = 1;
/// Number of process-exit paths hooked (ExitProcess, TerminateProcess,
/// RtlExitUserProcess, NtTerminateProcess).
pub(crate) const CRASH_EXIT_TARGET_COUNT: usize = 4;
// Hardware write-watchpoint on GameMan+0xc30 (the save-mount map write): set DR0 to
// &c30 + DR7 to a 4-byte data-write breakpoint on the game threads, so the EXACT
// writing instruction (vanilla OR Seamless/ERSC) traps into our VEH with its RIP +
// call stack -- no guessing which function does the deserialize. Win64 CONTEXT field
// offsets (fixed by the ABI) + the debug-register encodings.
pub(crate) const EXCEPTION_SINGLE_STEP_CODE: u32 = 0x80000004;
pub(crate) const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
pub(crate) const CONTEXT_AMD64_SIZE: usize = 0x4d0;
pub(crate) const CONTEXT_FLAGS_OFFSET: usize = 0x30;
pub(crate) const CONTEXT_DR0_OFFSET: usize = 0x48;
pub(crate) const CONTEXT_DR6_OFFSET: usize = 0x68;
pub(crate) const CONTEXT_DR7_OFFSET: usize = 0x70;
pub(crate) const CONTEXT_RIP_OFFSET: usize = 0xf8;
/// CONTEXT_AMD64 (0x100000) | CONTEXT_DEBUG_REGISTERS (0x10).
pub(crate) const CONTEXT_DEBUG_REGISTERS_FLAG: u32 = 0x0010_0010;
/// DR7: L0 (bit0) enable DR0 local + R/W0=01 (data write, bits16-17) + LEN0=11
/// (4 bytes, bits18-19) = 0xd0001.
pub(crate) const DR7_C30_WRITE_WATCH: u64 = 0xd0001;
pub(crate) const DR7_DISARM: u64 = 0;
pub(crate) const DR6_CLEAR: u64 = 0;
/// DR6 bit0 set == the DR0 watchpoint condition was the cause.
pub(crate) const DR6_DR0_HIT_MASK: u64 = 0x1;
/// THREAD_SUSPEND_RESUME(0x2) | THREAD_GET_CONTEXT(0x8) | THREAD_SET_CONTEXT(0x10).
pub(crate) const THREAD_WATCH_ACCESS: u32 = 0x1a;
pub(crate) const TH32CS_SNAPTHREAD: u32 = 0x4;
pub(crate) const TOOLHELP_ALL_PROCESSES: u32 = 0;
pub(crate) const TOOLHELP_INVALID_SNAPSHOT: isize = -1;
pub(crate) const INVALID_THREAD_HANDLE: isize = 0;
pub(crate) const TOOLHELP_ITER_OK: i32 = 1;
pub(crate) const SET_THREAD_CONTEXT_OK: i32 = 1;
/// Cap watchpoint hit log lines (multiple c30 writes across a session).
pub(crate) const MAX_C30_WATCH_HITS: usize = 12;
pub(crate) const C30_WATCH_HIT_INCREMENT: usize = 1;
pub(crate) const C30_WATCH_NEVER_ARMED: usize = 0;
/// Re-arm cadence (frames) until the first hit, to cover load threads spawned after
/// the initial arm.
pub(crate) const C30_WATCH_REARM_INTERVAL: usize = 64;
pub(crate) const C30_WATCH_TICK_BIAS: usize = 1;
pub(crate) const C30_WATCH_ARM_COUNT_NONE: i32 = 0;
pub(crate) static C30_WATCH_LAST_ARM_TICK: AtomicUsize = AtomicUsize::new(C30_WATCH_NEVER_ARMED);
pub(crate) static C30_WATCH_HITS: AtomicUsize = AtomicUsize::new(0);
/// 16-byte alignment for the stack CONTEXT buffer (Get/SetThreadContext require it);
/// mask = align-1. Over-allocate by CONTEXT_ALIGN then round the pointer up.
pub(crate) const CONTEXT_ALIGN: usize = 16;
pub(crate) const CONTEXT_ALIGN_MASK: usize = 0xf;
pub(crate) const CONTEXT_ZERO_FILL: u8 = 0;
pub(crate) const C30_WATCH_ARM_INCREMENT: i32 = 1;
/// OpenThread bInheritHandle = FALSE.
pub(crate) const INHERIT_HANDLE_FALSE: i32 = 0;
/// Monotonic per-frame counter that paces the watchpoint re-arm cadence without
/// taking the EffectsState lock before the player check.
pub(crate) static C30_WATCH_FRAME_COUNTER: AtomicUsize = AtomicUsize::new(0);

include!("constants/software_breakpoints.rs");
include!("constants/anti_debug.rs");
include!("constants/portrait_semaphores.rs");
include!("constants/portrait_camera.rs");
include!("constants/portrait_lookat.rs");
include!("constants/tpf_textures.rs");
include!("constants/stats_panel_background.rs");
include!("constants/stats_panel_text.rs");
include!("constants/gaitem_restore.rs");
include!("constants/own_load_pump.rs");
include!("constants/stage2_menu_drive.rs");
include!("constants/player_correctness.rs");
include!("constants/autoload_state.rs");
include!("constants/profile_render.rs");
include!("constants/return_title.rs");
include!("constants/switch_liveness.rs");
include!("constants/loading_cover.rs");
include!("constants/system_quit.rs");
include!("constants/menu_sort.rs");
