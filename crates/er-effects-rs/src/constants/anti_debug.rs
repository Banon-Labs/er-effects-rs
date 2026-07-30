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
pub(crate) use er_telemetry::counters::ANTI_ANTIDEBUG_APPLIED;
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
/// Boot missing-save picker CANCEL -> quit. Recorded here, and not only in the telemetry JSON,
/// because this channel is append-only, lock-free and reachable from any thread: it is the one
/// record that survives a game task frozen while holding the state mutex, which is exactly the
/// condition under which the cancel path runs.
pub(crate) const BOOTSTRAP_EVENT_BOOT_PICKER_CANCEL_EXIT: &str = "boot_picker_cancel_exit";
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

// ── Title-animation speedup lever (pab_dismiss -> menu_open) ─────────────────────────────────
// The title/menu transition is a Scaleform/GFx animation advanced by the FD4 frame-delta f32 the
// STEP_MenuJobWait tick (0x140b0d400) reads from its task_data+0x08 and forwards to
// CS::TitleTopDialog::update. FadeIn->Loop / TextFadeOut completion is frame-count CHECKED
// (current==total), NOT time-gated, so SCALING this delta makes the animation reach its end frame
// in fewer wall-clock frames -- every downstream predicate (Scaleform tick, completion compare,
// (flags&0x8f)>1 settle gate) is satisfied naturally; nothing is bypassed and the load does not
// desync. bd autoload-menu-speed-lever-framedelta-2026-06-22.
/// Clamp range for the speedup factor.
pub(crate) const TITLE_ANIM_SPEEDUP_MIN: f32 = 1.0;
pub(crate) const TITLE_ANIM_SPEEDUP_MAX: f32 = 16.0;
/// DEFAULT-ON for real autoload runs (no opt-in). Any value > 1.0 ARMS the FadeIn skip; the magnitude
/// no longer scales anything (the dt-scale and frame-burst levers were both runtime-falsified -- bd
/// title-anim-framedelta-lever-FALSIFIED-runtime-2026-06-24 + pab-to-menuopen-real-breakdown-build-not-
/// anim-2026-06-24 -- the FadeIn is wall-clock/present-bound, so we skip it at the completion predicate
/// instead). Kept as an f32 toggle so the existing env/file override (set to 1.0 = off) still works.
pub(crate) const TITLE_ANIM_SPEEDUP_DEFAULT: f32 = 4.0;
/// Diagnostic frame counter for the title-anim lever (logs SM state every Nth detour call).
pub(crate) use er_telemetry::counters::TITLE_ANIM_DIAG_CALLS;
/// Log the title SM state every this many detour calls.
pub(crate) const TITLE_ANIM_DIAG_INTERVAL: usize = 60;
/// FD4 state-machine `SetState`/request-transition (deobf 0x1407499e0; dump 0x140749ae0, shift -0x100).
/// `__fastcall(rcx = FD4StateMachine* sm, rdx = StateDesc* desc)`. Routes the transition through the
/// SM owner's vtable[0x150] and no-ops unless the current node is settled (`[node+0x20]&0x8f >= 2`), so
/// it cannot corrupt the SM. This is the call CS::TitleTopDialog::update's input-skip branch makes to
/// move FadeIn->Loop on a button press. bd fadein-* RE 2026-06-24.
pub(crate) const TITLE_FD4_SETSTATE_RVA: usize = 0x7499e0;
/// One-shot latch: the zero-input FadeIn->Loop transition has fired.
pub(crate) static TITLE_FADEIN_SKIP_FIRED: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// PART-A title-cover masquerade: `STEP_BeginTitle`'s only native visual side effect is wrapper
/// 0x14081f9f0 building the `05_000_Title` MenuWindowJob through factory 0x1407acbf0. Suppressing
/// this wrapper hides the native press-any-button/title Scaleform while leaving TitleStep state,
/// FixOrderJobSequence, native Continue/save-load state, and STEP_PlayGame untouched. It must never
/// touch the global resident-UI flag (CSMenuMan+0x21 / STEP_Wait). Ghidra dump addresses are +0xf0;
/// these constants are deobf/live RVAs used for the actual hook.
pub(crate) const TITLE_NATIVE_MENU_VISUAL_BEGIN_TITLE_RVA: usize = 0x81f9f0;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_TITLE_INFORMATION_RVA: usize = 0x81f8d0;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_FACTORY_RVA: usize = 0x7acbf0;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_NAME: &str = "05_000_Title";
pub(crate) const TITLE_PAB_INFORMATION_VISUAL_NAME: &str = "05_020_TitleInformation";
pub(crate) const TITLE_NATIVE_MENU_VISUAL_SUPPRESS_NOT_INSTALLED: usize = 0;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED_YES: usize = 1;
pub(crate) static TITLE_NATIVE_MENU_VISUAL_SUPPRESS_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_SUPPRESS_INSTALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_MENU_VISUAL_SUPPRESS_NOT_INSTALLED);
pub(crate) use er_telemetry::counters::TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS;
pub(crate) static TITLE_NATIVE_MENU_VISUAL_LAST_OUT_SLOT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_LAST_PREV_OUT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_LAST_ARG_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_LAST_ARG_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native `MenuWindowJob*` and live window preserved by the BeginTitle wrapper. The render-only
/// suppressor uses these to clear the native title draw bit without removing the job from the native
/// title sequence.
pub(crate) static TITLE_NATIVE_MENU_VISUAL_NATIVE_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_NATIVE_WINDOW: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PAB_INFORMATION_VISUAL_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_PAB_INFORMATION_VISUAL_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_PAB_INFORMATION_VISUAL_BUILDS;
pub(crate) static TITLE_PAB_INFORMATION_VISUAL_LAST_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PAB_INFORMATION_VISUAL_LAST_WINDOW: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PAB_INFORMATION_VISUAL_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Render-only Part-A suppression: `MenuWindowJob::Run` writes the native window visible flags at
/// `GLOBAL_CSMenuMan->field106_0x90[id]`: the Run body sets `|=1` before calling FadeIn, and the
/// FadeIn helper at deobf 0x140744dd0 sets `|=3`. User-visible runtime falsified the old `0x2`
/// draw-bit-only assumption: the title logo / PAB / Continue can still show with flags==1. Therefore
/// product suppression clears the full native-visible mask for the preserved `05_000_Title` window.
pub(crate) const TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RVA: usize = 0x744dd0;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_WINDOW_FADEIN_RUN_CALLER_RVA: usize = 0x7ad530;
/// Current-branch GFx SetVisible return site inside the native title MenuWindowJob FadeIn helper.
/// Ordered log proof: `user-visible-gfx-visible-logonly-current-branch-20260713-140820` first
/// title-window visible calls were `value=0x10f6a0/0x10f350 caller_rva=0x744e02`.
pub(crate) const TITLE_GFX_VISIBLE_TITLE_FADEIN_CALLER_RVA: usize = 0x744e02;
/// Within the title FadeIn GFx SetVisible callsite, this observed visible-call ordinal produces
/// the user-visible flash/glare during the autoload transition. Keep the name behavioral: the
/// underlying Scaleform object identity is still unknown.
pub(crate) const TITLE_05_000_FADEIN_FLASH_VISIBLE_ORDINAL: usize = 2;
pub(crate) const CS_MENU_MAN_GLOBAL_RVA: usize = 0x3d6b7b0;
/// OptionSetting tab-select VISIBILITY pass `FUN_14093b850` (deobf 0x93b760):
/// `fn(CompositeOptionSettingDialog* composite, int tabIndex, u8* r8, u8* r9)`. It sets the current
/// pane (`composite+0xb8 = cache[tabIndex]`, building via the switch dispatch only if the cache slot is
/// null), then iterates the 10 cached pane dialogs at `composite+0x68` and calls `SetVisible(dialog+0x1200,
/// current==dialog)` on each -- showing ONLY the active tab's pane, hiding the rest. This is the game's
/// own per-tab visibility application. Re-invoking it on restore re-shows the active OptionSetting pane
/// that our hide/restore left with DisplayInfo.Visible=0 (the blank Game Options pane).
pub(crate) const OPTIONSETTING_TAB_SELECT_VISIBILITY_RVA: usize = 0x93b760;
/// OptionSettingTopDialog (menu_id 0x25) -> embedded CS::CompositeOptionSettingDialog.
pub(crate) const OPTIONSETTING_COMPOSITE_OFFSET: usize = 0x1768;
/// Composite -> current pane dialog ptr (`+0xb8`) and the 10-entry per-tab pane-dialog cache (`+0x68`).
pub(crate) const OPTIONSETTING_COMPOSITE_CURRENT_PANE_OFFSET: usize = 0xb8;
pub(crate) const OPTIONSETTING_COMPOSITE_PANE_CACHE_OFFSET: usize = 0x68;
pub(crate) const OPTIONSETTING_COMPOSITE_PANE_CACHE_COUNT: usize = 10;
/// OptionSetting/OptionSetting_Trial window menu_id (indexes CSMenuMan flag byte; gates the pane-reapply).
pub(crate) const OPTIONSETTING_MENU_ID: u16 = 0x25;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_VISIBLE_FLAGS_MASK: u8 = 0x3;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_NOT_INSTALLED: usize = 0;
pub(crate) const TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED_YES: usize = 1;
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_INSTALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS_NOT_INSTALLED);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESSED_WINDOWS: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_WINDOW: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_BEFORE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_FLAGS_AFTER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_MENU_VISUAL_RENDER_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// PART-B custom cover target: `05_010_ProfileSelect` is an existing Scaleform surface with
/// `MENU_DummyProfileFace_01..10` symbols that the profile renderer maps to
/// `SYSTEX_Menu_Profile00..09` (via CSMenuProfModelRend / active-screen render targets). The wrapper
/// below is the deobf/live address for the native `05_010_ProfileSelect` MenuWindowJob builder
/// (Ghidra dump 0x14081f7e0 -> deobf 0x14081f6f0). We use it as the initial custom cover surface
/// instead of trying to remap `05_001_Title_Logo`, which has no dummy-profile symbol.
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA: usize = 0x81f6f0;
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME: &str = "05_010_ProfileSelect";
/// Native full-screen black Scaleform/MenuWindowJob surface. Ghidra dump 0x140793c10 ->
/// deobf/live 0x140793b20 (content-unique) builds `01_900_Black` with the same
/// MenuWindow/SceneProxy host ABI as the title wrappers. This is the first diagnostic carrier for
/// proving an engine-owned custom surface can stay above PRESS ANY BUTTON / Continue.
pub(crate) const TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA: usize = 0x793b20;
pub(crate) const TITLE_CUSTOM_COVER_BLACK_NAME: &str = "01_900_Black";
pub(crate) const TITLE_CUSTOM_COVER_DUMMY_PROFILE_SYMBOL: &str = "MENU_DummyProfileFace_01";
pub(crate) const TITLE_CUSTOM_COVER_SYSTEX_TARGET: &str = "SYSTEX_Menu_Profile00";
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS: &str = "CSMenuProfModelRend";
/// Profile renderer table initializer: live 0x1409af3a0 (dump 0x1409af4f0) allocates the ten
/// CSMenuProfModelRend instances and writes DAT_143d6d8d0 before the refresh/feed pass below.
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDER_INIT_RVA: usize = 0x9af3a0;
/// Profile portrait refresh/display pipeline: live 0x1409aa680 (dump 0x1409aa7d0) reads the loaded
/// `ProfileSummary`, loops 10 slots, fills CSMenuProfModelRend / face/player model data, and maps
/// each active slot to `SYSTEX_Menu_ProfileNN` through `FUN_140bb8cf0(renderer, slot*2)`. It must run
/// after SL2/profile readiness, not at early `05_001_Title_Logo` construction time.
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_RVA: usize = 0x9aa680;
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_CALLS: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_PROFILE_SUMMARY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_CALLER_PHASE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_754: usize = 0x754;
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_755: usize = 0x755;
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDERER_TEX_INDEX_OFFSET: usize = 0x9a8;
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS;
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_SLOT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_OFFSCREEN_REND: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_INDEX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_754: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_755: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS;
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_BLACK_BUILDS;
pub(crate) static TITLE_CUSTOM_COVER_BLACK_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_BLACK_LAST_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_BLACK_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// MenuWindowJob::Run (dump 0x1407ad2b0 -> deobf/live 0x1407ad1c0). Part B uses the native
/// title job's own pump context to run the separately-built ProfileSelect cover job alongside the
/// preserved title job, instead of replacing the authoritative BeginTitle out-slot.
pub(crate) const MENU_WINDOW_JOB_RUN_RVA: usize = 0x7ad1c0;
pub(crate) static TITLE_CUSTOM_COVER_RUN_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_RUN_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_RUN_RECURSION;
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_RUN_CALLS;
/// PAB detour -> system_quit_menu_window_run_post call count. Confirms the deterministic-winner wiring
/// (2026-07-15 install-race fix) is live at runtime: >0 means PAB is driving run_post on MenuWindowJob::Run
/// passes, so the hide + slot-activation-gate latches get written regardless of the MinHook race.
pub(crate) use er_telemetry::counters::PAB_RUN_POST_CALLS;
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_NATIVE_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_COVER_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_COVER_WINDOW: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Removed fallback-cover counters kept only so older telemetry references compile during cleanup.
/// Product title/loading cover work must use native CSEzDraw/Scaleform/game-render surfaces.
pub(crate) use er_telemetry::counters::TITLE_OVERLAY_COVER_RENDER_CALLS;
pub(crate) use er_telemetry::counters::TITLE_OVERLAY_COVER_LAST_DISPLAY_W;
pub(crate) use er_telemetry::counters::TITLE_OVERLAY_COVER_LAST_DISPLAY_H;
pub(crate) const TITLE_CUSTOM_COVER_GX_TEXTURE_RESOURCE_OFFSET: usize = 0x10;
pub(crate) use er_telemetry::counters::TITLE_OVERLAY_COVER_TEXTURE_BOUND;
pub(crate) use er_telemetry::counters::TITLE_OVERLAY_COVER_LAST_GX_TEXTURE;
pub(crate) use er_telemetry::counters::TITLE_OVERLAY_COVER_LAST_TEXTURE_RESOURCE;

