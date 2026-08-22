//! Constants, statics, and plain data types moved VERBATIM out of the root
//! er-effects-rs crate for the title-flow extraction (Stage B of
//! docs/plans/title-flow-crate-extraction.md). Each item's original site now
//! carries a `pub(crate) use er_title_flow::NAME;` shim, so the root crate and
//! this crate share the single definition below. Only visibility changed
//! (`pub(crate)` -> `pub`); bodies and doc comments are untouched.
// PARITY: DEBT -- verbatim transcription of constants moved out of er-effects-rs, kept
// import-for-import identical to keep that move reviewable as a pure move. The unused
// imports are the cost of that fidelity and should go once the move stops being audited.
#![allow(unused_imports)]

use std::sync::{
    Mutex, Once, OnceLock,
    atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering},
};

use eldenring::cs::{GameDataMan, GameMan};
use er_telemetry::counters::*;
use fromsoftware_shared::F32Vector4;

// ===== moved verbatim from crates/er-effects-rs/src/constants.rs =====

pub const NULL_MODULE_BASE: usize = 0;

pub const HOOK_FALSE_RETURN: u8 = 0;

// ===== moved verbatim from crates/er-effects-rs/src/constants/anti_debug.rs =====

pub const TITLE_OWNER_VTABLE_RVA: usize = TitleSessionRva::TitleOwnerVtable as usize;

pub const TITLE_OWNER_STATE_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, requested_state);

/// Committed/current state the inner-TitleStep dispatcher actually runs (the pump
/// commits +0x4c -> +0x48 each frame and dispatches on +0x48). +0x4c is the
/// requested/next state. Read +0x48 to know the live state.
pub const TITLE_OWNER_STATE_COMMITTED_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, committed_state);

/// The inner TitleStep stores a per-instance copy of its state-dispatch table
/// base (0x143d71580) at owner+0x10; the dispatcher reads [owner+0x10]. Requiring
/// this rejects stray .data vtable matches (e.g. the 0x1000ffc58 false positive).
pub const TITLE_OWNER_INSTANCE_TABLE_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, instance_table);

pub const INNER_TITLE_STATE_TABLE_RVA: usize = 0x3d71580;

pub const TITLE_OWNER_SCAN_ALIGNMENT: usize = core::mem::align_of::<usize>();

pub const TITLE_OWNER_SCAN_MAX_ADDRESS: usize =
    (true as usize) << (usize::BITS as usize - (u16::BITS as usize + true as usize));

pub const TITLE_OWNER_TRACE_LIMIT: usize = TraceSampleLimit::Value64 as usize;

/// How many `title_owner` calls to skip between full-memory owner scans.
///
/// The owner scan walks every committed region via `VirtualQuery`; running it
/// every frame while the owner does not yet exist (or cannot be matched)
/// collapses the game's frame rate. Throttling to roughly once per second at
/// 60 fps keeps a failed lookup from being user-visible.
pub const TITLE_OWNER_SCAN_CALL_INTERVAL: usize = TitleNativeJobTiming::FrameRate as usize;

pub const TITLE_OWNER_SCAN_COUNTDOWN_STEP: usize = true as usize;

pub const TITLE_OWNER_SCAN_COUNTDOWN_READY: usize = usize::MIN;

pub const TITLE_MENU_JOB_WAIT_RVA: usize = MenuTraceRva::MenuJobWait as usize;

/// Legacy native-autoload startup delay is a diagnostic tick throttle only; product autoload
/// phases must use semantic predicates plus wall-clock fail-safe deadlines, never frame budgets.
pub const TITLE_NATIVE_JOB_MIN_TICK: u64 = 170;

pub const MEM_COMMIT_NUMERIC: u32 = 0x1000;

pub const PAGE_NOACCESS_NUMERIC: u32 = 0x01;

pub const PAGE_GUARD_NUMERIC: u32 = 0x100;

pub const DIRECT_INPUT_FAILURE_HRESULT: i32 = -1;

pub const MENU_TRACE_UNSEEN_SEQ: usize = NULL_MODULE_BASE;

pub const TITLE_OWNER_SCAN_START_ADDRESS: usize = usize::MIN;

pub const TITLE_OWNER_QUERY_FAILED_BYTES: usize = usize::MIN;

pub const PAGE_PROTECTION_NO_FLAGS: u32 = 0;

pub const TITLE_OWNER_MIN_STATE: i32 = TitleStepState::Min as i32;

pub const TITLE_OWNER_MAX_STATE: i32 = TitleStepState::Finish as i32;

pub const TITLE_NATIVE_JOB_NOT_CALLED: usize = false as usize;

pub const TITLE_TRACE_SEQUENCE_INCREMENT: usize = 1;

pub const TITLE_NATIVE_JOB_TASK_DATA_ZERO: u8 = false as u8;

pub const TITLE_NATIVE_JOB_TASK_DATA_BYTES: usize = core::mem::size_of::<TitleNativeJobTaskData>();

pub const TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR: f32 = true as u8 as f32;

pub const TITLE_NATIVE_JOB_FRAME_RATE: f32 = TitleNativeJobTiming::FrameRate as u32 as f32;

pub const TITLE_NATIVE_JOB_DELTA_OFFSET_START: usize =
    core::mem::offset_of!(TitleNativeJobTaskData, frame_delta);

pub const TITLE_NATIVE_JOB_DELTA_OFFSET_END: usize =
    TITLE_NATIVE_JOB_DELTA_OFFSET_START + core::mem::size_of::<f32>();

pub const TITLE_NATIVE_JOB_CALLED_VALUE: usize = true as usize;

/// Clamp range for the speedup factor.
pub const TITLE_ANIM_SPEEDUP_MIN: f32 = 1.0;

/// Log the title SM state every this many detour calls.
pub const TITLE_ANIM_DIAG_INTERVAL: usize = 60;

/// FD4 state-machine `SetState`/request-transition (deobf 0x1407499e0; dump 0x140749ae0, shift -0x100).
/// `__fastcall(rcx = FD4StateMachine* sm, rdx = StateDesc* desc)`. Routes the transition through the
/// SM owner's vtable[0x150] and no-ops unless the current node is settled (`[node+0x20]&0x8f >= 2`), so
/// it cannot corrupt the SM. This is the call CS::TitleTopDialog::update's input-skip branch makes to
/// move FadeIn->Loop on a button press. bd fadein-* RE 2026-06-24.
/// NOT an FD4 SetState: 0x7499e0 is a Scaleform **frame-label goto** on a SceneObjProxy
/// (er-loading-portrait names it correctly as SCALEFORM_LABEL_GOTO_RVA and this now derives
/// from it, 2026-08-01). Its operands are frame LABELS, not StateDescs. The old name is kept
/// for its call sites in title_load_step_hooks.rs.
pub const TITLE_FD4_SETSTATE_RVA: usize = er_loading_portrait::SCALEFORM_LABEL_GOTO_RVA;

/// One-shot latch: the zero-input FadeIn->Loop transition has fired.
pub static TITLE_FADEIN_SKIP_FIRED: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub const TITLE_CUSTOM_COVER_SYSTEX_TARGET: &str = "SYSTEX_Menu_Profile00";

pub const TITLE_CUSTOM_COVER_PROFILE_RENDERER_CLASS: &str = "CSMenuProfModelRend";

/// Profile renderer table initializer: live 0x1409af3a0 (dump 0x1409af4f0) allocates the ten
/// CSMenuProfModelRend instances and writes DAT_143d6d8d0 before the refresh/feed pass below.
pub const TITLE_CUSTOM_COVER_PROFILE_RENDER_INIT_RVA: usize =
    er_loading_portrait::PROFILE_TABLE_BUILDER_RVA;

/// Profile portrait refresh/display pipeline: live 0x1409aa680 (dump 0x1409aa7d0) reads the loaded
/// `ProfileSummary`, loops 10 slots, fills CSMenuProfModelRend / face/player model data, and maps
/// each active slot to `SYSTEX_Menu_ProfileNN` through `FUN_140bb8cf0(renderer, slot*2)`. It must run
/// after SL2/profile readiness, not at early `05_001_Title_Logo` construction time.
pub const TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_RVA: usize =
    er_loading_portrait::PROFILE_RENDERER_REFRESH_RVA;

pub static TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_CALLS: AtomicUsize = AtomicUsize::new(0);

pub static TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_PROFILE_SUMMARY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_RENDER_REFRESH_LAST_CALLER_PHASE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub const TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_754: usize = 0x754;

pub const TITLE_CUSTOM_COVER_PROFILE_RENDER_READY_FIELD_755: usize = 0x755;

pub const TITLE_CUSTOM_COVER_PROFILE_RENDERER_TEX_INDEX_OFFSET: usize = 0x9a8;

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_SLOT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_RENDERER_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_OFFSCREEN_REND: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_RESCAP: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_TEX_INDEX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_754: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_CUSTOM_COVER_PROFILE_SOURCE_READY_755: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

// ===== moved verbatim from crates/er-effects-rs/src/constants/autoload_state.rs =====

/// CS::MenuMemberFuncJob<TitleTopDialog> vtable 0x142b265d0 (RVA): the registry-entry node the
/// registrar 0x1409b24e0 inserts into [dialog+0xa48]; its run is MENU_MEMBER_FUNC_JOB_RUN_RVA.
/// (Mirrors the local MEMBERFUNCJOB_VTABLE_RVA in scan_dialog_for_loadgame.)
pub const MEMBERFUNCJOB_VTABLE_RVA: usize = 0x2b265d0;

/// TitleTopDialog row registry [dialog+0xa48] (the FD4 delegate registry the registrar populates).
/// Used as the live-menu readiness signal: populated == the menu rows are registered + rendered.
pub const DIALOG_ROW_REGISTRY_A48_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, row_registry);

/// GameMan+0xb80 (== GameMan.save_state == load_in_progress) FSM values. The full-save read walks
/// IDLE(0) -> OPENING(1) -> READING(2) -> RESIDENT(3); a healthy load then drains RESIDENT -> IDLE as
/// the deserialize consumes the 0x280000 buffer. `load_in_progress_b80_name` (constants::return_title)
/// gives the display names. The finalize case-7 gate (FUN_14067a170 == save_state==0) waits on b80
/// reaching IDLE; on the warm reload it is stuck at RESIDENT because the deserialize never consumes it.
pub const GAME_MAN_SAVE_STATE_IDLE: i32 = 0;

/// GameMan+0xb80 == 3 == RESIDENT (the full-save read drained into the 0x280000 buffer). The DRAIN
/// phase ticks the lane + poll each frame until b80 reaches this.
pub const FULLREAD_B80_RESIDENT: i32 = 3;

/// Full-read chain phase machine states (one step per frame).
pub const FULLREAD_PHASE_SUBMIT: usize = 0;

pub const FULLREAD_PHASE_GUARD: usize = 3;

/// Live phase + drain-wait counters for the full-read chain (one-shot per run).
pub static FULLREAD_PHASE: AtomicUsize = AtomicUsize::new(FULLREAD_PHASE_SUBMIT);

/// The native full-read chain shares the semantic `title_menu_action_ready` menu readiness gate;
/// it no longer latches a first-seen frame before starting the save-read phase machine.
/// `save_requested`: bound to the upstream typed layout (compiler-verified equal to our prior
/// hand-decoded offset).
pub const GAME_MAN_ARM_FLAG_B72_OFFSET: usize = core::mem::offset_of!(GameMan, save_requested);

pub const GAME_MAN_FLAG_B73_PROBE_OFFSET: usize =
    GAME_MAN_ARM_FLAG_B72_OFFSET + core::mem::offset_of!(GameManAutoloadFlagCluster, probe_b73);

pub const GAME_MAN_FLAG_B75_PROBE_OFFSET: usize =
    GAME_MAN_ARM_FLAG_B72_OFFSET + core::mem::offset_of!(GameManAutoloadFlagCluster, probe_b75);

/// `requested_save_slot_load_index`: bound to upstream (compiler-verified equal to our offset).
pub const GAME_MAN_REQUESTED_SLOT_B78_OFFSET: usize =
    core::mem::offset_of!(GameMan, requested_save_slot_load_index);

pub const GAME_MAN_FLAG_BC4_OFFSET: usize =
    core::mem::offset_of!(GameMan, is_in_online_mode) - core::mem::size_of::<u32>();

/// Submit-gate diagnostics (b80-submit-kick-exact-false-gate-decoded-2026). The b72
/// autoload initiator 0x14067b750 sets GameMan+0xb80=1 ONLY if the async submit
/// 0x140e6ec70 returns true; the submit body 0x140e6f940 bails FALSE if the IO device
/// has a STALE request in-flight ([iodev+0x10]!=0) or a stale request handle
/// ([iodev+0x20]!=0). The IO device global is abs 0x144589390 (RVA 0x4589390); we read
/// it both as a possible pointer-to-device and as a struct base so the log
/// disambiguates. Also: the b72 effective-getter 0x1406793d0 zeroes b72 if
/// [GameMan+0xbc4]==3 or [inputmgr+0x13c]!=0, so log those too.
pub const IODEV_GLOBAL_RVA: usize = er_game_base::rva::SL_IODEV_GLOBAL_RVA;

pub const IODEV_INFLIGHT_10_OFFSET: usize = 0x10;

/// The async-IO request handle the poll 0x140e6e080 actually reads is the PAIR
/// [iodev+0x18] && [iodev+0x20] (a *started* request). 0x14067b4e0's preview read
/// (0x140e6ec80) is what populates these; 0x14067b200's queue (0x140e6eb80) goes to
/// the file-device-mgr instead, so it never appears here. Logging both pins which
/// initiator actually started the iodev read (menu-b80-mount-orchestration-sequence).
pub const IODEV_REQHANDLE_18_OFFSET: usize = 0x18;

pub const IODEV_REQHANDLE_20_OFFSET: usize = 0x20;

pub const ARM_PROBE_MIN_TICK: u64 = 60;

