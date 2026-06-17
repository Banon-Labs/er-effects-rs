#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use debug::InputBlocker;
use eldenring::{
    cs::{CSTaskGroupIndex, CSTaskImp, ChrInsExt, GameMan, PlayerIns},
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoadMethod, SaveLoader};
use fromsoftware_shared::{FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_KEYDOWN,
                WM_KEYUP,
            },
        },
        core::{BOOL, PCSTR},
    },
};

mod crashlog;
mod experiments;
mod ffi;
mod hooks;
mod telemetry;

#[allow(unused_imports)]
use crate::{crashlog::*, experiments::*, ffi::*, hooks::*, telemetry::*};

pub(crate) const DLL_MAIN_SUCCESS: i32 = 1;
pub(crate) const APPEAR_ANIMATION_ID: i32 = 63010;
pub(crate) const OVERLAY_INITIAL_POSITION: [f32; 2] = [24.0, 24.0];
pub(crate) const OVERLAY_INITIAL_SIZE: [f32; 2] = [420.0, 420.0];
/// TimeAct animation IDs at or below this value mark unused/cleared queue
/// slots rather than a real animation.
pub(crate) const INVALID_ANIMATION_ID_FLOOR: i32 = 0;
pub(crate) const ANIM_QUEUE_SLOT_STEP: u32 = 1;
pub(crate) const ANIM_QUEUE_SCAN_FLOOR: u32 = 0;
pub(crate) const CUSTOM_CALL_DEFAULT_ID: i32 = 0;
pub(crate) const NEXT_INDEX_OFFSET: usize = 1;
pub(crate) const TITLE_BOOTSTRAP_UNSEEN: usize = 0;
pub(crate) const TITLE_BOOTSTRAP_SEEN_VALUE: usize = 1;
pub(crate) const STACK_TRACE_FRAME_COUNT: usize = 8;
pub(crate) const STACK_TRACE_FRAMES_TO_SKIP: u32 = 0;
pub(crate) const NULL_MODULE_BASE: usize = 0;
pub(crate) const HOOK_ORIGINAL_UNSET: usize = 0;
pub(crate) const HOOK_FALSE_RETURN: u8 = 0;
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
/// Zero fill for synthetic qword scratch buffers.
pub(crate) const SYNTHETIC_ZERO_QWORD: u64 = 0;
/// FromSoft assert wrapper 0x141eb97a0 (calls the core 0x141eb98d0 which, in the
/// default mode, deliberately crashes via a null write at 0x141eb9999). Hooking
/// it captures the failing assertion's expr/message/file (its rcx/rdx/r8 are
/// .rdata wide-string pointers) before the crash.
pub(crate) const ASSERT_WRAPPER_RVA: usize = 0x1eb97a0;
pub(crate) const MAX_ASSERT_LOG_LINES: usize = 16;
pub(crate) const BOOTSTRAP_TELEMETRY_UNSEEN: usize = 0;
pub(crate) const BOOTSTRAP_TELEMETRY_SEEN_VALUE: usize = 1;
pub(crate) const BOOTSTRAP_EVENT_DLL_MAIN_ATTACH: &str = "dllmain_attach";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_REQUESTED: &str = "continue_trace_thread_requested";
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_REQUESTED: &str = "game_task_thread_requested";
pub(crate) const BOOTSTRAP_EVENT_OVERLAY_SKIPPED_AUTOLOAD: &str = "overlay_skipped_autoload_only";
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_THREAD_STARTED: &str = "game_task_thread_started";
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY: &str = "game_task_instance_ready";
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED: &str =
    "game_task_recurring_registered";
