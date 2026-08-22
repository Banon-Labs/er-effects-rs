//! Runtime constants, static state, and reverse-engineered layout facts.
//!
//! This is intentionally broad for the first lib.rs slimming pass. Split into
//! narrower constants submodules once stable clusters emerge.

use std::sync::{Once, atomic::AtomicUsize};

use eldenring::{
    cs::{ChrAsm, EquipGameData, FaceData, FaceDataBuffer, GameDataMan, GameMan, PlayerGameData},
    dlkr::DLAllocator,
};
use fromsoftware_shared::FromStatic;

pub(crate) const DLL_MAIN_SUCCESS: i32 = 1;
pub(crate) const DIRECTINPUT_FORWARD_UNRESOLVED: usize = 0;
pub(crate) const DIRECTINPUT_FORWARD_ERROR_MOD_NOT_FOUND: i32 = 0x8007_007e_u32 as i32;
pub(crate) const DINPUT8_SYSTEM_DLL: &[u8] = b"C:\\windows\\system32\\dinput8.dll\0";
pub(crate) const DIRECTINPUT8_CREATE_SYMBOL: &[u8] = b"DirectInput8Create\0";
pub(crate) const APPEAR_ANIMATION_ID: i32 = 63010;
/// TimeAct animation IDs at or below this value mark unused/cleared queue
/// slots rather than a real animation.
pub(crate) const INVALID_ANIMATION_ID_FLOOR: i32 = 0;
/// Current local-player TimeAct animation id, or 0 when none/player unavailable. This is the product
/// semaphore for "player animations are going" and is later than bare world/player-present readiness.
pub(crate) use er_telemetry::counters::PLAYER_CURRENT_ANIMATION_ID;
pub(crate) const ANIM_QUEUE_SLOT_STEP: u32 = 1;
pub(crate) const ANIM_QUEUE_SCAN_FLOOR: u32 = 0;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const CUSTOM_CALL_DEFAULT_ID: i32 = 0;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const NEXT_INDEX_OFFSET: usize = 1;
pub(crate) const TITLE_HANDOFF_INCOMPLETE: usize = 0;
pub(crate) const TITLE_HANDOFF_COMPLETE_VALUE: usize = 1;
pub(crate) const STACK_TRACE_FRAME_COUNT: usize = 8;
pub(crate) const STACK_TRACE_FRAMES_TO_SKIP: u32 = 0;
pub(crate) use er_title_flow::NULL_MODULE_BASE;
pub(crate) const HOOK_ORIGINAL_UNSET: usize = 0;
pub(crate) use er_title_flow::HOOK_FALSE_RETURN;

#[repr(usize)]
pub(crate) enum RuntimeGlobalRva {
    NowLoadingSingleton = 0x3d60ec8,
    FakeLoadingScreenSingleton = 0x3d74868,
    CsGraphicsSingleton = 0x3d71c48,
    RendManSingleton = 0x3d7b0c0,
    CsScaleformSingleton = 0x3d83148,
    Fd4IoPool = 0x4853048,
    /// `SaveLoad2::SLSystemImpl*`. Named `Fd4IoWorkerManager` until 2026-08-01, which was
    /// wrong: the 1.16.2 dump shows its lazy initializer `FUN_14240dee0` opens with
    /// `*param_1 = SaveLoad2::SLSystemImpl::vftable`, and all 11 xrefs sit in the SaveLoad2
    /// region (`0x14240a...`, alongside requestLoad). `experiments/own_stepper/
    /// bootstrap_drive.rs` already had it right in a comment. See bd
    /// `rva-4852f88-is-saveload2-slsystemimpl-not-fd4-io-worker-2026-08-01`.
    SaveLoad2SlSystemImpl = 0x4852f88,
    IoDeviceSingleton = 0x4589390,
    DluidInputManager = 0x485dc18,
}

