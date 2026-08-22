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
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
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
pub(crate) use er_title_flow::TitleSessionRva;

pub(crate) use er_title_flow::TitleOwnerLayout;

#[repr(C)]
#[allow(dead_code)] // Retained RE layout: decoded struct shape, nothing constructs it today.
pub(crate) struct TitleOwnerLoadJobLayout {
    pub(crate) unknown_000: [u8; 0xd8],
    pub(crate) pending: i32,
}

pub(crate) use er_title_flow::TITLE_OWNER_STATE_OFFSET;
pub(crate) use er_title_flow::TITLE_OWNER_STATE_COMMITTED_OFFSET;
pub(crate) use er_title_flow::TraceSampleLimit;

pub(crate) use er_title_flow::TITLE_OWNER_SCAN_COUNTDOWN_READY;
pub(crate) use er_title_flow::MenuTraceRva;

pub(crate) const TRACE_MENU_CONTINUE_WRAPPER_RVA: u32 = MenuTraceRva::ContinueWrapper as u32;
pub(crate) const TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = MenuTraceRva::NewOrLoadWrapper as u32;
pub(crate) const TRACE_MENU_OTHER_LOAD_WRAPPER_RVA: u32 =
    er_save_loader::MENU_OTHER_LOAD_WRAPPER_RVA;
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const TRACE_MENU_TASK_UPDATE_TABLE_RVA: u32 = MenuTraceRva::TaskUpdateTable as u32;
pub(crate) const TRACE_TASK_ENQUEUE_RVA: u32 = MenuTraceRva::TaskEnqueue as u32;
pub(crate) const RESULT_EVENT_HANDLER_RVA: u32 = MENU_JOB_EMIT_RESULT_RVA;
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
pub(crate) const DIRECT_INPUT_KEY_DOWN_MASK: u8 = 0x80;
pub(crate) use er_title_flow::MENU_TRACE_UNSEEN_SEQ;
pub(crate) const POST_MAP_CONTINUATION_STATE_QWORD: usize = 2;
pub(crate) use er_title_flow::TITLE_OWNER_SCAN_START_ADDRESS;
pub(crate) use er_title_flow::TITLE_NATIVE_JOB_NOT_CALLED;



// ── Title-animation speedup lever (pab_dismiss -> menu_open) ─────────────────────────────────
// The title/menu transition is a Scaleform/GFx animation advanced by the FD4 frame-delta f32 the
// STEP_MenuJobWait tick (0x140b0d400) reads from its task_data+0x08 and forwards to
// CS::TitleTopDialog::update. FadeIn->Loop / TextFadeOut completion is frame-count CHECKED
// (current==total), NOT time-gated, so SCALING this delta makes the animation reach its end frame
// in fewer wall-clock frames -- every downstream predicate (Scaleform tick, completion compare,
// (flags&0x8f)>1 settle gate) is satisfied naturally; nothing is bypassed and the load does not
// desync. bd autoload-menu-speed-lever-framedelta-2026-06-22.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const TITLE_ANIM_SPEEDUP_MAX: f32 = 16.0;
/// DEFAULT-ON for real autoload runs (no opt-in). Any value > 1.0 ARMS the FadeIn skip; the magnitude
/// no longer scales anything (the dt-scale and frame-burst levers were both runtime-falsified -- bd
/// title-anim-framedelta-lever-FALSIFIED-runtime-2026-06-24 + pab-to-menuopen-real-breakdown-build-not-
/// anim-2026-06-24 -- the FadeIn is wall-clock/present-bound, so we skip it at the completion predicate
/// instead). Kept as an f32 toggle so the existing env/file override (set to 1.0 = off) still works.
pub(crate) const TITLE_ANIM_SPEEDUP_DEFAULT: f32 = 4.0;
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
pub(crate) const CS_MENU_MAN_GLOBAL_RVA: usize = er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;
/// OptionSetting tab-select VISIBILITY pass `FUN_14093b850` (deobf 0x93b760):
/// `fn(CompositeOptionSettingDialog* composite, int tabIndex, u8* r8, u8* r9)`. It sets the current
/// pane (`composite+0xb8 = cache[tabIndex]`, building via the switch dispatch only if the cache slot is
/// null), then iterates the 10 cached pane dialogs at `composite+0x68` and calls `SetVisible(dialog+0x1200,
/// current==dialog)` on each -- showing ONLY the active tab's pane, hiding the rest. This is the game's
/// own per-tab visibility application. Re-invoking it on restore re-shows the active OptionSetting pane
/// that our hide/restore left with DisplayInfo.Visible=0 (the blank Game Options pane).
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
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
/// (NOT a shift: 0x14081f7e0 and 0x14081f6f0 are two DIFFERENT functions. 1.16.2 shift is 0; 0x14081f7e0 (size 235) builds L"05_000_Title", 0x14081f6f0 (size 239) builds L"05_010_ProfileSelect". The old parenthetical came from dump-deobf-shift.py against the 1.16.1 dump and would send you to the Title-movie builder. Corrected 2026-08-01.). We use it as the initial custom cover surface
/// instead of trying to remap `05_001_Title_Logo`, which has no dummy-profile symbol.
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_SELECT_WRAPPER_RVA: usize = PROFILE_SELECT_WRAPPER_RVA as usize;
pub(crate) const TITLE_CUSTOM_COVER_PROFILE_SELECT_NAME: &str = "05_010_ProfileSelect";
/// Native full-screen black Scaleform/MenuWindowJob surface. Ghidra dump 0x140793c10 ->
/// deobf/live 0x140793b20 (content-unique) builds `01_900_Black` with the same
/// MenuWindow/SceneProxy host ABI as the title wrappers. This is the first diagnostic carrier for
/// proving an engine-owned custom surface can stay above PRESS ANY BUTTON / Continue.
pub(crate) const TITLE_CUSTOM_COVER_BLACK_WRAPPER_RVA: usize = 0x793b20;
pub(crate) const TITLE_CUSTOM_COVER_BLACK_NAME: &str = "01_900_Black";
pub(crate) const TITLE_CUSTOM_COVER_DUMMY_PROFILE_SYMBOL: &str = "MENU_DummyProfileFace_01";
pub(crate) use er_title_flow::TITLE_CUSTOM_COVER_SYSTEX_TARGET;
pub(crate) use er_title_flow::TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS;
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_PROFILE_SOURCE_SAMPLE_CALLS;
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
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const MENU_WINDOW_JOB_RUN_RVA: usize = 0x7ad1c0;
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static TITLE_CUSTOM_COVER_RUN_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_RUN_RECURSION;
pub(crate) use er_telemetry::counters::TITLE_CUSTOM_COVER_RUN_CALLS;
/// PAB detour -> system_quit_menu_window_run_post call count. Confirms the deterministic-winner wiring
/// (2026-07-15 install-race fix) is live at runtime: >0 means PAB is driving run_post on MenuWindowJob::Run
/// passes, so the hide + slot-activation-gate latches get written regardless of the MinHook race.
pub(crate) use er_telemetry::counters::PAB_RUN_POST_CALLS;
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_NATIVE_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_COVER_JOB: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_COVER_WINDOW: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static TITLE_CUSTOM_COVER_RUN_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const TITLE_CUSTOM_COVER_GX_TEXTURE_RESOURCE_OFFSET: usize = 0x10;