pub(crate) const BOOTSTRAP_EVENT_TELEMETRY_WRITE: &str = "telemetry_write";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_STARTED: &str = "continue_trace_started";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLIED: &str = "continue_trace_applied";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLY_FAILED: &str = "continue_trace_apply_failed";
pub(crate) const BOOTSTRAP_DETAIL_START: &str = "start";
pub(crate) const BOOTSTRAP_DETAIL_DONE: &str = "done";
pub(crate) const BOOTSTRAP_DETAIL_PLAYER_AVAILABLE: &str = "player_available";
pub(crate) const BOOTSTRAP_DETAIL_PLAYER_UNAVAILABLE: &str = "player_unavailable";
pub(crate) const INITIAL_GAME_TASK_TICKS: u64 = 0;
pub(crate) const GAME_TASK_TICK_INCREMENT: u64 = 1;
pub(crate) const SAFE_INPUT_MAX_CONFIRM_PULSES: u32 = 16;
pub(crate) const SAFE_INPUT_DEFAULT_INTERVAL_TICKS: u64 = 30;
pub(crate) const SAFE_INPUT_INITIAL_LAST_PULSE_TICK: u64 = 0;
pub(crate) const SAFE_INPUT_CONFIRM_HOOK_FRAMES: usize = 4;
pub(crate) const SAFE_INPUT_KEY_UP_STATE: i16 = 0;
pub(crate) const VK_RETURN_KEY: usize = 0x0d;
pub(crate) const VK_SPACE_KEY: usize = 0x20;
pub(crate) const KEYDOWN_LPARAM: isize = 1;
pub(crate) const KEYUP_LPARAM: isize = 0xc0000001u32 as isize;
pub(crate) const DIK_RETURN: usize = 0x1c;
pub(crate) const DIK_SPACE: usize = 0x39;
pub(crate) const DIRECT_INPUT_CREATE_DEVICE_VTBL_INDEX: usize = 3;
pub(crate) const DIRECT_INPUT_DEVICE_GET_STATE_VTBL_INDEX: usize = 9;
pub(crate) const HRESULT_SUCCESS_FLOOR: i32 = 0;
pub(crate) const SAFE_INPUT_DIRECT_INPUT_WAIT_TICKS: u64 = 300;
// The TitleStep ctor (0x140b0b1c0) stores this derived vtable to owner+0
// (`lea rax,[0x142b63bb0]; mov [rdi],rax` at 0x140b0b1e5). The previous value
// 0x02b63ba0 was off by 0x10 (the base/parent vtable), so the owner scan never
// matched the live object.
pub(crate) const TITLE_OWNER_VTABLE_RVA: usize = 0x02b63bb0;
pub(crate) const TITLE_OWNER_STATE_OFFSET: usize = 0x4c;
/// Committed/current state the inner-TitleStep dispatcher actually runs (the pump
/// commits +0x4c -> +0x48 each frame and dispatches on +0x48). +0x4c is the
/// requested/next state. Read +0x48 to know the live state.
pub(crate) const TITLE_OWNER_STATE_COMMITTED_OFFSET: usize = 0x48;
/// The inner TitleStep stores a per-instance copy of its state-dispatch table
/// base (0x143d71580) at owner+0x10; the dispatcher reads [owner+0x10]. Requiring
/// this rejects stray .data vtable matches (e.g. the 0x1000ffc58 false positive).
pub(crate) const TITLE_OWNER_INSTANCE_TABLE_OFFSET: usize = 0x10;
pub(crate) const INNER_TITLE_STATE_TABLE_RVA: usize = 0x3d71580;
pub(crate) const TITLE_OWNER_SCAN_ALIGNMENT: usize = 8;
pub(crate) const TITLE_OWNER_SCAN_MAX_ADDRESS: usize = 0x0000_8000_0000_0000;
pub(crate) const TITLE_OWNER_TRACE_LIMIT: usize = 64;
/// How many `title_owner` calls to skip between full-memory owner scans.
///
/// The owner scan walks every committed region via `VirtualQuery`; running it
/// every frame while the owner does not yet exist (or cannot be matched)
/// collapses the game's frame rate. Throttling to roughly once per second at
/// 60 fps keeps a failed lookup from being user-visible.
pub(crate) const TITLE_OWNER_SCAN_CALL_INTERVAL: usize = 60;
pub(crate) const TITLE_OWNER_SCAN_COUNTDOWN_STEP: usize = 1;
pub(crate) const TITLE_OWNER_SCAN_COUNTDOWN_READY: usize = 0;
pub(crate) const TITLE_MENU_JOB_WAIT_RVA: usize = 0x00b0d400;
pub(crate) const TITLE_NATIVE_JOB_MIN_TICK: u64 = 170;
pub(crate) const MEM_COMMIT_NUMERIC: u32 = 0x1000;
pub(crate) const PAGE_NOACCESS_NUMERIC: u32 = 0x01;
pub(crate) const PAGE_GUARD_NUMERIC: u32 = 0x100;
pub(crate) const TRACE_MENU_CONTINUE_WRAPPER_RVA: u32 = 0x0082bac0;
pub(crate) const TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = 0x0082ba80;
pub(crate) const TRACE_MENU_OTHER_LOAD_WRAPPER_RVA: u32 = 0x0082bb00;
pub(crate) const TRACE_MENU_TASK_UPDATE_WRAPPER_RVA: u32 = 0x0082a0f0;
pub(crate) const TRACE_MENU_TASK_UPDATE_TABLE_RVA: u32 = 0x02ac72a0;
pub(crate) const TRACE_TASK_ENQUEUE_RVA: u32 = 0x007a7b60;
pub(crate) const TRACE_UNKNOWN_TABLE_RVA: u32 = 0;
pub(crate) const MENU_TASK_STATE_PAYLOAD_PTR_OFFSET: usize = 0x30;
pub(crate) const MENU_TASK_STATE_DELAY_OFFSET: usize = 0x08;
pub(crate) const TASK_ENQUEUE_TRACE_LIMIT: usize = 256;
pub(crate) const NO_SAFE_INPUT_CONFIRM_FRAMES: usize = 0;
pub(crate) const SAFE_INPUT_CONFIRM_FRAME_DECREMENT: usize = 1;
pub(crate) const SAFE_INPUT_NO_CONFIRM_PULSES: u32 = 0;
pub(crate) const SAFE_INPUT_FIRST_PULSE_INDEX: u32 = 0;
pub(crate) const SAFE_INPUT_NEXT_PULSE_OFFSET: u32 = 1;
pub(crate) const SAFE_INPUT_POST_MAP_MIN_CONFIRM_COUNT: u32 = 5;
pub(crate) const SAFE_INPUT_INITIAL_DELAY_TICKS: u64 = 0;
pub(crate) const WINDOW_PID_UNSET: u32 = 0;
pub(crate) const ENUM_WINDOWS_STOP_NUMERIC: i32 = 0;
pub(crate) const ENUM_WINDOWS_CONTINUE_NUMERIC: i32 = 1;
pub(crate) const DIRECT_INPUT_FAILURE_HRESULT: i32 = -1;
pub(crate) const DIRECT_INPUT_KEY_DOWN_MASK: u8 = 0x80;
pub(crate) const MENU_TRACE_UNSEEN_SEQ: usize = 0;
pub(crate) const POST_MAP_CONTINUATION_STATE_QWORD: usize = 2;
pub(crate) const TITLE_OWNER_SCAN_START_ADDRESS: usize = 0;
pub(crate) const TITLE_OWNER_QUERY_FAILED_BYTES: usize = 0;
pub(crate) const PAGE_PROTECTION_NO_FLAGS: u32 = 0;
pub(crate) const TITLE_OWNER_MIN_STATE: i32 = 0;
pub(crate) const TITLE_OWNER_MAX_STATE: i32 = 11;
pub(crate) const TITLE_NATIVE_JOB_NOT_CALLED: usize = 0;
pub(crate) const TITLE_TRACE_SEQUENCE_INCREMENT: usize = 1;
pub(crate) const TITLE_NATIVE_JOB_TASK_DATA_ZERO: u8 = 0;
pub(crate) const TITLE_NATIVE_JOB_TASK_DATA_BYTES: usize = 16;
pub(crate) const TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR: f32 = 1.0;
pub(crate) const TITLE_NATIVE_JOB_FRAME_RATE: f32 = 60.0;
pub(crate) const TITLE_NATIVE_JOB_DELTA_OFFSET_START: usize = 8;
pub(crate) const TITLE_NATIVE_JOB_DELTA_OFFSET_END: usize = 12;
pub(crate) const TITLE_NATIVE_JOB_CALLED_VALUE: usize = 1;
pub(crate) const TITLE_STEP_BEGIN_TITLE: i32 = 3;
pub(crate) const TITLE_STEP_PLAY_GAME: i32 = 5;
pub(crate) const TITLE_STEP_MENU_JOB_WAIT: i32 = 10;
/// Sentinel logged when the inner TitleStep owner can no longer be found (the
/// title flow advanced past the title and the owner was finalized/destructed).
pub(crate) const TITLE_STATE_OWNER_GONE: i32 = -1;
pub(crate) const FORCE_PLAY_GAME_STATE_UNOBSERVED: i32 = -999;
/// One-shot "PlayGame requested" flag on the TitleStep owner. STEP_PlayGame only
/// runs its real load-trigger (`consume_owner300` 0x140ca89e0 on owner+0x300,
/// gated at 0x140b0d70c) when this byte is nonzero, then clears it. The menu
/// "Continue" selection normally sets it; we set it so the forced PlayGame step
/// actually starts the load instead of resetting via GameStepWait.
pub(crate) const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET: usize = 0x3e1;
pub(crate) const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET: u8 = 1;
/// The save slot STEP_PlayGame actually loads. Its handler (0x140b0d5b0) reads
/// `mov eax,[owner+0xbc]` and feeds it through submit -> validate -> pair, which
/// writes the value to GameMan+0x14 (the load value). The +0xac0 save slot only
/// feeds global+0x1200, not the load pair — so this is the field to select.
pub(crate) const TITLE_OWNER_PLAY_GAME_SLOT_OFFSET: usize = 0xbc;
/// STEP_GameStepWait (handler 0x140b0cde0) waits on the load job at owner+0x2e8:
/// `cmp dword [job+0xd8],0 / jne wait`. Observe job+0xd8 while holding here to
/// learn whether anything drains the job (needs a pump) or it is static.
pub(crate) const TITLE_STEP_GAME_STEP_WAIT: i32 = 6;
pub(crate) const TITLE_OWNER_JOB_OFFSET: usize = 0x2e8;
pub(crate) const TITLE_OWNER_JOB_PENDING_OFFSET: usize = 0xd8;
pub(crate) const TITLE_JOB_OBSERVE_TICK_INTERVAL: u64 = 30;
pub(crate) const FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA: usize = 0x0067a810;
/// Corrected play-game submit recipe (play-game-submit-and-continue-load-recipe-2026):
/// the Continue/Load handler 0x140b0e180 sets owner+0xbc to a PACKED MAP id, clears
/// the new-game flag owner+0x284, and calls SetState 0x140b0d960(owner, 5=PlayGame)
/// -- then the existing pump runs PlayGame -> child MoveMap_Init -> builds CSFeMan.
/// (force_play_game wrote owner+0x4c=5 raw + a raw slot in +0xbc, so it orphaned.)
pub(crate) const TITLE_SET_STATE_RVA: usize = 0xb0d960;
pub(crate) const TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET: usize = 0x284;
/// Packed map id for m60_42_34_00 (the new-game default; resolver 0x14071fd60 packs
/// mAA_BB_CC_DD decimal -> byte3=AA..byte0=DD). A valid map to pass the PlayGame
/// map-area gate (area byte 0x32..0x58) while we prove the SetState(5) path builds
/// CSFeMan; the real slot map comes from GameMan+0xc30 once peeked.
pub(crate) const DEFAULT_PLAY_GAME_MAP: i32 = 0x3c2a2200;
/// Full sync slot deserialize 0x14067b290(ecx=slot) -- CSFeMan-LESS (verified): reads
/// the save, writes the real saved map to GameMan+0xc30, applies the character. The
/// cycle-breaker for slot loading (slot9-load-phase-machine-b80-csfeman-less-2026).
pub(crate) const DESERIALIZE_SLOT_RVA: usize = 0x67b290;
pub(crate) const GAME_MAN_SAVED_MAP_C30_OFFSET: usize = 0xc30;
/// submit_play_game 3-phase states: build CSFeMan -> deserialize slot -> re-submit
/// the real map. Driven one step per game-task tick.
pub(crate) const SUBMIT_PHASE_INIT: i32 = 0;
pub(crate) const SUBMIT_PHASE_BUILT: i32 = 1;
pub(crate) const SUBMIT_PHASE_DESER: i32 = 2;
pub(crate) const SUBMIT_PHASE_DONE: i32 = 3;
/// Phase C world-priming (ingameinit-decoded-world-built-by-movemapstep-msbload-2026):
/// the world singletons are built by the MoveMapStep's OWN step machine (STEP_MsbLoad),
/// driven by per-frame update 0x140aff640(rcx=MoveMapStep, rdx=FD4TaskData). The
/// MoveMapStep is held at InGameStep(owner+0x2e8)+0xe8. Pump it until child+0xd8 drains.
pub(crate) const MOVEMAPSTEP_UPDATE_RVA: usize = 0xaff640;
pub(crate) const INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET: usize = 0xe8;
pub(crate) const INGAMESTEP_PENDING_D8_PENDING: i32 = 1;
/// Native b80 load INITIATOR 0x14067b4e0(ecx=slot): begins the async slot-IO read
/// and sets GameMan+0xb80=1. The scheduler ticks CSTaskGroup 20 (MoveMapStep) every
/// frame (fd4-scheduler-no-group-active-gate-runs-all-2026), so once initiated the
/// b80 machine (dispatcher-1 deserialize + dispatcher-2 apply) + MsbLoad PRIME the
/// world-stream natively -> resident -> child+0xd8 drains. This is the stream-priming
/// step the direct 0x14067b290 deserialize skipped.
pub(crate) const LOAD_INITIATOR_RVA: usize = 0x67b4e0;
/// FULL-LOAD (deserialize-arm) initiator 0x14067b1a0(ecx=slot): begins the slot read and
/// sets GameMan+0xb80=2 (the b80==2 deserialize arm), NOT b80=1 (the preview lane that
/// 0x14067b4e0 uses and that resets to 0 without deserializing). Runtime-proven the
/// preview lane never reaches b80==3; the b80=2 arm is the one the poll 0x140679180
/// advances 2->3 (resident) so the full deserialize 0x14067b290 can run.
pub(crate) const B80_FULL_LOAD_INITIATOR_RVA: usize = 0x67b1a0;
/// The MENU's STEP_LoadSaveData initiator 0x14067b200(ecx=slot): sets GameMan+0xb80=2
/// (the deserialize arm) the way the real Load-Game list does. Distinct from the
/// preview 0x67b4e0 (b80=1) and the 0x67b1a0 variant. Hooked for the b80-mount capture
/// to pin which initiator the real .co2 load fires and in what order.
pub(crate) const B80_LOAD_SAVE_DATA_INITIATOR_RVA: usize = 0x67b200;
/// The save-header parser 0x14067bd70(rcx=GameMan, rdx=buf, r8d=size) -- the SOLE
/// writer of GameMan+0xc30 (3 callers). Hooked with a caller stack so a real load
/// reveals WHICH deserializer set the saved map (vanilla 0x67b290/0x67e150 chain or,
/// if it never fires under Seamless, ERSC writing c30 from its own module).
pub(crate) const C30_WRITER_RVA: usize = 0x67bd70;
/// World-resource streaming lever (worldres-loadstate-creator-and-streaming-enable-
/// gate-2026). Gap 1: the block-load request is built from the InGameStep target
/// coord [InGameStep+0x100]; set it to slot 9's real map then re-submit via
/// 0x140aed820 so the builder creates the m10 load-states. Gap 2: the resmgr
/// ([InGameStep+0x250]) streaming-enable flag [resmgr+0xb7c1]==0; the virtual
/// enabler 0x14066e2e4 sets it + builds the session singletons + starts the IO jobs.
pub(crate) const INGAMESTEP_TARGET_COORD_100_OFFSET: usize = 0x100;
pub(crate) const INGAMESTEP_RESMGR_250_OFFSET: usize = 0x250;
pub(crate) const REQUEST_SUBMIT_RVA: usize = 0xaed820;
pub(crate) const STREAMING_ENABLE_RVA: usize = 0x66e2e4;
/// Direct poke of the streaming-enable flag [resmgr+0xb7c1]=1 (the virtual enabler
/// 0x14066e2e4 crashes -- wrong receiver). The virtual also builds session singletons
/// 0x143d687a0 / 0x143d67bd0; read them to see if the poke is safe (already built) or
/// if the job machine will deref null.
pub(crate) const RESMGR_STREAM_ENABLE_B7C1_OFFSET: usize = 0xb7c1;
pub(crate) const SESSION_SINGLETON_A_RVA: usize = 0x3d687a0;
pub(crate) const SESSION_SINGLETON_B_RVA: usize = 0x3d67bd0;
/// Corrected streaming-enable (worldres-enable-0x14066e2e4-decoded-receiver-and-
/// driver-singleton-2026): the CORRECT resmgr is deref(deref(MoveMapStep+0xf0)+0x10)
/// with vtable 0x142a7e030 (NOT InGameStep+0x250, which is the WorldRes-owner, vtable
/// 0x142a7de60 -- the wrong object that crashed). The hard floor is the streaming/
/// session driver singleton 0x143d7c088 (job machine asserts if null); build it via
/// the lazy getter 0x140cd6c50 before calling enable 0x14066e2e4(resmgr).
pub(crate) const RESMGR_EXPECTED_VTABLE_RVA: usize = 0x2a7e030;
pub(crate) const STREAMING_DRIVER_SINGLETON_RVA: usize = 0x3d7c088;
pub(crate) const STREAMING_DRIVER_BUILDER_RVA: usize = 0xcd6c50;
/// World-stream worker build+register: IngameInit's SetState tail 0x140b0a980, whose
/// `[this+0x48] >= 7` arm constructs the world-stream worker 0x144842d40 (ctor
/// 0x141eceb10) and registers it with the FD4 scheduler (key 0x59682f01 via
/// 0x142656b00) -- the piece our forced path skips (b80-initiate-advances-mms-but-
/// async-io-stalls). The arm uses ONLY globals/stack after the +0x48 check, so calling
/// it with a synthetic `this` (a zeroed buffer with +0x48=7) replicates the build
/// without needing the real 0x143d71340 step object.
pub(crate) const WORLD_WORKER_BUILD_RVA: usize = 0xb0a980;
pub(crate) const SYNTHETIC_STEP_THIS_SIZE: usize = 0x60;
pub(crate) const SYNTHETIC_STEP_STATE_OFFSET: usize = 0x48;
pub(crate) const WORLD_WORKER_BUILD_STATE: i32 = 7;
/// The world-stream worker singleton 0x144842d40 (built by the arm above). Reading it
/// non-null verifies the build+register fired.
pub(crate) const WORLD_STREAM_WORKER_RVA: usize = 0x4842d40;
/// World/scene singletons built by MoveMapStep::STEP_MsbLoad 0x140af8f00. Non-null
/// == MsbLoad ran (the IsResident-relevant world exists). Diagnostic for whether the
/// worker is servicing the stream vs the b80 lane stalling first.
pub(crate) const WORLD_SINGLETON_A_RVA: usize = 0x3d691d8;
pub(crate) const WORLD_SINGLETON_B_RVA: usize = 0x3d69ba8;
/// World-resource manager chain for STEP_WorldResWait residency (0x14066d3e0):
/// resmgr = [[MoveMapStep+0xf0]+0x10]; loaded-block count = [resmgr+0xb3140].
/// count==0 -> no map-block registered (setup gap); count>0 but block not at load
/// phase 0xa -> streaming gap. Diagnostic for the final wall.
pub(crate) const MOVEMAPSTEP_WORLDRES_F0_OFFSET: usize = 0xf0;
pub(crate) const WORLDRES_RESMGR_10_OFFSET: usize = 0x10;
pub(crate) const RESMGR_BLOCK_COUNT_B3140_OFFSET: usize = 0xb3140;
pub(crate) const DIAG_NULL_CHAIN: i32 = -2;
/// The block coord/map-id the MoveMapStep requests in STEP_WorldResWait: at
/// [[MoveMapStep+0xf0]+0x2c] (0x140624bd0 reads byte3 as the target area). byte3 ==
/// 0x0a means slot 9's m10 IS being requested (loader/streaming issue); 0 means the
/// saved world position never loaded (coord issue).
pub(crate) const WORLDRES_COORD_2C_OFFSET: usize = 0x2c;
/// Resource-manager block array scan (mirrors 0x14066d3e0): entries at
/// [resmgr+0xb3030 + i*8]; each entry's block area = [[entry+0x8]+0xc]. We scan for
/// the target area 0x0a (m10) to learn if slot 9's block is registered (streaming
/// gap) or absent (loader never picks up the request).
pub(crate) const WORLDRES_BLOCK_ARRAY_B3030_OFFSET: usize = 0xb3030;
pub(crate) const BLOCK_ENTRY_AREAOBJ_8_OFFSET: usize = 0x8;
pub(crate) const BLOCK_AREAOBJ_AREA_C_OFFSET: usize = 0xc;
pub(crate) const TARGET_AREA_M10: i32 = 0x0a;
pub(crate) const BLOCK_SCAN_MAX: i32 = 64;
pub(crate) const BLOCK_ENTRY_STRIDE: usize = 8;
pub(crate) const BLOCK_SAMPLE_COUNT: usize = 4;
pub(crate) const BLOCK_AREA_BYTE_MASK: u32 = 0xff;
pub(crate) const BLOCK_SAMPLE_SHIFT: u32 = 8;
/// m10 block load-state (mirrors 0x14066d3e0 readiness tail): loadstate =
/// entry->vtable[+0x10](entry); ready iff [loadstate+0x2d]!=0 AND [loadstate+0x35]==0xa.
/// Reading [+0x35] live shows which load phase the m10 block is stuck at (<0xa).
pub(crate) const BLOCK_LOADSTATE_GETTER_VT_10_OFFSET: usize = 0x10;
pub(crate) const BLOCK_LOADSTATE_FLAG_2D_OFFSET: usize = 0x2d;
pub(crate) const BLOCK_LOADSTATE_PHASE_35_OFFSET: usize = 0x35;
pub(crate) const DIAG_PHASE_NONE: i32 = -1;
pub(crate) const DIAG_COUNT_ZERO: i32 = 0;
pub(crate) const DIAG_COUNT_ONE: i32 = 1;
pub(crate) const DIAG_SAMPLE_ZERO: u32 = 0;
/// Global holding the GameMan pointer (`mov rax,[rip]` in set_save_slot 0x67a810
/// / save_slot_get 0x678ca0). Read-only diagnostics of the PlayGame load-pair
/// preconditions read GameMan through this.
pub(crate) const FORCE_PLAY_GAME_GAME_MAN_GLOBAL_RVA: usize = 0x3d69918;
pub(crate) const FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET: usize = 0xac0;
/// Save-manager load-in-progress flag (GameMan/save-mgr singleton 0x143d69918):
/// `0x14067b570` sets `[mgr+0xb80]=1` when it begins the load and clears it to 0
/// when finished. The native autoload (recipe A) arms the load by setting the
/// slot (`+0xac0`) and the force flag `0x143d856a0`, then the save-manager
/// per-frame update `0x14067f5d0` performs it.
pub(crate) const GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET: usize = 0xb80;
/// Read-only autoload-arm precondition probe. The native save-mgr update
/// 0x14067f5d0 arms autoload (sets GameMan+0xb72=1 -> load) only when its gates
/// pass; the one runtime unknown is whether the slot-record container
/// [slotmgr+0x8] is populated at the pre-bootstrap title. These RVAs/offsets let
/// us read those preconditions without touching state.
pub(crate) const SLOT_MANAGER_RVA: usize = 0x3d5df38;
pub(crate) const SLOT_MANAGER_DATA_OFFSET: usize = 0x8;
pub(crate) const SLOT_MANAGER_CONTAINER_OFFSET: usize = 0x78;
pub(crate) const CSFEMAN_SINGLETON_RVA: usize = 0x3d6b880;
/// Session manager singleton (absolute 0x1447ef360; NULL at the title, built by
/// the move-map/load path). RVA = 0x1447ef360 - 0x140000000 = 0x47ef360.
pub(crate) const SESSION_SINGLETON_RVA: usize = 0x47ef360;
pub(crate) const TITLE_INPUT_MANAGER_RVA: usize = 0x3d6b7b0;
/// Pure-observe snapshot interval (game-task ticks). Logs the title->menu->load state
/// every N ticks with NO forcing, to capture what the REAL button press does.
pub(crate) const OBSERVE_INTERVAL: u64 = 10;
/// Observe change-detection: log a snapshot only when the packed signature changes
/// (full granularity, minimal file I/O). Multiplier for the rolling signature.
pub(crate) const OBSERVE_SIG_MULT: i64 = 0x100000001b3;
pub(crate) static OBSERVE_LAST_SIG: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);
/// OWN-THE-STEPPER (own-stepper-control-verified-and-driver-call-2026): the
/// SimpleTitleStep step-fn table (base abs 0x143d71580, owner+0x10) is in WRITABLE
/// .data. idx10 = STEP_MenuJobWait func slot = base + 10*0x10 = abs 0x143d71620
/// (RVA 0x3d71620) is dispatched every frame at the press-any-button title. We patch
/// it to our own handler so the FD4 scheduler runs OUR code IN-CONTEXT (rcx=owner,
/// rdx=FD4Time), instead of trampolining from an external CSTask.
pub(crate) const TITLE_STEP_IDX10_SLOT_RVA: usize = 0x3d71620;
/// Native Continue/Load confirm handler (reads owner=[rcx+8]; slot-select + child
/// request + SetState(5)). Invoked via a {[+8]=owner} shim.
pub(crate) const CONTINUE_CONFIRM_RVA: usize = 0xb0e180;
pub(crate) const OWN_STEPPER_LOG_INTERVAL: u64 = 60;
pub(crate) const OWN_STEPPER_CALL_INC: usize = 1;
/// Driver phases for the in-context idx10 handler.
pub(crate) const OWN_STEPPER_PHASE_MENU: usize = 0;
pub(crate) const OWN_STEPPER_PHASE_CONTINUE: usize = 1;
pub(crate) const OWN_STEPPER_PHASE_DONE: usize = 2;
/// PHASE 3 (MOUNT): mount the slot at state 10 BEFORE SetState(5) -- the only place the
/// MoveMapStep dispatcher (which resets b80 via its b80==1 lane) is NOT running, so our
/// own b80 poll can drive the save-IO machine 1->2->3 cleanly (minimal-save-mount-
/// primitive-recipe-2026). Register the FD4 stream worker (0x140b0a980 stub), initiate
/// the slot read (0x14067b4e0 -> b80=1), poll 0x140679180 until b80==3, then full
/// deserialize 0x14067b290 (c30 = real map + character applied), then SetState(5).
pub(crate) const OWN_STEPPER_PHASE_MOUNT: usize = 3;
/// b80 save-IO poll/driver 0x140679180(0,0): advances GameMan+0xb80 toward 3 (resident)
/// as the stream worker drains the async slot read; sets b80=3 when the IO request state
/// (0x14240a1f0) is resident. We call it ourselves each frame at state 10.
pub(crate) const B80_POLL_RVA: usize = 0x679180;
/// Both fastcall args (cl, dl) to the b80 poll 0x140679180 are 0 in the native menu
/// drive (matches the captured real-load poll calls poll(0,0)).
pub(crate) const B80_POLL_ARG_ZERO: u8 = 0;
/// b80==1 PREVIEW-lane driver 0x140679510: per-frame IO tick of the preview read started by
/// 0x14067b4e0; resets GameMan+0xb80 1->0 when the iodev request goes resident. NOT a
/// dispatcher (no CSFeMan apply / no save write) -- just the lane tick the menu runs via
/// dispatcher-1. We call it ourselves to drain the preview read to resident.
pub(crate) const B80_LANE1_DRIVER_RVA: usize = 0x679510;
/// Max frames to poll b80 toward 3 before giving up the mount (avoid an infinite title
/// hang if the worker never drains). ~10s at 60fps.
pub(crate) const OWN_STEPPER_MOUNT_POLL_MAX: u64 = 600;
pub(crate) static OWN_STEPPER_MOUNT_POLLS: AtomicUsize = AtomicUsize::new(0);
/// PHASE 4 (DRIVE): the validated dispatcher-driven mount (real-load-c30-mount-write-
/// confirmed-seamless-2026 + menu-b80-mount-orchestration-sequence-2026). Runtime + the
/// real-load capture proved hand-calling a single b80 initiator never converges (the
/// two initiators target different queues; only 0x14067b4e0's preview read populates the
/// iodev request handle the poll reads, and the char-applying deserialize runs ONLY from
/// dispatcher-1's b80==2 arm). So: start the preview read (0x14067b4e0), set the b78
/// route slot, then each frame call the two MoveMapStep b80 dispatchers on a SYNTHETIC
/// owner -- dispatcher-1 0x140afbad0 ticks the b80==1 preview lane to resident then
/// (b80==2 arm) polls -> b80=3 -> deserialize 0x14067b290 (c30=real map + char applied +
/// ac0=slot); dispatcher-2 0x140afb880 (b78-route) transitions b80 0->2 + owner+0x12c=slot.
/// The dispatchers self-sequence the 1->0->2->3 dance the menu does. Success = ac0==slot.
pub(crate) const OWN_STEPPER_PHASE_DRIVE: usize = 4;
/// GameMan+0xc30 unset sentinel (0xffffffff as i32). At the bare press-any-button title
/// (BeginTitle skipped) c30 is unset; the full deserialize 0x14067b290 is the ONLY thing
/// that writes it to the slot's real saved map during the mount, so c30 != UNSET is the
/// genuine "the character was deserialized" signal (ac0 is NOT -- set_save_slot pre-sets it).
pub(crate) const GAME_MAN_C30_UNSET: i32 = -1;
/// MoveMapStep b80 dispatchers (called per-frame from the MoveMapStep update 0x140aff640
/// in native order: dispatcher-1 then dispatcher-2). Neither derefs the owner vtable;
/// both read only GameMan globals + the owner's deserialize-tracking fields (+0x12a skip-
/// apply, +0x12c slot, +0x130 result), so a zeroed synthetic owner >=0x138 bytes drives
/// them. dispatcher-1 = deserialize arm; dispatcher-2 = initiate (b78-route).
pub(crate) const B80_DISPATCHER1_RVA: usize = 0xafbad0;
pub(crate) const B80_DISPATCHER2_RVA: usize = 0xafb880;
/// Synthetic MoveMapStep `owner` for the b80 dispatchers. Zeroed; +0x12a=1 forces the
/// CSFeMan-apply arms (dispatcher-1 b80==1 idx1, dispatcher-2 @0x140afb9e4) to be SKIPPED
/// (they are gated owner+0x12a==0 and would deref the null-at-title CSFeMan 0x143d6b880);
/// +0x12c carries the deserialize slot. Size >0x138 (the highest field touched is +0x130).
pub(crate) const SYNTH_MMS_OWNER_SIZE: usize = 0x140;
pub(crate) const SYNTH_MMS_SKIP_APPLY_12A_OFFSET: usize = 0x12a;
pub(crate) const SYNTH_MMS_DESER_SLOT_12C_OFFSET: usize = 0x12c;
pub(crate) const SYNTH_MMS_SKIP_APPLY_ON: u8 = 1;
pub(crate) static mut SYNTH_MMS_OWNER: [u8; SYNTH_MMS_OWNER_SIZE] =
    [MOVIE_SKIP_FLAG_CLEAR; SYNTH_MMS_OWNER_SIZE];