pub const ARM_PROBE_TICK_INTERVAL: u64 = 30;

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
/// **This is the engine's SHUTDOWN/CLEANUP flag, not a title latch.** The game writes this
/// byte exactly ONCE in the whole image, at 0x140c8ff41 inside `MainLoop` (0x140c8fe90, sole
/// caller `WinMain`) -- immediately after the `while (MainUpdate())` loop exits and immediately
/// before the `while (CleanupUpdate())` teardown loop. It is false for the entire normal game
/// lifetime. Of its 25 xrefs, 1 is that write and 24 are readers of MIXED polarity: some
/// suppress on set (`SaveRequest_Profile`, `RequestSave`), some ACT on set
/// (`STEP_MenuJobWait` -> SetState(0xb)). Step machines short-circuit to terminal states at
/// teardown, which is exactly why product code writing it appears to "work" -- it advances the
/// title by telling the whole engine it is shutting down. See bd er-effects-rs-d4em.
/// Canonical name should be GAME_SHUTDOWN_CLEANUP_FLAG_RVA; the four existing aliases now
/// derive from this one declaration (2026-08-01) so the value has a single home.
pub const TITLE_ACCEPT_LATCH_RVA: usize = 0x3d856a0;

/// Boot intro/movie singleton (ptr) and its decoder skip-flag byte. The latch
/// 0x143d856a0 is set by the intro thread 0x140c8fe90 only after its movie-wait
/// loop ends; the movie-dismiss gate 0x140e90820 finishes on decode-complete or
/// when the skip-flag byte 0x14458b8a5 is non-zero (sole non-WNDPROC effect is the
/// movie's own stop). Setting the skip-flag drives a genuine zero-input dismiss.
pub const MOVIE_SINGLETON_RVA: usize = 0x458b890;

pub const MOVIE_SKIP_FLAG_RVA: usize = 0x458b8a5;

pub const MOVIE_SKIP_FLAG_CLEAR: u8 = 0;

pub const MOVIE_SKIP_FLAG_SET: u8 = 1;

/// Movie controller vtable RVA (0x142bfe088), HWND field offset (M+8), and the
/// USER32 constants for mirroring the WNDPROC WM_CLOSE teardown.
pub const MOVIE_VTABLE_RVA: usize = 0x2bfe088;

pub const MOVIE_HWND_OFFSET: usize = 0x8;

pub const WND_SC_CLOSE: u32 = 0xf060;

pub const WND_MF_BYCOMMAND: u32 = 0;

pub const WND_SW_HIDE: i32 = 0;

pub const WND_GET_SYSTEM_MENU_KEEP: i32 = false as i32;

/// ONLINE-DISABLE (headless offline boot, no "Unable to start in online mode" modal).
/// `GameMan::IsOnlineMode` getter 0x14067a030 = `mov rax,[rip+..]; movzx eax,[rax+0xbc8]; ret`
/// (the canonical online/offline flag, default 1=online, read by ~22 consumers incl. the boot
/// login flow). Patching the getter body to `xor eax,eax; ret` forces every consumer onto the
/// game's own OFFLINE branch, so the boot never attempts online login and the connection-error
/// modal is never raised. Single leaf accessor, no side effects -> equivalent to "Play Offline";
/// no save/crash risk. Verified (self-disasm, online-disable RE 2026-06-17): first byte 0x48.
pub const ONLINE_DISABLE_RVA: usize = 0x67a030;

/// `xor eax,eax; ret` -- returns 0 (offline) for the whole getter (the original body is 15
/// bytes followed by the next function, so a 3-byte stub is self-contained).
/// First byte of the IsOnlineMode getter's prologue (`0x48`, a REX.W prefix). Validated before the
/// stub is written so a drifted image aborts the patch instead of corrupting an unrelated function.
/// Moved here from the product's `constants/autoload_state.rs` with the code-patch primitives (S5):
/// this crate now calls `er_hook::apply_xor_ret_stub` directly and must supply the byte itself.
pub const ONLINE_DISABLE_EXPECTED_FIRST: u8 = 0x48;
pub const ONLINE_DISABLE_STUB: [u8; 3] = [0x31, 0xc0, 0xc3];

/// Sign-in force (cold save-load gate). The SaveLoad2 storage-select op ctor (deobf 0x14240f1b0)
/// creates its runnable ONLY if the sign-in check returns true AND the user index is <= 3; cold
/// (no signed-in user) both fail, so the op is null and the load FSM parks (the b80 wall). Patch
/// both gate fns to pass so the cold menu-free path loads as if signed in as user 0. Addresses
/// ground-truthed against the deobf/live binary (the Ghidra dump's FUN_1424129a0 / FUN_14240f480
/// are shifted; live entries below). Scoped to the cold-mount attempt, not attach.
/// `CS::..::IsSignedIn`-class check (dump FUN_1424129a0) -> always true.
pub const SIGNIN_FORCE_RVA: usize = 0x24129b0;

pub const SIGNIN_FORCE_EXPECTED_FIRST: u8 = 0x40;

pub const SIGNIN_FORCE_STUB: [u8; 3] = [0xb0, 0x01, 0xc3]; // mov al,1; ret

/// User-index resolver (dump FUN_14240f480) -> return 0 (valid index, <= 3) instead of 0xffffffff.
pub const USERINDEX_FORCE_RVA: usize = 0x240f490;

pub const USERINDEX_FORCE_EXPECTED_FIRST: u8 = 0x4c;

pub const USERINDEX_FORCE_STUB: [u8; 3] = [0x31, 0xc0, 0xc3]; // xor eax,eax; ret

/// Login-readiness predicate 0x140cab230 (`sub rsp,0x18; ...`, returns 1 only if all 3 session
/// mgrs == 2). The boot/menu network-flow step calls it to decide ONLINE-attempt vs OFFLINE; a
/// non-zero return makes it attempt online login, which FAILS offline -> the connection-error
/// modal re-pops on every menu transition (the popup LOOP). Patching it to `xor eax,eax; ret`
/// (return "not ready") makes the flow take the clean OFFLINE fork and NEVER attempt online.
/// Same 3-byte stub; first byte 0x48 (verified disasm). Applied with the getter patch.
pub const ONLINE_PREDICATE_DISABLE_RVA: usize = 0xcab230;

/// MENU OFFLINE-NOTICE GATE -- the THIRD menu-open popup, root-caused 2026-06-23
/// (bd `menu-open-3rd-popup-offline-mode-notice-2026-06-23`, Ghidra RE `er-effects-rs-yvf`).
/// `Menu_IsEnableOnlineMode` (deobf 0x140e56310) is a lazy-init cached getter that DEFAULTS TRUE. The
/// TitleTopDialog ctx-init step (0x14082d0d0) computes
/// `TitleFlowContext->notReleaseFlag55 (+0x18C) = !Menu_IsEnableOnlineMode()`. With the getter TRUE and the
/// boot offline, `notReleaseFlag55 == 0` routes the title-flow offline step (0x14082fda0) into building the
/// "Starting in offline mode" `GR_System_Message` (id 401170) `CS::MessageBoxDialog` -- which BLOCKS the
/// Continue/Load/NewGame row build (the stage-3 / 0-node continue-readiness wall). Patching this getter to
/// `xor eax,eax; ret` (return false) makes the game's OWN ctx-init set `notReleaseFlag55 = 1` every time it
/// runs, so the offline step takes the clean no-popup branch and the menu rows build with ZERO MessageBoxDialog
/// builds. Race-free (re-evaluated on each ctx-init, unlike a one-shot field poke). Applied with the
/// IsOnlineMode getter patch (offline-gated -> Seamless online is unaffected). Verified prologue first byte 0x40
/// (`push rbx`; deobf disasm). Reuses `ONLINE_DISABLE_STUB` (`xor eax,eax; ret`).
pub const MENU_ONLINE_MODE_DISABLE_RVA: usize = 0xe56310;

pub const MENU_ONLINE_MODE_EXPECTED_FIRST: u8 = 0x40;

pub use er_game_base::rva::{MSGBOX_DIALOG_VTABLE_RVA, MsgBoxRva};

/// CS::SaveRetryDialog vtable (RVA). A MessageBoxDialog SUBCLASS: the wrapper 0x1407af9a0 overrides
/// the base vtable to this AFTER the builder 0x1409275b0 runs. It is the "save/load failed -- Retry?"
/// prompt the offline title flow builds (save-data/profile read error in a degraded/offline env). The
/// auto-accept must recognize it by THIS vtable -- not the base MessageBoxDialog vtable (0x2b03550) --
/// or it bails before dismissing (the vtable mismatch is why auto-accept never fired). bd
/// offline-title-modal-is-saveretrydialog + press-any-button-golden-lever-job1e8-readiness-2026-06-23.
pub const SAVE_RETRY_DIALOG_VTABLE_RVA: usize = 0x2aaabf8;

pub const IN_WORLD_REACHED_YES: usize = 1;

/// Live/deobf native menu-job submit helper (`FUN_1407a9340` dump -> live `0x1407a9250`).
pub const MENU_JOB_SUBMIT_RVA: u32 = 0x7a9250;

/// Live/deobf native menu-job queue idle predicate (`FUN_1407a9320` dump -> live `0x1407a9230`).
pub const MENU_JOB_QUEUE_READY_RVA: u32 = 0x7a9230;

// ===== moved verbatim from crates/er-effects-rs/src/constants/gaitem_restore.rs =====

/// Direct poke of the streaming-enable flag [resmgr+0xb7c1]=1 (the virtual enabler
/// 0x14066e2e4 crashes -- wrong receiver). The virtual also builds session singletons
/// 0x143d687a0 / 0x143d67bd0; read them to see if the poke is safe (already built) or
/// if the job machine will deref null.
pub const RESMGR_STREAM_ENABLE_B7C1_OFFSET: usize = 0xb7c1;

pub const STREAMING_DRIVER_SINGLETON_RVA: usize = 0x3d7c088;

/// World-resource manager chain for STEP_WorldResWait residency (0x14066d3e0):
/// resmgr = [[MoveMapStep+0xf0]+0x10]; loaded-block count = [resmgr+0xb3140].
/// count==0 -> no map-block registered (setup gap); count>0 but block not at load
/// phase 0xa -> streaming gap. Diagnostic for the final wall.
pub const MOVEMAPSTEP_WORLDRES_F0_OFFSET: usize = 0xf0;

pub const WORLDRES_RESMGR_10_OFFSET: usize = 0x10;

pub const RESMGR_BLOCK_COUNT_B3140_OFFSET: usize = 0xb3140;

/// m10 block load-state (mirrors 0x14066d3e0 readiness tail): loadstate =
/// entry->vtable[+0x10](entry); ready iff [loadstate+0x2d]!=0 AND [loadstate+0x35]==0xa.
/// Reading [+0x35] live shows which load phase the m10 block is stuck at (<0xa).
pub const BLOCK_LOADSTATE_GETTER_VT_10_OFFSET: usize = 0x10;

pub const BLOCK_LOADSTATE_FLAG_2D_OFFSET: usize = 0x2d;

pub const BLOCK_LOADSTATE_PHASE_35_OFFSET: usize = 0x35;

/// PHASE-2 STALL DISCRIMINATORS (added 2026-07-30 for the profile-switch reload freeze:
/// a warm reload parks at `[+0x35]==2` for 50s+ while the FIRST load in the same process
/// clears the same phase; run product-continue-direct-20260730-134058).
///
/// Two 1.16.1-era RE accounts of what phase 2 polls disagree -- one says the FD4FileCaps
/// hang off this WorldBlockRes at `[+0x40]`/`[+0x48]`, the other says they hang off the
/// WorldAreaRes (which `fc_present`/`fc_notloaded` already scan, and which read all-loaded
/// while the block stayed at phase 2). These offsets sample the BLOCK's own copies so the
/// next run discriminates the accounts instead of assuming one. All read-only.
///
/// `[+0x2f]` is the gate the phase machine recomputes every tick as `[+0x2f]=[+0x2d]` iff
/// the block's IO-request object state is 6 and a virtual predicate passes, else 0. Phase 2
/// exits to phase 5 when it is 0, and only polls the caps when it is non-zero -- so it
/// separates "gave up" from "still waiting on a cap".
pub const BLOCK_LOADSTATE_ASSET_GATE_2F_OFFSET: usize = 0x2f;

/// Countdown the phase-9 handler decrements; underflow reverts `[+0x35]` to 8.
pub const BLOCK_LOADSTATE_COUNTDOWN_3C_OFFSET: usize = 0x3c;

/// Sticky "load gave up" byte the phase-2/3 handlers set alongside a fallback to phase 5.
pub const BLOCK_LOADSTATE_GAVEUP_06_OFFSET: usize = 0x06;

/// The block's own FD4FileCap pointers, populated by the phase-1 load requester. FOUR slots,
/// not two: the 1.16.2 handler at `0x1406158a0` reads `param_1[8]`, `[9]`, `[10]` and `[0xb]`
/// (a `longlong*`, so byte offsets 0x40/0x48/0x50/0x58), taking `cap+0x90` from each.
pub const BLOCK_LOADSTATE_FILECAP_SLOTS: [usize; 4] = [0x40, 0x48, 0x50, 0x58];

/// FD4FileCap load status; `0x04` == load complete. Paired with a non-null bytes pointer.
pub const FD4_FILECAP_STATUS_88_OFFSET: usize = 0x88;

/// `MsbFileCap::msbResCap` -- the PARSED MSB resource, not the raw file buffer. `FD4FileCap` is
/// exactly 0x90 bytes, so this is the first field of the `MsbFileCap` subclass.
///
/// IT HAS EXACTLY ONE ASSIGNMENT SITE (1.16.2 static RE, bd
/// `msbrescap-single-assignment-site-and-null-content-shortcircuit-2026-07-30`): the load-complete
/// callback `FUN_14021bbf0`, which does
/// `content = AcquireContent(cap); if (content != 0 && header_ok) { msbResCap = MsbRepository::
/// GetOrCreate(name, content, size); } ReleaseContent(cap);`
/// -- and returns NORMALLY when `content` is null. `loadState` is already `4` by then, nothing
/// errors and nothing retries, so `(loadState=4, msbResCap=0)` is a reachable SILENT TERMINAL state.
/// That is precisely the profile-switch reload freeze: WorldBlockRes case 2 advances to phase 3 only
/// on `cap+0x90 != 0`, and its only other escape is also closed, so the block spins at phase 2 with
/// no timeout.
///
/// `0` and `0xDEADBEEF` mean DIFFERENT things and the distinction is the key discriminator:
/// `MsbFileCap::MsbFileCap` (0x14021b880) inits it to `0`, while `~MsbFileCap` (0x14021b940)
/// releases through the repository and then stores `0xDEADBEEF`. So `0` == NEVER PARSED, never
/// "freed after use".
pub const FD4_FILECAP_BYTES_90_OFFSET: usize = 0x90;