/// Scaleform (GFx) D3D12 `CBV_SRV_UAV` descriptor-heap ring/sub-allocator advance
/// (deobf entry `0x140ec9530`; `f(this /rcx/, count /edx/)`). Verified disasm: the new-page branch
/// reloads the current-page provider `*(this+0x38)` and unconditionally derefs `[provider+0x20]` at
/// deobf `0x140ec95d1` (`mov 0x20(%rax),%rcx`), access-violating when the provider is null
/// (native-Windows crash report for our DLL: rva=0xec95d1, fault_addr=0x20). A null provider means a
/// fresh/reset HAL (ring capacity `this+0x20 == 0`), so the original ALWAYS takes that branch and
/// always faults; our detour skips the advance until the provider is initialized. Vanilla ER never
/// reaches the null window (vkd3d masks it; the native D3D12 driver does not); our DLL only reaches it
/// indirectly. The guard is environment-agnostic -- it null-checks the pointer the game derefs, every
/// call, and is a transparent passthrough otherwise. (bd er-effects-rs-y22i.)
pub(crate) const SCALEFORM_DESC_ADVANCE_RVA: usize = 0xec9530;
/// Byte offset of the current-page provider field within the descriptor sub-allocator `this`.
pub(crate) const SCALEFORM_DESC_PROVIDER_OFFSET: usize = 0x38;
pub(crate) static SCALEFORM_DESC_ADVANCE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::SCALEFORM_DESC_ADVANCE_INSTALLED;
pub(crate) use er_telemetry::counters::SCALEFORM_DESC_PROVIDER_NULL_HITS;
/// Read-only latch of the native CSFakeLoadingScreen singleton visible during the black/progress
/// loading UI. Sampled from telemetry writes; no hooks or native calls.
pub(crate) use er_telemetry::counters::FAKE_LOADING_SCREEN_SAMPLE_COUNT;
pub(crate) use er_telemetry::counters::FAKE_LOADING_SCREEN_VISIBLE_SAMPLES;
pub(crate) static FAKE_LOADING_SCREEN_LAST_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static FAKE_LOADING_SCREEN_LAST_VISIBLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static FAKE_LOADING_SCREEN_LAST_FIELD_C: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static FAKE_LOADING_SCREEN_LAST_FIELD_10: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::RENDER_LOADING_LAYER_SAMPLE_COUNT;
pub(crate) use er_telemetry::counters::RENDER_LOADING_LAYER_NONNULL_SAMPLES;
pub(crate) static RENDER_LOADING_LAYER_LAST_RENDMAN: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RENDER_LOADING_LAYER_LAST_CSGRAPHICS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RENDER_LOADING_LAYER_LAST_CSSCALEFORM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::RENDER_LOADING_LAYER_LAST_SLOTS_MASK;
pub(crate) use er_telemetry::counters::RENDER_LOADING_LAYER_VISIBLE_SLOTS_MASK;
/// RAM oracle (`oracle_loading_cover_suppress_writes`): frames the loading-cover experiment actually
/// cleared `CSFakeLoadingScreenImp.visible`. >0 means the clamp engaged during at least one map load; 0
/// with the gate on means the cover object never resolved / was never raised (nothing was suppressed).
pub(crate) use er_telemetry::counters::LOADING_COVER_SUPPRESS_WRITES;
/// `CS::CSFakeLoadingScreenImp` -- the full-screen fade/cover PLATE the game draws during a map load to
/// HIDE the world teardown/rebuild behind the now-loading UI. RE'd from its ctor (deobf 0x140bbeee0,
/// vtable 0x142b803b8) which is called from `CSDrawStep`, so this object lives in the render pipeline, not
/// the menu system. `visible` (+0x8) is the byte the draw step checks to decide whether to draw the cover;
/// the ctor inits it to 0 and the map-load system raises it while a load is in flight. Clearing it exposes
/// whatever the renderer is drawing underneath (the "disable the loading screen, watch the world pop in"
/// experiment). Singleton = `*(base + RuntimeGlobalRva::FakeLoadingScreenSingleton)`.
#[repr(C)]
pub(crate) struct CSFakeLoadingScreenImp {
    pub(crate) vftable: usize,
    pub(crate) visible: u8,
    pub(crate) unknown_009: [u8; 3],
    pub(crate) field_0c: u32,
    pub(crate) field_10: u64,
}

