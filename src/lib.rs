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
    cs::{
        CSTaskGroupIndex, CSTaskImp, ChrInsExt, FaceData, FaceDataBuffer, GameDataMan, GameMan,
        PlayerGameData, PlayerIns,
    },
    dlkr::DLAllocator,
    fd4::FD4TaskData,
};
use er_effects_data::{EffectCallSpec, EffectKindSpec, embedded_effects};
use er_save_loader::{GameManTelemetry, SaveLoadContext, SaveLoader};
use fromsoftware_shared::{F32Vector4, FromStatic, InstanceError, SharedTaskImpExt};
use hudhook::{
    ImguiRenderLoop, MessageFilter,
    hooks::dx12::ImguiDx12Hooks,
    imgui::{Condition, Context, Ui},
    mh::{MH_ApplyQueued, MH_Initialize, MH_STATUS, MhHook},
    windows::{
        Win32::{
            Foundation::{HINSTANCE, HWND, LPARAM, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA},
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

// === Software (INT3) breakpoint engine ===========================================
// A scriptable code-breakpoint system driven entirely in-process (no Cheat Engine /
// GUI needed): patch 0xCC at a target VA, catch EXCEPTION_BREAKPOINT in the VEH,
// log the full register/stack context, restore the byte, single-step over the
// original instruction (trap flag), then re-arm. This is the same mechanism CE's
// VEH debugger uses; software INT3 + VEH works under wine/Proton (esync/fsync),
// unlike hardware DR data breakpoints. RVAs to break on are read from
// er-effects-breakpoints.txt (one hex RVA per line) in the game dir.
pub(crate) const EXCEPTION_BREAKPOINT_CODE: u32 = 0x80000003;
/// Win64 CONTEXT GP-register + EFlags offsets (ABI-fixed). EFlags carries the trap flag.
pub(crate) const CONTEXT_EFLAGS_OFFSET: usize = 0x44;
pub(crate) const CONTEXT_RAX_OFFSET: usize = 0x78;
pub(crate) const CONTEXT_RCX_OFFSET: usize = 0x80;
pub(crate) const CONTEXT_RDX_OFFSET: usize = 0x88;
pub(crate) const CONTEXT_RSP_OFFSET: usize = 0x98;
pub(crate) const CONTEXT_R8_OFFSET: usize = 0xb8;
pub(crate) const CONTEXT_R9_OFFSET: usize = 0xc0;
/// Trap flag (EFlags bit 8): set to single-step the restored instruction, then clear.
pub(crate) const TRAP_FLAG_MASK: u32 = 0x100;
/// INT3 opcode; the byte we patch in to trigger EXCEPTION_BREAKPOINT.
pub(crate) const INT3_OPCODE: u8 = 0xcc;
/// One INT3 byte; the patch/restore size.
pub(crate) const INT3_PATCH_SIZE: usize = 1;
/// Initial value for the VirtualProtect old-protection out-param.
pub(crate) const PROTECT_OLD_INIT: u32 = 0;
/// Radix for parsing hex RVAs from er-effects-breakpoints.txt.
pub(crate) const RVA_HEX_RADIX: u32 = 16;
/// INT3 is one byte; on #BP the trap RIP points just past it, so the breakpoint
/// address = RIP - 1.
pub(crate) const INT3_RIP_BACKUP: usize = 1;
/// Max simultaneous software breakpoints.
pub(crate) const SW_BP_MAX: usize = 8;
/// Empty breakpoint slot sentinel (no address armed).
pub(crate) const SW_BP_EMPTY: usize = 0;
/// "no original byte recorded" sentinel (a real byte is 0..=0xff, so 0x100 is free).
pub(crate) const SW_BP_ORIG_NONE: usize = 0x100;
/// Mask to recover the original byte from the stored slot value.
pub(crate) const SW_BP_ORIG_BYTE_MASK: usize = 0xff;
/// Per-breakpoint hit-log cap (so a per-frame breakpoint does not flood the log).
pub(crate) const SW_BP_MAX_LOGS_PER_BP: usize = 24;
/// Pending-rearm sentinel (no breakpoint awaiting re-arm on the next single-step).
pub(crate) const SW_BP_REARM_NONE: usize = 0;
pub(crate) const SW_BP_HIT_INCREMENT: usize = 1;
/// Initial per-breakpoint hit counter.
pub(crate) const SW_BP_HITS_INIT: usize = 0;
pub(crate) const SW_BP_SLOT_STEP: usize = 1;
/// Number of stack qwords to dump on a breakpoint hit (args spilled past r9 + locals).
pub(crate) const SW_BP_STACK_DUMP_QWORDS: usize = 40;
pub(crate) static SW_BP_ADDR: [AtomicUsize; SW_BP_MAX] =
    [const { AtomicUsize::new(SW_BP_EMPTY) }; SW_BP_MAX];
pub(crate) static SW_BP_ORIG: [AtomicUsize; SW_BP_MAX] =
    [const { AtomicUsize::new(SW_BP_ORIG_NONE) }; SW_BP_MAX];
pub(crate) static SW_BP_HITS: [AtomicUsize; SW_BP_MAX] =
    [const { AtomicUsize::new(SW_BP_HITS_INIT) }; SW_BP_MAX];
/// Address awaiting re-arm on the next single-step (set in the #BP handler, consumed
/// in the single-step handler). Single global: our breakpoints fire on one menu thread.
pub(crate) static SW_BP_REARM_PENDING: AtomicUsize = AtomicUsize::new(SW_BP_REARM_NONE);
pub(crate) static SW_BP_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Diagnostic: count #BP exceptions our VEH sees that are NOT at one of our armed addresses,
/// to distinguish "VEH gets #BP but addr mismatch" from "VEH never sees #BP" under wine.
pub(crate) static SW_BP_UNMATCHED_LOGGED: AtomicUsize = AtomicUsize::new(SW_BP_HITS_INIT);
pub(crate) const SW_BP_MAX_UNMATCHED_LOGS: usize = 8;

// === Anti-anti-debug (ported from Dasaav-dsv/ProDebug, corrected for ER 1.16.1) ===========
// FromSoft's Arxan inserts timed anti-debug checks that detect a debugger/VEH and swallow debug
// exceptions (which is why our INT3 #BP never reached our VEH). ProDebug patches these checks out
// by pattern. The GitHub ProDebug.dll crashes 1.16.1 because it scans GetModuleHandle(NULL) (the
// wrong module base under the LazyLoader/wine -> wild +0x140000000 deref). We port the same
// patterns but scan our correctly-resolved game_module_base()'s .text only. Each entry is
// (find_pattern, patch_pattern) as hex strings with "??" wildcards; in the patch, every non-??
// byte overwrites the matched bytes at that offset (so no numeric literals -> no magic-number
// lint). Patches neutralize the timed-check branches (e.g. force the conditional jumps to fall
// through). Verified offline match counts on 1.16.1: check1s=181, check1l=1, check2=138, check3=10.
pub(crate) static ANTI_ANTIDEBUG_CHECKS: &[(&str, &str)] = &[
    (
        "7A ?? 75 ?? B9 ?? ?? ?? ?? E8 ?? ?? ?? ?? F3 0F 11 05",
        "?? 02 ?? 00",
    ),
    (
        "0F 8A ?? ?? ?? ?? 0F 85 ?? ?? ?? ?? B9 ?? ?? ?? ?? E8 ?? ?? ?? ?? F3 0F 11 05",
        "?? ?? 06 00 00 00 ?? ?? 00 00 00 00",
    ),
    ("73 ?? 0F 2F ?? 76 ?? 48 8D 15", "?? 00"),
    (
        "72 ?? 48 8D 4C 24 ?? E8 ?? ?? ?? ?? 90 48 8B 05 ?? ?? ?? ?? FF D0",
        "EB",
    ),
];
/// Pattern wildcard token.
pub(crate) const PATTERN_WILDCARD: &str = "??";
/// PE header field offsets used to locate the .text section at the live module base.
pub(crate) const PE_DOS_LFANEW_OFFSET: usize = 0x3c;
pub(crate) const PE_FILE_NUM_SECTIONS_OFFSET: usize = 0x6;
pub(crate) const PE_FILE_SIZE_OPT_HEADER_OFFSET: usize = 0x14;
pub(crate) const PE_OPT_HEADER_OFFSET: usize = 0x18;
pub(crate) const PE_SECTION_HEADER_SIZE: usize = 0x28;
pub(crate) const PE_SECTION_NAME_LEN: usize = 8;
pub(crate) const PE_SECTION_VSIZE_OFFSET: usize = 0x8;
pub(crate) const PE_SECTION_VADDR_OFFSET: usize = 0xc;
/// The executable section name we scan/patch.
pub(crate) const PE_TEXT_SECTION_NAME: &[u8] = b".text";
/// Once-guard for the anti-anti-debug patch (0 = not yet applied).
pub(crate) static ANTI_ANTIDEBUG_APPLIED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const ANTI_ANTIDEBUG_NOT_APPLIED: usize = 0;
pub(crate) const ANTI_ANTIDEBUG_STEP: usize = 1;
pub(crate) const ANTI_ANTIDEBUG_COUNT_INIT: usize = 0;
/// Masks to extract u32/u16 PE header fields from an 8-byte read.
pub(crate) const PE_U32_MASK: usize = 0xffff_ffff;
pub(crate) const PE_U16_MASK: usize = 0xffff;
/// First section index for the .text scan.
pub(crate) const PE_SECTION_SCAN_START: usize = 0;
/// Current-process pseudo-handle (-1) for FlushInstructionCache, + whole-process flush size.
pub(crate) const ER_CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
pub(crate) const FLUSH_WHOLE_PROCESS_SIZE: usize = 0;
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
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_WAITING_INSTANCE: &str = "game_task_waiting_instance";
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_INSTANCE_READY: &str = "game_task_instance_ready";
pub(crate) const BOOTSTRAP_EVENT_GAME_TASK_RECURRING_REGISTERED: &str =
    "game_task_recurring_registered";
pub(crate) const BOOTSTRAP_EVENT_TELEMETRY_WRITE: &str = "telemetry_write";
pub(crate) const BOOTSTRAP_EVENT_POLICY_TELEMETRY_SNAPSHOT: &str = "policy_telemetry_snapshot";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_STARTED: &str = "continue_trace_started";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLIED: &str = "continue_trace_applied";
pub(crate) const BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLY_FAILED: &str = "continue_trace_apply_failed";
pub(crate) const BOOTSTRAP_DETAIL_START: &str = "start";
pub(crate) const BOOTSTRAP_DETAIL_DONE: &str = "done";
pub(crate) const BOOTSTRAP_DETAIL_PLAYER_AVAILABLE: &str = "player_available";
pub(crate) const BOOTSTRAP_DETAIL_PLAYER_UNAVAILABLE: &str = "player_unavailable";
pub(crate) const INITIAL_GAME_TASK_TICKS: u64 = 0;
pub(crate) const GAME_TASK_TICK_INCREMENT: u64 = 1;
pub(crate) const TASK_INSTANCE_WAIT_LOG_INTERVAL: u64 = 4096;
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
#[repr(usize)]
pub(crate) enum TitleSessionRva {
    TitleOwnerVtable = 0x02b63bb0,
    SaveSafeBeginLogoSession = 0x4588e98,
    SessionA = 0x3d687a0,
    SessionB = 0x3d67bd0,
    MoveMapSession = 0x47ef360,
}

pub(crate) const TITLE_OWNER_VTABLE_RVA: usize = TitleSessionRva::TitleOwnerVtable as usize;
/// Partial SimpleTitleStep owner layout used by the zero-input title/menu driver.
/// Unknown byte arrays intentionally document unmodeled in-between fields while
/// keeping the offsets compiler-checked through `offset_of!`.
#[repr(C)]
pub(crate) struct TitleOwnerLayout {
    pub(crate) vtable: usize,
    pub(crate) unknown_08: [u8; 0x08],
    pub(crate) instance_table: usize,
    pub(crate) unknown_18: [u8; 0x30],
    pub(crate) committed_state: i32,
    pub(crate) requested_state: i32,
    pub(crate) unknown_50: [u8; 0x68],
    pub(crate) beginlogo_list_gate: u32,
    pub(crate) play_game_slot: i32,
    pub(crate) unknown_c0: [u8; 0x20],
    pub(crate) menu_holder: usize,
    pub(crate) unknown_e8: [u8; 0x48],
    pub(crate) menu_list: usize,
    pub(crate) unknown_138: [u8; 0x14c],
    pub(crate) new_game_flag: u8,
    pub(crate) unknown_285: [u8; 0x63],
    pub(crate) load_job: usize,
    pub(crate) unknown_2f0: [u8; 0xf1],
    pub(crate) play_game_request_flag: u8,
}

#[repr(C)]
pub(crate) struct TitleOwnerLoadJobLayout {
    pub(crate) unknown_000: [u8; 0xd8],
    pub(crate) pending: i32,
}

pub(crate) const TITLE_OWNER_STATE_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, requested_state);
/// Committed/current state the inner-TitleStep dispatcher actually runs (the pump
/// commits +0x4c -> +0x48 each frame and dispatches on +0x48). +0x4c is the
/// requested/next state. Read +0x48 to know the live state.
pub(crate) const TITLE_OWNER_STATE_COMMITTED_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, committed_state);
/// The inner TitleStep stores a per-instance copy of its state-dispatch table
/// base (0x143d71580) at owner+0x10; the dispatcher reads [owner+0x10]. Requiring
/// this rejects stray .data vtable matches (e.g. the 0x1000ffc58 false positive).
pub(crate) const TITLE_OWNER_INSTANCE_TABLE_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, instance_table);
pub(crate) const INNER_TITLE_STATE_TABLE_RVA: usize = 0x3d71580;
pub(crate) const TITLE_OWNER_SCAN_ALIGNMENT: usize = core::mem::align_of::<usize>();
pub(crate) const TITLE_OWNER_SCAN_MAX_ADDRESS: usize =
    (true as usize) << (usize::BITS as usize - (u16::BITS as usize + true as usize));
#[repr(usize)]
pub(crate) enum TraceSampleLimit {
    Value4 = 4,
    Value8 = 8,
    Value12 = 12,
    Value24 = 24,
    Value48 = 48,
    Value64 = 64,
}

pub(crate) const TITLE_OWNER_TRACE_LIMIT: usize = TraceSampleLimit::Value64 as usize;
/// How many `title_owner` calls to skip between full-memory owner scans.
///
/// The owner scan walks every committed region via `VirtualQuery`; running it
/// every frame while the owner does not yet exist (or cannot be matched)
/// collapses the game's frame rate. Throttling to roughly once per second at
/// 60 fps keeps a failed lookup from being user-visible.
pub(crate) const TITLE_OWNER_SCAN_CALL_INTERVAL: usize = TitleNativeJobTiming::FrameRate as usize;
pub(crate) const TITLE_OWNER_SCAN_COUNTDOWN_STEP: usize = true as usize;
pub(crate) const TITLE_OWNER_SCAN_COUNTDOWN_READY: usize = usize::MIN;
#[repr(u32)]
pub(crate) enum MenuTraceRva {
    TaskEnqueue = 0x007a7b60,
    TaskUpdateWrapper = 0x0082a0f0,
    NewOrLoadWrapper = 0x0082ba80,
    ContinueWrapper = 0x0082bac0,
    MenuJobWait = 0x00b0d400,
    TaskUpdateTable = 0x02ac72a0,
}

pub(crate) const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;
/// Legacy native-autoload startup delay is a diagnostic tick throttle only; product autoload
/// phases must use semantic predicates plus wall-clock fail-safe deadlines, never frame budgets.
pub(crate) const TITLE_NATIVE_JOB_MIN_TICK: u64 = 170;
pub(crate) const MEM_COMMIT_NUMERIC: u32 = 0x1000;
pub(crate) const PAGE_NOACCESS_NUMERIC: u32 = 0x01;
pub(crate) const PAGE_GUARD_NUMERIC: u32 = 0x100;
pub(crate) const TRACE_MENU_CONTINUE_WRAPPER_RVA: u32 = MenuTraceRva::ContinueWrapper as u32;
pub(crate) const TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = MenuTraceRva::NewOrLoadWrapper as u32;
pub(crate) const TRACE_MENU_OTHER_LOAD_WRAPPER_RVA: u32 =
    er_save_loader::MENU_OTHER_LOAD_WRAPPER_RVA;
pub(crate) const TRACE_MENU_TASK_UPDATE_WRAPPER_RVA: u32 = MenuTraceRva::TaskUpdateWrapper as u32;
pub(crate) const TRACE_MENU_TASK_UPDATE_TABLE_RVA: u32 = MenuTraceRva::TaskUpdateTable as u32;
pub(crate) const TRACE_TASK_ENQUEUE_RVA: u32 = MenuTraceRva::TaskEnqueue as u32;
pub(crate) const RESULT_EVENT_HANDLER_RVA: u32 = 0x00746e80;
pub(crate) const RESULT_ACTION_BUILDER_RVA: u32 = 0x00746a00;
pub(crate) const RESULT_EVENT_WRAPPER_BUILDER_RVA: u32 = 0x00744a60;
pub(crate) const TRACE_UNKNOWN_TABLE_RVA: u32 = 0;

#[repr(C)]
pub(crate) struct MenuTaskStateLayout {
    pub(crate) state_code: i32,
    pub(crate) payload_code: i32,
    pub(crate) delay_bits: u32,
    pub(crate) unknown_0c: [u8; 0x24],
    pub(crate) payload_ptr: usize,
}

pub(crate) const MENU_TASK_STATE_PAYLOAD_PTR_OFFSET: usize =
    core::mem::offset_of!(MenuTaskStateLayout, payload_ptr);
pub(crate) const MENU_TASK_STATE_DELAY_OFFSET: usize =
    core::mem::offset_of!(MenuTaskStateLayout, delay_bits);
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
pub(crate) const MENU_TRACE_UNSEEN_SEQ: usize = NULL_MODULE_BASE;
pub(crate) const POST_MAP_CONTINUATION_STATE_QWORD: usize = 2;
pub(crate) const TITLE_OWNER_SCAN_START_ADDRESS: usize = usize::MIN;
pub(crate) const TITLE_OWNER_QUERY_FAILED_BYTES: usize = usize::MIN;
pub(crate) const PAGE_PROTECTION_NO_FLAGS: u32 = 0;
pub(crate) const TITLE_OWNER_MIN_STATE: i32 = TitleStepState::Min as i32;
pub(crate) const TITLE_OWNER_MAX_STATE: i32 = TitleStepState::Finish as i32;
pub(crate) const TITLE_NATIVE_JOB_NOT_CALLED: usize = false as usize;
pub(crate) const TITLE_TRACE_SEQUENCE_INCREMENT: usize = 1;
#[repr(C)]
pub(crate) struct TitleNativeJobTaskData {
    pub(crate) unknown_00: [u8; 0x08],
    pub(crate) frame_delta: f32,
    pub(crate) unknown_0c: [u8; 0x04],
}

#[repr(u32)]
pub(crate) enum TitleNativeJobTiming {
    FrameRate = 60,
}

pub(crate) const TITLE_NATIVE_JOB_TASK_DATA_ZERO: u8 = false as u8;
pub(crate) const TITLE_NATIVE_JOB_TASK_DATA_BYTES: usize =
    core::mem::size_of::<TitleNativeJobTaskData>();
pub(crate) const TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR: f32 = true as u8 as f32;
pub(crate) const TITLE_NATIVE_JOB_FRAME_RATE: f32 = TitleNativeJobTiming::FrameRate as u32 as f32;
pub(crate) const TITLE_NATIVE_JOB_DELTA_OFFSET_START: usize =
    core::mem::offset_of!(TitleNativeJobTaskData, frame_delta);
pub(crate) const TITLE_NATIVE_JOB_DELTA_OFFSET_END: usize =
    TITLE_NATIVE_JOB_DELTA_OFFSET_START + core::mem::size_of::<f32>();
pub(crate) const TITLE_NATIVE_JOB_CALLED_VALUE: usize = true as usize;
#[repr(i32)]
pub(crate) enum TitleStepState {
    Min = 0,
    BeginLogo = 2,
    BeginTitle = 3,
    PlayGame = 5,
    MenuJobWait = 10,
    Finish = 11,
}

pub(crate) const TITLE_STEP_BEGIN_TITLE: i32 = TitleStepState::BeginTitle as i32;
pub(crate) const TITLE_STEP_PLAY_GAME: i32 = TitleStepState::PlayGame as i32;
pub(crate) const TITLE_STEP_MENU_JOB_WAIT: i32 = TitleStepState::MenuJobWait as i32;
/// STEP_BeginLogo (idx2, handler 0x140b0c2a0): the native press-any-button advance target.
/// The parked press-any-button screen is the FIRST state 10; the engine's own press handler
/// 0x140b0b6b0 issues SetState(owner, 2), then the native pump advances 2->3->10, building
/// the FULL main menu (Continue / Load-Game item d180 / New Game / ...). SetState(3)=BeginTitle
/// ALONE (skipping BeginLogo) only built the BackScreen (c000), not the main-menu items -- so
/// we replicate the full sequence by SetState(2) from our idx10 handler (zero-input, the
/// game's own SetState, not input synthesis). CAVEAT: STEP_BeginLogo hard-asserts the session
/// singleton 0x144588e98 at entry (0x140b0c2c3); only SetState(2) when that is non-null.
pub(crate) const TITLE_STEP_BEGIN_LOGO: i32 = TitleStepState::BeginLogo as i32;
/// STEP_BeginLogo list-build gate at [owner+0xb8]. STEP_BeginLogo 0x140b0c2a0 builds the FULL
/// main-menu list (Continue / Load-Game d180 / New Game / Settings / Quit) into owner+0xe0 via
/// the list builder 0x14081f180 ONLY when [owner+0xb8]==0; if it is SET, BeginLogo short-circuits
/// (@0x140b0c356) straight to SetState(3) and SKIPS the list build -- producing only the 3 title
/// composition items (BackScreen/Caption/dialog). That is why our prior SetState(2) never built
/// the main menu. We clear this gate before SetState(2) so BeginLogo runs the full build
/// (zero-input, menu-UI only -> save-safe). bd mainmenu-item-builder-into-iterator-tree-2026.
pub(crate) const TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, beginlogo_list_gate);
/// Cleared value (0) for the BeginLogo list-build gate [owner+0xb8].
pub(crate) const TITLE_OWNER_BEGINLOGO_GATE_CLEAR: u32 = false as u32;
/// owner+0xe0 = the menu-job/dialog holder (CS::TitleTopDialog built by BeginTitle).
pub(crate) const TITLE_OWNER_MENU_HOLDER_E0_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, menu_holder);
/// owner+0x130 = where STEP_BeginLogo COMMITS the main-menu list (Continue/Load d180/NewGame).
/// Decoded from the commit fn 0x140b0e530: `lea rcx,[owner+0x130]; call 0x1407a9460` stores the
/// 0x14081f180-built list there, then SetState(owner,10). So the Load-Game d180 item lives under
/// owner+0x130, NOT owner+0xe0 -- walk this to find/invoke it.
pub(crate) const TITLE_OWNER_MENU_LIST_130_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, menu_list);
/// Session singleton 0x144588e98 (RVA = abs - base). Asserted by STEP_BeginLogo(2) and the
/// MoveMapListStep load menu. Built by the boot/session bootstrap (may be non-null at the
/// splash-skipped parked title -- UNVERIFIED, hence read it live before SetState(2)).
/// RUNTIME-CONFIRMED non-null at the parked splash-skipped title (STAGE 1c).
pub(crate) const SESSION_SINGLETON_144588E98_RVA: usize =
    TitleSessionRva::SaveSafeBeginLogoSession as usize;
/// CS::TitleTopDialog "open main menu / populate entries" registrar 0x1409b24e0 (RVA
/// 0x9b24e0; file offset 0x9b1ae0 -- objdump-disasm-confirmed: `mov byte [rcx+0xa40],1;
/// add rcx,0xa60; lea rdx,desc 0x142b264f0; call set_state 0x1407499e0`). The press-any-button
/// title holder at owner+0xe0 (a CS::TitleTopDialog, vtable 0x142b26468) is built by BeginTitle
/// but left in the press-prompt state; this method sets the menu-opened latch [dialog+0xa40]=1,
/// advances the FD4 state machine at [dialog+0xa60] to the menu-list state, and
/// constructs+registers the Continue / Load-Game(d180) / New-Game MenuWindowJobs into the
/// holder. It is normally called from TitleTopDialog::update gated on the global accept byte
/// 0x144589bdc, but the registrar itself reads NO input -- calling it directly with rcx=dialog
/// is the zero-input menu-open (no input synthesis, no save write). (NB: a subagent first
/// reported the entry as 0x1409b1ae0 -- a foff->VA conversion slip of 0xa00; the disasm-verified
/// entry is 0x1409b24e0.)
#[repr(usize)]
pub(crate) enum TitleDialogRva {
    IsInState = 0x749b20,
    LiveDialogFactory = 0x81ead0,
    Cleanup = 0x9a8890,
    OpenMenu = 0x9b24e0,
    Vtable = 0x2b26468,
    ActiveScreenArray = 0x3d6d8d0,
}