/// `FD4ResCapHolderItem::resourceString` is an `FD4BasicHashString` at `cap+0x08`, whose
/// `DLString<wchar_t>` starts at `cap+0x10`: union (inline `wchar[8]` OR pointer) at `+0x08`,
/// `length` at `+0x18`, `capacity` at `+0x20`. `capacity > 7` means the union holds a POINTER.
/// Reading it names WHICH msb the stalled cap is, which separates "wrong file requested" from
/// "right file, empty read".
pub const FD4_FILECAP_NAME_UNION_18_OFFSET: usize = 0x18;

pub const FD4_FILECAP_NAME_LENGTH_28_OFFSET: usize = 0x28;

pub const FD4_FILECAP_NAME_CAPACITY_30_OFFSET: usize = 0x30;

/// Inline-vs-pointer threshold for `DLString<wchar_t>` (SSO capacity).
pub const DLSTRING_INLINE_CAPACITY_MAX: usize = 7;

/// Cap the wide-name read so a garbage `length` cannot walk the probe off a page.
pub const FD4_FILECAP_NAME_MAX_CHARS: usize = 96;

/// `FD4ResCapHolderItem::referenceCount`. Discriminates a FRESH cap (the reload built it) from a
/// CACHE-HIT SURVIVOR still held by the outgoing world -- the two remaining explanations for a
/// null `msbResCap`.
pub const FD4_FILECAP_REFCOUNT_58_OFFSET: usize = 0x58;

/// `FD4FileCap::loadProcess` -> `FD4FileLoadProcess::fileLoadProcessor` (`+0x20`) -> the content
/// the load-complete callback gates on. A null anywhere along this chain makes `AcquireContent`
/// return null, which is exactly the short-circuit that leaves `msbResCap` at `0`.
pub const FD4_FILECAP_LOADPROCESS_78_OFFSET: usize = 0x78;

/// `FD4FileCap::flags`; the `MsbFileCap` factory `FUN_1401f3560` sets `0x20` on every cap it builds
/// before handing it to `AddFileCap`.
pub const FD4_FILECAP_FLAGS_89_OFFSET: usize = 0x89;

/// `DLIO::DLFileDeviceManager` singleton. `GetFileDeviceManager` (0x141f48b40) is literally
/// `MOV RAX,[0x1448464a8]` plus a null-check branch, so this global IS the manager pointer.
///
/// NOTE this corrects bd `step3-census-registry-null-on-load2-mount-skip-confirmed-2026-07-17`,
/// which called the same address "the mounted-archive registry". It is not: a genuinely null
/// manager would break every file read in the process, so that census reading `null` was a
/// deref-depth/timing artifact and the conclusion drawn from it does not follow.
pub const DL_FILE_DEVICE_MANAGER_SINGLETON_RVA: usize = 0x0484_64a8;

/// `FUN_140e05fb0(CSDlcImp*, bool)` -- the DLC virtual-root REFILL: re-queries Steam DLC ownership
/// and calls `CSDlcImp::AddVirtualFileRoots`.
pub(crate) const DLC_ROOTS_REFILL_RVA: usize = er_game_base::rva::DLC_ROOTS_REFILL_RVA;

/// `GLOBAL_CSDlc` -- the `CSDlcImp` singleton.
pub(crate) const CSDLC_SINGLETON_RVA: usize = er_game_base::rva::CSDLC_SINGLETON_RVA;

/// The DLIO alias every failing `m28` read resolves through.
pub(crate) const DLC_ROOT_ALIAS_NAME: &str = "mapstudio_dlc2";

/// `DLFileDeviceManager::virtualRoots` -- a `FileDeviceVirtualRootVector`
/// (`allocator +0x00`, `start +0x08`, `end +0x10`, `capacity +0x18`).
///
/// THIS IS THE PHASE-2 FREEZE SUSPECT. The stalled caps are named
/// `mapstudio_dlc2:/m28_00_00_00.msb`, and `mapstudio_dlc2` is an entry in THIS vector, not a data
/// archive. It has a two-phase lifecycle: `FUN_140e06490(CSDlc, true)` -- called only from the title
/// start-game flow `FUN_1409b24e0` -- registers 13 `*_dlc2` aliases with an EMPTY root `L""`, and
/// only `CSDlcImp::AddVirtualFileRoots` (0x140e06b80, reachable solely via `FUN_140e05fb0`, whose
/// callers are `CS::MoveMapListStep::STEP_LoadListWait` and one title-flow function) fills in the
/// real `mapstudio_dlc2 -> "map_dlc2:/mapstudio"`. If the title blanks it and the
/// `STEP_LoadListWait` gate (`loadList == NULL || *loadList in {2,3}`) does not pass on the warm
/// reload, the alias stays empty, the msb read resolves against nothing and returns 0 bytes, and
/// `msbResCap` never gets written. Reading the alias AT the stall settles that without any hook.
pub const DL_FILE_DEVICE_MANAGER_VIRTUAL_ROOTS_48_OFFSET: usize = 0x48;

pub const FILE_DEVICE_VIRTUAL_ROOT_VECTOR_START_08_OFFSET: usize = 0x08;

pub const FILE_DEVICE_VIRTUAL_ROOT_VECTOR_END_10_OFFSET: usize = 0x10;

/// `FileDeviceVirtualRootVectorEntry`: `root` (the alias name) and `path` (what it resolves to),
/// both `DLString<wchar_t>` (48 bytes each), so the entry stride is 0x60.
pub const FILE_DEVICE_VIRTUAL_ROOT_ENTRY_STRIDE: usize = 0x60;

pub const FILE_DEVICE_VIRTUAL_ROOT_ENTRY_PATH_30_OFFSET: usize = 0x30;

/// `DLString<wchar_t>` field offsets RELATIVE TO THE STRING ITSELF (`allocator +0x00`,
/// union `+0x08`, `length +0x18`, `capacity +0x20`). The `FD4_FILECAP_NAME_*` constants above are
/// the same layout pre-added to the cap's `+0x10` string base, which is why their numbers differ.
pub const DLSTRING_UNION_08_OFFSET: usize = 0x08;

pub const DLSTRING_LENGTH_18_OFFSET: usize = 0x18;

pub const DLSTRING_CAPACITY_20_OFFSET: usize = 0x20;

/// Bound the virtual-root walk: the table is a few dozen aliases, so anything past this is a
/// corrupt/mid-teardown vector and the probe must stop rather than walk off a page.
pub const FILE_DEVICE_VIRTUAL_ROOT_MAX_ENTRIES: usize = 256;

/// Alias prefixes worth reporting at the stall: the DLC roots that back `m28`, plus the base-game
/// `mapstudio` for contrast (if the base alias is populated and the dlc2 one is not, that is the
/// answer outright).
pub const VIRTUAL_ROOTS_OF_INTEREST: [&str; 4] =
    ["mapstudio_dlc2", "map_dlc2", "game_dlc2", "mapstudio"];

pub const FD4_FILELOADPROCESS_PROCESSOR_20_OFFSET: usize = 0x20;

/// `FD4FileLoadProcessor`: `content_` at `+0x20`, its byte count at `+0x28`, and the
/// acquire/release refcount at `+0x30` whose `1 -> 0` edge nulls `content_` and frees the buffer.
pub const FD4_FILELOADPROCESSOR_CONTENT_20_OFFSET: usize = 0x20;

pub const FD4_FILELOADPROCESSOR_SIZE_28_OFFSET: usize = 0x28;

pub const FD4_FILELOADPROCESSOR_ACQUIRE_30_OFFSET: usize = 0x30;

/// GameMan `save_slot` (compiler-verified equal to the upstream typed field).
pub const FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET: usize = core::mem::offset_of!(GameMan, save_slot);

/// Save-manager load-in-progress flag (GameMan/save-mgr singleton 0x143d69918):
/// `0x14067b570` sets `[mgr+0xb80]=1` when it begins the load and clears it to 0
/// when finished. The native autoload (recipe A) arms the load by setting the
/// slot (`+0xac0`) and the force flag `0x143d856a0`, then the save-manager
/// per-frame update `0x14067f5d0` performs it.
/// Bound to upstream `GameMan::save_state` (compiler-verified equal to our offset); our research
/// reads this same dword as the load-in-progress lane (set 1 on load begin, cleared on finish).
pub const GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET: usize = core::mem::offset_of!(GameMan, save_state);

/// GameDataMan -> main player save data (compiler-verified equal to the upstream typed field).
pub const SLOT_MANAGER_DATA_OFFSET: usize =
    core::mem::offset_of!(GameDataMan, main_player_game_data);

pub const CSFEMAN_SINGLETON_RVA: usize = 0x3d6b880;

/// Session manager singleton (absolute 0x1447ef360; NULL at the title, built by
/// the move-map/load path). RVA = 0x1447ef360 - 0x140000000 = 0x47ef360.
pub const SESSION_SINGLETON_RVA: usize = TitleSessionRva::MoveMapSession as usize;

/// Alias of the `CSMenuMan` singleton. Derived from `er-game-base`'s table so the value has
/// exactly one definition (2026-08-01 RVA dedupe); the name is kept for its call sites.
pub const TITLE_INPUT_MANAGER_RVA: usize = er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;

/// Pure-observe snapshot interval (game-task ticks). Logs the title->menu->load state
/// every N ticks with NO forcing, to capture what the REAL button press does.
pub const OBSERVE_INTERVAL: u64 = 10;

/// Observe change-detection: log a snapshot only when the packed signature changes
/// (full granularity, minimal file I/O). Multiplier for the rolling signature.
pub const OBSERVE_SIG_MULT: i64 = er_game_base::fnv1a::FNV1A64_PRIME as i64;

pub static OBSERVE_LAST_SIG: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(i64::MIN);

/// `CS::MenuJobQueue::PushBackJob` (live entry `0x1407a9250` -- prologue-grounded vs eldenring-deobf.bin:
/// `mov [rsp+0x10],rdx; push rdi; sub rsp,0x30; movq $-2,[rsp+0x20]`; dump `FUN_1407a9340`). CORRECTED
/// from the prior `0x7a9254`, which was +4 INTO the first instruction (mid-`mov`) and would execute
/// garbage -- a latent bug that likely helped kill the gated `own_load_install_job` path. APPENDS a job
/// into a MenuJobQueue (`AtomicIncrement`s the job, then appends into the container at
/// `owner+0x8`).
///
/// CORRECTED 2026-08-01 -- the two safety properties this doc used to assert are BOTH FALSE, and
/// the transmute at `er-effects-rs .../own_load/loaders.rs` cites them as its justification:
///   * "does NOT ... zero `*src`" -- it DOES. The tail Unrefs the caller's reference and then
///     executes `*param_2 = 0`, clearing the source slot.
///   * "is overflow-safe (NOT the cap-8 FixOrderJobSequence)" -- the insert it delegates to,
///     `FUN_1407a8820`, is typed by the dump as `(undefined8, FixOrderJobSequence *)`. It is a
///     BOUNDED, FAILABLE push: it inserts only when `capacity_field == 0 || count < capacity`,
///     and otherwise silently drops the job and returns 0. A caller that ignores the return can
///     lose an enqueue with no error.
///
/// Win64 fastcall `(rcx = queue_base, rdx = src: *MenuJob* (a DLReferenceCount
/// Pointer slot whose [0] is the job))`. Queue targets: `owner+0x130` (ring +0x138, count +0x178;
/// STEP_MenuJobWait's ExecuteMenuJob ticks it) OR `dialog+0x10` (ring +0x18; the per-frame menu pump
/// 0x1409aa680 over the active-screen array drains it -- the native Continue post target).
/// bd continue-load-POST-primitive-pushbackjob-kick-2026-06-22.
pub const MENUJOB_PUSHBACK_RVA: usize = MENU_JOB_SUBMIT_RVA as usize;

// ===== moved verbatim from crates/er-effects-rs/src/constants/own_load_pump.rs =====

/// `FD4::FD4Time` size (dump `/FD4/FD4Time` len 16): `+0x0 vtable ptr`, `+0x8 f32 time` (the frame
/// delta the map-stream sub-job advances on). Run only READS `time+8`. Pass a 16-byte buffer with the
/// f32 frame delta at +8 (a zeroed buffer => delta 0.0 is valid; the deser self-builds regardless).
pub const FD4_TIME_SIZE: usize = 0x10;

pub const FD4_TIME_DELTA_8_OFFSET: usize = 0x8;

pub const DIALOG_OWNER_CTX_A38_OFFSET: usize = 0xa38;

/// CS::TitleFlowContext dispatch-state field (`tfc = *(TitleTopDialog+0xa38)`; `tfc+0x14c`). The
/// live user-driven Continue capture (bd LIVE-continue-chain-via-selector-NOT-confirm-handler) showed
/// the load runs through the selector `0x1409a8eb0` which reads this field and dispatches to the load
/// dispatcher `0x1409b3070` (0=idle, 1=load, 3/5=busy). Setting it to 1 at the settled main menu is
/// the candidate DIRECT "Continue pressed" trigger (no input) -- the exact bit we change.
pub const TFC_DISPATCH_STATE_14C_OFFSET: usize = 0x14c;

pub const TFC_DISPATCH_STATE_LOAD: i32 = 1;

/// CS::TitleFlowContext `notReleaseFlag55` byte at `tfc+0x18c`. The load dispatcher `0x1409b3070`
/// gates its BUILD-the-LoadGame-job branch on `IsNotReleaseFlag55` (`0x14082cd60`: `cmpb $0,0x18c(rcx)`
/// -> returns 1 iff the byte is 0); the dispatcher takes the LOAD branch ONLY when that returns
/// nonzero, i.e. when `*(u8*)(tfc+0x18c)==0`. The open-menu path sets this nonzero AFTER press-any-
/// button, so a Continue trigger fired post-menu-open lands on the ABORT branch (empty job, no load).
/// Force this to 0 before invoking the selector to guarantee the real LoadGame build. bd
/// dispatcher-abort-branch-force-tfc-18c-zero-2026-06-23.
pub const TFC_NOT_RELEASE_FLAG_18C_OFFSET: usize = 0x18c;

pub const TFC_NOT_RELEASE_FLAG_CLEAR: u8 = 0;