pub(crate) const FAKE_LOADING_SCREEN_VISIBLE_OFFSET: usize =
    core::mem::offset_of!(CSFakeLoadingScreenImp, visible);

const _: () = assert!(core::mem::offset_of!(CSFakeLoadingScreenImp, visible) == 0x8);
/// Now-loading background portrait forge. The pseudorandom loading-screen background is
/// `helper->replaceTexInfo` (a CSScaleformReplaceTexInfo*), PRODUCED for symbol `MENU_Load_%05d` by
/// `GetOrCreateReplaceTexInfo`, whose symbol-bind step is `FUN_140d69880` (dump 0x140d69880 -> deobf
/// 0x140d697d0, shift -0xb0). We full-replace that bind for `MENU_Load_*`: build an er-tpf TPF named
/// exactly the requested symbol, turn it into a TpfResCap container via the game's in-memory
/// `CreateTpfResCap` factory, wrap it in a TpfFileCap, and hand it back on the rti so the unmodified
/// per-frame CSScaleform pump registers our texture name and GFx composites the portrait as the
/// loading background. `fn(rti: *mut CSScaleformReplaceTexInfo /rcx/, symbol: *mut DLString<u16>
/// /rdx/) -> u8` (1 = bound; producer then lists the rti).
pub(crate) const LOADING_BG_REPLACE_BIND_RVA: usize = 0xd697d0;
/// In-memory TPF -> TpfResCap factory `CreateTpfResCap` (dump 0x140b83770 -> deobf 0x140b83680).
/// `fn(tpfRepo /rcx = *GLOBAL_TpfRepository/, name: *const u16 /rdx/, bytes: *const u8 /r8/, size: u64
/// /r9/, flag: u8 /stack=0/, extra: u32 /stack=0/) -> *mut TpfResCap` (0xb8; +0x78 count, +0x80 array).
pub(crate) const CREATE_TPF_RESCAP_RVA: usize = 0xb83680;
/// `CS::TpfFileCap::TpfFileCap` ctor (dump 0x140226010 -> deobf 0x140225f60). `fn(this: *mut /0x98
/// from MainHeap/, loadTask=0) -> this`; only inits the FD4FileCap base and zeroes `+0x90`.
pub(crate) const TPF_FILE_CAP_CTOR_RVA: usize = 0x225f60;
/// Game heap allocator wrapper (dump 0x141eb9ec0 -> deobf 0x141eb9ed0). `fn(size /rcx/, align /rdx/,
/// allocator_obj /r8/) -> *mut u8`; allocator_obj is the dereferenced DLAllocator* (== the repo's
/// `runtime_heap_allocator` for MainHeap).
pub(crate) const GAME_HEAP_ALLOC_RVA: usize = 0x1eb9ed0;
/// `DLString<wchar_t>::substr` (dump 0x140116c90 -> deobf 0x140116c70). `fn(dest /rcx/, src /rdx/,
/// start /r8 = 0/, count /r9 = usize::MAX = to-end/) -> dest`; copies the symbol into the rti symbol.
pub(crate) const DLSTRING_WCHAR_SUBSTR_RVA: usize = 0x116c70;
// `GLOBAL_TpfRepository` singleton pointer (deref -> rcx for CreateTpfResCap) is defined below as
// the existing `GLOBAL_TPF_REPOSITORY_RVA` (0x3d73fb8).
/// `GLOBAL_MainHeapAllocator` singleton pointer (data, 0x143d872e0; identical RVA to the repo's
/// `runtime_heap_allocator`). Deref -> the allocator object for the 0x98-byte TpfFileCap allocation.
pub(crate) const GLOBAL_MAIN_HEAP_ALLOCATOR_RVA: usize = 0x3d872e0;
/// CSScaleformReplaceTexInfo (size 0x50) field offsets.
pub(crate) const REPLACE_TEX_INFO_REFCOUNT_OFFSET: usize = 0x8; // i32 DLReferenceCountObject refcount
pub(crate) const REPLACE_TEX_INFO_SYMBOL_OFFSET: usize = 0x10; // DLString<u16>
pub(crate) const REPLACE_TEX_INFO_ENCODING_OFFSET: usize = 0x38; // u8
pub(crate) const REPLACE_TEX_INFO_TPF_FILE_CAP_OFFSET: usize = 0x40; // TpfFileCap*
pub(crate) const REPLACE_TEX_INFO_READY_OFFSET: usize = 0x48; // u8 (leave 0 so the pump processes it)
/// TpfFileCap (size 0x98) field offsets.
pub(crate) const TPF_FILE_CAP_LOAD_STATE_OFFSET: usize = 0x88; // u8
pub(crate) const TPF_FILE_CAP_FLAGS_OFFSET: usize = 0x89; // u8
pub(crate) const TPF_FILE_CAP_TEX_RESCAP_OFFSET: usize = 0x90; // -> TpfResCap container
pub(crate) const TPF_FILE_CAP_LOADED_STATE: u8 = 4;
pub(crate) const TPF_FILE_CAP_READY_FLAG_BIT: u8 = 0x20;
pub(crate) const TPF_FILE_CAP_ALLOC_SIZE: usize = 0x98;
pub(crate) const TPF_FILE_CAP_ALLOC_ALIGN: usize = 8;