pub(crate) const TITLE_TOP_DIALOG_OPEN_MENU_RVA: usize = TitleDialogRva::OpenMenu as usize;
/// CS::TitleTopDialog vtable 0x142b26468 (RVA). Verify [owner+0xe0][0]==base+this before
/// calling the registrar (wrong receiver would fault on [dialog+0xa38]/[+0xa60]).
pub(crate) const TITLE_TOP_DIALOG_VTABLE_RVA: usize = TitleDialogRva::Vtable as usize;
/// CS::TitleTopDialog::update (the per-frame title menu pump) = deobf 0x1409aac10 = vtable slot 2
/// (`*(vtable+0x10)`, verified by reading the deobf vtable + the prologue). `__fastcall(rcx =
/// TitleTopDialog*, xmm1 = f32 delta, r8 = *InputData)`. It runs each frame with the LIVE dialog and,
/// at its tail, calls MenuWindow::Update (the FD4 job pump) which drains the menu jobs. Hooking it
/// lets our in-context Continue build run in the pump's frame (live dialog fields) -- the timing our
/// game-task build lacked (mis-context crash). bd HOOK-DESIGN-titletopdialog-update-0x1409aac10.
pub(crate) const TITLE_TOP_DIALOG_UPDATE_RVA: usize = 0x9aac10;
/// CS::TitleTopDialog cleanup/destructor body 0x1409a8890 (RVA). Static disassembly shows it
/// first restores the TitleTopDialog vtable, calls native active-screen clear 0x1409b2db0, then
/// releases dialog-owned renderer/resources before tail-calling the base cleanup. Unlike the
/// deleting destructor wrapper 0x1409aa250, this helper does not free the object allocation; it is
/// a safer post-world cleanup candidate for stale title-logo/frontend state after PlayerIns is
/// already valid.
pub(crate) const TITLE_TOP_DIALOG_CLEANUP_RVA: usize = TitleDialogRva::Cleanup as usize;
/// CS::MenuWindow vtable 0x142a93a60 (.?AVMenuWindow@CS@@) (RVA). The live MenuWindow* the LIVE
/// Load-Game dialog factory needs as its rdx call-frame arg. Located by the active-screen scan.
pub(crate) const MENU_WINDOW_VTABLE_RVA: usize = 0x2a93a60;
/// CS::MenuWindowProxy vtable 0x142a94318 (RVA). The proxy variant of MenuWindow that the
/// active-screen array may hold instead of the concrete MenuWindow; either is a valid factory rdx.
pub(crate) const MENU_WINDOW_PROXY_VTABLE_RVA: usize = 0x2a94318;
/// Active-screen array 0x143d6d8d0 (RVA): the per-frame pump 0x1409aa680 iterates it. 10 contiguous
/// screen* slots (stride 8). The LIVE-dialog scan reads each slot's [scr] vtable to find the live
/// TitleTopDialog and MenuWindow (the factory's SceneProxy capture + rdx) -- no blind heap scan.
pub(crate) const ACTIVE_SCREEN_ARRAY_RVA: usize = TitleDialogRva::ActiveScreenArray as usize;
#[repr(C)]
pub(crate) struct ActiveScreenArrayLayout {
    pub(crate) slots: [usize; 10],
}

/// Active-screen array slot count (bounded scan; the native pump iterates the same span).
pub(crate) const ACTIVE_SCREEN_ARRAY_SLOTS: usize =
    core::mem::size_of::<ActiveScreenArrayLayout>() / core::mem::size_of::<usize>();
/// Active-screen array slot stride (one screen* per slot).
pub(crate) const ACTIVE_SCREEN_ARRAY_STRIDE: usize = core::mem::size_of::<usize>();
/// Scan slot start / step.
pub(crate) const ACTIVE_SCREEN_SLOT_START: usize = usize::MIN;
pub(crate) const ACTIVE_SCREEN_SLOT_STEP: usize = true as usize;
/// PROBE-2 GROUND TRUTH (2026-06-18, runtime, REFUTES the static group->holder->screen walk):
/// the 10 slots of the active-screen array 0x143d6d8d0 each hold a menu MODEL RENDERER (vtable
/// 0x142b80128 CSMenuProfModelRend / 0x142b7f310 CSMenuAsmModelRend), NOT screen/group controllers,
/// so the +0xa8 holder / +0x48 screen walk leads nowhere. That walk (and the MENU_GROUP_* /
/// MENU_HOLDER_* offsets it used) is removed. What IS runtime-reliable: TitleTopDialog at owner+0xe0
/// (vtable-gated, TITLE_TOP_DIALOG_VTABLE_RVA). The live MenuWindow* is NOT statically pinned; it is
/// read DETERMINISTICALLY by `locate_live_loadgame_node` from the SceneProxy back-ref at proxy+0x20.
///
/// Field-scan stride: one qword pointer per step (also the SceneProxy diagnostic scan stride).
pub(crate) const FIELD_SCAN_STRIDE: usize = 8;
/// Partial TitleTopDialog layout for the menu-driver fields this crate reads.
#[repr(C)]
pub(crate) struct TitleTopDialogLayout {
    pub(crate) unknown_000: [u8; 0xa38],
    pub(crate) scene_proxy_capture: usize,
    pub(crate) menu_opened: u8,
    pub(crate) unknown_a41: [u8; 0x07],
    pub(crate) row_registry: usize,
    pub(crate) unknown_a50: [u8; 0x10],
    pub(crate) state_machine: usize,
}

/// TitleTopDialog SceneProxy capture slot: [dialog+0xa38] holds the live SceneProxy* the
/// TitleTopDialog ctor 0x1409a81a0 stored at 0x1409a8213. The LIVE-dialog factory 0x14081ead0
/// reads the SceneProxy from [rcx], so we pass rcx = dialog+0xa38 (factory r8 = *(dialog+0xa38)).
pub(crate) const DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, scene_proxy_capture);
/// CS::ProfileLoadDialog build factory 0x14081ead0 (RVA). Called as
/// `extern "system" fn(rcx = dialog+0xa38, rdx = MenuWindow*) -> dialog*` to build + register the
/// LIVE ProfileLoadDialog (vtable 0x142b229f8) into the active-screen set + menu group.
pub(crate) const LIVE_DIALOG_FACTORY_RVA: usize = TitleDialogRva::LiveDialogFactory as usize;
/// CONVERGED ACQUISITION RECIPE (2026-06-18, bd live-dialog-menuwindow-via-sceneproxy-backref-0x20):
/// the live MenuWindow* (factory rdx) is read DETERMINISTICALLY from the SceneProxy we already hold
/// at [td+0xa38] -- NOT via the menu MANAGER. CS::SceneObjProxy ctor 0x14074a700 does
/// `mov [proxy+0x20], rbx` where rbx is the MenuWindow (0x14074a735), so the back-ref lives at
/// proxy+0x20. The dead menu-manager/registry/menu-step scans (and the owner/dialog field scans) are
/// removed. CS::SceneObjProxy vtable 0x142a94a70 (RVA): require *(proxy) == base+this before reading
/// the +0x20 back-ref; LOG *(proxy) regardless (self-diagnostic).
pub(crate) const SCENE_OBJ_PROXY_VTABLE_RVA: usize = 0x2a94a70;
/// SceneProxy MenuWindow back-ref: the live MenuWindow* sits at proxy+0x20 (ctor 0x14074a735).
pub(crate) const SCENE_PROXY_MENU_WINDOW_20_OFFSET: usize = 0x20;
/// Generic CS::SceneObjProxy context/back-ref slot. The named-child constructor 0x14074a7c0
/// copies `[parent+0x20]` into `[proxy+0x20]` before binding the child by name into the proxy's
/// handle at +0x28. Used for the title `PressStart` / GFX `PRESS BUTTON` component gate.
pub(crate) const SCENE_OBJ_PROXY_CONTEXT_20_OFFSET: usize = 0x20;
/// TitleTopDialog embedded CS::SceneObjProxy for the title prompt component. Static evidence:
/// 05_000_title.gfx contains the visible text `PRESS BUTTON` and symbol `PressStart`; the
/// TitleTopDialog constructor xref at 0x1409a8275 calls the named-child proxy constructor with
/// rdx=dialog+0xb78 and r8="PressStart" (RVA 0x2b26500).
pub(crate) const TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET: usize = 0xb78;
pub(crate) const TITLE_PRESS_START_NAME_RVA: usize = 0x2b26500;
/// Diagnostic span: if *(proxy) is NOT SceneObjProxy, scan [proxy .. proxy+0x40] stride 8 logging
/// each qword + its [0] vtable so the next probe reveals the real layout. Also bounds the fallback.
pub(crate) const SCENE_PROXY_DIAG_SCAN_SPAN: usize = 0x40;
/// FD4 StateMachine sub-object EMBEDDED at dialog+0xa60. NB: the registrar / set_state /
/// is_in_state receiver is the ADDRESS dialog+0xa60 (they do `add rcx,0xa60; call`), NOT
/// `*(dialog+0xa60)`. Its first qword is the SM vtable.
pub(crate) const TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, state_machine);
/// Byte latch at [dialog+0xa40]: 0 = menu not opened (the native non-input registrar path
/// requires it ==0), 1 = registrar ran. We READ it (never write/clear it -- pre-setting it
/// poisons the native non-input open path, bd titletopdialog-loop-ready-gate-2026).
pub(crate) const TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, menu_opened);
/// Mask to extract the latch byte from an 8-byte read at dialog+0xa40.
pub(crate) const TITLE_TOP_DIALOG_LATCH_BYTE_MASK: usize = u8::MAX as usize;
/// CS FD4 `is_in_state(rcx = sm-receiver = dialog+0xa60, rdx = state descriptor ptr) -> bool`
/// (0x140749b20). Returns true iff the SM's CURRENT node is SETTLED (flags&0x8f>=2) AND its name
/// matches the descriptor's inline ASCII name. We call the game's own checker to read the live
/// state by NAME -- robust, no hand pointer-chase / SSO parsing.
pub(crate) const TITLE_TOP_DIALOG_IS_IN_STATE_RVA: usize = TitleDialogRva::IsInState as usize;
/// FD4 state name-descriptor RVAs (inline ASCII at the VA). FadeIn = the intro-fade node;
/// Loop = the settled press-prompt node (the correct gate to open the menu); TextFadeOut = the
/// menu-list-active node the registrar transitions to. bd titletopdialog-fadein-gate-...-2026.
pub(crate) const TITLE_STATE_DESC_FADEIN_RVA: usize = 0x2a90500;
pub(crate) const TITLE_STATE_DESC_LOOP_RVA: usize = 0x2a8f9e8;
pub(crate) const TITLE_STATE_DESC_TEXTFADEOUT_RVA: usize = 0x2b264f0;
/// Boolean-false byte returned by the game's `is_in_state` (compare `!= this` for true).
pub(crate) const OWN_STEPPER_FALSE: u8 = false as u8;
/// Initial value (0) for the open-menu registrar one-shot guard.
pub(crate) const OWN_STEPPER_MENU_OPENED_NO: usize = OWN_STEPPER_FALSE as usize;
/// STAGE1d probes the dialog's FD4 state immediately and opens only on the semantic Loop+latch
/// predicate; it does not use a fixed pre-probe settle delay.
/// Interval (frames) for logging the state probe (FadeIn/Loop/TextFadeOut + latch), so the log
/// shows the dialog progressing without spamming every frame.
pub(crate) const STAGE1D_RETRY_INTERVAL: u64 = 30;
/// Set by cap_sequence_iter_hook when the Sequence iterator first walks a MenuWindowJob child
/// (vt 0x142aa97e8) -- i.e. the main menu actually opened and its entries are registered. The
/// retry loop stops once this is set (the title views tick via a different pump, so this fires
/// ONLY on the real main-menu entries).
pub(crate) const MENU_ENTRIES_SEEN_NO: usize = false as usize;
pub(crate) const MENU_ENTRIES_SEEN_YES: usize = true as usize;
pub(crate) static MENU_ENTRIES_SEEN: AtomicUsize = AtomicUsize::new(MENU_ENTRIES_SEEN_NO);
pub(crate) static OWN_STEPPER_MENU_OPENED: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(OWN_STEPPER_MENU_OPENED_NO);
/// Count of TitleTopDialog entry-vector dumps emitted (the Continue/Load-Game rows live there,
/// not in the FD4 tree). Capped so the diagnostic samples the entries as they realize after
/// menu-open without spamming the log every frame.
pub(crate) static OWN_STEPPER_TITLETOP_DUMPS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Max TitleTopDialog entry dumps + the per-dump frame interval.
pub(crate) const OWN_STEPPER_TITLETOP_DUMP_CAP: usize = TraceSampleLimit::Value8 as usize;
/// Recon-only Load-Game fingerprint scan (`scan_dialog_for_loadgame`) counter + cap. Runs in the
/// post-open SAFE-DEFAULT park, independent of the pre-open `OWN_STEPPER_TITLETOP_DUMPS` (which the
/// d180-locate exhausts before menu-open). 2026-06-18 reconciliation: the title rows are
/// TitleTopDialog registry entries, not FD4 jobs.
pub(crate) static OWN_STEPPER_LOADGAME_SCANS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const OWN_STEPPER_LOADGAME_SCAN_CAP: usize = TraceSampleLimit::Value12 as usize;
/// Sentinel logged when the inner TitleStep owner can no longer be found (the
/// title flow advanced past the title and the owner was finalized/destructed).
pub(crate) const TITLE_STATE_OWNER_GONE: i32 = -1;
pub(crate) const FORCE_PLAY_GAME_STATE_UNOBSERVED: i32 = -999;
/// One-shot "PlayGame requested" flag on the TitleStep owner. STEP_PlayGame only
/// runs its real load-trigger (`consume_owner300` 0x140ca89e0 on owner+0x300,
/// gated at 0x140b0d70c) when this byte is nonzero, then clears it. The menu
/// "Continue" selection normally sets it; we set it so the forced PlayGame step
/// actually starts the load instead of resetting via GameStepWait.
pub(crate) const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, play_game_request_flag);
pub(crate) const TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET: u8 = true as u8;
/// The save slot STEP_PlayGame actually loads. Its handler (0x140b0d5b0) reads
/// `mov eax,[owner+0xbc]` and feeds it through submit -> validate -> pair, which
/// writes the value to GameMan+0x14 (the load value). The +0xac0 save slot only
/// feeds global+0x1200, not the load pair — so this is the field to select.
pub(crate) const TITLE_OWNER_PLAY_GAME_SLOT_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, play_game_slot);
/// STEP_GameStepWait (handler 0x140b0cde0) waits on the load job at owner+0x2e8:
/// `cmp dword [job+0xd8],0 / jne wait`. Observe job+0xd8 while holding here to
/// learn whether anything drains the job (needs a pump) or it is static.
pub(crate) const TITLE_STEP_GAME_STEP_WAIT: i32 = 6;
pub(crate) const TITLE_OWNER_JOB_OFFSET: usize = core::mem::offset_of!(TitleOwnerLayout, load_job);
pub(crate) const TITLE_OWNER_JOB_PENDING_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLoadJobLayout, pending);
pub(crate) const TITLE_JOB_OBSERVE_TICK_INTERVAL: u64 = 30;
pub(crate) const FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA: usize =
    er_save_loader::SET_SAVE_SLOT_RVA as usize;
/// Corrected play-game submit recipe (play-game-submit-and-continue-load-recipe-2026):
/// the Continue/Load handler 0x140b0e180 sets owner+0xbc to a PACKED MAP id, clears
/// the new-game flag owner+0x284, and calls SetState 0x140b0d960(owner, 5=PlayGame)
/// -- then the existing pump runs PlayGame -> child MoveMap_Init -> builds CSFeMan.
/// (force_play_game wrote owner+0x4c=5 raw + a raw slot in +0xbc, so it orphaned.)
pub(crate) const TITLE_SET_STATE_RVA: usize = 0xb0d960;
pub(crate) const TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, new_game_flag);
/// Packed map id for m60_42_34_00 (the new-game default; resolver 0x14071fd60 packs
/// mAA_BB_CC_DD decimal -> byte3=AA..byte0=DD). A valid map to pass the PlayGame
/// map-area gate (area byte 0x32..0x58) while we prove the SetState(5) path builds
/// CSFeMan; the real slot map comes from GameMan+0xc30 once peeked.
pub(crate) const DEFAULT_PLAY_GAME_MAP: i32 = 0x3c2a2200;
/// Full sync slot deserialize 0x14067b290(ecx=slot) -- CSFeMan-LESS (verified): reads
/// the save, writes the real saved map to GameMan+0xc30, applies the character. The
/// cycle-breaker for slot loading (slot9-load-phase-machine-b80-csfeman-less-2026).
pub(crate) const DESERIALIZE_SLOT_RVA: usize = 0x67b290;
/// The title menu's CONTINUE row wrapper 0x14082bac0 calls this native loader as
/// `continue_load(-1, 0, 0)`: it resolves `-1` through GameMan+0xac0, submits the
/// 0x280000 save read, and arms GameMan+0xb80 for the b80 drain/deser chain.
pub(crate) const CONTINUE_LOAD_RVA: usize = 0x67b750;
/// Private saved-map slot inside the GameMan block immediately after
/// `stay_in_multiplay_area_saved_rotation`; derive it from the adjacent typed
/// vector layout instead of retaining the raw absolute field offset.
pub(crate) const GAME_MAN_SAVED_MAP_C30_OFFSET: usize =
    core::mem::offset_of!(GameMan, stay_in_multiplay_area_saved_rotation)
        + core::mem::size_of::<F32Vector4>()
        + core::mem::size_of::<F32Vector4>();
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
/// play_game_submit-handoff discriminators on the InGameStep object (own-load-worldreswait-is-block-
/// registration-not-coord-2026-06-22). play_game_submit 0x140aebdc0 sets InGameStep+0xd8=1 and
/// InGameStep+0x100=requested BlockId. So reading these PURELY (no call) tells us whether the native
/// request handoff ran: +0x100 == the saved BlockId (e.g. 0x1c000000) means play_game_submit primed
/// the m28 request; +0x100 == 0/unset means it did NOT. +0xd8 is the matching pending phase byte.
pub(crate) const INGAMESTEP_PHASE_D8_OFFSET: usize = 0xd8;
pub(crate) const INGAMESTEP_REQ_BLOCKID_100_OFFSET: usize = 0x100;
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
/// The save-data subsystem gate the c30-writer 0x67bd70 checks before it writes
/// GameMan+0xc30: `[0x143d68078]` (RVA 0x3d68078). It is a 0x270-byte heap object
/// built by the save-load boot 0x6798d0..0x679904 and zeroed on teardown 0x6789bf.
/// If null at the writer's entry, 0x67bd70 returns without writing c30 (gate (a) in
/// the c30-stays-default diagnosis). The save-safe c30-writer probe logs this.
pub(crate) const SAVE_DATA_SUBSYSTEM_GATE_RVA: usize = 0x3d68078;
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
pub(crate) const SESSION_SINGLETON_A_RVA: usize = TitleSessionRva::SessionA as usize;
pub(crate) const SESSION_SINGLETON_B_RVA: usize = TitleSessionRva::SessionB as usize;
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
/// MISIDENTIFIED-CORRECTED (autoresearch 2026-06-18): 0x4842d40 is upstream `eldenring`'s
/// `runtime_heap_allocator` (the `DLAllocator` singleton, `rva::get().runtime_heap_allocator`),
/// confirmed by static RE -- it has 4057 RIP-relative refs (allocator footprint, not a task) and
/// the cached-singleton getter at 0x140078ed5. It is built at startup and is ALWAYS non-null, so
/// reading it non-null is NOT evidence that any "world-stream worker"/FD4 stream task was built.
/// The save-IO/worldres "worker present" levers below that relied on that inference are FALSE
/// POSITIVES and need the real stream-task RVA. Name kept generic and accurate; see bd
/// `rva-4842d40-is-heap-allocator-not-stream-task`.
pub(crate) fn runtime_heap_allocator_ptr_or_null() -> usize {
    DLAllocator::runtime_heap_allocator() as *const DLAllocator as usize
}
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
/// Recurring-observer aliases for the same resmgr block-array layout, named per the registration-vs-
/// streaming probe (own-load-worldreswait-is-block-registration-not-coord-2026-06-22). Defined as
/// aliases so the offsets live in exactly one place (no duplicated magic numbers). The recurring
/// observer scans base_arr = resmgr + RESMGR_BLOCK_ARRAY_B3030_OFFSET, stride 8, and for each
/// non-null block reads inner = *(block + BLOCK_INNER_8_OFFSET) then areaId = *(inner +
/// BLOCK_AREA_C_OFFSET) as u8 -- PURE READS, NO block->vtable call this round.
pub(crate) const RESMGR_BLOCK_ARRAY_B3030_OFFSET: usize = WORLDRES_BLOCK_ARRAY_B3030_OFFSET;
pub(crate) const BLOCK_INNER_8_OFFSET: usize = BLOCK_ENTRY_AREAOBJ_8_OFFSET;
pub(crate) const BLOCK_AREA_C_OFFSET: usize = BLOCK_AREAOBJ_AREA_C_OFFSET;
/// The target areaId is DERIVED from the requested block coord (wrm+0x2c / req_coord), not
/// hardcoded: areaId = (block_coord >> TARGET_AREA_FROM_COORD_SHIFT) & TARGET_AREA_FROM_COORD_MASK.
/// For the m28 save the low dword is 0x1c000000 so this yields 0x1c, but the value is data-driven.
pub(crate) const TARGET_AREA_FROM_COORD_SHIFT: u32 = 24;
pub(crate) const TARGET_AREA_FROM_COORD_MASK: u32 = 0xff;
/// Cap the recurring observer's block-array scan at min(block_count, this) for safety.
pub(crate) const OBSERVER_BLOCK_SCAN_CAP: i64 = 64;
/// How many distinct areaIds the observer collects for the log line.
pub(crate) const OBSERVER_AREAID_SAMPLE_MAX: usize = 8;
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
/// OWN-LOAD m28 direct-enqueue lever (adddefaultfileloadprocess-lever-viable-2026-06-22).
/// `FD4::FD4FileCap::AddDefaultFileLoadProcess` deobf VA 0x142658c60 (prologue-grounded
/// `40 55 56 57 41 56 41 57`; dump 0x142658c50 is +0x10). Stored as an RVA offset from the
/// 0x140000000 image base, resolved at runtime as `module_base + RVA` like the other native-call
/// RVAs in this file (e.g. `CONTINUE_CONFIRM_RVA`). Signature (Win64 fastcall):
/// `bool AddDefaultFileLoadProcess(FD4FileCap* cap /*rcx*/, FD4FileLoadProcess* loadProcess /*rdx*/)`.
/// It builds the FD4FileLoadProcessor internally + self-enqueues IO to the already-live FD4 workers
/// (RequestDCX -> RSResourceFileRequest -> GLOBAL_LoadManager). PushTask / AssignFileCap are NOT
/// needed. Reaches ONLY world-asset file-load streaming -- no save IO, cannot autosave.
pub(crate) const ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60 - 0x140000000;
/// FD4FileCap layout (struct len 0x90): the cap's EXISTING `FD4FileLoadProcess*` lives at +0x78 --
/// READ it for arg2, we never construct one. `loadState` at +0x88 == 4 means the cap is already
/// resident (skip). Both grounded in the Ghidra dump decomp of the lever.
pub(crate) const FILECAP_LOAD_PROCESS_78_OFFSET: usize = 0x78;
pub(crate) const FILECAP_LOADSTATE_88_OFFSET: usize = 0x88;
/// `loadState` sentinel meaning the FD4FileCap finished loading (already resident -> do not dispatch).
pub(crate) const FILECAP_LOADSTATE_COMPLETE: i32 = 4;
/// WorldBlockRes holds the m28 area's FD4FileCap(s): the primary at +0x40 and an OPTIONAL second at
/// +0x48 (the IsNonDebugArea branch; m28/0x1c populates both, and phase-2 gates on BOTH). Dispatch
/// each non-null cap. These are off the SAME WorldBlockRes entry the recurring observer block-walk
/// already finds for the player area.
pub(crate) const WORLDBLOCKRES_FILECAP_40_OFFSET: usize = 0x40;
pub(crate) const WORLDBLOCKRES_FILECAP2_48_OFFSET: usize = 0x48;
/// The resmgr 0xb3030 array entry `block` is a CONTAINER (WorldBlockData): the WorldBlockRes elements
/// live in an inline array at `*(block+0xce0)`, count `*(block+0xcd8)` (i32), stride 0xb98 -- decoded
/// from the keyed getter vt+0x8 (deobf 0x14062f470): `movslq 0xcd8(rcx); mov 0xce0(rcx),r11;
/// elem=r11+i*0xb98`. Each element is a WorldBlockRes (phase byte +0x35, caps +0x40/+0x48). We iterate
/// this array DIRECTLY (plain reads) instead of calling the getter -- the getter takes a second `key`
/// arg in rdx and AV-crashes if called without it.
pub(crate) const WORLDBLOCK_CONTAINER_COUNT_CD8_OFFSET: usize = 0xcd8;
pub(crate) const WORLDBLOCK_CONTAINER_ARRAY_CE0_OFFSET: usize = 0xce0;
pub(crate) const WORLDBLOCKRES_ELEM_STRIDE_B98: usize = 0xb98;
pub(crate) const DIAG_PHASE_NONE: i32 = -1;
pub(crate) const DIAG_COUNT_ZERO: i32 = 0;
pub(crate) const DIAG_COUNT_ONE: i32 = 1;
pub(crate) const DIAG_SAMPLE_ZERO: u32 = 0;
/// Global holding the GameMan pointer (`mov rax,[rip]` in set_save_slot 0x67a810
/// / save_slot_get 0x678ca0). Read-only diagnostics of the PlayGame load-pair
/// preconditions read GameMan through this.
pub(crate) fn game_man_ptr_or_null() -> usize {
    GameMan::instance_ptr().map_or(NULL_MODULE_BASE, |ptr| ptr as usize)
}
/// GameMan `save_slot` (compiler-verified equal to the upstream typed field).
pub(crate) const FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET: usize =
    core::mem::offset_of!(GameMan, save_slot);
