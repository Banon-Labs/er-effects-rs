use super::*;

pub(crate) const PRODUCT_CORE_BLOCKER_UNSEEN: usize = 0;
pub(crate) const PRODUCT_CORE_BLOCKER_READY: usize = 1;
pub(crate) const PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER: usize = 2;
pub(crate) const PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE: usize = 3;
pub(crate) const PRODUCT_CORE_BLOCKER_TITLE_TABLE: usize = 4;
pub(crate) const PRODUCT_CORE_BLOCKER_SESSION: usize = 5;
pub(crate) const PRODUCT_CORE_BLOCKER_GAME_DATA_MAN: usize = 6;
pub(crate) const PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY: usize = 7;
pub(crate) const PRODUCT_CORE_BLOCKER_IODEV: usize = 8;
pub(crate) const PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR: usize = 9;
pub(crate) const PRODUCT_CORE_BLOCKER_TITLE_DIALOG: usize = 10;
pub(crate) const PRODUCT_CORE_BLOCKER_PRESS_START: usize = 11;
pub(crate) const PRODUCT_CORE_BLOCKER_TITLE_STATE: usize = 12;
pub(crate) const PRODUCT_CORE_BLOCKER_UNKNOWN: usize = 13;

pub(crate) use er_telemetry::counters::COLD_CHAR_MOUNT_FILE_ARMED;
/// Module-level mirror of cold_char_mount_drive's internal MOUNT_PHASE, stored as `phase + 1`
/// (0 = the cold mount never ran; 5 = PHASE_DONE = terminal, evidence collected). Exposed in
/// telemetry as `oracle_cold_char_mount_phase` so the readiness watcher can tear the game down the
/// instant the b80 outcome is observed instead of idling to the wall-clock cap.
pub(crate) use er_telemetry::counters::COLD_CHAR_MOUNT_PHASE_PUB;
/// Armed from the reliable autoload-file channel (`own_dispatch=1` in er-effects-autoload.txt) so the
/// OWN-LOAD m28 direct-enqueue lever (`AddDefaultFileLoadProcess`) runs without depending on env-var
/// propagation through Proton. Defaults OFF; the lever ALSO requires `OWN_LOAD_CONTINUE_FIRED` at fire
/// time, so arming this alone cannot dispatch on a vanilla native menu load. Touches only world-asset
/// file-load streaming -- no save IO, cannot autosave.
pub(crate) use er_telemetry::counters::OWN_DISPATCH_FILE_ARMED;
/// Armed from the reliable autoload-file channel (`own_load_continue=1` in er-effects-autoload.txt)
/// so the FINAL guarded `continue_confirm`/`SetState5` world-stream step (after the verify-only
/// `own_load_drive` parse) runs without depending on env-var propagation through Proton.
/// SAVE-WRITING when it fires -- gated hard on a REAL c30 + char fingerprint inside `own_load_drive`.
pub(crate) use er_telemetry::counters::OWN_LOAD_CONTINUE_FILE_ARMED;
/// Armed from the reliable autoload-file channel (`own_load=1` in er-effects-autoload.txt) so the
/// SAVE-SAFE verify-only OWN-LOAD buffer-feed probe (`own_load_drive`) runs without depending on
/// env-var propagation through Proton.
pub(crate) use er_telemetry::counters::OWN_LOAD_FILE_ARMED;
/// Armed from the reliable autoload-file channel (`own_load_install_job=1` in er-effects-autoload.txt)
/// so the menu-free LoadGame-JOB INSTALL lever runs without depending on env-var propagation through
/// Proton. Defaults OFF. When armed (and `own_load` is armed so `own_load_drive` runs), the verify-only
/// parse is followed by BUILD (`FUN_140826510`) + INSTALL (`FUN_1407a9560`) of the LoadGame
/// MenuJobWithContext into `owner+0x130` -- INSTEAD of the guarded continue_confirm/SetState5. SAVE-SAFE
/// (build + first-tick deser only READ the save; no SetState5, no autosave, no save write).
pub(crate) use er_telemetry::counters::OWN_LOAD_INSTALL_JOB_FILE_ARMED;
/// Monotonic count of LoadGame-JOB install-lever fires (build + install into owner+0x130). Exposed in
/// telemetry as `oracle_own_load_install_job_fired` so a probe can confirm the lever actually ran.
pub(crate) use er_telemetry::counters::OWN_LOAD_INSTALL_JOB_FIRED;
/// Module-level mirror of `own_load_drive`'s internal phase, stored as `phase + 1` (0 = the probe
/// never ran; PHASE_DONE+1 = terminal, evidence collected). Exposed in telemetry as
/// `oracle_own_load_phase` so the readiness watcher can tear the game down the instant the verify
/// outcome is observed instead of idling to the wall-clock cap.
pub(crate) use er_telemetry::counters::OWN_LOAD_PHASE_PUB;
/// Armed from the reliable autoload-file channel (`own_stepper=1` / `cold_char_mount=1` in
/// er-effects-autoload.txt) so the menu-free own-stepper + cold-char-mount paths can be enabled
/// without depending on env-var propagation through Proton or game_directory_path() trigger files.
pub(crate) use er_telemetry::counters::OWN_STEPPER_FILE_ARMED;
pub(crate) use er_telemetry::counters::PRODUCT_AUTOLOAD_ARMED;
/// Sentinel for an unreadable / not-yet-sampled world-load telemetry field (distinguishes
/// "the chain pointer was null / RPM faulted" from a genuine 0). Chosen well outside any real
/// state/count value so the readiness watcher and the agent can tell "frozen at a real value"
/// from "never sampled".
pub(crate) const OWN_LOAD_STREAM_FIELD_UNREAD: i64 = i64::MIN;
/// Per-frame OWN-LOAD world-stream stall telemetry (own-load-reaches-loading-screen-2026-06-22 /
/// full-pipeline-traced-to-worldreswait-map-block-streaming). After own_load_continue fires the
/// guarded continue_confirm/SetState5, the engine reaches the real-char LOADING SCREEN but STALLS
/// (player never spawns). These mirror the deepest world-load pump values each frame so a probe log
/// shows whether ANY of them ADVANCE over time (progress) vs are FROZEN (genuine stall). All are
/// pure fault-tolerant reads (safe_read_*); they NEVER change load behavior.
/// Title owner committed/live state field (owner+0x48, == TITLE_OWNER_STATE_COMMITTED_OFFSET). 5 ==
/// PlayGame/streaming after SetState5.
pub(crate) static OWN_LOAD_STREAM_OWNER_STATE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// Title owner requested/next state field (owner+0x4c, == TITLE_OWNER_STATE_OFFSET; the value the
/// continue_confirm disasm context writes). Logged alongside +0x48 to disambiguate committed vs next.
pub(crate) static OWN_LOAD_STREAM_OWNER_REQ_STATE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// MoveMapStep step-machine state (mms_state) = [[InGameStep(owner+0x2e8)+0xe8]+0x48]. The known
/// stall floor is step 3 = STEP_WorldResWait. UNREAD if the InGameStep/MoveMapStep chain is null
/// (e.g. before SetState5 builds it). This is the KEY world-load pump state.
pub(crate) static OWN_LOAD_STREAM_MMS_STATE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// Loaded-block count read by STEP_WorldResWait residency: [[[MoveMapStep+0xf0]+0x10]+0xb3140].
/// 0 == no map-block registered yet (setup gap); >0 == streaming in progress (the count/phase
/// is the real progress signal). UNREAD if the resmgr chain is null.
pub(crate) static OWN_LOAD_STREAM_BLOCK_COUNT: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// The world coord/map-id MoveMapStep requests in STEP_WorldResWait ([[MoveMapStep+0xf0]+0x2c]).
/// byte3 == 0x0a means slot 9's m10 is being requested (loader/streaming issue); 0 means the saved
/// world position never loaded (coord issue). UNREAD if the chain is null.
pub(crate) static OWN_LOAD_STREAM_REQ_COORD: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// IO device in-flight word [iodev+0x10]. Non-zero == a save/world read is pending in the iodev.
/// At the observed stall this was 0 (iodev idle -> the stall is NOT in save-IO we bypassed).
pub(crate) static OWN_LOAD_STREAM_IO_INFLIGHT: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// IO device started-request handle [iodev+0x20]. Pairs with +0x18 as a *started* async-IO read.
pub(crate) static OWN_LOAD_STREAM_IO_REQHANDLE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// GameMan+0xc30 saved-map id (the streamed map). Real (e.g. 0x1c000000) after a successful mount.
pub(crate) static OWN_LOAD_STREAM_C30: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// Monotonic count of frames the per-frame stall telemetry has sampled (since own_load armed). Pairs
/// with the values above: if frames climb but every value is frozen, that is a genuine stall.
pub(crate) use er_telemetry::counters::OWN_LOAD_STREAM_FRAMES;
/// Whether the local player (WorldChrMan/PlayerIns) has resolved during the world-stream observe
/// window. 1 == present (the world spawned), 0 == absent (still on the loading screen), UNREAD
/// (i64::MIN) == not yet observed. The recurring observer publishes this so a probe can see the
/// loading screen -> spawn transition (or its absence) alongside mms_state/block_count.
pub(crate) static OWN_LOAD_STREAM_PLAYER_PRESENT: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// InGameStep+0xd8 pending phase byte, read PURELY by the recurring observer (no call). Together with
/// the requested BlockId below it discriminates whether play_game_submit's handoff ran. UNREAD if the
/// InGameStep handle is null. (own-load-worldreswait-is-block-registration-not-coord-2026-06-22)
pub(crate) static OWN_LOAD_STREAM_INGAME_PHASE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// InGameStep+0x100 requested BlockId (u32), read PURELY. == the saved BlockId (e.g. 0x1c000000) when
/// play_game_submit primed the request; 0/unset when it did not. UNREAD if InGameStep is null.
pub(crate) static OWN_LOAD_STREAM_REQ_BLOCKID: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// 1 == a block whose areaId equals the coord-derived target area (e.g. 0x1c for m28) is REGISTERED
/// in [resmgr+0xb3030]; 0 == absent (registration gap). UNREAD if the resmgr/scan chain is null. The
/// presence/absence of this block is THE discriminator (registration gap vs streaming gap).
pub(crate) static OWN_LOAD_STREAM_TARGET_BLOCK_PRESENT: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// Set true the instant `own_load_continue_fire` returns from the native continue_confirm (SetState5
/// started the title->ingame transition). The RECURRING game task gates its world-stream observer on
/// this flag so it keeps logging THROUGH the loading screen -- own_stepper_idx10 (a TITLE-PHASE task)
/// STOPS ticking once SetState5 starts the transition, so the observer must live in the per-frame
/// game task instead. (own-load-stream-observer-must-be-recurring-task-2026-06-22)
pub(crate) static OWN_LOAD_CONTINUE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// InGameStep = *(owner+TITLE_OWNER_JOB_OFFSET), cached at fire time. It was already non-null at
/// frame 0 (observed 0x7fff21e09a40) so caching it then captures a stable handle the recurring
/// observer can walk to MoveMapStep even after the title task stops running. 0 == not cached.
pub(crate) use er_telemetry::counters::OWN_LOAD_INGAMESTEP_CACHED;
/// The SetState-able TITLE owner threaded into continue_confirm, cached at fire time so the recurring
/// observer reads the world-stream from the SAME object the load was kicked on (NOT a fresh
/// own_stepper owner, which stops being supplied once the title task dies). 0 == not cached.
pub(crate) use er_telemetry::counters::OWN_LOAD_OWNER_CACHED;
/// PATH B (own_load_pump). Armed from the reliable autoload-file channel (`own_load_pump=1` in
/// er-effects-autoload.txt). Defaults OFF. When armed (and `own_load` is armed so `own_load_drive`
/// runs the verify-only parse), the parse is followed by BUILD of the LoadGame `MenuJobWithContext`
/// with REAL mss-derived ctx; the job ptr is then PRIVATELY pumped (its `Run` ticked every frame from
/// the recurring game task) to completion -- WITHOUT installing into owner+0x130 / any queue / the
/// CSMenuMan dialog stack. After the pumped job reaches `state==Success`, the guarded SetState5
/// transition fires ONCE to drive title->ingame. Takes precedence over own_load_install_job /
/// own_load_continue. (autoload-world-load-coupled-to-csmenuman-dialog-verdict-2026-06-22)
pub(crate) use er_telemetry::counters::OWN_LOAD_PUMP_FILE_ARMED;
/// The built LoadGame job pointer the recurring task pumps each frame. 0 == not built / not armed.
/// Set once by `own_load_pump_fire`; read+ticked by the recurring observer's sibling pump.
pub(crate) use er_telemetry::counters::OWN_LOAD_PUMP_JOB;
/// Monotonic frame counter for the RECURRING world-stream observer (advances every game-task frame
/// the observer is active). Distinct from OWN_LOAD_STREAM_FRAMES (which the old own_stepper-sited
/// telemetry also bumps): this is the "frame=N" the recurring observer's debug line prints so the
/// trend across the loading screen is visible.
pub(crate) use er_telemetry::counters::OWN_LOAD_STREAM_RECUR_FRAMES;
/// The MenuJobState the last `Run` pump returned (result+0x0): 1=Continue (still working), 2=Success
/// (done OK), 3=Failed. `i64::MIN` (UNREAD) before the first pump. Exposed as `oracle_own_load_pump_state`.
pub(crate) static OWN_LOAD_PUMP_STATE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// The inner deser sub-code the last pump observed (result+0x4): 5/2/6 from the deser step. UNREAD before
/// the first pump. Exposed as `oracle_own_load_pump_subcode` for the 5/2/6 streaming-stage discriminator.
pub(crate) static OWN_LOAD_PUMP_SUBCODE: std::sync::atomic::AtomicI64 =
    std::sync::atomic::AtomicI64::new(OWN_LOAD_STREAM_FIELD_UNREAD);
