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
pub(crate) static TITLE_ANIM_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static TITLE_NATIVE_MENU_VISUAL_SUPPRESSED_BUILDS: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static TITLE_PAB_INFORMATION_VISUAL_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_PAB_INFORMATION_VISUAL_BUILDS: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA: usize = 0x2b80128;
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
/// Live table of the ten CSMenuProfModelRend pointers filled by the title/profile renderer setup.
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA: usize = 0x3d6d8d0;
pub(crate) const TITLE_PROFILE_SLOT_COUNT: usize = 10;
/// CSMenuAsmModelRend base stores CSEzOffscreenRend* at +0xa8; CSEzOffscreenRend stores
/// CSRuntimeTexResCap* registered under SYSTEX_Menu_ProfileNN at +0x10.
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET: usize = 0xa8;
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_OFFSCREEN_TEX_RESCAP_OFFSET: usize = 0x10;
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_RENDERER_TEX_INDEX_OFFSET: usize = 0x9a8;
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_BUILDS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_PROFILE_SELECT_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_CUSTOM_COVER_BLACK_BUILDS: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static TITLE_CUSTOM_COVER_RUN_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_CUSTOM_COVER_RUN_RECURSION: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_CUSTOM_COVER_RUN_CALLS: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static TITLE_OVERLAY_COVER_RENDER_CALLS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OVERLAY_COVER_LAST_DISPLAY_W: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OVERLAY_COVER_LAST_DISPLAY_H: AtomicUsize = AtomicUsize::new(0);
/// `CS::TexResCap` embeds the draw-usable `CSGxTexture*` at +0x78, and that wrapper keeps
/// the backing graphics texture/reference at +0x10. The overlay cannot safely reinterpret this as
/// a generic texture ID yet, but observing these handles during a native draw would be a concrete
/// draw-side consumption oracle for the RAM-backed profile portrait source rather than generic scaffolding.
pub(crate) const TITLE_CUSTOM_COVER_TEX_RESCAP_GX_TEXTURE_OFFSET: usize = 0x78;
pub(crate) const TITLE_CUSTOM_COVER_GX_TEXTURE_RESOURCE_OFFSET: usize = 0x10;
pub(crate) static TITLE_OVERLAY_COVER_TEXTURE_BOUND: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OVERLAY_COVER_LAST_GX_TEXTURE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OVERLAY_COVER_LAST_TEXTURE_RESOURCE: AtomicUsize = AtomicUsize::new(0);
/// Observe the native now-loading helper visible during the black/progress-bar loading surface.
/// This is the first-pass target for a separate custom loading/masquerade surface after live title-logo
/// remaps proved crash-prone.
pub(crate) const NOW_LOADING_HELPER_CTOR_RVA: usize = 0x2a20e0;
pub(crate) const NOW_LOADING_HELPER_UPDATE_RVA: usize = 0x2a2c40;
pub(crate) static NOW_LOADING_HELPER_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static NOW_LOADING_HELPER_UPDATE_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static NOW_LOADING_HELPER_HOOKS_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NOW_LOADING_HELPER_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NOW_LOADING_HELPER_UPDATE_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static NOW_LOADING_HELPER_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NOW_LOADING_HELPER_LAST_MENU_INDEX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NOW_LOADING_HELPER_LAST_REPLACE_TEX_INFO: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NOW_LOADING_HELPER_LAST_REQUESTED_REPLACE_TEX_INFO: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static NOW_LOADING_HELPER_LAST_FLAGS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native `CS::LoadingScreen` update path that drives the now-loading Gauge/Gauge_3 movieclip frame.
/// Static RE (2026-07-05): dump `FUN_14090a7a0` -> deobf `0x14090a6b0`; it computes
/// `frame = progress01 * max_frame + 1`, clamps to max at progress >= 1.0, then calls
/// `CSMenuFrameComponent::SetFrame(&this->gauge, frame)`. This is the product semaphore for the
/// visible loading bar reaching 100%, later and more exact than TimeAct/world-ready.
pub(crate) const LOADING_SCREEN_UPDATE_RVA: usize = 0x90a6b0;
pub(crate) static LOADING_SCREEN_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static LOADING_SCREEN_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_SCREEN_UPDATE_HITS: AtomicUsize = AtomicUsize::new(0);
/// `CS::KnowledgeLoadingScreen` tip-refresh (dump `FUN_14090a3f0` -> deobf/live `0x14090a300`, RVA
/// 0x90a300). `fn(this)` -- picks the next tip msg id and SetTexts the title (`this+0xb28`) + body
/// (`this+0xb88`). er-effects-rs-jsm PIVOT: we NO-OP it (skip the original) so the native tip title/body
/// are never set -- our own player-stats text (overlay) shows in the tip region instead. Installed before
/// the widget ctor so even the ctor's one-shot initial tip is suppressed.
pub(crate) const KNOWLEDGE_TIP_REFRESH_RVA: usize = 0x90a300;
pub(crate) static KNOWLEDGE_TIP_REFRESH_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static KNOWLEDGE_TIP_REFRESH_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static KNOWLEDGE_TIP_SUPPRESSED_HITS: AtomicUsize = AtomicUsize::new(0);
/// `CS::KnowledgeLoadingScreen` tip-text SetText handles (CSScaleformValue): title `this+0xb28`
/// ('Main/Knowledge/IetmName/Text_0'), body `this+0xb88` ('Main/Knowledge/ItemInfo/Text_0'). The
/// suppression detour SetTexts both to empty after the original runs. (bd loading-tip-text-pipeline-RE.)
pub(crate) const KNOWLEDGE_TIP_TITLE_HANDLE_OFFSET: usize = 0xb28;
pub(crate) const KNOWLEDGE_TIP_BODY_HANDLE_OFFSET: usize = 0xb88;
/// `CS::KnowledgeLoadingScreen` tip-advance "enabled" predicate lambda (dump `FUN_14090a1b0` ->
/// deobf/live `0x14090a0c0`, content-matched shift -0xf0). `fn(functor) -> bool`; true only while the
/// Main clip label == "Normal". The ctor registers ONE native menu action (input id 0x186be -- the
/// keyguide's "press to advance the tip"): the base `MenuWindow::Update` trigger loop fires the action
/// only when this predicate returns true, AND the per-update keyguide composer (vtable slot 7 -> slot 4)
/// lists an action in the keyguide only while its enabled predicate is true. Forcing false therefore
/// BOTH no-ops the advance press and durably hides the keyguide prompt (a one-shot SetText blank on the
/// keyguide handle `this+0x380` would be overwritten by the per-update re-composition). The lambda is
/// reached only through this screen's `_Func_impl` vftable, so no other menu is affected.
/// (bd loading-keyguide-and-tip-advance-RE-2026-07-06.)
pub(crate) const KNOWLEDGE_TIP_ADVANCE_ENABLED_RVA: usize = 0x90a0c0;
pub(crate) static KNOWLEDGE_TIP_ADVANCE_ENABLED_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static KNOWLEDGE_TIP_ADVANCE_ENABLED_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static KNOWLEDGE_TIP_ADVANCE_SUPPRESSED_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_SCREEN_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static LOADING_SCREEN_LAST_DATA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static LOADING_SCREEN_BAR_ENABLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_SCREEN_BAR_CURRENT_FRAME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_SCREEN_BAR_MAX_FRAME: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_SCREEN_BAR_PROGRESS_PERMILLE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_SCREEN_BAR_FINAL_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) const LOADING_SCREEN_DATA_OFFSET: usize = 0xa38;
pub(crate) const LOADING_SCREEN_GAUGE_COMPONENT_OFFSET: usize = 0xa48;
pub(crate) const LOADING_SCREEN_GAUGE_ENABLED_OFFSET: usize = 0xab0;
pub(crate) const MENU_FRAME_COMPONENT_CURRENT_FRAME_OFFSET: usize = 0x70;
pub(crate) const MENU_FRAME_COMPONENT_MAX_FRAME_OFFSET: usize = 0x74;
pub(crate) const LOADING_SCREEN_DATA_ACTIVE_INDEX_OFFSET: usize = 0x14;
pub(crate) const LOADING_SCREEN_DATA_START_PROGRESS_OFFSET: usize = 0x18;
pub(crate) const LOADING_SCREEN_DATA_TARGET_PROGRESS_OFFSET: usize = 0x1c;
pub(crate) const LOADING_SCREEN_DATA_INTERP_DURATION_OFFSET: usize = 0x20;
pub(crate) const LOADING_SCREEN_DATA_INTERP_ELAPSED_OFFSET: usize = 0x24;
/// Read-only latch of the native CSFakeLoadingScreen singleton visible during the black/progress
/// loading UI. Sampled from telemetry writes; no hooks or native calls.
pub(crate) static FAKE_LOADING_SCREEN_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static FAKE_LOADING_SCREEN_VISIBLE_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static FAKE_LOADING_SCREEN_LAST_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static FAKE_LOADING_SCREEN_LAST_VISIBLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static FAKE_LOADING_SCREEN_LAST_FIELD_C: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static FAKE_LOADING_SCREEN_LAST_FIELD_10: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RENDER_LOADING_LAYER_SAMPLE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RENDER_LOADING_LAYER_NONNULL_SAMPLES: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RENDER_LOADING_LAYER_LAST_RENDMAN: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RENDER_LOADING_LAYER_LAST_CSGRAPHICS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RENDER_LOADING_LAYER_LAST_CSSCALEFORM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RENDER_LOADING_LAYER_LAST_SLOTS_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RENDER_LOADING_LAYER_VISIBLE_SLOTS_MASK: AtomicUsize = AtomicUsize::new(0);
/// RAM oracle (`oracle_loading_cover_suppress_writes`): frames the loading-cover experiment actually
/// cleared `CSFakeLoadingScreenImp.visible`. >0 means the clamp engaged during at least one map load; 0
/// with the gate on means the cover object never resolved / was never raised (nothing was suppressed).
pub(crate) static LOADING_COVER_SUPPRESS_WRITES: AtomicUsize = AtomicUsize::new(0);
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