/// Save-manager load-in-progress flag (GameMan/save-mgr singleton 0x143d69918):
/// `0x14067b570` sets `[mgr+0xb80]=1` when it begins the load and clears it to 0
/// when finished. The native autoload (recipe A) arms the load by setting the
/// slot (`+0xac0`) and the force flag `0x143d856a0`, then the save-manager
/// per-frame update `0x14067f5d0` performs it.
/// Bound to upstream `GameMan::save_state` (compiler-verified equal to our offset); our research
/// reads this same dword as the load-in-progress lane (set 1 on load begin, cleared on finish).
pub(crate) const GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET: usize =
    core::mem::offset_of!(GameMan, save_state);
/// Read-only autoload-arm precondition probe. The native save-mgr update
/// 0x14067f5d0 arms autoload (sets GameMan+0xb72=1 -> load) only when its gates
/// pass; the one runtime unknown is whether the slot-record container
/// [slotmgr+0x8] is populated at the pre-bootstrap title. These RVAs/offsets let
/// us read those preconditions without touching state.
/// Alias for the GameDataMan singleton RVA: the "slot manager" the save-snapshot probe reads IS
/// GameDataMan. Reference the canonical const so the RVA is decoded in exactly one place.
pub(crate) fn game_data_man_ptr_or_null() -> usize {
    GameDataMan::instance_ptr().map_or(NULL_MODULE_BASE, |ptr| ptr as usize)
}
/// GameDataMan -> main player save data (compiler-verified equal to the upstream typed field).
pub(crate) const SLOT_MANAGER_DATA_OFFSET: usize =
    core::mem::offset_of!(GameDataMan, main_player_game_data);
/// GameDataMan private tail fields used by the save/profile probes.
#[repr(C)]
pub(crate) struct GameDataManProfileSummaryLayout {
    pub(crate) unknown_000: [u8; 0x78],
    pub(crate) profile_summary: usize,
}

/// GameDataMan -> `profile_summary`; private upstream, but documented locally as a typed layout.
pub(crate) const SLOT_MANAGER_CONTAINER_OFFSET: usize =
    core::mem::offset_of!(GameDataManProfileSummaryLayout, profile_summary);
pub(crate) const CSFEMAN_SINGLETON_RVA: usize = 0x3d6b880;
/// Session manager singleton (absolute 0x1447ef360; NULL at the title, built by
/// the move-map/load path). RVA = 0x1447ef360 - 0x140000000 = 0x47ef360.
pub(crate) const SESSION_SINGLETON_RVA: usize = TitleSessionRva::MoveMapSession as usize;
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
/// Continue/Load MANAGER object global (.data abs 0x143d5df38; ==0 at rest in the deobf
/// image, built at runtime). `[mgr]` = the manager vtable, `[mgr + 8]` = the recipe's
/// literal "owner" used by the native-fullread COMMIT recipe. Used READ-ONLY here for the
/// OWN-LOAD owner diagnostic; the continue_confirm owner is the threaded SetState-able
/// title owner (see `own_load_continue_fire`), NOT this literal.
pub(crate) const CONTINUE_MANAGER_GLOBAL_RVA: usize = 0x3d5df38;

/// LoadGame-JOB BUILD factory (`FUN_140826510` live; dump VA `0x140826600` lands +0xF0 mid-instr in
/// the deobf image -- the real prologue is here, prologue-grounded vs `eldenring-deobf.bin`). Builds
/// the LoadGame `CS::MenuJobWithContext<LoadJobContext>` via the menu-heap factory and returns it in
/// `*out` with refcount 1. Win64 fastcall `(out: *DLRefCountPtr<MenuJob>, ctx_parent, save_slot:i32,
/// owner_ctx)`. Only `out` (our local) and `save_slot` (the int slot) are required by the deser/map
/// self-build path; `ctx_parent`/`owner_ctx` are the OUTER profile-selection UI context (stored as
/// lambda captures, every build-path deref null-guarded) -- passed as 0 here (see `own_load_install_job`).
pub(crate) const LOADGAME_JOB_BUILD_RVA: usize = 0x826510;
/// `DLUT::DLReferenceCountPointer<MenuJob>` ASSIGN/INSTALL helper (`FUN_1407a9560` live; dump entry
/// `0x1407a9650` is the same fn, prologue-grounded vs `eldenring-deobf.bin`). Win64 fastcall
/// `(slot: *MenuJob*, src: *MenuJob* (longlong*))`: writes `*slot = *src`, `AtomicIncrement`s the new
/// occupant, then `AtomicDecrement`s/releases the PRIOR occupant and zeroes `*src` (move-assign).
/// Installs the built job into `owner+0x130`, releasing the idle `IfElseJob` it replaces.
pub(crate) const MENUJOB_ASSIGN_RVA: usize = 0x7a9560;
/// `CS::MenuJobQueue::PushBackJob` (live entry `0x1407a9250` -- prologue-grounded vs eldenring-deobf.bin:
/// `mov [rsp+0x10],rdx; push rdi; sub rsp,0x30; movq $-2,[rsp+0x20]`; dump `FUN_1407a9340`). CORRECTED
/// from the prior `0x7a9254`, which was +4 INTO the first instruction (mid-`mov`) and would execute
/// garbage -- a latent bug that likely helped kill the gated `own_load_install_job` path. APPENDS a job
/// into a MenuJobQueue's auto-growing deque ring (`AtomicIncrement`s the job, ring-push behind the
/// active job) -- does NOT replace the active job or zero `*src`, and is overflow-safe (NOT the cap-8
/// FixOrderJobSequence). Win64 fastcall `(rcx = queue_base, rdx = src: *MenuJob* (a DLReferenceCount
/// Pointer slot whose [0] is the job))`. Queue targets: `owner+0x130` (ring +0x138, count +0x178;
/// STEP_MenuJobWait's ExecuteMenuJob ticks it) OR `dialog+0x10` (ring +0x18; the per-frame menu pump
/// 0x1409aa680 over the active-screen array drains it -- the native Continue post target).
/// bd continue-load-POST-primitive-pushbackjob-kick-2026-06-22.
pub(crate) const MENUJOB_PUSHBACK_RVA: usize = 0x7a9250;
/// MenuJobQueue field offsets (for diagnostics): the queued-job ring count at +0x178 grows by 1 on a
/// successful PushBackJob; the active job stays at +0x130.
pub(crate) const MENUJOB_QUEUE_COUNT_178_OFFSET: usize = 0x178;
/// The MenuJob slot `CS::TitleStep::STEP_MenuJobWait` ticks every frame via
/// `ExecuteMenuJob((MenuJob**)&owner->field85_0x130, &time)`. Installing the LoadGame job here makes
/// the per-frame title step drive it (self-build -> deser -> world stream). Owner-relative byte offset.
pub(crate) const TITLE_OWNER_MENUJOB_SLOT_130_OFFSET: usize = 0x130;
/// LoadGame `MenuJobWithContext<LoadJobContext>` vtable (dump VA `0x142ac71e0`). DIAGNOSTIC ONLY: the
/// installed job's vtable should read back as this (modulo the dump->live `.rdata` shift) -- logged,
/// never used to gate the call. The IfElseJob it replaces reads vtable dump `0x142aa2958`.
pub(crate) const MENUJOB_LOADGAME_VTABLE_DUMP_VA: usize = 0x142ac71e0;
/// Idle title `CS::MenuJobSequence::IfElseJob` vtable (dump VA `0x142aa2958`) that occupies
/// `owner+0x130` before install. DIAGNOSTIC ONLY (logged for the before/after vtable-flip evidence).
pub(crate) const MENUJOB_IFELSE_VTABLE_DUMP_VA: usize = 0x142aa2958;
/// MenuJob `+0x68` built-flag byte (0 before first Run tick, 1 after self-build) and `+0x70` inner
/// FixOrderJobSequence ptr (0 -> built). DIAGNOSTIC ONLY: dumped before/after to witness self-build.
pub(crate) const MENUJOB_BUILT_FLAG_68_OFFSET: usize = 0x68;
pub(crate) const MENUJOB_INNER_SEQ_70_OFFSET: usize = 0x70;
/// `CS::FixOrderJobSequence::currentJobIndex` (`+0x10`) on the IfElseJob/inner seq -- advances as the
/// job sequence steps. DIAGNOSTIC ONLY (dumped before/after install).
pub(crate) const MENUJOB_CURRENT_JOB_INDEX_10_OFFSET: usize = 0x10;

// ===== PATH B: PRIVATE-PUMP "own the load" (own_load_pump) =====
// Static-verified 2026-06-22 against the runtime dump (PathBVerify*.java, /home/banon/ghidra_maporch/
// pathb*_verify_out.txt). The menu-free alternative to BOTH owner+0x130 install (a proven dead end --
// owner+0x130 is the title IfElseJob, only ticked by STEP_MenuJobWait/CSMenuMan-dialog pumps) AND the
// SetState5-only continue (reached the loading screen but never mounted m28). We BUILD the LoadGame job
// with REAL mss-derived ctx, then PRIVATELY pump its Run every frame from our recurring game task until
// it self-builds + deserializes + map-streams (m28 mount), THEN drive the title->ingame transition.

/// LoadGame `CS::MenuJobWithContext<LoadJobContext>::Run` (vtable+0x10). Live entry 0x140826e10
/// (dump `FUN_140826e40` lands inside; prologue-grounded vs `eldenring-deobf.bin`). Win64 fastcall
/// `Run(this /*rcx*/, result: *MenuJobResult /*rdx*/, time: *FD4Time /*r8*/, param4 /*r9*/) -> *MenuJobResult`.
/// On the first tick (`*(this+0x68)==0`) it builds the inner deser->map FixOrderJobSequence
/// (`FUN_140828360`) and ticks it; thereafter it forwards to the inner seq's Run. It READS the f32
/// frame delta at `time+8` and writes the FD4Time vtable into `*time`; it does NOT read `time+0`.
pub(crate) const LOADGAME_JOB_RUN_RVA: usize = 0x826e10;
/// `CS::MenuJobResult` size + layout (dump `/auto_structs/MenuJobResult` len 8): `+0x0 MenuJobState
/// state` (4 bytes), `+0x4 undefined4` (the inner deser sub-code 5/2/6 lands here via `param_2[1]`).
/// Pass a zero-init 8-byte buffer as `result`; read `state` (+0x0) for the done condition.
pub(crate) const MENUJOB_RESULT_SIZE: usize = 0x8;
pub(crate) const MENUJOB_RESULT_STATE_0_OFFSET: usize = 0x0;
pub(crate) const MENUJOB_RESULT_SUBCODE_4_OFFSET: usize = 0x4;
/// `CS::MenuJobState` enum (dump `/auto_structs/MenuJobState` len 4): Continue=1, Success=2, Failed=3.
/// `MenuJobResult::ShouldContinue` (0x1407a92f0) is exactly `Continue < state`, i.e. done == state>1.
pub(crate) const MENUJOB_STATE_CONTINUE: i32 = 1;
pub(crate) const MENUJOB_STATE_SUCCESS: i32 = 2;
pub(crate) const MENUJOB_STATE_FAILED: i32 = 3;
/// `FD4::FD4Time` size (dump `/FD4/FD4Time` len 16): `+0x0 vtable ptr`, `+0x8 f32 time` (the frame
/// delta the map-stream sub-job advances on). Run only READS `time+8`. Pass a 16-byte buffer with the
/// f32 frame delta at +8 (a zeroed buffer => delta 0.0 is valid; the deser self-builds regardless).
pub(crate) const FD4_TIME_SIZE: usize = 0x10;
pub(crate) const FD4_TIME_DELTA_8_OFFSET: usize = 0x8;
/// GameDataMan singleton global (.data abs `0x143d5df38`, == `CONTINUE_MANAGER_GLOBAL_RVA` deref base).
/// `GetMenuSystemSaveLoad() = GLOBAL_GameDataMan->menuSystemSaveLoad`, i.e. `mss = *(*(base+RVA)+0x60)`.
pub(crate) const GAME_DATA_MAN_GLOBAL_RVA: usize = 0x3d5df38;
/// `GameDataMan->menuSystemSaveLoad` field offset (`mss = *(GameDataMan + 0x60)`).
pub(crate) const GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET: usize = 0x60;
/// LoadGame build factory REAL ctx args (golden Continue trace): `ctx_parent = mss + 0x50`,
/// `owner_ctx = *(mss + 0xa38)` (CS::TitleFlowContext). Non-null real ctx; the prior ctx=0 build AV'd
/// when the outer profile-selection sub-job dereffed the captured null.
pub(crate) const MSS_CTX_PARENT_50_OFFSET: usize = 0x50;
pub(crate) const MSS_OWNER_CTX_A38_OFFSET: usize = 0xa38;
/// CORRECTED LoadGame build ctx args (static RE 2026-06-22, triple-verified: golden factory site
/// `0x1409ac9cb mov 0xa38(%r13),%r9` where r13 == the live TitleTopDialog; TitleTopDialog ctor
/// `0x1409a82d0` populates +0xa38; live-SWBP capture `r9=owner+0x138 rdx=dialog+0x50`). The owner
/// context + parent come from the LIVE `CS::TitleTopDialog`, NOT from `CSMenuSystemSaveLoad`
/// (`mss+0xa38` was a red herring -- r13 was misidentified as mss). `ctx_parent = dialog+0x50`,
/// `owner_ctx = *(dialog+0xa38)` (a `CS::TitleFlowContext*`, written UNCONDITIONALLY by the dialog
/// ctor -> valid at the settled press-any-button title, unlike `mss+0xa38` which read back garbage).
/// bd `loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22`.
pub(crate) const DIALOG_CTX_PARENT_50_OFFSET: usize = 0x50;
pub(crate) const DIALOG_OWNER_CTX_A38_OFFSET: usize = 0xa38;
/// CS::TitleFlowContext dispatch-state field (`tfc = *(TitleTopDialog+0xa38)`; `tfc+0x14c`). The
/// live user-driven Continue capture (bd LIVE-continue-chain-via-selector-NOT-confirm-handler) showed
/// the load runs through the selector `0x1409a8eb0` which reads this field and dispatches to the load
/// dispatcher `0x1409b3070` (0=idle, 1=load, 3/5=busy). Setting it to 1 at the settled main menu is
/// the candidate DIRECT "Continue pressed" trigger (no input) -- the exact bit we change.
pub(crate) const TFC_DISPATCH_STATE_14C_OFFSET: usize = 0x14c;
pub(crate) const TFC_DISPATCH_STATE_LOAD: i32 = 1;
/// CS::TitleFlowContext `notReleaseFlag55` byte at `tfc+0x18c`. The load dispatcher `0x1409b3070`
/// gates its BUILD-the-LoadGame-job branch on `IsNotReleaseFlag55` (`0x14082cd60`: `cmpb $0,0x18c(rcx)`
/// -> returns 1 iff the byte is 0); the dispatcher takes the LOAD branch ONLY when that returns
/// nonzero, i.e. when `*(u8*)(tfc+0x18c)==0`. The open-menu path sets this nonzero AFTER press-any-
/// button, so a Continue trigger fired post-menu-open lands on the ABORT branch (empty job, no load).
/// Force this to 0 before invoking the selector to guarantee the real LoadGame build. bd
/// dispatcher-abort-branch-force-tfc-18c-zero-2026-06-23.
pub(crate) const TFC_NOT_RELEASE_FLAG_18C_OFFSET: usize = 0x18c;
pub(crate) const TFC_NOT_RELEASE_FLAG_CLEAR: u8 = 0;
/// CS::TitleTopDialog Continue-item SELECTOR `0x1409a8eb0` -- the menu-item-action funclet that the
/// engine invokes on Continue confirm (it is NOT pumped from the idle menu; setting tfc+0x14c alone
/// is dormant -- bd tfc-bit-dormant-even-at-open-menu). ABI `__fastcall(rcx = &dialog_slot, rdx = out
/// MenuJobResult*)`: it does `rcx=*(rcx)` (dialog), `*(dialog+0xa38)`=tfc, reads `*(tfc+0x14c)`; when
/// that == 1 (TFC_DISPATCH_STATE_LOAD) it takes the LOAD branch -- `r8=dialog+0x50`, calls the load
/// dispatcher `0x1409b3070` (the PROPER CS::MenuJob::ChainMenuJobs enqueue, no FixOrderJobSequence
/// overflow), and wraps the built job into rdx. Pass rcx = owner+0xe0 (its [0] is the live dialog).
/// Verified by disasm of 0x1409a8eb0 + the live user-Continue capture (selector body 0x9a8f09 ->
/// 0x9b3070). bd LIVE-continue-chain-via-selector-NOT-confirm-handler.
pub(crate) const TITLE_CONTINUE_SELECTOR_RVA: usize = 0x9a8eb0;
/// CS::TitleTopDialog MenuJobQueue at `dialog+0x10` (ring at +0x18) -- the queue the native Continue
/// path posts the built LoadGame job into, drained each frame by the menu pump `0x1409aa680` (which
/// iterates the active-screen array `0x143d6d8d0` that holds the live `owner+0xe0` dialog). The
/// selector/dispatcher only BUILD + return the job; we PushBackJob it here so it is pumped to
/// completion. bd continue-load-POST-primitive-pushbackjob-kick-2026-06-22.
pub(crate) const DIALOG_MENU_QUEUE_10_OFFSET: usize = 0x10;
/// Menu-pump KICK pointer: `*(base+0x3b37c98)` holds `0x1409b3ff0` (a `jmp` thunk into the obfuscated
/// per-frame pump trigger). The native posts a MenuJob then calls this zero-arg to drain it promptly;
/// we replicate that after PushBackJob. RVA = abs - base; the stored value is an ABSOLUTE code ptr.
pub(crate) const MENU_PUMP_KICK_PTR_RVA: usize = 0x3b37c98;
/// MenuJobQueue per-frame DRAIN wrapper (deobf `0x1407a90f0`; dump `FUN_1407a91e0`). The zero-input,
/// input-free way to pump a job we PushBackJob'd -- this is what the native front-end `Update` /
/// `STEP_MenuJobWait` call each frame (NOT the Arxan kick, which is a Scaleform render refresh needing
/// render-thread r8). `__fastcall(rcx = queue_owner /*the dialog: +0x8 active MenuJob* slot, +0x10 the
/// MenuJobQueue we push into, +0x38 pending*/, rdx = *FD4Time {vtbl; f32 delta@+0x8})`: if the active
/// slot is empty and a job is pending it pops (`0x1407a8780`) + Assigns (`0x1407a9460`) the queued job
/// into the active slot, then runs `ExecuteMenuJob` (deobf `0x1407a9600`: `cur->vtable[2](cur,&result,
/// &FD4Time)`). Call it each frame with rcx=dialog to drive our posted LoadGame job to completion.
/// Grounded by prologue on eldenring-deobf.bin (dump->deobf shift ~-0xf0 here, anchored on PushBackJob
/// dump 0x1407a9340 == deobf 0x1407a9250). bd continue-load-drain-via-executemenujob-not-kick-2026-06-23.
pub(crate) const MENU_DRAIN_WRAPPER_RVA: usize = 0x7a90f0;
/// `ExecuteMenuJob` (deobf `0x1407a9600`; dump `0x1407a96f0`). `__fastcall(rcx = *MenuJob* (slot),
/// rdx = *FD4Time {vtbl; f32 delta@+0x8})`: `cur=*rcx; if(!cur) return; AtomicIncrement(cur+8);
/// cur->vtable[+0x10](cur, &result, &{FD4Time vtbl, delta}); if(!MenuJobResult::ShouldContinue)
/// *rcx=0; AtomicDecrement`. We call this directly on OUR built job each frame (rcx=&job_slot) to
/// pump it via its OWN vtable[2] -- correct for the dispatcher's chained LoadGame job, and it avoids
/// the dialog's `+0x8` slot (which is NOT a MenuJob and AV'd the queue-drain wrapper). Grounded by
/// prologue on eldenring-deobf.bin (the `vtable[2]` call site `0x1407a968b call *0x10(rax)`).
pub(crate) const EXECUTE_MENU_JOB_RVA: usize = 0x7a9600;
/// CS::MenuManImp singleton global (`*(base+0x3d6b7b0)` = CSMenuManImp*). Verified: HasTopMenuJob
/// 0x14080d960 does `mov rax,[0x143d6b7b0]; mov rcx,0x80(rax)` (popupMenu) then reads +0xB0. (Same
/// singleton whose +0x90 is the menu input bitmap.) bd menu-job-install-mechanism-2026-06-23.
pub(crate) const GLOBAL_CSMENUMAN_RVA: usize = 0x3d6b7b0;
/// CSMenuManImp -> CSPopupMenu* at +0x80.
pub(crate) const CSMENUMAN_POPUP_80_OFFSET: usize = 0x80;
/// CSPopupMenu -> `currentTopMenuJob` (MenuJob*) at +0xB0 -- the single top-job slot the per-frame
/// menu pump drains (no cap). Install our built LoadGame job here so the native pump runs its Run
/// IN CONTEXT (vs our menu-jumping self-pump).
pub(crate) const CSPOPUP_TOP_JOB_B0_OFFSET: usize = 0xB0;
/// `CS::MenuJob::Assign(rcx = dest MenuJob**, rdx = out MenuJob**, r8 = src MenuJob**)` (deobf
/// 0x1407a9460 -- verified prologue: homes r8/rdx, `rbx=*dest`; if `*dest != *src` AtomicDecrements
/// the old occupant (0x141eba200) + dtors if last, then installs `*dest=*src` + AtomicIncrement).
/// Refcount-correct slot replace -- use to install our job into currentTopMenuJob without leaking the
/// displaced title-FSM job. NOTE: distinct from MENUJOB_ASSIGN_RVA (0x7a9560, a 2-arg move-assign).
pub(crate) const MENU_JOB_ASSIGN3_RVA: usize = 0x7a9460;
/// CS::MenuJob (DLReferenceCountObject) refcount field at +0x8 (vfptr at +0x0).
pub(crate) const MENU_JOB_REFCOUNT_8_OFFSET: usize = 0x8;
/// CS::TitleTopDialog embedded MenuWindowJob `DLFixedVector<MenuJob*,8>` at `dialog+0x50` -- the push
/// target our built load job's `CS::MenuWindowJob::Run` (`0x1407ad53b call 0x140733ef0`) inserts its
/// window into. Pinned via the push-site sw-bp diagnostic (rcx=`dialog+0x50`). Cap-8 and already FULL
/// with the dialog's windows, so the load window's push #9 overflows ("out of memory"
/// DLFixedVector.inl:662). Reset its count to make room. bd OVERFLOW-VECTOR-PINNED-dialog-plus-0x50.
pub(crate) const DIALOG_MENUWINDOW_VEC_50_OFFSET: usize = 0x50;
/// DLFixedVector element-count field at +0x48 (the push reads/increments `[vector+0x48]`, panics >8).
/// The dialog+0x50 vector's count is thus at `dialog+0x50+0x48 = dialog+0x98`.
pub(crate) const DLFIXEDVECTOR_COUNT_48_OFFSET: usize = 0x48;
/// CSMenuSystemSaveLoad save-slot field (`mss+0x1200`). The native confirm handler `0x1409a9250`
/// writes the slot here (the builder `0x1409ac8b0` reads it at `0x1409ac9d2` as the factory `r8`).
/// Replicate that write so the direct trigger loads the intended slot.
pub(crate) const MSS_SAVE_SLOT_1200_OFFSET: usize = 0x1200;
/// GameMan/GameDataMan singleton global read by `GetSaveSlot` (`*(0x143d69918)`, slot at `+0xac0`):
/// the "rest of GameMan is set up" readiness signal the user observed after press-any-button. The
/// direct continue trigger only fires once this is non-null. RVA = abs - base.
pub(crate) const GAME_SAVE_SLOT_SINGLETON_RVA: usize = 0x3d69918;
/// Plausible-pointer bounds for validating `owner_ctx = *(mss+0xa38)`: at `title_boot_ready` the
/// TitleFlowContext is often uninitialized (reads as 0x8080808080808080 -- non-null garbage), so a
/// `!= 0` check is insufficient. A real wine-heap pointer sits roughly in `0x1_0000 .. 0x8000_0000_0000`
/// (the golden value was 0x7fff..); anything outside is treated as "not built yet" -> pass NULL.
pub(crate) const OWNER_CTX_MIN_PLAUSIBLE_PTR: usize = 0x1_0000;
pub(crate) const OWNER_CTX_MAX_PLAUSIBLE_PTR: usize = 0x8000_0000_0000;

pub(crate) const OWN_STEPPER_LOG_INTERVAL: u64 = TitleNativeJobTiming::FrameRate as u64;
pub(crate) const OWN_STEPPER_CALL_INC: usize = true as usize;

#[repr(usize)]
pub(crate) enum OwnStepperPhase {
    Menu,
    Continue,
    Done,
    Mount,
    Drive,
    MenuBuild,
    S2Invoke,
    S2Activate,
    S2MountPoll,
    S2Confirm,
}