// Mechanism and its version-anchored Scaleform identities live in `er-scaleform-hooks` (R8).
// The product retains only the telemetry readers used by its runtime-oracle writer.
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
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const LOADING_BG_REPLACE_BIND_RVA: usize = 0xd697d0;
/// In-memory TPF -> TpfResCap factory `CreateTpfResCap` (dump 0x140b83770 -> deobf 0x140b83680).
/// `fn(tpfRepo /rcx = *GLOBAL_TpfRepository/, name: *const u16 /rdx/, bytes: *const u8 /r8/, size: u64
/// /r9/, flag: u8 /stack=0/, extra: u32 /stack=0/) -> *mut TpfResCap` (0xb8; +0x78 count, +0x80 array).
pub(crate) const CREATE_TPF_RESCAP_RVA: usize = 0xb83680;
/// `CS::TpfFileCap::TpfFileCap` ctor (dump 0x140226010 -> deobf 0x140225f60). `fn(this: *mut /0x98
/// from MainHeap/, loadTask=0) -> this`; only inits the FD4FileCap base and zeroes `+0x90`.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const TPF_FILE_CAP_CTOR_RVA: usize = 0x225f60;
/// Game heap allocator wrapper (dump 0x141eb9ec0 -> deobf 0x141eb9ed0). `fn(size /rcx/, align /rdx/,
/// allocator_obj /r8/) -> *mut u8`; allocator_obj is the dereferenced DLAllocator* (== the repo's
/// `runtime_heap_allocator` for MainHeap).
pub(crate) const GAME_HEAP_ALLOC_RVA: usize = 0x1eb9ed0;
/// `DLString<wchar_t>::substr` (dump 0x140116c90 -> deobf 0x140116c70). `fn(dest /rcx/, src /rdx/,
/// start /r8 = 0/, count /r9 = usize::MAX = to-end/) -> dest`; copies the symbol into the rti symbol.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const DLSTRING_WCHAR_SUBSTR_RVA: usize = 0x116c70;
// `GLOBAL_TpfRepository` singleton pointer (deref -> rcx for CreateTpfResCap) is defined below as
// the existing `GLOBAL_TPF_REPOSITORY_RVA` (0x3d73fb8).
/// `GLOBAL_MainHeapAllocator` singleton pointer (data, 0x143d872e0; identical RVA to the repo's
/// `runtime_heap_allocator`). Deref -> the allocator object for the 0x98-byte TpfFileCap allocation.
pub(crate) const GLOBAL_MAIN_HEAP_ALLOCATOR_RVA: usize =
    er_game_base::rva::GLOBAL_MAIN_HEAP_ALLOCATOR_RVA;
/// CSScaleformReplaceTexInfo (size 0x50) field offsets.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const REPLACE_TEX_INFO_REFCOUNT_OFFSET: usize = 0x8; // i32 DLReferenceCountObject refcount
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const REPLACE_TEX_INFO_SYMBOL_OFFSET: usize = 0x10; // DLString<u16>
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const REPLACE_TEX_INFO_ENCODING_OFFSET: usize = 0x38; // u8
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const REPLACE_TEX_INFO_TPF_FILE_CAP_OFFSET: usize = 0x40; // TpfFileCap*
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const REPLACE_TEX_INFO_READY_OFFSET: usize = 0x48; // u8 (leave 0 so the pump processes it)
/// TpfFileCap (size 0x98) field offsets.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const TPF_FILE_CAP_LOAD_STATE_OFFSET: usize = 0x88; // u8
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const TPF_FILE_CAP_FLAGS_OFFSET: usize = 0x89; // u8
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const TPF_FILE_CAP_TEX_RESCAP_OFFSET: usize = 0x90; // -> TpfResCap container
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const TPF_FILE_CAP_LOADED_STATE: u8 = 4;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const TPF_FILE_CAP_READY_FLAG_BIT: u8 = 0x20;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const TPF_FILE_CAP_ALLOC_SIZE: usize = 0x98;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
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
