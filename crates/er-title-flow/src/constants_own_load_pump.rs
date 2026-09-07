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
pub const LOADGAME_JOB_RUN_RVA: usize = 0x826e10;
/// `CS::MenuJobResult` size + layout (dump `/auto_structs/MenuJobResult` len 8): `+0x0 MenuJobState
/// state` (4 bytes), `+0x4 undefined4` (the inner deser sub-code 5/2/6 lands here via `param_2[1]`).
/// Pass a zero-init 8-byte buffer as `result`; read `state` (+0x0) for the done condition.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const MENUJOB_RESULT_SIZE: usize = 0x8;
pub const MENUJOB_RESULT_STATE_0_OFFSET: usize = 0x0;
pub const MENUJOB_RESULT_SUBCODE_4_OFFSET: usize = 0x4;
/// `CS::MenuJobState` enum (dump `/auto_structs/MenuJobState` len 4): Continue=1, Success=2, Failed=3.
/// `MenuJobResult::ShouldContinue` (0x1407a92f0) is exactly `Continue < state`, i.e. done == state>1.
pub const MENUJOB_STATE_CONTINUE: i32 = 1;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const MENUJOB_STATE_SUCCESS: i32 = 2;
pub const MENUJOB_STATE_FAILED: i32 = 3;
/// GameDataMan singleton global (.data abs `0x143d5df38`, == `CONTINUE_MANAGER_GLOBAL_RVA` deref base).
/// `GetMenuSystemSaveLoad() = GLOBAL_GameDataMan->menuSystemSaveLoad`, i.e. `mss = *(*(base+RVA)+0x60)`.
pub const GAME_DATA_MAN_GLOBAL_RVA: usize = er_game_base::rva::GAME_DATA_MAN_GLOBAL_RVA;
/// `GameDataMan->menuSystemSaveLoad` field offset (`mss = *(GameDataMan + 0x60)`).
pub const GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET: usize = 0x60;
/// LoadGame build factory REAL ctx args (golden Continue trace): `ctx_parent = mss + 0x50`,
/// `owner_ctx = *(mss + 0xa38)` (CS::TitleFlowContext). Non-null real ctx; the prior ctx=0 build AV'd
/// when the outer profile-selection sub-job dereffed the captured null.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub const MSS_CTX_PARENT_50_OFFSET: usize = 0x50;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub const MSS_OWNER_CTX_A38_OFFSET: usize = 0xa38;
/// CORRECTED LoadGame build ctx args (static RE 2026-06-22, triple-verified: golden factory site
/// `0x1409ac9cb mov 0xa38(%r13),%r9` where r13 == the live TitleTopDialog; TitleTopDialog ctor
/// `0x1409a82d0` populates +0xa38; live-SWBP capture `r9=owner+0x138 rdx=dialog+0x50`). The owner
/// context + parent come from the LIVE `CS::TitleTopDialog`, NOT from `CSMenuSystemSaveLoad`
/// (`mss+0xa38` was a red herring -- r13 was misidentified as mss). `ctx_parent = dialog+0x50`,
/// `owner_ctx = *(dialog+0xa38)` (a `CS::TitleFlowContext*`, written UNCONDITIONALLY by the dialog
/// ctor -> valid at the settled press-any-button title, unlike `mss+0xa38` which read back garbage).
/// bd `loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22`.
pub const DIALOG_CTX_PARENT_50_OFFSET: usize = 0x50;
/// CSMenuManImp -> menuData* at +0x8. Return-title final functor writes `menuData+0x5d = 1`.
pub const CSMENUMAN_MENU_DATA_08_OFFSET: usize = 0x8;
/// CSMenuMan menuData return-title request flag written by final functor `FUN_1407a3990`.
pub const CSMENUMAN_MENU_DATA_RETURN_TITLE_FLAG_5D_OFFSET: usize = 0x5d;
/// `CSMenuManImp+0x494`, set to 1 by `CS::CSLuaEventScriptImitation::RegistReturnTitle`
/// (1.16.2 `0x14059d8b0` -> 1.17 `0x14059e700`, paired on a 43B/21B-fixed signature and read back
/// to confirm the same body: `proxy->controlFlags |= 0x800`, `CSMenuMan+0x494 = 1`, `CSNetMan`
/// singleton check).
///
/// WHY THIS IS AN ORACLE AND NOT DECORATION (2026-09-04). `RegistReturnTitle` has SIX callers and
/// every one of them is a network-disconnect handler: `OnDisconnectEOSServer`,
/// `OnDisconnectGameServer`, `OnFailedGetBlockNum`, `OnLanCutError`, `OnNpServerSignOut`,
/// `OnSuspendResumeLanDisconnect`. So a 1 here means a disconnect forced a return to title, which
/// is a completely different cause from anything in our own switch/cover code -- and it is the
/// leading suspect for the black screen, where a SPURIOUS return-title request (InGameStep+0xd8
/// going 1 -> 2) appeared 17.2s after the world loaded with nothing in our code asking for it
/// (run br-20260904-165518-e3be). The flag is a pure read: no detour, no patched byte.
pub const CSMENUMAN_REGIST_RETURN_TITLE_FLAG_494_OFFSET: usize = 0x494;
/// Companion global flag written by the same return-title final functor (`DAT_143d6c5e8 = 1`).
pub const RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA: usize = 0x3d6c5e8;
/// `GLOBAL_CSRegulationManager` singleton pointer. Native corrupted-save branch `FUN_14082d090`
/// checks this for null before comparing `TitleFlowContext+0x148` against manager `+0x44`.
pub const GLOBAL_CS_REGULATION_MANAGER_RVA: usize = 0x3d86c58;
pub const TFC_REGULATION_VERSION_148_OFFSET: usize = 0x148;
pub const REGULATION_MANAGER_VERSION_44_OFFSET: usize = 0x44;
/// Native TFC regulation-version record helper: dump FUN_14082cbf0 -> deobf/live 0x14082cb00.
pub const TITLE_FLOW_CONTEXT_RECORD_REGULATION_VERSION_RVA: usize = 0x82cb00;
pub static TITLE_FLOW_CONTEXT_RECORD_REGULATION_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub use er_telemetry_core::counters::TITLE_FLOW_CONTEXT_RECORD_REGULATION_FIXUPS;
pub use er_telemetry_core::counters::TITLE_FLOW_CONTEXT_RECORD_REGULATION_INSTALLED;