/// Monotonic count of `Run` pumps fired (each frame the job is ticked). 0 == the pump never ran.
/// Exposed as `oracle_own_load_pump_fired` so a probe can confirm the per-frame pump is actually ticking.
pub(crate) use er_telemetry::counters::OWN_LOAD_PUMP_FIRED;
/// Set true once the pumped job reached a terminal state (Success/Failed) AND the one-shot transition
/// was handled, so we never re-pump or re-transition. Exposed as `oracle_own_load_pump_done`.
pub(crate) static OWN_LOAD_PUMP_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) use er_telemetry::counters::PRODUCT_CORE_AUTOLOAD_TICKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_CALLSITE_BASE_OK_TICKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_CALLSITE_LAST_SLOT;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_CALLSITE_SLOT_OK_TICKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_CALLSITE_TICKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_OWNER_TICKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_READY_BLOCKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_READY_SUCCESSES;
pub(crate) static PRODUCT_CORE_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_TITLE_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_TITLE_DIALOG_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::PRODUCT_CORE_LAST_TITLE_IN_LOOP;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT;
pub(crate) static PRODUCT_CORE_LAST_MENU_OPENED_LATCH: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PRESS_START_PROXY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PRESS_START_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PRESS_START_CONTEXT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_RETURN_TITLE_JOB_PREDICATE_BC4: AtomicUsize =
    AtomicUsize::new(usize::MAX);
pub(crate) static PRODUCT_CORE_LAST_PHASE: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PHASE_MENU);
pub(crate) static PRODUCT_CORE_LAST_BLOCKER: AtomicUsize =
    AtomicUsize::new(PRODUCT_CORE_BLOCKER_UNSEEN);
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_ATTEMPTS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_STATE_REJECTS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_TABLE_REJECTS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_VTABLE_HITS;
pub(crate) static TITLE_OWNER_SCAN_LAST_CANDIDATE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_OWNER_SCAN_LAST_TABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_LAST_STATE_BITS;
pub(crate) static MENU_CONTINUE_ENTRY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_CTOR_HITS;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS;
pub(crate) static MENU_WINDOW_JOB_CTOR_LAST_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_CTOR_LAST_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_CTOR_LAST_FUNCTOR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_CTOR_LAST_DOCALL: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_CTOR_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS;
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_OUT_SLOT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_FUNCTOR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_DOCALL: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS;
pub(crate) use er_telemetry::counters::MENU_WINDOW_JOB_IDLE_CTOR_HITS;
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_DOCALL: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::MENU_CONTINUE_IDLE_INSERT_HITS;
pub(crate) static MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_IDLE_INSERT_LAST_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_IDLE_INSERT_LAST_ARG1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_IDLE_INSERT_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_IDLE_INSERT_LAST_ARG1_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_IDLE_INSERT_LAST_RET_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TASK_ENQUEUE_GENERIC_HITS;
pub(crate) static TASK_ENQUEUE_GENERIC_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_LAST_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_LAST_ARG0_POINTEE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_LAST_ARG1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_LAST_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0_POINTEE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE0_ARG1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE0_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE1_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE1_ARG1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TASK_ENQUEUE_GENERIC_SAMPLE1_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS;
pub(crate) static TASK_ENQUEUE_GENERIC_IDLE_ITEM_LAST_MATCH_KIND: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_LAST_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_LAST_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_LAST_FUNCTOR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_LAST_DOCALL: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::MENU_ITEM_UPDATE_HITS;
pub(crate) use er_telemetry::counters::MENU_ITEM_UPDATE_SEMANTIC_HITS;
pub(crate) static MENU_ITEM_UPDATE_LAST_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_ITEM_UPDATE_LAST_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_ITEM_UPDATE_LAST_FUNCTOR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_ITEM_UPDATE_LAST_DOCALL: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_ITEM_UPDATE_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_CANDIDATE_ITEM: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES;
pub(crate) use er_telemetry::counters::MENU_CONTINUE_CANDIDATE_HITS;
pub(crate) use er_telemetry::counters::MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS;
pub(crate) use er_telemetry::counters::MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS;
pub(crate) use er_telemetry::counters::MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS;
pub(crate) static MENU_CONTINUE_CANDIDATE_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_NATIVE_READY_PREDICATE_HITS;
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_CALLER_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_VTABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_GETTER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) use er_telemetry::counters::TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS;
pub(crate) use er_telemetry::counters::TITLE_NATIVE_READY_PREDICATE_LAST_MASKED;
pub(crate) use er_telemetry::counters::TITLE_NATIVE_READY_PREDICATE_LAST_RET;
pub(crate) static B80_NATIVE_DISPATCHER_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_ITEM_FIELD_LOG_COUNT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static B80_DISPATCHER2_OBSERVE_COUNT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static B80_DISPATCHER2_OBSERVE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_CONTINUE_FUNCTOR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_DOCALL: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_ROUTER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_CONTINUE_INDEX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static AUTOLOAD_PHASE_EPOCH: OnceLock<Instant> = OnceLock::new();
pub(crate) static OWN_STEPPER_MENU_BUILD_STARTED_MS: AtomicU64 =
    AtomicU64::new(PHASE_TIMER_UNSET_MS);