/// CS::TitleTopDialog Continue-item SELECTOR `0x1409a8eb0` -- the menu-item-action funclet that the
/// engine invokes on Continue confirm (it is NOT pumped from the idle menu; setting tfc+0x14c alone
/// is dormant -- bd tfc-bit-dormant-even-at-open-menu). ABI `__fastcall(rcx = &dialog_slot, rdx = out
/// MenuJobResult*)`: it does `rcx=*(rcx)` (dialog), `*(dialog+0xa38)`=tfc, reads `*(tfc+0x14c)`; when
/// that == 1 (TFC_DISPATCH_STATE_LOAD) it takes the LOAD branch -- `r8=dialog+0x50`, calls the load
/// dispatcher `0x1409b3070` (the PROPER CS::MenuJob::ChainMenuJobs enqueue, no FixOrderJobSequence
/// overflow), and wraps the built job into rdx. Pass rcx = owner+0xe0 (its [0] is the live dialog).
/// Verified by disasm of 0x1409a8eb0 + the live user-Continue capture (selector body 0x9a8f09 ->
/// 0x9b3070). bd LIVE-continue-chain-via-selector-NOT-confirm-handler.
pub const TITLE_CONTINUE_SELECTOR_RVA: usize = 0x9a8eb0;

/// CS::TitleTopDialog MenuJobQueue at `dialog+0x10` (ring at +0x18) -- the queue the native Continue
/// path posts the built LoadGame job into, drained each frame by the menu pump `0x1409aa680` (which
/// iterates the active-screen array `0x143d6d8d0` that holds the live `owner+0xe0` dialog). The
/// selector/dispatcher only BUILD + return the job; we PushBackJob it here so it is pumped to
/// completion. bd continue-load-POST-primitive-pushbackjob-kick-2026-06-22.
pub const DIALOG_MENU_QUEUE_10_OFFSET: usize = 0x10;

/// Menu-pump KICK pointer: `*(base+0x3b37c98)` holds `0x1409b3ff0` (a `jmp` thunk into the obfuscated
/// per-frame pump trigger). The native posts a MenuJob then calls this zero-arg to drain it promptly;
/// we replicate that after PushBackJob. RVA = abs - base; the stored value is an ABSOLUTE code ptr.
pub const MENU_PUMP_KICK_PTR_RVA: usize = 0x3b37c98;

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
pub const MENU_DRAIN_WRAPPER_RVA: usize = 0x7a90f0;

/// `ExecuteMenuJob` (deobf `0x1407a9600`; dump `0x1407a96f0`). `__fastcall(rcx = *MenuJob* (slot),
/// rdx = *FD4Time {vtbl; f32 delta@+0x8})`: `cur=*rcx; if(!cur) return; AtomicIncrement(cur+8);
/// cur->vtable[+0x10](cur, &result, &{FD4Time vtbl, delta}); if(!MenuJobResult::ShouldContinue)
/// *rcx=0; AtomicDecrement`. We call this directly on OUR built job each frame (rcx=&job_slot) to
/// pump it via its OWN vtable[2] -- correct for the dispatcher's chained LoadGame job, and it avoids
/// the dialog's `+0x8` slot (which is NOT a MenuJob and AV'd the queue-drain wrapper). Grounded by
/// prologue on eldenring-deobf.bin (the `vtable[2]` call site `0x1407a968b call *0x10(rax)`).
pub const EXECUTE_MENU_JOB_RVA: usize = 0x7a9600;

/// CS::MenuManImp singleton global (`*(base+0x3d6b7b0)` = CSMenuManImp*). Verified: HasTopMenuJob
/// 0x14080d960 does `mov rax,[0x143d6b7b0]; mov rcx,0x80(rax)` (popupMenu) then reads +0xB0. (Same
/// singleton whose +0x90 is the menu input bitmap.) bd menu-job-install-mechanism-2026-06-23.
pub const GLOBAL_CSMENUMAN_RVA: usize = er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;

/// CSMenuManImp -> CSPopupMenu* at +0x80.
pub const CSMENUMAN_POPUP_80_OFFSET: usize = 0x80;

/// CSPopupMenu -> `currentTopMenuJob` (MenuJob*) at +0xB0 -- the single top-job slot the per-frame
/// menu pump drains (no cap). Install our built LoadGame job here so the native pump runs its Run
/// IN CONTEXT (vs our menu-jumping self-pump).
pub const CSPOPUP_TOP_JOB_B0_OFFSET: usize = 0xB0;

/// `CS::MenuJob::Assign(rcx = dest MenuJob**, rdx = out MenuJob**, r8 = src MenuJob**)` (deobf
/// 0x1407a9460 -- verified prologue: homes r8/rdx, `rbx=*dest`; if `*dest != *src` AtomicDecrements
/// the old occupant (0x141eba200) + dtors if last, then installs `*dest=*src` + AtomicIncrement).
/// Refcount-correct slot replace -- use to install our job into currentTopMenuJob without leaking the
/// displaced title-FSM job. NOTE: distinct from MENUJOB_ASSIGN_RVA (0x7a9560, a 2-arg move-assign).
pub const MENU_JOB_ASSIGN3_RVA: usize = 0x7a9460;

/// CS::MenuJob (DLReferenceCountObject) refcount field at +0x8 (vfptr at +0x0).
pub const MENU_JOB_REFCOUNT_8_OFFSET: usize = 0x8;

/// CS::TitleTopDialog embedded MenuWindowJob `DLFixedVector<MenuJob*,8>` at `dialog+0x50` -- the push
/// target our built load job's `CS::MenuWindowJob::Run` (`0x1407ad53b call 0x140733ef0`) inserts its
/// window into. Pinned via the push-site sw-bp diagnostic (rcx=`dialog+0x50`). Cap-8 and already FULL
/// with the dialog's windows, so the load window's push #9 overflows ("out of memory"
/// DLFixedVector.inl:662). Reset its count to make room. bd OVERFLOW-VECTOR-PINNED-dialog-plus-0x50.
pub const DIALOG_MENUWINDOW_VEC_50_OFFSET: usize = 0x50;

/// DLFixedVector element-count field at +0x48 (the push reads/increments `[vector+0x48]`, panics >8).
/// The dialog+0x50 vector's count is thus at `dialog+0x50+0x48 = dialog+0x98`.
pub const DLFIXEDVECTOR_COUNT_48_OFFSET: usize = 0x48;

/// CSMenuSystemSaveLoad save-slot field (`mss+0x1200`). The native confirm handler `0x1409a9250`
/// writes the slot here (the builder `0x1409ac8b0` reads it at `0x1409ac9d2` as the factory `r8`).
/// Replicate that write so the direct trigger loads the intended slot.
pub const MSS_SAVE_SLOT_1200_OFFSET: usize = 0x1200;

/// GameMan/GameDataMan singleton global read by `GetSaveSlot` (`*(0x143d69918)`, slot at `+0xac0`):
/// the "rest of GameMan is set up" readiness signal the user observed after press-any-button. The
/// direct continue trigger only fires once this is non-null. RVA = abs - base.
pub const GAME_SAVE_SLOT_SINGLETON_RVA: usize = er_game_base::rva::GAME_MAN_SINGLETON_RVA;

/// Plausible-pointer bounds for validating `owner_ctx = *(mss+0xa38)`: at `title_boot_ready` the
/// TitleFlowContext is often uninitialized (reads as 0x8080808080808080 -- non-null garbage), so a
/// `!= 0` check is insufficient. A real wine-heap pointer sits roughly in `0x1_0000 .. 0x8000_0000_0000`
/// (the golden value was 0x7fff..); anything outside is treated as "not built yet" -> pass NULL.
pub const OWNER_CTX_MIN_PLAUSIBLE_PTR: usize = 0x1_0000;

pub const OWNER_CTX_MAX_PLAUSIBLE_PTR: usize = 0x8000_0000_0000;

pub const OWN_STEPPER_LOG_INTERVAL: u64 = TitleNativeJobTiming::FrameRate as u64;

pub const OWN_STEPPER_CALL_INC: usize = true as usize;

/// Driver phases for the in-context idx10 handler.
pub const OWN_STEPPER_PHASE_MENU: usize = OwnStepperPhase::Menu as usize;

pub const OWN_STEPPER_PHASE_DONE: usize = OwnStepperPhase::Done as usize;

pub const OWN_STEPPER_SLOT_NONE: i32 = !OWN_STEPPER_SLOT_ZERO;

/// Lowest valid save-slot index (used to bounds-check the dialog cursor in STAGE 2).
pub const OWN_STEPPER_SLOT_ZERO: i32 = false as i32;

/// Save slot to load (parsed from the trigger file "slot=N"; -1 => leave the game's
/// own most-recent selection).
pub static OWN_STEPPER_SLOT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(OWN_STEPPER_SLOT_NONE);

pub static OWN_STEPPER_PHASE: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PHASE_MENU);

// ===== moved verbatim from crates/er-effects-rs/src/constants/player_correctness.rs =====

pub static OBSERVE_MENU_OPEN_EMITTED: AtomicUsize = AtomicUsize::new(OBSERVE_MARKER_NOT_EMITTED);

pub const OBSERVE_MARKER_NOT_EMITTED: usize = 0;

pub const OBSERVE_MARKER_EMITTED: usize = 1;

pub static OWN_STEPPER_BASE: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

// ===== moved verbatim from crates/er-effects-rs/src/constants/profile_render.rs =====

pub const SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE: usize = 0;

pub const SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED: usize = 2;

pub const SYSTEM_QUIT_QUICKLOAD_PHASE_TITLE_OWNER_SEEN: usize = 3;

pub const SYSTEM_QUIT_QUICKLOAD_PHASE_AUTOLOAD_HANDOFF: usize = 4;

/// Child-done-query override (FUN_140eb5550, deobf 0x140eb5530). STEP_MoveMap_Update tears the
/// MoveMapStep child down when this returns done; for load2 it returns done PREMATURELY (field25=0),
/// stranding the reload. The MoveMapStep child's EzChildStepBase = MoveMapStep + 0x108 (isolates its
/// call from the generic query's other callers). We hold its result not-done while the finalize is
/// mid-walk on a committed reload, so the child survives and the advancer completes.
pub const CHILD_DONE_QUERY_RVA: usize = 0xeb5530;

pub const MOVEMAPSTEP_CHILD_EZSTEP_BASE_OFFSET: usize = 0x108;

/// Held frozen-signature frames before the in-world drive fires. ~2s at load2's ~20fps; short so the
/// RAM-gated drive completes before an incidental unfocused-mouse click can contaminate the run.
pub const INWORLD_FINALIZE_DRIVE_RELEASE_FRAMES: usize = 40;

/// Sustained stuck-at-18 frames before the recovery drives the ending request (~2s at task rate).
pub const ENDING_REQUEST_STALL_RELEASE_FRAMES: usize = 120;

// ===== moved verbatim from crates/er-effects-rs/src/constants/render_handoff.rs =====

/// `CSMenuMan+0x728` -- `loadingScreenData.mode` written by deobf `FUN_14067a410` via the helper at
/// `0x140860d80`: `CSMenuMan+0x720+8 = mode`.
#[allow(dead_code)]
pub const CSMENUMAN_LOADINGSCREEN_MODE_728_OFFSET: usize = 0x728;

// ===== moved verbatim from crates/er-effects-rs/src/constants/return_title.rs =====

/// The "return-to-title / menu-rebuild requested" byte at menuData+0x5d.
pub const CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET: usize = 0x5d;

/// The "ending request" flag at menuData+0x5e that STEP_MoveMap's advancer FUN_140afa7c0 WRITES each
/// frame (`GLOBAL_CSMenuMan->menuData->field_0x5e = cVar10`). cVar10 = "an ending/load-completion
/// condition holds" (return-title 0x5d, warp, session WaitReload, deadReset==2, force-flag 0x3d856a0,
/// GameMan checks, state==8). STEP_MoveMap only walks the child toward its -1 terminal when this is 1;
/// if it stays 0 on a re-load, the child parks at resident step 18 and the InGameStep parent
/// (finished == MoveMapStep+0x48==-1) waits forever = the 2nd (runtime-accumulation) soft-lock. The
/// linchpin diagnostic: read 0x5e (the output) + 0x5d and the force-flag (inputs) at the lock.
pub const CS_MENU_DATA_ENDING_FLAG_5E_OFFSET: usize = 0x5e;

/// The force/ending latch global (BOOL_143d856a0) = one of the `cVar10` ending-request inputs.
pub const ENDING_REQUEST_FORCE_FLAG_3D856A0_RVA: usize = TITLE_ACCEPT_LATCH_RVA;

/// The remaining `cVar10` ending-request INPUTS that read GameMan directly (the load-in signals a
/// normal load sets so STEP_MoveMap walks the child to its -1 terminal): GameMan+0xb7c (FUN_140679520),
/// GameMan+0xb7d (FUN_140679530), and warpRequested at GameMan+0x10 (GameManIsWarpRequested). On the
/// stuck re-load one of these is 0 when it should be 1 -- that's the stale runtime flag to reset.
pub const GAME_MAN_ENDING_FLAG_B7C_OFFSET: usize = 0xb7c;

pub const GAME_MAN_ENDING_FLAG_B7D_OFFSET: usize = 0xb7d;

/// Gate used by the loading-screen mode setter at deobf `FUN_14067a410`: when this byte is 0,
/// mode 2 is normalized to mode 0 before calling the `CSMenuMan+0x720` mode writer.
pub const GAME_MAN_LOADING_MODE_BF5_OFFSET: usize = 0xbf5;

pub const GAME_MAN_WARP_REQUESTED_10_OFFSET: usize = 0x10;

/// `CSMenuManImp::disableSaveMenu` BOOL at CSMenuMan+0x13c. RE of the 1.16.1 dump (2026-07-16, persistent
/// Ghidra project): `CanShowSaveMenu` (dump 0x14080d150) returns `GLOBAL_CSMenuMan->disableSaveMenu != 0`,
/// and the native quit-save (GameMan `bc4` 1->2 pump `FUN_14067b840`/`FUN_14067ba30`, and `ShouldSave`
/// 0x1406794c0) ABORTS -- clearing `saveRequested` -- the instant this byte is non-zero. `bc4`
/// (GameMan+0xbc4) is the return-title predicate: REQUEST `FUN_14067a490` sets it 1, the quit-save pumps
/// 1->2, `FUN_14067aa70` pumps 2->3, and the world only tears down once it reaches 3. On a 2nd in-process
/// System->Quit switch `disableSaveMenu` is left set from the prior switch's menu flow, so the save never
/// runs, `bc4` freezes at 1, and the world never tears down (the observed switch-2 soft-lock). Switch 1
/// has it 0. We clear it while the switch is active so every switch matches switch 1. `GLOBAL_CSMenuMan`
/// (dump 0x143d6b7b0) == our `CS_MENU_MAN_GLOBAL_RVA` base+0x3d6b7b0, so the offset is version-stable.
pub const CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET: usize = 0x13c;