/// Max frames to drive the dispatchers before giving up (stay at title, no save write).
pub(crate) const OWN_STEPPER_DRIVE_MAX: u64 = 600;
pub(crate) static OWN_STEPPER_DRIVE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// PHASE 5 (MENU_BUILD): the parked press-any-button title is the FIRST state 10 and has
/// NOT run STEP_BeginTitle(3) yet, so the Continue/Load-Game items do not exist at
/// owner+0x138 until we drive 10->3 zero-input. idx10 SetState(owner,3) builds the main
/// menu (BeginTitle needs no session, writes NO save), then this phase waits for the menu
/// to populate and walks owner+0x138 to identify the Load-Game leaf (its +0xa8 action
/// functor's _Do_call chain resolves to dialog_factory 0x14081ead0). Max state reached =
/// main menu (no PlayGame) -> save-safe.
pub(crate) const OWN_STEPPER_PHASE_MENU_BUILD: usize = 5;
/// Max idx10 re-entries to wait for the main menu to build before giving up (stay at the
/// title, no save write). ~3s at 60fps.
pub(crate) const OWN_STEPPER_MENU_BUILD_WAIT_MAX: u64 = 180;
pub(crate) static OWN_STEPPER_MENU_BUILD_WAITS: AtomicUsize = AtomicUsize::new(0);
/// How many in-context idx10 calls to wait before driving (let the boot settle to the
/// stable press-any-button state 10 first).
pub(crate) const OWN_STEPPER_SETTLE_CALLS: u64 = 30;
/// Shim callback object for the native Continue confirm 0x140b0e180 (reads
/// owner=[shim+8]). Persistent (not stack) so the call cannot read freed memory.
pub(crate) const OWN_STEPPER_SHIM_LEN: usize = 8;
pub(crate) const OWN_STEPPER_SHIM_OWNER_IDX: usize = 1;
/// idx6 = STEP_GameStepWait func slot = table base + 6*0x10 = abs 0x143d715e0 (RVA
/// 0x3d715e0). We own it too, to drive the 3-phase load after the MoveMapStep builds.
pub(crate) const TITLE_STEP_IDX6_SLOT_RVA: usize = 0x3d715e0;
pub(crate) static OWN_STEPPER_ORIG_IDX6: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OWN_STEPPER_IDX6_CALLS: AtomicUsize = AtomicUsize::new(0);
/// Path A re-target single-shot latch: 0 = the native b78-route deserialize has not yet
/// landed a real GameMan+0xc30, 1 = idx6 has already re-targeted owner+0xbc to the real
/// map + SetState(5). Prevents re-firing the re-target every frame.
pub(crate) static OWN_STEPPER_RETARGETED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const OWN_STEPPER_RETARGET_NO: usize = 0;
pub(crate) const OWN_STEPPER_RETARGET_YES: usize = 1;
/// GameMan+0xb80 load-phase value meaning the save IO is resident (mounted).
pub(crate) const OWN_STEPPER_B80_RESIDENT: i32 = 3;
/// GameMan+0xb80 == 1: the PREVIEW lane (0x14067b4e0 read in flight); drive the lane tick
/// 0x140679510 to drain it to resident (which resets b80 -> 0).
pub(crate) const OWN_STEPPER_B80_PREVIEW_LANE: i32 = 1;
/// GameMan+0xb80 == 0: idle/drained; fire the LoadSaveData initiator 0x14067b200 -> b80=2
/// (reusing the resident iodev request the preview started).
pub(crate) const OWN_STEPPER_B80_IDLE: i32 = 0;
/// idx6 calls to wait (MoveMapStep settle) before deserializing the real slot.
pub(crate) const OWN_STEPPER_IDX6_SETTLE: u64 = 120;
pub(crate) const OWN_STEPPER_SLOT_NONE: i32 = -1;
/// Save slot to load (parsed from the trigger file "slot=N"; -1 => leave the game's
/// own most-recent selection).
pub(crate) static OWN_STEPPER_SLOT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(OWN_STEPPER_SLOT_NONE);
pub(crate) static OWN_STEPPER_PHASE: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PHASE_MENU);
pub(crate) static mut OWN_STEPPER_SHIM: [usize; OWN_STEPPER_SHIM_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; OWN_STEPPER_SHIM_LEN];
/// Synthetic `this` for the IngameInit-tail stream-worker register call 0x140b0a980
/// (+0x48 set to WORLD_WORKER_BUILD_STATE hits the build+register arm).
pub(crate) static mut OWN_STEPPER_WORKER_THIS: [u8; SYNTHETIC_STEP_THIS_SIZE] =
    [MOVIE_SKIP_FLAG_CLEAR; SYNTHETIC_STEP_THIS_SIZE];