/// Own-stepper phase progress is semantic. These values are wall-clock fail-safe caps only:
/// they abort to a no-write state if a native predicate never arrives, and must never be used as
/// success/readiness gates.
pub(crate) const OWN_STEPPER_MOUNT_POLL_TIMEOUT_MS: u64 = 10_000;
pub(crate) const OWN_STEPPER_DRIVE_TIMEOUT_MS: u64 = 10_000;
pub(crate) const OWN_STEPPER_MENU_BUILD_TIMEOUT_MS: u64 = 50_000;
pub(crate) const OWN_STEPPER_S2_PHASE_TIMEOUT_MS: u64 = 20_000;
pub(crate) const OWN_STEPPER_IDX6_SETTLE_TICKS: u64 = 120;
pub(crate) const CAP_SELECTOR_TICK_LOG_INTERVAL_TICKS: usize = 120;

/// Driver phases for the in-context idx10 handler.
pub(crate) const OWN_STEPPER_PHASE_MENU: usize = OwnStepperPhase::Menu as usize;
pub(crate) const OWN_STEPPER_PHASE_CONTINUE: usize = OwnStepperPhase::Continue as usize;
pub(crate) const OWN_STEPPER_PHASE_DONE: usize = OwnStepperPhase::Done as usize;
/// PHASE 3 (MOUNT): mount the slot at state 10 BEFORE SetState(5) -- the only place the
/// MoveMapStep dispatcher (which resets b80 via its b80==1 lane) is NOT running, so our
/// own b80 poll can drive the save-IO machine 1->2->3 cleanly (minimal-save-mount-
/// primitive-recipe-2026). Register the FD4 stream worker (0x140b0a980 stub), initiate
/// the slot read (0x14067b4e0 -> b80=1), poll 0x140679180 until b80==3, then full
/// deserialize 0x14067b290 (c30 = real map + character applied), then SetState(5).
pub(crate) const OWN_STEPPER_PHASE_MOUNT: usize = OwnStepperPhase::Mount as usize;
/// b80 save-IO poll/driver 0x140679180(0,0): advances GameMan+0xb80 toward 3 (resident)
/// as the stream worker drains the async slot read; sets b80=3 when the IO request state
/// (0x14240a1f0) is resident. We call it ourselves each frame at state 10.
pub(crate) const B80_POLL_RVA: usize = 0x679180;
/// Both fastcall args (cl, dl) to the b80 poll 0x140679180 are 0 in the native menu
/// drive (matches the captured real-load poll calls poll(0,0)).
pub(crate) const B80_POLL_ARG_ZERO: u8 = false as u8;
/// b80==1 PREVIEW-lane driver 0x140679510: per-frame IO tick of the preview read started by
/// 0x14067b4e0; resets GameMan+0xb80 1->0 when the iodev request goes resident. NOT a
/// dispatcher (no CSFeMan apply / no save write) -- just the lane tick the menu runs via
/// dispatcher-1. We call it ourselves to drain the preview read to resident.
pub(crate) const B80_LANE1_DRIVER_RVA: usize = 0x679510;
/// Wall-clock fail-safe to poll b80 toward 3 before giving up the mount (avoid an infinite
/// title hang if the worker never drains). Not a readiness/success predicate.
pub(crate) const OWN_STEPPER_MOUNT_POLL_MAX: u64 = OWN_STEPPER_MOUNT_POLL_TIMEOUT_MS;
pub(crate) static OWN_STEPPER_MOUNT_POLLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
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
pub(crate) const OWN_STEPPER_PHASE_DRIVE: usize = OwnStepperPhase::Drive as usize;
/// GameMan+0xc30 unset sentinel (0xffffffff as i32). At the bare press-any-button title
/// (BeginTitle skipped) c30 is unset; the full deserialize 0x14067b290 is the ONLY thing
/// that writes it to the slot's real saved map during the mount, so c30 != UNSET is the
/// genuine "the character was deserialized" signal (ac0 is NOT -- set_save_slot pre-sets it).
pub(crate) const GAME_MAN_C30_UNSET: i32 = !OWN_STEPPER_SLOT_ZERO;
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
pub(crate) const SYNTH_MMS_SKIP_APPLY_ON: u8 = true as u8;
pub(crate) static mut SYNTH_MMS_OWNER: [u8; SYNTH_MMS_OWNER_SIZE] =
    [MOVIE_SKIP_FLAG_CLEAR; SYNTH_MMS_OWNER_SIZE];
/// Wall-clock fail-safe to drive the dispatchers before giving up (stay at title, no save write).
pub(crate) const OWN_STEPPER_DRIVE_MAX: u64 = OWN_STEPPER_DRIVE_TIMEOUT_MS;
pub(crate) static OWN_STEPPER_DRIVE_CALLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// PHASE 5 (MENU_BUILD): the parked press-any-button title is the FIRST state 10 and has
/// NOT run STEP_BeginTitle(3) yet, so the Continue/Load-Game items do not exist at
/// owner+0x138 until we drive 10->3 zero-input. idx10 SetState(owner,3) builds the main
/// menu (BeginTitle needs no session, writes NO save), then this phase waits for the menu
/// to populate and walks owner+0x138 to identify the Load-Game leaf (its +0xa8 action
/// functor's _Do_call chain resolves to dialog_factory 0x14081ead0). Max state reached =
/// main menu (no PlayGame) -> save-safe.
pub(crate) const OWN_STEPPER_PHASE_MENU_BUILD: usize = OwnStepperPhase::MenuBuild as usize;
/// Wall-clock fail-safe to wait for semantic menu-build predicates before giving up (stay at the
/// title, no save write). The intro/menu animation cadence varies by runtime, so this is a no-write
/// abort deadline only; readiness comes from native dialog/menu predicates.
pub(crate) const OWN_STEPPER_MENU_BUILD_WAIT_MAX: u64 = OWN_STEPPER_MENU_BUILD_TIMEOUT_MS;
/// Menu-build poll counter for diagnostics/log throttling, not a readiness gate.
pub(crate) static OWN_STEPPER_MENU_BUILD_WAITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// MenuWindowJob::Update 0x1407ad1c0 -- the native menu pump calls it with rcx = a
/// menu-item each tick. We hook it to CAPTURE the live Load-Game item (the one whose
/// +0xa8 action functor's _Do_call chain resolves to dialog_factory 0x14081ead0) without
/// guessing the CSMenu container layout the static walk could not penetrate. The captured
/// item pointer is stored in MENU_LOAD_GAME_ITEM for the own-stepper idx10 to read +
/// (Stage 2) invoke zero-input. 0 = not yet captured.
pub(crate) const MENU_ITEM_UPDATE_RVA: u32 = ProfileLoadMenuRva::MenuItemUpdate as u32;
pub(crate) static MENU_ITEM_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// MenuWindowJob constructor 0x1407ac8c0. Product autoload observes constructed items so the
/// semantic Continue item can be latched before the first updated/idle input leaf pollutes state.
pub(crate) const MENU_WINDOW_JOB_CTOR_RVA: u32 = 0x007ac8c0;
pub(crate) static MENU_WINDOW_JOB_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// MenuWindowJob native-accept constructor variant 0x1407acb00. Static xrefs show many menu
/// callers use this sibling constructor, and it installs the native accept predicate 0x1407ad810.
/// Hook passively so product telemetry can distinguish "no native-accept row was constructed" from
/// "we only hooked the wrong constructor variant".
pub(crate) const MENU_WINDOW_JOB_NATIVE_CTOR_B_RVA: u32 = 0x007acb00;
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// MenuWindowJob disabled/idle constructor 0x1407acf80. Product diagnostics observe this variant
/// because static RE shows it installs the constant-false accept predicate 0x1407add70 into the
/// same +0xf0 accept functor slot as the native-accept constructors.
pub(crate) const MENU_WINDOW_JOB_IDLE_CTOR_RVA: u32 = 0x007acf80;
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Tiny native title-row readiness predicate 0x140733150. The native title builder at
/// 0x1409b6730 calls this on `TitleTopDialog+0x2610`; false branches skip native MenuWindowJob
/// row construction entirely. The hook records the returned state object and `[obj+0x20] & 0x8f`
/// result so runtime artifacts can distinguish "waiting for native readiness" from a missing row.
pub(crate) const TITLE_NATIVE_READY_PREDICATE_RVA: u32 = 0x00733150;
pub(crate) static TITLE_NATIVE_READY_PREDICATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_LOAD_GAME_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// FD4 Sequence::Update / child-iterator 0x1407aa1f0 (RVA). At the opened main menu the
/// Load-Game leaf d180 is REGISTERED but does NOT tick (only the focused entry ticks via the
/// leaf Update 0x1407ad1c0, so the leaf hook misses d180). This iterator runs on every
/// Sequence node and dispatches its children at [seq+0x18 + i*8] (count [seq+0x60]); hooking
/// it lets us ENUMERATE every Sequence's children and capture d180 as a CHILD (functor ->
/// dialog_factory) regardless of focus -- the robust zero-input locator.
pub(crate) const SEQUENCE_ITER_RVA: u32 = 0x007aa1f0;
pub(crate) static SEQUENCE_ITER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Child-array offsets on an FD4 Sequence node (inline array at +0x18, count at +0x60,
/// stride 8). Sane child-count bound to skip non-Sequence nodes the hook also sees.
pub(crate) const SEQUENCE_CHILDREN_BASE_18_OFFSET: usize = 0x18;
pub(crate) const SEQUENCE_COUNT_60_OFFSET: usize = 0x60;
pub(crate) const SEQUENCE_CHILD_COUNT_MIN: usize = 1;
pub(crate) const SEQUENCE_CHILD_COUNT_MAX: usize = 64;
/// CS::MenuWindowJob vtable 0x142aa97e8 (RVA) -- the menu-item leaf class. Used to log only
/// real menu-item children when enumerating the opened-menu structure via the iterator hook.
pub(crate) const MENU_WINDOW_JOB_VTABLE_RVA: usize = 0x2aa97e8;
/// Diagnostic: log distinct MenuWindowJob children the Sequence iterator walks (with docall
/// chain) so one run reveals the full opened-menu entry set. Capped to avoid flooding.
pub(crate) static SEQ_ITER_CHILD_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SEQ_ITER_CHILD_LOG_MAX: usize = 240;
pub(crate) static SEQ_ITER_CHILD_LAST: AtomicUsize = AtomicUsize::new(0);
/// Unconditional structural dump of the first N Sequence-iterator calls (seq vtable, count,
/// child0 vtable) -- reveals what the iterator actually walks (Sequence vs MenuWindowJob,
/// real counts) regardless of the count-range gate, to diagnose why no menu-item child was
/// found. Capped.
pub(crate) static SEQ_ITER_DEBUG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SEQ_ITER_DEBUG_MAX: usize = 80;
pub(crate) static MENU_ITEM_UPDATE_CAPTURE_COUNT: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Cap on leaf-hook "distinct item ticked" log lines. A few title/menu items rotate every
/// frame, so without a cap this floods the size-capped trace and rolls early diagnostics off.
pub(crate) const MENU_ITEM_UPDATE_LOG_MAX: usize = TraceSampleLimit::Value48 as usize;
/// Last menu-item pointer we logged from the Update hook; we log only on change (the pump
/// ticks one item per frame, so this surfaces each distinct item as the user navigates,
/// without flooding the trace).
pub(crate) static MENU_ITEM_UPDATE_LAST: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The product autoload no longer gates startup on a fixed idx10 call count; it polls
/// `title_boot_ready` for the native owner/state/session/dialog predicates instead.
/// Shim callback object for the native Continue confirm 0x140b0e180 (reads
/// owner=[shim+8]). Persistent (not stack) so the call cannot read freed memory.
#[repr(C)]
pub(crate) struct OwnStepperShimLayout {
    pub(crate) unknown_00: usize,
    pub(crate) owner: usize,
    pub(crate) scratch: [usize; 6],
}

pub(crate) const OWN_STEPPER_SHIM_LEN: usize =
    core::mem::size_of::<OwnStepperShimLayout>() / core::mem::size_of::<usize>();
pub(crate) const OWN_STEPPER_SHIM_OWNER_IDX: usize =
    core::mem::offset_of!(OwnStepperShimLayout, owner) / core::mem::size_of::<usize>();
/// idx6 = STEP_GameStepWait func slot = table base + 6*0x10 = abs 0x143d715e0 (RVA
/// 0x3d715e0). We own it too, to drive the 3-phase load after the MoveMapStep builds.
pub(crate) const TITLE_STEP_IDX6_SLOT_RVA: usize = 0x3d715e0;
pub(crate) static OWN_STEPPER_ORIG_IDX6: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static OWN_STEPPER_IDX6_CALLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Path A re-target single-shot latch: 0 = the native b78-route deserialize has not yet
/// landed a real GameMan+0xc30, 1 = idx6 has already re-targeted owner+0xbc to the real
/// map + SetState(5). Prevents re-firing the re-target every frame.
pub(crate) static OWN_STEPPER_RETARGETED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_RETARGET_NO);
pub(crate) const OWN_STEPPER_RETARGET_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_RETARGET_YES: usize = true as usize;
/// GameMan+0xb80 load-phase value meaning the save IO is resident (mounted).
#[repr(i32)]
pub(crate) enum OwnStepperB80State {
    Idle,
    PreviewLane,
    Active,
    Resident,
}

pub(crate) const OWN_STEPPER_B80_RESIDENT: i32 = OwnStepperB80State::Resident as i32;
/// GameMan+0xb80 == 1: the PREVIEW lane (0x14067b4e0 read in flight); drive the lane tick
/// 0x140679510 to drain it to resident (which resets b80 -> 0).
pub(crate) const OWN_STEPPER_B80_PREVIEW_LANE: i32 = OwnStepperB80State::PreviewLane as i32;
/// GameMan+0xb80 == 0: idle/drained; fire the LoadSaveData initiator 0x14067b200 -> b80=2
/// (reusing the resident iodev request the preview started).
pub(crate) const OWN_STEPPER_B80_IDLE: i32 = OwnStepperB80State::Idle as i32;
/// idx6 calls to wait (MoveMapStep settle) before deserializing the real slot.
pub(crate) const OWN_STEPPER_IDX6_SETTLE: u64 = OWN_STEPPER_IDX6_SETTLE_TICKS;
pub(crate) const OWN_STEPPER_SLOT_NONE: i32 = !OWN_STEPPER_SLOT_ZERO;
/// Lowest valid save-slot index (used to bounds-check the dialog cursor in STAGE 2).
pub(crate) const OWN_STEPPER_SLOT_ZERO: i32 = false as i32;
/// Save slot to load (parsed from the trigger file "slot=N"; -1 => leave the game's
/// own most-recent selection).
pub(crate) static OWN_STEPPER_SLOT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(OWN_STEPPER_SLOT_NONE);
pub(crate) static OWN_STEPPER_PHASE: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PHASE_MENU);
pub(crate) static mut OWN_STEPPER_SHIM: [usize; OWN_STEPPER_SHIM_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; OWN_STEPPER_SHIM_LEN];
/// 2026-06-18 DIRECT BUILD: persistent buffers for building a ProfileLoadDialog directly via
/// dialog_factory 0x14081ead0 (bypassing the input-gated router_this/d180-on-confirm layer that
/// never builds headless). `cap[0]` = owner+0x138 (the ctor r8 = *(capture+8)); the factory reads
/// *(rcx). `ctx` is the zeroed incoming-ctx the factory reads to build the (empty cosmetic) label.
/// Both PERSISTENT (the built dialog retains a pointer to the ctx), so static, never stack.
pub(crate) const DIRECT_BUILD_CAP_LEN: usize = 1;
pub(crate) static mut DIRECT_BUILD_CAP: [usize; DIRECT_BUILD_CAP_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; DIRECT_BUILD_CAP_LEN];
pub(crate) const DIRECT_BUILD_CTX_LEN: usize = 8;
pub(crate) static mut DIRECT_BUILD_CTX: [usize; DIRECT_BUILD_CTX_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; DIRECT_BUILD_CTX_LEN];
/// One-shot latch so the native build fires at most once per run.
pub(crate) static OWN_STEPPER_DIRECT_BUILT: AtomicUsize =
    AtomicUsize::new(OWN_STEPPER_DIRECT_BUILT_NO);
pub(crate) const OWN_STEPPER_DIRECT_BUILT_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_DIRECT_BUILT_YES: usize = true as usize;
/// MODEL B (live_dialog_enabled): one-shot latch so the live Load-Game node's native run
/// 0x1409aaba0 fires at most once per run (a re-fire would double-build / leak the dialog).
pub(crate) static OWN_STEPPER_LIVE_FIRED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_LIVE_FIRED_NO);
pub(crate) const OWN_STEPPER_LIVE_FIRED_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_LIVE_FIRED_YES: usize = true as usize;

// ============================================================================================
// STAGE 2 -- the VERIFIED in-context menu-drive that actually COMPLETES a character load.
// After PHASE_MENU_BUILD identifies the Load-Game leaf d180 (MENU_LOAD_GAME_ITEM), STAGE 2
// invokes its +0xa8 functor (-> ProfileLoadDialog), sets the dialog slot cursor, calls the
// dialog's vtable-slot-20 `load_activate` (which reads the cursor [dialog+0xb0c] -- NOT an
// arg), lets the NATIVE menu pump tick the registered selector step 0x140826d50 (which
// populates iodev io18/io20 and runs the menu deserialize 0x14082c240 -> ac0=N + c30=real +
// character applied, b80-INDEPENDENT), then `continue_confirm` 0x140b0e180 -> SetState(5).
// All offsets VERIFIED against the on-disk decrypted exe (STAGE-2 spec 2026-06-16).
// ============================================================================================
/// PHASE 6 (S2 INVOKE): fire d180's +0xa8 action functor to build the ProfileLoadDialog.
pub(crate) const OWN_STEPPER_PHASE_S2_INVOKE: usize = OwnStepperPhase::S2Invoke as usize;
/// PHASE 7 (S2 ACTIVATE): write the slot cursor [dialog+0xb0c]=N (bounds [dialog+0xb08]) then
/// call the dialog's vtable-slot-20 load_activate(rcx=dialog), registering the selector step.
pub(crate) const OWN_STEPPER_PHASE_S2_ACTIVATE: usize = OwnStepperPhase::S2Activate as usize;
/// PHASE 8 (S2 MOUNT_POLL): pass-through each frame so the native pump ticks the selector;
/// watch for the mount (ac0==N + io18/io20 set->clear; c30 leaving the new-game default).
pub(crate) const OWN_STEPPER_PHASE_S2_MOUNT_POLL: usize = OwnStepperPhase::S2MountPoll as usize;
/// PHASE 9 (S2 CONFIRM): guard (ac0==N && c30==latched-mount && io consumed) then
/// continue_confirm -> SetState(5) so the native pump streams the real world. The ONLY
/// save-write-risking step; gated entirely by a verified real mount (fail-closed otherwise).
pub(crate) const OWN_STEPPER_PHASE_S2_CONFIRM: usize = OwnStepperPhase::S2Confirm as usize;
#[repr(usize)]
pub(crate) enum ProfileLoadMenuRva {
    ProfileSlotActivate = 0x262250,
    MenuItemUpdate = 0x007ad1c0,
    ProfileLoadSelectorTick = 0x826d50,
    MenuDeser = 0x0082c240,
    CsMenuCtor = 0x009060d0,
    MenuMemberFuncJobRun = 0x9aaba0,
    MenuLoadGameFunctorVtable = 0x02ac3ea8,
    SelectorStepVtable = 0x2ac71e0,
    ProfileLoadDialogVtable = 0x2b229f8,
}

/// CS::ProfileLoadDialog vtable (RVA). The dialog built by d180's functor (dialog_factory
/// 0x14081ead0 -> ctor 0x1409a3d90 writes this vtable). Used to VALIDATE the built dialog
/// before any dialog call (a wrong this-pointer would AV).
pub(crate) const PROFILE_LOAD_DIALOG_VTABLE_RVA: usize =
    ProfileLoadMenuRva::ProfileLoadDialogVtable as usize;
/// Dialog vtable slot 20 (offset 0xa0) = load_activate 0x1409a4670. Read the live slot from
/// the dialog vtable (robust to relocation) rather than hard-calling the RVA.
#[repr(C)]
pub(crate) struct ProfileLoadDialogVtableLayout {
    pub(crate) unknown_slots_00_19: [usize; 20],
    pub(crate) load_activate: usize,
}

pub(crate) const DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogVtableLayout, load_activate);

#[repr(C)]
pub(crate) struct ProfileLoadDialogLayout {
    pub(crate) unknown_000: [u8; 0xb08],
    pub(crate) slot_bound: i32,
    pub(crate) slot_cursor: i32,
    pub(crate) unknown_b10: [u8; 0x11b8],
    pub(crate) load_job_ctx: usize,
}

/// Dialog selected-list-index cursor (= [dialog+0xa38+0xd4]); load_activate reads it as the
/// slot. WRITE the desired slot N here before calling load_activate.
pub(crate) const DIALOG_SLOT_CURSOR_B0C_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogLayout, slot_cursor);
/// Dialog list inclusive upper bound; load_activate clamps the cursor to [0, bound).
pub(crate) const DIALOG_SLOT_BOUND_B08_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogLayout, slot_bound);

/// MenuWindowJob (d180) layout: +0xa8 action std::function, +0x10 dialog ctx-out (functor
/// fires only when ==0), +0x130 built-dialog result slot.
#[repr(C)]
pub(crate) struct MenuWindowJobLayout {
    pub(crate) unknown_000: [u8; 0x10],
    pub(crate) dialog_context: usize,
    pub(crate) unknown_018: [u8; 0x90],
    pub(crate) action_functor: usize,
    pub(crate) unknown_0b0: [u8; 0x80],
    pub(crate) dialog_result: usize,
}

pub(crate) const MENU_ITEM_FUNCTOR_A8_OFFSET: usize =
    core::mem::offset_of!(MenuWindowJobLayout, action_functor);
pub(crate) const MENU_ITEM_CTX_10_OFFSET: usize =
    core::mem::offset_of!(MenuWindowJobLayout, dialog_context);
pub(crate) const MENU_ITEM_DIALOG_RESULT_130_OFFSET: usize =
    core::mem::offset_of!(MenuWindowJobLayout, dialog_result);
/// Main-title Continue row action `_Do_call` thunk. This is the `+0xa8` action on the
/// first focused MenuWindowJob after native `TitleTopDialog::open_menu`; it builds the native
/// row result consumed by the FD4 menu submit helper, not a save-load/direct-confirm shortcut.
pub(crate) const MENU_TITLE_CONTINUE_DOCALL_RVA: usize = 0x00764b80;
/// Native FD4 row submit helper used by `MenuWindowJob::Update` for one result-mode branch.
/// It forwards event `3` to the row result's own vtable slot `+0x60`.
pub(crate) const MENU_ITEM_SUBMIT_RVA: usize = 0x007ac890;
/// Row-result field consumed by `MenuWindowJob::Update` to choose which native accept event branch
/// to send to the built row result.
pub(crate) const MENU_ITEM_RESULT_MODE_58_OFFSET: usize = 0x58;
/// Row-result virtual event handler slot. Both native accept branches dispatch through this slot.
pub(crate) const MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET: usize = 0x60;
/// Tiny FD4 event constructor: writes `{ code: edx, payload: r8d }` to the output slot.
pub(crate) const FD4_EVENT_CONSTRUCTOR_RVA: usize = 0x007a91e0;
pub(crate) const MENU_ITEM_RESULT_MODE_EVENT3: i32 = 1;
pub(crate) const MENU_ITEM_RESULT_MODE_EVENT4: i32 = 2;
pub(crate) const MENU_ITEM_RESULT_EVENT4_CODE: i32 = 4;
pub(crate) const MENU_ITEM_RESULT_EVENT4_PAYLOAD: i32 = -1;
/// GameMan+0xc30 new-game DEFAULT map (m10_01_00_00). The mount writes the slot's REAL map
/// here; for a NON-m10 char `c30 != this` corroborates the mount (for an m10 char it is
/// ambiguous -- ac0 is the primary mount oracle). Packed mAA_BB_CC_DD.
#[repr(i32)]
pub(crate) enum GameManMapId {
    NewGameDefault = 0x0a01_0000,
}

pub(crate) const GAME_MAN_NEWGAME_DEFAULT_MAP: i32 = GameManMapId::NewGameDefault as i32;
/// STAGE 2 invocation is gated by concrete menu/action/dialog readiness, not by a fixed
/// post-open settle frame count.
/// Wall-clock fail-safe per S2 phase before failing closed (stay at the menu, NO SetState(5),
/// NO write). Readiness is still semantic (`ProfileLoadDialog`, selector tick, mount latch, char
/// fingerprint), not elapsed time.
pub(crate) const OWN_STEPPER_S2_PHASE_MAX: u64 = OWN_STEPPER_S2_PHASE_TIMEOUT_MS;
/// Per-phase poll counter for S2 diagnostics/log throttling, not a readiness gate.
pub(crate) static OWN_STEPPER_S2_WAITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// The built+validated ProfileLoadDialog pointer (0 until PHASE_S2_INVOKE succeeds).
pub(crate) static OWN_STEPPER_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The CS::MenuJobWithContext<LoadJobContext> selector step (vtable 0x142ac71e0) that
/// load_activate 0x1409a4670 builds at `dialog+0x18`. A cold standalone dialog is not ticked by
/// the MENU task-group, so STAGE 2 reads this and SELF-PUMPS the tick 0x140826d50 each frame
/// (installer -> io18/io20 full-save read -> menu_deser 0x14082c240 -> mount).
pub(crate) static OWN_STEPPER_SELECTOR_STEP: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The selector tick context observed at builder `owner+0xf8`; natural selector_tick calls use this
/// as arg2 while arg1 is the heap selector step stored at `[owner]` by builder 0x140826510.
pub(crate) static OWN_STEPPER_SELECTOR_CTX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// One-shot guard: fire the deserialize 0x67b290 exactly once when the full-save read is resident.
/// State machine for the shared DESER latch: NOT_FIRED -> {FIRED_FAIL, FIRED_OK}. Used by both
/// the STAGE2 mount and cold_char_mount_drive's DESER phase so the result is observable.
#[repr(usize)]
pub(crate) enum OwnStepperDeserState {
    NotFired,
    FiredFail,
    FiredOk,
}