/// `TitleStep::InGameStep` pointer (TitleStep+0x2e8, read by STEP_GameStepWait at dump 0x140b0cee2).
pub const TITLE_STEP_IN_GAME_STEP_2E8_OFFSET: usize = 0x2e8;

/// `InGameStep` request-code register (+0xd8): 0=end session, 1=move-map pending, 2=move done /
/// stable in-world idle (see block comment above).
pub const IN_GAME_STEP_REQUEST_CODE_D8_OFFSET: usize = 0xd8;

/// In-game menu job pointer at CSMenuMan+0x798 (unnamed in fromsoftware-rs `unk748`); nonzero while
/// the in-game session's menu job lives. STEP_RequestWait ends the session when it reads 0 at
/// request code 2.
pub const CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET: usize = 0x798;

/// Loading-screen active bit written by `CS::InGameStep::STEP_MoveMap_Finish` before common finalize
/// and by `STEP_RequestWait` while the in-game menu job remains alive. Field path from Ghidra decompile
/// of dump 0x140aec140 / 0x140aecd00.
pub const CS_MENU_MAN_FIELD_6B0_OFFSET: usize = 0x6b0;

/// `[GLOBAL_CSDelayDeleteMan]` pointer global. Ghidra label `GLOBAL_CSDelayDeleteMan` at dump
/// `0x1445896a8`; `scripts/dump-deobf-shift.py 0x1445896a8` reports zero-shift data-region estimate.
pub const CS_DELAY_DELETE_MAN_GLOBAL_RVA: usize = 0x45896a8;

/// `CSDelayDeleteMan+0x40` pending-delete count/gate checked by `InGameStep::STEP_MoveMap_Finish`.
pub const CS_DELAY_DELETE_PENDING_40_OFFSET: usize = 0x40;

/// `CSDelayDeleteMan+0x54` flag toggled by `InGameStep::STEP_MoveMap_Finish`: 0 while pending deletes
/// exist, 1 immediately before `_Common_Finalize(param_1)`.
pub const CS_DELAY_DELETE_FINALIZE_54_OFFSET: usize = 0x54;

/// Native builder for a MenuJob wrapping the final return-title functor (`FUN_14079f780` dump ->
/// live/deobf `0x14079f690`). Submit this job through the native queue so the flag transition happens
/// in menu-pump ownership, not from our game-task thread.
pub const SYSTEM_QUIT_RETURN_TITLE_FINAL_JOB_BUILDER_RVA: u32 = 0x79f690;

pub static SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT: AtomicUsize = AtomicUsize::new(0);

pub static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT: AtomicUsize = AtomicUsize::new(0);

pub const INGAMESTEP_REQUEST_CODE_MOVEMAP_PENDING: i32 = 1;

pub const INGAMESTEP_REQUEST_CODE_STABLE_IN_WORLD: i32 = 2;

/// Last sampled InGameStep requestCode (+0xd8) for visible loading-bar sub-milestones.
pub static SWITCH_ORACLE_REQUEST_CODE: AtomicI32 = AtomicI32::new(-1);

/// Last sampled MoveMapStep finalize substate (+0x12a, 0..9) -- the real native sub-progression of
/// the visible MOVE MAP (18) loading phase, published for the loading-bar parenthesized sub-milestone.
/// -1 = no live MoveMapStep. See MOVEMAPSTEP_FINALIZE_SUBSTATE_NAMES.
pub static SWITCH_ORACLE_FINALIZE_12A: AtomicI32 = AtomicI32::new(-1);

/// Last sampled GameMan load-in-progress FSM (b80, == GameMan.save_state): 0 idle/done, 2 read
/// submitted, 3 resident. Published for the loading bar (a distinct, meaningful load-state the user
/// asked to see). The finalize case-7 gate (FUN_14067a170 = saveState==0) needs this back at 0.
pub static SWITCH_ORACLE_B80: AtomicI32 = AtomicI32::new(-1);

pub static SWITCH_ORACLE_LOADING_FIELD10: AtomicI32 = AtomicI32::new(-1);

pub static SWITCH_ORACLE_LOADING_FIELD11: AtomicI32 = AtomicI32::new(-1);

/// Last-seen streaming-enable bit + block count for the stall log (RAM semaphore, -1 = null chain).
pub static SWITCH_ORACLE_MMS_B7C1: AtomicI32 = AtomicI32::new(-1);

pub static SWITCH_ORACLE_MMS_BLOCKS: AtomicI32 = AtomicI32::new(-1);

/// Byte offset of the MoveMapStep finalize SUBSTATE within the STEP_MoveMap (step 18) phase. The
/// native advancer `FUN_140afa7c0` (dump VA) drives this `switch`-based sub-state 0..9; the load
/// orchestrator `FUN_140afb970` treats the world as ready ONLY when it is back to 0. So this is the
/// inner sub-progression of the visible "MOVE MAP 18" loading phase (see oracle finalize_substate_12a).
pub const MOVEMAPSTEP_FINALIZE_SUBSTATE_12A_OFFSET: usize = 0x12a;

/// The MoveMapStep child step index whose handler is `STEP_MoveMap` (dump registrar
/// FUN_1400a40c0: MoveMapStep_StepperArray[0x12]). This is the FINAL fade/finalize step; index 19 =
/// Cleanup, 20 = Finish follow. The 3rd-load softlock parks the child here.
pub const MOVEMAPSTEP_STEP_MOVEMAP_INDEX: i32 = 18;

/// Live/deobf RVA for `CS::MoveMapStep::STEP_MoveMap` (dump 0x140af7de0 -> deobf 0x140af7cf0,
/// content-unique shift -0xf0). Hooked after-original to clear +0x4b8 before the state machine consumes
/// the gate when the same-session reload has not proved movement yet.
pub const MOVEMAPSTEP_STEP_MOVEMAP_RVA: usize = 0x00af7cf0;

/// `CS::InGameStep::STEP_MoveMap_Update` (dump 0x140aec810 -> deobf 0x140aec720, content-unique shift
/// -0xf0). This is the PARENT step handler: it polls input/flipper, then `if (FUN_140eb5550(child)==0)
/// return;` (its own per-frame wait), and only past that does `field24_0xd8 = 2; FUN_140eb54e0(child)`
/// (advance requestCode to STABLE + tear the ending child down). On the warm reload FUN_140eb5550 (an
/// outer-stepper vtable done-query, DECOUPLED from the MoveMapStep finalize substate) reports finished
/// while the ending advancer is only at substate 8, so the teardown races ahead of case 8 (which would
/// post substate 9) and strands the reload -> revert to title (bd er-effects-rs-9fmm, fresh
/// load1-vs-load2 diff). The defer detour replicates the native's own "child not finished" early-return
/// while the MoveMapStep finalize substate is in [1..=8], giving the advancer the frames to reach 9.
pub const INGAMESTEP_STEP_MOVEMAP_UPDATE_RVA: usize = 0x00aec720;

/// Fail-soft cap: after this many consecutive held frames, stop deferring and let native decide (so a
/// genuine return-to-title whose finalize never completes can never be held forever). ~2s at 60fps.
pub const INGAMESTEP_MOVEMAP_UPDATE_DEFER_MAX: usize = 120;

/// MoveMapStep advance-gate byte (`field_0x4b8`). STEP_MoveMap sets the u16 at +0x4b8 to 1 each frame,
/// then blockers knock it down; it advances only when the LOW byte (+0x4b8) stays nonzero. Low byte 0 =
/// blocked; +0x4b9 high byte 1 with low 0 = the WorldChrMan-not-ready (`0x100`) branch fired.
pub const MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET: usize = 0x4b8;

pub const MOVEMAPSTEP_ADVANCE_GATE_HI_4B9_OFFSET: usize = 0x4b9;

/// STEP_MoveMap transition state (2026-07-16): after bc4 cleared, the child still parks at step 18 with
/// the per-frame gate (+0x4b8) ready, so the real 18->19 transition is a separate finalize condition. The
/// FD4StepTemplate "step done, advance" flag is `field8_0x50` (STEP_WorldResWait/STEP_MoveMap_Finish set
/// it; STEP_MoveMap's handler never does -> external/fade-driven). Read the child's next-step (+0x4c),
/// done-flag (+0x50), the fade hold-timer (+0x270, f32 bits; only counts down while the screen fade < 1.0
/// so a stuck-opaque fade freezes it), and the finalize counters (+0x100 field17, +0x248 field298) to
/// name the second gate at runtime.
pub const MOVEMAPSTEP_NEXT_STEP_4C_OFFSET: usize = 0x4c;

pub const MOVEMAPSTEP_DONE_FLAG_50_OFFSET: usize = 0x50;

pub const MOVEMAPSTEP_HOLD_TIMER_270_OFFSET: usize = 0x270;

pub const MOVEMAPSTEP_COUNTDOWN_100_OFFSET: usize = 0x100;

pub const MOVEMAPSTEP_FINALIZE_REQ_248_OFFSET: usize = 0x248;

/// TEARDOWN SAVE-REQUEST CLEAR (2026-07-16). The MoveMapStep ending sub-machine (FUN_140afa7c0) that
/// walks the old world's child out of STEP_MoveMap(18) hangs at case 7 unless `ShouldSave() == false`
/// AND `FUN_140679460() == false`. Those read `GameMan.saveRequested` (b72) and `GameMan+0xb73`, both
/// set by our return-title REQUEST (which intends a quit-save we suppress by design). Clearing them each
/// teardown frame makes the gate deterministically false so the world tears down with NO save. `_COUNT`
/// = frames we cleared the flags (a switch that stalls-then-recovers shows it climbing during teardown).
pub const GAME_MAN_SAVE_REQUESTED_B72_OFFSET: usize = 0xb72;

pub const GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET: usize = 0xb73;

/// STEP-3 (WORLD RES WAIT) DETERMINANT instrumentation (2026-07-16, Ghidra-proven). STEP_WorldResWait
/// (dump 0x140af9de0) advances 3->4 only when FUN_14066d4d0(worldInfoOwner, &currentBlockId) finds the
/// block matching currentBlockId's areaId in the world block-list AND that block's load-state reaches
/// +0x35==10. FieldArea = MoveMapStep+0xf0 (the oracle's `mms_wrm`); currentBlockId (BlockId u32) =
/// FieldArea+0x2c; worldInfoOwner = FieldArea+0x10 (`mms_resmgr`); block-list = worldInfoOwner+0xb3030
/// (array of block ptrs, count = worldInfoOwner+0xb3140 = the oracle's `blocks`). Each list entry i:
/// block_ptr=*(u64*)(list+i*8); inner=*(u64*)(block_ptr+0x8); block areaId=*(u32*)(inner+0xc). If, on a
/// step-3 stall, currentBlockId's areaId is NOT among the listed blocks -> the target block was never
/// registered (teardown left the wrong block set); if present -> its stream-state is stuck below 10.
pub const FIELDAREA_CURRENT_BLOCK_ID_2C_OFFSET: usize = 0x2c;

pub const WORLDINFO_BLOCK_LIST_B3030_OFFSET: usize = 0xb3030;

pub const WORLDINFO_BLOCK_ENTRY_INNER_8_OFFSET: usize = 0x8;

pub const WORLDINFO_BLOCK_AREA_ID_C_OFFSET: usize = 0xc;

pub const MOVEMAPSTEP_STEP_WORLDRESWAIT_INDEX: i32 = 3;

/// `CS::WorldInfoOwner::ProcessMsbLoadLists(WorldInfoOwner*, LoadlistlistFileCap*, LoadlistlistFileCap* dlc02)`.
/// ADDRESS CORRECTION (2026-07-17): the previous value 0x0066b2c0 was the DUMP RVA; the deobf/RUNTIME
/// address is 0x0066b1d0 (shift -0xf0, ground-truthed by scripts/dump-deobf-shift.py 0x14066b2c0). The
/// old value jumped 0xf0 INTO the function -> the "reactive ProcessMsbLoadLists AVs mid-stream" crash
/// (commit c43879c) AND the init-point crash (2026-07-17) were BOTH this wrong-address bug, not a timing
/// constraint. Runs ResetAreaResLists + PopulateLists to rebuild the per-block world-res from the loadlist;
/// dlc02 is null-checked in the callee, so 0 is safe for base-game (non-dlc) areas.
pub const WORLDINFO_PROCESS_MSB_LOADLISTS_RVA: u32 = 0x0066b1d0;

/// Load-request flag on the load-state object (FUN_14066d8d0 sets `+0x2c = 1` to request the block's
/// load). If the load-state exists but +0x2c is 0, the load was never requested.
pub const BLOCK_LOADSTATE_REQUEST_2C_OFFSET: usize = 0x2c;

/// The OVERWORLD block list on the WorldInfoOwner: `+0xb3148` = a u32 BlockId array (4-aligned),
/// `+0xb31d0` = its entry count. FUN_14066d8d0 routes OVERWORLD blocks (areaId in [0x32,0x59)) here
/// (via FUN_14063c5a0) instead of the +0xb3030 non-overworld path. Instrumented to confirm the
/// residual-outgoing-overworld hypothesis: if the boot char's m60 overworld blocks (area 0x3c) are
/// still resident here while we wait on the incoming legacy block (area 0x1c), the overworld residual
/// is what starves the legacy load-request. Each entry's areaId is its BlockId byte[3].
pub const WORLDINFO_OVERWORLD_LIST_B3148_OFFSET: usize = 0xb3148;

pub const WORLDINFO_OVERWORLD_COUNT_B31D0_OFFSET: usize = 0xb31d0;