pub(crate) const OWN_STEPPER_PATCHED_NO: usize = 0;
pub(crate) const OWN_STEPPER_PATCHED_YES: usize = 1;
/// Original idx10 func ptr (STEP_MenuJobWait), saved so our handler can pass through.
pub(crate) static OWN_STEPPER_ORIG_IDX10: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OWN_STEPPER_BASE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OWN_STEPPER_PATCHED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PATCHED_NO);
pub(crate) static OWN_STEPPER_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) const GAME_MAN_ARM_FLAG_B72_OFFSET: usize = 0xb72;
pub(crate) const GAME_MAN_FLAG_B73_PROBE_OFFSET: usize = 0xb73;
pub(crate) const GAME_MAN_FLAG_B75_PROBE_OFFSET: usize = 0xb75;
pub(crate) const GAME_MAN_REQUESTED_SLOT_B78_OFFSET: usize = 0xb78;
pub(crate) const GAME_MAN_FLAG_BC4_OFFSET: usize = 0xbc4;
/// Submit-gate diagnostics (b80-submit-kick-exact-false-gate-decoded-2026). The b72
/// autoload initiator 0x14067b750 sets GameMan+0xb80=1 ONLY if the async submit
/// 0x140e6ec70 returns true; the submit body 0x140e6f940 bails FALSE if the IO device
/// has a STALE request in-flight ([iodev+0x10]!=0) or a stale request handle
/// ([iodev+0x20]!=0). The IO device global is abs 0x144589390 (RVA 0x4589390); we read
/// it both as a possible pointer-to-device and as a struct base so the log
/// disambiguates. Also: the b72 effective-getter 0x1406793d0 zeroes b72 if
/// [GameMan+0xbc4]==3 or [inputmgr+0x13c]!=0, so log those too.
pub(crate) const IODEV_GLOBAL_RVA: usize = 0x4589390;
pub(crate) const IODEV_INFLIGHT_10_OFFSET: usize = 0x10;
/// The async-IO request handle the poll 0x140e6e080 actually reads is the PAIR
/// [iodev+0x18] && [iodev+0x20] (a *started* request). 0x14067b4e0's preview read
/// (0x140e6ec80) is what populates these; 0x14067b200's queue (0x140e6eb80) goes to
/// the file-device-mgr instead, so it never appears here. Logging both pins which
/// initiator actually started the iodev read (menu-b80-mount-orchestration-sequence).
pub(crate) const IODEV_REQHANDLE_18_OFFSET: usize = 0x18;
pub(crate) const IODEV_REQHANDLE_20_OFFSET: usize = 0x20;
pub(crate) const INPUTMGR_PENDING_13C_OFFSET: usize = 0x13c;
pub(crate) const ARM_PROBE_MIN_TICK: u64 = 60;
pub(crate) const ARM_PROBE_TICK_INTERVAL: u64 = 30;
/// Lever 2 (zero-input title-accept via input-event injection). Inner TitleStep
/// state is at owner+0x4c (==10 MenuJobWait); the press-any-button job is at
/// owner+0x130; its vtable[+0x18] fills a descriptor whose first i32 indexes the
/// event table 0x143d6a860 (stride 0x60); eventId=[entry+4], value=[entry+8];
/// the game's node update writes inputmgr(0x143d6b7b0)+0xdc+eventId*4 = value.
/// Injecting that event makes the game's own node update accept and run the real
/// front-end bootstrap. Verdict is [job+0x1e8] >= 2.
/// The press-any-button job (owner+0x130) is an AND-combiner (vtable RVA
/// 0x2aa2958) over child condition nodes at [job+0x18 + i*8], count [job+0x60].
/// The real input node is the child with vtable RVA 0x2aa97e8; its keycode is at
/// child+0x180. Accept = set the inputmgr keystate bitmap (inputmgr+0x90+keycode
/// |= 3 pressed+triggered) so the leaf returns accepted and the combiner ANDs to
/// done -> MenuJobWait advances 10->11 and the front-end bootstraps.
/// Logical input-event array on the inputmgr (inputmgr+0xdc, i32 per event id,
/// ids 0..=0x15e). The leaf input node detects a press via this layer (then
/// mirrors into the keystate bitmap), so injecting here is what actually accepts.
pub(crate) const TITLE_ACCEPT_LATCH_RVA: usize = 0x3d856a0;
/// Boot intro/movie singleton (ptr) and its decoder skip-flag byte. The latch
/// 0x143d856a0 is set by the intro thread 0x140c8fe90 only after its movie-wait
/// loop ends; the movie-dismiss gate 0x140e90820 finishes on decode-complete or
/// when the skip-flag byte 0x14458b8a5 is non-zero (sole non-WNDPROC effect is the
/// movie's own stop). Setting the skip-flag drives a genuine zero-input dismiss.
pub(crate) const MOVIE_SINGLETON_RVA: usize = 0x458b890;
pub(crate) const MOVIE_SKIP_FLAG_RVA: usize = 0x458b8a5;
pub(crate) const MOVIE_SKIP_FLAG_CLEAR: u8 = 0;
pub(crate) const MOVIE_SKIP_FLAG_SET: u8 = 1;
/// Movie controller vtable RVA (0x142bfe088), HWND field offset (M+8), and the
/// USER32 constants for mirroring the WNDPROC WM_CLOSE teardown.
pub(crate) const MOVIE_VTABLE_RVA: usize = 0x2bfe088;
pub(crate) const MOVIE_HWND_OFFSET: usize = 0x8;
pub(crate) const WND_SC_CLOSE: u32 = 0xf060;
pub(crate) const WND_MF_BYCOMMAND: u32 = 0;
pub(crate) const WND_SW_HIDE: i32 = 0;
pub(crate) const WND_GET_SYSTEM_MENU_KEEP: i32 = 0;
/// Render-thread liveness probe logging cadence (in render frames).
pub(crate) const RENDER_PROBE_INTERVAL: usize = 120;
/// Splash-skip static patch (ports chozandrias76/er-skip-splash-screens to 1.16.1):
/// inside STEP_BeginLogo 0x140b0c2a0, the branch `cmp [rdi+0xb8],0; je 0x140b0c3b2`
/// at RVA 0xb0c35d plays the logo when the byte is 0; flipping je(0x74)->jg(0x7f)
/// falls through to the SetState(state 3) advance instead, skipping the logo via
/// the game's own flow. Applied early (DLL attach) before the title runs state 2.
pub(crate) const SPLASH_SKIP_RVA: usize = 0xb0c35d;
pub(crate) const SPLASH_SKIP_EXPECTED_JE: u8 = 0x74;
pub(crate) const SPLASH_SKIP_REPLACEMENT_JG: u8 = 0x7f;
pub(crate) const SPLASH_PATCH_LEN: usize = 1;
pub(crate) const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub(crate) const PAGE_PROTECT_UNSET: u32 = 0;
/// Earliest game-task tick to fire the movie dismiss -- a settle floor; the real
/// gate is the movie singleton being present with the expected vtable. Kept modest
/// so the dismiss reliably fires within the runtime window.
pub(crate) const DISMISS_MIN_TICK: u64 = 120;
/// Generous upper bound on the game image span, to sanity-check that a candidate
/// object's vtable points into the module before dereferencing deeper.
/// Sentinel logged when GameMan is null so the field could not be read.
pub(crate) const ARM_PROBE_FIELD_ABSENT: i64 = -1;
/// IngameInit drive (recipe B, flagless). The SimpleTitleStep container that
/// bears IngameInit is compiled-in but NEVER instantiated in this build, so we
/// call IngameInit (its state-2 handler) with a SYNTHETIC `this`: it only reads
/// +0xc0 (the InGameStep) and +0x130 (the map -- != -1 = continue, -1 = new
/// game), primes the world subsystems, and SetupLoad-submits the load. Never
/// touches the force flag 0x143d856a0. The map id is produced by the same parser
/// (0x71fd60) over the default map string the new-game path uses.
pub(crate) const OUTER_STEP_INGAMESTEP_OFFSET: usize = 0xc0;
pub(crate) const OUTER_STEP_MAP_OVERRIDE_130_OFFSET: usize = 0x130;
pub(crate) const INGAMEINIT_HANDLER_RVA: usize = 0xb0a1f0;
pub(crate) const INGAMEINIT_MAP_PARSER_RVA: usize = 0x71fd60;
pub(crate) const DEFAULT_MAP_STRING_RVA: usize = 0x2b62c70;
pub(crate) const INGAMEINIT_SYNTHETIC_QWORDS: usize = 0x40;
/// Genuine offline continue drive (recipe Option 1). The MoveMapList save-load
/// dispatcher 0x140afb880 (clean entry; its Arxan-scrambled body cross-jumps to
/// the offline-continue deserialize 0x14067b290 at 0x140afbc3e). With GameMan
/// b73 set it selects current_slot_load 0x67b570 (begin), then drives the async
/// task (GameMan+0xb80 1->2->3) and synchronously deserializes the REAL slot
/// character, also building the world singletons. owner is rbx; owner+0x12c =
/// slot. Done when GameMan+0x10 == 1. Never writes 0x143d856a0.
pub(crate) const MOVEMAP_DISPATCHER_RVA: usize = 0xafb880;
pub(crate) const GAME_MAN_B73_FLAG_OFFSET: usize = 0xb73;
pub(crate) const GAME_MAN_B73_FLAG_SET: u8 = 1;
pub(crate) const GAME_MAN_REAL_LOAD_DONE_OFFSET: usize = 0x10;
pub(crate) const GAME_MAN_REAL_LOAD_DONE_VALUE: i32 = 1;
pub(crate) const CONTINUE_OWNER_SLOT_OFFSET: usize = 0x12c;
pub(crate) const CONTINUE_OWNER_FLAG_12A_OFFSET: usize = 0x12a;
pub(crate) const CONTINUE_OWNER_FLAG_12A_VALUE: u8 = 0;
pub(crate) const CONTINUE_OWNER_QWORDS: usize = 0x40;
pub(crate) const CONTINUE_DRIVE_MIN_TICK: u64 = 120;
pub(crate) const FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET: usize = 0x14;
pub(crate) const FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET: usize = 0xb28;
pub(crate) const FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET: usize = 0x12d;
pub(crate) const FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET: usize = 0x12e;
/// SelectBot selection-injection lane (runs 300/301 static decode). The
/// SimpleTitleStep MenuLoop pump 0xb0a5e0 parses a serialized SelectBot stream
/// keyed by "CSEzSelectBot.MoveMapListStep" into owner+0x130 (parsed selection)
/// and submits a task onto owner+0x128 (title queue). The stream data lives in
/// the registry object pointed to by global [0x143d87360]. The pump's direct
/// PlayGame trigger 0xb0a78b is gated by byte [0x143d856a0] (load-active, which
/// the sole writer 0x140c8fe90 sets downstream of the load). This read-only
/// probe samples those fields to confirm the registry is live and the pump idles
/// with an empty stream before any write is attempted.
pub(crate) const SELECTBOT_OWNER_TITLE_QUEUE_128_OFFSET: usize = 0x128;
pub(crate) const SELECTBOT_OWNER_PARSED_SELECTION_130_OFFSET: usize = 0x130;
pub(crate) const SELECTBOT_REGISTRY_GLOBAL_RVA: usize = 0x3d87360;
pub(crate) const SELECTBOT_LOAD_GATE_RVA: usize = 0x3d856a0;
/// The MenuLoop pump 0xb0a5e0 sets `[input_manager+0x6b0]=1` near its entry
/// (`mov rax,[0x143d6b7b0]; mov byte [rax+0x6b0],1` at 0xb0a64d) every frame it
/// executes. Sampling this byte tells us whether the outer SimpleTitleStep is
/// actually running MenuLoop at the title idle (so SelectBot injection would be
/// parsed) or is still parked before it (so injection alone would be a no-op
/// until the title-accept advances the outer state).
pub(crate) const SELECTBOT_INPUT_MANAGER_GLOBAL_RVA: usize = 0x3d6b7b0;
pub(crate) const SELECTBOT_PUMP_RAN_FLAG_OFFSET: usize = 0x6b0;
/// Lever-1 title-accept experiment (runs 304+). Static RE (bd
/// `title-accept-lever-143d856a0`) shows inner MenuJobWait (state 10, 0xb0d400)
/// advances to state 11 (Finish) iff the global byte `[0x143d856a0]` (==
/// `SELECTBOT_LOAD_GATE_RVA`) is non-zero — it is the title-accept/"proceed"
/// latch, not a load-downstream flag. We set it ONCE, only while the inner owner
/// is confirmed at MenuJobWait, to drive the native title-accept with zero input,
/// then keep sampling to observe the cascade.
pub(crate) const TITLE_STEP_MENU_JOB_WAIT_STATE: i32 = 10;
pub(crate) const TITLE_PROCEED_GATE_SET_VALUE: u8 = 1;
/// InGameStep manual-tick experiment (lever / "direct drive the load"). The
/// load job at `owner+0x2e8` is a `CS::InGameStep` whose step machine only
/// advances while its FD4StepTemplate::Execute pump (`0x140b0bd60`) is ticked
/// each frame. `force_play_game` submits the load (`job+0xd8=1`) but never ticks
/// the step, so it orphans. The engine already calls `0x140b0bd60` every frame
/// on the inner TitleStep, so we DETOUR it and, when it fires for the inner
/// TitleStep at GameStepWait, also call the original on the InGameStep with the
/// SAME live ctx — reusing the engine's real per-frame context (float dt at
/// ctx+0x8) instead of fabricating one. The InGameStep's own state lives at
/// `+0x48` (`-1` == finished); we tick only while `+0xd8 != 0` and `+0x48 != -1`.
pub(crate) const STEP_PUMP_DRIVER_RVA: u32 = 0x00b0bd60;
pub(crate) const INGAMESTEP_STEP_STATE_OFFSET: usize = 0x48;
pub(crate) const INGAMESTEP_NEXT_STATE_OFFSET: usize = 0x4c;
pub(crate) const INGAMESTEP_FINISHED_SENTINEL: i32 = -1;
pub(crate) const INGAMESTEP_LOAD_DONE: i32 = 0;
pub(crate) const INGAMESTEP_PUMP_D8_UNOBSERVED: i32 = -2;
/// FD4StepTemplate force-state override fields (pump `0x140b0bd60` @ 0xb0be01:
/// `if byte[+0x69]!=0 && byte[+0xa8]==0 { +0x48 = +0x4c = [+0xac]; +0xa8=0 }`).
/// If `+0x69` is set and `+0xac` pins the step index, the machine never advances.
pub(crate) const INGAMESTEP_OVERRIDE_TRIGGER_OFFSET: usize = 0x69;
pub(crate) const INGAMESTEP_OVERRIDE_GUARD_OFFSET: usize = 0xa8;
pub(crate) const INGAMESTEP_OVERRIDE_TARGET_OFFSET: usize = 0xac;
pub(crate) const INGAMESTEP_OVERRIDE_TRIGGER_CLEAR: u8 = 0;
pub(crate) const MENU_TASK_NULL_STATE_QWORD: usize = 0;
pub(crate) const MENU_TASK_NULL_PAYLOAD_PTR: usize = 0;
pub(crate) const MENU_TASK_STATE_PAYLOAD_CODE_OFFSET: usize = 4;
pub(crate) const MENU_TRACE_EVENT_INCREMENT: usize = 1;
pub(crate) const TASK_ENQUEUE_TRACE_INCREMENT: usize = 1;
pub(crate) static START_GAME_TASK: Once = Once::new();
pub(crate) static START_CONTINUE_TRACE: Once = Once::new();
pub(crate) static START_SAFE_INPUT_HOOKS: Once = Once::new();
pub(crate) static START_SPLASH_SKIP: Once = Once::new();
pub(crate) static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize =
    AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);