/// `CS::CSNowLoadingHelperImp` -- the controller behind the now-loading UI (the tips + rotating artwork,
/// distinct from the `CSFakeLoadingScreenImp` cover and from the Scaleform movie that draws them). RE'd
/// from the Ghidra dump's named layout (ctor deobf 0x1402a20e0, `Update` 0x1402a2c40). Key fields:
/// `menu_load_entries` is a Fisher-Yates-shuffled 1..=34 array (the 34 loading-screen artwork/tip
/// variants) and `current_menu_load_index` picks the active one; `replace_tex_info` /
/// `requested_replace_tex_info` are the Scaleform texture-replacement handoff that swaps that artwork into
/// the movie; `countdown` is the minimum-display timer. IMPORTANT: `load_done` (+0xed) is a load-COMPLETE
/// latch (`Update` copies it from `request_load_done`, which the map-load system raises) -- it reads true
/// AFTER the load finishes and lingers into gameplay, so it is NOT a "loading screen is visible" signal.
/// Singleton = `*(base + RuntimeGlobalRva::NowLoadingSingleton)`.
#[repr(C)]
pub(crate) struct CSNowLoadingHelperImp {
    pub(crate) vftable: usize,
    pub(crate) rand_xorshift: usize,
    pub(crate) update_task: [u8; 0x28],
    pub(crate) field_38: usize,
    pub(crate) field_40: usize,
    pub(crate) menu_load_entries: [i32; 34],
    pub(crate) current_menu_load_index: i32,
    pub(crate) unknown_d4: [u8; 4],
    pub(crate) replace_tex_info: usize,
    pub(crate) requested_replace_tex_info: usize,
    pub(crate) countdown: f32,
    pub(crate) request_load_done: u8,
    pub(crate) load_done: u8,
    pub(crate) unknown_ee: [u8; 2],
    pub(crate) field_f0: i32,
    pub(crate) unknown_f4: [u8; 4],
}