pub(crate) static OWN_STEPPER_S2_PHASE_STARTED_MS: AtomicU64 = AtomicU64::new(PHASE_TIMER_UNSET_MS);

pub(crate) const PHASE_TIMER_UNSET_MS: u64 = u64::MAX;
pub(crate) const PHASE_TIMER_ZERO_MS: u64 = 0;
pub(crate) const U64_MAX_AS_U128: u128 = u64::MAX as u128;

pub(crate) const PROFILE_SLOT_ACTIVATE_RVA: usize =
    ProfileLoadMenuRva::ProfileSlotActivate as usize;
pub(crate) const PROFILE_LOAD_SELECTOR_TICK_RVA: usize =
    ProfileLoadMenuRva::ProfileLoadSelectorTick as usize;

/// One-shot guard for the autonomous open-menu (`maybe_auto_open_menu`).
pub(crate) use er_telemetry::counters::TFC_AUTO_MENU_OPENED;
/// One-shot guard for `maybe_fire_tfc_continue` (0 = not yet fired).
pub(crate) use er_telemetry::counters::TFC_CONTINUE_FIRED;
/// The queue-owner dialog whose MenuJobQueue (`dialog+0x10`) holds the posted LoadGame job, for the
/// per-frame drain (`tfc_continue_drain_tick`). 0 = nothing to drain. Set by `maybe_fire_tfc_continue`
/// after a successful PushBackJob.
pub(crate) use er_telemetry::counters::TFC_DRAIN_DIALOG;
/// The built LoadGame job pointer (selector's out[0]) to pump directly via `ExecuteMenuJob` each
/// frame. 0 = nothing to pump. Set by `maybe_fire_tfc_continue`; cleared when the job completes
/// (ExecuteMenuJob zeroes the slot) or the tick cap is hit. Pumping our own job avoids the dialog's
/// +0x8 slot that AV'd the queue-drain wrapper.
pub(crate) use er_telemetry::counters::TFC_DRAIN_JOB;
/// Per-frame drain tick counter (caps the drain so a stuck job cannot spin forever).
pub(crate) use er_telemetry::counters::TFC_DRAIN_TICKS;
/// Throttle counter for the dialog+0x50 load-vector readiness gate in `maybe_fire_tfc_continue`
/// (logs the count value occasionally while waiting for it to become a valid has-room vector).
pub(crate) use er_telemetry::counters::TFC_LOAD_VEC_WAIT_TICKS;
/// One-shot guard for installing the TitleTopDialog::update hook (`install_title_update_hook`).
pub(crate) use er_telemetry::counters::TITLE_UPDATE_HOOK_INSTALLED;
/// Trampoline for the hooked TitleTopDialog::update (`title_update_detour` -> original). 0 = not hooked.
pub(crate) use er_telemetry::counters::TITLE_UPDATE_ORIG;
/// Max drain ticks (~ a generous loading-screen budget at 60fps) before giving up on the drain.
pub(crate) const TFC_DRAIN_TICK_CAP: usize = 4096;

/// One-shot log latch for `force_offline_connection_bytes` (only logs the first 1->0 clear).
pub(crate) use er_telemetry::counters::FORCE_OFFLINE_BYTES_CLEARED;

/// Detour for CS::TitleTopDialog::update (0x1409aac10, vtable slot 2). Runs IN THE PUMP'S FRAME with
/// the LIVE dialog (rcx) -- the in-context timing our recurring-game-task build lacked. Calls the
/// original first (the pump sets up the live dialog state + drains the menu jobs), then runs the gated
/// one-shot Continue build (`maybe_fire_tfc_continue`), so it builds with the now-live dialog fields
/// (dialog+0x50 valid -> no mis-context overflow). Build is catch_unwind-wrapped so the pump always
/// proceeds. bd HOOK-DESIGN-titletopdialog-update-0x1409aac10-incontext-build-2026-06-23.
pub(crate) unsafe extern "system" fn title_update_detour(dialog: usize, delta: f32, input: usize) {
    let orig_addr = TITLE_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig_addr != TITLE_OWNER_SCAN_START_ADDRESS && orig_addr != 0 {
        let orig: unsafe extern "system" fn(usize, f32, usize) =
            unsafe { std::mem::transmute(orig_addr) };
        unsafe { orig(dialog, delta, input) };
    }
    // In-context now (pump frame, live dialog). Run the gated one-shot Continue build.
    if let Ok(base) = game_module_base() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            maybe_fire_tfc_continue(base)
        }));
    }
}

// ===== READINESS-GATED press-any-button advance (golden path, zero-input) =====
// The press-any-button gate is CODE, not a packed asset: the per-frame node-update/builder
// 0x1407ad1c0 builds the MenuJobWait job into [step+0x130]; the job completes when predicate
// 0x1407a9200 (= `*rcx>=2`) sees [job+0x1e8]>=2 (the press-count the native node bumps on the bound
// keycode [job+0x180]). We READINESS-gate the EXISTING job zero-input: hook the node-update, and once
// the job is built+valid (we are at press-any-button) and settled, write [job+0x1e8]=2 so the job's
// OWN predicate passes and it completes via its NORMAL path (bootstrap cascade intact). No new job (no
// cap-8 overflow), no replace, no file mod, no input. Distinct from the DEAD latch-force 0x143d856a0
// (skipped bookkeeping -> crash). bd press-any-button-golden-lever-job1e8-readiness-2026-06-23.

/// Press-any-button node-update/builder RVA (deobf/live; prologue re-confirmed in the 0x1407adxxx
/// region, which is otherwise flagged unreliable). `__fastcall(rcx=step, rdx, r8[, r9])`.
pub(crate) const PAB_NODE_UPDATE_RVA: u32 = 0x7ad1c0;
/// The built press-any-button job within the node-update receiver: `[step+0x130]`.
pub(crate) const PAB_JOB_SLOT_130_OFFSET: usize = 0x130;
/// The job's completion press-count the predicate 0x1407a9200 reads (>=2 == complete).
pub(crate) const PAB_JOB_PRESS_COUNT_1E8_OFFSET: usize = 0x1e8;
/// The job's bound keycode (logged for identity validation + the documented fallback input bit).
pub(crate) const PAB_JOB_KEYCODE_180_OFFSET: usize = 0x180;
/// The "pressed" value the predicate treats as complete.
pub(crate) const PAB_PRESS_COUNT_SATISFIED: u32 = 2;
/// Upper sanity bound for a plausible press-count (reject garbage/unreadable reads -> keep waiting).
pub(crate) const PAB_COUNT_SANITY_MAX: u32 = 8;
/// Frames the press-any-button job must be built+valid before we advance (screen settle).
pub(crate) const PAB_ADVANCE_SETTLE_FRAMES: usize = 10;
/// Minimum plausible heap pointer (reject not-yet-built / garbage job slots).
pub(crate) const PAB_MIN_HEAP_PTR: usize = 0x10000;

/// One-shot latch: the readiness advance has fired (0 = not yet).
pub(crate) use er_telemetry::counters::PAB_ADVANCE_FIRED;
/// One-shot guard for installing the PAB node-update hook.
pub(crate) use er_telemetry::counters::PAB_ADVANCE_HOOK_INSTALLED;
/// Trampoline to the original PAB node-update. 0 = not hooked.
pub(crate) use er_telemetry::counters::PAB_ADVANCE_ORIG;
/// Valid-job-frame settle counter for the readiness advance.
pub(crate) use er_telemetry::counters::PAB_ADVANCE_SETTLE;