/// Own-stepper phase progress is semantic. These values are wall-clock fail-safe caps only:
/// they abort to a no-write state if a native predicate never arrives, and must never be used as
/// success/readiness gates.
pub const OWN_STEPPER_MOUNT_POLL_TIMEOUT_MS: u64 = 10_000;
pub const OWN_STEPPER_DRIVE_TIMEOUT_MS: u64 = 10_000;
pub const OWN_STEPPER_MENU_BUILD_TIMEOUT_MS: u64 = 50_000;
pub const OWN_STEPPER_S2_PHASE_TIMEOUT_MS: u64 = 20_000;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const OWN_STEPPER_IDX6_SETTLE_TICKS: u64 = 120;
pub const CAP_SELECTOR_TICK_LOG_INTERVAL_TICKS: usize = 120;

pub const OWN_STEPPER_PHASE_CONTINUE: usize = OwnStepperPhase::Continue as usize;
/// PHASE 3 (MOUNT): mount the slot at state 10 BEFORE SetState(5) -- the only place the
/// MoveMapStep dispatcher (which resets b80 via its b80==1 lane) is NOT running, so our
/// own b80 poll can drive the save-IO machine 1->2->3 cleanly (minimal-save-mount-
/// primitive-recipe-2026). Register the FD4 stream worker (0x140b0a980 stub), initiate
/// the slot read (0x14067b4e0 -> b80=1), poll 0x140679180 until b80==3, then full
/// deserialize 0x14067b290 (c30 = real map + character applied), then SetState(5).
pub const OWN_STEPPER_PHASE_MOUNT: usize = OwnStepperPhase::Mount as usize;
/// b80 save-IO poll/driver 0x140679180(0,0): advances GameMan+0xb80 toward 3 (resident)
/// as the stream worker drains the async slot read; sets b80=3 when the IO request state
/// (0x14240a1f0) is resident. We call it ourselves each frame at state 10 -- but ONLY while
/// `GameMan.saveState` says a LOAD owns the device, because this poll answers 4 and writes
/// `saveState = 0` without releasing anything whenever a SAVE owns it instead.
pub const B80_POLL_RVA: usize = er_game_base::rva::SL_LOAD_POLL_WRAPPER_RVA;
/// Both fastcall args (cl, dl) to the b80 poll 0x140679180 are 0 in the native menu
/// drive (matches the captured real-load poll calls poll(0,0)).
pub const B80_POLL_ARG_ZERO: u8 = false as u8;
/// b80==1 PREVIEW-lane driver 0x140679510: per-frame IO tick of the preview read started by
/// 0x14067b4e0; resets GameMan+0xb80 1->0 when the iodev request goes resident. NOT a
/// dispatcher (no CSFeMan apply / no save write) -- just the lane tick the menu runs via
/// dispatcher-1. We call it ourselves to drain the preview read to resident.
pub const B80_LANE1_DRIVER_RVA: usize = er_game_base::rva::SL_SAVE_LANE_WRAPPER_RVA;
/// Wall-clock fail-safe to poll b80 toward 3 before giving up the mount (avoid an infinite
/// title hang if the worker never drains). Not a readiness/success predicate.
pub const OWN_STEPPER_MOUNT_POLL_MAX: u64 = OWN_STEPPER_MOUNT_POLL_TIMEOUT_MS;
pub static OWN_STEPPER_MOUNT_POLLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
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
pub const OWN_STEPPER_PHASE_DRIVE: usize = OwnStepperPhase::Drive as usize;
/// GameMan+0xc30 unset sentinel (0xffffffff as i32). At the bare press-any-button title
/// (BeginTitle skipped) c30 is unset; the full deserialize 0x14067b290 is the ONLY thing
/// that writes it to the slot's real saved map during the mount, so c30 != UNSET is the
/// genuine "the character was deserialized" signal (ac0 is NOT -- set_save_slot pre-sets it).
pub const GAME_MAN_C30_UNSET: i32 = !OWN_STEPPER_SLOT_ZERO;
/// MoveMapStep b80 dispatchers (called per-frame from the MoveMapStep update 0x140aff640
/// in native order: dispatcher-1 then dispatcher-2). Neither derefs the owner vtable;
/// both read only GameMan globals + the owner's deserialize-tracking fields (+0x12a skip-
/// apply, +0x12c slot, +0x130 result), so a zeroed synthetic owner >=0x138 bytes drives
/// them. dispatcher-1 = deserialize arm; dispatcher-2 = initiate (b78-route).
pub const B80_DISPATCHER1_RVA: usize = 0xafbad0;
pub const B80_DISPATCHER2_RVA: usize = 0xafb880;
/// Synthetic MoveMapStep `owner` for the b80 dispatchers. Zeroed; +0x12a=1 forces the
/// CSFeMan-apply arms (dispatcher-1 b80==1 idx1, dispatcher-2 @0x140afb9e4) to be SKIPPED
/// (they are gated owner+0x12a==0 and would deref the null-at-title CSFeMan 0x143d6b880);
/// +0x12c carries the deserialize slot. Size >0x138 (the highest field touched is +0x130).
pub const SYNTH_MMS_OWNER_SIZE: usize = 0x140;
pub const SYNTH_MMS_SKIP_APPLY_12A_OFFSET: usize = 0x12a;
pub const SYNTH_MMS_DESER_SLOT_12C_OFFSET: usize = 0x12c;
pub const SYNTH_MMS_SKIP_APPLY_ON: u8 = true as u8;
pub static mut SYNTH_MMS_OWNER: [u8; SYNTH_MMS_OWNER_SIZE] =
    [MOVIE_SKIP_FLAG_CLEAR; SYNTH_MMS_OWNER_SIZE];