pub(crate) static OWN_STEPPER_DESER_FIRED: AtomicUsize =
    AtomicUsize::new(OWN_STEPPER_DESER_NOT_FIRED);
pub(crate) const OWN_STEPPER_DESER_NOT_FIRED: usize = OwnStepperDeserState::NotFired as usize;
pub(crate) const OWN_STEPPER_DESER_FIRED_FAIL: usize = OwnStepperDeserState::FiredFail as usize;
pub(crate) const OWN_STEPPER_DESER_FIRED_OK: usize = OwnStepperDeserState::FiredOk as usize;
/// deserialize 0x67b290 success return code (ret==1 == real char applied + c30 written from save).
pub(crate) const OWN_STEPPER_DESER_SUCCESS_RET: i32 = true as i32;
/// One-shot latch: set once the zero-input title-confirm fire (fire_titletop_load_entry) has
/// fired the Load-Game row action, so it is not re-fired while the ProfileLoadDialog builds.
pub(crate) static OWN_STEPPER_TITLE_FIRED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
/// The RESOLVED target slot the mount is expected to land on: the configured `slot=N` if
/// >=0, else (slot=-1 "most-recent") the dialog's natural highlight cursor read live at
/// PHASE_S2_ACTIVATE. MOUNT_POLL/CONFIRM compare `GameMan+0xac0` against this.
pub(crate) static OWN_STEPPER_EXPECTED_SLOT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(OWN_STEPPER_SLOT_NONE);
/// Latched real GameMan+0xc30 at the moment the mount is detected; re-read & required-equal
/// at PHASE_S2_CONFIRM (the save-write guard).
pub(crate) static OWN_STEPPER_MOUNT_C30: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(GAME_MAN_C30_UNSET);
/// Latch: the iodev request pair (io18 & io20) was observed non-null at least once -- so
/// "io18==0 && io20==0" means "request consumed/mounted", not "never started".
pub(crate) static OWN_STEPPER_IO_WAS_SET: AtomicUsize = AtomicUsize::new(OWN_STEPPER_IO_WAS_SET_NO);
pub(crate) const OWN_STEPPER_IO_WAS_SET_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_IO_WAS_SET_YES: usize = true as usize;
/// One-shot latch so PHASE_S2_INVOKE hand-invokes the functor at most once.
pub(crate) static OWN_STEPPER_INVOKED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_FALSE as usize);
/// One-shot latch so PHASE_S2_CONFIRM fires SetState(5) at most once.
pub(crate) static OWN_STEPPER_CONFIRMED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_FALSE as usize);
// ---- CS::PlayerGameData correctness oracle (read at in-world) ----
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
pub(crate) const GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET: usize =
    core::mem::offset_of!(GameDataMan, main_player_game_data);
pub(crate) const PGD_CURRENT_HP_10_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_hp);
pub(crate) const PGD_CURRENT_MAX_HP_14_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_max_hp);
pub(crate) const PGD_BASE_MAX_HP_18_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_max_hp);
pub(crate) const PGD_CURRENT_FP_1C_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_fp);
pub(crate) const PGD_CURRENT_MAX_FP_20_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_max_fp);
pub(crate) const PGD_BASE_MAX_FP_24_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_max_fp);
pub(crate) const PGD_CURRENT_STAMINA_2C_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_stamina);
pub(crate) const PGD_CURRENT_MAX_STAMINA_30_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, current_max_stamina);
pub(crate) const PGD_BASE_MAX_STAMINA_34_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_max_stamina);
pub(crate) const PGD_LEVEL_68_OFFSET: usize = core::mem::offset_of!(PlayerGameData, level);
pub(crate) const PGD_RUNE_COUNT_6C_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, rune_count);
pub(crate) const PGD_RUNE_MEMORY_70_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, rune_memory);
pub(crate) const PGD_CHR_TYPE_98_OFFSET: usize = core::mem::offset_of!(PlayerGameData, chr_type);
pub(crate) const PGD_GENDER_BE_OFFSET: usize = core::mem::offset_of!(PlayerGameData, gender);
pub(crate) const PGD_ARCHETYPE_BF_OFFSET: usize = core::mem::offset_of!(PlayerGameData, archetype);
pub(crate) const PGD_VOICE_TYPE_C2_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, voice_type);
pub(crate) const PGD_STARTING_GIFT_C3_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, starting_gift);
pub(crate) const PGD_UNLOCKED_TALISMAN_SLOTS_C6_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, unlocked_talisman_slots);
pub(crate) const PGD_SPIRIT_ASH_LEVEL_C7_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, matchmaking_spirit_ashes_level);
pub(crate) const PGD_MAX_CRIMSON_FLASK_101_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, max_hp_flask);
pub(crate) const PGD_MAX_CERULEAN_FLASK_102_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, max_fp_flask);
pub(crate) const PGD_FACE_DATA_OFFSET: usize = core::mem::offset_of!(PlayerGameData, face_data);
pub(crate) const FACE_DATA_BUFFER_OFFSET: usize = core::mem::offset_of!(FaceData, face_data_buffer);
pub(crate) const FACE_DATA_BUFFER_MAGIC_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, magic);
pub(crate) const FACE_DATA_BUFFER_VERSION_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, version);
pub(crate) const FACE_DATA_BUFFER_SIZE_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, buffer_size);
pub(crate) const FACE_DATA_BUFFER_PAYLOAD_OFFSET: usize =
    core::mem::offset_of!(FaceDataBuffer, buffer);
pub(crate) const FACE_DATA_BUFFER_PAYLOAD_SIZE: usize =
    core::mem::size_of::<FaceDataBuffer>() - FACE_DATA_BUFFER_PAYLOAD_OFFSET;
pub(crate) const FACE_DATA_BUFFER_TOTAL_SIZE: usize =
    FACE_DATA_BUFFER_PAYLOAD_OFFSET + FACE_DATA_BUFFER_PAYLOAD_SIZE;
/// Face-body values are the face payload that begins at FaceDataBuffer::buffer.
pub(crate) const FACE_BODY_FIELD_FACE_MODEL_OFFSET: usize = FACE_DATA_BUFFER_PAYLOAD_OFFSET;
pub(crate) const FACE_BODY_FIELD_HAIR_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_FACE_MODEL_OFFSET + core::mem::size_of::<u32>();
/// The eyebrow field follows the hair field after one u32-sized reserved/model slot in the
/// serialized face-body payload.
pub(crate) const FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_HAIR_MODEL_OFFSET + core::mem::size_of::<u32>() + core::mem::size_of::<u32>();
pub(crate) const FACE_BODY_FIELD_BEARD_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_EYEBROW_MODEL_OFFSET + core::mem::size_of::<u32>();
pub(crate) const FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET: usize =
    FACE_BODY_FIELD_BEARD_MODEL_OFFSET + core::mem::size_of::<u32>();
/// The apparent-age byte follows the model-id cluster after three u32-sized face-shape slots.
pub(crate) const FACE_BODY_FIELD_APPARENT_AGE_OFFSET: usize = FACE_BODY_FIELD_EYE_PATCH_MODEL_OFFSET
    + core::mem::size_of::<u32>()
    + core::mem::size_of::<u32>()
    + core::mem::size_of::<u32>();
pub(crate) const FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET: usize =
    FACE_BODY_FIELD_APPARENT_AGE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_FORM_EMPHASIS_OFFSET: usize =
    FACE_BODY_FIELD_FACIAL_AESTHETIC_OFFSET + core::mem::size_of::<u8>();
#[repr(C)]
pub(crate) struct FaceBodyLayout {
    pub(crate) unknown_000: [u8; 0xac],
    pub(crate) head_size: u8,
}

pub(crate) const FACE_BODY_FIELD_HEAD_SIZE_OFFSET: usize =
    core::mem::offset_of!(FaceBodyLayout, head_size);
pub(crate) const FACE_BODY_FIELD_CHEST_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_HEAD_SIZE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_CHEST_SIZE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_ARMS_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_ABDOMEN_SIZE_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_LEGS_SIZE_OFFSET: usize =
    FACE_BODY_FIELD_ARMS_SIZE_OFFSET + core::mem::size_of::<u8>();
/// Skin color follows the body-size bytes after two one-byte face-body values that are not part
/// of the oracle fingerprint.
pub(crate) const FACE_BODY_FIELD_SKIN_COLOR_R_OFFSET: usize = FACE_BODY_FIELD_LEGS_SIZE_OFFSET
    + core::mem::size_of::<u8>()
    + core::mem::size_of::<u8>()
    + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_SKIN_COLOR_G_OFFSET: usize =
    FACE_BODY_FIELD_SKIN_COLOR_R_OFFSET + core::mem::size_of::<u8>();
pub(crate) const FACE_BODY_FIELD_SKIN_COLOR_B_OFFSET: usize =
    FACE_BODY_FIELD_SKIN_COLOR_G_OFFSET + core::mem::size_of::<u8>();
/// `character_name` is private upstream, so compute its start from the preceding public `chr_type`
/// field and its length from the following public `gender` field.
pub(crate) const PGD_NAME_9C_OFFSET: usize = core::mem::offset_of!(PlayerGameData, chr_type)
    + core::mem::size_of::<eldenring::cs::ChrType>();
pub(crate) const PGD_NAME_LEN_U16: usize =
    (PGD_GENDER_BE_OFFSET - PGD_NAME_9C_OFFSET) / core::mem::size_of::<u16>();
/// Base/end of the contiguous stat block; upstream's first post-stat field is `base_hero_point`.
pub(crate) const PGD_STAT_BASE_3C_OFFSET: usize = core::mem::offset_of!(PlayerGameData, vigor);
pub(crate) const PGD_STAT_END_OFFSET: usize =
    core::mem::offset_of!(PlayerGameData, base_hero_point);
pub(crate) const PGD_STAT_COUNT: usize =
    (PGD_STAT_END_OFFSET - PGD_STAT_BASE_3C_OFFSET) / core::mem::size_of::<u32>();
/// GameMan last field: `character_name_is_empty` (a cheap blank/new-game discriminator).
/// RESOLVED (autoresearch 2026-06-18) via static RE of `eldenring-deobf.bin`: the in-game
/// getter at 0x140679d90 is `mov rax,[GameMan]; movzbl 0xe70(rax),eax; ret`, so the field is
/// at +0xe70 -- our prior hand-decoded offset was 8 bytes too far (read padding past the field),
/// a real BUG. Now bound to the upstream typed field, which the disassembly confirms correct.
pub(crate) const GAME_MAN_NAME_IS_EMPTY_E70_OFFSET: usize =
    core::mem::offset_of!(GameMan, character_name_is_empty);
/// One-shot latch for the in-world LOAD-CORRECTNESS dump.
pub(crate) static LOAD_CORRECTNESS_DUMPED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const LOAD_CORRECTNESS_NOT_DUMPED: usize = 0;
/// One-shot latches for the OBSERVE-mode title->menu timing baseline (T0 at the parked title,
/// T_menu_open when the TitleTopDialog reaches TextFadeOut). Lets a true-vanilla run (no forcing,
/// modals + presses by the user) emit the SAME markers as the DLL-headless run for comparison.
pub(crate) static OBSERVE_T0_EMITTED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static OBSERVE_MENU_OPEN_EMITTED: AtomicUsize =
    AtomicUsize::new(OBSERVE_MARKER_NOT_EMITTED);
pub(crate) const OBSERVE_MARKER_NOT_EMITTED: usize = 0;
pub(crate) const OBSERVE_MARKER_EMITTED: usize = 1;
/// Synthetic `this` for the IngameInit-tail stream-worker register call 0x140b0a980
/// (+0x48 set to WORLD_WORKER_BUILD_STATE hits the build+register arm).
pub(crate) static mut OWN_STEPPER_WORKER_THIS: [u8; SYNTHETIC_STEP_THIS_SIZE] =
    [MOVIE_SKIP_FLAG_CLEAR; SYNTHETIC_STEP_THIS_SIZE];
pub(crate) const OWN_STEPPER_PATCHED_NO: usize = false as usize;
pub(crate) const OWN_STEPPER_PATCHED_YES: usize = true as usize;
/// Original idx10 func ptr (STEP_MenuJobWait), saved so our handler can pass through.
pub(crate) static OWN_STEPPER_ORIG_IDX10: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static OWN_STEPPER_BASE: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static OWN_STEPPER_PATCHED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PATCHED_NO);
pub(crate) static OWN_STEPPER_CALLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);

// ---------------------------------------------------------------------------
// NATIVE-LOAD gate (observe-only own_stepper; corrected-autoload-design-observe-not-force-native-load-2026).
// A SEPARATE gate from own_stepper: when enabled, the idx10 handler does NOT force the title
// state machine (no SetState(2/3), no beginlogo-gate clear, no registrar self-fire, no
// direct_build / cold_char_mount). It lets OWN_STEPPER_ORIG_IDX10 pass-through advance the NATIVE
// title machine, and ONCE the live TitleTopDialog menu is rendered + settled, it fires the native
// Load-Game MenuMemberFuncJob node's run 0x1409aaba0(rcx=node) exactly ONCE, then observes so the
// golden oracle is written as the native pump loads the char.
// ---------------------------------------------------------------------------
/// CS::MenuMemberFuncJob<TitleTopDialog>::run 0x1409aaba0 (RVA 0x9aaba0). Takes rcx=node (a
/// MenuMemberFuncJob, vtable TITLE_TOP_DIALOG run-node = MEMBERFUNCJOB_VTABLE_RVA); internally it
/// computes rcx=[node+0x10]+[node+0x20] (the member `this`, dialog + adjustor) and calls the
/// member-fn pointer at [node+0x18] -- which chains to the Load-Game dialog factory 0x14081ead0.
/// Firing it on the NATURALLY-booted menu builds a LIVE registered ProfileLoadDialog the native
/// pump drives (the live-dialog MenuWindow wall was a forcing artifact -- this de-risks step 4).
pub(crate) const MENU_MEMBER_FUNC_JOB_RUN_RVA: usize =
    ProfileLoadMenuRva::MenuMemberFuncJobRun as usize;
/// CS::MenuMemberFuncJob<TitleTopDialog> vtable 0x142b265d0 (RVA): the registry-entry node the
/// registrar 0x1409b24e0 inserts into [dialog+0xa48]; its run is MENU_MEMBER_FUNC_JOB_RUN_RVA.
/// (Mirrors the local MEMBERFUNCJOB_VTABLE_RVA in scan_dialog_for_loadgame.)
pub(crate) const MEMBERFUNCJOB_VTABLE_RVA: usize = 0x2b265d0;
/// TitleTopDialog row registry [dialog+0xa48] (the FD4 delegate registry the registrar populates).
/// Used as the live-menu readiness signal: populated == the menu rows are registered + rendered.
pub(crate) const DIALOG_ROW_REGISTRY_A48_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, row_registry);
/// NATIVE-LOAD fire latch states (one-shot: fire the Load-Game run exactly once).
pub(crate) const NATIVE_LOAD_FIRED_NO: usize = 0;
pub(crate) const NATIVE_LOAD_FIRED_YES: usize = 1;
pub(crate) static NATIVE_LOAD_FIRED: AtomicUsize = AtomicUsize::new(NATIVE_LOAD_FIRED_NO);
/// The native-load observer now fires only when `title_menu_action_ready` validates the concrete
/// Load-Game `MenuMemberFuncJob` node/action; there is no fixed post-menu settle frame count.
/// Throttle interval for native-load observe logging (frames).
pub(crate) const NATIVE_LOAD_LOG_INTERVAL: u64 = 120;

/// NATIVE-CONTINUE fire latch states (one-shot: fire the Continue run exactly once).
///
/// PATH B (autoload-path-B-drive-native-load-chosen-2026-06-22): mirror of the native-load latch,
/// but the one-shot `MENU_MEMBER_FUNC_JOB_RUN_RVA` (0x1409aaba0) fire targets the main-menu
/// **Continue** (load-most-recent) `MenuMemberFuncJob` registry node instead of Load-Game. The
/// Continue node is the registry MenuMemberFuncJob whose member-fn (at node+0x18) chains through the
/// thunk hops to the native Continue wrapper `TRACE_MENU_CONTINUE_WRAPPER_RVA` (0x14082bac0) -- the
/// SAME discriminator the product Continue capture already uses
/// (`capture_continue_member_node_candidate`), as opposed to Load-Game whose member-fn chains to the
/// ProfileLoadDialog factory `LIVE_DIALOG_FACTORY_RVA` (0x14081ead0). Firing Continue at the
/// NATURALLY-rendered title menu runs the FULL native load (parse + world-asset streaming + spawn),
/// which the golden proved completes the world-stream in ~6s (vs our menu-free continue_confirm-only
/// path stalling at WorldBlockRes phase 2).
pub(crate) const NATIVE_CONTINUE_FIRED_NO: usize = 0;
pub(crate) const NATIVE_CONTINUE_FIRED_YES: usize = 1;
pub(crate) static NATIVE_CONTINUE_FIRED: AtomicUsize = AtomicUsize::new(NATIVE_CONTINUE_FIRED_NO);
/// Throttle interval for native-continue observe logging (frames). Mirrors NATIVE_LOAD_LOG_INTERVAL.
pub(crate) const NATIVE_CONTINUE_LOG_INTERVAL: u64 = 120;

/// === NATIVE FULL-SAVE-READ observe chain (native-full-save-read-slot-resolve-chain-observe-recipe-2026). ===
/// The slot-resolve GLOBAL the menu cursor / Continue selection writes: resolver 0x1406793c0 returns
/// *(u32*)(GameMan+0xb78). Step 1 of the recipe sets GameMan+0xb78=slot before set_save_slot so the
/// native chain resolves OUR slot. (Same offset as GAME_MAN_REQUESTED_SLOT_B78_OFFSET; named per the
/// recipe for the full-read chain.)
pub(crate) const GAME_MAN_SLOT_SELECT_B78_OFFSET: usize =
    core::mem::offset_of!(GameMan, requested_save_slot_load_index);
/// GameMan+0xb80 == 3 == RESIDENT (the full-save read drained into the 0x280000 buffer). The DRAIN
/// phase ticks the lane + poll each frame until b80 reaches this.
pub(crate) const FULLREAD_B80_RESIDENT: i32 = 3;
/// GameMan+0xc30 m10 new-game default (golden-oracle-baseline). c30 == this == FAILURE (the char did
/// NOT deserialize). The step-6 guard requires c30 != this before the (gated) continue_confirm.
pub(crate) const FULLREAD_C30_M10_DEFAULT: i32 = 0xa010000;
/// Minimum REAL character level (a new-game default is <10; the golden Banon is 150). The step-6
/// guard requires the live PlayerGameData level >= this AND a non-empty name (via char_fingerprint).
pub(crate) const FULLREAD_MIN_REAL_LEVEL: u32 = 10;
/// Poll arg (0) for the b80 poll 0x140679180 and the lane driver 0x140679510 in the DRAIN phase.
pub(crate) const FULLREAD_POLL_ARG: u8 = 0;
/// DRAIN-phase budget: max frames to tick lane+poll waiting for b80==3 before TIMEOUT (no write).
pub(crate) const FULLREAD_DRAIN_MAX: u64 = 1200;
/// Throttle interval for the full-read chain per-frame logging (frames).
pub(crate) const FULLREAD_LOG_INTERVAL: u64 = 30;
/// Default slot for the full-read chain when neither OWN_STEPPER_SLOT (>=0) nor ER_EFFECTS_AUTOLOAD_SLOT
/// is set (Banon = slot 0).
pub(crate) const FULLREAD_DEFAULT_SLOT: i32 = 0;
/// continue_confirm shim field that owner+0x284 (new-game flag) must equal before the confirm runs
/// the SetState5: the native continue_confirm reads owner = *(shim[OWN_STEPPER_SHIM_OWNER_IDX]) =
/// *(base+0x3d5df38+8), checks owner+0x284==0, then sets owner+0xbc=c30 + SetState5 (autosaves).
pub(crate) const FULLREAD_OWNER_NEW_GAME_OK: u8 = 0;
/// owner = *(game_data_man_ptr_or_null() + this offset) -- the GameDataMan+0x8 chain the
/// continue_confirm shim owner is read from (recipe step 7: owner = *(base+0x3d5df38+8)).
pub(crate) const FULLREAD_OWNER_GDM_08_OFFSET: usize = 0x08;
/// Full-read chain phase machine states (one step per frame).
pub(crate) const FULLREAD_PHASE_SUBMIT: usize = 0;
pub(crate) const FULLREAD_PHASE_DRAIN: usize = 1;
pub(crate) const FULLREAD_PHASE_DESER: usize = 2;
pub(crate) const FULLREAD_PHASE_GUARD: usize = 3;
pub(crate) const FULLREAD_PHASE_DONE: usize = 4;
/// Live phase + drain-wait counters for the full-read chain (one-shot per run).
pub(crate) static FULLREAD_PHASE: AtomicUsize = AtomicUsize::new(FULLREAD_PHASE_SUBMIT);
pub(crate) static FULLREAD_DRAIN_WAITS: AtomicUsize = AtomicUsize::new(0);
/// The native full-read chain shares the semantic `title_menu_action_ready` menu readiness gate;
/// it no longer latches a first-seen frame before starting the save-read phase machine.
/// `save_requested`: bound to the upstream typed layout (compiler-verified equal to our prior
/// hand-decoded offset).
pub(crate) const GAME_MAN_ARM_FLAG_B72_OFFSET: usize =
    core::mem::offset_of!(GameMan, save_requested);

#[repr(C)]
pub(crate) struct GameManAutoloadFlagCluster {
    pub(crate) save_requested: u8,
    pub(crate) probe_b73: u8,
    pub(crate) probe_b74: u8,
    pub(crate) probe_b75: u8,
}

pub(crate) const GAME_MAN_FLAG_B73_PROBE_OFFSET: usize =
    GAME_MAN_ARM_FLAG_B72_OFFSET + core::mem::offset_of!(GameManAutoloadFlagCluster, probe_b73);
pub(crate) const GAME_MAN_FLAG_B75_PROBE_OFFSET: usize =
    GAME_MAN_ARM_FLAG_B72_OFFSET + core::mem::offset_of!(GameManAutoloadFlagCluster, probe_b75);
/// `requested_save_slot_load_index`: bound to upstream (compiler-verified equal to our offset).
pub(crate) const GAME_MAN_REQUESTED_SLOT_B78_OFFSET: usize =
    core::mem::offset_of!(GameMan, requested_save_slot_load_index);