/// Detour for the press-any-button node-update 0x1407ad1c0. Calls the original (builds/updates the job
/// at `[step+0x130]`) then runs the gated, fail-closed, one-shot readiness advance. Pass-through return.
pub(crate) unsafe extern "system" fn pab_node_update_detour(
    step: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let orig_addr = PAB_ADVANCE_ORIG.load(Ordering::SeqCst);
    let ret = if orig_addr != TITLE_OWNER_SCAN_START_ADDRESS && orig_addr != 0 {
        let orig: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig_addr) };
        unsafe { orig(step, rdx, r8, r9) }
    } else {
        0
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        pab_advance_try(step)
    }));
    // PAB deterministically WINS the shared MinHook slot at 0x7ad1c0 (MENU_WINDOW_JOB_RUN_RVA ==
    // PAB_NODE_UPDATE_RVA), so it must also run the System->Quit post-original work here. Root cause
    // (2026-07-15): THREE detours target 0x7ad1c0 (PAB, System->Quit MenuWindowJob::Run, and the dead
    // title-cover hook); MinHook binds only ONE. On native Windows the inline/early PAB install wins and
    // the background-thread System->Quit install fails ALREADY_CREATED, so `system_quit_menu_window_run_post`
    // -- the SOLE writer of the hide latch (SYSTEM_QUIT_REAL_WINDOWS_HIDDEN) and the slot-activation gate
    // latch (SYSTEM_QUIT_PROFILE_SELECT_WINDOW) -- never ran, giving BOTH the ghosting and the
    // non-interactive ProfileSelect. Under Wine the scheduler happened to let System->Quit win. Making the
    // guaranteed winner (PAB) call run_post removes the race on both platforms. `step`==rcx==the
    // MenuWindowJob `this`; `ret`==the Run return; run_post early-returns for non-System/ProfileSelect jobs
    // so this is cheap on every other Run pass. Recursion-guarded: a Run re-entered from run_post's own
    // return-title submit must not re-run run_post.
    if crate::constants::TITLE_CUSTOM_COVER_RUN_RECURSION.load(Ordering::SeqCst) == 0 {
        crate::constants::TITLE_CUSTOM_COVER_RUN_RECURSION.store(1, Ordering::SeqCst);
        let n = crate::constants::PAB_RUN_POST_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
        if n == 1 || n % 1200 == 0 {
            append_autoload_debug(format_args!(
                "pab-run-post: PAB detour (deterministic 0x7ad1c0 winner) drove system_quit_menu_window_run_post #{n}"
            ));
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            crate::experiments::startup_hooks::system_quit_menu_window_run_post(step, ret)
        }));
        crate::constants::TITLE_CUSTOM_COVER_RUN_RECURSION.store(0, Ordering::SeqCst);
    }
    ret
}