/// Wall-clock fail-safe to drive the dispatchers before giving up (stay at title, no save write).
pub const OWN_STEPPER_DRIVE_MAX: u64 = OWN_STEPPER_DRIVE_TIMEOUT_MS;
pub static OWN_STEPPER_DRIVE_CALLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// PHASE 5 (MENU_BUILD): the parked press-any-button title is the FIRST state 10 and has
/// NOT run STEP_BeginTitle(3) yet, so the Continue/Load-Game items do not exist at
/// owner+0x138 until we drive 10->3 zero-input. idx10 SetState(owner,3) builds the main
/// menu (BeginTitle needs no session, writes NO save), then this phase waits for the menu
/// to populate and walks owner+0x138 to identify the Load-Game leaf (its +0xa8 action
/// functor's _Do_call chain resolves to dialog_factory 0x14081ead0). Max state reached =
/// main menu (no PlayGame) -> save-safe.
pub const OWN_STEPPER_PHASE_MENU_BUILD: usize = OwnStepperPhase::MenuBuild as usize;
/// Wall-clock fail-safe to wait for semantic menu-build predicates before giving up (stay at the
/// title, no save write). The intro/menu animation cadence varies by runtime, so this is a no-write
/// abort deadline only; readiness comes from native dialog/menu predicates.
pub const OWN_STEPPER_MENU_BUILD_WAIT_MAX: u64 = OWN_STEPPER_MENU_BUILD_TIMEOUT_MS;
/// Menu-build poll counter for diagnostics/log throttling, not a readiness gate.
pub static OWN_STEPPER_MENU_BUILD_WAITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// MenuWindowJob::Update 0x1407ad1c0 -- the native menu pump calls it with rcx = a
/// menu-item each tick. We hook it to CAPTURE the live Load-Game item (the one whose
/// +0xa8 action functor's _Do_call chain resolves to dialog_factory 0x14081ead0) without
/// guessing the CSMenu container layout the static walk could not penetrate. The captured
/// item pointer is stored in MENU_LOAD_GAME_ITEM for the own-stepper idx10 to read +
/// (Stage 2) invoke zero-input. 0 = not yet captured.
pub const MENU_ITEM_UPDATE_RVA: u32 = ProfileLoadMenuRva::MenuItemUpdate as u32;
pub static MENU_ITEM_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// MenuWindowJob constructor 0x1407ac8c0. Product autoload observes constructed items so the
/// semantic Continue item can be latched before the first updated/idle input leaf pollutes state.
pub const MENU_WINDOW_JOB_CTOR_RVA: u32 = 0x007ac8c0;
pub static MENU_WINDOW_JOB_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// MenuWindowJob native-accept constructor variant 0x1407acb00. Static xrefs show many menu
/// callers use this sibling constructor, and it installs the native accept predicate 0x1407ad810.
/// Hook passively so product telemetry can distinguish "no native-accept row was constructed" from
/// "we only hooked the wrong constructor variant".
pub const MENU_WINDOW_JOB_NATIVE_CTOR_B_RVA: u32 = 0x007acb00;
pub static MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// MenuWindowJob disabled/idle constructor 0x1407acf80. Product diagnostics observe this variant
/// because static RE shows it installs the constant-false accept predicate 0x1407add70 into the
/// same +0xf0 accept functor slot as the native-accept constructors.
pub const MENU_WINDOW_JOB_IDLE_CTOR_RVA: u32 = 0x007acf80;
pub static MENU_WINDOW_JOB_IDLE_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Tiny native title-row readiness predicate 0x140733150. The native title builder at
/// 0x1409b6730 calls this on `TitleTopDialog+0x2610`; false branches skip native MenuWindowJob
/// row construction entirely. The hook records the returned state object and `[obj+0x20] & 0x8f`
/// result so runtime artifacts can distinguish "waiting for native readiness" from a missing row.
pub const TITLE_NATIVE_READY_PREDICATE_RVA: u32 = 0x00733150;
pub static TITLE_NATIVE_READY_PREDICATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub static MENU_LOAD_GAME_ITEM: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// FD4 Sequence::Update / child-iterator 0x1407aa1f0 (RVA). At the opened main menu the
/// Load-Game leaf d180 is REGISTERED but does NOT tick (only the focused entry ticks via the
/// leaf Update 0x1407ad1c0, so the leaf hook misses d180). This iterator runs on every
/// Sequence node and dispatches its children at [seq+0x18 + i*8] (count [seq+0x60]); hooking
/// it lets us ENUMERATE every Sequence's children and capture d180 as a CHILD (functor ->
/// dialog_factory) regardless of focus -- the robust zero-input locator.
pub const SEQUENCE_ITER_RVA: u32 = 0x007aa1f0;
pub static SEQUENCE_ITER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Child-array offsets on an FD4 Sequence node (inline array at +0x18, count at +0x60,
/// stride 8). Sane child-count bound to skip non-Sequence nodes the hook also sees.
pub const SEQUENCE_CHILDREN_BASE_18_OFFSET: usize = 0x18;
pub const SEQUENCE_COUNT_60_OFFSET: usize = 0x60;
pub const SEQUENCE_CHILD_COUNT_MIN: usize = 1;
pub const SEQUENCE_CHILD_COUNT_MAX: usize = 64;
/// CS::MenuWindowJob vtable 0x142aa97e8 (RVA) -- the menu-item leaf class. Used to log only
/// real menu-item children when enumerating the opened-menu structure via the iterator hook.
pub const MENU_WINDOW_JOB_VTABLE_RVA: usize = 0x2aa97e8;
/// Diagnostic: log distinct MenuWindowJob children the Sequence iterator walks (with docall
/// chain) so one run reveals the full opened-menu entry set. Capped to avoid flooding.
pub use er_telemetry_core::counters::SEQ_ITER_CHILD_LOG_COUNT;
pub const SEQ_ITER_CHILD_LOG_MAX: usize = 240;
pub use er_telemetry_core::counters::SEQ_ITER_CHILD_LAST;
/// Unconditional structural dump of the first N Sequence-iterator calls (seq vtable, count,
/// child0 vtable) -- reveals what the iterator actually walks (Sequence vs MenuWindowJob,
/// real counts) regardless of the count-range gate, to diagnose why no menu-item child was
/// found. Capped.
pub use er_telemetry_core::counters::SEQ_ITER_DEBUG_COUNT;
pub const SEQ_ITER_DEBUG_MAX: usize = 80;
pub static MENU_ITEM_UPDATE_CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Cap on leaf-hook "distinct item ticked" log lines. A few title/menu items rotate every
/// frame, so without a cap this floods the size-capped trace and rolls early diagnostics off.
pub const MENU_ITEM_UPDATE_LOG_MAX: usize = TraceSampleLimit::Value48 as usize;
/// Last menu-item pointer we logged from the Update hook; we log only on change (the pump
/// ticks one item per frame, so this surfaces each distinct item as the user navigates,
/// without flooding the trace).
pub static MENU_ITEM_UPDATE_LAST: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The product autoload no longer gates startup on a fixed idx10 call count; it polls
/// `title_boot_ready` for the native owner/state/session/dialog predicates instead.
/// Shim callback object for the native Continue confirm 0x140b0e180 (reads
/// owner=[shim+8]). Persistent (not stack) so the call cannot read freed memory.
#[repr(C)]
pub struct OwnStepperShimLayout {
    pub unknown_00: usize,
    pub owner: usize,
    pub scratch: [usize; 6],
}

pub const OWN_STEPPER_SHIM_LEN: usize =
    core::mem::size_of::<OwnStepperShimLayout>() / core::mem::size_of::<usize>();
pub const OWN_STEPPER_SHIM_OWNER_IDX: usize =
    core::mem::offset_of!(OwnStepperShimLayout, owner) / core::mem::size_of::<usize>();
/// idx6 = STEP_GameStepWait func slot = table base + 6*0x10 = abs 0x143d715e0 (RVA
/// 0x3d715e0). We own it too, to drive the 3-phase load after the MoveMapStep builds.
pub const TITLE_STEP_IDX6_SLOT_RVA: usize = 0x3d715e0;
pub static OWN_STEPPER_ORIG_IDX6: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub static OWN_STEPPER_IDX6_CALLS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Path A re-target single-shot latch: 0 = the native b78-route deserialize has not yet
/// landed a real GameMan+0xc30, 1 = idx6 has already re-targeted owner+0xbc to the real
/// map + SetState(5). Prevents re-firing the re-target every frame.
pub static OWN_STEPPER_RETARGETED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_RETARGET_NO);
pub const OWN_STEPPER_RETARGET_NO: usize = false as usize;
pub const OWN_STEPPER_RETARGET_YES: usize = true as usize;
/// GameMan+0xb80 load-phase value meaning the save IO is resident (mounted).
#[repr(i32)]
pub enum OwnStepperB80State {
    Idle,
    PreviewLane,
    #[allow(dead_code)]
    // Retained RE constant: no live reader today, kept with the table it was decoded into.
    Active,
    Resident,
}