pub(crate) static SAFE_INPUT_CONFIRM_FRAMES_REMAINING: AtomicUsize = AtomicUsize::new(0);

pub(crate) static MENU_CONTINUE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_NEW_OR_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_OTHER_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TASK_UPDATE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TASK_ENQUEUE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SET_SAVE_SLOT_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_REQUEST_PROFILE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static REQUEST_SAVE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CURRENT_SLOT_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CONTINUE_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static COMBINED_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MAP_LOAD_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAVE_LOAD_STATE_INIT_ORIG: AtomicUsize = AtomicUsize::new(0);
// MENU-UI capture (Path B / zero-input state-stepper): log-only trampolines on the title
// menu-navigation functions so one real user navigation (press-any-key -> Continue/Load ->
// slot -> confirm) yields the exact this-pointers + construction order + call sequence for
// the 4 interactions. SetState (state sequence), Continue confirm, ProfileLoadDialog activate
// (slot-20 + variant), the enter-Load-Game builder, the selector-step tick, the menu mount.
pub(crate) static CAP_SETSTATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_CONTINUE_CONFIRM_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_LOAD_ACTIVATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_LOAD_ACTIVATE2_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_BUILDER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_SELECTOR_TICK_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_MENU_DESER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ProfileLoadDialog lambda factory 0x14081ead0 (op-new 0x1cd0 + ctor 0x1409a3d90). Hooking
/// it with a caller backtrace captures the full construction chain: press-any-key -> main
/// menu -> "Load Game" activated -> dialog built, plus the rcx/rdx context the factory needs
/// (so the dialog can be built zero-input in the replay).
pub(crate) static CAP_DIALOG_FACTORY_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_SELECTOR_TICK_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const CAP_SELECTOR_TICK_LOG_FIRST: usize = 4;
pub(crate) const CAP_SELECTOR_TICK_LOG_INTERVAL: usize = 120;
/// Selector-owner step (0x140826d50) install-flag field: 0 on the first tick (fires the
/// delegate-installer 0x140828270), 1 afterwards.
pub(crate) const SELECTOR_STEP_INSTALL_FLAG_68_OFFSET: usize = 0x68;
// b80 save-mount orchestration capture (own-stepper-dispatcher-mount-failed-and-wrote-
// save-2026 next-approach): entry/exit logging trampolines on the 5 b80 functions so a
// real user-driven .co2 load yields the exact call order + args + which fn populates
// io18/io20 + which transitions b80 + which applies the character.
pub(crate) static B80_PREVIEW_INITIATOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_LOAD_SAVE_DATA_INITIATOR_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_FULL_LOAD_INITIATOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_POLL_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_DESERIALIZE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static C30_WRITER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static GET_ASYNC_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GET_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DIRECT_INPUT8_CREATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DIRECT_INPUT_CREATE_DEVICE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DIRECT_INPUT_GET_DEVICE_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_BOOTSTRAP_SEEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OWNER_PTR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OWNER_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_NATIVE_JOB_CALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static FORCE_PLAY_GAME_CALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SUBMIT_PLAY_GAME_PHASE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(SUBMIT_PHASE_INIT);
pub(crate) static FORCE_PLAY_GAME_LAST_STATE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(FORCE_PLAY_GAME_STATE_UNOBSERVED);
pub(crate) static TITLE_PROCEED_GATE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static INGAMESTEP_PUMP_LAST_D8: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(INGAMESTEP_PUMP_D8_UNOBSERVED);
pub(crate) static INGAMESTEP_PUMP_LAST_NEXT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(INGAMESTEP_PUMP_D8_UNOBSERVED);
pub(crate) static INGAMESTEP_UNPIN_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static NATIVE_AUTOLOAD_ARMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static SYNTHETIC_OUTER_PTR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CONTINUE_OWNER_PTR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CONTINUE_DRIVE_BEGUN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static ORIGINAL_EXIT_PROCESS: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_TERMINATE_PROCESS: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_RTL_EXIT_USER_PROCESS: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_NT_TERMINATE_PROCESS: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_ASSERT_WRAPPER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ASSERT_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RENDER_FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROCESS_EXIT_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static AV_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CRASH_LOGGER_INSTALLED: std::sync::Once = std::sync::Once::new();
pub(crate) static INGAMEINIT_DRIVE_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static TITLE_OWNER_SCAN_COUNTDOWN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SAFE_INPUT_CONFIRM_PULSE_SEQ: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_EVENT_SEQ: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_LAST_SEQ: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_LAST_HOOK_RVA: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_LAST_TABLE_RVA: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_LAST_THIS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_LAST_STATE_QWORD: AtomicUsize = AtomicUsize::new(0);
pub(crate) static MENU_TRACE_LAST_PAYLOAD_PTR: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TASK_ENQUEUE_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// A named runtime effect call the overlay can trigger.
///
/// Adding a new call kind (e.g. an SFX/FXR spawn once `fromsoftware-rs`
/// exposes a wrapper for it) takes three mechanical steps:
/// 1. add a variant to `er_effects_data::EffectKindSpec` (the
///    `data/effects.json` schema),
/// 2. add the matching variant here plus arms in `label`/`apply`/`remove`/
///    `is_active`,
/// 3. map it in `call_kind_from_spec`.
/// The overlay and the game task dispatch exclusively through those four
/// methods, so nothing else needs to change.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectCallKind {
    SpEffect { id: i32 },
}