#[derive(Clone, Copy)]
pub(crate) struct MenuActionNode {
    pub(crate) node: usize,
    pub(crate) node_vt: usize,
    pub(crate) registry: usize,
    pub(crate) member_dialog: usize,
    pub(crate) member_fn: usize,
    pub(crate) member_adjust: usize,
    pub(crate) window_item: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct NativeContinueEntry {
    pub(crate) entry: usize,
    pub(crate) functor: usize,
    pub(crate) do_call: usize,
    pub(crate) router: usize,
    pub(crate) index: usize,
    pub(crate) cursor: i32,
}

#[derive(Clone, Copy)]
pub(crate) struct NativeContinueItemAction {
    pub(crate) item: usize,
    pub(crate) result: usize,
    pub(crate) result_vt: usize,
    pub(crate) functor: usize,
    pub(crate) do_call: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct LiveDialogFireReady {
    pub(crate) title_dialog: usize,
    pub(crate) title_dialog_vt: usize,
    pub(crate) capture_slot: usize,
    pub(crate) capture: usize,
    pub(crate) capture_vt: usize,
    pub(crate) registry_vt: usize,
    pub(crate) menu_opened_latch: usize,
    pub(crate) menu_window: usize,
    pub(crate) menu_window_vt: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct ProfileLoadDialogReady {
    pub(crate) dialog: usize,
    pub(crate) dvt: usize,
    pub(crate) bound: i32,
    pub(crate) cursor_now: i32,
    pub(crate) cursor_target: i32,
    pub(crate) expected_slot: i32,
    pub(crate) load_activate: usize,
    pub(crate) load_job_ctx: usize,
    pub(crate) load_job_ctx_vt: usize,
    pub(crate) player_game_data: usize,
}

#[derive(Clone, Copy)]
pub(crate) enum StartupModalBlockingState {
    Clear,
    Blocking {
        dialog: usize,
        vtable: usize,
        closing_latch: usize,
    },
}

pub(crate) struct ProductCoreAutoloadReady {
    pub(crate) committed: i32,
    pub(crate) requested: i32,
    pub(crate) table: usize,
    pub(crate) session: usize,
    pub(crate) game_data_man: usize,
    pub(crate) profile_summary: usize,
    pub(crate) iodev: usize,
    pub(crate) heap_allocator: usize,
    pub(crate) title_dialog: usize,
    pub(crate) title_in_loop: bool,
    pub(crate) title_in_textfadeout: bool,
    pub(crate) menu_opened_latch: usize,
    pub(crate) press_start_proxy: usize,
    pub(crate) press_start_context: usize,
}

pub(crate) struct TitlePressButtonComponent {
    pub(crate) proxy: usize,
    pub(crate) context: usize,
}

pub(crate) struct TitleDialogState {
    pub(crate) in_loop: bool,
    pub(crate) in_textfadeout: bool,
    pub(crate) menu_opened_latch: usize,
}

/// OWN-THE-STEPPER step 2 (the load driver): runs IN-CONTEXT at idx10 (STEP_MenuJobWait,
/// rcx=owner, rdx=FD4Time) as a real FD4 step. After letting the boot settle to the
/// stable press-any-button state, it drives the game's OWN load: SetState(3=BeginTitle)
/// builds the Continue/Load menu + sets GameMan+0xc30 to the most-recent saved map, then
/// the native Continue confirm 0x140b0e180 (via a {[+8]=owner} shim) does slot-select +
/// child-request + SetState(5=PlayGame). The native pump then loads the world, SKIPPING
/// the entire variable UI -- no input, no menu traversal.
pub(crate) unsafe extern "system" fn own_stepper_idx10(owner: usize, framectx: usize) {
    let n = OWN_STEPPER_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let base = OWN_STEPPER_BASE.load(Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let gm = game_man_ptr_or_null();
    // GOLDEN BASELINE mode: cache the live TITLE owner (stable pointer, supplied as our first arg every
    // title frame) into OWN_LOAD_OWNER_CACHED so the RECURRING world-stream observer can re-derive
    // InGameStep/MoveMapStep live from it on a user-driven vanilla load. We deliberately DO NOT cache
    // InGameStep here (leave OWN_LOAD_INGAMESTEP_CACHED at 0): on a vanilla load InGameStep is built
    // later during the loading screen, so the observer's `ingame_cached == 0` fallback must resolve it
    // fresh each frame. OBSERVE-ONLY -- never fires continue/SetState5/any load. (Skipped once our own
    // OWN-LOAD continue fired, which already cached the precise owner/InGameStep it kicked the load on.)
    if golden_observe_enabled()
        && !OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst)
        && owner != TITLE_OWNER_SCAN_START_ADDRESS
        && owner != 0
    {
        OWN_LOAD_OWNER_CACHED.store(owner, Ordering::SeqCst);
    }
    let read_gm = |off: usize| {
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let pass_through = |force_log: bool| {
        if force_log || n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            append_autoload_debug(format_args!(
                "own_stepper: pass-through #{n} phase={phase} owner=0x{owner:x} c30=0x{c30:x} framectx=0x{framectx:x}"
            ));
        }
        let orig = OWN_STEPPER_ORIG_IDX10.load(Ordering::SeqCst);
        if orig != TITLE_OWNER_SCAN_START_ADDRESS {
            let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
            unsafe { f(owner, framectx) };
        }
    };
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    // OBSERVE-ONLY NATIVE-LOAD mode (gated OFF by default). Takes precedence over ALL the
    // own_stepper forcing logic below: it does NOT force the title machine -- the native boot
    // advances naturally via pass-through, and once the live menu is rendered + settled we fire
    // the native Load-Game node's run exactly once, then keep observing so the golden oracle is
    // written as the native pump loads the char. Pure read-only until the one-shot fire.
    // OBSERVE-ONLY NATIVE FULL-SAVE-READ mode (gated OFF by default). Takes precedence over ALL the
    // own_stepper forcing logic below AND over native_load: it does NOT force the title machine --
    // the native boot advances naturally via pass-through, and once the live menu is rendered +
    // settled it runs the full-save-read load chain (SUBMIT -> DRAIN -> DESER -> GUARD -> CONFIRM)
    // at the LIVE menu (where the FD4 IO worker pool is live so the submit drains). The sole save
    // write (continue_confirm -> SetState5) is HARD-gated behind the step-6 guard AND the commit
    // sub-gate (default = VERIFY-ONLY). NO SetState forcing for boot, NO selector pump.
    if native_fullread_enabled() {
        unsafe { native_fullread_tick(owner, base, n) };
        pass_through(false);
        return;
    }
    if native_load_enabled() {
        unsafe { native_load_tick(owner, base, n) };
        pass_through(false);
        return;
    }
    // OBSERVE-ONLY NATIVE-CONTINUE mode (PATH B, gated OFF by default). Same precedence/structure as
    // native_load: it does NOT force the title machine -- the native boot advances naturally via
    // pass-through, and once the live menu is rendered + settled we fire the native Continue
    // (load-most-recent) node's run exactly once, then keep observing so the golden oracle +
    // world-stream telemetry are written as the FULL native load (parse+stream+spawn) runs. Pure
    // read-only until the one-shot fire; NO SetState forcing.
    if native_continue_enabled() {
        // native_continue's Continue-node scan/fire was DEAD CODE: the continue-scan never found the
        // node (found_continue_node=0x0 every frame). The zero-input load actually fires via
        // pab-advance + title-accept-byte natural menu-open (verified 2026-06-24,
        // autoload-zero-input-world-reached-validated). Keep only the save-not-loaded watchdog (aborts
        // if the gold never loads) + per-frame world-stream telemetry (mms_state/block_count/
        // io_inflight/player_present), then pass through so the NATIVE title machine advances untouched.
        unsafe { save_load_watchdog() };
        unsafe { own_load_stream_telemetry(base, gm, owner, n) };
        pass_through(false);
        return;
    }
    let read_iodev = || {
        let iodev = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        if iodev != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe {
                (
                    *((iodev + IODEV_INFLIGHT_10_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_18_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_20_OFFSET) as *const usize),
                )
            }
        } else {
            (
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
                TITLE_OWNER_SCAN_START_ADDRESS,
            )
        }
    };
    // SAVE-SAFE world-res streaming-driver cold-build probe (gated OFF by default; one-shot).
    // Builds the CSEmkResManImp driver (0x143d7c088) + stream worker (0x144842d40) at the parked
    // title via the CSResStep getter with a stub `this` -- NO SetState, NO world load, NO save
    // write. Validates emk-resman-streaming-driver-coldbuild-stub-lever-2026 live. Additive: the
    // normal phase logic continues (default = stay at the open menu, save-safe).
    if worldres_coldbuild_probe_enabled() && unsafe { title_boot_ready(owner, base) } {
        unsafe { worldres_coldbuild_probe(base) };
    }
    // DECISIVE save-data experiment (gated OFF by default; SAVE-SAFE). Register the stream worker,
    // then drive the cold b80 save-IO mount (preview -> poll to b80==3 -> deserialize) so 0x67b290
    // mounts the real char to memory -- NO SetState, NO save write. Bypasses the menu drive while
    // active; pass-through keeps the title ticking so the scheduler ticks the registered worker.
    // SAVE-SAFE verify-only OWN-LOAD buffer-feed probe (gated OFF by default; one-shot). Takes
    // precedence over cold_char_mount: hooks 0x67b100 to feed our sliced .sl2 slot body, calls the
    // native parser 0x67b290(slot), and reads back c30 + the char fingerprint. NO SetState5, NO
    // save write. Bypasses the menu drive while active; pass-through keeps the title ticking.
    if own_load_enabled() && unsafe { title_boot_ready(owner, base) } {
        unsafe { own_load_drive(base, gm, owner, want_slot, n) };
        // Per-frame world-stream stall telemetry (pure reads). own_load_drive's one-shot phase
        // machine fast-forwards to PHASE_DONE after the verify/continue fires, so this runs EVERY
        // own_load frame -- including all the post-continue_confirm/SetState5 loading-screen frames
        // -- and publishes the deepest world-load pump values so a probe log shows whether the
        // stream advances or is frozen at WorldResWait. Gated to the own_load path: never in play.
        unsafe { own_load_stream_telemetry(base, gm, owner, n) };
        pass_through(false);
        return;
    }
    if cold_char_mount_enabled() && unsafe { title_boot_ready(owner, base) } {
        unsafe { cold_char_mount_drive(base, gm, want_slot, n) };
        pass_through(false);
        return;
    }
    if phase == OWN_STEPPER_PHASE_MENU {
        // Drive only when the title owner/scheduler/session/dialog are semantically ready.
        // want_slot == -1 is the "most-recent" intent (resolved from the dialog's natural
        // highlight at PHASE_S2_ACTIVATE), NOT a "do nothing" signal.
        if !unsafe { title_boot_ready(owner, base) } {
            if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: waiting for title_boot_ready before menu drive #{n} owner=0x{owner:x}"
                ));
            }
            pass_through(false);
            return;
        }
        if let StartupModalBlockingState::Blocking {
            dialog,
            vtable,
            closing_latch,
        } = startup_modal_blocking_state()
        {
            if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: startup_modal_blocking_state=Blocking dialog=0x{dialog:x} vt=0x{vtable:x} closing_latch={closing_latch} before menu drive #{n}"
                ));
            }
            pass_through(false);
            return;
        }
        // NO-WRITE CHECKPOINT. Path A (b78-route) is RUNTIME-FALSIFIED
        // (pathA-b78-route-falsified-b80-stuck-latch-gate-2026): disp2 0x140afb880's b78-route
        // is gated by the title-accept latch [0x143d856a0] (SET by load time -> disp2 bails to
        // cleanup every frame), so GameMan+0xb80 never leaves 0 and the native PlayGame
        // defaults to a NEW-GAME null character (which autosaved over the live slot in the
        // Seamless run). Every hand-driven b80 lever (cold slot-int primitives, b72 lever,
        // b78-route) hits the SAME wall: b80 reaches 3 ONLY when the native MoveMapListStep
        // async job pumps the menu deserialize 0x14082c240; FD4 stream-worker registration
        // alone does NOT advance b80 (0x140af1b40 registers the same task 0x144842d40 under the
        // same key 0x59682f01 as the in-game 0x140b0a980 milestone lever-c already tried with
        // b80 still 0). So idx10 NO LONGER SetState(5)s -- it stays at the title (NO save
        // write) pending the Path B menu-drive (drive the selector-owner step 0x140826d50 /
        // native Load-Game menu entry so the native async job mounts c30=real before PlayGame).
        // STAGE 1 (NO-WRITE layout verification + zero-input main-menu build). The parked
        // press-any-button title is the FIRST state 10 and has NOT run BeginTitle, so
        // owner+0x138 holds only intro items, not Continue/Load. (1) Walk the bare tree and
        // log it to VERIFY the live FD4 SBO pointer-vector layout against the static RE
        // (the captured recipe pointers were suspiciously low -- verify before any invoke).
        // (2) Build the main menu zero-input via SetState(owner, 3=BeginTitle): BeginTitle
        // needs no session and writes NO save (it is a menu-UI build), so this is save-safe;
        // it is exactly what the native press does after BeginLogo. The next frames run
        // BeginTitle (populating Continue/Load into owner+0x138) then return to state 10,
        // where PHASE_MENU_BUILD walks + identifies the Load-Game leaf. Stage 2 (invoke its
        // +0xa8 functor -> drive the dialog -> native mount) follows once this confirms the
        // live layout + item. Every hand-driven b80 lever is dead (the menu async job is the
        // only thing that mounts c30 before PlayGame); this is the Path B menu-drive.
        // T0: the common timeline start -- the title is parked at state 10 and we begin the
        // DLL drive. The first timeline_event sets the wall-clock epoch (so all later ms= are
        // measured from here); a native-baseline observe run sets T0 the same way.
        timeline_event(
            "T0",
            n,
            format_args!("owner=0x{owner:x} state10 slot={want_slot} c30=0x{c30:x}"),
        );
        // PASSIVE mode: do NOT force the menu. Hand off to PHASE_MENU_BUILD which waits for the
        // user to navigate to Load Game (surfacing d180 via the capture hooks), then runs STAGE 2.
        if own_stepper_passive_enabled() {
            append_autoload_debug(format_args!(
                "own_stepper: PASSIVE -- not forcing the menu; waiting for the user to open Load Game so d180 is captured, then STAGE 2 drives the load (input UNBLOCKED) #{n}"
            ));
            own_stepper_enter_menu_build_phase();
            pass_through(false);
            return;
        }
        let (bare, bare_tree) = if live_dialog_enabled() {
            (None, None)
        } else {
            (
                unsafe { diagnostic_menu_walk(owner, base, "bare", true) },
                unsafe {
                    diagnostic_job_tree_walk(
                        owner,
                        base,
                        TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                        "bare-tree",
                        true,
                    )
                },
            )
        };
        // STAGE 1c: build the FULL main menu by replicating the engine's OWN press path.
        // The parked press-any-button screen is the FIRST state 10; the native press handler
        // 0x140b0b6b0 issues SetState(owner,2)=BeginLogo, after which the native pump advances
        // 2->3->10 and builds the Continue / Load-Game(d180) / New-Game items into the CSMenu
        // registry at owner+0xe0. The registry update 0x1409aac10 then ticks EVERY registered
        // entry each frame, so our menu-item Update hook (functor_chain_hits_factory) will
        // capture d180. SetState(3)=BeginTitle ALONE (skipping BeginLogo) only built the
        // BackScreen (runtime: only c000 ticked), so we drive the full sequence. BeginLogo(2)
        // hard-asserts session singleton 0x144588e98 at entry -- read it live; SetState(2) only
        // when non-null, else fall back to SetState(3). Save-safe either way: BeginLogo/BeginTitle
        // are menu-UI builds with NO save write (only SetState(5)/PlayGame writes).
        let session = unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let target_state = if session != TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_STEP_BEGIN_LOGO
        } else {
            TITLE_STEP_BEGIN_TITLE
        };
        // CRITICAL: STEP_BeginLogo builds the main-menu list (Continue/Load d180/...) into
        // owner+0xe0 via 0x14081f180 ONLY when [owner+0xb8]==0; if set it short-circuits to
        // SetState(3) and skips the build (bd mainmenu-item-builder-into-iterator-tree-2026) --
        // which is why our prior SetState(2) only produced the 3 title-composition items. Clear
        // the gate so BeginLogo runs the full build (zero-input, menu-UI only -> save-safe).
        let beginlogo_gate =
            unsafe { safe_read_usize(owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if target_state == TITLE_STEP_BEGIN_LOGO {
            unsafe {
                *((owner + TITLE_OWNER_BEGINLOGO_LIST_GATE_B8_OFFSET) as *mut u32) =
                    TITLE_OWNER_BEGINLOGO_GATE_CLEAR;
            }
        }
        let set_state: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + TITLE_SET_STATE_RVA) };
        unsafe { set_state(owner, target_state) };
        own_stepper_enter_menu_build_phase();
        append_autoload_debug(format_args!(
            "own_stepper: STAGE1c bare-walk done (load_game_138=0x{:x} load_game_tree=0x{:x}) session(0x144588e98)=0x{session:x} beginlogo_gate(0xb8)=0x{beginlogo_gate:x} -> SetState({target_state}) [{}] to build the FULL main menu zero-input (#{n}) slot={want_slot} gm=0x{gm:x} c30=0x{c30:x} b80={b80}",
            bare.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            bare_tree.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
            if target_state == TITLE_STEP_BEGIN_LOGO {
                "BeginLogo 2->3->10 full menu"
            } else {
                "BeginTitle fallback (session null)"
            }
        ));
        // Suppress unused warnings for consts/statics retained from the falsified cold
        // slot-int drive, synthetic-dispatcher, b78-route, and Continue-shim work.
        let _ = (
            invoke_menu_item_functor as usize,
            CONTINUE_CONFIRM_RVA,
            B80_FULL_LOAD_INITIATOR_RVA,
            OWN_STEPPER_PHASE_MOUNT,
            OWN_STEPPER_PHASE_DRIVE,
            OWN_STEPPER_PHASE_CONTINUE,
            B80_DISPATCHER1_RVA,
            B80_DISPATCHER2_RVA,
            SYNTH_MMS_SKIP_APPLY_12A_OFFSET,
            SYNTH_MMS_DESER_SLOT_12C_OFFSET,
            SYNTH_MMS_SKIP_APPLY_ON,
            OWN_STEPPER_DRIVE_MAX,
            OWN_STEPPER_SHIM_OWNER_IDX,
            OWN_STEPPER_MOUNT_POLL_MAX,
            OWN_STEPPER_B80_RESIDENT,
            OWN_STEPPER_B80_PREVIEW_LANE,
            OWN_STEPPER_B80_IDLE,
            B80_POLL_RVA,
            B80_POLL_ARG_ZERO,
            B80_LANE1_DRIVER_RVA,
            B80_LOAD_SAVE_DATA_INITIATOR_RVA,
            DESERIALIZE_SLOT_RVA,
            LOAD_INITIATOR_RVA,
            WORLD_WORKER_BUILD_RVA,
            crate::runtime_heap_allocator_ptr_or_null as fn() -> usize,
            WORLD_WORKER_BUILD_STATE,
            SYNTHETIC_STEP_STATE_OFFSET,
            FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
            GAME_MAN_REQUESTED_SLOT_B78_OFFSET,
            GAME_MAN_ARM_FLAG_B72_OFFSET,
            TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET,
            TITLE_OWNER_PLAY_GAME_SLOT_OFFSET,
            DEFAULT_PLAY_GAME_MAP,
            TITLE_STEP_PLAY_GAME,
            &raw const OWN_STEPPER_SHIM,
            &raw const SYNTH_MMS_OWNER,
            &raw mut OWN_STEPPER_WORKER_THIS,
            &OWN_STEPPER_DRIVE_CALLS,
            &OWN_STEPPER_MOUNT_POLLS,
        );
        let _ = read_iodev;
        pass_through(false);
        return;
    }
    if phase == OWN_STEPPER_PHASE_MENU_BUILD {
        let waits =
            OWN_STEPPER_MENU_BUILD_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
        let menu_elapsed_ms = own_stepper_menu_build_elapsed_ms();
        let menu_build_timed_out = own_stepper_menu_build_timed_out();
        if let StartupModalBlockingState::Blocking {
            dialog,
            vtable,
            closing_latch,
        } = startup_modal_blocking_state()
        {
            if waits % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: PHASE_MENU_BUILD startup modal still blocking dialog=0x{dialog:x} vt=0x{vtable:x} closing_latch={closing_latch} -- polling modal lifecycle, not a grace counter"
                ));
            }
            pass_through(false);
            return;
        }
        // ZERO-INPUT d180 LOCATE (replaces the old simulated-input cursor nav, which wrote the
        // keystate bitmap inputmgr+0x90 to move the cursor onto Load-Game -- that is synthesized
        // input and VIOLATES the No-Compromises zero-input standard). SetState(2)->3->10 builds the
        // main-menu job tree; the Load-Game item d180 (a MenuWindowJob whose +0xa8 functor's
        // _Do_call chains to dialog_factory 0x14081ead0) is constructed into the tree at BUILD time,
        // so a pure-read recursive walk can surface it WITHOUT the pump ticking it and WITHOUT any
        // input. A user-driven capture (2026-06-17) pinned d180's functor object = {_Func_impl
        // vtable 0x142ac3ea8, captured owner+0x138}; the factory reads [capture+8]=owner+0x138 as
        // the dialog owner. We walk the candidate holder roots and, on the first functor->factory
        // hit, latch the item into MENU_LOAD_GAME_ITEM so STAGE 2 drives the load. (The
        // cap_menu_item_update hook also sets it if d180 ever ticks; whichever fires first wins.)
        // Throttled; pure reads -> save-safe.
        const D180_ROOT_E0: usize = 0xe0;
        const D180_ROOT_130: usize = 0x130;
        const D180_ROOT_138: usize = 0x138;
        // d180's +0xa8 functor object = {_Func_impl vtable base+0x2ac3ea8, capture[+8]=owner+0x138}
        // (user-driven capture 2026-06-17) -- a strong fingerprint corroborating the functor->factory
        // classification.
        const MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA: usize =
            ProfileLoadMenuRva::MenuLoadGameFunctorVtable as usize;
        if !own_stepper_passive_enabled()
            && !input_probe_enabled()
            && !live_dialog_enabled()
            && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { title_scheduler_ready(owner, base) }
        {
            // Walk the candidate roots; on the first functor->dialog_factory hit (= the Load-Game
            // item d180), validate its fingerprint and LATCH it into MENU_LOAD_GAME_ITEM. STAGE 2
            // then drives it via the NATIVE MenuWindowJob::Update 0x1407ad1c0 (which wires the ctx
            // item+0x10 from the descriptor item+0x58 before firing the functor -> NO synthetic
            // ctx, NO save write). The cap_menu_item_update hook also sets it if d180 ever ticks;
            // whichever fires first wins. Throttled; pure reads here (save-safe).
            const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
            const ITEM_CTX_10: usize = 0x10;
            const ITEM_RESULT_130: usize = 0x130;
            let verbose = OWN_STEPPER_TITLETOP_DUMPS
                .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                < OWN_STEPPER_TITLETOP_DUMP_CAP;
            let roots = [D180_ROOT_E0, D180_ROOT_130, D180_ROOT_138];
            for &root in roots.iter() {
                if let Some(item) =
                    unsafe { diagnostic_job_tree_walk(owner, base, root, "d180-locate", verbose) }
                {
                    let null = TITLE_OWNER_SCAN_START_ADDRESS;
                    let functor =
                        unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
                    let fvt = if functor != null {
                        unsafe { safe_read_usize(functor) }.unwrap_or(null)
                    } else {
                        null
                    };
                    let fcap = if functor != null {
                        unsafe { safe_read_usize(functor + core::mem::size_of::<usize>()) }
                            .unwrap_or(null)
                    } else {
                        null
                    };
                    let ctx10 = unsafe { safe_read_usize(item + ITEM_CTX_10) }.unwrap_or(null);
                    let res130 = unsafe { safe_read_usize(item + ITEM_RESULT_130) }.unwrap_or(null);
                    MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "own_stepper: ZERO-INPUT d180 LOCATED item=0x{item:x} via owner+0x{root:x} functor=0x{functor:x} fvt=0x{fvt:x}(want base+0x{:x}) fcap=0x{fcap:x}(want owner+0x138=0x{:x}) ctx10=0x{ctx10:x} result130=0x{res130:x} -- latched, STAGE2 will native-Update it",
                        MENU_ITEM_LOADGAME_FUNCTOR_VTABLE_RVA,
                        owner.wrapping_add(D180_ROOT_138)
                    ));
                    break;
                }
            }
        }
        // STAGE 1d: open the main menu zero-input. SetState(2)->3->10 built the TitleTopDialog at
        // owner+0xe0 (vt 0x142b26468). The dialog's native update 0x1409aac10 (ticked every frame
        // by pass_through -> STEP_MenuJobWait) runs the intro FadeIn animation, transitions
        // FadeIn->Loop on anim-complete (NOT input), and on its NON-INPUT Loop-ready path
        // (0x1409aade8) calls the open-menu registrar 0x1409b24e0 ITSELF, which set_state's the
        // SM [dialog+0xa60] to "TextFadeOut" and registers Continue/Load(d180)/New-Game. So the
        // PRIMARY path is to do NOTHING and let the native update self-open the menu.
        //
        // The prior force-call was harmful (bd titletopdialog-loop-ready-gate-2026): firing the
        // registrar on bare flags>=2 fired from the FadeIn node (wrong state) AND set the latch
        // [dialog+0xa40]=1, which PERMANENTLY blocks the native non-input path (it needs latch==0).
        // So here we (a) READ-ONLY probe the live state by NAME via the game's own is_in_state
        // (FadeIn/Loop/TextFadeOut) + the latch, logging it; and (b) only as a FALLBACK self-fire
        // the registrar on the CORRECT gate -- is_in_state(Loop)==true && latch==0 -- which is
        // exactly the native path's own precondition (zero input, NO save write). If the native
        // path fires first (latch->1 in Loop) we simply observe the menu open.
        const MENU_JOB_HOLDER_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
        if MENU_ENTRIES_SEEN.load(Ordering::SeqCst) == MENU_ENTRIES_SEEN_NO {
            let dialog = unsafe { safe_read_usize(owner + MENU_JOB_HOLDER_E0) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let dialog_vt = if dialog != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            // Only call into the dialog's FD4 state machine once owner+0xe0 IS the TitleTopDialog.
            if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
                // is_in_state receiver = the ADDRESS dialog+0xa60 (the embedded SM sub-object), per
                // the registrar's `add rcx,0xa60; call`. is_in_state(sm, desc) -> bool reads the
                // live state by name (no hand pointer-chase). Read-only / no side effects.
                let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
                let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
                    unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
                let in_fadein = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_FADEIN_RVA) }
                    != OWN_STEPPER_FALSE;
                let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) }
                    != OWN_STEPPER_FALSE;
                let in_textfadeout =
                    unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) }
                        != OWN_STEPPER_FALSE;
                let latch =
                    unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
                        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
                        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if waits % STAGE1D_RETRY_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1d probe dialog=0x{dialog:x} sm=0x{sm:x} fadein={in_fadein} loop={in_loop} textfadeout={in_textfadeout} latch={latch} waits={waits} (self-fire open-menu on Loop+latch-clear)"
                    ));
                }
                // SELF-FIRE the open-menu registrar on the CORRECT gate (the native path's own
                // precondition: settled in Loop + latch clear). RUNTIME-PROVEN NECESSARY
                // (headless-load 2026-06-17): with the modal suppressed (online-disable), the
                // TitleTopDialog SM sits in Loop forever -- the Loop-ready predicate needs the
                // accept byte (input), which never comes headless (latch=0 for 3000 waits). So the
                // "native self-opens" assumption is FALSE for a clean offline boot; we must fire
                // 0x1409b24e0 ourselves (the zero-input-menu-open milestone proved this opens the
                // menu). Default ON now (no flag) since headless cannot rely on a button press;
                // gated to the correct state (in_loop, NOT FadeIn) + once + latch-clear so it can
                // neither corrupt the SM (titletopdialog-fadein-gate) nor double-fire.
                if in_loop
                    && latch == TITLE_OWNER_SCAN_START_ADDRESS
                    && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) == OWN_STEPPER_MENU_OPENED_NO
                {
                    let open_menu: unsafe extern "system" fn(usize) =
                        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_OPEN_MENU_RVA) };
                    unsafe { open_menu(dialog) };
                    OWN_STEPPER_MENU_OPENED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    // Deterministic timing endpoint: the DLL has driven boot -> modal-skip ->
                    // past press-any-button -> a READY main menu with ZERO input. ms-from-T0 here
                    // is the headless boot-to-menu time (the part vanilla needs >=3 human inputs +
                    // an online-attempt timeout to reach).
                    timeline_event(
                        "T_menu_open",
                        n,
                        format_args!("dialog=0x{dialog:x} waits={waits}"),
                    );
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1d self-fire open-menu 0x{:x}(dialog=0x{dialog:x}) -- in Loop + latch clear (correct gate, zero-input) waits={waits}",
                        base + TITLE_TOP_DIALOG_OPEN_MENU_RVA
                    ));
                }
            }
        }
        // DETERMINISTIC INPUT PROBE: once the menu is open, drive a frame-precise Down->Confirm
        // (targeted input as a MEASUREMENT oracle) and short-circuit the zero-input locate/STAGE2
        // path -- the injected Confirm drives the native load; idx6 watches it. Answers whether the
        // d180 leaf ticks on highlight alone (so the zero-input functor-invoke route is viable).
        if input_probe_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
        {
            unsafe { menu_input_probe(owner, base) };
            pass_through(false);
            return;
        }
        // INJECT-NAV instrument-capture: self-drive the cursor with synthesized menu-DOWN while
        // the user's input stays blocked. The menu is KEYBOARD-navigated under Proton (XInput is
        // not polled), so the primary vehicle is the DInput keyboard block, into which we stamp
        // DIK_DOWN on the schedule (InputBlocker::set_injected_key); the gamepad button state is
        // also published for the XInput hook in case a controller is present. This runs every
        // frame (unlike the XInput hook). Capture-only: DOWN nav, never Confirm -> no load/write.
        if inject_nav_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
        {
            let nf = INJECT_NAV_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let buttons = inject_nav_buttons(nf);
            INJECT_NAV_CUR_BUTTONS.store(buttons as usize, Ordering::SeqCst);
            let dik = if buttons != INJECT_NAV_NO_BUTTONS {
                DIK_DOWN
            } else {
                DIK_NONE
            };
            InputBlocker::get_instance().set_injected_key(dik);
            // Find the cursor offset by observing it across the ONE deterministic Down: snapshot
            // before (cursor=0), diff after it settles (cursor=1). The 0->1 dword IS the cursor.
            if nf as usize == CURSOR_PROBE_BASELINE_FRAME {
                unsafe { cursor_offset_probe(owner, base, true) };
            } else if nf as usize == CURSOR_PROBE_POSTDOWN_FRAME {
                unsafe { cursor_offset_probe(owner, base, false) };
            }
            if dik != DIK_NONE {
                let lc = INJECT_NAV_LOG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                if lc < INJECT_NAV_LOG_FIRST {
                    append_autoload_debug(format_args!(
                        "inject-nav: frame={nf} menu-DOWN asserted (DIK=0x{dik:x} wButtons=0x{buttons:x})"
                    ));
                }
            }
            pass_through(false);
            return;
        }
        // 2026-06-18 MODEL B LIVE-DIALOG (gated, OFF by default). SIBLING to direct_build: instead
        // of FORGING a non-live dialog (which loads the wrong map + crashes), locate the REAL
        // Load-Game registry node and fire its NATIVE run 0x1409aaba0 -> a LIVE registered
        // ProfileLoadDialog the native menu group pumps. own_stepper_live_dialog_fire latches the
        // fire (one-shot), waits for the live dialog at owner+0xe0, then routes to STAGE2 ACTIVATE
        // (load_activate + char-fingerprint-gated continue_confirm). Fail-closed at every step.
        // Checked BEFORE direct_build so enabling live-dialog takes the live path, not the forge.
        if live_dialog_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
        {
            unsafe {
                own_stepper_live_dialog_fire(
                    owner,
                    base,
                    waits,
                    menu_build_timed_out,
                    menu_elapsed_ms,
                )
            };
            pass_through(false);
            return;
        }
        // 2026-06-18 DIRECT BUILD (gated, OFF by default). Once the menu is open, build the
        // ProfileLoadDialog DIRECTLY (factory 0x14081ead0) -- bypassing the input-gated row
        // controller that never constructs headless -- then drive STAGE 2 (mount + guarded
        // continue_confirm). One-shot + fail-closed (validates r8 read-only before the native
        // call). A plain (un-gated) run skips this and stays the safe read-only scan below.
        if direct_build_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_DIRECT_BUILT.load(Ordering::SeqCst) == OWN_STEPPER_DIRECT_BUILT_NO
        {
            unsafe { own_stepper_direct_build(owner, base) };
            pass_through(false);
            return;
        }
        // SAFE DEFAULT (RTTI-corrected, 2026-06-17). The "title-confirm" menu-drive below was built
        // on a MISIDENTIFIED function: 0x14078e1c0 is CommandSelectDialog::Update (an in-game
        // dialog), NOT the TitleTopDialog (owner+0xe0, RTTI vt 0x142b26468) confirm router, so its
        // cursor [+0xb0c] / rows [+0x1290] offsets do not apply here (bd rtti-correction-...). It is
        // now DEMOTED behind legacy_menu_drive_enabled(). A plain own_stepper run must NOT take that
        // wrong route -- it reaches the open menu zero-input and STAYS there (no fire, no SetState,
        // save-safe). The real headless Load path is the own-the-stepper / session-activation route,
        // not driving these fake-menu steppers.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && !own_stepper_passive_enabled()
            && !legacy_menu_drive_enabled()
            && !input_probe_enabled()
            && !inject_nav_enabled()
        {
            if OWN_STEPPER_TITLE_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
                == TITLE_OWNER_SCAN_START_ADDRESS
            {
                append_autoload_debug(format_args!(
                    "own_stepper: menu open zero-input; disproven title-confirm menu-drive is gated OFF (RTTI-corrected) -- STAY at open menu (NO-WRITE). Set er-effects-legacy-disproven-menu-drive.txt to revisit the dead path."
                ));
            }
            // 2026-06-18 RECON-ONLY fingerprint scan for the Load-Game entry, run HERE (the open-menu
            // park is where a plain own_stepper run actually lives -- the dump block further down is
            // unreachable behind this early return). Result discarded -> no latch into
            // MENU_LOAD_GAME_ITEM, no STAGE2 advance -> stays NO-WRITE. Dedicated cap/interval so it
            // logs a handful of times across the ~20s post-open window without spamming.
            if OWN_STEPPER_LOADGAME_SCANS.load(Ordering::SeqCst) < OWN_STEPPER_LOADGAME_SCAN_CAP
                && (waits % STAGE1D_RETRY_INTERVAL) == TITLE_OWNER_SCAN_START_ADDRESS as u64
            {
                OWN_STEPPER_LOADGAME_SCANS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                let _ = unsafe { scan_dialog_for_loadgame(owner, base) };
            }
            if menu_build_timed_out {
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            pass_through(false);
            return;
        }
        // LEGACY / DISPROVEN title-confirm Load -- gated behind legacy_menu_drive_enabled() (OFF by
        // default). Built on titletop-confirm-route-static-validated-no-input-needed-2026, which RTTI
        // later REFUTED (0x14078e1c0 = CommandSelectDialog::Update). fire_titletop_load_entry is
        // self-validating so it fail-closes on the wrong object, but it is the WRONG layer entirely;
        // kept only to revisit the dead path deliberately. Never the default.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && !own_stepper_passive_enabled()
            && legacy_menu_drive_enabled()
        {
            let null = TITLE_OWNER_SCAN_START_ADDRESS;
            let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
                .unwrap_or(null);
            let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
            let cur_vt = if dialog != null {
                unsafe { safe_read_usize(dialog) }.unwrap_or(null)
            } else {
                null
            };
            if cur_vt == pld_vt {
                // The fired Load-Game action already built the ProfileLoadDialog at owner+0xe0.
                OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
                append_autoload_debug(format_args!(
                    "own_stepper: title-confirm built ProfileLoadDialog=0x{dialog:x} at owner+0xe0 -- entering STAGE2 ACTIVATE (slot={want_slot})"
                ));
                pass_through(false);
                return;
            }
            if OWN_STEPPER_TITLE_FIRED.load(Ordering::SeqCst) == null {
                // Not yet fired: attempt the validated fire (fail-closed no-op + retry if the rows
                // are not realized yet -- never writes on a non-realized/contaminated state).
                if unsafe { fire_titletop_load_entry(dialog, base) } {
                    OWN_STEPPER_TITLE_FIRED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                }
                pass_through(false);
                return;
            }
            // Fired; waiting for the ProfileLoadDialog to appear at owner+0xe0. Bounded timeout.
            if menu_build_timed_out {
                append_autoload_debug(format_args!(
                    "own_stepper: title-confirm fired but ProfileLoadDialog not at owner+0xe0 after {waits} polls/{menu_elapsed_ms}ms -- STAY (NO-WRITE)"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            pass_through(false);
            return;
        }
        // Wait for the registered entries to tick: the menu-item Update hook + Sequence-iterator
        // hook capture the Load-Game leaf (functor->dialog_factory) as the native pump ticks
        // them. Fallback: our static tree walk. NO SetState here -> stays at the main menu,
        // save-safe. STAGE 2 (invoke the leaf functor) follows once the live item is confirmed.
        // (REFUTED d180-locate path, retained only for the input-probe/inject-nav diagnostic modes.)
        let hooked = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
        // The real Continue/Load-Game rows are TitleTopDialog entries (NOT FD4 jobs). Once the
        // menu is open, sample the dialog's entry vector a few times as it realizes -- save-safe
        // read-only enumeration that identifies the Load-Game/Continue entries for STAGE 2.
        if OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_TITLETOP_DUMPS.load(Ordering::SeqCst) < OWN_STEPPER_TITLETOP_DUMP_CAP
            && (waits % STAGE1D_RETRY_INTERVAL) == TITLE_OWNER_SCAN_START_ADDRESS as u64
        {
            OWN_STEPPER_TITLETOP_DUMPS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let (tt_load, tt_cont, tt_cursor) = unsafe { dump_titletop_menu_entries(owner, base) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE1b titletop-entries load_game=0x{:x} continue=0x{:x} cursor={tt_cursor} (entries are dialog rows, not FD4 jobs)",
                tt_load.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
                tt_cont.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            ));
        }
        // Search BOTH the owner+0x130 BeginLogo commit target (where the main-menu list with d180
        // actually lands, per the commit fn 0x140b0e530) AND owner+0xe0 (the dialog holder).
        let found = if hooked != TITLE_OWNER_SCAN_START_ADDRESS {
            Some(hooked)
        } else {
            unsafe {
                diagnostic_job_tree_walk(
                    owner,
                    base,
                    TITLE_OWNER_MENU_LIST_130_OFFSET,
                    "list130",
                    false,
                )
            }
            .or_else(|| unsafe {
                diagnostic_job_tree_walk(
                    owner,
                    base,
                    TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                    "built-tree",
                    false,
                )
            })
        };
        match found {
            Some(item) => {
                let _ = unsafe { diagnostic_menu_walk(owner, base, "built-138", true) };
                let _ = unsafe {
                    diagnostic_job_tree_walk(
                        owner,
                        base,
                        TITLE_OWNER_MENU_LIST_130_OFFSET,
                        "list130",
                        true,
                    )
                };
                let _ = unsafe {
                    diagnostic_job_tree_walk(
                        owner,
                        base,
                        TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                        "built-tree",
                        true,
                    )
                };
                // Ensure MENU_LOAD_GAME_ITEM is set (the item may have come from the static
                // tree walk rather than the leaf/iterator hook) so STAGE 2 reads it.
                if MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
                    MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
                }
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE1b LOAD-GAME item identified=0x{item:x} after {waits} waits -- entering STAGE 2 load drive (slot={want_slot}) c30=0x{c30:x} b80={b80}"
                ));
                timeline_event(
                    "T_menu_built",
                    n,
                    format_args!("item=0x{item:x} c30=0x{c30:x}"),
                );
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_INVOKE);
            }
            None => {
                if menu_build_timed_out && !own_stepper_passive_enabled() {
                    let _ = unsafe { diagnostic_menu_walk(owner, base, "built138-timeout", true) };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(
                            owner,
                            base,
                            TITLE_OWNER_MENU_LIST_130_OFFSET,
                            "list130-timeout",
                            true,
                        )
                    };
                    let _ = unsafe {
                        diagnostic_job_tree_walk(
                            owner,
                            base,
                            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
                            "built-tree-timeout",
                            true,
                        )
                    };
                    append_autoload_debug(format_args!(
                        "own_stepper: STAGE1b menu-build TIMEOUT after {waits} polls/{menu_elapsed_ms}ms -- Load-Game item not found; staying at title (NO-WRITE)"
                    ));
                    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
                }
            }
        }
        pass_through(false);
        return;
    }
    if phase == OWN_STEPPER_PHASE_S2_INVOKE
        || phase == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        // STAGE 2: drive the verified menu load (functor -> dialog -> load_activate -> native
        // pump mounts c30=real+ac0+char -> continue_confirm -> SetState(5)). Pass-through each
        // frame so STEP_MenuJobWait keeps the native menu task ticking the registered selector.
        unsafe { own_stepper_stage2(owner, base, gm, want_slot, n, framectx) };
        pass_through(false);
        return;
    }
    // phase DONE: idx6 watches the native load; idx10 just passes through if re-entered.
    pass_through(false);
}
