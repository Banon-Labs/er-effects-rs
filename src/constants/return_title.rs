// ---- Return-title "rebuild the title" request flags set by the final functor (0x7a3900) ----
// The functor does `*(*([GLOBAL_CSMenuMan]+0x8)+0x5d)=1` and `*(0x143d6c5e8)=1`. These are LEVEL
// flags (not edge-consumed); we set them to tear down the OLD char for the switch, but nothing
// resets them, so once the reloaded character's world comes up the still-set +0x5d re-requests the
// quit-to-title -> GameMan.save_requested flips true again (~3.6s post-load, proven by the gm-snap
// trace) -> a second save + SetState(2) bounces the freshly-loaded world back to the title. We clear
// both once the reload commits (continue_confirm), which is after the teardown they were needed for.
/// (`CS_MENU_MAN_GLOBAL_RVA` = `[GLOBAL_CSMenuMan]` pointer global is already defined above.)
/// `CSMenuManImp::menuData` pointer at CSMenuMan+0x8.
pub(crate) const CS_MENU_MAN_MENU_DATA_OFFSET: usize = 0x8;
/// The "return-to-title / menu-rebuild requested" byte at menuData+0x5d.
pub(crate) const CS_MENU_DATA_RETURN_TITLE_REQUEST_5D_OFFSET: usize = 0x5d;
/// `DAT_143d6c5e8` companion rebuild flag (data RVA). No readers found in the dump, but cleared for
/// symmetry so we fully undo what the final functor set.
pub(crate) const RETURN_TITLE_REBUILD_FLAG_DAT_RVA: usize = 0x3d6c5e8;
// ---- In-game session liveness gate (the post-reload bounce decision, static RE 2026-07-02) ----
// TitleStep state 6 (STEP_GameStepWait, dump 0x140b0ced0) exits to the quit-to-title transition
// (SetState(2) -> BeginLogo -> BeginTitle -> MenuJobWait) the first tick it sees
// `InGameStep->requestCode == 0`. The request-code register (InGameStep+0xd8, int) lifecycle:
// ctor=0; RequestMoveMap (dump 0x140aebeb0, called by STEP_PlayGame for the initial world load with
// the map from TitleStep+0xbc) =1; STEP_MoveMap_Update (dump 0x140aec810) =2 when the map move's
// child MoveMapStep finishes; STEP_RequestWait (dump 0x140aecd00) at ==2 waits for the in-game menu
// job qword at CSMenuMan+0x798 to be nonzero -- while it IS nonzero the session idles at code 2
// (the stable in-world state); if that qword reads 0 it writes the request code to 0, which is what
// STEP_GameStepWait converts into the return-to-title. So a reloaded world only STAYS up if
// CSMenuMan+0x798 is (re)populated after the load.
/// `TitleStep::InGameStep` pointer (TitleStep+0x2e8, read by STEP_GameStepWait at dump 0x140b0cee2).
pub(crate) const TITLE_STEP_IN_GAME_STEP_2E8_OFFSET: usize = 0x2e8;
/// `InGameStep` request-code register (+0xd8): 0=end session, 1=move-map pending, 2=move done /
/// stable in-world idle (see block comment above).
pub(crate) const IN_GAME_STEP_REQUEST_CODE_D8_OFFSET: usize = 0xd8;
/// In-game menu job pointer at CSMenuMan+0x798 (unnamed in fromsoftware-rs `unk748`); nonzero while
/// the in-game session's menu job lives. STEP_RequestWait ends the session when it reads 0 at
/// request code 2.
pub(crate) const CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET: usize = 0x798;
/// `CS::EzChildStepBase::RequestFinish` (dump `0x140eb5590` -> live `0x140eb5570`, shift -0x20,
/// content-unique). One-shot: calls the wrapper's CSSetFinishHelper virtual (which sets the child
/// step's finish-requested byte at child+0xb4) then latches wrapper+0x10. The quit-to-title
/// teardown ends the in-world MoveMapStep session through here; the post-switch reload bounce is
/// this firing against the FRESH MoveMapStep child right after streaming completes. Read-only
/// trace hook logs every call + caller RVA to identify the stale requester.
pub(crate) const EZ_CHILD_STEP_REQUEST_FINISH_RVA: u32 = 0xeb5570;
/// `EzChildStep<MoveMapStep>` wrapper offset inside `InGameStep` (ctor dump 0x140aeabf3).
pub(crate) const IN_GAME_STEP_MOVE_MAP_WRAPPER_E0_OFFSET: usize = 0xe0;
/// `EzChildStep<InGameStayStep>` wrapper offset inside `InGameStep` (ctor dump 0x140aeabc3).
pub(crate) const IN_GAME_STEP_STAY_WRAPPER_B8_OFFSET: usize = 0xb8;
/// `EzChildStepBase::stepper` (the owned child step object) at wrapper+0x8; the finish latch byte
/// is wrapper+0x10 and the CSSetFinishHelper pointer wrapper+0x18 (dump 0x140eb5590 decompile).
pub(crate) const EZ_CHILD_STEP_STEPPER_OFFSET: usize = 0x8;
pub(crate) static SYSTEM_QUIT_CHILD_FINISH_TRACE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_CHILD_FINISH_TRACE_INSTALLED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_CHILD_FINISH_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Native builder for a MenuJob wrapping the final return-title functor (`FUN_14079f780` dump ->
/// live/deobf `0x14079f690`). Submit this job through the native queue so the flag transition happens
/// in menu-pump ownership, not from our game-task thread.
pub(crate) const SYSTEM_QUIT_RETURN_TITLE_FINAL_JOB_BUILDER_RVA: u32 = 0x79f690;
pub(crate) static SYSTEM_QUIT_RETURN_TITLE_FINAL_FUNCTOR_CALL_COUNT: AtomicUsize =
    AtomicUsize::new(0);
/// Count of quick-load handoffs that invoked the original native Quit Game row action trampoline
/// instead of the low-level accepted callback alone. This is an experiment to test whether the full
/// native return-title menu-job chain is the missing teardown boundary.
pub(crate) static SYSTEM_QUIT_QUICKLOAD_NATIVE_QUIT_ACTION_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_SUBMIT_COUNT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_READY_BLOCK_COUNT: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_DIALOG: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DIRECT_RETURN_TITLE_CHAIN_LAST_QUEUE_READY: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_QUICKLOAD_TITLE_OWNER_SEEN_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_QUICKLOAD_AUTOLOAD_HANDOFF_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_QUICKLOAD_LAST_TITLE_OWNER: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_DIALOG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_CURSOR: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_PROFILE_LOAD_ACTIVATE_LAST_BOUND: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_ARMED_LIST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// Original System dialog saved for the post-ProfileSelect quickload return-title chain.
/// Unlike SYSTEM_QUIT_TOP_HIDE_ARMED_DIALOG, this must survive the ProfileSelect append observer reset.
pub(crate) static SYSTEM_QUIT_QUICKLOAD_RETURN_CHAIN_SYSTEM_DIALOG: AtomicUsize =
    AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_TOP_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_PROFILE_WINDOW: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_LIST: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_TOP_MENU_ID: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static SYSTEM_QUIT_TOP_HIDE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_TOP_RESTORE_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `PropertyEditDialog`/System dialog embedded `SceneObjProxy` used by the Quit tab builder for child binds.
pub(crate) const SYSTEM_QUIT_DIALOG_SCENE_PROXY_1200_OFFSET: usize = 0x1200;
pub(crate) static SYSTEM_QUIT_DUPLICATE_LAST_COUNT_BEFORE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SYSTEM_QUIT_DUPLICATE_LAST_COUNT_AFTER: AtomicUsize = AtomicUsize::new(0);
pub(crate) static START_SYSTEM_QUIT_DUPLICATE_BUTTON_HOOK: Once = Once::new();
/// One-shot spawn guard for the save-source redirect hook install (CreateFileW/CopyFileW path
/// redirect). Armed at process attach only when `enforce_save_override_or_abort` resolved a valid
/// env save source (Redirect mode); see save-override-no-default-fallback-mandatory-env-2026-06-23.
pub(crate) static START_SAVE_REDIRECT: Once = Once::new();
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
// Startup modal handling is lifecycle-driven by `startup_modal_blocking_state`, not by a fixed
// grace window.