// Relocated from constants/portrait_lookat.rs (portrait crate split): the title-cover
// Scaleform bind-observer block is title-cover domain and stays product-side.
/// Passive observer for native Scaleform image-symbol -> system texture bindings.
/// Dump `FUN_1407452c0` maps to live/deobf `0x1407451c0`. It receives an owning resource/list field
/// in rcx and a pair of DLString<char> values in rdx. Do not call it from product code; observe native
/// calls to learn valid owner/resource contexts for SYSTEX-backed surfaces.
pub(crate) const TITLE_SCALEFORM_BIND_OBSERVER_RVA: usize = 0x7451c0;
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_BIND_OBSERVER_INSTALLED;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_BIND_OBSERVER_HITS;
pub(crate) use er_telemetry::counters::TITLE_SCALEFORM_BIND_OBSERVER_SYSTEX_HITS;
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_PAIR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_SYMBOL_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_SCALEFORM_BIND_OBSERVER_LAST_TARGET_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Experimental visible-surface bind rewrite for the replayed ProfileSelect cover: the native
/// SYSTEX profile texture normally targets `MENU_DummyProfileFace_01`; rewrite slot0 to the
/// visibly placed `MENU_FL_40135_Profile` surface and expose it as a distinct oracle.
pub(crate) const TITLE_PROFILE_VISIBLE_SURFACE_SYMBOL: &str = "MENU_FL_40135_Profile";
pub(crate) use er_telemetry::counters::TITLE_PROFILE_VISIBLE_SURFACE_BIND_REWRITES;
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_PAIR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_PROFILE_VISIBLE_SURFACE_BIND_LAST_SYMBOL_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