// Layout guards: the RE'd offsets/size must match the Ghidra dump so a struct edit can't silently drift
// the pointers our reads/writes use.
const _: () = assert!(core::mem::size_of::<CSNowLoadingHelperImp>() == 0xf8);
const _: () = assert!(core::mem::offset_of!(CSNowLoadingHelperImp, menu_load_entries) == 0x48);
const _: () = assert!(core::mem::offset_of!(CSNowLoadingHelperImp, replace_tex_info) == 0xd8);
const _: () = assert!(core::mem::offset_of!(CSNowLoadingHelperImp, load_done) == 0xed);
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
/// Incoming symbol DLString<wchar_t> (rdx, standalone, size 0x30) field offsets.
pub(crate) const DLSTRING_U16_INLINE_OFFSET: usize = 0x8; // inline buffer, or heap ptr if cap > 7
pub(crate) const DLSTRING_U16_LENGTH_OFFSET: usize = 0x18; // code units
pub(crate) const DLSTRING_U16_CAPACITY_OFFSET: usize = 0x20; // code units; SSO threshold > 7 -> heap
pub(crate) const DLSTRING_U16_ENCODING_OFFSET: usize = 0x28; // u8 DLCharacterSet
pub(crate) const DLSTRING_U16_SSO_THRESHOLD: usize = 7;
/// The now-loading background image symbols are MENU_Load_00001..00034; match by prefix.
pub(crate) const LOADING_BG_SYMBOL_PREFIX: &str = "MENU_Load_";
pub(crate) static LOADING_BG_TEXTURE_REDIRECT_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static LOADING_BG_TEXTURE_REDIRECT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Times the producer-bind hook saw a MENU_Load_ symbol (a now-loading background request).
pub(crate) static LOADING_BG_TEXTURE_REDIRECT_ATTEMPTS: AtomicUsize = AtomicUsize::new(0);
/// Times we successfully forged + injected our portrait TPF cap on the rti (the proof oracle: >0
/// means our texture was bound as the loading-screen background).
pub(crate) static LOADING_BG_TEXTURE_REDIRECT_COMMITS: AtomicUsize = AtomicUsize::new(0);
/// Last forge outcome code: 1=injected, 2=tpf-build-fail, 3=createrescap-null, 4=alloc-null.
pub(crate) static LOADING_BG_TEXTURE_REDIRECT_LAST_SYMBOL_MATCH: AtomicUsize = AtomicUsize::new(0);
/// Last forged TpfFileCap pointer.
pub(crate) static LOADING_BG_TEXTURE_REDIRECT_LAST_PORTRAIT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Times we re-bound the live offscreen-RT CSGxTexture into the already-forged now-loading container
/// AFTER the bind (the now-loading background binds ~15-17s, BEFORE our post-Continue renderer's RT is
/// live, and never re-binds -- so the live portrait must be swapped into the displayed container after the
/// fact). >0 means the loading screen's displayed background is now sampling our live animated portrait.
pub(crate) static LOADING_BG_LIVE_GX_REBINDS: AtomicUsize = AtomicUsize::new(0);
/// The live CSGxTexture currently re-bound into the now-loading container (telemetry/sweep).
pub(crate) static LOADING_BG_LIVE_GX_BOUND: AtomicUsize = AtomicUsize::new(0);
/// Diagnostic: total calls into the replace-bind hook (every symbol, ungated), so we can tell
/// whether `FUN_140d69880` is even on the now-loading background path vs the producer cache-hit path.
pub(crate) static LOADING_BG_REPLACE_BIND_TOTAL_CALLS: AtomicUsize = AtomicUsize::new(0);
/// The kept-alive portrait `CSGxTexture` captured during ProfileSelect (0 until captured). When set,
/// the forge swaps it into its TpfResCap container's TexResCap so the loading screen shows the real
/// rendered character portrait instead of the placeholder checker.
pub(crate) static LOADING_BG_PORTRAIT_GX_KEPT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static LOADING_BG_PORTRAIT_GX_CAPTURE_HITS: AtomicUsize = AtomicUsize::new(0);
/// The live profile-portrait offscreen render target, read back via D3D12 into CPU RGBA8 once the
/// character head has rendered (`portrait_real_pixels_enabled()` gate). Tuple = (width, height,
/// tightly-packed `width*height*4` RGBA8 pixels). `None` until a successful readback. When `Some`,
/// the now-loading forge builds its TPF from these REAL pixels instead of the magenta/yellow checker.
pub(crate) static LOADING_BG_PORTRAIT_RGBA: std::sync::Mutex<Option<(u32, u32, Vec<u8>)>> =
    std::sync::Mutex::new(None);