impl EffectCallKind {
    fn label(self) -> String {
        match self {
            Self::SpEffect { id } => format!("SpEffect {id}"),
        }
    }

    fn apply(self, player: &mut PlayerIns, network_sync: bool) {
        match self {
            Self::SpEffect { id } => {
                let dont_sync = !network_sync;
                player.apply_speffect(id, dont_sync);
            }
        }
    }

    fn remove(self, player: &mut PlayerIns) {
        match self {
            Self::SpEffect { id } => player.chr_ins.remove_speffect(id),
        }
    }

    /// Whether the call is currently in force on the player. The game's
    /// apply/remove calls return nothing, so the active-SpEffect list is the
    /// ground truth for surfacing success and failure in the overlay.
    fn is_active(self, player: &PlayerIns) -> bool {
        match self {
            Self::SpEffect { id } => player
                .chr_ins
                .special_effect
                .entries()
                .any(|entry| entry.param_id == id),
        }
    }
}

pub(crate) struct NamedEffectCall {
    name: String,
    kind: EffectCallKind,
    enabled: bool,
    remove_requested: bool,
    /// Live status, recomputed every game tick from the player's SpEffect
    /// list.
    active: bool,
    /// Set when an apply attempt did not take (e.g. the ID has no
    /// `SpEffectParam` row); cleared as soon as the effect shows up active.
    apply_failed: bool,
}

impl NamedEffectCall {
    fn new(name: String, kind: EffectCallKind, enabled: bool) -> Self {
        Self {
            name,
            kind,
            enabled,
            remove_requested: false,
            active: false,
            apply_failed: false,
        }
    }
}

pub(crate) fn call_kind_from_spec(kind: EffectKindSpec, id: i32) -> EffectCallKind {
    match kind {
        EffectKindSpec::SpEffect => EffectCallKind::SpEffect { id },
    }
}

pub(crate) fn named_call_from_spec(spec: EffectCallSpec) -> NamedEffectCall {
    let kind = call_kind_from_spec(spec.kind, spec.id);
    NamedEffectCall::new(spec.name, kind, spec.enabled)
}

#[derive(Default)]
pub(crate) struct SafeInputRuntime {
    loaded: bool,
    confirm_count: u32,
    pulses_sent: u32,
    interval_ticks: u64,
    initial_delay_ticks: u64,
    last_pulse_tick: u64,
    hooks_requested: bool,
    last_status: Option<String>,
}

pub(crate) struct EffectsState {
    calls: Vec<NamedEffectCall>,
    /// Parse error for the embedded `data/effects.json`, shown in the overlay
    /// instead of silently starting with an empty list.
    load_error: Option<String>,
    current_animation_id: Option<i32>,
    applied_for_current_appear: bool,
    /// TimeAct queue write index at the previous tick; used to detect appear
    /// animations that were enqueued (and possibly finished) between ticks.
    last_write_idx: Option<u32>,
    manual_apply_requested: bool,
    remove_all_requested: bool,
    network_sync: bool,
    custom_call_id: i32,
    last_telemetry_write: Option<Instant>,
    last_driver_command: Option<String>,
    autoload: SaveLoader,
    game_task_ticks: u64,
    safe_input: SafeInputRuntime,
}