/// Access-violation NTSTATUS (0xC0000005) as the i32 the OS passes to a VEH.
pub(crate) const EXCEPTION_ACCESS_VIOLATION_CODE: u32 = 0xC000_0005;
/// VEH disposition: leave the exception for the game's own handlers.
pub(crate) const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
/// Run our VEH first so it logs before Arxan's handlers consume the exception.
pub(crate) const VECTORED_FIRST_HANDLER: u32 = 1;
/// Cap access-violation log lines so an Arxan exception storm cannot fill disk. Raised 32->256
/// (2026-07-15) so the late 2nd-in-process character-reload AV is not silenced by earlier Arxan
/// first-chance AVs hitting the cap before the real faulting RIP is logged.
pub(crate) const MAX_AV_LOG_LINES: usize = 256;
pub(crate) const AV_LOG_LINE_INCREMENT: usize = 1;
/// NTSTATUS severity field (bits 30-31) and its "error" value. The VEH's catch-all arm logs only
/// ERROR-severity exceptions: that admits the whole crash family (stack overflow, fastfail, heap
/// corruption, illegal instruction, C++/Rust throw) while excluding the codes the process raises as
/// routine control flow -- `DBG_PRINTEXCEPTION_C` (0x40010006), the MSVC thread-name exception
/// (0x406D1388, both severity `informational`) and our own #BP/single-step traps (severity
/// `warning`), which earlier arms of the handler own anyway.
pub(crate) const EXCEPTION_SEVERITY_MASK: u32 = 0xC000_0000;
pub(crate) const EXCEPTION_SEVERITY_ERROR: u32 = 0xC000_0000;
/// The exception codes this DLL's own failures arrive as, none of which were logged before
/// 2026-07-30 because the VEH gated ALL logging on `EXCEPTION_ACCESS_VIOLATION_CODE`.
pub(crate) const EXCEPTION_STACK_OVERFLOW_CODE: u32 = 0xC000_00FD;
pub(crate) const EXCEPTION_ILLEGAL_INSTRUCTION_CODE: u32 = 0xC000_001D;
pub(crate) const EXCEPTION_HEAP_CORRUPTION_CODE: u32 = 0xC000_0374;
pub(crate) const EXCEPTION_FAIL_FAST_CODE: u32 = 0xC000_0409;
pub(crate) const EXCEPTION_CPP_THROW_CODE: u32 = 0xE06D_7363;
pub(crate) const EXCEPTION_IN_PAGE_ERROR_CODE: u32 = 0xC000_0006;
pub(crate) const EXCEPTION_INT_DIVIDE_BY_ZERO_CODE: u32 = 0xC000_0094;
pub(crate) const EXCEPTION_PRIVILEGED_INSTRUCTION_CODE: u32 = 0xC000_0096;
pub(crate) const EXCEPTION_NONCONTINUABLE_CODE: u32 = 0xC000_0025;
/// Dedicated budget for the process-fatal codes, kept separate from the general one so a C++/Rust
/// throw storm can never spend the budget that has to be there for the single stack-overflow line.
pub(crate) const MAX_FATAL_EXCEPTION_LOG_LINES: usize = 4;
/// Shared budget for every other ERROR-severity code (first-chance C++ throws are frequent).
pub(crate) const MAX_OTHER_EXCEPTION_LOG_LINES: usize = 24;
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
pub(crate) use er_telemetry::counters::C30_WATCH_HITS;
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
pub(crate) use er_telemetry::counters::C30_WATCH_FRAME_COUNTER;

// The portrait constants files (portrait_semaphores.rs, portrait_camera.rs,
// portrait_lookat.rs) and several anti_debug.rs/stats_panel_text.rs/gaitem_restore.rs
// blocks moved to the er-loading-portrait crate (portrait crate split); the glob shim
// re-exports them so every remaining flat-namespace reference keeps compiling unchanged.
pub(crate) use er_loading_portrait::*;

include!("constants/software_breakpoints.rs");
include!("constants/anti_debug.rs");
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
include!("constants/render_handoff.rs");
include!("constants/system_quit.rs");