pub(crate) const GAME_MAN_FLAG_BC4_OFFSET: usize =
    core::mem::offset_of!(GameMan, is_in_online_mode) - core::mem::size_of::<u32>();
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
/// The save-DEVICE MOUNT/OPEN routine 0x140e6e8d0(rcx=iodev): the title->Continue boot
/// (single native call site 0x140defec2) runs it to BIND the .sl2 file to the IO device.
/// It opens the OS handle (via 0x140e45660), registers the save paths, then writes the
/// open status byte to [iodev+0x40] @0x140e6eb56 -- the device-ready flag the async
/// router 0x140e6eb80 tests (jne BOUND real-read 0x140e6f430 / else COLD empty-noop
/// 0x140e6f5b0). The menu-free cold path SKIPS this, so [iodev+0x40]==0 and the cold
/// async full read no-ops EMPTY (b80 2->0, never resident=3). Calling it before the
/// submit routes the read through the bound branch. Internally gated by 0x14240acd0(
/// [0x143d872e0]) which needs the IO worker registry [0x144843038+0x18]!=0. Decoded in
/// bd b80-mount-routine-0x140e6e8d0-recipe-and-guard-open-question-2026-06-21.
pub(crate) const IODEV_MOUNT_OPEN_RVA: usize = 0xe6e8d0;
/// The iodev getter 0x140e6e060() -> iodev (lazily creates the singleton if null).
pub(crate) const IODEV_GETTER_RVA: usize = 0xe6e060;
/// ROOT-CAUSE FIX (b80-ROOTCAUSE-worker-empty-iodev-dir-string-...): the cold full read
/// completes EMPTY because the worker builds a MALFORMED save path -- the request's
/// directory std::u16string is unset (the worker's `"%s\%s%s%s"` format yields a bare
/// `.sl2`). The LIVE title->Continue boot populates that directory via the iodev state
/// machine (opcode 0x17/0x18 handler 0x140e6ded0): it builds `<userdata>/EldenRing/<steamid>/`
/// then installs it on the path DB. The menu-free cold path skips that opcode, so the
/// directory is never set. PRE-submit replay is REFUTED (io20=[iodev+0x20] is NULL before
/// submit; bd b80-COLD-FIX-REFUTED-...). The correct replay is POST-submit, on the LIVE
/// io20, in the SAME game-task invocation (tightest race vs the worker drain):
///   1. SAVE_DIR_BUILDER 0x140e0e680(rcx=&wrapper): self-fetches the userdata folder
///      (SHGetFolderPathW CSIDL 0x1a) + Steam id (0x140e8d550) and formats `%s/EldenRing/%s/`
///      (fmt @0x142bda858) into the wrapper. Guarded by the Steam interface pointer
///      *0x143b48ff0 being non-null (else it would deref null).
///   2. SAVE_DIR_SETTER 0x14240a2a0(rcx=io20 path-DB, edx=slot=0, r8=raw char16_t*): stores
///      the directory into the path database (via 0x14240dce0 -> entry+0xb0, which COPIES
///      our buffer) -- exactly what the opcode-0x17/0x18 handler does. r8 is the RAW data
///      pointer (cap>=8 ? heap ptr @+0x08 : &SSO @+0x08), NOT the wrapper object.
pub(crate) const SAVE_DIR_BUILDER_RVA: usize = 0xe0e680;
pub(crate) const SAVE_DIR_SETTER_RVA: usize = 0x240a2a0;
/// The wrapper's stateful allocator getter (0x141eba960): `call 0x141ebb680; add rax,0x28`
/// -- a trivial singleton accessor returning the arena ptr SAVE_DIR_BUILDER stores at the
/// wrapper's +0x00 (the string's stateful allocator). Must be installed before the builder.
pub(crate) const SAVE_DIR_ALLOC_GETTER_RVA: usize = 0x1eba960;
/// Path-DB slot-entry lookup (0x14240c270): rcx=collection ([io20]), edx=key ([io20+8]) ->
/// entry (find-or-create; idempotent post-setter). The setter writes the directory into
/// `entry+0xb0`. Used for the post-setter readback.
pub(crate) const SAVE_DIR_SLOT_LOOKUP_RVA: usize = 0x240c270;
/// Steam-interface guard pointer (abs 0x143b48ff0): SAVE_DIR_BUILDER derefs the Steam
/// interface to read the account id; if this is null the builder must be skipped.
pub(crate) const STEAM_INTERFACE_GUARD_RVA: usize = 0x3b48ff0;
/// SAVE_DIR_BUILDER's output is a MSVC `basic_string<char16_t, ..., StatefulAllocator>`
/// (the stateful allocator occupies the first member): allocator ptr at +0x00, the _Bx
/// SSO/heap union at +0x08 (8 char16 SSO when cap<8, else `char16_t*`), _Mysize (code units)
/// at +0x18, _Myres (capacity) at +0x20. A default-empty string has size=0 and cap=7. The
/// builder ASSUMES a pre-constructed empty string, so we pre-init allocator/+0x20=7 before
/// the call. (This differs from a stateless-allocator string whose data union is at +0x00.)
pub(crate) const U16STRING_ALLOC_OFFSET: usize = 0x00;
pub(crate) const U16STRING_DATA_OFFSET: usize = 0x08;
pub(crate) const U16STRING_SIZE_OFFSET: usize = 0x18;
pub(crate) const U16STRING_CAP_OFFSET: usize = 0x20;
pub(crate) const U16STRING_SSO_CAP: usize = 7;
/// [iodev+0x40] = the device-ready/bound byte flag (0 cold; set by the mount above).
pub(crate) const IODEV_READY_FLAG_40_OFFSET: usize = 0x40;
/// [iodev+0x30] = the OS file-handle slot (0xffffffff invalid until the mount opens it).
pub(crate) const IODEV_OS_HANDLE_30_OFFSET: usize = 0x30;
/// The FD4 IO worker REGISTRY singleton (abs 0x144843038); its size/count is at +0x18.
/// The mount's guard 0x14240acd0 bails (no open) when [registry+0x18]==0 (no workers
/// registered), so logging it tells us whether the mount can fire at the cold state.
pub(crate) const IO_WORKER_REGISTRY_RVA: usize = 0x4843038;
pub(crate) const IO_WORKER_REGISTRY_COUNT_18_OFFSET: usize = 0x18;
/// The FD4 IO worker MANAGER singleton (abs 0x144852f88) the read job is posted to. The
/// enqueue 0x14240e420 IMMEDIATELY DISCARDS the request (no-op completion 0x14240a000,
/// status 0xe, b80 2->0 in one frame) when [worker+0x19]!=0 (the worker no-accept/shutdown
/// byte) @0x14240e472. Prime suspect for the read-completes-empty wall (b80-DEVICE-MOUNT-
/// REFUTED-...).
pub(crate) const FD4_IO_WORKER_MGR_RVA: usize = 0x4852f88;
pub(crate) const FD4_IO_WORKER_NOACCEPT_19_OFFSET: usize = 0x19;
/// The worker's job QUEUE fields the normal (non-discard) enqueue pushes to: 0x14240e420
/// pushes onto [worker+0x8] (via 0x14240c060) and [worker+0x10] (via 0x14240f2c0). Reading
/// these before vs after the submit DISTINGUISHES enqueued (queue changes) from DISCARDED
/// (queue unchanged) -- the decisive fork for the read-completes-empty wall.
pub(crate) const FD4_IO_WORKER_QUEUE_08_OFFSET: usize = 0x8;
pub(crate) const FD4_IO_WORKER_QUEUE_10_OFFSET: usize = 0x10;
/// The FD4 IO thread POOL singleton (abs 0x144853048).
pub(crate) const FD4_IO_POOL_RVA: usize = 0x4853048;
/// The 2nd discard gate 0x141ee1240 searches the worker-registry's intrusive list at
/// [registry+0x28] for a node matching a key from the calling context (lock 0x141ee05f0);
/// returns false (=> DISCARD) when not found (e.g. the calling thread is not a registered
/// IO context). Empty when [[registry+0x28]] == [registry+0x28].
pub(crate) const IO_WORKER_REGISTRY_LIST_28_OFFSET: usize = 0x28;
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
pub(crate) const WND_GET_SYSTEM_MENU_KEEP: i32 = false as i32;
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
/// ONLINE-DISABLE (headless offline boot, no "Unable to start in online mode" modal).
/// `GameMan::IsOnlineMode` getter 0x14067a030 = `mov rax,[rip+..]; movzx eax,[rax+0xbc8]; ret`
/// (the canonical online/offline flag, default 1=online, read by ~22 consumers incl. the boot
/// login flow). Patching the getter body to `xor eax,eax; ret` forces every consumer onto the
/// game's own OFFLINE branch, so the boot never attempts online login and the connection-error
/// modal is never raised. Single leaf accessor, no side effects -> equivalent to "Play Offline";
/// no save/crash risk. Verified (self-disasm, online-disable RE 2026-06-17): first byte 0x48.
pub(crate) const ONLINE_DISABLE_RVA: usize = 0x67a030;
pub(crate) const ONLINE_DISABLE_EXPECTED_FIRST: u8 = 0x48;
/// `xor eax,eax; ret` -- returns 0 (offline) for the whole getter (the original body is 15
/// bytes followed by the next function, so a 3-byte stub is self-contained).
pub(crate) const ONLINE_DISABLE_STUB: [u8; 3] = [0x31, 0xc0, 0xc3];
pub(crate) const ONLINE_DISABLE_PATCH_LEN: usize = 3;
pub(crate) const ONLINE_DISABLE_BYTE_STEP: usize = 1;
/// Foreground-force: `CS::CSWindowImp::IsGameInForeground` (0x14266def0,
/// `return this->windowHandle == GetForegroundWindow()`) is the engine's foreground oracle; the
/// present/flip pacer `UpdateFlipTiming` (0x140e829d0) and friends throttle the game to a few fps
/// when it returns false. An UNFOCUSED probe window therefore runs at ~6 fps and never boots in the
/// runtime cap. Patch it to `mov al,1; ret` so the game always believes it is foreground -> full
/// speed regardless of focus. Safe for the probe: input is blocked, and "always foreground" only
/// removes the background throttle/pause. Verified prologue first byte 0x40 (`push rbx`).
// NB: address ground-truthed against the deobf/live binary (scripts/disas-deobf.sh), NOT the Ghidra
// dump -- the dump placed this fn at 0x14266def0 but the live entry is 0x14266df00 (dump<->deobf has
// regional shifts; trust the deobf binary for addresses to patch/call).
pub(crate) const FOREGROUND_FORCE_RVA: usize = 0x266df00;
pub(crate) const FOREGROUND_FORCE_EXPECTED_FIRST: u8 = 0x40;
/// `mov al,1; ret` -- returns true (foreground) for the whole getter.
pub(crate) const FOREGROUND_FORCE_STUB: [u8; 3] = [0xb0, 0x01, 0xc3];
/// Sign-in force (cold save-load gate). The SaveLoad2 storage-select op ctor (deobf 0x14240f1b0)
/// creates its runnable ONLY if the sign-in check returns true AND the user index is <= 3; cold
/// (no signed-in user) both fail, so the op is null and the load FSM parks (the b80 wall). Patch
/// both gate fns to pass so the cold menu-free path loads as if signed in as user 0. Addresses
/// ground-truthed against the deobf/live binary (the Ghidra dump's FUN_1424129a0 / FUN_14240f480
/// are shifted; live entries below). Scoped to the cold-mount attempt, not attach.
/// `CS::..::IsSignedIn`-class check (dump FUN_1424129a0) -> always true.
pub(crate) const SIGNIN_FORCE_RVA: usize = 0x24129b0;
pub(crate) const SIGNIN_FORCE_EXPECTED_FIRST: u8 = 0x40;
pub(crate) const SIGNIN_FORCE_STUB: [u8; 3] = [0xb0, 0x01, 0xc3]; // mov al,1; ret
/// User-index resolver (dump FUN_14240f480) -> return 0 (valid index, <= 3) instead of 0xffffffff.
pub(crate) const USERINDEX_FORCE_RVA: usize = 0x240f490;
pub(crate) const USERINDEX_FORCE_EXPECTED_FIRST: u8 = 0x4c;
pub(crate) const USERINDEX_FORCE_STUB: [u8; 3] = [0x31, 0xc0, 0xc3]; // xor eax,eax; ret
/// Login-readiness predicate 0x140cab230 (`sub rsp,0x18; ...`, returns 1 only if all 3 session
/// mgrs == 2). The boot/menu network-flow step calls it to decide ONLINE-attempt vs OFFLINE; a
/// non-zero return makes it attempt online login, which FAILS offline -> the connection-error
/// modal re-pops on every menu transition (the popup LOOP). Patching it to `xor eax,eax; ret`
/// (return "not ready") makes the flow take the clean OFFLINE fork and NEVER attempt online.
/// Same 3-byte stub; first byte 0x48 (verified disasm). Applied with the getter patch.
pub(crate) const ONLINE_PREDICATE_DISABLE_RVA: usize = 0xcab230;
/// AUTO-ACCEPT every `CS::MessageBoxDialog` popup that appears BEFORE the character is in-world
/// (connection-error, EULA, warnings, "save data" notices, ...), so the headless autoload never
/// stops on a startup modal. We hook the dialog's finished-poll getter 0x1407b0cf0
/// (`cmp [rcx+0x25e8],2; setge al; ret`, rcx=dialog) and, for the MessageBoxDialog vtable only,
/// write the result fields (button=OK, state=decided) and return "finished" -- exactly as if OK
/// were pressed. Scoped by vtable + pre-in-world so in-game dialogs + the load flow are untouched.
/// Verified self-disasm (online-disable RE 2026-06-17 + local disasm).
#[repr(usize)]
pub(crate) enum MsgBoxRva {
    ForceStop = 0x78dfd0,
    FinishedGetter = 0x7b0cf0,
    Builder = 0x9275b0,
    OnDecide = 0x927ba0,
    DialogVtable = 0x2b03550,
}

pub(crate) const MSGBOX_FINISHED_GETTER_RVA: u32 = MsgBoxRva::FinishedGetter as u32;
pub(crate) const MSGBOX_DIALOG_VTABLE_RVA: usize = MsgBoxRva::DialogVtable as usize;

#[repr(C)]
pub(crate) struct MsgBoxDialogLayout {
    pub(crate) unknown_000: [u8; 0x3b0],
    pub(crate) closing_latch: u8,
    pub(crate) unknown_3b1: [u8; 0x180f],
    pub(crate) confirm_latch: u8,
    pub(crate) unknown_1bc1: [u8; 0xa1f],
    pub(crate) result_button: i32,
    pub(crate) unknown_25e4: [u8; 0x04],
    pub(crate) state: i32,
}

pub(crate) const MSGBOX_RESULT_BUTTON_25E0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, result_button);
pub(crate) const MSGBOX_STATE_25E8_OFFSET: usize = core::mem::offset_of!(MsgBoxDialogLayout, state);
/// Affirmative/OK button index (the consumer treats -1 as "none yet").
pub(crate) const MSGBOX_OK_BUTTON: i32 = false as i32;
/// Dialog state >= 2 satisfies the finished-poll.
#[repr(i32)]
pub(crate) enum MsgBoxState {
    Decided = 2,
}

pub(crate) const MSGBOX_STATE_DECIDED: i32 = MsgBoxState::Decided as i32;
/// CS::SaveRetryDialog vtable (RVA). A MessageBoxDialog SUBCLASS: the wrapper 0x1407af9a0 overrides
/// the base vtable to this AFTER the builder 0x1409275b0 runs. It is the "save/load failed -- Retry?"
/// prompt the offline title flow builds (save-data/profile read error in a degraded/offline env). The
/// auto-accept must recognize it by THIS vtable -- not the base MessageBoxDialog vtable (0x2b03550) --
/// or it bails before dismissing (the vtable mismatch is why auto-accept never fired). bd
/// offline-title-modal-is-saveretrydialog + press-any-button-golden-lever-job1e8-readiness-2026-06-23.
pub(crate) const SAVE_RETRY_DIALOG_VTABLE_RVA: usize = 0x2aaabf8;
/// SaveRetryDialog fade gate the OK-handler (0x78e030) reads: it commits/closes only when
/// fade_current (+0x1278) <= fade_target (+0x2300). Writing fade_current = fade_target bits makes it
/// commit on the first frame (no fade-in animation = no visible flash) instead of ~20 frames.
pub(crate) const MSGBOX_FADE_CURRENT_1278_OFFSET: usize = 0x1278;
pub(crate) const MSGBOX_FADE_TARGET_2300_OFFSET: usize = 0x2300;
pub(crate) const MSGBOX_FINISHED_TRUE: u8 = true as u8;
pub(crate) const MSGBOX_FINISHED_FALSE: u8 = false as u8;
pub(crate) const AUTO_ACCEPT_LOG_INTERVAL: usize = 30;
/// Original finished-poll getter trampoline (0 until the hook installs).
pub(crate) static MSGBOX_FINISHED_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static AUTO_ACCEPT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const AUTO_ACCEPT_NOT_INSTALLED: usize = 0;
pub(crate) const AUTO_ACCEPT_INSTALLED_YES: usize = 1;
pub(crate) static AUTO_ACCEPT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Set once when the local player first exists in-world; gates the auto-accept OFF so in-game
/// MessageBoxDialogs (which need real choices) are never force-accepted.
pub(crate) static IN_WORLD_REACHED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const IN_WORLD_NOT_REACHED: usize = 0;
pub(crate) const IN_WORLD_REACHED_YES: usize = 1;
/// DIAGNOSTIC: identify the REAL connection-error dialog (the inferred MessageBoxDialog vtable
/// 0x142b03550 did NOT match -- the auto-accept never fired). Hook the dialog builder
/// 0x1409275b0 to log each created dialog's vtable/class + args (the FMG message id is in an
/// arg) + caller; and log every distinct vtable that polls the finished-getter pre-world.
pub(crate) const MSGBOX_BUILDER_RVA: u32 = MsgBoxRva::Builder as u32;
pub(crate) static MSGBOX_BUILDER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MSGBOX_BUILDER_LOG: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const MSGBOX_BUILDER_LOG_MAX: usize = TraceSampleLimit::Value24 as usize;
/// Native policy/ToS surface oracle: constructor 0x1409b5970 builds the TosTitle UI object and
/// binds asset UI paths such as `TosTitle`, `TosTitle/Text`, and the ToS_win64-backed text body.
/// This is NOT a generic string-presence check; a hit means the live policy/privacy screen object
/// was constructed during runtime. Any hit is invalid product proof.
pub(crate) const POLICY_TOS_TITLE_CTOR_RVA: u32 = 0x9b5970;
pub(crate) const POLICY_TOS_TITLE_CTOR_WRAPPER_RVA: u32 = 0x9b6070;
pub(crate) const POLICY_TOS_SELECTOR_WRAPPER_RVA: u32 = 0x9b6140;
pub(crate) const POLICY_TOS_SELECTOR_CTOR_RVA: u32 = 0x9b49f0;
pub(crate) const POLICY_TOS_SELECTOR_VTABLE_RVA: usize = 0x2b27788;
pub(crate) const POLICY_TOS_TITLE_VTABLE_RVA: usize = 0x2b28100;
pub(crate) const POLICY_TOS_TITLE_TEXT_PATH_RVA: usize = 0x2b27330;
pub(crate) static POLICY_TOS_TITLE_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_TITLE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const POLICY_TOS_TITLE_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const POLICY_TOS_TITLE_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static POLICY_TOS_TITLE_TOTAL_BUILDS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Count of TosMultiLangDialog builds our wrapper skipped (zero-input ToS-modal
/// suppression). Non-zero only when `policy_tos_suppress_enabled()` is on; the
/// suppressed build returns null, mimicking the wrapper's native allocation-failure
/// path so the unnecessary startup ToS modal is never constructed.
pub(crate) static POLICY_TOS_TITLE_SUPPRESSED_BUILDS: AtomicUsize = AtomicUsize::new(0);
/// Return value our suppressed ToS-modal wrapper hands back: 0 (null), identical to the
/// native wrapper's allocation-failure return, a path the caller already tolerates.
pub(crate) const POLICY_TOS_MODAL_SUPPRESSED_RETURN: usize = 0;
pub(crate) static POLICY_TOS_TITLE_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_ARG_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_ARG_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_ARG_R9: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_STACK_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST: usize = 0x8;
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_RECORD: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native policy/status predicate 0x1409b72b0: returns true if the policy gate at 0x140e4fda0
/// is set, otherwise falls back to `[this+8]+0x29c0`. Hooked passively to explain legal/status
/// gate failures in direct/offline runs; never used to auto-accept or skip the UI.
pub(crate) const POLICY_TOS_STATUS_PREDICATE_RVA: u32 = 0x9b72b0;
pub(crate) const POLICY_TOS_FLAG_SETTER_RVA: u32 = 0x9b6b30;
pub(crate) static POLICY_TOS_STATUS_PREDICATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_FLAG_SETTER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static POLICY_TOS_STATUS_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_STATUS_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_FLAG_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_FLAG_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_STATUS_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_FORCE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_BEFORE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_AFTER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static START_POLICY_TOS_TITLE_HOOK: Once = Once::new();
/// Native server/login status-text formatter. Static asset/native scan (see
/// `target/autoresearch/server-semaphore-assets/server-semaphore-static-summary.json`) maps
/// `GR_System_Message_win64.fmg` status IDs 401120/401150/401160/401165 to state records at
/// 0x142acbe40. Product proof must fail if this online/login status UI appears.
pub(crate) const SERVER_STATUS_FORMATTER_RVA: u32 = 0x83ac60;
pub(crate) const SERVER_STATUS_RECORD_STATE_OFFSET: usize = 0x0;
pub(crate) const SERVER_STATUS_RECORD_TEXT_ID_OFFSET: usize = 0x10;
pub(crate) const SERVER_STATUS_CHECKING_NETWORK_TEXT_ID: usize = 401_120;
pub(crate) const SERVER_STATUS_LOGGING_IN_TEXT_ID: usize = 401_150;
pub(crate) const SERVER_STATUS_RETRIEVING_DATA_TEXT_ID: usize = 401_160;
pub(crate) const SERVER_STATUS_SAVING_DATA_TEXT_ID: usize = 401_165;
pub(crate) static SERVER_STATUS_FORMATTER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SERVER_STATUS_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const SERVER_STATUS_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const SERVER_STATUS_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static SERVER_STATUS_TOTAL_SEEN: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static SERVER_STATUS_LAST_STATE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static SERVER_STATUS_LAST_TEXT_ID: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static START_SERVER_STATUS_HOOK: Once = Once::new();
pub(crate) static AUTO_ACCEPT_VT_LAST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static AUTO_ACCEPT_VT_LOG: AtomicUsize = AtomicUsize::new(0);
pub(crate) const AUTO_ACCEPT_VT_LOG_MAX: usize = 24;
/// CS::SceneObjProxy ctor 0x14074a700 -- the fn the title dialog-build path runs to wrap the live
/// host MenuWindow in a transient SceneObjProxy. Disasm-verified prologue: `mov %rdx,%rbx`
/// (0x14074a720) then store `mov %rbx,0x20(%rsi)` (0x14074a735) -> proxy+0x20 = the incoming RDX =
/// the engine-VERIFIED MenuWindow (probe-6 proved the TitleTopDialog factory rdx was a std::function
/// delegate, NOT the MenuWindow). We MinHook this ctor at process attach and LATCH the validated
/// MenuWindow (arg2/rdx) on EVERY valid call (most-recent live host window wins) so the live-dialog
/// path reuses it as the Load-Game factory 0x14081ead0 rdx
/// (bd live-dialog-probe6-factory-fires-returns-dialog-rdx-not-menuwindow-2026).
pub(crate) const SCENE_OBJ_PROXY_CTOR_RVA: u32 = 0x74a700;
/// Trampoline for the SceneObjProxy-ctor latch hook (0 = unset).
pub(crate) static SCENE_OBJ_PROXY_CTOR_ORIG: AtomicUsize = AtomicUsize::new(0);
/// The host MenuWindow* latched from the SceneObjProxy ctor (incoming rdx) at title build. 0 until
/// the title builds. Updated on every VALID (vtable-checked) call. Read by
/// `locate_live_loadgame_node` (SeqCst); fail-closed when still 0.
pub(crate) static LATCHED_MENU_WINDOW: AtomicUsize = AtomicUsize::new(0);
/// One-shot install guard for the MenuWindow-latch factory hook (mirrors AUTO_ACCEPT_INSTALLED).
pub(crate) static MENU_WINDOW_LATCH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const MENU_WINDOW_LATCH_NOT_INSTALLED: usize = 0;
pub(crate) const MENU_WINDOW_LATCH_INSTALLED_YES: usize = 1;
pub(crate) static START_MENU_WINDOW_LATCH: Once = Once::new();
/// One-shot install guard for the SAVE-SAFE c30-writer diagnostic hook (mirrors
/// MENU_WINDOW_LATCH_INSTALLED). Installed unconditionally at process attach; the
/// hook is a pure passthrough that logs the c30-write gate, c30 before/after, and a
/// window of the resident save buffer to diagnose why GameMan+0xc30 stays default.
pub(crate) static C30_WRITER_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) const C30_WRITER_HOOK_NOT_INSTALLED: usize = 0;
pub(crate) const C30_WRITER_HOOK_INSTALLED_YES: usize = 1;
pub(crate) static START_C30_WRITER_HOOK: Once = Once::new();
/// Rate limit for the c30-writer diagnostic log: only the first few calls are logged
/// (the cold deserialize drives a small bounded number of c30-writer entries).
pub(crate) static C30_WRITER_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const C30_WRITER_LOG_MAX: usize = 8;
/// Bytes of the resident save buffer (rdx) to dump as hex from the c30-writer ENTER,
/// so the real target map record can be spotted offline. Read-only header window.
pub(crate) const C30_WRITER_BUFFER_DUMP_BYTES: usize = 0x40;
/// The live MessageBoxDialog captured at build time (the connection-error / startup popup), so
/// the game task can force its result fields (OK + decided) each frame until the caller consumes
/// it. The finished-getter 0x1407b0cf0 is NOT polled for this dialog, so writing the fields
/// directly is the dismiss lever. 0 = none captured.
pub(crate) static CONNECTION_ERROR_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Last vtable-validated MessageBoxDialog built by the game. Unlike CONNECTION_ERROR_DIALOG this
/// is never used to auto-dismiss; telemetry reads it at the end of a run to fail the oracle if a
/// blocking dialog is still alive after character/world load.
pub(crate) static MSGBOX_LAST_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_TOTAL_BUILDS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MSGBOX_POSTLOAD_BUILDS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MSGBOX_LAST_ARG_RCX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_LAST_ARG_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_LAST_ARG_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MSGBOX_LAST_ARG_R9: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static DISMISS_WRITE_LOG: AtomicUsize = AtomicUsize::new(0);
/// The dialog pointer OnDecide was last fired on, so we press OK exactly ONCE per dialog instead
/// of every frame (re-dispatching every frame keeps the dialog stuck "deciding" and it never
/// closes). A newly-built dialog has a different pointer, so it gets its own single OK.
pub(crate) static LAST_ONDECIDE_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// CS::MessageBoxDialog OnDecide/finalize (sub-object vtable slot 13) -- the genuine OK handler:
/// reads the chosen-button index [dialog+0x25e0] (builder-defaulted to OK) and dispatches it,
/// driving the dialog to emit "stop" to its parent MenuWindowJob (which then tears it down).
/// This is the verified headless dismiss: call with rcx=dialog. (Field writes do NOT close it --
/// +0x25e8 is the button COUNT, +0x25e0 the chosen index; both are config/output, not triggers.)
pub(crate) const MSGBOX_ONDECIDE_RVA: usize = MsgBoxRva::OnDecide as usize;
/// Force-stop / notify-owner-closed 0x14078dfd0(rcx=dialog): if owner [dialog+0x1c80]!=0 ->
/// owner->vtable[+0x10](dialog); else StepResult(3=stop)+EmitResult. Directly emits "stop" to
/// the parent MenuWindowJob so it tears the dialog down -- a more direct dismiss than OnDecide
/// (which only moved the selection to OK). Acceptable because the connection-error OK is a no-op.
pub(crate) const MSGBOX_FORCE_STOP_RVA: usize = MsgBoxRva::ForceStop as usize;
/// Startup modal handling is lifecycle-driven by `startup_modal_blocking_state`, not by a fixed
/// grace window.
// ============================================================================================
// IN-PROCESS MENU INPUT DRIVER (verified RE 2026-06-17). The main menu (built by SetState(2)=
// BeginLogo) reads input from the keystate bitmap inputmgr+0x90+eventId (edge-triggered &1).
// Confirm=0x3d, vertical-move=0x0/0x45. The Load-Game item d180 is INPUT-GATED -- it only ticks
// (and so is captured by the leaf/iterator hooks) once the cursor is navigated ONTO it. Main-menu
// order: Continue(0), Load Game=d180(1), so ONE Down from the default reaches Load Game. We inject
// Down taps in-process (NO host input, NO window focus) until d180 is captured, then STAGE 2
// invokes its functor directly -- so we never Confirm a wrong item (no New-Game/save-write risk).
// ============================================================================================
/// inputmgr keystate bitmap offset (inputmgr = [0x143d6b7b0]); bit0 = pressed-this-frame (edge).
pub(crate) const INPUTMGR_BITMAP_90_OFFSET: usize = 0x90;
pub(crate) const MENU_EVENT_PRESSED_BIT: u8 = true as u8;
/// Front-end menu event ids (verified): Confirm/OK, and the two vertical-move candidates (one is
/// Down, one Up -- we inject both; only Down moves the cursor down, Up saturates at the top so it
/// is harmless from Continue). We do NOT inject Confirm (STAGE 2 invokes d180's functor instead).
#[repr(usize)]
pub(crate) enum MenuEventId {
    MoveA = 0x00,
    Confirm = 0x3d,
    MoveB = 0x45,
}