impl Default for EffectsState {
    fn default() -> Self {
        let (calls, load_error) = match embedded_effects() {
            Ok(effects) => (
                effects
                    .calls
                    .into_iter()
                    .map(named_call_from_spec)
                    .collect(),
                None,
            ),
            Err(error) => (
                Vec::new(),
                Some(format!("failed to parse embedded effects.json: {error}")),
            ),
        };

        Self {
            calls,
            load_error,
            current_animation_id: None,
            applied_for_current_appear: false,
            last_write_idx: None,
            manual_apply_requested: false,
            remove_all_requested: false,
            network_sync: false,
            custom_call_id: CUSTOM_CALL_DEFAULT_ID,
            last_telemetry_write: None,
            last_driver_command: None,
            autoload: SaveLoader::from_env(),
            game_task_ticks: INITIAL_GAME_TASK_TICKS,
            safe_input: SafeInputRuntime::default(),
        }
    }
}

pub(crate) struct EffectsOverlay {
    state: Arc<Mutex<EffectsState>>,
}

impl ImguiRenderLoop for EffectsOverlay {
    fn initialize(&mut self, ctx: &mut Context, _render_context: &mut dyn hudhook::RenderContext) {
        // Elden Ring hides/captures the OS cursor under Proton. Draw ImGui's
        // software cursor so overlay hit-testing is visible during Linux smoke
        // tests and normal in-game use.
        ctx.io_mut().mouse_draw_cursor = true;
    }

    fn render(&mut self, ui: &mut Ui) {
        // render_liveness_probe() temporarily disabled: isolating whether the
        // boot-crash is the render-thread probe vs the game-task scan.
        let blocker = InputBlocker::get_instance();
        unsafe {
            let _ = blocker.install_hooks();
        }
        blocker.block_from_io(ui.io());

        let mut state = state_or_return(&self.state);
        process_global_driver_command(&mut state);
        let player_available = if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
            process_driver_command(player, &mut state);
            refresh_call_status(player, &mut state);
            true
        } else {
            process_autoload_request(&mut state);
            false
        };
        write_telemetry_throttled(&mut state, player_available);

        ui.window("ER Effects")
            .position(OVERLAY_INITIAL_POSITION, Condition::FirstUseEver)
            .size(OVERLAY_INITIAL_SIZE, Condition::FirstUseEver)
            .build(|| {
                if let Some(error) = &state.load_error {
                    ui.text_wrapped(format!("effects.json error: {error}"));
                    ui.separator();
                }

                ui.text(format!(
                    "Current animation: {}",
                    state
                        .current_animation_id
                        .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
                ));
                ui.text(format!("Appear trigger animation: {APPEAR_ANIMATION_ID}"));

                ui.separator();
                ui.checkbox(
                    "Sync effect calls over the network",
                    &mut state.network_sync,
                );

                if ui.button("Apply selected now") {
                    if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
                        apply_selected_calls(player, &mut state);
                        refresh_call_status(player, &mut state);
                    } else {
                        state.manual_apply_requested = true;
                    }
                }
                if ui.button("Remove all listed effects") {
                    if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
                        for call in &mut state.calls {
                            call.kind.remove(player);
                            call.remove_requested = false;
                        }
                        refresh_call_status(player, &mut state);
                    } else {
                        state.remove_all_requested = true;
                    }
                }

                ui.separator();
                ui.input_int("Custom SpEffect ID", &mut state.custom_call_id)
                    .build();
                if ui.button("Add custom call") {
                    add_custom_call(&mut state);
                }

                ui.separator();
                ui.text("Named calls");

                let network_sync = state.network_sync;
                let mut apply_requested_without_player = false;
                for call in &mut state.calls {
                    let label = format!("{} ({})", call.name, call.kind.label());
                    if ui.checkbox(&label, &mut call.enabled) {
                        if let Ok(player) = unsafe { PlayerIns::local_player_mut() } {
                            if call.enabled {
                                call.kind.apply(player, network_sync);
                                call.active = call.kind.is_active(player);
                                call.apply_failed = !call.active;
                            } else {
                                call.kind.remove(player);
                                call.remove_requested = false;
                                call.apply_failed = false;
                                call.active = call.kind.is_active(player);
                            }
                        } else if call.enabled {
                            apply_requested_without_player = true;
                        } else {
                            call.remove_requested = true;
                        }
                    }
                    ui.same_line();
                    ui.text(call_status_text(call));
                }
                if apply_requested_without_player {
                    state.manual_apply_requested = true;
                }
            });
    }

    fn message_filter(&self, io: &hudhook::imgui::Io) -> MessageFilter {
        if io.want_capture_mouse || io.want_capture_keyboard {
            MessageFilter::InputAll
        } else {
            MessageFilter::empty()
        }
    }
}

pub(crate) fn call_status_text(call: &NamedEffectCall) -> &'static str {
    if call.active {
        "[active]"
    } else if call.apply_failed {
        "[apply failed]"
    } else {
        "[inactive]"
    }
}

pub(crate) fn add_custom_call(state: &mut EffectsState) {
    let id = state.custom_call_id;
    let kind = EffectCallKind::SpEffect { id };
    if state.calls.iter().any(|call| call.kind == kind) {
        return;
    }
    state
        .calls
        .push(NamedEffectCall::new(format!("Custom {id}"), kind, true));
}

#[unsafe(no_mangle)]
/// # Safety
///
/// This is called by Windows when the DLL is loaded. Do not call it directly.
pub unsafe extern "C" fn DllMain(hmodule: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason != DLL_PROCESS_ATTACH {
        return DLL_MAIN_SUCCESS;
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_DLL_MAIN_ATTACH, BOOTSTRAP_DETAIL_START);

    // Install the crash/exit logger first so it can observe an exit or access
    // violation from any later subsystem. Opt-in; off by default.
    if crash_logger_enabled() {
        install_crash_logger();
    }

    let state = Arc::new(Mutex::new(EffectsState::default()));

    // Splash-skip: apply the clean BeginLogo branch-flip as early as possible,
    // from a thread, so it lands before the title state machine runs state 2.
    if splash_skip_enabled() {
        START_SPLASH_SKIP.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-splash-skip".to_owned())
                .spawn(apply_splash_skip);
        });
    }

    let direct_autoload_configured = {
        let state = state_or_return(&state);
        state.autoload.method() == SaveLoadMethod::DirectMenuLoad && state.autoload.slot().is_some()
    };
    if safe_input_path().exists() {
        START_SAFE_INPUT_HOOKS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-safe-input-hooks".to_owned())
                .spawn(install_safe_input_hooks);
        });
    }
    if (trace_continue_enabled() || direct_autoload_configured) && !continue_trace_disabled() {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_CONTINUE_TRACE_REQUESTED,
            BOOTSTRAP_DETAIL_START,
        );
        START_CONTINUE_TRACE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-continue-trace".to_owned())
                .spawn(install_continue_trace_hooks);
        });
    }
    write_bootstrap_event(BOOTSTRAP_EVENT_GAME_TASK_REQUESTED, BOOTSTRAP_DETAIL_START);
    START_GAME_TASK.call_once({
        let state = Arc::clone(&state);
        move || spawn_game_task(state)
    });

    let autoload_without_overlay = state_or_return(&state).autoload.slot().is_some();
    if autoload_without_overlay {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_OVERLAY_SKIPPED_AUTOLOAD,
            BOOTSTRAP_DETAIL_DONE,
        );
        return DLL_MAIN_SUCCESS;
    }

    debug::initialize::<ImguiDx12Hooks>(
        hmodule,
        reason,
        || {
            let _ = wait_for_task_instance();
        },
        EffectsOverlay { state },
    )
}

pub(crate) fn wait_for_task_instance() -> &'static CSTaskImp {
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => {
                std::thread::yield_now()
            }
        }
    }
}