/// LOADLIST ROOT LEAD (2026-07-16). STEP_MoveMap_LoadlistInit (InGameStep step 4, dump 0x140aec660)
/// builds the world-res loadlist ONLY when `worldloadlistlistVirtualPath.size != 0`
/// (`CMP qword [InGameStep+0x220], 0`); it then stores the built cap in `loadlistlistFileCap`
/// (`MOV [InGameStep+0x238], RAX`). If the path is empty, the loadlist is never built ->
/// `loadlistlistFileCap` stays null -> no world-res block load-states -> STEP_WorldResWait's null
/// load-state (blk_ls=0) stall. So at the stall `ll_size==0` + `ll_fcap==0` confirms the loadlist
/// was never built for the target area (our switch left the virtual path empty/stale).
pub const INGAMESTEP_WORLDLOADLIST_VPATH_BASE_210_OFFSET: usize = 0x210;

pub const INGAMESTEP_WORLDLOADLIST_VPATH_SIZE_220_OFFSET: usize = 0x220;

pub const INGAMESTEP_LOADLISTLIST_FILECAP_238_OFFSET: usize = 0x238;

/// Original System dialog saved for the post-ProfileSelect quickload return-title chain.
/// Unlike SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG, this must survive the ProfileSelect append observer reset.
pub static SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG: AtomicUsize = AtomicUsize::new(0);

/// The live MessageBoxDialog captured at build time (the connection-error / startup popup), so
/// the game task can force its result fields (OK + decided) each frame until the caller consumes
/// it. The finished-getter 0x1407b0cf0 is NOT polled for this dialog, so writing the fields
/// directly is the dismiss lever. 0 = none captured.
pub static CONNECTION_ERROR_DIALOG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

// ===== moved verbatim from crates/er-effects-rs/src/constants/stage2_menu_drive.rs =====

/// PHASE 6 (S2 INVOKE): fire d180's +0xa8 action functor to build the ProfileLoadDialog.
pub const OWN_STEPPER_PHASE_S2_INVOKE: usize = OwnStepperPhase::S2Invoke as usize;

/// PHASE 7 (S2 ACTIVATE): write the slot cursor [dialog+0xb0c]=N (bounds [dialog+0xb08]) then
/// call the dialog's vtable-slot-20 load_activate(rcx=dialog), registering the selector step.
pub const OWN_STEPPER_PHASE_S2_ACTIVATE: usize = OwnStepperPhase::S2Activate as usize;

/// PHASE 8 (S2 MOUNT_POLL): pass-through each frame so the native pump ticks the selector;
/// watch for the mount (ac0==N + io18/io20 set->clear; c30 leaving the new-game default).
pub const OWN_STEPPER_PHASE_S2_MOUNT_POLL: usize = OwnStepperPhase::S2MountPoll as usize;

/// PHASE 9 (S2 CONFIRM): guard (ac0==N && c30==latched-mount && io consumed) then
/// continue_confirm -> SetState(5) so the native pump streams the real world. The ONLY
/// save-write-risking step; gated entirely by a verified real mount (fail-closed otherwise).
pub const OWN_STEPPER_PHASE_S2_CONFIRM: usize = OwnStepperPhase::S2Confirm as usize;

/// CS::ProfileLoadDialog vtable (RVA). The dialog built by d180's functor (dialog_factory
/// 0x14081ead0 -> ctor 0x1409a3d90 writes this vtable). Used to VALIDATE the built dialog
/// before any dialog call (a wrong this-pointer would AV).
pub const PROFILE_LOAD_DIALOG_VTABLE_RVA: usize =
    ProfileLoadMenuRva::ProfileLoadDialogVtable as usize;

pub const DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogVtableLayout, load_activate);

#[repr(C)]
pub struct ProfileLoadDialogLayout {
    pub unknown_000: [u8; 0xb08],
    pub slot_bound: i32,
    pub slot_cursor: i32,
    pub unknown_b10: [u8; 0x11b8],
    pub load_job_ctx: usize,
}

/// Dialog selected-list-index cursor (= [dialog+0xa38+0xd4]); load_activate reads it as the
/// slot. WRITE the desired slot N here before calling load_activate.
pub const DIALOG_SLOT_CURSOR_B0C_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogLayout, slot_cursor);

/// Dialog list inclusive upper bound; load_activate clamps the cursor to [0, bound).
pub const DIALOG_SLOT_BOUND_B08_OFFSET: usize =
    core::mem::offset_of!(ProfileLoadDialogLayout, slot_bound);

/// The built+validated ProfileLoadDialog pointer (0 until PHASE_S2_INVOKE succeeds).
pub static OWN_STEPPER_DIALOG: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// One-shot latch, claimed with `swap`. It named the zero-input title-confirm fire
/// (`fire_titletop_load_entry`) until that route was deleted as disproven; the remaining claimants
/// use it to make a once-per-run announcement at the parked open menu rather than to suppress a
/// re-fire. Readers still treat "not `TITLE_NATIVE_JOB_NOT_CALLED`" as "the menu stage has run".
pub static OWN_STEPPER_TITLE_FIRED: AtomicUsize = AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);

// ===== moved verbatim from crates/er-effects-rs/src/constants/stats_panel_background.rs =====

pub const TITLE_STEP_BEGIN_NEW_GAME: i32 = TitleStepState::BeginNewGame as i32;

pub const TITLE_STEP_PLAY_GAME: i32 = TitleStepState::PlayGame as i32;

pub const TITLE_STEP_MENU_JOB_WAIT: i32 = TitleStepState::MenuJobWait as i32;

/// STEP_BeginLogo splash gate at [owner+0xb8]. CORRECTED 2026-06-23 (2 independent Ghidra REs +
/// deobf disasm, bd `beginlogo-builds-LOGO-not-menu-REFUTES-bd-2026-06-23`): 0x14081f180 builds the
/// boot LOGO/LEGAL SPLASH chain (05_905_Logo_Copyright / 05_900_Logo_FromSoft / 05_901_Logo_BNE /
/// 05_902_Logo_ESRB / 05_903_Warn_IllegalCopy), NOT the Continue/Load/NewGame menu. STEP_BeginLogo
/// 0x140b0c2a0 branches at 0x140b0c356 (`cmpb 0,[owner+0xb8]; je 0x140b0c3b2`): [0xb8]==0 -> 0x3b2 =
/// play logos (call 0x14081f180) then commit to owner+0x130 + SetState(10); [0xb8]!=0 -> SetState(3)
/// = STEP_BeginTitle, which SKIPS the logos and is what actually builds the Scaleform `05_000_Title`
/// menu (builder 0x14081f9f0). The splash-skip patch (0xb0c35d je->jg) makes [0xb8]==0 fall through to
/// SetState(3), so splash-skip ALREADY routes to the menu builder -- do NOT clear this gate + SetState(2)
/// to "build the menu" (that just replays the logos). The real continue-blocker is the offline-mode
/// notice popup; see bd `menu-open-3rd-popup-offline-mode-notice-2026-06-23`. Field kept for the (now
/// deprecated) own_stepper SetState(2) path only.
pub const TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, beginlogo_list_gate);

/// owner+0xe0 = the menu-job/dialog holder (CS::TitleTopDialog built by BeginTitle).
pub const TITLE_OWNER_MENU_HOLDER_E0_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, menu_holder);

/// owner+0x130 = where STEP_BeginLogo COMMITS the main-menu list (Continue/Load d180/NewGame).
/// Decoded from the commit fn 0x140b0e530: `lea rcx,[owner+0x130]; call 0x1407a9460` stores the
/// 0x14081f180-built list there, then SetState(owner,10). So the Load-Game d180 item lives under
/// owner+0x130, NOT owner+0xe0 -- walk this to find/invoke it.
pub const TITLE_OWNER_MENU_LIST_130_OFFSET: usize =
    core::mem::offset_of!(TitleOwnerLayout, menu_list);

/// Session singleton 0x144588e98 (RVA = abs - base). Asserted by STEP_BeginLogo(2) and the
/// MoveMapListStep load menu. Built by the boot/session bootstrap (may be non-null at the
/// splash-skipped parked title -- UNVERIFIED, hence read it live before SetState(2)).
/// RUNTIME-CONFIRMED non-null at the parked splash-skipped title (STAGE 1c).
pub const SESSION_SINGLETON_144588E98_RVA: usize =
    TitleSessionRva::SaveSafeBeginLogoSession as usize;

pub const TITLE_TOP_DIALOG_OPEN_MENU_RVA: usize = TitleDialogRva::OpenMenu as usize;

/// CS::TitleTopDialog vtable 0x142b26468 (RVA). Verify [owner+0xe0][0]==base+this before
/// calling the registrar (wrong receiver would fault on [dialog+0xa38]/[+0xa60]).
pub const TITLE_TOP_DIALOG_VTABLE_RVA: usize = TitleDialogRva::Vtable as usize;

/// CS::TitleTopDialog::update (the per-frame title menu pump) = deobf 0x1409aac10 = vtable slot 2
/// (`*(vtable+0x10)`, verified by reading the deobf vtable + the prologue). `__fastcall(rcx =
/// TitleTopDialog*, xmm1 = f32 delta, r8 = *InputData)`. It runs each frame with the LIVE dialog and,
/// at its tail, calls MenuWindow::Update (the FD4 job pump) which drains the menu jobs. Hooking it
/// lets our in-context Continue build run in the pump's frame (live dialog fields) -- the timing our
/// game-task build lacked (mis-context crash). bd HOOK-DESIGN-titletopdialog-update-0x1409aac10.
pub const TITLE_TOP_DIALOG_UPDATE_RVA: usize = 0x9aac10;

/// CS::TitleTopDialog cleanup/destructor body 0x1409a8890 (RVA). Static disassembly shows it
/// first restores the TitleTopDialog vtable, calls native active-screen clear 0x1409b2db0, then
/// releases dialog-owned renderer/resources before tail-calling the base cleanup. Unlike the
/// deleting destructor wrapper 0x1409aa250, this helper does not free the object allocation; it is
/// a safer post-world cleanup candidate for stale title-logo/frontend state after PlayerIns is
/// already valid.
pub const TITLE_TOP_DIALOG_CLEANUP_RVA: usize = TitleDialogRva::Cleanup as usize;

/// Active-screen array 0x143d6d8d0 (RVA): the per-frame pump 0x1409aa680 iterates it. 10 contiguous
/// screen* slots (stride 8). The LIVE-dialog scan reads each slot's [scr] vtable to find the live
/// TitleTopDialog and MenuWindow (the factory's SceneProxy capture + rdx) -- no blind heap scan.
pub const ACTIVE_SCREEN_ARRAY_RVA: usize = TitleDialogRva::ActiveScreenArray as usize;

/// Active-screen array slot count (bounded scan; the native pump iterates the same span).
pub const ACTIVE_SCREEN_ARRAY_SLOTS: usize =
    core::mem::size_of::<ActiveScreenArrayLayout>() / core::mem::size_of::<usize>();

/// Active-screen array slot stride (one screen* per slot).
pub const ACTIVE_SCREEN_ARRAY_STRIDE: usize = core::mem::size_of::<usize>();

/// Scan slot start / step.
pub const ACTIVE_SCREEN_SLOT_START: usize = usize::MIN;

pub const ACTIVE_SCREEN_SLOT_STEP: usize = true as usize;

/// TitleTopDialog SceneProxy capture slot: [dialog+0xa38] holds the live SceneProxy* the
/// TitleTopDialog ctor 0x1409a81a0 stored at 0x1409a8213. The LIVE-dialog factory 0x14081ead0
/// reads the SceneProxy from [rcx], so we pass rcx = dialog+0xa38 (factory r8 = *(dialog+0xa38)).
pub const DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, scene_proxy_capture);

/// CS::ProfileLoadDialog build factory 0x14081ead0 (RVA). Called as
/// `extern "system" fn(rcx = dialog+0xa38, rdx = MenuWindow*) -> dialog*` to build + register the
/// LIVE ProfileLoadDialog (vtable 0x142b229f8) into the active-screen set + menu group.
pub const LIVE_DIALOG_FACTORY_RVA: usize = TitleDialogRva::LiveDialogFactory as usize;

/// CONVERGED ACQUISITION RECIPE (2026-06-18, bd live-dialog-menuwindow-via-sceneproxy-backref-0x20):
/// the live MenuWindow* (factory rdx) is read DETERMINISTICALLY from the SceneProxy we already hold
/// at [td+0xa38] -- NOT via the menu MANAGER. CS::SceneObjProxy ctor 0x14074a700 does
/// `mov [proxy+0x20], rbx` where rbx is the MenuWindow (0x14074a735), so the back-ref lives at
/// proxy+0x20. The dead menu-manager/registry/menu-step scans (and the owner/dialog field scans) are
/// removed. CS::SceneObjProxy vtable 0x142a94a70 (RVA): require *(proxy) == base+this before reading
/// the +0x20 back-ref; LOG *(proxy) regardless (self-diagnostic).
pub const SCENE_OBJ_PROXY_VTABLE_RVA: usize = 0x2a94a70;

/// Generic CS::SceneObjProxy context/back-ref slot. The named-child constructor 0x14074a7c0
/// copies `[parent+0x20]` into `[proxy+0x20]` before binding the child by name into the proxy's
/// handle at +0x28. Used for the title `PressStart` / GFX `PRESS BUTTON` component gate.
pub const SCENE_OBJ_PROXY_CONTEXT_20_OFFSET: usize = 0x20;

/// TitleTopDialog embedded CS::SceneObjProxy for the title prompt component. Static evidence:
/// 05_000_title.gfx contains the visible text `PRESS BUTTON` and symbol `PressStart`; the
/// TitleTopDialog constructor xref at 0x1409a8275 calls the named-child proxy constructor with
/// rdx=dialog+0xb78 and r8="PressStart" (RVA 0x2b26500).
pub const TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET: usize = 0xb78;

/// Generic SceneObjProxy display visibility wrapper for a proxy (`dump 0x140733440 -> live/deobf
/// 0x140733340`). It resolves the proxy's Scaleform value and calls the GFx visibility setter; use
/// this for the 05_000_Title `PressStart` component rather than hiding the whole MenuWindowJob.
pub const TITLE_PRESS_START_SET_VISIBLE_RVA: usize = 0x733340;

pub static TITLE_PRESS_START_GFX_VALUE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

// ===== moved verbatim from crates/er-effects-rs/src/constants/stats_panel_text.rs =====