pub const OWN_STEPPER_B80_RESIDENT: i32 = OwnStepperB80State::Resident as i32;
/// GameMan+0xb80 == 1: the PREVIEW lane (0x14067b4e0 read in flight); drive the lane tick
/// 0x140679510 to drain it to resident (which resets b80 -> 0).
pub const OWN_STEPPER_B80_PREVIEW_LANE: i32 = OwnStepperB80State::PreviewLane as i32;
/// GameMan+0xb80 == 0: idle/drained; fire the LoadSaveData initiator 0x14067b200 -> b80=2
/// (reusing the resident iodev request the preview started).
pub const OWN_STEPPER_B80_IDLE: i32 = OwnStepperB80State::Idle as i32;
/// idx6 calls to wait (MoveMapStep settle) before deserializing the real slot.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const OWN_STEPPER_IDX6_SETTLE: u64 = OWN_STEPPER_IDX6_SETTLE_TICKS;
pub static mut OWN_STEPPER_SHIM: [usize; OWN_STEPPER_SHIM_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; OWN_STEPPER_SHIM_LEN];
/// 2026-06-18 DIRECT BUILD: persistent buffers for building a ProfileLoadDialog directly via
/// dialog_factory 0x14081ead0 (bypassing the input-gated router_this/d180-on-confirm layer that
/// never builds headless). `cap[0]` = owner+0x138 (the ctor r8 = *(capture+8)); the factory reads
/// *(rcx). `ctx` is the zeroed incoming-ctx the factory reads to build the (empty cosmetic) label.
/// Both PERSISTENT (the built dialog retains a pointer to the ctx), so static, never stack.
pub const DIRECT_BUILD_CAP_LEN: usize = 1;
pub static mut DIRECT_BUILD_CAP: [usize; DIRECT_BUILD_CAP_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; DIRECT_BUILD_CAP_LEN];
pub const DIRECT_BUILD_CTX_LEN: usize = 8;
pub static mut DIRECT_BUILD_CTX: [usize; DIRECT_BUILD_CTX_LEN] =
    [TITLE_OWNER_SCAN_START_ADDRESS; DIRECT_BUILD_CTX_LEN];
/// One-shot latch so the native build fires at most once per run.
pub static OWN_STEPPER_DIRECT_BUILT: AtomicUsize = AtomicUsize::new(OWN_STEPPER_DIRECT_BUILT_NO);
pub const OWN_STEPPER_DIRECT_BUILT_NO: usize = false as usize;
pub const OWN_STEPPER_DIRECT_BUILT_YES: usize = true as usize;
/// MODEL B (live_dialog_enabled): one-shot latch so the live Load-Game node's native run
/// 0x1409aaba0 fires at most once per run (a re-fire would double-build / leak the dialog).
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub static OWN_STEPPER_LIVE_FIRED: AtomicUsize = AtomicUsize::new(OWN_STEPPER_LIVE_FIRED_NO);
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const OWN_STEPPER_LIVE_FIRED_NO: usize = false as usize;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub const OWN_STEPPER_LIVE_FIRED_YES: usize = true as usize;