pub(crate) fn spawn_game_task(state: Arc<Mutex<EffectsState>>) {
    std::thread::spawn(move || {
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_THREAD_STARTED,
            BOOTSTRAP_DETAIL_START,
        );
        let cs_task = wait_for_task_instance();
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY,
            BOOTSTRAP_DETAIL_DONE,
        );

        cs_task.run_recurring(
            move |task_data: &FD4TaskData| {
                // Bisect kill-switch: do nothing per frame. Isolates "our task
                // body crashes the title ~19s" from "the DLL's mere presence".
                if inert_mode() {
                    return;
                }
                // Hardware write-watchpoint on GameMan+0xc30: (re)arm each frame until
                // the save-mount write is caught, so the VEH logs the exact writer. Runs
                // before the player check so it arms at the title (pre-load), independent
                // of the active observe/own-stepper mode.
                if c30_watch_enabled() {
                    if let Ok(base) = game_module_base() {
                        let frame = C30_WATCH_FRAME_COUNTER
                            .fetch_add(C30_WATCH_HIT_INCREMENT, Ordering::SeqCst)
                            as u64;
                        unsafe { maybe_arm_c30_watch(base, frame) };
                    }
                }
                let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                    let mut state = state_or_return(&state);
                    state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                    // Bisect kill-switch: lock + tick only, NO filesystem I/O
                    // (no telemetry write, no experiments). Discriminates "our
                    // per-frame file I/O stalls the title" (lite survives) from
                    // "any per-frame work trips a budget" (lite still exits).
                    if lite_mode() {
                        return;
                    }
                    // OWN-THE-STEPPER: patch the idx10 step-fn slot to our handler so
                    // the FD4 scheduler runs OUR code in-context (step 1: verify the
                    // control point with a logging pass-through).
                    if own_stepper_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { own_stepper_patch_once(base) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Pure observe: log the title->menu->load transition each interval
                    // with NO forcing, to capture what the REAL button press does.
                    if observe_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { title_observe_tick(base, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Read-only: log the native autoload-arm preconditions
                    // (especially [slotmgr+0x8]) to decide the zero-input path.
                    if arm_probe_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { arm_precondition_probe(base, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Lever 2: zero-input title-accept via input-event injection
                    // (staged probe -> fill -> inject) to bootstrap the front-end.
                    if title_accept_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe {
                                title_accept_tick(
                                    base,
                                    state.game_task_ticks,
                                    title_accept_inject_enabled(),
                                )
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Corrected play-game submit: on the live FE-host at state 10,
                    // SetState(5) with a packed map (not raw state/slot like the old
                    // force_play_game) so the existing pump builds CSFeMan + loads.
                    if submit_play_game_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                submit_play_game_once(base, slot, state.game_task_ticks, task_data)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Per-frame native arm: re-set the slot each frame + latch so
                    // the save-mgr update can arm before the title resets the slot.
                    if native_arm_loop_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe { native_arm_loop_tick(base, slot, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Recipe Option 1 (flagless): drive the genuine offline
                    // continue (MoveMapList dispatcher + b73) to load the REAL slot.
                    if continue_drive_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe { continue_drive_tick(base, slot, state.game_task_ticks) };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // Recipe B (flagless): drive the outer IngameInit once + pump
                    // the InGameStep. Self-contained -- skips the other autoload
                    // branches to avoid double-submit. Needs the live FD4TaskData.
                    if ingameinit_drive_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                ingameinit_drive_tick(base, slot, state.game_task_ticks, task_data)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    process_safe_input_request(&mut state);
                    process_autoload_request(&mut state);
                    // Direct-drive the orphaned InGameStep load once force_play_game
                    // has reached GameStepWait (run 305: hooking the step pump froze
                    // the title, so we call its Execute directly with the live ctx).
                    if ingamestep_pump_enabled() {
                        if let Ok(base) = game_module_base() {
                            unsafe { ingamestep_pump_tick(base, task_data) };
                        }
                    }
                    write_telemetry_throttled(&mut state, false);
                    return;
                };

                let mut state = state_or_return(&state);
                state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                let observation = observe_animation(player, state.last_write_idx);
                state.current_animation_id = observation.current_animation_id;
                state.last_write_idx = Some(observation.write_idx);

                remove_requested_calls(player, &mut state);
                process_driver_command(player, &mut state);

                let appear_playing = observation.current_animation_id == Some(APPEAR_ANIMATION_ID);
                if !appear_playing {
                    state.applied_for_current_appear = false;
                }

                let should_apply_for_appear = (observation.appear_newly_queued || appear_playing)
                    && !state.applied_for_current_appear;
                let should_apply = should_apply_for_appear || state.manual_apply_requested;
                state.manual_apply_requested = false;

                if should_apply_for_appear {
                    state.applied_for_current_appear = true;
                }

                if should_apply {
                    apply_selected_calls(player, &mut state);
                }

                process_global_driver_command(&mut state);
                refresh_call_status(player, &mut state);
                write_telemetry_throttled(&mut state, true);
            },
            CSTaskGroupIndex::FrameBegin,
        );
        write_bootstrap_event(
            BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED,
            BOOTSTRAP_DETAIL_DONE,
        );
    });
}

pub(crate) fn process_autoload_request(state: &mut EffectsState) {
    if state.autoload.completed() || state.autoload.slot().is_none() {
        return;
    }

    let Ok(game_man) = (unsafe { GameMan::instance_mut() }) else {
        return;
    };

    let Ok(game_module_base) = game_module_base() else {
        return;
    };

    if selectbot_probe_enabled() || title_proceed_gate_enabled() {
        // selectbot_probe_once samples the SelectBot/pump state each title-idle
        // frame; when ER_EFFECTS_TITLE_PROCEED_GATE is set it ALSO fires the
        // one-shot title-accept latch write (lever 1) at state 10. Returns
        // without completing the autoload so sampling continues across the
        // cascade.
        unsafe { selectbot_probe_once(game_module_base, state.game_task_ticks) };
        return;
    }

    if native_autoload_enabled() {
        // Recipe A: arm the game's own built-in title autoload (slot + force flag)
        // and let the save-manager update perform the load with zero input.
        if let Some(slot) = state.autoload.slot() {
            unsafe { native_autoload_once(game_module_base, slot, state.game_task_ticks) };
        }
        return;
    }

    if force_play_game_enabled() {
        if let Some(slot) = state.autoload.slot() {
            unsafe { call_force_play_game_once(game_module_base, slot, state.game_task_ticks) };
        }
        return;
    }

    if native_title_job_enabled()
        && !unsafe { call_native_title_job_once(game_module_base, state.game_task_ticks) }
    {
        return;
    }

    let context = SaveLoadContext {
        game_module_base,
        title_bootstrap_seen: TITLE_BOOTSTRAP_SEEN.load(Ordering::SeqCst) != TITLE_BOOTSTRAP_UNSEEN,
    };
    let _ = unsafe {
        state.autoload.process(game_man, context, |message| {
            append_autoload_debug(format_args!("{message}"))
        })
    };
}

pub(crate) fn state_or_return(
    state: &Arc<Mutex<EffectsState>>,
) -> std::sync::MutexGuard<'_, EffectsState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct AnimationObservation {
    current_animation_id: Option<i32>,
    /// True when the appear animation was written into the TimeAct queue
    /// since the previous tick. This catches plays that are too short to be
    /// observed as the "current" slot between two task ticks.
    appear_newly_queued: bool,
    write_idx: u32,
}

/// Reads the player's TimeAct animation state.
///
/// The TimeAct module keeps a 10-slot circular buffer of animations:
/// `read_idx` is the last animation played or updated and `write_idx` is the
/// slot the next animation will be written to. The current animation is the
/// `read_idx` slot; additionally, every slot written since the previous tick
/// (`last_write_idx..write_idx`) is checked for the appear animation. A
/// re-application can occur when a queued appear animation is seen both as
/// newly queued and later as current — SpEffect application is idempotent, so
/// missing a trigger is the worse failure mode.
pub(crate) fn observe_animation(
    player: &PlayerIns,
    last_write_idx: Option<u32>,
) -> AnimationObservation {
    let time_act = &player.chr_ins.modules.time_act;
    let queue_len = time_act.anim_queue.len() as u32;
    let read_slot = (time_act.read_idx % queue_len) as usize;
    let current_animation_id = valid_animation_id(time_act.anim_queue[read_slot].anim_id);
    let write_idx = time_act.write_idx;

    let mut appear_newly_queued = false;
    if let Some(last_write_idx) = last_write_idx {
        let mut index = last_write_idx;
        // Bounded to one lap of the circular buffer in case the write index
        // jumped by more than the queue length between ticks.
        let mut remaining = queue_len;
        while index != write_idx && remaining > ANIM_QUEUE_SCAN_FLOOR {
            let slot = (index % queue_len) as usize;
            if time_act.anim_queue[slot].anim_id == APPEAR_ANIMATION_ID {
                appear_newly_queued = true;
            }
            index = index.wrapping_add(ANIM_QUEUE_SLOT_STEP);
            remaining -= ANIM_QUEUE_SLOT_STEP;
        }
    }

    AnimationObservation {
        current_animation_id,
        appear_newly_queued,
        write_idx,
    }
}

pub(crate) fn valid_animation_id(anim_id: i32) -> Option<i32> {
    (anim_id > INVALID_ANIMATION_ID_FLOOR).then_some(anim_id)
}

pub(crate) fn process_global_driver_command(state: &mut EffectsState) {
    let path = command_path();
    let Ok(raw_command) = fs::read_to_string(&path) else {
        return;
    };
    let command = raw_command.trim();
    if !command.starts_with("load_slot ") {
        return;
    }
    let _ = fs::remove_file(path);

    let parts: Vec<_> = command.split_whitespace().collect();
    state.last_driver_command = Some(match parts.as_slice() {
        ["load_slot", slot] => match slot.parse() {
            Ok(slot) => {
                state.autoload.queue_direct_menu_load(slot);
                process_autoload_request(state);
                format!("ok: {command}")
            }
            Err(error) => format!("error: {command}: invalid slot: {error}"),
        },
        _ => format!("error: {command}: expected load_slot <index>"),
    });
}

pub(crate) fn process_driver_command(player: &mut PlayerIns, state: &mut EffectsState) {
    let path = command_path();
    let Ok(raw_command) = fs::read_to_string(&path) else {
        return;
    };
    let _ = fs::remove_file(path);

    execute_and_record_driver_command(player, state, raw_command.trim());
}

pub(crate) fn execute_and_record_driver_command(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    command: &str,
) {
    if command.is_empty() {
        return;
    }

    state.last_driver_command = Some(match execute_driver_command(player, state, command) {
        Ok(()) => format!("ok: {command}"),
        Err(error) => format!("error: {command}: {error}"),
    });
}

pub(crate) fn execute_driver_command(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    command: &str,
) -> Result<(), String> {
    let parts: Vec<_> = command.split_whitespace().collect();
    match parts.as_slice() {
        ["apply_all"] => {
            apply_selected_calls(player, state);
            refresh_call_status(player, state);
            Ok(())
        }
        ["remove_all"] => {
            for call in &mut state.calls {
                call.kind.remove(player);
                call.enabled = false;
                call.remove_requested = false;
                call.apply_failed = false;
            }
            refresh_call_status(player, state);
            Ok(())
        }
        ["apply", index] => set_call_enabled(player, state, parse_call_index(index)?, true),
        ["remove", index] => set_call_enabled(player, state, parse_call_index(index)?, false),
        ["set", index, "on"] => set_call_enabled(player, state, parse_call_index(index)?, true),
        ["set", index, "off"] => set_call_enabled(player, state, parse_call_index(index)?, false),
        ["toggle", index] => {
            let index = parse_call_index(index)?;
            let enabled = !state
                .calls
                .get(index)
                .ok_or_else(|| format!("call index {index} out of range"))?
                .enabled;
            set_call_enabled(player, state, index, enabled)
        }
        _ => Err("expected apply_all, remove_all, apply <index>, remove <index>, set <index> on|off, toggle <index>, or load_slot <index> before player load".to_owned()),
    }
}

pub(crate) fn parse_call_index(index: &str) -> Result<usize, String> {
    index
        .parse()
        .map_err(|error| format!("invalid call index {index:?}: {error}"))
}

pub(crate) fn set_call_enabled(
    player: &mut PlayerIns,
    state: &mut EffectsState,
    index: usize,
    enabled: bool,
) -> Result<(), String> {
    let call = state
        .calls
        .get_mut(index)
        .ok_or_else(|| format!("call index {index} out of range"))?;

    call.enabled = enabled;
    if enabled {
        call.kind.apply(player, state.network_sync);
        call.active = call.kind.is_active(player);
        call.apply_failed = !call.active;
    } else {
        call.kind.remove(player);
        call.remove_requested = false;
        call.apply_failed = false;
        call.active = call.kind.is_active(player);
    }

    Ok(())
}

pub(crate) fn remove_requested_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    if state.remove_all_requested {
        for call in &mut state.calls {
            call.kind.remove(player);
            call.remove_requested = false;
            call.apply_failed = false;
        }
        state.remove_all_requested = false;
        return;
    }

    for call in &mut state.calls {
        if call.remove_requested {
            call.kind.remove(player);
            call.remove_requested = false;
            call.apply_failed = false;
        }
    }
}

pub(crate) fn apply_selected_calls(player: &mut PlayerIns, state: &mut EffectsState) {
    let network_sync = state.network_sync;
    for call in state.calls.iter_mut().filter(|call| call.enabled) {
        call.kind.apply(player, network_sync);
        // The game call reports nothing, so check the active list directly.
        call.apply_failed = !call.kind.is_active(player);
    }
}

pub(crate) fn refresh_call_status(player: &PlayerIns, state: &mut EffectsState) {
    for call in &mut state.calls {
        call.active = call.kind.is_active(player);
        if call.active {
            call.apply_failed = false;
        }
    }
}