pub static TITLE_PRESS_START_GFX_HIDE_LAST_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_PRESS_START_GFX_HIDE_LAST_PROXY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_PRESS_START_GFX_HIDE_LAST_CONTEXT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_PRESS_START_GFX_HIDE_LAST_CALLER_PHASE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// Actual visible native title-logo layer. Static RE of `TitleTopDialog` (dump 0x1409a82d0 ->
/// live 0x1409a8180) shows `CS::TitleBackViewParts` embedded at dialog+0xaa8 and constructed from
/// the `05_001_Title_Logo` resource; this is distinct from the preserved `05_000_Title` MenuWindowJob.
pub const TITLE_LOGO_BACK_VIEW_PARTS_AA8_OFFSET: usize = 0xaa8;

pub const TITLE_LOGO_BACK_VIEW_PARTS_NAME: &str = "TitleBackViewParts";

pub const TITLE_LOGO_RESOURCE_NAME: &str = "05_001_Title_Logo";

pub const TITLE_LOGO_BACK_VIEW_PARTS_SET_VISIBLE_RVA: usize = 0x9a62c0;

pub static TITLE_LOGO_GFX_HIDE_LAST_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_LOGO_GFX_HIDE_LAST_LOGO: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_LOGO_GFX_HIDE_LAST_CALLER_PHASE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// FD4 StateMachine sub-object EMBEDDED at dialog+0xa60. NB: the registrar / set_state /
/// is_in_state receiver is the ADDRESS dialog+0xa60 (they do `add rcx,0xa60; call`), NOT
/// `*(dialog+0xa60)`. Its first qword is the SM vtable.
pub const TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, state_machine);

/// Byte latch at [dialog+0xa40]: 0 = menu not opened (the native non-input registrar path
/// requires it ==0), 1 = registrar ran. We READ it (never write/clear it -- pre-setting it
/// poisons the native non-input open path, bd titletopdialog-loop-ready-gate-2026).
pub const TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET: usize =
    core::mem::offset_of!(TitleTopDialogLayout, menu_opened);

/// Mask to extract the latch byte from an 8-byte read at dialog+0xa40.
pub const TITLE_TOP_DIALOG_LATCH_BYTE_MASK: usize = u8::MAX as usize;

/// CS FD4 `is_in_state(rcx = sm-receiver = dialog+0xa60, rdx = state descriptor ptr) -> bool`
/// (0x140749b20). Returns true iff the SM's CURRENT node is SETTLED (flags&0x8f>=2) AND its name
/// matches the descriptor's inline ASCII name. We call the game's own checker to read the live
/// state by NAME -- robust, no hand pointer-chase / SSO parsing.
pub const TITLE_TOP_DIALOG_IS_IN_STATE_RVA: usize = TitleDialogRva::IsInState as usize;

/// FD4 state name-descriptor RVAs (inline ASCII at the VA). FadeIn = the intro-fade node;
/// Loop = the settled press-prompt node (the correct gate to open the menu); TextFadeOut = the
/// menu-list-active node the registrar transitions to. bd titletopdialog-fadein-gate-...-2026.
pub const TITLE_STATE_DESC_FADEIN_RVA: usize = 0x2a90500;

pub const TITLE_STATE_DESC_LOOP_RVA: usize = 0x2a8f9e8;

pub const TITLE_STATE_DESC_TEXTFADEOUT_RVA: usize = 0x2b264f0;

/// Boolean-false byte returned by the game's `is_in_state` (compare `!= this` for true).
pub const OWN_STEPPER_FALSE: u8 = false as u8;

/// Initial value (0) for the open-menu registrar one-shot guard.
pub const OWN_STEPPER_MENU_OPENED_NO: usize = OWN_STEPPER_FALSE as usize;

pub static OWN_STEPPER_MENU_OPENED: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(OWN_STEPPER_MENU_OPENED_NO);

/// Sentinel logged when the inner TitleStep owner can no longer be found (the
/// title flow advanced past the title and the owner was finalized/destructed).
pub const TITLE_STATE_OWNER_GONE: i32 = -1;

/// STEP_GameStepWait (handler 0x140b0cde0) waits on the load job at owner+0x2e8:
/// `cmp dword [job+0xd8],0 / jne wait`. Observe job+0xd8 while holding here to
/// learn whether anything drains the job (needs a pump) or it is static.
pub const TITLE_STEP_GAME_STEP_WAIT: i32 = TitleStepState::GameStepWait as i32;

pub const TITLE_OWNER_JOB_OFFSET: usize = core::mem::offset_of!(TitleOwnerLayout, load_job);

pub const TITLE_JOB_OBSERVE_TICK_INTERVAL: u64 = 30;

pub const FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA: usize = er_save_loader::SET_SAVE_SLOT_RVA as usize;

/// Corrected play-game submit recipe (play-game-submit-and-continue-load-recipe-2026):
/// the Continue/Load handler 0x140b0e180 sets owner+0xbc to a PACKED MAP id, clears
/// the new-game flag owner+0x284, and calls SetState 0x140b0d960(owner, 5=PlayGame)
/// -- then the existing pump runs PlayGame -> child MoveMap_Init -> builds CSFeMan.
/// (force_play_game wrote owner+0x4c=5 raw + a raw slot in +0xbc, so it orphaned.)
pub const TITLE_SET_STATE_RVA: usize = 0xb0d960;

/// Private saved-map slot inside the GameMan block immediately after
/// `stay_in_multiplay_area_saved_rotation`; derive it from the adjacent typed
/// vector layout instead of retaining the raw absolute field offset.
pub const GAME_MAN_SAVED_MAP_C30_OFFSET: usize =
    core::mem::offset_of!(GameMan, stay_in_multiplay_area_saved_rotation)
        + core::mem::size_of::<F32Vector4>()
        + core::mem::size_of::<F32Vector4>();

/// Unnamed native Quit Game / return-title job-chain predicate field.
/// Ghidra labels this `GameMan::field143_0xbc4`; known writes are 1 -> 2 -> 3, and
/// the native wait predicate tests `== 3`. This is NOT a named enum until further RE proves one.
pub const GAME_MAN_RETURN_TITLE_JOB_PREDICATE_BC4_OFFSET: usize = 0xbc4;

/// Terminal value for `GameMan::field143_0xbc4` observed after the native return-title job tail.
/// Keep this value named as a predicate terminal, not a semantic enum state.
pub const GAME_MAN_RETURN_TITLE_JOB_PREDICATE_READY: usize = 3;

/// "Return-title requested, save not yet pumped" value for `GameMan::field143_0xbc4`: the native
/// return-title REQUEST (`FUN_14067a490`) sets bc4 = 1, then the quit-save pump advances it 1 -> 2 -> 3.
/// The switch-2 soft-lock is bc4 FROZEN at this value because the quit-save (`ShouldSave`) aborts on a
/// stale `CSMenuMan->disableSaveMenu` (see [`CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET`] in return_title.rs).
pub const GAME_MAN_RETURN_TITLE_JOB_PREDICATE_PENDING: usize = 1;

pub const INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET: usize = 0xe8;

// ===== moved verbatim from crates/er-effects-rs/src/constants/switch_liveness.rs =====

/// inputmgr keystate bitmap offset (inputmgr = [0x143d6b7b0]); bit0 = pressed-this-frame (edge).
pub const INPUTMGR_BITMAP_90_OFFSET: usize = 0x90;

pub const MENU_EVENT_PRESSED_BIT: u8 = true as u8;

pub const MENU_EVENT_CONFIRM_3D: usize = MenuEventId::Confirm as usize;

/// AUTO-CONFIRM (observe natural flow past the modal): tap Confirm on a SET/GAP cycle slow enough
/// that the connection-error modal (which appears ~90 frames after the press) gets its own tap.
pub const AUTO_CONFIRM_CYCLE_FRAMES: u64 = 120;

pub const AUTO_CONFIRM_SET_FRAMES: u64 = 3;

pub const AUTO_CONFIRM_LOG_INTERVAL: u64 = 60;

// ===== moved verbatim from crates/er-effects-rs/src/constants/system_quit.rs =====

/// XINPUT_GAMEPAD.wButtons D-pad Down bit (the menu "move down" gamepad input).
pub const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;

/// Settle the freshly-opened menu before injecting (poll-frames).
pub const INJECT_NAV_SETTLE_FRAMES: usize = 90;

/// Down asserted for this many consecutive poll-frames = one clean edge (one cursor step).
pub const INJECT_NAV_TAP_LEN: usize = 4;

/// Released gap between taps (edge re-arm; menu nav is edge-triggered, not auto-repeat).
pub const INJECT_NAV_GAP_LEN: usize = 16;

/// One tap+gap cycle length.
pub const INJECT_NAV_CYCLE: usize = INJECT_NAV_TAP_LEN + INJECT_NAV_GAP_LEN;

/// Number of Down taps to drive. The problem is fully deterministic: the cursor starts on
/// Continue (index 0) and Load Game is index 1, so EXACTLY ONE Down reaches it. There is no state
/// of knowledge that justifies more than one tap, so this is a literal 1 (not a tunable).
pub const INJECT_NAV_MAX_CYCLES: usize = 1;

/// Latched true once a load sustained >=MOVE_PROBE_REQUIRED_FRAMES consecutive frames of havok-position
/// motion under the injected stick (input-causes-movement PROVEN). Cleared when a new load epoch begins.
pub static CAN_MOVE_CONFIRMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// "result emitted / closing" latch, set =1 by EmitResult once the dialog begins teardown. We
/// stop calling OnDecide once this is set (avoids re-dispatch / UAF after teardown).
pub const MSGBOX_CLOSING_LATCH_3B0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, closing_latch);

pub const MSGBOX_CLOSING_YES: usize = true as usize;

pub const MSGBOX_LATCH_BYTE_MASK: usize = u8::MAX as usize;

/// Earliest game-task tick to fire the movie dismiss -- a settle floor; the real
/// gate is the movie singleton being present with the expected vtable. Kept modest
/// so the dismiss reliably fires within the runtime window.
pub const DISMISS_MIN_TICK: u64 = 120;

/// Generous upper bound on the game image span, to sanity-check that a candidate
/// object's vtable points into the module before dereferencing deeper.
/// Sentinel logged when GameMan is null so the field could not be read.
pub const ARM_PROBE_FIELD_ABSENT: i64 = -1;

/// PlayGame load-pair target block, bound to upstream `GameMan::move_map_target`
/// (audit-confirmed equal to the hand-decoded 0x14).
pub const FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET: usize =
    core::mem::offset_of!(GameMan, move_map_target);

/// SelectBot selection-injection lane (runs 300/301 static decode). The
/// SimpleTitleStep MenuLoop pump 0xb0a5e0 parses a serialized SelectBot stream
/// keyed by "CSEzSelectBot.MoveMapListStep" into owner+0x130 (parsed selection)
/// and submits a task onto owner+0x128 (title queue). The stream data lives in
/// the registry object pointed to by global [0x143d87360]. The pump's direct
/// PlayGame trigger 0xb0a78b is gated by byte [0x143d856a0] (load-active, which
/// the sole writer 0x140c8fe90 sets downstream of the load). This read-only
/// probe samples those fields to confirm the registry is live and the pump idles
/// with an empty stream before any write is attempted.
pub const SELECTBOT_OWNER_TITLE_QUEUE_128_OFFSET: usize = 0x128;

pub const SELECTBOT_OWNER_PARSED_SELECTION_130_OFFSET: usize = 0x130;

pub const SELECTBOT_REGISTRY_GLOBAL_RVA: usize = 0x3d87360;

pub const SELECTBOT_LOAD_GATE_RVA: usize = TITLE_ACCEPT_LATCH_RVA;

/// The MenuLoop pump 0xb0a5e0 sets `[input_manager+0x6b0]=1` near its entry
/// (`mov rax,[0x143d6b7b0]; mov byte [rax+0x6b0],1` at 0xb0a64d) every frame it
/// executes. Sampling this byte tells us whether the outer SimpleTitleStep is
/// actually running MenuLoop at the title idle (so SelectBot injection would be
/// parsed) or is still parked before it (so injection alone would be a no-op
/// until the title-accept advances the outer state).
pub const SELECTBOT_INPUT_MANAGER_GLOBAL_RVA: usize = er_game_base::rva::CS_MENU_MAN_GLOBAL_RVA;

pub const SELECTBOT_PUMP_RAN_FLAG_OFFSET: usize = 0x6b0;

/// Lever-1 title-accept experiment (runs 304+). Static RE (bd
/// `title-accept-lever-143d856a0`) shows inner MenuJobWait (state 10, 0xb0d400)
/// advances to state 11 (Finish) iff the global byte `[0x143d856a0]` (==
/// `SELECTBOT_LOAD_GATE_RVA`) is non-zero — it is the title-accept/"proceed"
/// latch, not a load-downstream flag. We set it ONCE, only while the inner owner
/// is confirmed at MenuJobWait, to drive the native title-accept with zero input,
/// then keep sampling to observe the cascade.
pub const TITLE_STEP_MENU_JOB_WAIT_STATE: i32 = TITLE_STEP_MENU_JOB_WAIT;

pub const TITLE_PROCEED_GATE_SET_VALUE: u8 = true as u8;

/// Global menu-accept byte 0x144589bdc (RVA 0x4589bdc): the decoded "a button was accepted"
/// flag the input pipeline sets on press, read via getter 0x140e85f50 from TitleTopDialog::update
/// (and 22 other menu accept-gates). When non-zero at the parked title, update runs the open-menu
/// registrar 0x1409b24e0 NATURALLY (build Continue/Load + transfer focus -> select-layer build) --
/// unlike a direct registrar self-fire which opened a competing dialog and reverted. Setting this
/// flag zero-input is the ToS-style "satisfy the accept side-effect" advance (NOT a synthesized
/// DInput/keystate/XInput event). bd title-global-accept-byte-144589bdc-zeroinput-advance-2026.
pub const TITLE_GLOBAL_ACCEPT_BYTE_RVA: usize = 0x4589bdc;

/// Menu-system manager singleton pointer global 0x143d5dea8 (89 refs). The title press-accept
/// handler 0x1409b1260 does `mov rax,[0x143d5dea8]; if rax: movb [rax],1; jmp registrar 0x1409b24e0`
/// -- it sets the singleton's +0 byte (a "menu-open in progress" flag) then opens the main menu
/// IN PLACE. Replicating this (set the flag, then registrar on the validated TitleTopDialog) is the
/// NARROW title-specific advance that should reach the main menu WITHOUT the language/ToS build that
/// the broad global accept byte over-triggers, and without the competing-dialog revert a bare
/// registrar self-fire caused. bd title-accept-to-registrar-narrow-path-143d5dea8-2026.
pub const TITLE_MENU_TRANSITION_SINGLETON_RVA: usize = 0x3d5dea8;