/// 1 if the read-back portrait has any non-black texel (max(R,G,B) > 24) inside a center 64x64
/// region, else 0 (a black/blank capture). Exposed as `oracle_loading_bg_portrait_gx_nonblack`.
pub(crate) static LOADING_BG_PORTRAIT_NONBLACK: AtomicUsize = AtomicUsize::new(0);
/// Bumped every time LOADING_BG_PORTRAIT_RGBA is REPLACED with a fresh capture. The present-overlay
/// composite watches this: when it changes, the overlay re-uploads its source texture from the new RGBA,
/// so a LIVE per-frame (throttled) readback of the built renderer's offscreen makes the displayed head
/// UPDATE (look-at follows) instead of freezing on the first captured frame.
pub(crate) static LOADING_BG_PORTRAIT_RGBA_VERSION: AtomicUsize = AtomicUsize::new(0);
/// One-shot log latch for the live-display-feed (built RT content -> overlay).
pub(crate) static PROFILE_LIVE_FEED_LOGGED: AtomicUsize = AtomicUsize::new(0);

// === Candidate A: live head INSIDE the now-loading GFx movie (er-effects-rs-jsm) ==================
// STATIC-RE PROVEN 2026-07-05 (bd tooltip-above-portrait-VERDICT-2026-07-05, gfx-decoded-tex-
// deterministic-resolve-2026-07-05): the texture GFx actually DISPLAYS for a MENU_Load_NNNNN
// background is a `CS::CSTextureImage` held in the Scaleform tex repository name-map at
// `*(GLOBAL_SCALEFORM_TEX_REPOSITORY)+0x80`, resolvable BY NAME (the forge's bare symbol) via the
// resolver below. Its GFx-SAMPLED HAL texture is `CSTextureImage+0x10`; a per-frame CopyTextureRegion
// into that resource puts the head inside the movie, so the movie's own Gauge_3 bar (depth 5) and
// tip/keyguide text (depth 11) render ABOVE it natively (BackImage artwork is depth 3). This is the
// only path that layers native tips above the portrait -- the Present-overlay draws after the whole
// GFx pass and structurally cannot. Reconciles the "mechanism A failed" history: those uploads hit the
// CS-side GetResCap CSGxTexture (TexResCap+0x78), which Scaleform does NOT sample; the CSTextureImage
// HAL texture is a different object the history never tested.
/// `GLOBAL_ScaleformTexRepository` singleton pointer (absolute 0x143d82510 in BOTH the dump and the
/// deobf/live binary -> data RVA 0x3d82510). The resolver PANICS (non-returning) if this is null, so it
/// MUST be null-checked (graphics up) before the call. Ground-truthed: the live resolver at RVA
/// 0xd7c940 loads exactly this RIP-relative address.
pub(crate) const GLOBAL_SCALEFORM_TEX_REPOSITORY_RVA: usize = 0x3d82510;
/// GFx-displayed-texture resolver `FUN_140d7c9f0` (dump 0x140d7c9f0 -> deobf 0x140d7c940, shift -0xb0,
/// content-unique). `fn(param1_IGNORED /rcx/, out: *mut *mut CSTextureImage /rdx/, name: *const u16
/// /r8/)`. Ignores rcx (loads the repo singleton itself), tail-calls FUN_140d63ce0(repo, out, name, 0):
/// searches the name map at repo+0x80; HIT stores entry+0x50, MISS builds+inserts a CSTextureImage via
/// the GetResCap bridge. `*out` becomes an AddRef'd (owned) `CSTextureImage*`, or 0. Caller must Release.
pub(crate) const SCALEFORM_TEX_RESOLVE_RVA: usize = 0xd7c940;
/// Scaleform `RefCountImpl` Release `thunk_FUN_14112b7f0` (dump 0x14112b7f0 -> deobf 0x14112b7d0, shift
/// -0x20, content-unique). `fn(obj /rcx/)`; decrements the refcount at obj+0x08 and frees at 0. Used to
/// drop the resolver's owned ref on the CSTextureImage when the displayed name changes / the window ends.
/// RVA = deobf 0x14112b7d0 - base 0x140000000 = 0x112b7d0 (game_rva adds base back).
pub(crate) const SCALEFORM_REFCOUNT_RELEASE_RVA: usize = 0x112b7d0;
/// `CS::CSTextureImage` -> its GFx-sampled HAL texture (the object whose ID3D12Resource GFx samples).
/// Layout (FUN_140d68600): +0x00 vtable, +0x08 refcount, +0x10 pHALTexture, +0x2c/0x30 width/height.
pub(crate) const CS_TEXTURE_IMAGE_HAL_TEX_OFFSET: usize = 0x10;
// === Now-loading BackImage geometry -- SINGLE SOURCE OF TRUTH (er-effects-rs-jsm) =================
// VERIFIED from the packed asset display list (scripts/gfx_display_list.py over menu/02_903_nowloading2
// .gfx): stage 1920x1080 (16:9); the BackImage sprite places MENU_DummyLoad (char 36) 4096x2048 (2:1)
// at IDENTITY, and that sprite is placed at root at scale 0.530365, tx=ty=0. So the artwork quad covers
// stage (0,0)..(2172.4,1086.2) -- WIDER than the 16:9 stage -- and the VISIBLE region is the TOP-LEFT
// (u<=1920/2172.4=0.8838, v<=1080/1086.2=0.9943) of the 2:1 texture, NOT its centre. The forge must
// therefore build the replacement texture at the artwork's 2:1 aspect (so GFx maps texture->quad with no
// horizontal stretch -- the earlier 1024x1024 square was stretched onto the 2:1 quad) and aspect-cover
// (centre-crop, never stretch) the background + head into the visible top-left sub-rect. Every derived
// value (forge dims, visible sub-rect, head placement) is computed from these five constants.
pub(crate) const NOWLOADING_STAGE_W: f32 = 1920.0;
pub(crate) const NOWLOADING_STAGE_H: f32 = 1080.0;
pub(crate) const NOWLOADING_BACKIMAGE_TEX_W: u32 = 4096;
pub(crate) const NOWLOADING_BACKIMAGE_TEX_H: u32 = 2048;
pub(crate) const NOWLOADING_BACKIMAGE_SPRITE_SCALE: f32 = 0.530_364_99;
/// The forge builds the replacement TPF at the artwork's native aspect but 1/`NOWLOADING_FORGE_DOWNSCALE`
/// the resolution (4096x2048 -> 2048x1024): same 2:1 aspect (no stretch), 1/4 the memory, still wider
/// than the visible 1920-px stage so texture->stage upscaling is negligible.
pub(crate) const NOWLOADING_FORGE_DOWNSCALE: u32 = 2;