pub(crate) const MENU_EVENT_CONFIRM_3D: usize = MenuEventId::Confirm as usize;
pub(crate) const MENU_EVENT_MOVE_A_00: usize = MenuEventId::MoveA as usize;
pub(crate) const MENU_EVENT_MOVE_B_45: usize = MenuEventId::MoveB as usize;
/// AUTO-CONFIRM (observe natural flow past the modal): tap Confirm on a SET/GAP cycle slow enough
/// that the connection-error modal (which appears ~90 frames after the press) gets its own tap.
pub(crate) const AUTO_CONFIRM_CYCLE_FRAMES: u64 = 120;
pub(crate) const AUTO_CONFIRM_SET_FRAMES: u64 = 3;
pub(crate) const AUTO_CONFIRM_LOG_INTERVAL: u64 = 60;
pub(crate) static AUTO_CONFIRM_FRAME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static AUTO_CONFIRM_MODAL_SEEN: AtomicUsize = AtomicUsize::new(0);
/// Menu list cursor (highlighted index) and item count, on the list object (cursor getter
/// 0x140739e20 = `mov eax,[rcx+0xd4]`). Used to LOG the live cursor (diagnostic) while injecting.
#[repr(C)]
pub(crate) struct MenuListLayout {
    pub(crate) unknown_000: [u8; 0xd0],
    pub(crate) count: i32,
    pub(crate) cursor: i32,
}

pub(crate) const MENU_LIST_CURSOR_D4_OFFSET: usize = core::mem::offset_of!(MenuListLayout, cursor);
pub(crate) const MENU_LIST_COUNT_D0_OFFSET: usize = core::mem::offset_of!(MenuListLayout, count);
/// Down-tap cadence: assert the move bit for SET frames (edge), then GAP idle frames (so the menu
/// sees a clean single edge + auto-repeat is avoided), one cursor step per cycle.
#[repr(u64)]
pub(crate) enum MenuTapSchedule {
    SetFrames = 2,
    GapFrames = 10,
    MaxTaps = 12,
}

pub(crate) const MENU_TAP_SET_FRAMES: u64 = MenuTapSchedule::SetFrames as u64;
pub(crate) const MENU_TAP_GAP_FRAMES: u64 = MenuTapSchedule::GapFrames as u64;
pub(crate) const MENU_TAP_CYCLE_FRAMES: u64 = MENU_TAP_SET_FRAMES + MENU_TAP_GAP_FRAMES;
/// Max Down taps before giving up (menu has 5 items; cap generously). Down saturates at the last
/// item (no wrap), so this also bounds an overshoot.
pub(crate) const MENU_NAV_MAX_TAPS: u64 = MenuTapSchedule::MaxTaps as u64;
/// Per-frame counter for the menu-input nav (starts when nav begins, after the modal grace).
pub(crate) static MENU_NAV_FRAME: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Forced entry-diagnostic counter (log the first few menu_input_drive calls unconditionally,
/// before any early return, so we can see the inputmgr value + capture state).
pub(crate) static MENU_DRIVE_ENTER_LOG: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const MENU_DRIVE_ENTER_LOG_MAX: usize = TraceSampleLimit::Value8 as usize;
// ============================================================================================
// DETERMINISTIC MENU INPUT PROBE (instrumentation oracle, er-effects-input-probe.txt). After the
// menu opens, inject a single Down tap (Continue->Load Game) at a KNOWN frame, observe a window
// with NO further input, then inject Confirm at a KNOWN frame. Because WE choose the inject
// frames, the decisive question is frame-precise: does the Load-Game leaf d180 tick its leaf
// Update (0x1407ad1c0 -> MENU_D180_LEAF_TICKED grows) on HIGHLIGHT alone (between Down and
// Confirm), or only at Confirm? This is targeted input used as a MEASUREMENT (NOT the zero-input
// deliverable); the Confirm drives the native load so the full chain is captured at a known frame.
// ============================================================================================
/// Probe frame counter (per own_stepper idx10 call, starting when the probe first runs after the
/// menu opens). Schedule below is in these frames.
pub(crate) static INPUT_PROBE_FRAME: AtomicUsize = AtomicUsize::new(0);
/// Set to 1 once the probe is active so the hot menu hooks can cheaply enable the extra
/// leaf-tick accounting (MENU_D180_LEAF_TICKED) without a per-frame file-exists check.
pub(crate) static INPUT_PROBE_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch: set when the d180 leaf tick is observed during the HIGHLIGHT window (decisive).
pub(crate) static INPUT_PROBE_D180_PRECONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Snapshot of MENU_D180_LEAF_TICKED captured at the Down-inject frame; HIGHLIGHT growth is
/// measured strictly above this baseline.
pub(crate) static INPUT_PROBE_DOWN_LEAF_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Count of genuine d180 leaf-Update ticks (bumped ONLY by cap_menu_item_update_hook when the
/// ticked item classifies to dialog_factory). Distinct from MENU_LOAD_GAME_ITEM, which the static
/// sequence-iter walk can also set without d180 actually ticking.
pub(crate) static MENU_D180_LEAF_TICKED: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Frame to begin the single Down injection (settle the opened menu first).
pub(crate) const INPUT_PROBE_DOWN_START: u64 = 120;
/// Assert the move bit for this many consecutive frames = one clean edge (one cursor step).
pub(crate) const INPUT_PROBE_DOWN_TAP_FRAMES: u64 = 2;
/// Observation window AFTER the Down, with NO input, before the Confirm injection.
pub(crate) const INPUT_PROBE_HIGHLIGHT_FRAMES: u64 = 180;
/// Frame to begin the Confirm injection (= Down end + highlight window).
pub(crate) const INPUT_PROBE_CONFIRM_START: u64 =
    INPUT_PROBE_DOWN_START + INPUT_PROBE_DOWN_TAP_FRAMES + INPUT_PROBE_HIGHLIGHT_FRAMES;
pub(crate) const INPUT_PROBE_CONFIRM_TAP_FRAMES: u64 = 2;
pub(crate) const INPUT_PROBE_LOG_INTERVAL: u64 = 20;

// ============================================================================================
// SELF-DRIVEN GAMEPAD NAV INJECTION (instrument-capture). Distinct from the disproven
// inputmgr+0x90 keystate write (PROVEN non-functional): this injects at the XInput poll source
// (XInputGetState, the stage the game actually reads gamepad from), so a synthesized D-pad Down
// reaches the real input pipeline. The block stays ON (user input suppressed) while the hook
// fabricates the pad state on a schedule, cycling the title-menu cursor so the input/focus-gated
// row populate fires and the row-push/csmenu-ctor hooks capture WHO triggers it -- with the
// user's input blocked so nothing pollutes. Capture-only: D-pad Down nav, NEVER Confirm/A (no
// load, no save write).
// ============================================================================================
/// XInput poll counter, incremented each XInputGetState call while inject-nav is active and the
/// menu is open. The schedule below is in these poll-frames.
pub(crate) static INJECT_NAV_FRAME: AtomicUsize = AtomicUsize::new(0);
/// XINPUT_GAMEPAD.wButtons D-pad Down bit (the menu "move down" gamepad input).
pub(crate) const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
/// Settle the freshly-opened menu before injecting (poll-frames).
pub(crate) const INJECT_NAV_SETTLE_FRAMES: usize = 90;
/// Down asserted for this many consecutive poll-frames = one clean edge (one cursor step).
pub(crate) const INJECT_NAV_TAP_LEN: usize = 4;
/// Released gap between taps (edge re-arm; menu nav is edge-triggered, not auto-repeat).
pub(crate) const INJECT_NAV_GAP_LEN: usize = 16;
/// One tap+gap cycle length.
pub(crate) const INJECT_NAV_CYCLE: usize = INJECT_NAV_TAP_LEN + INJECT_NAV_GAP_LEN;
/// Number of Down taps to drive. The problem is fully deterministic: the cursor starts on
/// Continue (index 0) and Load Game is index 1, so EXACTLY ONE Down reaches it. There is no state
/// of knowledge that justifies more than one tap, so this is a literal 1 (not a tunable).
pub(crate) const INJECT_NAV_MAX_CYCLES: usize = 1;
/// Throttle the per-tap log.
pub(crate) static INJECT_NAV_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const INJECT_NAV_LOG_FIRST: usize = 20;
/// The current frame's synthesized gamepad wButtons, computed by the per-frame schedule in
/// own_stepper idx10 and READ by the XInput hook (so the schedule lives in one place that runs
/// every frame, instead of the XInput hook which the game may never poll). 0 = no input.
pub(crate) static INJECT_NAV_CUR_BUTTONS: AtomicUsize = AtomicUsize::new(0);
/// DInput keyboard scancode DIK_DOWN (down-arrow) -- the menu "move down" keyboard input. The
/// menu is keyboard-navigated under Proton with no controller (XInput is not polled), so the
/// schedule drives this via InputBlocker::set_injected_key (stamped into the blocked keyboard
/// state). 0xD0 = DIK_DOWNARROW.
pub(crate) const DIK_DOWN: u8 = 0xd0;
/// No key injected (clears the stamp on gap/settle frames).
pub(crate) const DIK_NONE: u8 = 0;
/// No gamepad buttons asserted this frame.
pub(crate) const INJECT_NAV_NO_BUTTONS: u16 = 0;
/// CURSOR-OFFSET PROBE: with exactly ONE deterministic Down (Continue idx0 -> Load Game idx1),
/// snapshot the live TitleTopDialog dwords just BEFORE the Down (cursor should read 0) and again
/// AFTER it settles (cursor should read 1); the dword that goes 0->1 IS the cursor field. This
/// observes the real offset instead of trusting the unverified +0xb0c guess (which the self-fire
/// run read as 0). Frames are relative to the first poll after menu-open.
pub(crate) const CURSOR_PROBE_BASELINE_FRAME: usize = INJECT_NAV_SETTLE_FRAMES - 2;
pub(crate) const CURSOR_PROBE_POSTDOWN_FRAME: usize = INJECT_NAV_SETTLE_FRAMES + 12;
/// Dwords to scan from the dialog base (covers 0..0x2400, the known field range).
pub(crate) const CURSOR_PROBE_SCAN_DWORDS: usize = 0x900;
/// Only dwords in [0, this) are logged as cursor candidates (a row index is small).
pub(crate) const CURSOR_PROBE_SMALL_MAX: u32 = 8;
/// Cap the candidate-dword log per snapshot.
pub(crate) const CURSOR_PROBE_LOG_CAP: usize = 96;
/// "result emitted / closing" latch, set =1 by EmitResult once the dialog begins teardown. We
/// stop calling OnDecide once this is set (avoids re-dispatch / UAF after teardown).
pub(crate) const MSGBOX_CLOSING_LATCH_3B0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, closing_latch);
pub(crate) const MSGBOX_CLOSING_YES: usize = true as usize;
pub(crate) const MSGBOX_LATCH_BYTE_MASK: usize = u8::MAX as usize;
/// THE OK-BUTTON HANDLER 0x14078e030(rcx=dialog) -- the std::function the menu router invokes when
/// OK is pressed. Captured from a real OK-press (commit 0x14078ef20 fired with caller 0x78e09c, in
/// the function entered at 0x78e030). It takes ONLY rcx=dialog: reads the dialog cursor (0x140739e20
/// = [dialog+0xd4]), gets the OK callback (0x14078fbd0 from [dialog+0x1298]), builds the result
/// struct (0x1407411e0), and COMMITS (0x14078ef20(dialog, &struct, 1)) -- which closes the dialog
/// AND emits its result to the parent so the title flow PROCEEDS. Calling this each frame on every
/// captured MessageBoxDialog skips ALL of them generically (connection-error, starting-offline, ...)
/// with no input -- it is exactly what a real OK-press runs. Verified entry: `rex push rbx; ... mov
/// rbx,rcx` at 0x78e030; only rcx used.
pub(crate) const MSGBOX_OK_HANDLER_RVA: usize = 0x78e030;
/// CONFIRM latch [dialog+0x1bc0] u8 -- the field a real OK-press sets. The dialog's own per-frame
/// UPDATE 0x140927d30 reads it -> commit 0x14078ef20 builds the result functor into [dialog+0x10]
/// -> next UPDATE emits stop via EmitResult (sets the +0x3b0 closing latch) -> the dialog TEARS
/// DOWN. OnDecide alone only highlights/dispatches OK WITHOUT closing (the modal stays visible and
/// blocks the title flow); setting this latch is what actually closes it like a real press.
pub(crate) const MSGBOX_CONFIRM_LATCH_1BC0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, confirm_latch);
pub(crate) const MSGBOX_CONFIRM_LATCH_SET: u8 = true as u8;
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
pub(crate) const GAME_MAN_B73_FLAG_OFFSET: usize = GAME_MAN_FLAG_B73_PROBE_OFFSET;
pub(crate) const GAME_MAN_B73_FLAG_SET: u8 = true as u8;
pub(crate) const GAME_MAN_REAL_LOAD_DONE_OFFSET: usize =
    core::mem::offset_of!(GameMan, warp_requested);
pub(crate) const GAME_MAN_REAL_LOAD_DONE_VALUE: i32 = true as i32;
#[repr(C)]
pub(crate) struct ContinueOwnerLayout {
    pub(crate) storage: [usize; 0x40],
}

#[repr(C)]
pub(crate) struct ContinueOwnerFields {
    pub(crate) unknown_000: [u8; 0x12a],
    pub(crate) flag_12a: u8,
    pub(crate) unknown_12b: u8,
    pub(crate) slot: i32,
}

pub(crate) const CONTINUE_OWNER_SLOT_OFFSET: usize =
    core::mem::offset_of!(ContinueOwnerFields, slot);
pub(crate) const CONTINUE_OWNER_FLAG_12A_OFFSET: usize =
    core::mem::offset_of!(ContinueOwnerFields, flag_12a);
pub(crate) const CONTINUE_OWNER_FLAG_12A_VALUE: u8 = false as u8;
pub(crate) const CONTINUE_OWNER_QWORDS: usize =
    core::mem::size_of::<ContinueOwnerLayout>() / core::mem::size_of::<usize>();
pub(crate) const CONTINUE_DRIVE_MIN_TICK: u64 = 120;
pub(crate) const CONTINUE_DRIVE_AFTER_GAME_MAN_TICKS: u64 = u64::MIN;
/// PlayGame load-pair target block, bound to upstream `GameMan::move_map_target`
/// (audit-confirmed equal to the hand-decoded 0x14).
pub(crate) const FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET: usize =
    core::mem::offset_of!(GameMan, move_map_target);
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
pub(crate) const TITLE_STEP_MENU_JOB_WAIT_STATE: i32 = TITLE_STEP_MENU_JOB_WAIT;
pub(crate) const TITLE_PROCEED_GATE_SET_VALUE: u8 = true as u8;
/// Global menu-accept byte 0x144589bdc (RVA 0x4589bdc): the decoded "a button was accepted"
/// flag the input pipeline sets on press, read via getter 0x140e85f50 from TitleTopDialog::update
/// (and 22 other menu accept-gates). When non-zero at the parked title, update runs the open-menu
/// registrar 0x1409b24e0 NATURALLY (build Continue/Load + transfer focus -> select-layer build) --
/// unlike a direct registrar self-fire which opened a competing dialog and reverted. Setting this
/// flag zero-input is the ToS-style "satisfy the accept side-effect" advance (NOT a synthesized
/// DInput/keystate/XInput event). bd title-global-accept-byte-144589bdc-zeroinput-advance-2026.
pub(crate) const TITLE_GLOBAL_ACCEPT_BYTE_RVA: usize = 0x4589bdc;
/// Menu-system manager singleton pointer global 0x143d5dea8 (89 refs). The title press-accept
/// handler 0x1409b1260 does `mov rax,[0x143d5dea8]; if rax: movb [rax],1; jmp registrar 0x1409b24e0`
/// -- it sets the singleton's +0 byte (a "menu-open in progress" flag) then opens the main menu
/// IN PLACE. Replicating this (set the flag, then registrar on the validated TitleTopDialog) is the
/// NARROW title-specific advance that should reach the main menu WITHOUT the language/ToS build that
/// the broad global accept byte over-triggers, and without the competing-dialog revert a bare
/// registrar self-fire caused. bd title-accept-to-registrar-narrow-path-143d5dea8-2026.
pub(crate) const TITLE_MENU_TRANSITION_SINGLETON_RVA: usize = 0x3d5dea8;
pub(crate) const TITLE_MENU_TRANSITION_FLAG_SET_VALUE: u8 = true as u8;
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
pub(crate) const INGAMESTEP_OVERRIDE_TRIGGER_CLEAR: u8 = false as u8;
pub(crate) const MENU_TASK_NULL_STATE_QWORD: usize = NULL_MODULE_BASE;
pub(crate) const MENU_TASK_NULL_PAYLOAD_PTR: usize = NULL_MODULE_BASE;
pub(crate) const MENU_TASK_STATE_PAYLOAD_CODE_OFFSET: usize =
    core::mem::offset_of!(MenuTaskStateLayout, payload_code);
pub(crate) const MENU_TRACE_EVENT_INCREMENT: usize = true as usize;
pub(crate) const TASK_ENQUEUE_TRACE_INCREMENT: usize = true as usize;
pub(crate) static START_GAME_TASK: Once = Once::new();
pub(crate) static START_CONTINUE_TRACE: Once = Once::new();
pub(crate) static START_SAFE_INPUT_HOOKS: Once = Once::new();
pub(crate) static START_SPLASH_SKIP: Once = Once::new();
pub(crate) static START_ONLINE_DISABLE: Once = Once::new();
pub(crate) static START_FOREGROUND_FORCE: Once = Once::new();
pub(crate) static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize =
    AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);
pub(crate) static SAFE_INPUT_CONFIRM_FRAMES_REMAINING: AtomicUsize = AtomicUsize::new(0);

pub(crate) static MENU_CONTINUE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_NEW_OR_LOAD_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_OTHER_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_TASK_UPDATE_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static NATIVE_SUBMIT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static RESULT_EVENT_HANDLER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static RESULT_ACTION_BUILDER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static RESULT_EVENT_WRAPPER_BUILDER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TASK_ENQUEUE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SET_SAVE_SLOT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SAVE_REQUEST_PROFILE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static REQUEST_SAVE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CURRENT_SLOT_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CONTINUE_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static COMBINED_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MAP_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SAVE_LOAD_STATE_INIT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
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
/// Title CSMenu-controller ("router_this") ctor 0x1409060d8: installs the controller vtable
/// (runtime 0x142afa070) and the +0x1290 selectable-row vector. Hooking it captures the live
/// router_this -- the object that owns the Continue/Load-Game/NewGame rows -- which is NOT
/// field-linked from the TitleTopDialog (a dialog-struct scan misses it). Latched into
/// MENU_ROUTER_THIS so the own-stepper can read its rows + drive the Load-Game select zero-input.
pub(crate) static CAP_CSMENU_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_CSMENU_CTOR_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_CSMENU_CTOR_LOG_FIRST: usize = TraceSampleLimit::Value8 as usize;
/// The captured title CSMenu controller (router_this). 0 until its ctor 0x1409060d8 latches it.
pub(crate) static MENU_ROUTER_THIS: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The title-menu "Load Game" ROW entry (stride-0x210 row whose action functor [entry+0xf8]
/// chains to dialog_factory 0x14081ead0). Captured by the row-push hook's post-build scan. Its
/// layout is the CSMenu-row layout (action at +0xf8), DISTINCT from the FD4 MenuWindowJob d180
/// (+0xa8). Invoking its action builds the ProfileLoadDialog zero-input.
pub(crate) static MENU_LOADGAME_ROW_ENTRY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The matching "Continue" row entry (action -> continue_confirm 0x140b0e180), for reference.
pub(crate) static MENU_CONTINUE_ROW_ENTRY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native title-menu task node whose update wrapper is ContinueWrapper 0x14082bac0. Captured by
/// the FD4 registry enqueue hook after TitleTopDialog::open_menu materializes the native menu.
pub(crate) static MENU_CONTINUE_TASK_NODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native TitleTopDialog Continue MenuMemberFuncJob node whose member function reaches
/// ContinueWrapper 0x14082bac0. This is a passive semantic latch only; product proof must still
/// advance through native accept/submit semantics, not direct-load shortcuts.
pub(crate) static MENU_CONTINUE_MEMBER_NODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Passive native submit/result-chain telemetry. These hooks only call through and record whether
/// product execution entered native submit, result.vtable+0x60, and the action builder; they must
/// never drive load directly.
pub(crate) static NATIVE_SUBMIT_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static NATIVE_SUBMIT_LAST_RESULT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_HANDLER_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_ACTION_BUILDER_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_EVENT_LAST_RESULT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_EVENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_RAW_QWORD0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_FD4_CODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_FD4_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_RESULT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_EVENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WORD0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WORD1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_INSERT_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_ACTION_LAST_INSERT_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_ARG1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_WRAPPER_BUILDER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// router_this ctor RVA and its installed (runtime) primary vtable RVA (= base+this at runtime;
/// on-disk objdump shows 0x2af9270, +0xe00 dump/PE skew).
/// REAL function entry is 0x1409060d0 (`rex push rbp` prologue, objdump-verified); the doc's
/// 0x9060d8 lands AFTER 5 pushes (push rbp/rsi/rdi/r12/r13) -- hooking there installs a
/// trampoline mid-prologue and corrupts the stack, so the prior capture was unreliable.
pub(crate) const CSMENU_CTOR_RVA: u32 = ProfileLoadMenuRva::CsMenuCtor as u32;
pub(crate) const ROUTER_THIS_VTABLE_RVA: usize = 0x02afa070;
/// Row-push functions (RELIABLE .text RVAs, no .rdata skew): rebuild_rows 0x14078d2c0 (bulk
/// emplace) and append_one 0x14078eea0 (single). If EITHER fires headless the Continue/Load rows
/// ARE materialized zero-input (and rcx reaches router_this); if NEITHER fires the interactive
/// menu controller is input-instantiated (the architectural floor). rcx = list-model container;
/// [container+8] = router_this back-ptr.
pub(crate) static CAP_REBUILD_ROWS_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_APPEND_ONE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// FD4/menu registry insertion helper 0x1407a7b60, called directly by TitleTopDialog::open_menu
/// after each menu entry descriptor is built. The existing task_enqueue_7a7b60 hook logs
/// rcx/rdx/ret fingerprints to map where the opened Continue/Load-Game entries are stored.
pub(crate) static CAP_MENU_INSERT_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_MENU_INSERT_LOG_FIRST: usize = TraceSampleLimit::Value24 as usize;

#[repr(C)]
pub(crate) struct CapMenuInsertTraceLayout {
    pub(crate) vtable: usize,
    pub(crate) qword_8: usize,
    pub(crate) qword_10: usize,
    pub(crate) qword_18: usize,
    pub(crate) unknown_20: [u8; 0x18],
    pub(crate) qword_38: usize,
    pub(crate) unknown_40: [u8; 0x10],
    pub(crate) qword_50: usize,
}

pub(crate) const CAP_MENU_INSERT_VTABLE_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, vtable);
pub(crate) const CAP_MENU_INSERT_QWORD_8_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_8);
pub(crate) const CAP_MENU_INSERT_QWORD_10_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_10);
pub(crate) const CAP_MENU_INSERT_QWORD_18_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_18);
pub(crate) const CAP_MENU_INSERT_QWORD_38_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_38);
pub(crate) const CAP_MENU_INSERT_QWORD_50_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_50);
pub(crate) static CAP_ROW_PUSH_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_ROW_PUSH_LOG_FIRST: usize = 12;
/// UNCONDITIONAL row-push capture: log the caller stack of EVERY rebuild_rows/append_one fire
/// (first N), regardless of whether the container is the title menu. Under Model A the row
/// populate fires for the ProfileLoadDialog slot list (not the title Continue/Load list), so the
/// content-gated `inspect_row_container` log would miss it; this captures WHO triggers populate.
pub(crate) static CAP_ROW_PUSH_ALLFIRE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_ROW_PUSH_ALLFIRE_LOG_FIRST: usize = 24;
pub(crate) const REBUILD_ROWS_RVA: u32 = 0x0078d2c0;
pub(crate) const APPEND_ONE_RVA: u32 = 0x0078eea0;
pub(crate) const ROW_CONTAINER_BACKPTR_8: usize = 0x8;
pub(crate) static CAP_SELECTOR_TICK_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_SELECTOR_TICK_LOG_FIRST: usize = TraceSampleLimit::Value4 as usize;
pub(crate) const CAP_SELECTOR_TICK_LOG_INTERVAL: usize = CAP_SELECTOR_TICK_LOG_INTERVAL_TICKS;
/// Selector-owner step (0x140826d50) install-flag field: 0 on the first tick (fires the
/// delegate-installer 0x140828270), 1 afterwards.
#[repr(C)]
pub(crate) struct SelectorStepLayout {
    pub(crate) unknown_000: [u8; 0x68],
    pub(crate) install_flag: u8,
}