pub const TITLE_MENU_TRANSITION_FLAG_SET_VALUE: u8 = true as u8;

pub const INGAMESTEP_STEP_STATE_OFFSET: usize = 0x48;

pub const INGAMESTEP_NEXT_STATE_OFFSET: usize = 0x4c;

pub static TITLE_OWNER_PTR: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_OWNER_TRACE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);

pub static TITLE_NATIVE_JOB_CALLED: AtomicUsize = AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);

pub static TITLE_PROCEED_GATE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// One-shot latch for the global-accept-byte (0x144589bdc) zero-input title-advance lever.
pub static TITLE_ACCEPT_BYTE_GATE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub static NATIVE_AUTOLOAD_ARMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub static TITLE_OWNER_SCAN_COUNTDOWN: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_COUNTDOWN_READY);

// ===== moved verbatim from crates/er-effects-rs/src/experiments/mod/own_stepper_idx6_memory.rs =====

/// Pseudo-handle for the current process (GetCurrentProcess() is constant -1).
pub const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;

/// Bytes read per ReadProcessMemory call when scanning a region for the title
/// vtable. One syscall per 64KB chunk (then an in-process buffer scan) keeps the
/// fault-tolerant scan fast -- a syscall per 8-byte cursor would stall the thread.
pub const SCAN_CHUNK_SIZE: usize = 0x10000;

// ===== moved verbatim from crates/er-effects-rs/src/experiments/mod/product_core_own_stepper.rs =====

pub const PRODUCT_CORE_BLOCKER_UNSEEN: usize = 0;

pub const PRODUCT_CORE_BLOCKER_READY: usize = 1;

pub const PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER: usize = 2;

pub const PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE: usize = 3;

pub const PRODUCT_CORE_BLOCKER_TITLE_TABLE: usize = 4;

pub const PRODUCT_CORE_BLOCKER_SESSION: usize = 5;

pub const PRODUCT_CORE_BLOCKER_GAME_DATA_MAN: usize = 6;

pub const PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY: usize = 7;

pub const PRODUCT_CORE_BLOCKER_IODEV: usize = 8;

pub const PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR: usize = 9;

pub const PRODUCT_CORE_BLOCKER_TITLE_DIALOG: usize = 10;

pub const PRODUCT_CORE_BLOCKER_PRESS_START: usize = 11;

pub const PRODUCT_CORE_BLOCKER_TITLE_STATE: usize = 12;

pub const PRODUCT_CORE_BLOCKER_UNKNOWN: usize = 13;

pub static PRODUCT_CORE_LAST_OWNER: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_TITLE_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_TITLE_DIALOG_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_MENU_OPENED_LATCH: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_PRESS_START_PROXY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_PRESS_START_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_PRESS_START_CONTEXT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static PRODUCT_CORE_LAST_RETURN_TITLE_JOB_PREDICATE_BC4: AtomicUsize =
    AtomicUsize::new(usize::MAX);

pub static PRODUCT_CORE_LAST_PHASE: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PHASE_MENU);

pub static PRODUCT_CORE_LAST_BLOCKER: AtomicUsize = AtomicUsize::new(PRODUCT_CORE_BLOCKER_UNSEEN);

pub static TITLE_OWNER_SCAN_LAST_CANDIDATE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

pub static TITLE_OWNER_SCAN_LAST_TABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);

/// Max drain ticks (~ a generous loading-screen budget at 60fps) before giving up on the drain.
pub const TFC_DRAIN_TICK_CAP: usize = 4096;

/// Press-any-button node-update/builder RVA (deobf/live; prologue re-confirmed in the 0x1407adxxx
/// region, which is otherwise flagged unreliable). `__fastcall(rcx=step, rdx, r8[, r9])`.
pub const PAB_NODE_UPDATE_RVA: u32 = 0x7ad1c0;

/// The built press-any-button job within the node-update receiver: `[step+0x130]`.
pub const PAB_JOB_SLOT_130_OFFSET: usize = 0x130;

/// The job's completion press-count the predicate 0x1407a9200 reads (>=2 == complete).
pub const PAB_JOB_PRESS_COUNT_1E8_OFFSET: usize = 0x1e8;

/// The job's bound keycode (logged for identity validation + the documented fallback input bit).
pub const PAB_JOB_KEYCODE_180_OFFSET: usize = 0x180;

/// The "pressed" value the predicate treats as complete.
pub const PAB_PRESS_COUNT_SATISFIED: u32 = 2;

/// Upper sanity bound for a plausible press-count (reject garbage/unreadable reads -> keep waiting).
pub const PAB_COUNT_SANITY_MAX: u32 = 8;

/// Frames the press-any-button job must be built+valid before we advance (screen settle).
pub const PAB_ADVANCE_SETTLE_FRAMES: usize = 10;

/// Minimum plausible heap pointer (reject not-yet-built / garbage job slots).
pub const PAB_MIN_HEAP_PTR: usize = 0x10000;

#[derive(Clone, Copy)]
pub struct MenuActionNode {
    pub node: usize,
    pub node_vt: usize,
    pub registry: usize,
    pub member_dialog: usize,
    pub member_fn: usize,
    pub member_adjust: usize,
    pub window_item: usize,
}

#[derive(Clone, Copy)]
pub struct LiveDialogFireReady {
    pub title_dialog: usize,
    pub title_dialog_vt: usize,
    pub capture_slot: usize,
    pub capture: usize,
    pub capture_vt: usize,
    pub registry_vt: usize,
    pub menu_opened_latch: usize,
    pub menu_window: usize,
    pub menu_window_vt: usize,
}

#[derive(Clone, Copy)]
pub struct ProfileLoadDialogReady {
    pub dialog: usize,
    pub dvt: usize,
    pub bound: i32,
    pub cursor_now: i32,
    pub cursor_target: i32,
    pub expected_slot: i32,
    pub load_activate: usize,
    pub load_job_ctx: usize,
    pub load_job_ctx_vt: usize,
    pub player_game_data: usize,
}

#[derive(Clone, Copy)]
pub enum StartupModalBlockingState {
    Clear,
    Blocking {
        dialog: usize,
        vtable: usize,
        closing_latch: usize,
    },
}

pub struct ProductCoreAutoloadReady {
    pub committed: i32,
    pub requested: i32,
    pub table: usize,
    pub session: usize,
    pub game_data_man: usize,
    pub profile_summary: usize,
    pub iodev: usize,
    pub heap_allocator: usize,
    pub title_dialog: usize,
    pub title_in_loop: bool,
    pub title_in_textfadeout: bool,
    pub menu_opened_latch: usize,
    pub press_start_proxy: usize,
    pub press_start_context: usize,
}

pub struct TitlePressButtonComponent {
    pub proxy: usize,
    pub context: usize,
}

pub struct TitleDialogState {
    pub in_loop: bool,
    pub in_textfadeout: bool,
    pub menu_opened_latch: usize,
}

// ===== moved verbatim from crates/er-effects-rs/src/experiments/own_load/drive.rs =====

/// How often (in own_stepper frames) the OWN-LOAD world-stream stall telemetry emits a throttled
/// debug line. The oracle_* atomics are refreshed EVERY frame; only the human-readable log is
/// throttled so a probe log shows the trend without flooding.
pub const OWN_LOAD_STREAM_LOG_INTERVAL: u64 = 30;

// ===== wave 2: layout/enum types the wave-1 constants reference (moved verbatim) =====

// ----- from crates/er-effects-rs/src/constants/anti_debug.rs -----

#[repr(usize)]
pub enum TitleSessionRva {
    TitleOwnerVtable = 0x02b63bb0,
    SaveSafeBeginLogoSession = 0x4588e98,
    SessionA = 0x3d687a0,
    SessionB = 0x3d67bd0,
    MoveMapSession = 0x47ef360,
}

/// Partial SimpleTitleStep owner layout used by the zero-input title/menu driver.
/// Unknown byte arrays intentionally document unmodeled in-between fields while
/// keeping the offsets compiler-checked through `offset_of!`.
#[repr(C)]
pub struct TitleOwnerLayout {
    pub vtable: usize,
    pub unknown_08: [u8; 0x08],
    pub instance_table: usize,
    pub unknown_18: [u8; 0x30],
    pub committed_state: i32,
    pub requested_state: i32,
    pub unknown_50: [u8; 0x68],
    pub beginlogo_list_gate: u32,
    pub play_game_slot: i32,
    pub unknown_c0: [u8; 0x20],
    pub menu_holder: usize,
    pub unknown_e8: [u8; 0x48],
    pub menu_list: usize,
    pub unknown_138: [u8; 0x14c],
    pub new_game_flag: u8,
    pub unknown_285: [u8; 0x63],
    pub load_job: usize,
    pub unknown_2f0: [u8; 0xf1],
    pub play_game_request_flag: u8,
}

#[repr(usize)]
pub enum TraceSampleLimit {
    Value4 = 4,
    Value8 = 8,
    Value12 = 12,
    Value24 = 24,
    Value48 = 48,
    Value64 = 64,
}

#[repr(u32)]
pub enum MenuTraceRva {
    TaskEnqueue = 0x007a7b60,
    TaskUpdateWrapper = 0x0082a0f0,
    NewOrLoadWrapper = 0x0082ba80,
    ContinueWrapper = 0x0082bac0,
    MenuJobWait = 0x00b0d400,
    TaskUpdateTable = 0x02ac72a0,
}

#[repr(C)]
pub struct TitleNativeJobTaskData {
    pub unknown_00: [u8; 0x08],
    pub frame_delta: f32,
    pub unknown_0c: [u8; 0x04],
}

#[repr(u32)]
pub enum TitleNativeJobTiming {
    FrameRate = 60,
}

// ----- from crates/er-effects-rs/src/constants/autoload_state.rs -----

#[repr(C)]
pub struct GameManAutoloadFlagCluster {
    pub save_requested: u8,
    pub probe_b73: u8,
    pub probe_b74: u8,
    pub probe_b75: u8,
}

#[repr(C)]
pub struct MsgBoxDialogLayout {
    pub unknown_000: [u8; 0x1e8],
    pub job_result_state: i32,
    pub job_result_subcode: i32,
    pub unknown_1f0: [u8; 0x1c0],
    pub closing_latch: u8,
    pub unknown_3b1: [u8; 0x180f],
    pub confirm_latch: u8,
    pub unknown_1bc1: [u8; 0xa1f],
    pub default_cursor_index: i32,
    pub unknown_25e4: [u8; 0x04],
    pub button_count: i32,
}

// ----- from crates/er-effects-rs/src/constants/own_load_pump.rs -----

#[repr(usize)]
pub enum OwnStepperPhase {
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

// ----- from crates/er-effects-rs/src/constants/stage2_menu_drive.rs -----

#[repr(usize)]
pub enum ProfileLoadMenuRva {
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

/// Dialog vtable slot 20 (offset 0xa0) = load_activate 0x1409a4670. Read the live slot from
/// the dialog vtable (robust to relocation) rather than hard-calling the RVA.
#[repr(C)]
pub struct ProfileLoadDialogVtableLayout {
    pub unknown_slots_00_19: [usize; 20],
    pub load_activate: usize,
}

// ----- from crates/er-effects-rs/src/constants/stats_panel_background.rs -----

#[repr(i32)]
pub enum TitleStepState {
    Min = 0,
    BeginLogo = 2,
    BeginTitle = 3,
    /// STEP_BeginNewGame (idx4): fresh-character world entry; `SetState(4)` fired by the New Game
    /// confirm variants. RE 2026-07-07: one of the two world-load entry states.
    BeginNewGame = 4,
    PlayGame = 5,
    /// STEP_GameStepWait (idx6): the in-world state. `committed_was=6` in the SetState trace is a
    /// live in-world session; a native `SetState(owner, 2/BeginLogo)` from here is a revert-to-title.
    GameStepWait = 6,
    /// STEP_EndFlow (idx7) / STEP_EndFlowWait (idx8): the session teardown states that return the
    /// world to the title. The AUTOLOAD-HANDOFF parent-fix intercepts a premature 7/8 and forces it
    /// back to GameStepWait(6) so a just-loaded world is not returned to title. See
    /// `product_core_autoload_tick` and bd system-quit-load-profile-NOCRASH-milestone-2026-07-01.
    EndFlow = 7,
    EndFlowWait = 8,
    MenuJobWait = 10,
    Finish = 11,
}

/// STEP_EndFlow (7) / STEP_EndFlowWait (8): the session teardown -> title states. Named here so the
/// `product_core_autoload_tick` parent-fix references the enum instead of bare 7/8 literals.
pub const TITLE_STEP_END_FLOW: i32 = TitleStepState::EndFlow as i32;

pub const TITLE_STEP_END_FLOW_WAIT: i32 = TitleStepState::EndFlowWait as i32;

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
pub enum TitleDialogRva {
    IsInState = 0x749b20,
    LiveDialogFactory = 0x81ead0,
    Cleanup = 0x9a8890,
    OpenMenu = 0x9b24e0,
    Vtable = 0x2b26468,
    ActiveScreenArray = 0x3d6d8d0,
}

#[repr(C)]
pub struct ActiveScreenArrayLayout {
    pub slots: [usize; 10],
}

/// Partial TitleTopDialog layout for the menu-driver fields this crate reads.
#[repr(C)]
pub struct TitleTopDialogLayout {
    pub unknown_000: [u8; 0xa38],
    pub scene_proxy_capture: usize,
    pub menu_opened: u8,
    pub unknown_a41: [u8; 0x07],
    pub row_registry: usize,
    pub unknown_a50: [u8; 0x10],
    pub state_machine: usize,
}

// ----- from crates/er-effects-rs/src/constants/switch_liveness.rs -----

/// Front-end menu event ids (verified): Confirm/OK, and the two vertical-move candidates (one is
/// Down, one Up -- we inject both; only Down moves the cursor down, Up saturates at the top so it
/// is harmless from Continue). We do NOT inject Confirm (STAGE 2 invokes d180's functor instead).
#[repr(usize)]
pub enum MenuEventId {
    MoveA = 0x00,
    Confirm = 0x3d,
    MoveB = 0x45,
}
