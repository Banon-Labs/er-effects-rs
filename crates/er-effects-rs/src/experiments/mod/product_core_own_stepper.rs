use super::*;

#[path = "product_core_own_stepper/fallback_drives.rs"]
mod fallback_drives;
use fallback_drives::own_stepper_idx10_fallbacks;

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
pub(crate) use er_telemetry::counters::OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS;
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
pub(crate) use er_telemetry::counters::TFC_FORCED_CONTINUE_HANDOFF_MS;
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

pub(crate) fn mark_own_load_forced_continue_handoff() {
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    let _ = OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.compare_exchange(
        0,
        crate::experiments::boot_view_epoch_ms(),
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
}

pub(crate) fn mark_tfc_forced_continue_handoff() {
    TFC_CONTINUE_FIRED.store(1, Ordering::SeqCst);
    let _ = TFC_FORCED_CONTINUE_HANDOFF_MS.compare_exchange(
        0,
        crate::experiments::boot_view_epoch_ms(),
        Ordering::SeqCst,
        Ordering::SeqCst,
    );
    mark_own_load_forced_continue_handoff();
}
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
pub(crate) use er_telemetry::counters::PRODUCT_CORE_LAST_TITLE_IN_LOOP;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_OWNER_TICKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_READY_BLOCKS;
pub(crate) use er_telemetry::counters::PRODUCT_CORE_READY_SUCCESSES;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_ATTEMPTS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_LAST_STATE_BITS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_STATE_REJECTS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_TABLE_REJECTS;
pub(crate) use er_telemetry::counters::TITLE_OWNER_SCAN_VTABLE_HITS;
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
/// Throttle counter for the dialog+0x50 load-vector readiness gate in `maybe_fire_tfc_continue`
/// (logs the count value occasionally while waiting for it to become a valid has-room vector).
pub(crate) use er_telemetry::counters::TFC_LOAD_VEC_WAIT_TICKS;
/// Trampoline for the hooked TitleTopDialog::update (`title_update_detour` -> original). 0 = not hooked.
pub(crate) use er_telemetry::counters::TITLE_UPDATE_ORIG;

/// Detour for CS::TitleTopDialog::update (0x1409aac10, vtable slot 2). Runs IN THE PUMP'S FRAME with
/// the LIVE dialog (rcx) -- the in-context timing our recurring-game-task build lacked. Calls the
/// original first (the pump sets up the live dialog state + drains the menu jobs), then runs the gated
/// one-shot Continue build (`maybe_fire_tfc_continue`), so it builds with the now-live dialog fields
/// (dialog+0x50 valid -> no mis-context overflow). Build is catch_unwind-wrapped so the pump always
/// proceeds. bd HOOK-DESIGN-titletopdialog-update-0x1409aac10-incontext-build-2026-06-23.
pub(crate) unsafe extern "system" fn title_update_detour(dialog: usize, delta: f32, input: usize) {
    // TitleTopDialog::update reads the global accept byte and invokes open_menu at the tail of this
    // exact frame. Delivering it from the recurring game task races menu handlers that clear the
    // byte before that read; product autoload therefore arms it immediately before the original.
    if product_autoload_enabled()
        && let Ok(base) = game_module_base()
    {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            er_title_flow::maybe_set_title_accept_byte(base)
        }));
    }
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

/// Trampoline to the original PAB node-update. 0 = not hooked.
pub(crate) use er_telemetry::counters::PAB_ADVANCE_ORIG;

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
        if n == 1 || n.is_multiple_of(1200) {
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
#[allow(dead_code)] // Retained: Decoded layout of one native Continue menu entry (entry/functor/do_call/router/index/cursor), beside the live NativeContinueItemAction.
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

pub(crate) use er_title_flow::StartupModalBlockingState;

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
    own_stepper_idx10_fallbacks!(
        owner,
        framectx,
        n,
        base,
        phase,
        gm,
        c30,
        b80,
        want_slot,
        pass_through
    );
}