pub(crate) const SELECTOR_STEP_INSTALL_FLAG_68_OFFSET: usize =
    core::mem::offset_of!(SelectorStepLayout, install_flag);
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
pub(crate) static TITLE_HANDOFF_COMPLETE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OWNER_PTR: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_OWNER_TRACE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static TITLE_NATIVE_JOB_CALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
pub(crate) static FORCE_PLAY_GAME_CALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
pub(crate) static SUBMIT_PLAY_GAME_PHASE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(SUBMIT_PHASE_INIT);
pub(crate) static FORCE_PLAY_GAME_LAST_STATE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(FORCE_PLAY_GAME_STATE_UNOBSERVED);
pub(crate) static TITLE_PROCEED_GATE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// One-shot latch for the global-accept-byte (0x144589bdc) zero-input title-advance lever.
pub(crate) static TITLE_ACCEPT_BYTE_GATE_FIRED: std::sync::atomic::AtomicBool =
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
pub(crate) static CONTINUE_OWNER_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) const CONTINUE_DRIVE_GM_FIRST_SEEN_UNSET: u64 = 0;
pub(crate) static CONTINUE_DRIVE_GM_FIRST_SEEN_TICK: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(CONTINUE_DRIVE_GM_FIRST_SEEN_UNSET);
pub(crate) static CONTINUE_DRIVE_FIRST_ATTEMPT_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
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
pub(crate) static TITLE_OWNER_SCAN_COUNTDOWN: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_COUNTDOWN_READY);
pub(crate) static SAFE_INPUT_CONFIRM_PULSE_SEQ: AtomicUsize =
    AtomicUsize::new(SAFE_INPUT_FIRST_PULSE_INDEX as usize);
pub(crate) static MENU_TRACE_EVENT_SEQ: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MENU_TRACE_LAST_SEQ: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MENU_TRACE_LAST_HOOK_RVA: AtomicUsize =
    AtomicUsize::new(TRACE_UNKNOWN_TABLE_RVA as usize);
pub(crate) static MENU_TRACE_LAST_TABLE_RVA: AtomicUsize =
    AtomicUsize::new(TRACE_UNKNOWN_TABLE_RVA as usize);
pub(crate) static MENU_TRACE_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_TRACE_LAST_STATE_QWORD: AtomicUsize =
    AtomicUsize::new(MENU_TASK_NULL_STATE_QWORD);
pub(crate) static MENU_TRACE_LAST_PAYLOAD_PTR: AtomicUsize =
    AtomicUsize::new(MENU_TASK_NULL_PAYLOAD_PTR);
pub(crate) static TASK_ENQUEUE_TRACE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);

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
        // During the zero-input autoload/own-stepper probe, HARD-block every input source
        // (DInput keyboard+mouse + XInput gamepad) so a focused window cannot contaminate the
        // result -- the run must prove the load happens with no real input at any layer. NOTE:
        // under the offline launcher this render loop does NOT run at the title, so the game
        // task drives the same block too (enforce_input_block_now); this branch just covers the
        // in-game/overlay case. Otherwise fall back to the overlay's want-capture heuristic.
        if block_input_enabled() {
            enforce_input_block_now();
        } else {
            blocker.block_from_io(ui.io());
        }

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

type DirectInput8CreateFn =
    unsafe extern "system" fn(HINSTANCE, u32, *const c_void, *mut *mut c_void, *mut c_void) -> i32;

static DIRECTINPUT8_CREATE_FORWARD: AtomicUsize = AtomicUsize::new(DIRECTINPUT_FORWARD_UNRESOLVED);

unsafe fn directinput8_create_forward() -> Option<DirectInput8CreateFn> {
    let cached = DIRECTINPUT8_CREATE_FORWARD.load(Ordering::SeqCst);
    if cached != DIRECTINPUT_FORWARD_UNRESOLVED {
        return Some(unsafe { std::mem::transmute::<usize, DirectInput8CreateFn>(cached) });
    }
    let module = unsafe { LoadLibraryA(PCSTR(DINPUT8_SYSTEM_DLL.as_ptr())) }.ok()?;
    let proc = unsafe { GetProcAddress(module, PCSTR(DIRECTINPUT8_CREATE_SYMBOL.as_ptr())) }?;
    let addr = proc as usize;
    DIRECTINPUT8_CREATE_FORWARD.store(addr, Ordering::SeqCst);
    Some(unsafe { std::mem::transmute::<usize, DirectInput8CreateFn>(addr) })
}

#[unsafe(no_mangle)]
/// # Safety
///
/// This is the DINPUT8.dll proxy export Elden Ring imports. It forwards to the
/// system DINPUT8 implementation after our repo-built DLL is loaded as dinput8.dll.
pub unsafe extern "system" fn DirectInput8Create(
    hinst: HINSTANCE,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> i32 {
    let Some(forward) = (unsafe { directinput8_create_forward() }) else {
        return DIRECTINPUT_FORWARD_ERROR_MOD_NOT_FOUND;
    };
    unsafe { forward(hinst, version, riid, out, outer) }
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

    let initial_state = EffectsState::default();
    arm_product_autoload_from_request(&initial_state.autoload);
    let state = Arc::new(Mutex::new(initial_state));

    // Splash-skip: apply the clean BeginLogo branch-flip as early as possible,
    // from a thread, so it lands before the title state machine runs state 2.
    if splash_skip_enabled() {
        START_SPLASH_SKIP.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-splash-skip".to_owned())
                .spawn(apply_splash_skip);
        });
    }

    // Online-disable: patch GameMan::IsOnlineMode -> always-offline so the boot never attempts
    // online login and the "Unable to start in online mode" modal is never raised -- the headless
    // autoload reaches the real title/main-menu directly. Same early-attach pattern as splash-skip.
    if online_disable_enabled() {
        START_ONLINE_DISABLE.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-online-disable".to_owned())
                .spawn(apply_online_disable);
        });
    }

    // Foreground-force: ALWAYS ON (user directive 2026-06-21 -- "if it works, keep it on"),
    // independent of online-disable. The unfocused-window fps throttle hits during boot (before any
    // cold-mount runs), so patch it at attach so the game always runs full speed regardless of which
    // window holds focus. Verified to make a cold probe boot at 60fps unfocused (was ~6fps). Benign:
    // it only removes the background throttle/auto-pause; input is blocked during probes anyway.
    START_FOREGROUND_FORCE.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("er-effects-foreground-force".to_owned())
            .spawn(apply_foreground_force);
    });

    // MenuWindow latch: install the SceneObjProxy ctor hook (0x14074a700) as early as the
    // splash-skip / online-disable patches, from a thread, so it lands BEFORE the title state
    // machine builds the title dialog during boot. On each VALID call it latches rdx (the engine-
    // verified host MenuWindow*) for the live-dialog Load-Game path; pure latch + passthrough.
    // OPT-IN (off by default): only install when `menu_window_latch_enabled()` is set
    // (env ER_EFFECTS_MENU_WINDOW_LATCH=1 OR GAME_DIR file er-effects-menu-window-latch.txt).
    // When off, the hook is never installed (no MinHook, no detour) -- a clean run has neither.
    if menu_window_latch_enabled() || product_autoload_enabled() {
        START_MENU_WINDOW_LATCH.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-menu-window-latch".to_owned())
                .spawn(install_menu_window_latch_hook);
        });
    }

    // Native/asset-backed policy-window oracle: hook the TosTitle constructor early in product
    // autoload runs. Any hit means the Privacy/ToS surface was constructed and the runtime proof is
    // invalid; this is detection only, never auto-accept.
    if product_autoload_enabled() {
        START_POLICY_TOS_TITLE_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-policy-oracle".to_owned())
                .spawn(install_policy_tos_title_hook);
        });
        START_SERVER_STATUS_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-server-status-oracle".to_owned())
                .spawn(install_server_status_hook);
        });
    }

    // SAVE-SAFE c30-writer diagnostic: install the MinHook on the SOLE GameMan+0xc30
    // writer 0x67bd70 UNCONDITIONALLY at process attach (same early-attach pattern as the
    // MenuWindow latch). Pure passthrough + log of the c30-write gate, c30 before/after,
    // and a window of the resident save buffer -- NO SetState5, NO save write, harmless.
    // OPT-IN (off by default): only install when `c30_writer_diag_enabled()` is set
    // (env ER_EFFECTS_C30_DIAG=1 OR GAME_DIR file er-effects-c30-diag.txt). When off, the
    // hook is never installed (no MinHook, no detour on the hot 0x67bd70 deserialize path).
    if c30_writer_diag_enabled() {
        START_C30_WRITER_HOOK.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-c30-writer-hook".to_owned())
                .spawn(install_c30_writer_hook);
        });
    }

    if safe_input_path().exists() {
        START_SAFE_INPUT_HOOKS.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("er-effects-safe-input-hooks".to_owned())
                .spawn(install_safe_input_hooks);
        });
    }
    if trace_continue_enabled() && !continue_trace_disabled() {
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

    // Skip the hudhook/ImGui DX12 overlay when autoload-only (no overlay needed) OR when explicitly
    // disabled via `overlay_disabled()` (env ER_EFFECTS_NO_OVERLAY=1 / GAME_DIR file
    // er-effects-no-overlay.txt) -- e.g. a golden trace run that wants no extra DX12 hooks/overhead.
    let autoload_without_overlay = state_or_return(&state).autoload.slot().is_some();
    if autoload_without_overlay || overlay_disabled() {
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
    let mut wait_attempts = 0_u64;
    loop {
        match unsafe { CSTaskImp::instance() } {
            Ok(instance) => return instance,
            Err(InstanceError::NotFound(_)) | Err(InstanceError::Null(_)) => {
                wait_attempts = wait_attempts.saturating_add(1);
                if wait_attempts == 1 || wait_attempts % TASK_INSTANCE_WAIT_LOG_INTERVAL == 0 {
                    let detail = format!("attempts={wait_attempts}");
                    write_bootstrap_event(BOOTSTRAP_EVENT_GAME_TASK_WAITING_INSTANCE, &detail);
                }
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
                // HARD input block (DInput keyboard+mouse + XInput gamepad), driven from the
                // game task so it is active EVEN when the hudhook render loop is not running
                // (it does not under the offline launcher at the title). Runs every frame the
                // task ticks -- before the player check -- so a focused window cannot inject any
                // real input during the zero-input own-stepper/autoload probe. Pure suppression,
                // never synthesis.
                if block_input_enabled() {
                    enforce_input_block_now();
                } else {
                    release_input_block_now();
                }
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
                // RECURRING world-stream observer (own-load-stream-observer-must-be-recurring-task-2026-06-22).
                // Internally no-ops until own_load_continue_fire sets OWN_LOAD_CONTINUE_FIRED, so it
                // costs nothing during normal play and never spams. After continue_confirm/SetState5
                // fires, own_stepper_idx10 (a TITLE-PHASE task) STOPS ticking, so this per-frame game
                // task is the ONLY place that keeps logging the world-stream pump THROUGH the loading
                // screen. Runs BEFORE the player check so it ticks while there is no player yet (the
                // loading-screen frames are exactly when player_present is false). Pure reads only.
                // GOLDEN baseline mode (golden_observe_enabled) ALSO drives the observer even though our
                // continue never fired, so a NORMAL user-driven vanilla load is captured for diffing
                // against the menu-free OWN-LOAD stall. The observer self-gates and re-resolves the
                // owner->InGameStep->MoveMapStep chain live from OWN_LOAD_OWNER_CACHED (filled by
                // own_stepper_idx10 each title frame in golden mode). OBSERVE-ONLY: no load is fired.
                // OBSERVE-ONLY WorldBlockRes::Update diagnostic detour (worldblockres-phase-machine-
                // drives-loadstate-to-0xa-2026-06-22): installed ONCE (idempotent) whenever a diagnostic
                // OWN-LOAD / golden-observe context is armed, so normal play is untouched. The detour is a
                // pure-read pass-through (bumps a call counter + tracks max phase/gate atomics, then calls
                // the original and returns its value), so installing early is harmless and never alters
                // load behavior. It answers: is WorldBlockRes::Update ticked at all on our path, and do
                // any blocks' phase ([+0x35]) / FD4 gate ([+0x2f]) advance.
                if own_load_enabled()
                    || own_load_continue_enabled()
                    || own_load_pump_enabled()
                    || golden_observe_enabled()
                {
                    install_wbr_update_hook();
                }
                if (own_load_enabled() && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst))
                    || golden_observe_enabled()
                {
                    if let Ok(base) = game_module_base() {
                        let gm = game_man_ptr_or_null();
                        let player_present = unsafe { PlayerIns::local_player_mut() }.is_ok();
                        unsafe { own_load_stream_observe_recurring(base, gm, player_present) };
                    }
                }
                // PATH B PRIVATE PUMP (own_load_pump): if own_load_pump_fire built+armed the LoadGame job,
                // tick its Run privately EVERY frame here (the game thread) -- replicating native
                // ExecuteMenuJob's call shape (zero-init MenuJobResult + FD4Time carrying the frame delta)
                // -- to drive self-build -> deser -> m28 stream, then SetState5 on Success. Self-gates on
                // OWN_LOAD_PUMP_JOB != 0 / OWN_LOAD_PUMP_DONE, so it costs nothing until armed+built and
                // never re-pumps once terminal. Must run THROUGH the loading screen (player absent), so it
                // is here in the recurring game task, before the player check. Pure native call + reads.
                if own_load_pump_enabled() {
                    if let Ok(base) = game_module_base() {
                        let gm = game_man_ptr_or_null();
                        let frame_delta = task_data.delta_time.time;
                        unsafe { own_load_pump_tick(base, gm, frame_delta) };
                    }
                }
                // DIRECT "Continue pressed" trigger: at the settled main menu (post press-any-button,
                // GameMan set up), write the exact bit the native selector consumes
                // (*(TitleFlowContext+0x14c)=1), invoke the selector to BUILD the LoadGame job, and
                // PushBackJob it to the dialog queue. Self-gates + fires once; no input. Then DRAIN the
                // queue each frame (FUN_1407a90f0) so the posted job runs to completion (deser+world).
                if fire_tfc_continue_enabled() {
                    if let Ok(base) = game_module_base() {
                        // Autonomous press-any-button: self-fire the open-menu registrar when the
                        // title settles (zero-input), so no real button press is needed.
                        unsafe { maybe_auto_open_menu(base) };
                        // The Continue BUILD now runs IN-CONTEXT from the hooked TitleTopDialog::update
                        // detour (the pump's live-dialog frame), NOT from this game task -- that timing
                        // was the mis-context cause. Install the hook once; the detour fires the build.
                        unsafe { install_title_update_hook(base) };
                        let frame_delta = task_data.delta_time.time;
                        unsafe { tfc_continue_drain_tick(base, frame_delta) };
                    }
                }
                // GOLDEN-PATH zero-input boot -> open menu (DECOUPLED from fire_tfc_continue): the
                // readiness-gated press-any-button advance (hook 0x1407ad1c0 -> set [job+0x1e8]=2)
                // gets PAST press-any-button with no input, then maybe_auto_open_menu opens the main
                // menu -- with NO selector fire, so an observe run can reach the menu cleanly. bd
                // press-any-button-golden-lever-job1e8-readiness-2026-06-23.
                if pab_advance_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_pab_advance_hook(base) };
                        unsafe { maybe_auto_open_menu(base) };
                    }
                }
                // OFFLINE connection-state lever (milestone-3 fix): force GameMan+0xBC8/0xBC9 = 0 each
                // title frame so the connection-loss event handlers -- which build the GR_System_Message
                // "Cannot connect to network / connection lost" MessageBoxDialogs our offline boot
                // raises at menu-open -- short-circuit at their `IsInOnlineMode() &&
                // IsServerConnectionEnabled()` guard before enqueuing any popup. Gated by the offline
                // flag (this only forces state the offline boot already intends). bd er-effects-rs-0ye.
                if online_disable_enabled() {
                    // MILESTONE-3 FIX: short-circuit the offline title-flow check jobs to their
                    // no-modal exits so the title flow never enqueues a GR_System_Message MessageBox.
                    // ShowProgressJob::Run is the shared chokepoint for the save/network/sign-in/login
                    // check steps (the 3 observed modals); NetworkCheckJob::Run is the separate J6 job.
                    // Installed once, before menu-open. Offline-gated (no effect on an online check).
                    install_network_check_shortcircuit_hook();
                    install_show_progress_shortcircuit_hook();
                    if let Ok(base) = game_module_base() {
                        unsafe { force_offline_connection_bytes(base) };
                    }
                }
                // DIAGNOSTIC (gated by er-effects-grsysmsg-log.txt): log the GR_System_Message ids the
                // title flow fetches after menu-open, to DEFINITIVELY name the menu-open MessageBoxDialogs
                // (connection 4101/4102/4190 vs save 70000/4191) instead of guessing. Self-gates once.
                if grsysmsg_log_enabled() {
                    install_gr_sysmsg_log_hook();
                }
                // Anti-anti-debug (ported from ProDebug, correct base): neutralize FromSoft's
                // timed anti-debug so debug exceptions / our INT3 breakpoints reach our VEH.
                // Runs ONCE, BEFORE arming breakpoints, from the game task (game up, .text
                // decrypted) -- our own controlled timing, not the LazyLoader's.
                if anti_antidebug_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { apply_anti_antidebug_once(base) };
                    }
                }
                // Software (INT3) breakpoints from er-effects-breakpoints.txt: install once.
                // The VEH (crash logger) logs every hit's register/stack context + re-arms.
                if sw_breakpoints_enabled() {
                    if let Ok(base) = game_module_base() {
                        unsafe { install_sw_breakpoints_once(base) };
                    }
                }
                // STAY-ACTIVE: force ER's input-accept flag so a virtual gamepad keeps driving the
                // menus while ER is UNFOCUSED (user can work elsewhere during a golden capture). ER
                // clears [DLUID+0x88d] each frame when it isn't GetActiveWindow; re-set it to 1.
                if stay_active_enabled() {
                    if let Ok(base) = game_module_base() {
                        // DLUID (input-device-manager) singleton VA 0x14485dc18.
                        const DLUID_SINGLETON_RVA: usize =
                            RuntimeGlobalRva::DluidInputManager as usize;
                        #[repr(C)]
                        struct DluidInputManagerLayout {
                            unknown_000: [u8; 0x88d],
                            input_active: u8,
                        }
                        const DLUID_INPUT_ACTIVE_FLAG_OFFSET: usize =
                            core::mem::offset_of!(DluidInputManagerLayout, input_active);
                        const INPUT_ACTIVE: u8 = true as u8;
                        const NULL_DLUID: usize = NULL_MODULE_BASE;
                        let dluid = unsafe { safe_read_usize(base + DLUID_SINGLETON_RVA) }
                            .unwrap_or(NULL_DLUID);
                        // Defensive: only write once the flag byte is confirmed READABLE (so a
                        // not-yet-initialized or bad singleton ptr can never fault the game thread).
                        if dluid != NULL_DLUID
                            && unsafe { safe_read_usize(dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) }
                                .is_some()
                        {
                            unsafe {
                                *((dluid + DLUID_INPUT_ACTIVE_FLAG_OFFSET) as *mut u8) =
                                    INPUT_ACTIVE
                            };
                        }
                    }
                }
                let Ok(player) = (unsafe { PlayerIns::local_player_mut() }) else {
                    let mut state = state_or_return(&state);
                    state.game_task_ticks += GAME_TASK_TICK_INCREMENT;
                    // Install the MessageBoxDialog builder hook for native telemetry. Product
                    // autoload must NOT auto-accept: every pre/post-load message box is a hard
                    // investigation trigger whose semantic side effect must be skipped directly.
                    // The legacy OK-handler dismiss path remains only for non-product probes.
                    if online_disable_enabled() {
                        install_auto_accept_hook();
                        if !product_autoload_enabled() {
                            force_dismiss_startup_dialog();
                        }
                    }
                    // Observe the natural flow PAST the modal: tap Confirm (game's own input).
                    if auto_confirm_enabled() {
                        auto_confirm_tap();
                    }
                    // Bisect kill-switch: lock + tick only, NO filesystem I/O
                    // (no telemetry write, no experiments). Discriminates "our
                    // per-frame file I/O stalls the title" (lite survives) from
                    // "any per-frame work trips a budget" (lite still exits).
                    if lite_mode() {
                        return;
                    }
                    // Product autoload: run the native title open-menu predicate + minimal
                    // native save-load core from the recurring game task, before the idx10
                    // MenuJobWait hook path is needed. This bypasses title-accept/input
                    // injection while still advancing the data-driven PressStart/PRESS BUTTON
                    // component through its native open-menu registrar; readiness is checked
                    // inside product_core_autoload_tick.
                    if product_autoload_enabled() {
                        if let (Ok(base), Some(slot)) = (game_module_base(), state.autoload.slot())
                        {
                            unsafe {
                                product_core_autoload_tick(base, slot, state.game_task_ticks)
                            };
                        }
                        write_telemetry_throttled(&mut state, false);
                        return;
                    }
                    // OWN-THE-STEPPER: patch the idx10 step-fn slot to our handler so
                    // the FD4 scheduler runs OUR code in-context (step 1: verify the
                    // control point with a logging pass-through).
                    // OWN-STEPPER and the SEPARATE observe-only NATIVE-LOAD gate both install the
                    // idx10 patch so OUR handler runs each frame. own_stepper_idx10 dispatches to
                    // the native-load (observe-only, no forcing) path when native_load_enabled().
                    if own_stepper_enabled()
                        || native_load_enabled()
                        || native_continue_enabled()
                        || native_fullread_enabled()
                        || own_load_enabled()
                    {
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
                // In-world: latch OFF the startup popup auto-accept (in-game dialogs need real
                // choices), optionally clean stale title-dialog render resources, then run the
                // one-shot correctness dump.
                IN_WORLD_REACHED.store(IN_WORLD_REACHED_YES, Ordering::SeqCst);
                if own_stepper_enabled()
                    || native_load_enabled()
                    || native_continue_enabled()
                    || native_fullread_enabled()
                {
                    if let Ok(base) = game_module_base() {
                        unsafe {
                            cleanup_title_dialog_after_world_once(base, state.game_task_ticks)
                        };
                    }
                }
                // In-world correctness oracle: on the FIRST frame the local player exists, log
                // the load-correctness record + the T_controllable timeline marker ONCE. Fires
                // for both a native-menu load (observe) and a DLL-driven load (own-stepper), so
                // the two records are directly comparable (field-for-field == correct load).
                if (own_stepper_enabled()
                    || observe_enabled()
                    || native_load_enabled()
                    || native_continue_enabled()
                    || native_fullread_enabled())
                    && LOAD_CORRECTNESS_DUMPED
                        .swap(GAME_TASK_TICK_INCREMENT as usize, Ordering::SeqCst)
                        == LOAD_CORRECTNESS_NOT_DUMPED
                {
                    if let Ok(base) = game_module_base() {
                        timeline_event(
                            "T_controllable",
                            state.game_task_ticks,
                            format_args!("player=1"),
                        );
                        unsafe { dump_load_correctness(base, state.game_task_ticks) };
                    }
                }
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

    if selectbot_probe_enabled() || title_proceed_gate_enabled() || title_accept_byte_gate_enabled()
    {
        // selectbot_probe_once samples the SelectBot/pump state each title-idle
        // frame; when ER_EFFECTS_TITLE_PROCEED_GATE is set it ALSO fires the
        // one-shot title-accept latch write (lever 1) at state 10, and when
        // ER_EFFECTS_TITLE_ACCEPT_BYTE is set it fires lever 2 (global accept
        // byte 0x144589bdc) for the zero-input natural menu-open. Returns
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
        title_handoff_complete: TITLE_HANDOFF_COMPLETE.load(Ordering::SeqCst)
            != TITLE_HANDOFF_INCOMPLETE,
        // BYPASS arming signal: engine filled enough to build the LoadGame job at the title (GameDataMan
        // -> mss -> plausible TitleFlowContext), without waiting for the press-any-button handoff.
        loadgame_build_ctx_ready: unsafe {
            crate::experiments::loadgame_build_ctx_ready(game_module_base)
        },
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
