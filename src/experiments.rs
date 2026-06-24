//! experiments module (split from lib.rs; pure code reorganization, no behavior change).

#![allow(unused_imports)]

use std::{
    ffi::c_void,
    fmt::Write as _,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex, Once, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use std::os::windows::ffi::OsStrExt as _;

use debug::{InputBlocker, InputFlags};
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
            Foundation::{HINSTANCE, HWND, LPARAM, RECT, WPARAM},
            System::{
                LibraryLoader::{GetModuleHandleA, GetProcAddress},
                Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery},
                SystemServices::DLL_PROCESS_ATTACH,
                Threading::GetCurrentProcessId,
            },
            UI::WindowsAndMessaging::{
                ClipCursor, EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                WM_KEYDOWN, WM_KEYUP,
            },
        },
        core::{BOOL, PCSTR},
    },
};

#[allow(unused_imports)]
use crate::*;
#[allow(unused_imports)]
use crate::{crashlog::*, ffi::*, hooks::*, telemetry::*};

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

static PRODUCT_AUTOLOAD_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Armed from the reliable autoload-file channel (`own_stepper=1` / `cold_char_mount=1` in
/// er-effects-autoload.txt) so the menu-free own-stepper + cold-char-mount paths can be enabled
/// without depending on env-var propagation through Proton or game_directory_path() trigger files.
static OWN_STEPPER_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
static COLD_CHAR_MOUNT_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Armed from the reliable autoload-file channel (`own_load=1` in er-effects-autoload.txt) so the
/// SAVE-SAFE verify-only OWN-LOAD buffer-feed probe (`own_load_drive`) runs without depending on
/// env-var propagation through Proton.
static OWN_LOAD_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Armed from the reliable autoload-file channel (`own_load_continue=1` in er-effects-autoload.txt)
/// so the FINAL guarded `continue_confirm`/`SetState5` world-stream step (after the verify-only
/// `own_load_drive` parse) runs without depending on env-var propagation through Proton.
/// SAVE-WRITING when it fires -- gated hard on a REAL c30 + char fingerprint inside `own_load_drive`.
static OWN_LOAD_CONTINUE_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Armed from the reliable autoload-file channel (`own_dispatch=1` in er-effects-autoload.txt) so the
/// OWN-LOAD m28 direct-enqueue lever (`AddDefaultFileLoadProcess`) runs without depending on env-var
/// propagation through Proton. Defaults OFF; the lever ALSO requires `OWN_LOAD_CONTINUE_FIRED` at fire
/// time, so arming this alone cannot dispatch on a vanilla native menu load. Touches only world-asset
/// file-load streaming -- no save IO, cannot autosave.
static OWN_DISPATCH_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Armed from the reliable autoload-file channel (`own_load_install_job=1` in er-effects-autoload.txt)
/// so the menu-free LoadGame-JOB INSTALL lever runs without depending on env-var propagation through
/// Proton. Defaults OFF. When armed (and `own_load` is armed so `own_load_drive` runs), the verify-only
/// parse is followed by BUILD (`FUN_140826510`) + INSTALL (`FUN_1407a9560`) of the LoadGame
/// MenuJobWithContext into `owner+0x130` -- INSTEAD of the guarded continue_confirm/SetState5. SAVE-SAFE
/// (build + first-tick deser only READ the save; no SetState5, no autosave, no save write).
static OWN_LOAD_INSTALL_JOB_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// Monotonic count of LoadGame-JOB install-lever fires (build + install into owner+0x130). Exposed in
/// telemetry as `oracle_own_load_install_job_fired` so a probe can confirm the lever actually ran.
pub(crate) static OWN_LOAD_INSTALL_JOB_FIRED: AtomicU64 = AtomicU64::new(0);
/// Module-level mirror of `own_load_drive`'s internal phase, stored as `phase + 1` (0 = the probe
/// never ran; PHASE_DONE+1 = terminal, evidence collected). Exposed in telemetry as
/// `oracle_own_load_phase` so the readiness watcher can tear the game down the instant the verify
/// outcome is observed instead of idling to the wall-clock cap.
pub(crate) static OWN_LOAD_PHASE_PUB: AtomicUsize = AtomicUsize::new(0);
/// Module-level mirror of cold_char_mount_drive's internal MOUNT_PHASE, stored as `phase + 1`
/// (0 = the cold mount never ran; 5 = PHASE_DONE = terminal, evidence collected). Exposed in
/// telemetry as `oracle_cold_char_mount_phase` so the readiness watcher can tear the game down the
/// instant the b80 outcome is observed instead of idling to the wall-clock cap.
pub(crate) static COLD_CHAR_MOUNT_PHASE_PUB: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static OWN_LOAD_STREAM_FRAMES: AtomicU64 = AtomicU64::new(0);
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
/// The SetState-able TITLE owner threaded into continue_confirm, cached at fire time so the recurring
/// observer reads the world-stream from the SAME object the load was kicked on (NOT a fresh
/// own_stepper owner, which stops being supplied once the title task dies). 0 == not cached.
pub(crate) static OWN_LOAD_OWNER_CACHED: AtomicUsize = AtomicUsize::new(0);
/// InGameStep = *(owner+TITLE_OWNER_JOB_OFFSET), cached at fire time. It was already non-null at
/// frame 0 (observed 0x7fff21e09a40) so caching it then captures a stable handle the recurring
/// observer can walk to MoveMapStep even after the title task stops running. 0 == not cached.
pub(crate) static OWN_LOAD_INGAMESTEP_CACHED: AtomicUsize = AtomicUsize::new(0);
/// Monotonic frame counter for the RECURRING world-stream observer (advances every game-task frame
/// the observer is active). Distinct from OWN_LOAD_STREAM_FRAMES (which the old own_stepper-sited
/// telemetry also bumps): this is the "frame=N" the recurring observer's debug line prints so the
/// trend across the loading screen is visible.
pub(crate) static OWN_LOAD_STREAM_RECUR_FRAMES: AtomicU64 = AtomicU64::new(0);
/// PATH B (own_load_pump). Armed from the reliable autoload-file channel (`own_load_pump=1` in
/// er-effects-autoload.txt). Defaults OFF. When armed (and `own_load` is armed so `own_load_drive`
/// runs the verify-only parse), the parse is followed by BUILD of the LoadGame `MenuJobWithContext`
/// with REAL mss-derived ctx; the job ptr is then PRIVATELY pumped (its `Run` ticked every frame from
/// the recurring game task) to completion -- WITHOUT installing into owner+0x130 / any queue / the
/// CSMenuMan dialog stack. After the pumped job reaches `state==Success`, the guarded SetState5
/// transition fires ONCE to drive title->ingame. Takes precedence over own_load_install_job /
/// own_load_continue. (autoload-world-load-coupled-to-csmenuman-dialog-verdict-2026-06-22)
static OWN_LOAD_PUMP_FILE_ARMED: AtomicUsize = AtomicUsize::new(0);
/// The built LoadGame job pointer the recurring task pumps each frame. 0 == not built / not armed.
/// Set once by `own_load_pump_fire`; read+ticked by the recurring observer's sibling pump.
pub(crate) static OWN_LOAD_PUMP_JOB: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static OWN_LOAD_PUMP_FIRED: AtomicU64 = AtomicU64::new(0);
/// Set true once the pumped job reached a terminal state (Success/Failed) AND the one-shot transition
/// was handled, so we never re-pump or re-transition. Exposed as `oracle_own_load_pump_done`.
pub(crate) static OWN_LOAD_PUMP_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static PRODUCT_CORE_AUTOLOAD_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRODUCT_CORE_READY_BLOCKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRODUCT_CORE_READY_SUCCESSES: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRODUCT_CORE_OWNER_TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRODUCT_CORE_LAST_OWNER: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_TITLE_DIALOG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_TITLE_DIALOG_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_TITLE_IN_LOOP: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PRODUCT_CORE_LAST_MENU_OPENED_LATCH: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PRESS_START_PROXY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PRESS_START_VT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PRESS_START_CONTEXT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static PRODUCT_CORE_LAST_PHASE: AtomicUsize = AtomicUsize::new(OWN_STEPPER_PHASE_MENU);
pub(crate) static PRODUCT_CORE_LAST_BLOCKER: AtomicUsize =
    AtomicUsize::new(PRODUCT_CORE_BLOCKER_UNSEEN);
pub(crate) static TITLE_OWNER_SCAN_ATTEMPTS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TITLE_OWNER_SCAN_VTABLE_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TITLE_OWNER_SCAN_TABLE_REJECTS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TITLE_OWNER_SCAN_STATE_REJECTS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TITLE_OWNER_SCAN_LAST_CANDIDATE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_OWNER_SCAN_LAST_TABLE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_OWNER_SCAN_LAST_STATE_BITS: AtomicUsize = AtomicUsize::new(usize::MAX);
static MENU_CONTINUE_ENTRY: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static MENU_CONTINUE_ITEM: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_WINDOW_JOB_CTOR_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static MENU_CONTINUE_IDLE_INSERT_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static TASK_ENQUEUE_GENERIC_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static MENU_ITEM_UPDATE_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_ITEM_UPDATE_SEMANTIC_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static MENU_CONTINUE_CANDIDATE_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES: AtomicU64 = AtomicU64::new(0);
pub(crate) static MENU_CONTINUE_CANDIDATE_LAST_ACCEPT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_HITS: AtomicU64 = AtomicU64::new(0);
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
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_MASKED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_NATIVE_READY_PREDICATE_LAST_RET: AtomicUsize = AtomicUsize::new(0);
static B80_NATIVE_DISPATCHER_OWNER: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static MENU_CONTINUE_ITEM_FIELD_LOG_COUNT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static B80_DISPATCHER2_OBSERVE_COUNT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static B80_DISPATCHER2_OBSERVE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static MENU_CONTINUE_FUNCTOR: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static MENU_CONTINUE_DOCALL: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static MENU_CONTINUE_ROUTER: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static MENU_CONTINUE_INDEX: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
static AUTOLOAD_PHASE_EPOCH: OnceLock<Instant> = OnceLock::new();
static OWN_STEPPER_MENU_BUILD_STARTED_MS: AtomicU64 = AtomicU64::new(PHASE_TIMER_UNSET_MS);
static OWN_STEPPER_S2_PHASE_STARTED_MS: AtomicU64 = AtomicU64::new(PHASE_TIMER_UNSET_MS);

const PHASE_TIMER_UNSET_MS: u64 = u64::MAX;
const PHASE_TIMER_ZERO_MS: u64 = 0;
const U64_MAX_AS_U128: u128 = u64::MAX as u128;

const PROFILE_SLOT_ACTIVATE_RVA: usize = ProfileLoadMenuRva::ProfileSlotActivate as usize;
const PROFILE_LOAD_SELECTOR_TICK_RVA: usize = ProfileLoadMenuRva::ProfileLoadSelectorTick as usize;

fn autoload_phase_elapsed_ms() -> u64 {
    let elapsed = AUTOLOAD_PHASE_EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis();
    if elapsed > U64_MAX_AS_U128 {
        u64::MAX
    } else {
        elapsed as u64
    }
}

fn reset_phase_timer(timer: &AtomicU64) {
    timer.store(autoload_phase_elapsed_ms(), Ordering::SeqCst);
}

fn phase_elapsed_ms(timer: &AtomicU64) -> u64 {
    let started = timer.load(Ordering::SeqCst);
    if started == PHASE_TIMER_UNSET_MS {
        reset_phase_timer(timer);
        PHASE_TIMER_ZERO_MS
    } else {
        autoload_phase_elapsed_ms().saturating_sub(started)
    }
}

fn own_stepper_enter_menu_build_phase() {
    OWN_STEPPER_MENU_BUILD_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    reset_phase_timer(&OWN_STEPPER_MENU_BUILD_STARTED_MS);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU_BUILD, Ordering::SeqCst);
}

fn own_stepper_menu_build_timed_out() -> bool {
    phase_elapsed_ms(&OWN_STEPPER_MENU_BUILD_STARTED_MS) >= OWN_STEPPER_MENU_BUILD_WAIT_MAX
}

fn own_stepper_menu_build_elapsed_ms() -> u64 {
    phase_elapsed_ms(&OWN_STEPPER_MENU_BUILD_STARTED_MS)
}

fn own_stepper_enter_s2_phase(phase: usize) {
    OWN_STEPPER_S2_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    reset_phase_timer(&OWN_STEPPER_S2_PHASE_STARTED_MS);
    OWN_STEPPER_PHASE.store(phase, Ordering::SeqCst);
}

fn own_stepper_s2_timed_out() -> bool {
    phase_elapsed_ms(&OWN_STEPPER_S2_PHASE_STARTED_MS) >= OWN_STEPPER_S2_PHASE_MAX
}

fn own_stepper_s2_elapsed_ms() -> u64 {
    phase_elapsed_ms(&OWN_STEPPER_S2_PHASE_STARTED_MS)
}

pub(crate) fn arm_product_autoload_from_request(request: &SaveLoader) {
    // Arm the menu-free path flags from the reliable autoload-file channel, independent of slot
    // and method, so own_stepper_enabled()/cold_char_mount_enabled() do not depend on env-var
    // propagation through Proton or game_directory_path() trigger-file resolution.
    if request.own_stepper() {
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.cold_char_mount() {
        COLD_CHAR_MOUNT_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load() {
        // own_load drives through the idx10 detour (own_stepper_idx10), so arm the own_stepper file
        // flag too -- that is what makes own_stepper_patch_once install the detour so OUR handler
        // runs each frame. own_load takes precedence inside the handler (like cold_char_mount).
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_continue() {
        // The final guarded world-stream step rides on the SAME own_load probe (own_load_drive runs
        // the proven verify-only parse, then fires the guarded continue). Arm own_load too so the
        // probe actually runs even if only own_load_continue was set in the autoload file.
        OWN_LOAD_CONTINUE_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_dispatch() {
        // The m28 direct-enqueue lever rides the SAME OWN-LOAD path: it only fires AFTER our
        // continue_confirm sets OWN_LOAD_CONTINUE_FIRED. Arm own_load + own_load_continue too so the
        // path that sets that flag actually runs when only own_dispatch was set in the autoload file.
        OWN_DISPATCH_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_CONTINUE_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_install_job() {
        // The LoadGame-JOB INSTALL lever rides the SAME OWN-LOAD path: it runs INSTEAD of the
        // continue_confirm/SetState5 step at the END of own_load_drive. Arm own_load (+ own_stepper,
        // which installs the idx10 detour that runs own_load_drive) so the probe actually runs even if
        // only own_load_install_job was set in the autoload file. Deliberately does NOT arm
        // own_load_continue (the save-writing SetState5 lever): this is the non-SetState5 alternative.
        OWN_LOAD_INSTALL_JOB_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    if request.own_load_pump() {
        // PATH B PRIVATE-PUMP lever ("own the load"): builds the LoadGame job with REAL mss-derived ctx
        // then ticks its Run privately each frame to completion + drives the transition on Success. Rides
        // the SAME OWN-LOAD path: it runs INSTEAD of the install/continue step at the END of
        // own_load_drive. Arm own_load (+ own_stepper, which installs the idx10 detour that runs
        // own_load_drive) so the probe actually runs even if only own_load_pump was set in the autoload
        // file. Does NOT arm own_load_continue here -- the pump fires the guarded SetState5 transition
        // itself only after the pumped job reaches Success.
        OWN_LOAD_PUMP_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_LOAD_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        OWN_STEPPER_FILE_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let Some(slot) = request.slot() else {
        return;
    };

    if slot < OWN_STEPPER_SLOT_ZERO {
        return;
    }

    // OWN_STEPPER_SLOT is the shared target slot for the menu-free own_stepper /
    // native_fullread / cold_char_mount paths AND the menu-driven product_core path. Set it
    // whenever a valid slot is configured, regardless of method, so the menu-free paths (which
    // deliberately do NOT arm product_autoload, to avoid the open_menu self-fire that builds the
    // ToS) still receive the slot. Only DirectMenuLoad arms product_core (which self-fires
    // open_menu 0x1409b24e0 and therefore constructs the ToS MenuWindowJob).
    OWN_STEPPER_SLOT.store(slot, Ordering::SeqCst);
    if request.method() == SaveLoadMethod::DirectMenuLoad {
        PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
}

pub(crate) fn product_autoload_enabled() -> bool {
    PRODUCT_AUTOLOAD_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
}

pub(crate) fn product_core_ready_blocker_label(blocker: usize) -> &'static str {
    match blocker {
        PRODUCT_CORE_BLOCKER_UNSEEN => "unseen",
        PRODUCT_CORE_BLOCKER_READY => "ready",
        PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER => "no_title_owner",
        PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE => "title_owner_state",
        PRODUCT_CORE_BLOCKER_TITLE_TABLE => "title_table",
        PRODUCT_CORE_BLOCKER_SESSION => "session",
        PRODUCT_CORE_BLOCKER_GAME_DATA_MAN => "game_data_man",
        PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY => "profile_summary",
        PRODUCT_CORE_BLOCKER_IODEV => "iodev",
        PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR => "heap_allocator",
        PRODUCT_CORE_BLOCKER_TITLE_DIALOG => "title_dialog",
        PRODUCT_CORE_BLOCKER_PRESS_START => "press_start",
        PRODUCT_CORE_BLOCKER_TITLE_STATE => "title_state",
        _ => "unknown",
    }
}

pub(crate) fn game_module_base() -> Result<usize, String> {
    let module = unsafe { GetModuleHandleA(PCSTR::null()) }
        .map_err(|error| format!("failed to resolve game module: {error}"))?;
    Ok(module.0 as usize)
}

pub(crate) fn game_rva(rva: u32) -> Result<usize, String> {
    Ok(game_module_base()? + rva as usize)
}

/// Kill-switch to skip installing the continue_trace hooks (bisecting a ~19s
/// title crash caused by our DLL). When set, the continue/load-flow hooks are
/// not installed even if autoload is configured.
/// Bisect kill-switch: when set, the recurring game task does nothing each
/// frame, so we can tell whether the per-frame task body or the DLL's mere
/// presence is what terminates the title ~19s in.
pub(crate) fn inert_mode() -> bool {
    matches!(std::env::var("ER_EFFECTS_INERT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-inert.txt")
            .exists()
}

/// Bisect kill-switch: the recurring task does lock + tick only, with no
/// filesystem I/O. Lets us tell whether the per-frame file I/O (telemetry write)
/// is what stalls the title vs. any per-frame work at all.
pub(crate) fn lite_mode() -> bool {
    matches!(std::env::var("ER_EFFECTS_LITE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-lite.txt")
            .exists()
}

pub(crate) fn continue_trace_disabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NO_CONTINUE_TRACE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-no-continue-trace.txt")
        .exists()
}

pub(crate) fn trace_continue_enabled() -> bool {
    product_autoload_enabled()
        || matches!(
            std::env::var("ER_EFFECTS_TRACE_CONTINUE").as_deref(),
            Ok("1")
        )
        || trace_continue_default_path().exists()
        || PathBuf::from("er-effects-trace-continue.txt").exists()
}

pub(crate) fn trace_menu_task_update_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TRACE_MENU_TASK_UPDATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-trace-menu-task-update.txt")
        .exists()
}

pub(crate) fn native_title_job_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-title-job.txt")
        .exists()
}

pub(crate) fn force_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_AUTOLOAD_FORCE_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-force-play-game.txt")
        .exists()
}

pub(crate) fn selectbot_probe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SELECTBOT_PROBE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-selectbot-probe.txt")
        .exists()
}

/// Read-only runtime validation for the SelectBot selection-injection lane.
///
/// Static RE (runs 300/301) decoded the pump's selection path but the SelectBot
/// registry is FromSoftware's internal test-automation channel, so it may be
/// empty/inactive in the retail build. Before reversing the registry write API
/// and attempting an injection, this samples the live state each frame: the
/// SimpleTitleStep owner state (+0x4c), title queue (+0x128), parsed selection
/// (+0x130), the registry root pointer ([0x143d87360]) and the load-active gate
/// byte ([0x143d856a0]). It never writes game memory. A non-null registry with
/// an idle pump (state stable, queue/selection empty, gate 0) confirms the
/// injection target is real and reachable; a null registry means the SelectBot
/// harness is not initialized and the lane needs a different entry.
pub(crate) unsafe fn selectbot_probe_once(module_base: usize, tick: u64) {
    if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS as u64 {
        return;
    }
    // Owner-independent module globals: sample these ALWAYS. After the latch
    // advances the inner TitleStep to Finish (state 11 -> -1) the inner owner is
    // torn down, but `pump_ran` (does the outer MenuLoop spin up?) and the latch
    // byte live in module globals, so we must still capture them post-cascade.
    let registry = unsafe { *((module_base + SELECTBOT_REGISTRY_GLOBAL_RVA) as *const usize) };
    let load_gate = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let input_manager =
        unsafe { *((module_base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) as *const usize) };
    let pump_ran = if input_manager != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { *((input_manager + SELECTBOT_PUMP_RAN_FLAG_OFFSET) as *const u8) }
    } else {
        DIRECT_INPUT_FAILURE_HRESULT as u8
    };
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        append_autoload_debug(format_args!(
            "selectbot_probe: owner not resolved registry={registry:#x} load_gate={load_gate} input_mgr={input_manager:#x} pump_ran={pump_ran} tick={tick}"
        ));
        return;
    };
    let state = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    let queue128 = unsafe { *(owner.add(SELECTBOT_OWNER_TITLE_QUEUE_128_OFFSET) as *const usize) };
    let selection130 =
        unsafe { *(owner.add(SELECTBOT_OWNER_PARSED_SELECTION_130_OFFSET) as *const i32) };
    append_autoload_debug(format_args!(
        "selectbot_probe: state={state} queue128={queue128:#x} selection130={selection130} registry={registry:#x} load_gate={load_gate} input_mgr={input_manager:#x} pump_ran={pump_ran} tick={tick}"
    ));
    // Lever-1 title-accept experiment: set the proceed latch [0x143d856a0]=1 ONCE,
    // only while the inner owner is confirmed at MenuJobWait (state 10), so the
    // native MenuJobWait handler advances itself to state 11 (Finish) on its next
    // tick. Sampling continues above so the cascade (state, pump_ran, registry) is
    // observed after the write. Gated separately from the read-only probe.
    if title_proceed_gate_enabled()
        && state == TITLE_STEP_MENU_JOB_WAIT_STATE
        && !TITLE_PROCEED_GATE_FIRED.swap(true, Ordering::SeqCst)
    {
        unsafe {
            *((module_base + SELECTBOT_LOAD_GATE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
        }
        let after = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
        append_autoload_debug(format_args!(
            "title_proceed_gate: set [0x143d856a0]={after} at state {state} tick={tick}"
        ));
    }
    // Lever-2 (option c): satisfy the global menu-accept side-effect zero-input. At the parked
    // press-any-button title (state 10), set the global accept byte 0x144589bdc=1 ONCE so the
    // native TitleTopDialog::update runs the open-menu registrar on its own next tick -- the
    // NATURAL advance (builds Continue/Load + transfers focus -> select-layer/router_this), which
    // a direct registrar self-fire could not do without spawning a competing dialog that reverted.
    // Not an input event (this is the decoded accept flag, like the ToS-accepted flag). Gated OFF
    // by default. Sampling above continues so the cascade (menu_opened, router_this) is observed.
    if title_accept_byte_gate_enabled()
        && state == TITLE_STEP_MENU_JOB_WAIT_STATE
        && !TITLE_ACCEPT_BYTE_GATE_FIRED.swap(true, Ordering::SeqCst)
    {
        unsafe {
            *((module_base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) =
                TITLE_PROCEED_GATE_SET_VALUE;
        }
        let after = unsafe { *((module_base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *const u8) };
        append_autoload_debug(format_args!(
            "title_accept_byte_gate: set [0x144589bdc]={after} at state {state} tick={tick} -- zero-input natural menu-open"
        ));
    }
}

/// Operator gate for the zero-input global-accept-byte title-advance lever (option c). Default OFF.
pub(crate) fn title_accept_byte_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_ACCEPT_BYTE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-accept-byte.txt")
        .exists()
}

/// Operator gate for lever-3 (narrow registrar advance): set the menu-transition singleton flag
/// 0x143d5dea8->+0=1 before the validated open-menu self-fire, replicating the native title
/// press-accept handler so the menu opens in place without the ToS over-trigger. Default OFF;
/// used together with own_stepper + self-fire.
pub(crate) fn title_registrar_advance_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_REGISTRAR_ADVANCE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-registrar-advance.txt")
        .exists()
}

pub(crate) fn title_proceed_gate_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_PROCEED_GATE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-proceed-gate.txt")
        .exists()
}

pub(crate) fn ingamestep_pump_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMESTEP_PUMP").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingamestep-pump.txt")
        .exists()
}

/// Directly drives the orphaned InGameStep load to completion, called once per
/// game-thread frame from the recurring CSTask (NOT a hook — detouring the hot
/// step pump `0x140b0bd60` froze the title state machine, run 305).
///
/// `force_play_game` advances the inner TitleStep to GameStepWait (state 6) and
/// submits the load (`job+0xd8=1`), but the InGameStep step machine is a
/// parent-ticked child the title scheduler never routes to in the forced state,
/// so the load orphans. The InGameStep's own Execute pump is `0x140b0bd60`
/// (FD4StepTemplate::Execute, signature `execute(&mut self, &FD4TaskData)`), so
/// we call it directly on the InGameStep (`owner+0x2e8`) with the live
/// `FD4TaskData` the CSTask already supplies — the exact ctx the task system
/// would pass. The step handlers drain `job+0xd8` 1 -> 2 -> 0 and load the world.
pub(crate) unsafe fn ingamestep_pump_tick(module_base: usize, task_data: &FD4TaskData) {
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let inner_state = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    if inner_state != TITLE_STEP_GAME_STEP_WAIT {
        return;
    }
    let ingame = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const *mut u8) };
    if ingame.is_null() {
        return;
    }
    // Sample the InGameStep step machine. step_state (+0x48) is the CURRENT step,
    // next (+0x4c) is where it wants to go: if next advances while cur lags, the
    // machine IS progressing (real wait is downstream). The override fields
    // (+0x69/+0xa8/+0xac) reveal whether the pump force-re-stamps the step index
    // each frame (which would pin it). Log on change of (next, d8) to trace it.
    let cur = unsafe { *(ingame.add(INGAMESTEP_STEP_STATE_OFFSET) as *const i32) };
    let next = unsafe { *(ingame.add(INGAMESTEP_NEXT_STATE_OFFSET) as *const i32) };
    let d8 = unsafe { *(ingame.add(TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
    let ov_trigger = unsafe { *(ingame.add(INGAMESTEP_OVERRIDE_TRIGGER_OFFSET)) };
    let ov_guard = unsafe { *(ingame.add(INGAMESTEP_OVERRIDE_GUARD_OFFSET)) };
    let ov_target = unsafe { *(ingame.add(INGAMESTEP_OVERRIDE_TARGET_OFFSET) as *const i32) };
    let last_next = INGAMESTEP_PUMP_LAST_NEXT.swap(next, Ordering::SeqCst);
    let last_d8 = INGAMESTEP_PUMP_LAST_D8.swap(d8, Ordering::SeqCst);
    if next != last_next || d8 != last_d8 {
        append_autoload_debug(format_args!(
            "ingamestep_pump: cur={cur} next={next} d8={d8} ov_trigger={ov_trigger} ov_guard={ov_guard} ov_target={ov_target} ingame={ingame:p}"
        ));
    }
    if cur == INGAMESTEP_FINISHED_SENTINEL || d8 == INGAMESTEP_LOAD_DONE {
        return;
    }
    // Gated, one-shot "unpin": if the force-state override is re-stamping the step
    // index (trigger set, target == current stalled step), clear the trigger so
    // the natural step advance sticks. Read-only by default; opt in via
    // ER_EFFECTS_INGAMESTEP_UNPIN once the log confirms the machine is pinned.
    if ingamestep_unpin_enabled()
        && ov_trigger != INGAMESTEP_OVERRIDE_TRIGGER_CLEAR
        && ov_target == cur
        && !INGAMESTEP_UNPIN_DONE.swap(true, Ordering::SeqCst)
    {
        unsafe {
            *(ingame.add(INGAMESTEP_OVERRIDE_TRIGGER_OFFSET)) = INGAMESTEP_OVERRIDE_TRIGGER_CLEAR;
        }
        append_autoload_debug(format_args!(
            "ingamestep_pump: cleared force-override trigger (was {ov_trigger}, target={ov_target}) cur={cur} ingame={ingame:p}"
        ));
    }
    let Ok(pump) = game_rva(STEP_PUMP_DRIVER_RVA) else {
        return;
    };
    let pump: unsafe extern "system" fn(*mut u8, *const FD4TaskData) -> usize =
        unsafe { std::mem::transmute(pump) };
    let _ = unsafe { pump(ingame, task_data as *const FD4TaskData) };
}

pub(crate) fn native_autoload_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_AUTOLOAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-autoload.txt")
        .exists()
}

/// Recipe A: arm the game's OWN built-in title autoload with zero input.
///
/// The save-manager per-frame update `0x14067f5d0` performs an autoload when the
/// save slot (`GameMan+0xac0`) is set AND the force flag `0x143d856a0` is non-zero
/// — it primes the world/streaming subsystems through the game's own state
/// machine (which `force_play_game` bypassed). So we set the slot via the native
/// setter `0x67a810` and raise the force flag ONCE, then let the engine load.
/// The earlier crash from raising that flag came from leaving the slot at -1 (a
/// Finish teardown with no load armed); arming the slot first is the fix.
pub(crate) unsafe fn native_autoload_once(module_base: usize, slot: i32, tick: u64) {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return;
    }
    let game_man = game_man_ptr_or_null();
    if game_man == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let load_in_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    if NATIVE_AUTOLOAD_ARMED.load(Ordering::SeqCst) {
        // Observe the load cascade after arming.
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            let slot_now =
                unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
            let load14 =
                unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
            let latch = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
            let b72 = unsafe { *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8) };
            let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
            append_autoload_debug(format_args!(
                "native_autoload: observe slot={slot_now} b80={load_in_progress} load14={load14} latch={latch} b72={b72} csfeman=0x{csfeman:x} tick={tick}"
            ));
        }
        return;
    }
    if load_in_progress != TITLE_NATIVE_JOB_TASK_DATA_ZERO {
        append_autoload_debug(format_args!(
            "native_autoload: load already in progress (b80={load_in_progress}) before arm; skipping tick={tick}"
        ));
        return;
    }
    // CORRECTED recipe (native-continue-and-slotn-recipe-2026): the latch
    // 0x143d856a0 must stay CLEAR; the arm flag is [GameMan+0xb72]=1. (The old
    // code set the latch to 1, which the disasm proves aborts the load.)
    let latch_before = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(slot) };
    let slot_after = unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
    unsafe {
        *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
    }
    NATIVE_AUTOLOAD_ARMED.store(true, Ordering::SeqCst);
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    append_autoload_debug(format_args!(
        "native_autoload: armed slot={slot_after} b72=1 latch_left={latch_before} b80={load_in_progress} csfeman=0x{csfeman:x} tick={tick}"
    ));
}

pub(crate) fn observe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_OBSERVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-observe.txt")
            .exists()
}

pub(crate) fn own_stepper_enabled() -> bool {
    product_autoload_enabled()
        || OWN_STEPPER_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(std::env::var("ER_EFFECTS_OWN_STEPPER").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-stepper.txt")
            .exists()
}

/// OBSERVE-ONLY NATIVE-LOAD gate (corrected-autoload-design-observe-not-force-native-load-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_LOAD=1` OR a GAME_DIR file
/// `er-effects-native-load.txt`. Mirrors `own_stepper_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog
/// menu is rendered + settled, it fires the native Load-Game MenuMemberFuncJob node's run
/// 0x1409aaba0 exactly once -- testing whether that loads the real char in a NATURAL (non-forced)
/// menu. NO SetState(2/3), NO beginlogo-gate clear, NO registrar self-fire, NO direct_build /
/// cold_char_mount. De-risks design step 4.
pub(crate) fn native_load_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_NATIVE_LOAD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-native-load.txt")
            .exists()
}

/// OBSERVE-ONLY NATIVE-CONTINUE gate (PATH B, autoload-path-B-drive-native-load-chosen-2026-06-22).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_CONTINUE=1` OR a GAME_DIR file
/// `er-effects-native-continue.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test, OR the own-stepper opens the menu),
/// and ONCE the live TitleTopDialog menu is rendered + settled, it fires the native CONTINUE
/// (load-most-recent) MenuMemberFuncJob node's run 0x1409aaba0 exactly once -- which drives the FULL
/// native load (parse + world-asset streaming + spawn). NO SetState(2/3), NO beginlogo-gate clear,
/// NO registrar self-fire, NO direct_build / cold_char_mount. Observe + one-shot fire only.
pub(crate) fn native_continue_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_CONTINUE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-continue.txt")
        .exists()
}

/// OBSERVE-ONLY NATIVE FULL-SAVE-READ gate (native-full-save-read-slot-resolve-chain-observe-recipe-2026).
/// OFF by default; enable via env `ER_EFFECTS_NATIVE_FULLREAD=1` OR a GAME_DIR file
/// `er-effects-native-fullread.txt`. Mirrors `native_load_enabled` (env OR file). When ON, the idx10
/// handler installs the patch (so OUR handler runs each frame) but does NOT force the title state
/// machine: it lets OWN_STEPPER_ORIG_IDX10 pass-through advance the native boot naturally (the user
/// drives past press-any-button + modals in this hybrid test), and ONCE the live TitleTopDialog menu
/// is rendered + settled, it runs the native full-save-read load chain directly at the live menu --
/// where the FD4 IO worker pool is LIVE so the submit drains (SUBMIT -> DRAIN_POLL -> DESER -> GUARD
/// -> CONFIRM). NO SetState forcing for boot, NO selector-step pump (probe-12 crash). The sole save
/// write (continue_confirm 0x140b0e180 -> SetState5) is HARD-gated behind the step-6 guard AND the
/// separate commit sub-gate `native_fullread_commit_enabled` (default = VERIFY-ONLY).
pub(crate) fn native_fullread_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_FULLREAD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-fullread.txt")
        .exists()
}

/// COMMIT sub-gate for the native full-save-read chain (REQUIRED to actually fire continue_confirm
/// 0x140b0e180 -> SetState5, the SOLE save write). OFF by default; enable via env
/// `ER_EFFECTS_FULLREAD_COMMIT=1` OR a GAME_DIR file `er-effects-fullread-commit.txt`. Without it the
/// chain stops at the step-6 GUARD (deserialize + guard + log only): save-safe, NO continue_confirm,
/// NO SetState5. This lets a first test run VERIFY-ONLY (default) before any save write.
pub(crate) fn native_fullread_commit_enabled() -> bool {
    product_autoload_enabled()
        || matches!(
            std::env::var("ER_EFFECTS_FULLREAD_COMMIT").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-fullread-commit.txt")
            .exists()
}

/// OPT-IN post-world native TitleTopDialog cleanup. Static trace of 0x1409a8890 shows this is the
/// real dialog cleanup body: it clears active-screen renderers and releases dialog-owned resources.
/// It fires only after PlayerIns exists, so it cannot participate in save/load success.
pub(crate) fn cleanup_title_dialog_after_world_enabled() -> bool {
    product_autoload_enabled()
        || matches!(
            std::env::var("ER_EFFECTS_CLEANUP_TITLE_DIALOG_AFTER_WORLD").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-cleanup-title-dialog-after-world.txt")
            .exists()
}

pub(crate) unsafe fn cleanup_title_dialog_after_world_once(module_base: usize, frame: u64) {
    static TITLE_DIALOG_CLEANUP_DONE: AtomicUsize =
        AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
    if !cleanup_title_dialog_after_world_enabled()
        || TITLE_DIALOG_CLEANUP_DONE.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let owner = unsafe { title_owner(module_base) };
    let Some(owner_ptr) = owner else {
        append_autoload_debug(format_args!(
            "title-dialog-cleanup: skipped frame={frame} no title owner"
        ));
        return;
    };
    let owner_addr = owner_ptr as usize;
    let dialog = unsafe { safe_read_usize(owner_addr + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let dialog_vt = if dialog != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(dialog) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    if dialog_vt != module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "title-dialog-cleanup: skipped frame={frame} dialog=0x{dialog:x} vt=0x{dialog_vt:x} expected=0x{:x}",
            module_base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return;
    }
    let cleanup: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_CLEANUP_RVA) };
    let ret = unsafe { cleanup(dialog) };
    let mut remaining_slots = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut idx = ACTIVE_SCREEN_SLOT_START;
    while idx < ACTIVE_SCREEN_ARRAY_SLOTS {
        let slot = module_base + ACTIVE_SCREEN_ARRAY_RVA + idx * ACTIVE_SCREEN_ARRAY_STRIDE;
        let ptr = unsafe { safe_read_usize(slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
            remaining_slots += ACTIVE_SCREEN_SLOT_STEP;
        }
        idx += ACTIVE_SCREEN_SLOT_STEP;
    }
    append_autoload_debug(format_args!(
        "title-dialog-cleanup: called 0x{:x} frame={frame} owner=0x{owner_addr:x} dialog=0x{dialog:x} ret=0x{ret:x} remaining_active_slots={remaining_slots}",
        module_base + TITLE_TOP_DIALOG_CLEANUP_RVA
    ));
}

/// OPT-IN gate for the MenuWindow-latch diagnostic hook (SceneObjProxy ctor 0x14074a700).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the latch is needed, via env `ER_EFFECTS_MENU_WINDOW_LATCH=1` OR a GAME_DIR file
/// `er-effects-menu-window-latch.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: this hook was previously installed UNCONDITIONALLY at process-attach and was
/// NOT present in the prior working cold-mount run; gating it lets us isolate hook-induced
/// mount perturbation (see bd probe11 caveat).
pub(crate) fn menu_window_latch_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_MENU_WINDOW_LATCH").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-menu-window-latch.txt")
        .exists()
}

/// OPT-IN gate for the c30-writer diagnostic hook (hot deserialize-internal 0x67bd70).
/// OFF by default: a clean run installs NO MinHook / NO detour for this. Enable only when
/// the diagnostic is needed, via env `ER_EFFECTS_C30_DIAG=1` OR a GAME_DIR file
/// `er-effects-c30-diag.txt`. Mirrors `own_stepper_enabled` (env OR file).
/// Rationale: a trampoline on the HOT 0x67bd70 deserialize path may itself perturb the
/// mount (b80 stuck / crash); gating it lets us run without it to isolate (bd probe11).
pub(crate) fn c30_writer_diag_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_C30_DIAG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-c30-diag.txt")
            .exists()
}

/// PASSIVE own-stepper: do NOT force the menu (no SetState(2)/self-fire) and do NOT block input.
/// The user navigates to Load Game once (the input that surfaces the input-gated d180); the
/// capture hooks grab d180; then STAGE 2 drives mount->confirm->load. This both PROVES the load
/// (correct + faster than manual slot-select) and lets the iterator log the menu-structure change
/// so the pump-switch can be replayed zero-input later. File: er-effects-passive.txt.
pub(crate) fn own_stepper_passive_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_PASSIVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-passive.txt")
            .exists()
}

/// DETERMINISTIC MENU INPUT PROBE (er-effects-input-probe.txt / ER_EFFECTS_INPUT_PROBE). After the
/// menu opens, inject one Down tap then (after an observation window) one Confirm tap, at frames WE
/// choose -- so we know exactly the frame to break on. Decisive question: does the Load-Game leaf
/// d180 tick its leaf Update on HIGHLIGHT alone (Down, no Confirm yet), or only at Confirm? Targeted
/// input used purely as a MEASUREMENT oracle (NOT the zero-input deliverable).
pub(crate) fn input_probe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INPUT_PROBE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-input-probe.txt")
            .exists()
}

/// SELF-DRIVEN GAMEPAD NAV INJECTION (er-effects-inject-nav.txt / ER_EFFECTS_INJECT_NAV). When on,
/// the input block stays engaged PAST menu-open (user input fully suppressed) and the XInput hook
/// fabricates a D-pad Down nav schedule at the gamepad poll source, cycling the title-menu cursor
/// so the input/focus-gated row populate fires and the row-push/csmenu-ctor hooks capture its
/// trigger -- uncontaminated by user input. Capture-only (Down nav, never Confirm).
pub(crate) fn inject_nav_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_INJECT_NAV").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-inject-nav.txt")
            .exists()
}

/// DISPROVEN/LEGACY menu-drive escape hatch -- deliberately OFF by default and HARD to trigger.
///
/// The own_stepper "title-confirm" Load drive (fire_titletop_load_entry + the d180-locate walk) was
/// built on a MISIDENTIFIED function: RTTI on the dearxan-deobfuscated image proved 0x14078e1c0 is
/// `CommandSelectDialog::Update` (an in-game dialog), NOT the title menu's confirm router, so its
/// offsets (cursor [+0xb0c], rows [+0x1290]) do NOT apply to the TitleTopDialog at owner+0xe0
/// (RTTI vt 0x142b26468). See bd rtti-correction-0x14078e1c0-is-commandselectdialog-not-title-
/// confirm-2026. We keep the code (it still has diagnostic value) but it must NEVER be the default
/// path: a fresh session running plain own_stepper must not take this wrong route. The trigger name
/// is intentionally obscure so it cannot be stumbled into -- enable ONLY to revisit the dead path.
pub(crate) fn legacy_menu_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_LEGACY_DISPROVEN_MENU_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-legacy-disproven-menu-drive.txt")
        .exists()
}

/// WORLD-RES STREAMING-DRIVER COLD-BUILD PROBE gate (env ER_EFFECTS_WORLDRES_COLDBUILD /
/// er-effects-worldres-coldbuild.txt). OFF by default. When on, own_stepper runs a ONE-SHOT,
/// SAVE-SAFE probe at the parked title that cold-builds the CSEmkResManImp streaming driver
/// (0x143d7c088) + registers the stream worker (0x144842d40) via the CSResStep tick getter
/// 0x140cd6c50 with a stub `this` -- NO SetState, NO world load, zero save-write risk. See bd
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026.
pub(crate) fn worldres_coldbuild_probe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_WORLDRES_COLDBUILD").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-worldres-coldbuild.txt")
        .exists()
}

/// SAVE-SAFE one-shot cold-build probe of the world-resource streaming driver. Validates the lever
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026 live, WITHOUT SetState / world load.
/// The CSResStep tick getter 0x140cd6c50's body is context-free (builds the EMK resman cluster via
/// global RIP-relative stores + boot allocators; `this`/rsi is touched ONLY at prologue/tail). The
/// tail registers the stream worker when [this+0x48] >= 6. So a zeroed stub with [+0x48]=6 builds
/// the driver 0x143d7c088 + worker 0x144842d40, cold. Pure build -> read-back; no save write.
unsafe fn worldres_coldbuild_probe(base: usize) {
    const CSRES_GETTER_RVA: usize = STREAMING_DRIVER_BUILDER_RVA;
    const EMK_RESMAN_DRIVER_RVA: usize = STREAMING_DRIVER_SINGLETON_RVA;
    // NOTE: this global is upstream's `runtime_heap_allocator` (DLAllocator), always non-null --
    // NOT a world-stream worker. The BEFORE/AFTER "worker" reads below are a FALSE-POSITIVE lever
    // (allocator present regardless of the getter); kept for context via the fromsoftware-rs accessor.
    const STUB_LEN: usize = 0x80;
    const STUB_FILL: u8 = 0;
    const STUB_STATE_OFFSET: usize = 0x48;
    const STUB_STATE_VALUE: i32 = 6;
    const PROBE_DONE: usize = 1;
    static COLDBUILD_DONE: AtomicUsize = AtomicUsize::new(0);
    if COLDBUILD_DONE.swap(PROBE_DONE, Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let driver_before = unsafe { *((base + EMK_RESMAN_DRIVER_RVA) as *const usize) };
    let worker_before = crate::runtime_heap_allocator_ptr_or_null();
    // Persistent zeroed stub `this`: the getter only touches [+0x48] (state) / [+0x4c] / [+0x50].
    let stub: &'static mut [u8; STUB_LEN] = Box::leak(Box::new([STUB_FILL; STUB_LEN]));
    let stub_ptr = stub.as_mut_ptr() as usize;
    unsafe { *((stub_ptr + STUB_STATE_OFFSET) as *mut i32) = STUB_STATE_VALUE };
    append_autoload_debug(format_args!(
        "worldres-coldbuild: BEFORE driver[0x{:x}]=0x{driver_before:x} allocator=0x{worker_before:x} -- calling CSResStep getter 0x{:x}(stub=0x{stub_ptr:x})",
        base + EMK_RESMAN_DRIVER_RVA,
        base + CSRES_GETTER_RVA
    ));
    let getter: unsafe extern "system" fn(usize) -> usize =
        unsafe { std::mem::transmute(base + CSRES_GETTER_RVA) };
    let ret = unsafe { getter(stub_ptr) };
    let driver_after = unsafe { *((base + EMK_RESMAN_DRIVER_RVA) as *const usize) };
    let worker_after = crate::runtime_heap_allocator_ptr_or_null();
    append_autoload_debug(format_args!(
        "worldres-coldbuild: AFTER driver=0x{driver_after:x} worker=0x{worker_after:x} ret=0x{ret:x} (both non-null = lever VALIDATED, NO SetState/NO save write)"
    ));
}

/// COLD CHAR-MOUNT experiment gate (env ER_EFFECTS_COLD_CHAR_MOUNT / er-effects-cold-char-mount.txt,
/// OFF by default). The DECISIVE save-data experiment (save-io-infra-present-cold-char-mount-is-the-
/// decisive-untested-experiment-2026): with the stream worker REGISTERED, can the b80 save-IO read
/// drain to resident so 0x67b290 mounts the real char -- zero-input, SAVE-SAFE (reads the save,
/// applies char to memory; NO SetState, NO save write).
pub(crate) fn cold_char_mount_enabled() -> bool {
    COLD_CHAR_MOUNT_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_COLD_CHAR_MOUNT").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-cold-char-mount.txt")
            .exists()
}

/// SAVE-SAFE verify-only OWN-LOAD buffer-feed gate. OFF by default; enable via the reliable
/// autoload-file channel (`own_load=1` in er-effects-autoload.txt -> `OWN_LOAD_FILE_ARMED`), env
/// `ER_EFFECTS_OWN_LOAD=1`, or a GAME_DIR file `er-effects-own-load.txt`. When ON, `own_load_drive`
/// hooks the FSM-gated save read 0x67b100, feeds it our sliced plaintext .sl2 slot body, calls the
/// native parser 0x67b290(slot) in-process, then reads back GameMan+0xc30 + the PlayerGameData
/// fingerprint. NO SetState5, NO autosave, NO continue_confirm.
pub(crate) fn own_load_enabled() -> bool {
    OWN_LOAD_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(std::env::var("ER_EFFECTS_OWN_LOAD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load.txt")
            .exists()
}

/// GOLDEN BASELINE world-stream observe mode (er-effects-golden-observe.txt / ER_EFFECTS_GOLDEN_OBSERVE).
/// OFF by default; purely ADDITIVE and OBSERVE-ONLY -- it fires NO continue/SetState5/load of any kind.
/// When armed, the SAME recurring world-stream observer (`own_load_stream_observe_recurring`) runs on a
/// NORMAL (vanilla, menu-driven) load too, so we can capture a GOLDEN baseline to diff against the
/// menu-free OWN-LOAD stall. On a vanilla load neither `OWN_LOAD_CONTINUE_FIRED` nor the cached
/// pointers from our continue_confirm are set, so golden mode instead has `own_stepper_idx10` cache the
/// live TITLE owner into `OWN_LOAD_OWNER_CACHED` every title frame (the owner pointer is stable), and
/// the observer re-derives InGameStep/MoveMapStep LIVE from that owner each frame (its existing
/// `ingame_cached == 0` fallback) as the vanilla load builds the world.
pub(crate) fn golden_observe_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_GOLDEN_OBSERVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-golden-observe.txt")
        .exists()
}

/// Whether the FINAL guarded `continue_confirm`/`SetState5` world-stream step is armed. SAVE-WRITING
/// when it fires (`SetState5` autosaves), so it stays OFF by default: `own_load_drive` is verify-only
/// unless this is explicitly armed via the autoload-file channel (`own_load_continue=1` in
/// er-effects-autoload.txt -> `OWN_LOAD_CONTINUE_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_CONTINUE=1`,
/// or a GAME_DIR file `er-effects-own-load-continue.txt`. The hard c30/fingerprint guard inside
/// `own_load_drive` is the absolute save-safety backstop even when this is armed.
pub(crate) fn own_load_continue_enabled() -> bool {
    OWN_LOAD_CONTINUE_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_CONTINUE").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load-continue.txt")
            .exists()
}

/// Whether the OWN-LOAD m28 direct-enqueue lever (`AddDefaultFileLoadProcess`) is ARMED. This is the
/// arming gate ONLY; the lever additionally requires `OWN_LOAD_CONTINUE_FIRED` (our menu-free path
/// actually fired) at fire time, so on a vanilla native menu load -- where that flag is never set --
/// it can NEVER dispatch even if armed. Arm via the autoload-file channel (`own_dispatch=1` in
/// er-effects-autoload.txt -> `OWN_DISPATCH_FILE_ARMED`), env `ER_EFFECTS_OWN_DISPATCH=1`, or a
/// GAME_DIR file `er-effects-own-dispatch.txt`. SAVE-SAFE: reaches only world-asset file-load
/// streaming (RequestDCX -> RSResourceFileRequest -> GLOBAL_LoadManager), never save IO.
pub(crate) fn own_dispatch_enabled() -> bool {
    OWN_DISPATCH_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(std::env::var("ER_EFFECTS_OWN_DISPATCH").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-dispatch.txt")
            .exists()
}

/// Whether the menu-free LoadGame-JOB INSTALL lever is ARMED. When set (alongside `own_load`, which
/// makes `own_load_drive` run), the verify-only parse is followed by BUILD (`FUN_140826510`) +
/// INSTALL (`FUN_1407a9560`) of the native LoadGame `MenuJobWithContext` into the title owner's
/// `+0x130` MenuJob slot -- replacing the idle `IfElseJob` so `STEP_MenuJobWait` ticks it (self-build
/// -> deser -> world stream). This is the NON-SetState5 alternative to `own_load_continue`: no
/// `SetState5`, no autosave, no save write (build + first-tick deser only READ the save). OFF by
/// default; arm via the autoload-file channel (`own_load_install_job=1` ->
/// `OWN_LOAD_INSTALL_JOB_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_INSTALL_JOB=1`, or a GAME_DIR file
/// `er-effects-own-load-install-job.txt`.
pub(crate) fn own_load_install_job_enabled() -> bool {
    OWN_LOAD_INSTALL_JOB_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_INSTALL_JOB").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load-install-job.txt")
            .exists()
}

/// Whether the PATH B menu-free PRIVATE-PUMP lever (`own_load_pump`) is ARMED. When set (alongside
/// `own_load`, which makes `own_load_drive` run the verify-only parse), the parse is followed by BUILD
/// of the LoadGame `MenuJobWithContext` with REAL mss-derived ctx; the recurring game task then ticks
/// its `Run` privately every frame to completion (deser -> map stream -> m28 mount) and, once it reaches
/// `state==Success`, fires the guarded SetState5 transition ONCE. This is the "own the load" rebuild --
/// no owner+0x130 install, no CSMenuMan dialog, no queue. OFF by default; arm via the autoload-file
/// channel (`own_load_pump=1` -> `OWN_LOAD_PUMP_FILE_ARMED`), env `ER_EFFECTS_OWN_LOAD_PUMP=1`, or a
/// GAME_DIR file `er-effects-own-load-pump.txt`.
pub(crate) fn own_load_pump_enabled() -> bool {
    OWN_LOAD_PUMP_FILE_ARMED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC
        || matches!(
            std::env::var("ER_EFFECTS_OWN_LOAD_PUMP").as_deref(),
            Ok("1")
        )
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-own-load-pump.txt")
            .exists()
}

/// SAVE-SAFE PROBE GATE for `own_load_pump`: when set, the pump runs the corrected BUILD + per-frame
/// `Run` (deser -> map-stream, all READ-only up to world-stream per the path-b spec) but, on reaching
/// `state==Success`, LOGS the result and latches DONE WITHOUT firing the save-writing SetState5
/// transition. This isolates the dialog-ctx correction (does the build no longer AV? does the pump
/// progress to Success?) with ZERO save write -- so it can run against the user's real save with no
/// swap and no autosave risk. OFF by default; env `ER_EFFECTS_OWN_LOAD_PUMP_VERIFY=1` or a GAME_DIR
/// file `er-effects-own-load-pump-verify.txt`.
pub(crate) fn own_load_pump_verify_only() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_OWN_LOAD_PUMP_VERIFY").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-own-load-pump-verify.txt")
        .exists()
}

/// DIRECT "Continue pressed" trigger (bd LIVE-continue-chain-via-selector-NOT-confirm-handler):
/// once the title is at the settled main menu (STEP_MenuJobWait) after press-any-button AND
/// GameMan/GameDataMan is set up, write the exact bit the native Continue path consumes --
/// `*(TitleFlowContext+0x14c) = 1` (+ the save slot at `mss+0x1200`) -- so the native selector
/// `0x1409a8eb0` dispatches the load through the engine's own pump. ZERO simulated input: a pure
/// in-process field write replicating the confirm handler's side effects. OFF by default; arm via
/// env `ER_EFFECTS_FIRE_TFC_CONTINUE=1` or a GAME_DIR file `er-effects-fire-tfc-continue.txt`.
pub(crate) fn fire_tfc_continue_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_FIRE_TFC_CONTINUE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-fire-tfc-continue.txt")
        .exists()
}
/// One-shot guard for `maybe_fire_tfc_continue` (0 = not yet fired).
pub(crate) static TFC_CONTINUE_FIRED: AtomicUsize = AtomicUsize::new(0);
/// The queue-owner dialog whose MenuJobQueue (`dialog+0x10`) holds the posted LoadGame job, for the
/// per-frame drain (`tfc_continue_drain_tick`). 0 = nothing to drain. Set by `maybe_fire_tfc_continue`
/// after a successful PushBackJob.
pub(crate) static TFC_DRAIN_DIALOG: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for the autonomous open-menu (`maybe_auto_open_menu`).
pub(crate) static TFC_AUTO_MENU_OPENED: AtomicUsize = AtomicUsize::new(0);
/// Throttle counter for the dialog+0x50 load-vector readiness gate in `maybe_fire_tfc_continue`
/// (logs the count value occasionally while waiting for it to become a valid has-room vector).
pub(crate) static TFC_LOAD_VEC_WAIT_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Trampoline for the hooked TitleTopDialog::update (`title_update_detour` -> original). 0 = not hooked.
pub(crate) static TITLE_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for installing the TitleTopDialog::update hook (`install_title_update_hook`).
pub(crate) static TITLE_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// The built LoadGame job pointer (selector's out[0]) to pump directly via `ExecuteMenuJob` each
/// frame. 0 = nothing to pump. Set by `maybe_fire_tfc_continue`; cleared when the job completes
/// (ExecuteMenuJob zeroes the slot) or the tick cap is hit. Pumping our own job avoids the dialog's
/// +0x8 slot that AV'd the queue-drain wrapper.
pub(crate) static TFC_DRAIN_JOB: AtomicUsize = AtomicUsize::new(0);
/// Per-frame drain tick counter (caps the drain so a stuck job cannot spin forever).
pub(crate) static TFC_DRAIN_TICKS: AtomicUsize = AtomicUsize::new(0);
/// Max drain ticks (~ a generous loading-screen budget at 60fps) before giving up on the drain.
pub(crate) const TFC_DRAIN_TICK_CAP: usize = 4096;

/// Overlay kill switch: when set, the hudhook/ImGui DX12 overlay is NOT initialized (no extra DX12
/// hooks / render overhead) -- for golden/trace runs that want a clean game with only our diagnostics.
/// OFF by default; env `ER_EFFECTS_NO_OVERLAY=1` or a GAME_DIR file `er-effects-no-overlay.txt`.
pub(crate) fn overlay_disabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_NO_OVERLAY").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-no-overlay.txt")
            .exists()
}

/// Direct ProfileLoadDialog build mode (er-effects-direct-build.txt / ER_EFFECTS_DIRECT_BUILD).
/// OFF by default: a plain own_stepper run stays the safe read-only scan; the native dialog build
/// (which leads to a guarded SetState(5) save-write via STAGE 2) fires only when deliberately
/// enabled, so the first native-build run is a deliberate, save-backed experiment.
pub(crate) fn direct_build_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_DIRECT_BUILD").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-direct-build.txt")
            .exists()
}

/// MODEL B: LIVE-dialog Load-Game fire (er-effects-live-dialog.txt / ER_EFFECTS_LIVE_DIALOG).
/// OFF by default. SIBLING to direct_build (the forge). Instead of FORGING a ProfileLoadDialog
/// (factory 0x14081ead0 with a synthetic capture + no live MenuWindow -> a NON-LIVE dialog the
/// native menu group never pumps -> wrong-map/crash), this locates the REAL Load-Game registry
/// node (CS::MenuMemberFuncJob<TitleTopDialog>, vtable 0x142b265d0, member-fn chains to factory
/// 0x14081ead0) and invokes its native run 0x1409aaba0(rcx=node) -- so the ProfileLoadDialog is
/// born LIVE & registered in menu-group 0x143d87350, which the native pump drives. STAGE2 then
/// fires load_activate (vt+0xa0) + the guarded continue_confirm -> SetState(5). The forge path
/// (direct_build) is untouched; this is a deliberate, separately-gated experiment.
pub(crate) fn live_dialog_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_LIVE_DIALOG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-live-dialog.txt")
            .exists()
}

/// 2026-06-18 BREAKTHROUGH build: construct a CS::ProfileLoadDialog DIRECTLY at the open menu,
/// bypassing the input-gated router_this/d180-on-confirm layer (runtime-PROVEN never to build
/// headless -- loadgame-fingerprint-scan-confirms-router-this-not-built-headless-2026). The
/// ProfileLoadDialog ctor 0x1409a3d90 is COLD-VIABLE (it builds router_this + the slot rows
/// inline, no session/PlayerGameData/input-focus deps). We call dialog_factory 0x14081ead0,
/// which does op-new(0x1cd0) via allocator [0x143d87350] + ctx-build + ctor, passing:
///   rcx = &cap  (cap[0] = owner+0x138 = the ctor r8 = *(capture+8); factory reads *(rcx));
///   rdx = &ctx  (zeroed incoming-ctx -> empty cosmetic label).
/// Returns the dialog* in rax. FULLY read-only-validated before the native call (owner-obj vtable
/// 0x142ac7f20 + a populated row-vector [+0xa58..+0xa60]); fail-closed on any mismatch (NO call /
/// NO further action / NO write). On success: store OWN_STEPPER_DIALOG + advance to S2_ACTIVATE,
/// which own_stepper_stage2 drives (load_activate -> menu_deser mount -> guarded continue_confirm).
/// One-shot (OWN_STEPPER_DIRECT_BUILT). The ONLY save-write risk is STAGE 2's guarded SetState(5).
unsafe fn own_stepper_direct_build(owner: usize, base: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const FACTORY_RVA: usize = 0x81ead0;
    const OWNER_OBJ_138: usize = 0x138;
    const OWNER_OBJ_VTABLE_RVA: usize = 0x2ac7f20;
    const ROWVEC_BEGIN_A58: usize = 0xa58;
    const ROWVEC_END_A60: usize = 0xa60;
    const ROWVEC_MAX_SPAN: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    // CONVERGENCE (2026-06-18, cold-b80-drain-is-PREVIEW-metadata-lane + direct-build): ACTIVATE the
    // slot byte BEFORE building the dialog, so the ctor's list-builder 0x140875590 (which checks
    // 0x140261cd0 = [ProfileSummary+8+slot]) APPENDS the slot -> the dialog's save-rows populate
    // (bound>0) -> load_activate has a row to read. This wires the ACTIVATE-byte breakthrough into
    // the direct-built dialog. Save-safe (in-memory byte; the dialog build is no-write).
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let gdm = game_data_man_ptr_or_null();
    let profile_summary = if gdm != NULL {
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if profile_summary != NULL && want_slot >= OWN_STEPPER_SLOT_ZERO {
        let activate: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + PROFILE_SLOT_ACTIVATE_RVA) };
        unsafe { activate(profile_summary, want_slot) };
        // Record-state: load_activate 0x1409a4670's gate is INVERTED (load_activate-gate-inverted-
        // live-mount-is-nonbuild-path) -- the LIVE mount takes the NON-build branch (which calls
        // builder 0x140826510 @0x9a4985) when [rec+0x295]>=1 && accessor 0x140e362c0([rec+0x44])==2.
        // So set those so load_activate BUILDS the selector step (then we self-pump it -- the cold
        // standalone dialog is not ticked by the MENU group). rec = profile + 0x18 + slot*0x2a0.
        const RECORD_BASE_18: usize = 0x18;
        const RECORD_STRIDE_2A0: usize = 0x2a0;
        const RECORD_VALID_295: usize = 0x295;
        const RECORD_STATE_44: usize = 0x44;
        const RECORD_VALID_SET: u8 = 1;
        const RECORD_STATE_LOADABLE: i32 = 2;
        let rec = profile_summary + RECORD_BASE_18 + (want_slot as usize) * RECORD_STRIDE_2A0;
        unsafe { *((rec + RECORD_VALID_295) as *mut u8) = RECORD_VALID_SET };
        unsafe { *((rec + RECORD_STATE_44) as *mut i32) = RECORD_STATE_LOADABLE };
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD ACTIVATE 0x{:x}(profile=0x{profile_summary:x}, slot={want_slot}) + record [rec=0x{rec:x}+0x295]=1 [+0x44]=2 (rows populate + load_activate reaches the selector builder)",
            base + PROFILE_SLOT_ACTIVATE_RVA
        ));
    }
    let owner_obj = owner + OWNER_OBJ_138;
    // Read-only re-validation of r8 (owner_obj) before the native build: expected vtable + a
    // populated row-vector (begin < end, sane span). Fail-closed (latch set so we don't spin).
    let ovt = unsafe { safe_read_usize(owner_obj) }.unwrap_or(NULL);
    let begin = unsafe { safe_read_usize(owner_obj + ROWVEC_BEGIN_A58) }.unwrap_or(NULL);
    let end = unsafe { safe_read_usize(owner_obj + ROWVEC_END_A60) }.unwrap_or(NULL);
    let span = end.wrapping_sub(begin);
    let rows_ok = ovt == base + OWNER_OBJ_VTABLE_RVA
        && begin != NULL
        && (begin & PTR_ALIGN_MASK) == NULL
        && end > begin
        && span <= ROWVEC_MAX_SPAN;
    if !rows_ok {
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD ABORT (fail-closed, NO native call) owner_obj=0x{owner_obj:x} vt=0x{ovt:x}(want 0x{:x}) rowvec=[0x{begin:x}..0x{end:x}] span=0x{span:x}",
            base + OWNER_OBJ_VTABLE_RVA
        ));
        OWN_STEPPER_DIRECT_BUILT.store(OWN_STEPPER_DIRECT_BUILT_YES, Ordering::SeqCst);
        return;
    }
    // Stage the persistent buffers: cap[0] = owner_obj (factory reads *(rcx) for the ctor r8);
    // ctx stays zeroed (factory reads it to build an empty label).
    let cap_ptr = (&raw mut DIRECT_BUILD_CAP) as *mut usize;
    unsafe { *cap_ptr = owner_obj };
    let cap_addr = cap_ptr as usize;
    let ctx_addr = (&raw mut DIRECT_BUILD_CTX) as *mut usize as usize;
    let factory: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + FACTORY_RVA) };
    append_autoload_debug(format_args!(
        "own_stepper: DIRECT-BUILD calling factory 0x{:x}(rcx=&cap[=0x{owner_obj:x}], rdx=&ctx) owner_obj vt=0x{ovt:x} rowvec=[0x{begin:x}..0x{end:x}]",
        base + FACTORY_RVA
    ));
    let dialog = unsafe { factory(cap_addr, ctx_addr) };
    let dvt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    OWN_STEPPER_DIRECT_BUILT.store(OWN_STEPPER_DIRECT_BUILT_YES, Ordering::SeqCst);
    if dialog != NULL && dvt == base + PROFILE_LOAD_DIALOG_VTABLE_RVA {
        OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
        own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD SUCCESS dialog=0x{dialog:x} vt=0x{dvt:x} (ProfileLoadDialog) -- entering STAGE2 ACTIVATE (slot={})",
            OWN_STEPPER_SLOT.load(Ordering::SeqCst)
        ));
    } else {
        append_autoload_debug(format_args!(
            "own_stepper: DIRECT-BUILD returned dialog=0x{dialog:x} vt=0x{dvt:x} != ProfileLoadDialog 0x{:x} -- fail-closed, STAY (NO STAGE2, NO write)",
            base + PROFILE_LOAD_DIALOG_VTABLE_RVA
        ));
    }
}

/// Multi-frame cold char-mount drive (gated, SAVE-SAFE). Sequence (worker registered): build+register
/// the FD4 stream worker (0xb0a980 stub) so the scheduler ticks it and drains the save-IO read; set
/// the slot; PREVIEW 0x67b4e0 (b80=1 + starts the iodev read); poll 0x679180 each frame until
/// GameMan+0xb80==3 (the make-or-break -- the registered+ticked worker draining the read); then
/// deserialize 0x67b290 (mounts GameMan+0xc30=real map + applies the char to PlayerGameData).
/// NO SetState / NO save write. dump_load_correctness verifies the mounted char.
unsafe fn cold_char_mount_drive(base: usize, gm: usize, want_slot: i32, n: u64) {
    const PHASE_INIT: usize = 0;
    const PHASE_LANE: usize = 1;
    const PHASE_POLL: usize = 2;
    const PHASE_DESER: usize = 3;
    const PHASE_DONE: usize = 4;
    const STUB_FILL: u8 = 0;
    const POLL_ARG: u8 = 0;
    const B80_RESIDENT: i32 = 3;
    const B80_IDLE: i32 = 0;
    // A real worker-drained read goes resident within a handful of frames; a stuck cold read never
    // does. 240 frames (~4s) is ample to distinguish drain-vs-stuck while keeping the probe's
    // evidence-teardown fast (the old 1200 forced a ~20s stare at press-any-button for no signal).
    const MOUNT_POLL_MAX: usize = 240;
    const LOG_INTERVAL: usize = 30;
    const WAIT_INC: usize = 1;
    static MOUNT_PHASE: AtomicUsize = AtomicUsize::new(PHASE_INIT);
    static MOUNT_WAITS: AtomicUsize = AtomicUsize::new(0);
    // Fire the warm FD4 worker-kick (0x67b4e0) at most once per process.
    static WARM_KICK_FIRED: AtomicUsize = AtomicUsize::new(0);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if gm == null {
        return;
    }
    let read_i32 = |off: usize| unsafe { *((gm + off) as *const i32) };
    let iodev_summary = || -> (usize, usize, usize) {
        let iodev = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        if iodev == null {
            (null, null, null)
        } else {
            unsafe {
                (
                    *((iodev + IODEV_INFLIGHT_10_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_18_OFFSET) as *const usize),
                    *((iodev + IODEV_REQHANDLE_20_OFFSET) as *const usize),
                )
            }
        }
    };
    let phase = MOUNT_PHASE.load(Ordering::SeqCst);
    // Publish phase+1 so the readiness watcher can observe terminal completion (PHASE_DONE -> 5)
    // and tear down on evidence rather than on the wall-clock cap.
    COLD_CHAR_MOUNT_PHASE_PUB.store(phase + 1, Ordering::SeqCst);
    if phase == PHASE_INIT {
        const SLOT_MIN: i32 = 0;
        if want_slot < SLOT_MIN {
            append_autoload_debug(format_args!(
                "cold-char-mount: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-effects-own-stepper.txt -- ABORT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // (-2) SIGN-IN FORCE (bd b80-ROOTCAUSE-cold-no-user-signin). The SaveLoad2 storage-select op
        // ctor (0x14240f1b0) builds its runnable ONLY if the sign-in check returns true AND the user
        // index is <= 3; cold (no signed-in user) both fail -> the op is null and the load FSM parks
        // at idx 0x16 (the b80 wall). Patch the two gate fns (deobf-verified live entries) so the
        // cold path loads as if signed in as user 0. Save-safe (in-memory code patch). Done here, in
        // PHASE_INIT, before the submit so the select op the load triggers sees the patched gates.
        apply_signin_force(base);
        // (-1.5) SOURCE PROBE (read-only) for a future controlled public-requestLoad (0x14240ac00):
        // the dead load builder reads source globals that may be invalid cold (it crashed). Before
        // ever calling requestLoad, log the candidate sources so we know a valid one: SLLoadContent
        // *0x143d87358, the secondary *0x143d872e0, and owner+8 (what the dead builder passed as the
        // requestLoad source). Pure reads -- no calls into risky fns.
        const SLLOADCONTENT_SRC_RVA: usize = 0x3d87358;
        const SLLOAD_SRC2_RVA: usize = 0x3d872e0;
        let src1 = unsafe { safe_read_usize(base + SLLOADCONTENT_SRC_RVA) }.unwrap_or(null);
        let src2 = unsafe { safe_read_usize(base + SLLOAD_SRC2_RVA) }.unwrap_or(null);
        let owner_probe = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        let owner8 = if owner_probe != null {
            unsafe { safe_read_usize(owner_probe + 8) }.unwrap_or(null)
        } else {
            null
        };
        append_autoload_debug(format_args!(
            "cold-char-mount: SOURCE-PROBE SLLoadContent[*0x143d87358]=0x{src1:x} src2[*0x143d872e0]=0x{src2:x} owner=0x{owner_probe:x} owner8=0x{owner8:x} (non-null source needed for a safe public requestLoad 0x14240ac00)"
        ));
        // SLSYS-PROBE (read-only): is the SaveLoad2 SLSystemImpl + its SESSION MANAGER built cold? If
        // the session manager (sysimpl+0x8) is NULL, requestLoad derefs null -> that explains the
        // off-thread crash, and the NARROW menu-free fix is to call SaveLoad2 initialize first (build
        // the manager) before any load. If it's already built+ready (sysimpl+0x19!=0), the crash is a
        // deeper threading issue and the synthetic path is a real dead end. *0x144852f88 = SLSystemImpl
        // ptr; +0x8 = SLSessionManager; +0x10 = device/result table; +0x19 = manager-ready flag.
        const SLSYSTEMIMPL_PTR_RVA: usize = 0x4852f88;
        let sysimpl = unsafe { safe_read_usize(base + SLSYSTEMIMPL_PTR_RVA) }.unwrap_or(null);
        let (sl_mgr, sl_tbl, sl_ready) = if sysimpl != null {
            let m = unsafe { safe_read_usize(sysimpl + 0x8) }.unwrap_or(null);
            let t = unsafe { safe_read_usize(sysimpl + 0x10) }.unwrap_or(null);
            let r = unsafe { safe_read_usize(sysimpl + 0x18) }.unwrap_or(0);
            // +0x19 is a byte within the +0x18 qword (manager-ready flag).
            (m, t, (r >> 8) & 0xff)
        } else {
            (null, null, 0xff)
        };
        append_autoload_debug(format_args!(
            "cold-char-mount: SLSYS-PROBE SLSystemImpl[*0x144852f88]=0x{sysimpl:x} sessionMgr[+0x8]=0x{sl_mgr:x} table[+0x10]=0x{sl_tbl:x} ready[+0x19]={sl_ready} (sessionMgr=0 => requestLoad null-derefs = need SaveLoad2 initialize first = NARROW menu-free fix; built+ready => deeper dead end)"
        ));
        // (-1) Set the save-file path/name on the container so the device read returns slot N's REAL
        // .sl2 bytes. The native Continue handler runs this slot-mgr peek 0x140678a50 FIRST (reads
        // [GameDataMan+0x8] container, sync-reads the save path token 0x47054, copies the name to
        // container+0x94, sets GameMan+0xe70=1) before the load. The prior cold attempt SKIPPED it,
        // so the device read an EMPTY buffer (deserialize gave c30=0xffffffff + garbage char).
        // Save-safe (sets a path + reads metadata; NO save write).
        const SLOT_MGR_PEEK_RVA: usize = 0x678a50;
        let peek: unsafe extern "system" fn() =
            unsafe { std::mem::transmute(base + SLOT_MGR_PEEK_RVA) };
        unsafe { peek() };
        append_autoload_debug(format_args!(
            "cold-char-mount: slot-mgr peek 0x{:x}() -> set save-file path before mount (GameMan+0xe70 ready)",
            base + SLOT_MGR_PEEK_RVA
        ));
        // (0) REFRAME (2026-06-18, REFRAME-io-subsystem-present-cold-blocker-is-just-the-active-byte):
        // the FD4 IO subsystem (pool/task/iodev) is ALREADY present + CLEAN cold (snapshot-proven).
        // 0x67b200 fails cold ONLY because its slot-check 0x140261cd0 reads [ProfileSummary+8+slot]==0
        // (the session/ProfileSummary IS present). Set that byte directly via ACTIVATE 0x140262250
        // (byte[profile+slot+8]=1) so 0x67b200 passes its slot-check and submits the read onto the
        // present subsystem. Save-safe (sets an in-memory flag; the deserialize only READS the .sl2).
        const SLOT_ACTIVE_BYTE_BASE: usize = 0x8;
        let game_data_man = game_data_man_ptr_or_null();
        let profile_summary = if game_data_man != null {
            unsafe { *((game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) as *const usize) }
        } else {
            null
        };
        if profile_summary != null {
            let activate: unsafe extern "system" fn(usize, i32) =
                unsafe { std::mem::transmute(base + PROFILE_SLOT_ACTIVATE_RVA) };
            unsafe { activate(profile_summary, want_slot) };
            let abyte = unsafe {
                *((profile_summary + SLOT_ACTIVE_BYTE_BASE + want_slot as usize) as *const u8)
            };
            append_autoload_debug(format_args!(
                "cold-char-mount: ACTIVATE 0x{:x}(profile=0x{profile_summary:x}, slot={want_slot}) -> [profile+8+{want_slot}]={abyte} (so 0x67b200 slot-check 0x140261cd0 passes)",
                base + PROFILE_SLOT_ACTIVATE_RVA
            ));
        } else {
            append_autoload_debug(format_args!(
                "cold-char-mount: ProfileSummary null (gdm=0x{game_data_man:x}) -- cannot ACTIVATE; 0x67b200 will fail its slot-check"
            ));
        }
        // (1) build + register the FD4 stream worker so the scheduler ticks it (drains the read).
        let stub: &'static mut [u8; SYNTHETIC_STEP_THIS_SIZE] =
            Box::leak(Box::new([STUB_FILL; SYNTHETIC_STEP_THIS_SIZE]));
        let stub_ptr = stub.as_mut_ptr() as usize;
        unsafe {
            *((stub_ptr + SYNTHETIC_STEP_STATE_OFFSET) as *mut i32) = WORLD_WORKER_BUILD_STATE
        };
        let worker_build: unsafe extern "system" fn(usize) -> usize =
            unsafe { std::mem::transmute(base + WORLD_WORKER_BUILD_RVA) };
        unsafe { worker_build(stub_ptr) };
        let worker = crate::runtime_heap_allocator_ptr_or_null();
        // (1.5) DEVICE MOUNT/BIND (b80-mount-routine-0x140e6e8d0-recipe-...). ROOT CAUSE of
        // the cold full-read wall: the save IO device is UNMOUNTED cold -- [iodev+0x40]==0
        // (the device-ready flag the async router 0x140e6eb80 tests) and [iodev+0x30]==
        // 0xffffffff (no OS handle), so the full read takes the COLD async branch that
        // completes EMPTY (b80 2->0). The native title->Continue boot binds the device via
        // mount 0x140e6e8d0(iodev); the menu-free path skips it. Self-validating: log the
        // ACTUAL cold device state (we have never read +0x40/+0x30 at runtime -- the unbound
        // conclusion was static inference), call the native mount, log the post-state, then
        // submit. The mount is internally guarded by 0x14240acd0([0x143d872e0]) which needs
        // the IO worker registry [0x144843038+0x18]!=0; if it bails (al=0) the log shows it.
        // SAVE-SAFE: the mount only OPENS a handle + registers paths for READ; no save write.
        let iodev_before = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        let registry = unsafe { *((base + IO_WORKER_REGISTRY_RVA) as *const usize) };
        let reg_count = if registry != null {
            unsafe { *((registry + IO_WORKER_REGISTRY_COUNT_18_OFFSET) as *const u32) }
        } else {
            0
        };
        let read_dev = |iodev: usize| -> (u8, usize) {
            if iodev == null {
                (0, null)
            } else {
                unsafe {
                    (
                        *((iodev + IODEV_READY_FLAG_40_OFFSET) as *const u8),
                        *((iodev + IODEV_OS_HANDLE_30_OFFSET) as *const usize),
                    )
                }
            }
        };
        let (dev40_before, dev30_before) = read_dev(iodev_before);
        // The getter returns the iodev (lazily creating it if null) -- the exact value the
        // native boot passes to the mount.
        let iodev_getter: unsafe extern "system" fn() -> usize =
            unsafe { std::mem::transmute(base + IODEV_GETTER_RVA) };
        let iodev = unsafe { iodev_getter() };
        let mount: unsafe extern "system" fn(usize) -> u8 =
            unsafe { std::mem::transmute(base + IODEV_MOUNT_OPEN_RVA) };
        let mount_al = if iodev != null {
            unsafe { mount(iodev) }
        } else {
            0
        };
        let (dev40_after, dev30_after) = read_dev(iodev);
        append_autoload_debug(format_args!(
            "cold-char-mount: MOUNT 0x{:x}(iodev=0x{iodev:x}) al={mount_al} | registry=0x{registry:x} reg_count={reg_count} | dev40 {dev40_before}->{dev40_after} dev30 0x{dev30_before:x}->0x{dev30_after:x} (al=1 & dev40->nonzero = device bound; submit should now route to the BOUND read)",
            base + IODEV_MOUNT_OPEN_RVA
        ));
        // WORKER-GATE diagnostic (b80-DEVICE-MOUNT-REFUTED-...). The read drops b80 2->0 in
        // ONE frame = the enqueue 0x14240e420 DISCARDS the request (no-op completion). Two
        // discard gates: (1) [worker+0x19]!=0 (no-accept/shutdown byte); (2) the registry
        // intrusive list [registry+0x28] does not contain the caller's key (0x141ee1240).
        // Read both (no call) to pin which gate fires cold. reg_list_empty when [[+0x28]]==[+0x28].
        let worker_mgr = unsafe { *((base + FD4_IO_WORKER_MGR_RVA) as *const usize) };
        let worker_noaccept = if worker_mgr != null {
            unsafe { *((worker_mgr + FD4_IO_WORKER_NOACCEPT_19_OFFSET) as *const u8) }
        } else {
            0xff
        };
        let io_pool = unsafe { *((base + FD4_IO_POOL_RVA) as *const usize) };
        let reg_list_node = if registry != null {
            unsafe { *((registry + IO_WORKER_REGISTRY_LIST_28_OFFSET) as *const usize) }
        } else {
            null
        };
        let reg_list_first = if reg_list_node != null {
            unsafe { *(reg_list_node as *const usize) }
        } else {
            null
        };
        append_autoload_debug(format_args!(
            "cold-char-mount: WORKER-GATE worker_mgr=0x{worker_mgr:x} noaccept[+0x19]={worker_noaccept} io_pool=0x{io_pool:x} reg_list_node=0x{reg_list_node:x} reg_list_first=0x{reg_list_first:x} reg_list_empty={} (noaccept!=0 OR list_empty => enqueue 0x14240e420 DISCARDS the read)",
            reg_list_node == reg_list_first
        ));
        // Worker QUEUE snapshot BEFORE submit (b80-DEVICE-MOUNT-REFUTED-...). Compared against the
        // after-submit snapshot below: if [worker+0x8]/[worker+0x10] CHANGE, the read was ENQUEUED
        // (so the wall is the worker not processing / read-fail); if UNCHANGED, it was DISCARDED at
        // a gate in 0x14240e420 (so the wall is the discard gate / caller-context registration).
        let read_q = |off: usize| -> usize {
            if worker_mgr != null {
                unsafe { *((worker_mgr + off) as *const usize) }
            } else {
                null
            }
        };
        let q8_before = read_q(FD4_IO_WORKER_QUEUE_08_OFFSET);
        let q10_before = read_q(FD4_IO_WORKER_QUEUE_10_OFFSET);
        // Deref the queue fields too: if [worker+0x8]/[worker+0x10] are intrusive-list SENTINELS
        // (fixed), the field value won't move on enqueue but the sentinel.next ([q8]) will. Reading
        // the deref before/after disambiguates ENQUEUED (deref changes) from DISCARDED (no change).
        let qd8_before = unsafe { safe_read_usize(q8_before) }.unwrap_or(null);
        let qd10_before = unsafe { safe_read_usize(q10_before) }.unwrap_or(null);
        // (1.75) SAVE-DIRECTORY -- pre-submit population is REFUTED (bd b80-COLD-FIX-REFUTED-pathdb-
        // transient-setter-wants-char16ptr-2026-06-21). The original plan was to call SETTER
        // 0x14240a2a0([iodev+0x20], 0, &dir) before submit so the request copy-ctor would inherit a
        // real directory. RUNTIME PROOF it cannot work: [iodev+0x20] is 0 BEFORE submit (it only
        // becomes the request handle io20 AFTER submit). STATIC PROOF: the live opcode-0x17/0x18
        // handler 0x140e6ded0 calls the setter with rcx=[this+0x20] where `this` is a TRANSIENT
        // per-request command object (the pump 0x140e6e080 bails when [this+0x20]==0), and the setter
        // wants a RAW char16_t* in r8 (not a std::u16string). So the directory is filled on a
        // per-request object during its state-machine pump, not on a pokable global. The real fix
        // needs the request copy-ctor TEMPLATE source (request ctor 0x14240a850 forwards rdx to
        // copy-ctor 0x1424085b0 -- trace one frame up) OR a post-submit, non-racy write to the live
        // request. Tracked for the next session; the SAVE_DIR_* consts in lib.rs are kept for it.
        // We log the cold path-DB pointer (safe read, no call) so the next run confirms the timing.
        let path_db_cold = if iodev != null {
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_20_OFFSET) }.unwrap_or(null)
        } else {
            null
        };
        append_autoload_debug(format_args!(
            "cold-char-mount: SAVE-DIR pre-submit path_db=[iodev+0x20]=0x{path_db_cold:x} (expected 0 pre-submit; the request/path-DB only exists AFTER submit -- pre-submit setter is REFUTED, see bd)"
        ));
        // (2) Resolve + set the slot, then submit the FULL save read (b80=2). The old
        // preview+LoadSaveData path drained but only left metadata resident, so 0x67b290 could
        // report success while c30 stayed at the default map and the strict world oracle caught a
        // false positive. The live native_fullread recipe also writes GameMan+0xb78 before
        // set_save_slot because resolver 0x1406793c0 reads that selector; direct-build previously
        // omitted it and reached b80==3 but deserialized the wrong/default buffer. Use the
        // runtime-pinned full-read initiator 0x67b1a0, then co-drive lane+poll in PHASE_POLL until
        // b80 reaches RESIDENT before deserializing.
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = want_slot };
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(want_slot) };
        let submit: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + B80_FULL_LOAD_INITIATOR_RVA) };
        let sret = unsafe { submit(want_slot) };
        let (io10, io18, io20) = iodev_summary();
        let q8_after = read_q(FD4_IO_WORKER_QUEUE_08_OFFSET);
        let q10_after = read_q(FD4_IO_WORKER_QUEUE_10_OFFSET);
        let qd8_after = unsafe { safe_read_usize(q8_after) }.unwrap_or(null);
        let qd10_after = unsafe { safe_read_usize(q10_after) }.unwrap_or(null);
        append_autoload_debug(format_args!(
            "cold-char-mount: FULL-INIT slot={want_slot} b78={b78} worker=0x{worker:x} submit_ret={sret} b80={} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} | q8 0x{q8_before:x}->0x{q8_after:x} [q8] 0x{qd8_before:x}->0x{qd8_after:x} q10 0x{q10_before:x}->0x{q10_after:x} [q10] 0x{qd10_before:x}->0x{qd10_after:x} (any change=ENQUEUED; none=DISCARDED) -> POLL",
            read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET)
        ));
        // (2.4) SAVE-DIR READ-ONLY VERIFY (bd b80-cold-EXACT-dir-field-slot3-0x142410c60). The worker
        // (SLLoadSession::_Func02 0x142410cd0) -> name-builder FUN_14240d5b0 -> slot-3 0x142410c60
        // reads the dir std::u16string from [SLLoadSession+0xe0] == io18, at io18+0xe8 (data/SSO),
        // size io18+0xf8, cap io18+0x100 (cap>=8 => data is a heap ptr at io18+0xe8, else SSO inline).
        // Empty cold => slot-3 returns empty => builder ret 0 => _Func02 code 8 => no open. Confirm the
        // field+emptiness HERE (pure reads) before any write into this transient request object.
        if io18 != null {
            let dir_size = unsafe { safe_read_usize(io18 + 0xf8) }.unwrap_or(0);
            let dir_cap = unsafe { safe_read_usize(io18 + 0x100) }.unwrap_or(0);
            let dir_data_ptr = if dir_cap >= 8 {
                unsafe { safe_read_usize(io18 + 0xe8) }.unwrap_or(null)
            } else {
                io18 + 0xe8
            };
            let first8 = if dir_data_ptr != null {
                unsafe { safe_read_usize(dir_data_ptr) }.unwrap_or(0)
            } else {
                0
            };
            append_autoload_debug(format_args!(
                "cold-char-mount: SAVE-DIR VERIFY io18=0x{io18:x} dir@+0xe8 size={dir_size} cap={dir_cap} data=0x{dir_data_ptr:x} first8=0x{first8:x} (size==0/first8==0 => EMPTY dir = the cold wall: slot-3 0x142410c60 returns empty -> name-builder 0x14240d5b0 ret 0 -> code 8 -> no open)"
            ));
        }
        // (2.5) SAVE-DIRECTORY POST-SUBMIT INSTALL (bd savedir-CONFIG-LEVER-setter-0x14240a2a0-...).
        // The cold full read completes EMPTY because the path-DB's slot-0 directory std::u16string
        // is unset, so the worker formats a bare `.sl2` that fails to open. The LIVE Continue boot
        // fills it via the opcode-0x17/0x18 pump handler 0x140e6ded0; the menu-free cold path never
        // dispatches that opcode, so we replay its two native steps HERE -- on the LIVE io20
        // (=[iodev+0x20], which only exists AFTER submit) in this SAME task invocation, the tightest
        // window before the worker drains. A real save directory path is well under MAX_PATH;
        // anything larger is garbage/wrong-offset and is rejected before any decode or setter call.
        const REQ_DIR_SANE_MAX_CU: usize = 320;
        // Fault-safe UTF-16 decoder shared by the builder-output log and the slot readback.
        let decode_u16 = |data: usize, size: usize| -> String {
            let mut s = String::new();
            if data != null && size != 0 && size <= REQ_DIR_SANE_MAX_CU {
                let words = size.div_ceil(4);
                'decode: for w in 0..words {
                    let Some(word) = (unsafe { safe_read_usize(data + w * 8) }) else {
                        break;
                    };
                    for b in 0..4 {
                        let cu = ((word >> (b * 16)) & 0xffff) as u16;
                        if cu == 0 || w * 4 + b >= size {
                            break 'decode;
                        }
                        s.push(char::from_u32(cu as u32).unwrap_or('?'));
                    }
                }
            }
            s
        };
        // Build the canonical `<userdata>/EldenRing/<steamid>/` into a stack-resident MSVC
        // stateful-allocator u16string wrapper (allocator@+0, data@+0x08, size@+0x18, cap@+0x20).
        // The builder ASSUMES a pre-constructed empty string, so install the arena allocator at +0
        // and cap=7 (empty SSO) first. [u64;8] guarantees 8-byte alignment for the field writes.
        let mut wrapper = [0u64; 8];
        let wbase = wrapper.as_mut_ptr() as usize;
        let alloc_getter: unsafe extern "system" fn() -> usize =
            unsafe { std::mem::transmute(base + SAVE_DIR_ALLOC_GETTER_RVA) };
        let allocator = unsafe { alloc_getter() };
        unsafe {
            *((wbase + U16STRING_ALLOC_OFFSET) as *mut usize) = allocator;
            *((wbase + U16STRING_CAP_OFFSET) as *mut usize) = U16STRING_SSO_CAP;
        }
        // Guard: the builder derefs the Steam interface (*0x143b48ff0) for the account id; skip the
        // call (logging the cause) if it is null cold -- that would be hypothesis-2 (Steam not live).
        let steam_iface =
            unsafe { safe_read_usize(base + STEAM_INTERFACE_GUARD_RVA) }.unwrap_or(null);
        if steam_iface != null && allocator != null {
            let builder: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(base + SAVE_DIR_BUILDER_RVA) };
            unsafe { builder(wbase) };
        }
        let dir_cap = unsafe { *((wbase + U16STRING_CAP_OFFSET) as *const usize) };
        let dir_size = unsafe { *((wbase + U16STRING_SIZE_OFFSET) as *const usize) };
        let dir_data = if dir_cap >= 8 {
            unsafe { *((wbase + U16STRING_DATA_OFFSET) as *const usize) }
        } else {
            wbase + U16STRING_DATA_OFFSET
        };
        let built_text = decode_u16(dir_data, dir_size);
        append_autoload_debug(format_args!(
            "cold-char-mount: SAVE-DIR BUILD steam_iface=0x{steam_iface:x} allocator=0x{allocator:x} cap={dir_cap} size={dir_size} data=0x{dir_data:x} text=\"{built_text}\" (size>0 & real path = builder works cold = hypothesis-1 handler-never-ran; size=0 = Steam not live cold = hypothesis-2)"
        ));
        // Install on the LIVE path-DB slot-0 directory. The setter COPIES our buffer into the slot
        // entry's std::u16string at entry+0xb0 (via 0x14240dce0), so our stack wrapper can be dropped.
        let setter: unsafe extern "system" fn(usize, i32, usize) =
            unsafe { std::mem::transmute(base + SAVE_DIR_SETTER_RVA) };
        let set_fired =
            io20 != null && dir_data != null && dir_size > 0 && dir_size <= REQ_DIR_SANE_MAX_CU;
        if set_fired {
            unsafe { setter(io20, want_slot, dir_data) };
        }
        // Readback: re-resolve the slot entry (lookup is find-or-create, idempotent post-setter) and
        // decode its directory at entry+0xb0 to confirm the install landed. The dir there is a bare
        // (stateless-allocator) u16string: data union at +0, size at +0x10.
        let coll = if io20 != null {
            unsafe { safe_read_usize(io20) }.unwrap_or(null)
        } else {
            null
        };
        let key = if io20 != null {
            unsafe { safe_read_usize(io20 + 8) }.unwrap_or(0) as i32
        } else {
            0
        };
        let entry = if coll != null && set_fired {
            let lookup: unsafe extern "system" fn(usize, i32) -> usize =
                unsafe { std::mem::transmute(base + SAVE_DIR_SLOT_LOOKUP_RVA) };
            unsafe { lookup(coll, key) }
        } else {
            null
        };
        let (rb_data, rb_size) = if entry != null {
            let cap = unsafe { safe_read_usize(entry + 0xb0 + 0x18) }.unwrap_or(0);
            let size = unsafe { safe_read_usize(entry + 0xb0 + 0x10) }.unwrap_or(0);
            let data = if cap >= 8 {
                unsafe { safe_read_usize(entry + 0xb0) }.unwrap_or(null)
            } else {
                entry + 0xb0
            };
            (data, size)
        } else {
            (null, 0)
        };
        let rb_text = decode_u16(rb_data, rb_size);
        append_autoload_debug(format_args!(
            "cold-char-mount: SAVE-DIR INSTALL set_fired={set_fired} io20=0x{io20:x} coll=0x{coll:x} key={key} entry=0x{entry:x} readback size={rb_size} text=\"{rb_text}\" (set_fired & readback matches the built path = slot-0 dir installed -> the full read should now find the .sl2 -> b80->3)"
        ));
        // OWNER-FSM GATE MEASUREMENT (bd b80-owner-FSM-lifecycle-gates-2026-06-21). Runtime data
        // REFUTED the static "empty registry / null early-out" story: reg_count=16 (non-empty) and
        // io18/io20 (=owner+0x18/+0x20) persist non-null, so the poll's early-out is NOT the wall.
        // The real bounce is inside the native FSM tick setter 0x140679180: with df0==0 it polls the
        // owner FSM 0x140e6e080(owner); ONLY state-index 0x14 returns 0 (-> b80=3), any index>=2 (18
        // ->3, 0x19->2+teardown, 0x19... ) resets b80=0. The index comes from the PURE getter
        // 0x14240a1f0([owner+0x20]): returns 0x19 when the handle's container ([o20]) is null, else a
        // real node index; 0x14 only when idle-ready (container built, current-node null, deep gate 0).
        // Read the handle internals + index here while b80 is still 2, to pin the EXACT failing gate
        // before building any fix. All reads are fault-safe; the getter is a read-only status query.
        const STATE_INDEX_GETTER_RVA: usize = 0x240a1f0;
        const OWNER_HANDLE_CONTAINER_OFFSET: usize = 0x0;
        const OWNER_HANDLE_H10_OFFSET: usize = 0x10;
        const OWNER_DF0_OFFSET: usize = 0xdf0;
        let owner_fsm = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
        let container = if io20 != null {
            unsafe { safe_read_usize(io20 + OWNER_HANDLE_CONTAINER_OFFSET) }.unwrap_or(null)
        } else {
            null
        };
        let h10 = if io20 != null {
            unsafe { safe_read_usize(io20 + OWNER_HANDLE_H10_OFFSET) }.unwrap_or(null)
        } else {
            null
        };
        let h10_deep = if h10 != null {
            unsafe { safe_read_usize(h10 + OWNER_HANDLE_H10_OFFSET) }.unwrap_or(usize::MAX)
        } else {
            usize::MAX
        };
        let fsm_index = if io20 != null {
            let idx_getter: unsafe extern "system" fn(usize) -> i32 =
                unsafe { std::mem::transmute(base + STATE_INDEX_GETTER_RVA) };
            unsafe { idx_getter(io20) }
        } else {
            -1
        };
        let df0 = unsafe { *((gm + OWNER_DF0_OFFSET) as *const usize) };
        let b80_at_init = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        append_autoload_debug(format_args!(
            "cold-char-mount: OWNER-FSM owner=0x{owner_fsm:x} o18=0x{io18:x} o20=0x{io20:x} container=[o20]=0x{container:x} h10=[o20+0x10]=0x{h10:x} h10_deep=[h10+0x10]=0x{h10_deep:x} fsm_index=0x{fsm_index:x} df0=[gm+0xdf0]=0x{df0:x} b80={b80_at_init} (idx 0x14=idle->b80=3; 0x19=container-null; df0!=0=warm fast-path)"
        ));
        MOUNT_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        MOUNT_PHASE.store(PHASE_POLL, Ordering::SeqCst);
        return;
    }
    if phase == PHASE_LANE {
        // While b80==1, tick the b80==1 lane driver 0x679510 (IO tick) to drive the PREVIEW read to
        // resident. It keeps b80=1 while in-progress and resets b80=0 once the read completes (the
        // registered+ticked worker is what makes that completion happen). When b80==0, the iodev
        // request is resident; fire LoadSaveData 0x67b200 to re-enter the b80=2 lane (populates io18).
        let lane: unsafe extern "system" fn() -> i32 =
            unsafe { std::mem::transmute(base + B80_LANE1_DRIVER_RVA) };
        let _ = unsafe { lane() };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let w = MOUNT_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst);
        if w % LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS {
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: LANE waits={w} b80={b80} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        if b80 == B80_IDLE {
            let loadsave: unsafe extern "system" fn(i32) -> i32 =
                unsafe { std::mem::transmute(base + B80_LOAD_SAVE_DATA_INITIATOR_RVA) };
            let lret = unsafe { loadsave(want_slot) };
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: preview read RESIDENT (b80->0 after {w} lane ticks) -> LoadSaveData 0x67b200 ret={lret} b80={} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} -> POLL",
                read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET)
            ));
            MOUNT_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
            MOUNT_PHASE.store(PHASE_POLL, Ordering::SeqCst);
        } else if w >= MOUNT_POLL_MAX {
            append_autoload_debug(format_args!(
                "cold-char-mount: PREVIEW read never resident after {w} lane ticks (b80 stuck at {b80}, io18 never populated) -- the registered worker is NOT draining the read. TIMEOUT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
    if phase == PHASE_POLL {
        // Full-load submit is the b80==2 lane: tick the IO lane and poll every frame, matching the
        // native_fullread drain that proved 0x67b1a0 can make the 0x280000 full-save buffer resident.
        // NOTE (b80-fullread-CORRECTION-...): a lane-skip A/B run FALSIFIED the "lane 0x679510
        // prematurely completes the read" hypothesis -- with lane() removed, b80 was ALREADY 0 at
        // POLL waits=0 (it drops 2->0 in the native frame right after submit, before cold_char_mount
        // ticks anything). So the recipe-aligned lane+poll drain is restored; the real wall is that
        // the cold async full read completes EMPTY (b80->0, never resident=3) -- the worker is
        // registered+scheduler-ticked but does no actual 0x280000 disk IO. Next suspect: the df0
        // fast-path ([mgr+0xdf0]!=0 -> 0x67b100 skips the read).
        let lane: unsafe extern "system" fn() -> i32 =
            unsafe { std::mem::transmute(base + B80_LANE1_DRIVER_RVA) };
        let _ = unsafe { lane() };
        let poll: unsafe extern "system" fn(u8, u8) -> i32 =
            unsafe { std::mem::transmute(base + B80_POLL_RVA) };
        let _ = unsafe { poll(POLL_ARG, POLL_ARG) };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let w = MOUNT_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst);
        // WARM WORKER-KICK (bd b80-WARM-kick-0x14067b4e0-worker-0x140e6ec80). The cold submit
        // 0x67b1a0 only request_transitions state 0xa, so the owner-FSM node parks at idx 0x16 (an
        // async device-read node) and NOTHING pumps it: the node advances ONLY via the FD4 worker
        // that the warm Continue step (0x14082ba30) builds by calling 0x67b4e0(cl=0). That kick mints
        // a handle (0x141ed5fe0), captures it to GameMan+0xb98/0xba0, then 0x140e6ec80 subscribes the
        // node-advance callback to events 0x7..0x12 AND submits the real save-read as an FD4 job-pool
        // job (engine-wide, NOT menu-gated). On the menu-free cold path that kick never runs. Fire it
        // ONCE here -- b80 has bounced to 0, satisfying 0x67b4e0's b80==0 guard -- to pump the parked
        // node to completion. SAVE-SAFE: it submits a READ job; no save write. The single warm caller
        // passes cl=0 (xor ecx,ecx at 0x14082ba39).
        if b80 == B80_IDLE
            && WARM_KICK_FIRED.swap(WAIT_INC, Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
        {
            const NODE_FINALIZER_RVA: usize = 0xe6f200;
            const WARM_LOAD_KICK_RVA: usize = 0x67b4e0;
            const GAME_MAN_LOAD_HANDLE_B98_OFFSET: usize = 0xb98;
            const GAME_MAN_LOAD_HANDLE_BA0_OFFSET: usize = 0xba0;
            // RUNTIME-PROVEN cold gate (bd b80-WARM-kick-runtime-0x140e6ec80-returns0-cold): the
            // worker-builder 0x140e6ec80 (inside the kick) returns al=0 unless BOTH [owner+0x10]==0
            // (worker) AND [owner+0x20]==0 (node) -- it only builds when nothing exists yet. In the
            // warm path the worker is built BEFORE the node; our cold flow built the parked node
            // first (owner+0x20 = io20, non-null), so the kick bailed (ret=0, no FD4 job). Clear the
            // parked node via the finalizer 0x140e6f200 (zeroes owner+0x10/+0x18/+0x20 -- the same
            // teardown the idx-0x14 success path runs) so the kick rebuilds worker+node cleanly and
            // submits the real FD4 read job. owner = iodev = *0x144589390.
            let owner = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
            let (o10_pre, o20_pre) = if owner != null {
                unsafe {
                    (
                        safe_read_usize(owner + IODEV_INFLIGHT_10_OFFSET).unwrap_or(null),
                        safe_read_usize(owner + IODEV_REQHANDLE_20_OFFSET).unwrap_or(null),
                    )
                }
            } else {
                (null, null)
            };
            if owner != null {
                let finalizer: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(base + NODE_FINALIZER_RVA) };
                unsafe { finalizer(owner) };
            }
            let o20_post = if owner != null {
                unsafe { safe_read_usize(owner + IODEV_REQHANDLE_20_OFFSET) }.unwrap_or(null)
            } else {
                null
            };
            let _ = (
                WARM_LOAD_KICK_RVA,
                GAME_MAN_LOAD_HANDLE_B98_OFFSET,
                GAME_MAN_LOAD_HANDLE_BA0_OFFSET,
                o10_pre,
                o20_pre,
                o20_post,
            );
            // PROPER-LOAD (off-thread). Calling the load builder (deobf entry 0x140e6da42) INLINE on
            // the game task HUNG it -- requestLoad (0x14240ac00) blocks on async machinery (FD4 job
            // pool / session-manager tick) that needs the game task to keep pumping; blocking the game
            // task in requestLoad deadlocks it. Fix: run the load builder on a SEPARATE thread so the
            // game task stays free to pump the async read to completion. Also SAFER than inline: a hang
            // on this thread doesn't freeze the game (teardown cleans it). Preconditions: finalize
            // (above, game thread) cleared owner+0x10/0x18/0x20; signin forced; source validated
            // non-null. SAVE-SAFE: requestLoad is a READ. Watch owner+0x20 / b80 in the poll below.
            // PROPER-LOAD DISABLED -- DEAD END confirmed (3 attempts, all save-safe): the SaveLoad2
            // load builder (deobf 0x140e6da42) is uncallable in the cold menu-free context. Inline on
            // the game task HANGS (requestLoad deadlocks); on a SEPARATE thread it CRASHES
            // (process_exited). Wrong dump addr 0x140e6da37 crashed (misaligned). Sources were
            // validated non-null, so this is a fundamental boot/session/threading-context mismatch, not
            // a bad arg. The dead requestLoad path needs the engine's full boot+session-manager+worker
            // context that the menu-free path lacks. The realistic drive is to let the engine's boot/
            // session machinery run the load (input-blocked), not synthetic primitive calls -- a major
            // redesign that revisits the menu-Continue/save-write-risk constraint. finalize kept
            // (harmless). See bd b80-load-builder-hangs-inline-async-needed + the off-thread crash.
            append_autoload_debug(format_args!(
                "cold-char-mount: PROPER-LOAD disabled (load builder uncallable cold: inline hangs, off-thread crashes) -- finalize 0x{:x}(owner=0x{owner:x}) done, no load call",
                base + NODE_FINALIZER_RVA
            ));
        }
        // (select-node pump REMOVED with the PIVOT: it was for the low-level select-node hypothesis
        // and dereferenced owner+0x20 as a select container; owner+0x20 is now a proper requestLoad
        // handle, so that deref/advance is wrong and unsafe. The proper requestLoad's SLLoadSession is
        // driven autonomously by the SaveLoad2 session manager + FD4 job pool, like the warm path.)
        if w % LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS {
            let (io10, io18, io20) = iodev_summary();
            // Pure-read trajectory telemetry across poll frames (no function calls -- io20 is now a
            // requestLoad handle of unknown internal type, so we only safe-read raw fields): the
            // handle's [o20+0] and [[o20+0x10]+0x10]. Combined with b80 + the char fingerprint below,
            // this shows whether the proper requestLoad drives the load to RESIDENT.
            let (o20_first, h10_deep) = if io20 != null {
                let c0 = unsafe { safe_read_usize(io20) }.unwrap_or(null);
                let h10 = unsafe { safe_read_usize(io20 + 0x10) }.unwrap_or(null);
                let deep = if h10 != null {
                    unsafe { safe_read_usize(h10 + 0x10) }.unwrap_or(usize::MAX)
                } else {
                    usize::MAX
                };
                (c0, deep)
            } else {
                (null, usize::MAX)
            };
            append_autoload_debug(format_args!(
                "cold-char-mount: POLL waits={w} b80={b80} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} [o20]=0x{o20_first:x} h10_deep=0x{h10_deep:x}"
            ));
        }
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        const C30_ZERO: i32 = 0;
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        if b80 == B80_IDLE
            && ac0 == want_slot
            && c30 != GAME_MAN_C30_UNSET
            && c30 != C30_ZERO
            && fp_real
        {
            OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "cold-char-mount: FULL-LATCH success without b80==3 after {w} polls ac0={ac0} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) -- full-read/poll already populated PlayerGameData; NO explicit deserialize needed"
            ));
            unsafe { dump_load_correctness(base, n) };
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        } else if b80 == B80_RESIDENT {
            append_autoload_debug(format_args!(
                "cold-char-mount: b80 reached RESIDENT(3) after {w} polls -- the registered worker DRAINED the read -> DESERIALIZE"
            ));
            MOUNT_PHASE.store(PHASE_DESER, Ordering::SeqCst);
        } else if w >= MOUNT_POLL_MAX {
            append_autoload_debug(format_args!(
                "cold-char-mount: b80 STUCK at {b80} after {w} polls (worker registered but read never resident) ac0={ac0} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) -- TIMEOUT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
    if phase == PHASE_DESER {
        // DIAGNOSTIC (char-apply debug, COLD-B80-WALL-BROKEN-...): before the deserialize, read the
        // suspects for why c30/char did not apply: [mgr+0xdf0] (deserialize-ready -- if set, 0x67b100
        // takes the fast-path and does NOT read into 0x67b290's buffer = lane mismatch / empty parse);
        // [mgr+0x18] (the async load job 0x140e6eb80 queued); [0x143d68078] (the c30-write gate that
        // gates 0x67bd70 inside 0x67b290).
        const DF0_OFFSET: usize = 0xdf0;
        const ASYNC_JOB_18_OFFSET: usize = 0x18;
        const C30_WRITE_GATE_RVA: usize = 0x3d68078;
        let df0 = unsafe { *((gm + DF0_OFFSET) as *const usize) };
        let job18 = unsafe { *((gm + ASYNC_JOB_18_OFFSET) as *const usize) };
        let c30_gate = unsafe { *((base + C30_WRITE_GATE_RVA) as *const usize) };
        let deser: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
        let dret = unsafe { deser(want_slot) };
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        append_autoload_debug(format_args!(
            "cold-char-mount: DESERIALIZE slot={want_slot} ret={dret} c30=0x{c30:x} ac0={ac0} | pre-deser df0(mgr+0xdf0)=0x{df0:x} async_job(mgr+0x18)=0x{job18:x} c30_gate(0x143d68078)=0x{c30_gate:x} (df0!=0 -> 0x67b100 fast-path skips the read = empty parse). NO SetState/NO save write:"
        ));
        unsafe { dump_load_correctness(base, n) };
        // Publish the result so a STAGE2 caller that delegates here can observe completion + the
        // c30/char result. The return code is not a sufficient oracle for m10_01 saves: runtime
        // evidence shows ret=0 with PlayerGameData already populated. Treat a real mounted
        // character fingerprint as success, while still fail-closing on a default/new-game PGD.
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        if dret == OWN_STEPPER_DESER_SUCCESS_RET || fp_real {
            OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "cold-char-mount: DESER-LATCH success dret={dret} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) c30=0x{c30:x}"
            ));
        } else {
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_FAIL, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "cold-char-mount: DESER-LATCH fail dret={dret} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) c30=0x{c30:x}"
            ));
        }
        MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
}

// ===== SAVE-SAFE verify-only OWN-LOAD buffer-feed probe (bd er-effects-rs-lds) ===============
//
// MECHANISM (static-validated 2026-06-22): hook the FSM-gated save read 0x67b100(rcx=out_buf,
// edx=size). When the one-shot gate `OWN_LOAD_GATE` is set, memcpy our sliced PLAINTEXT BND4 slot
// body (from `er_save_loader::bnd4::slot_body`) into out_buf for min(edx, body.len()) bytes and
// return al=1; otherwise call the original. Then call the native parser 0x67b290(slot) in-process
// UNCHANGED -- it allocs the buffer, invokes our hooked 0x67b100 (gets our bytes), and runs the
// REAL native parse (c30 write 0x67bd70 + stream deserialize + char-apply) with zero
// re-implementation. The gate is MANDATORY: 0x67b100 is SHARED with the native menu loader (4
// callers, only one is ours); we must never intercept the menu path. VERIFY is read-back only:
// GameMan+0xc30 (map id) + the PlayerGameData fingerprint. NO SetState5, NO autosave.

/// FSM-gated save read 0x67b100(rcx=out_buf, edx=size) -> al. The leaf read helper our parser
/// 0x67b290 invokes (and the native menu loader -- hence the mandatory gate).
const READ_67B100_RVA: usize = 0x67b100;

/// One-shot gate: true ONLY for the single 0x67b290(slot) call we make from `own_load_drive`. The
/// hook feeds our body + returns al=1 only while this is set; every other (native menu) read passes
/// straight through to the original.
static OWN_LOAD_GATE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Trampoline to the original 0x67b100 (set on hook install).
static READ_67B100_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// One-shot install guard for the 0x67b100 detour.
static OWN_LOAD_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// The sliced plaintext slot body the hook feeds: a leaked `&'static [u8]`, exposed to the detour
/// as (ptr, len) atomics so the game-thread detour reads it lock-free. Set BEFORE arming the gate.
static OWN_LOAD_BODY_PTR: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_BODY_LEN: AtomicUsize = AtomicUsize::new(0);
/// Count of bytes the gated hook fed into the engine buffer on the latched call (verify telemetry).
static OWN_LOAD_FED_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Gated detour for 0x67b100. While `OWN_LOAD_GATE` is set, copies our sliced plaintext slot body
/// into the engine-allocated out_buf (`rcx`) for `min(size, body.len())` bytes and returns al=1 --
/// the engine then parses OUR bytes instead of reading the FSM-gated iodev resident. Otherwise it
/// is a pure pass-through to the original (the native menu loader's reads are never disturbed).
pub(crate) unsafe extern "system" fn read_67b100_hook(out_buf: usize, size: u32) -> u8 {
    const FEED_SUCCESS_RET: u8 = 1;
    if OWN_LOAD_GATE.load(Ordering::SeqCst) {
        let body_ptr = OWN_LOAD_BODY_PTR.load(Ordering::SeqCst);
        let body_len = OWN_LOAD_BODY_LEN.load(Ordering::SeqCst);
        if out_buf != TITLE_OWNER_SCAN_START_ADDRESS && body_ptr != 0 && body_len != 0 {
            // Data-driven length: copy the smaller of the engine's requested size (its own edx) and
            // our body length -- never assume the 0x280000 literal (bd dont-hardcode-savefile-tied).
            let n = core::cmp::min(size as usize, body_len);
            unsafe {
                std::ptr::copy_nonoverlapping(body_ptr as *const u8, out_buf as *mut u8, n);
            }
            OWN_LOAD_FED_BYTES.store(n, Ordering::SeqCst);
            return FEED_SUCCESS_RET;
        }
    }
    let orig = READ_67B100_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return FEED_SUCCESS_RET;
    }
    let orig: unsafe extern "system" fn(usize, u32) -> u8 = unsafe { std::mem::transmute(orig) };
    unsafe { orig(out_buf, size) }
}

/// Install the gated 0x67b100 detour (MhHook + MH_Initialize + queue_enable + MH_ApplyQueued),
/// mirroring the `install_c30_writer_hook` precedent. Idempotent. The detour is harmless until the
/// gate is armed (pure pass-through), so installing it early is safe.
pub(crate) fn install_own_load_hook() -> bool {
    if OWN_LOAD_HOOK_INSTALLED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("own-load: MH_Initialize failed: {status:?}"));
            return false;
        }
    }
    let Ok(read_addr) = game_rva(READ_67B100_RVA as u32) else {
        append_autoload_debug(format_args!("own-load: failed to resolve 0x67b100 rva"));
        return false;
    };
    match unsafe { MhHook::new(read_addr as *mut c_void, read_67b100_hook as *mut c_void) } {
        Ok(hook) => {
            READ_67B100_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("own-load: queue_enable failed: {status:?}"));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    OWN_LOAD_HOOK_INSTALLED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "own-load: hooked 0x{read_addr:x} (GATED feed of sliced .sl2 body; pass-through until armed)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "own-load: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("own-load: MhHook::new failed: {status:?}"));
            false
        }
    }
}

// ===== WorldBlockRes::Update DIAGNOSTIC detour (worldblockres-phase-machine-drives-loadstate-to-0xa)
//
// PURPOSE: discriminate WHY the requested map block never reaches loadstate phase 0xa on the menu-free
// OWN-LOAD path. `WorldBlockRes::Update` (deobf 0x140614870) is the per-block phase state-machine
// (switch on the phase byte [this+0x35]); the 9->0xa transition fires ONLY when the FD4 file-load
// completion gate [this+0x2f]!=0. It is ticked per-block-per-frame by the FieldArea/WorldAreaRes
// block-update loop (FUN_14062f840 / FUN_14063a930) which our menu-free path may never run.
//
// The detour is OBSERVE-ONLY: it bumps a call counter, reads the phase byte ([+0x35]) and the gate
// byte ([+0x2f]) via FAULT-TOLERANT reads (never derefs raw), tracks the MAX phase seen and whether
// ANY block's gate was set, then calls the original (trampoline) and returns its return value
// UNCHANGED. No per-call logging (this is high-rate: ~33 blocks * per-frame) -- only atomics.
//
// READS:  wbr_update_calls==0 across the stall  => the FieldArea update loop is NOT ticking (cause 1).
//         calls>0 but max_phase<0xa & any_gate_set=false => loop ticks but the FD4 file-load never
//         completes -> the IO/CSFile path is the gap (cause 2).

/// `CS::WorldBlockRes::Update` real entry (deobf-grounded; the dump entry FUN_1406148e0 is +0x10).
const WORLDBLOCKRES_UPDATE_RVA: usize = 0x614870;
/// Phase byte the switch dispatches on: `this+0x35` (9 -> 0xa is the residency transition).
const WBR_PHASE_35_OFFSET: usize = 0x35;
/// FD4 file-load completion gate: `this+0x2f` (recomputed each tick; !=0 lets phase 9 advance to 0xa).
const WBR_GATE_2F_OFFSET: usize = 0x2f;

/// Total calls to `WorldBlockRes::Update` observed via the detour (per-block-per-frame; 0 == the
/// FieldArea update loop never ticked our block on this path).
pub(crate) static OWN_LOAD_WBR_UPDATE_CALLS: AtomicU64 = AtomicU64::new(0);
/// Max phase byte ([this+0x35]) seen across all observed calls. <0xa across the stall == the block's
/// resource-stream never reached residency.
pub(crate) static OWN_LOAD_WBR_MAX_PHASE: AtomicU64 = AtomicU64::new(0);
/// Whether ANY observed block had its FD4 completion gate ([this+0x2f]) set non-zero. false across the
/// stall == the FD4 file-load never completed for any block (the IO/CSFile gap).
pub(crate) static OWN_LOAD_WBR_ANY_GATE_SET: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Count of successful OWN-LOAD m28 `AddDefaultFileLoadProcess` dispatch calls (one per cap, one-shot
/// per cap pointer). 0 == the lever never fired. Exposed as telemetry `oracle_own_m28_dispatch_fired`.
pub(crate) static OWN_LOAD_M28_DISPATCH_FIRED: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard: FD4FileCap pointers we already dispatched `AddDefaultFileLoadProcess` for.
/// `AppendFileLoadProcessor` does NOT early-out on an already-present processor, so a double-call
/// would append a second processor -- this set makes each cap fire exactly once. Const-constructible
/// (`Mutex::new(Vec::new())`) so no lazy init is needed.
static OWN_LOAD_M28_DISPATCHED_CAPS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// Trampoline to the original `WorldBlockRes::Update` (set on hook install).
static WBR_UPDATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// GROUND-TRUTH cap-layout diagnostic: when `WorldBlockRes::Update`'s `this` (the REAL WBR, straight
/// from the engine) is at the stuck phase 2, dump its candidate cap fields READ-ONLY so we locate the
/// FD4FileCap on the authoritative object instead of reconstructing it from the resmgr container.
/// Throttled to the first few sightings (the hook fires ~500k times).
static WBR_PHASE2_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
const WBR_PHASE2_DIAG_MAX: usize = 24;
const WBR_STUCK_PHASE: u8 = 2;
/// One-shot install guard for the `WorldBlockRes::Update` diagnostic detour.
static WBR_UPDATE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// `__fastcall WorldBlockRes::Update(this)` diagnostic detour. `rcx` = WorldBlockRes* (`this`).
/// OBSERVE-ONLY: increments the call counter, fault-tolerantly reads [this+0x35] (phase) and
/// [this+0x2f] (gate), updates the max-phase / any-gate-set atomics, then ALWAYS calls the original
/// and returns its return value unchanged (the fn likely returns void/this; declaring usize and
/// passing through the original's return value is safe for both void and value returns). No load
/// behavior is altered and nothing is written into `this`.
pub(crate) unsafe extern "system" fn wbr_update_hook(this: usize) -> usize {
    OWN_LOAD_WBR_UPDATE_CALLS.fetch_add(1, Ordering::SeqCst);
    if this != TITLE_OWNER_SCAN_START_ADDRESS {
        if let Some(phase) = unsafe { safe_read_u8(this + WBR_PHASE_35_OFFSET) } {
            OWN_LOAD_WBR_MAX_PHASE.fetch_max(u64::from(phase), Ordering::SeqCst);
            // Ground-truth cap-layout dump on the REAL WBR at the stuck phase 2 (throttled, read-only).
            if phase == WBR_STUCK_PHASE {
                let n = WBR_PHASE2_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
                if n < WBR_PHASE2_DIAG_MAX {
                    let rd = |p: usize| unsafe { safe_read_usize(p) }.unwrap_or(0);
                    let info = rd(this + BLOCK_INNER_8_OFFSET);
                    let area = if info != 0 {
                        unsafe { safe_read_i32(info + BLOCK_AREA_C_OFFSET) }.unwrap_or(-1) & 0xff
                    } else {
                        -1
                    };
                    // Decisive phase-2->3 gate read on the REAL WBR: for BOTH caps, loadState as a
                    // BYTE (the engine does movzbl 0x88; ==4 complete) + the +0x90 byte-count + +0x78
                    // load-process. The block advances 2->3 only when BOTH caps reach loadState 4 with
                    // +0x90 != 0, so this shows exactly which cap is the holdout.
                    let cap0 = rd(this + WORLDBLOCKRES_FILECAP_40_OFFSET);
                    let cap1 = rd(this + WORLDBLOCKRES_FILECAP2_48_OFFSET);
                    let capb = |cap: usize, off: usize| -> i32 {
                        if cap != 0 {
                            unsafe { safe_read_u8(cap + off) }
                                .map(i32::from)
                                .unwrap_or(-1)
                        } else {
                            -1
                        }
                    };
                    let capq = |cap: usize, off: usize| -> usize {
                        if cap != 0 { rd(cap + off) } else { 0 }
                    };
                    let ls0 = capb(cap0, FILECAP_LOADSTATE_88_OFFSET);
                    let ls1 = capb(cap1, FILECAP_LOADSTATE_88_OFFSET);
                    let by0 = capq(cap0, 0x90);
                    let by1 = capq(cap1, 0x90);
                    let lp0 = capq(cap0, FILECAP_LOAD_PROCESS_78_OFFSET);
                    let lp1 = capq(cap1, FILECAP_LOAD_PROCESS_78_OFFSET);
                    let gate2f = unsafe { safe_read_u8(this + WBR_GATE_2F_OFFSET) }.unwrap_or(255);
                    let flag2d = unsafe { safe_read_u8(this + 0x2d) }.unwrap_or(255);
                    append_autoload_debug(format_args!(
                        "wbr-phase2: this=0x{this:x} area=0x{area:x} container=0x{:x} +0x2d={flag2d} +0x2f(gate)={gate2f} cap0=0x{cap0:x} ls0={ls0} by0=0x{by0:x} lp0=0x{lp0:x} | cap1=0x{cap1:x} ls1={ls1} by1=0x{by1:x} lp1=0x{lp1:x} #{n}",
                        rd(this + 0x18)
                    ));
                    // FD4FileCap header sweep (cap0) to locate the requested-resource NAME pointer
                    // (FD4ResCap-style name string near the start) + the +0xa0/+0xa8 fields, so we can
                    // tell whether the load completed empty because the request was built with no/blank
                    // file (our-path bug) or a real .dcx whose archive simply isn't mounted. Also probe
                    // the alt-gate object *(WBR+0x8) and its +0x28. All READ-ONLY.
                    if cap0 != 0 && n < 4 {
                        let info8 = rd(this + 0x08);
                        append_autoload_debug(format_args!(
                            "wbr-phase2-cap0: cap0=0x{cap0:x} +00=0x{:x} +08=0x{:x} +10=0x{:x} +18=0x{:x} +20=0x{:x} +28=0x{:x} +30=0x{:x} +38=0x{:x} +40=0x{:x} +48=0x{:x} +50=0x{:x} +98=0x{:x} +a0=0x{:x} +a8=0x{:x} | info8=0x{info8:x} info8+0x28=0x{:x} #{n}",
                            rd(cap0 + 0x00),
                            rd(cap0 + 0x08),
                            rd(cap0 + 0x10),
                            rd(cap0 + 0x18),
                            rd(cap0 + 0x20),
                            rd(cap0 + 0x28),
                            rd(cap0 + 0x30),
                            rd(cap0 + 0x38),
                            rd(cap0 + 0x40),
                            rd(cap0 + 0x48),
                            rd(cap0 + 0x50),
                            rd(cap0 + 0x98),
                            rd(cap0 + 0xa0),
                            rd(cap0 + 0xa8),
                            if info8 != 0 { rd(info8 + 0x28) } else { 0 }
                        ));
                    }
                }
            }
        }
        if let Some(gate) = unsafe { safe_read_u8(this + WBR_GATE_2F_OFFSET) } {
            if gate != 0 {
                OWN_LOAD_WBR_ANY_GATE_SET.store(true, Ordering::SeqCst);
            }
        }
    }
    let orig = WBR_UPDATE_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let orig: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { orig(this) }
}

/// Install the OBSERVE-ONLY `WorldBlockRes::Update` diagnostic detour (MhHook + MH_Initialize +
/// queue_enable + MH_ApplyQueued), mirroring `install_own_load_hook`. Idempotent. The detour is a
/// pure-read pass-through, so installing it early (when own_load is armed) leaves normal play
/// untouched and never changes load behavior.
pub(crate) fn install_wbr_update_hook() -> bool {
    if WBR_UPDATE_HOOK_INSTALLED.load(Ordering::SeqCst) == OWN_STEPPER_CALL_INC {
        return true;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("wbr-update: MH_Initialize failed: {status:?}"));
            return false;
        }
    }
    let Ok(update_addr) = game_rva(WORLDBLOCKRES_UPDATE_RVA as u32) else {
        append_autoload_debug(format_args!(
            "wbr-update: failed to resolve 0x{WORLDBLOCKRES_UPDATE_RVA:x} rva"
        ));
        return false;
    };
    match unsafe { MhHook::new(update_addr as *mut c_void, wbr_update_hook as *mut c_void) } {
        Ok(hook) => {
            WBR_UPDATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("wbr-update: queue_enable failed: {status:?}"));
                return false;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    WBR_UPDATE_HOOK_INSTALLED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "wbr-update: hooked 0x{update_addr:x} (OBSERVE-ONLY phase/gate diagnostic; pure pass-through)"
                    ));
                    true
                }
                status => {
                    append_autoload_debug(format_args!(
                        "wbr-update: MH_ApplyQueued failed: {status:?}"
                    ));
                    false
                }
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("wbr-update: MhHook::new failed: {status:?}"));
            false
        }
    }
}

/// Locate the on-disk save file (`.../EldenRing/<steamid>/ER0000.sl2` or `.co2`) and read its bytes.
/// The directory is built by the NATIVE builder 0x140e0e680 (`SAVE_DIR_BUILDER_RVA`) -- the same
/// path the engine uses -- so we never hardcode the user-data/steamid prefix. Inside that directory
/// we pick the save file by extension (`.sl2`/`.co2`) rather than assuming an exact filename, so the
/// probe works for vanilla and Seamless without a hardcoded name (bd dont-hardcode-savefile-tied).
unsafe fn own_load_read_sl2_bytes(base: usize) -> Option<Vec<u8>> {
    const REQ_DIR_SANE_MAX_CU: usize = 320;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Build the canonical save directory into a stack-resident MSVC stateful-allocator u16string
    // wrapper (allocator@+0, data@+0x08, size@+0x18, cap@+0x20) -- identical to the cold-char-mount
    // SAVE-DIR BUILD step, reusing the native builder so the path matches the engine's.
    let mut wrapper = [0u64; 8];
    let wbase = wrapper.as_mut_ptr() as usize;
    let alloc_getter: unsafe extern "system" fn() -> usize =
        unsafe { std::mem::transmute(base + SAVE_DIR_ALLOC_GETTER_RVA) };
    let allocator = unsafe { alloc_getter() };
    unsafe {
        *((wbase + U16STRING_ALLOC_OFFSET) as *mut usize) = allocator;
        *((wbase + U16STRING_CAP_OFFSET) as *mut usize) = U16STRING_SSO_CAP;
    }
    // The builder derefs the Steam interface (*0x143b48ff0) for the account id; bail (logging) if it
    // is null cold (Steam not live) rather than crashing.
    let steam_iface = unsafe { safe_read_usize(base + STEAM_INTERFACE_GUARD_RVA) }.unwrap_or(null);
    if steam_iface == null || allocator == null {
        append_autoload_debug(format_args!(
            "own-load: SAVE-DIR build skipped steam_iface=0x{steam_iface:x} allocator=0x{allocator:x} (need both non-null) -- cannot locate .sl2"
        ));
        return None;
    }
    let builder: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + SAVE_DIR_BUILDER_RVA) };
    unsafe { builder(wbase) };
    let dir_cap = unsafe { *((wbase + U16STRING_CAP_OFFSET) as *const usize) };
    let dir_size = unsafe { *((wbase + U16STRING_SIZE_OFFSET) as *const usize) };
    let dir_data = if dir_cap >= 8 {
        unsafe { *((wbase + U16STRING_DATA_OFFSET) as *const usize) }
    } else {
        wbase + U16STRING_DATA_OFFSET
    };
    // Decode the UTF-16 directory into a Rust path string (fault-safe, bounded).
    let mut dir = String::new();
    if dir_data != null && dir_size != 0 && dir_size <= REQ_DIR_SANE_MAX_CU {
        let words = dir_size.div_ceil(4);
        'decode: for w in 0..words {
            let Some(word) = (unsafe { safe_read_usize(dir_data + w * 8) }) else {
                break;
            };
            for b in 0..4 {
                let cu = ((word >> (b * 16)) & 0xffff) as u16;
                if cu == 0 || w * 4 + b >= dir_size {
                    break 'decode;
                }
                dir.push(char::from_u32(cu as u32).unwrap_or('?'));
            }
        }
    }
    if dir.is_empty() {
        append_autoload_debug(format_args!(
            "own-load: SAVE-DIR builder returned empty (cap={dir_cap} size={dir_size}) -- cannot locate .sl2"
        ));
        return None;
    }
    // The native dir uses backslashes (Windows under Proton); normalise for std::fs lookup.
    let dir_path = PathBuf::from(dir.replace('\\', "/"));
    // Pick the save file by extension, not a hardcoded name: prefer .sl2 (vanilla), then .co2
    // (Seamless). This matches whichever container the active runtime actually wrote.
    let paths: Vec<PathBuf> = std::fs::read_dir(&dir_path)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    let mut chosen: Option<PathBuf> = None;
    for ext in ["sl2", "co2"] {
        if let Some(p) = paths
            .iter()
            .find(|p| p.extension().and_then(|e| e.to_str()) == Some(ext))
        {
            chosen = Some(p.clone());
            break;
        }
    }
    let Some(path) = chosen else {
        append_autoload_debug(format_args!(
            "own-load: no .sl2/.co2 file under dir=\"{}\" -- cannot read save",
            dir_path.display()
        ));
        return None;
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            append_autoload_debug(format_args!(
                "own-load: read save file \"{}\" ({} bytes) for slicing",
                path.display(),
                bytes.len()
            ));
            Some(bytes)
        }
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load: failed to read save file \"{}\": {e}",
                path.display()
            ));
            None
        }
    }
}

/// How often (in own_stepper frames) the OWN-LOAD world-stream stall telemetry emits a throttled
/// debug line. The oracle_* atomics are refreshed EVERY frame; only the human-readable log is
/// throttled so a probe log shows the trend without flooding.
pub(crate) const OWN_LOAD_STREAM_LOG_INTERVAL: u64 = 30;
/// MoveMapStep step-machine state field offset (mms_state). The step machine commits its current
/// step at +0x48 (same layout as the title owner committed_state); STEP_WorldResWait == 3 is the
/// observed stall floor. Read [[InGameStep(owner+0x2e8)+0xe8]+0x48].
pub(crate) const MOVEMAPSTEP_STATE_48_OFFSET: usize = TITLE_OWNER_STATE_COMMITTED_OFFSET;

/// SAVE-SAFE per-frame OWN-LOAD world-stream stall telemetry. PURE READS ONLY (safe_read_*; never
/// changes load behavior). Walks the deepest world-load pump chain each frame and publishes the
/// values to the OWN_LOAD_STREAM_* oracle atomics, plus a throttled human-readable debug line, so a
/// probe log reveals whether ANY value advances over time (progress) or all are frozen (genuine
/// stall). Gated to the own_load path only -- the caller invokes this exclusively inside the
/// `own_load_enabled()` branch, so it never spams during normal play.
///
/// Chain (full-pipeline-traced-to-worldreswait-map-block-streaming):
///   title_owner+0x48 = committed/live title state (5 == PlayGame after SetState5)
///   title_owner+0x4c = requested/next title state
///   InGameStep = [title_owner+0x2e8] (load_job); MoveMapStep = [InGameStep+0xe8]
///   mms_state   = [MoveMapStep+0x48]      (STEP_WorldResWait == 3 == the stall floor)
///   resmgr      = [[MoveMapStep+0xf0]+0x10]; block_count = [resmgr+0xb3140]
///   req_coord   = [[MoveMapStep+0xf0]+0x2c]
///   iodev       = [base+IODEV_GLOBAL_RVA]; inflight = [iodev+0x10]; reqhandle = [iodev+0x20]
///   c30         = [gm+0xc30]
unsafe fn own_load_stream_telemetry(base: usize, gm: usize, title_owner: usize, n: u64) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Fault-tolerant deref helper: returns Some(child) only on a real, non-null read.
    let deref = |addr: usize| -> Option<usize> {
        match unsafe { safe_read_usize(addr) } {
            Some(v) if v != null => Some(v),
            _ => None,
        }
    };
    // Title owner state fields (owner+0x48 committed, owner+0x4c requested).
    let owner_state = if title_owner != null {
        unsafe { safe_read_i32(title_owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    let owner_req_state = if title_owner != null {
        unsafe { safe_read_i32(title_owner + TITLE_OWNER_STATE_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // InGameStep (owner+0x2e8) -> MoveMapStep (+0xe8) -> mms_state (+0x48).
    let ingame = if title_owner != null {
        deref(title_owner + TITLE_OWNER_JOB_OFFSET)
    } else {
        None
    };
    let movemapstep = ingame.and_then(|ig| deref(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET));
    let mms_state = match movemapstep {
        Some(mms) => unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // World-resource manager chain: resmgr = [[MoveMapStep+0xf0]+0x10]; block_count = [resmgr+0xb3140].
    let resmgr = movemapstep
        .and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET))
        .and_then(|wrm| deref(wrm + WORLDRES_RESMGR_10_OFFSET));
    let block_count = match resmgr {
        Some(rm) => unsafe { safe_read_i32(rm + RESMGR_BLOCK_COUNT_B3140_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // Requested world coord/map-id ([[MoveMapStep+0xf0]+0x2c]).
    let req_coord = match movemapstep.and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET)) {
        Some(wrm) => unsafe { safe_read_usize(wrm + WORLDRES_COORD_2C_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // IO device inflight / started-request handle.
    let iodev = deref(base + IODEV_GLOBAL_RVA);
    let io_inflight = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_INFLIGHT_10_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let io_reqhandle = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_REQHANDLE_20_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // GameMan+0xc30 saved-map id (the streamed map).
    let c30 = if gm != null {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // Publish every frame (the oracle_* fields are the machine-readable progress signal).
    OWN_LOAD_STREAM_OWNER_STATE.store(owner_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_OWNER_REQ_STATE.store(owner_req_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_MMS_STATE.store(mms_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_BLOCK_COUNT.store(block_count, Ordering::SeqCst);
    OWN_LOAD_STREAM_REQ_COORD.store(req_coord, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_INFLIGHT.store(io_inflight, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_REQHANDLE.store(io_reqhandle, Ordering::SeqCst);
    OWN_LOAD_STREAM_C30.store(c30, Ordering::SeqCst);
    let frames = OWN_LOAD_STREAM_FRAMES.fetch_add(1, Ordering::SeqCst);
    // Throttled human-readable trend line.
    if frames % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        let ig = ingame.unwrap_or(null);
        let mms = movemapstep.unwrap_or(null);
        let rm = resmgr.unwrap_or(null);
        append_autoload_debug(format_args!(
            "own-load-stream: frame={frames} (n={n}) owner_state={owner_state} owner_req={owner_req_state} ingame=0x{ig:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{rm:x} block_count={block_count} req_coord=0x{req_coord:x} io_inflight=0x{io_inflight:x} io_reqhandle=0x{io_reqhandle:x} c30=0x{c30:x}"
        ));
    }
}

/// Diagnostic throttle for `own_load_m28_dispatch`: log the first HEAD entries, then every INTERVALth.
static OWN_LOAD_M28_DISPATCH_DIAG_CALLS: AtomicUsize = AtomicUsize::new(0);
const OWN_LOAD_M28_DISPATCH_DIAG_HEAD: usize = 8;
const OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL: usize = 600;

/// OWN-LOAD m28 direct-enqueue lever (adddefaultfileloadprocess-lever-viable-2026-06-22). `block` is
/// the matched player-area (m28, 0x1c) `WorldBlockRes`. For each of its FD4FileCap slots (+0x40 primary,
/// +0x48 optional second) this: skips null caps, skips caps already resident (`loadState +0x88 == 4`),
/// skips caps we already dispatched (one-shot per cap pointer), reads the cap's EXISTING
/// `FD4FileLoadProcess*` at +0x78, then calls `FD4::FD4FileCap::AddDefaultFileLoadProcess(cap, lp)`,
/// which builds the processor internally and self-enqueues IO to the already-live FD4 workers. Every
/// pointer read is fault-tolerant (`deref` / `safe_read_*`) and the native call is wrapped in
/// `catch_unwind` so a fault can never unwind across the FFI boundary into the FD4 task. SAVE-SAFE:
/// reaches only world-asset file-load streaming (RequestDCX -> RSResourceFileRequest ->
/// GLOBAL_LoadManager); it does NOT touch save IO and cannot autosave.
unsafe fn own_load_m28_dispatch(
    base: usize,
    block: usize,
    deref: &impl Fn(usize) -> Option<usize>,
) {
    // Throttled diagnostics: this helper runs once per matched player-area block per observer frame,
    // so logging every frame would flood the log. Log the first few entries and then every
    // OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL-th, plus ALWAYS on an actual dispatch / panic / lp-null.
    // Seeing ANY of these lines proves the gate passed (own_dispatch armed + continue fired) and the
    // helper was entered -- so it disambiguates "gate off" (no line at all) from "caps null/resident"
    // (skip lines), which is exactly why the lever was a silent no-op on the first clean run.
    let diag_n = OWN_LOAD_M28_DISPATCH_DIAG_CALLS.fetch_add(1, Ordering::SeqCst);
    let diag = diag_n < OWN_LOAD_M28_DISPATCH_DIAG_HEAD
        || diag_n % OWN_LOAD_M28_DISPATCH_DIAG_INTERVAL == 0;
    // The resmgr 0xb3030 array entry `block` is a WRAPPER (WorldBlockData), NOT the WorldBlockRes --
    // its +0x40/+0x48 are unrelated wrapper fields (observed null at runtime). The real WorldBlockRes
    // (FD4FileCaps at +0x40/+0x48, phase byte at +0x35) is what the engine reaches via the native
    // getter `block->vtable[+0x10](block)` (canonical scanner 0x14066d3e0; phase handlers 0x1406157f0 /
    // 0x140615340). SAFETY PIVOT (confirmed 2x: process_exited_before_ready @ ~767 game-task ticks with
    // NO diag line written -> the getter CALL itself AV-faulted BEFORE any logging; a hardware AV is not
    // a Rust panic so catch_unwind cannot contain it; matches the prior menu-free getter-fault memory).
    // So we do NOT call the getter. This pass is VERIFY-ONLY / READ-ONLY: capture the getter address
    // (for static disasm) and sweep the wrapper's fields to locate the WorldBlockRes pointer by
    // signature (a field P where P+0x35 is a small phase byte and P+0x40/+0x48 are FD4FileCaps). NO
    // native call is made here, so this cannot crash the game. The real dispatch is re-enabled once the
    // WBR field offset is grounded from this sweep + the getter disassembly.
    let Some(vtbl) = deref(block) else {
        return;
    };
    let getter_addr = deref(vtbl + BLOCK_LOADSTATE_GETTER_VT_10_OFFSET).unwrap_or(0);
    if diag {
        let q = |off: usize| deref(block + off).unwrap_or(0);
        append_autoload_debug(format_args!(
            "own-load-m28-dispatch: VERIFY block=0x{block:x} vtbl=0x{vtbl:x} getter@vt+0x10=0x{getter_addr:x} base=0x{base:x} call#{diag_n}"
        ));
        append_autoload_debug(format_args!(
            "own-load-m28-dispatch: WRAP-SWEEP block=0x{block:x} +00=0x{:x} +08=0x{:x} +10=0x{:x} +18=0x{:x} +20=0x{:x} +28=0x{:x} +30=0x{:x} +38=0x{:x} +40=0x{:x} +48=0x{:x} +50=0x{:x} +58=0x{:x} +60=0x{:x} +68=0x{:x} +70=0x{:x} +78=0x{:x} +80=0x{:x} +88=0x{:x} +90=0x{:x} +98=0x{:x} +a0=0x{:x} +a8=0x{:x}",
            q(0x00),
            q(0x08),
            q(0x10),
            q(0x18),
            q(0x20),
            q(0x28),
            q(0x30),
            q(0x38),
            q(0x40),
            q(0x48),
            q(0x50),
            q(0x58),
            q(0x60),
            q(0x68),
            q(0x70),
            q(0x78),
            q(0x80),
            q(0x88),
            q(0x90),
            q(0x98),
            q(0xa0),
            q(0xa8)
        ));
        // Container layout (decoded from getter 0x14062f470): WorldBlockRes elements live in an inline
        // array at *(block+0xce0), count *(block+0xcd8), stride 0xb98; caps at element+0x40/+0x48. Dump
        // count, array base, and element-0's phase/caps + cap0's loadState(+0x88)/lp(+0x78) READ-ONLY to
        // confirm the layout before enabling the array-iteration dispatch.
        let count =
            unsafe { safe_read_i32(block + WORLDBLOCK_CONTAINER_COUNT_CD8_OFFSET) }.unwrap_or(-1);
        let arr = deref(block + WORLDBLOCK_CONTAINER_ARRAY_CE0_OFFSET).unwrap_or(0);
        let elem0 = arr; // element 0 = arr + 0*0xb98
        let e_phase =
            unsafe { safe_read_i32(elem0 + BLOCK_LOADSTATE_PHASE_35_OFFSET) }.unwrap_or(-1) & 0xff;
        let cap0 = deref(elem0 + WORLDBLOCKRES_FILECAP_40_OFFSET).unwrap_or(0);
        let cap1 = deref(elem0 + WORLDBLOCKRES_FILECAP2_48_OFFSET).unwrap_or(0);
        let c0_ls = unsafe { safe_read_i32(cap0 + FILECAP_LOADSTATE_88_OFFSET) }.unwrap_or(-1);
        let c0_lp = deref(cap0 + FILECAP_LOAD_PROCESS_78_OFFSET).unwrap_or(0);
        let c0_90 = deref(cap0 + 0x90).unwrap_or(0);
        append_autoload_debug(format_args!(
            "own-load-m28-dispatch: CONTAINER block=0x{block:x} count={count} arr=0x{arr:x} stride=0x{stride:x} elem0=0x{elem0:x} elem0_phase=0x{e_phase:x} cap0(+0x40)=0x{cap0:x} cap1(+0x48)=0x{cap1:x} | cap0+0x78(lp)=0x{c0_lp:x} cap0+0x88(loadState)={c0_ls} cap0+0x90=0x{c0_90:x}",
            stride = WORLDBLOCKRES_ELEM_STRIDE_B98
        ));
    }
}

/// SAVE-SAFE RECURRING world-stream observer, called from the per-frame GAME TASK (NOT the
/// title-phase own_stepper_idx10, which stops ticking once SetState5 starts the title->ingame
/// transition). Runs when `OWN_LOAD_CONTINUE_FIRED` (our menu-free OWN-LOAD path) OR
/// `golden_observe_enabled()` (GOLDEN baseline mode observing a user-driven vanilla load) is set, so it
/// never spams during normal play. PURE READS ONLY (safe_read_*; never changes load behavior). In
/// golden mode `OWN_LOAD_OWNER_CACHED` is filled by own_stepper_idx10 each title frame and the cached
/// InGameStep stays 0, so the live `ingame_cached == 0` re-derivation below resolves the chain fresh
/// every frame as the vanilla load builds the world.
///
/// It re-reads the world-stream from the CACHED title owner + InGameStep (snapshotted at fire time),
/// NOT from a fresh own_stepper owner, so it keeps observing through the whole loading screen:
///   owner       = OWN_LOAD_OWNER_CACHED              (cached at continue_confirm fire)
///   InGameStep  = OWN_LOAD_INGAMESTEP_CACHED         (== owner+0x2e8 at fire; non-null at frame 0)
///   MoveMapStep = [InGameStep+0xe8]                  (INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET)
///   mms_state   = [MoveMapStep+0x48]                 (MOVEMAPSTEP_STATE_48_OFFSET; 3 == WorldResWait)
///   resmgr      = [[MoveMapStep+0xf0]+0x10]          (MOVEMAPSTEP_WORLDRES_F0_OFFSET / WORLDRES_RESMGR_10_OFFSET)
///   block_count = [resmgr+0xb3140]                   (RESMGR_BLOCK_COUNT_B3140_OFFSET)
///   owner_state = [owner+0x48]                       (TITLE_OWNER_STATE_COMMITTED_OFFSET)
///   c30         = [gm+0xc30]                          (GAME_MAN_SAVED_MAP_C30_OFFSET)
///   player_present is resolved by the caller (WorldChrMan/PlayerIns) and passed in.
///
/// `frame=N` advances every active frame (OWN_LOAD_STREAM_RECUR_FRAMES) so a probe sees whether
/// mms_state advances/sticks and whether block_count stays 0 vs grows ACROSS the loading screen.
/// Publishes the SAME oracle_own_load_stream_* fields so they keep updating through the load.
pub(crate) unsafe fn own_load_stream_observe_recurring(
    base: usize,
    gm: usize,
    player_present: bool,
) {
    // Run after our own continue_confirm fired (OWN-LOAD path) OR in GOLDEN baseline mode (observing a
    // user-driven vanilla load). Golden mode supplies `owner` via own_stepper_idx10's per-frame cache
    // and leaves the cached InGameStep at 0, so the `ingame_cached == 0` fallback below re-derives the
    // chain LIVE each frame. Either way this stays pure-read and never changes load behavior.
    if !OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst) && !golden_observe_enabled() {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let deref = |addr: usize| -> Option<usize> {
        match unsafe { safe_read_usize(addr) } {
            Some(v) if v != null && v != 0 => Some(v),
            _ => None,
        }
    };
    let owner = OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst);
    let ingame_cached = OWN_LOAD_INGAMESTEP_CACHED.load(Ordering::SeqCst);
    // owner+0x48 committed state (5 == PlayGame/streaming after SetState5).
    let owner_state = if owner != null && owner != 0 {
        unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    let owner_req_state = if owner != null && owner != 0 {
        unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // Prefer the cached InGameStep; if that snapshot is null/0, re-derive from the cached owner.
    let ingame = if ingame_cached != null && ingame_cached != 0 {
        Some(ingame_cached)
    } else if owner != null && owner != 0 {
        deref(owner + TITLE_OWNER_JOB_OFFSET)
    } else {
        None
    };
    let movemapstep = ingame.and_then(|ig| deref(ig + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET));
    let mms_state = match movemapstep {
        Some(mms) => unsafe { safe_read_i32(mms + MOVEMAPSTEP_STATE_48_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let resmgr = movemapstep
        .and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET))
        .and_then(|wrm| deref(wrm + WORLDRES_RESMGR_10_OFFSET));
    let block_count = match resmgr {
        Some(rm) => unsafe { safe_read_i32(rm + RESMGR_BLOCK_COUNT_B3140_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let req_coord = match movemapstep.and_then(|mms| deref(mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET)) {
        Some(wrm) => unsafe { safe_read_usize(wrm + WORLDRES_COORD_2C_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let iodev = deref(base + IODEV_GLOBAL_RVA);
    let io_inflight = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_INFLIGHT_10_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let io_reqhandle = match iodev {
        Some(dev) => unsafe { safe_read_usize(dev + IODEV_REQHANDLE_20_OFFSET) }
            .map(|v| v as i64)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let c30 = if gm != null && gm != 0 {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    let player_present_i64 = i64::from(player_present);
    // play_game_submit handoff discriminators (PURE READS, no call). InGameStep+0xd8 = pending phase,
    // InGameStep+0x100 = requested BlockId. req_blockid == saved BlockId means play_game_submit ran;
    // 0/unset means it did not. UNREAD if the InGameStep handle is null.
    let ingame_phase = match ingame {
        Some(ig) => unsafe { safe_read_i32(ig + INGAMESTEP_PHASE_D8_OFFSET) }
            .map(i64::from)
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    let req_blockid = match ingame {
        Some(ig) => unsafe { safe_read_usize(ig + INGAMESTEP_REQ_BLOCKID_100_OFFSET) }
            .map(|v| i64::from(v as u32))
            .unwrap_or(OWN_LOAD_STREAM_FIELD_UNREAD),
        None => OWN_LOAD_STREAM_FIELD_UNREAD,
    };
    // resmgr block-array scan (PURE READS, NO block->vtable call this round). The target areaId is
    // DERIVED from req_coord (the low dword's high byte), not hardcoded. We count how many registered
    // blocks match the target (presence == the registration-vs-streaming discriminator) and collect
    // the first OBSERVER_AREAID_SAMPLE_MAX distinct areaIds for the log. Scan is clamped to
    // min(block_count, OBSERVER_BLOCK_SCAN_CAP) and every deref is null/fault-tolerant.
    let target_area: u8 = if req_coord != OWN_LOAD_STREAM_FIELD_UNREAD {
        (((req_coord as u32) >> TARGET_AREA_FROM_COORD_SHIFT) & TARGET_AREA_FROM_COORD_MASK) as u8
    } else {
        0
    };
    let mut target_block_count: i64 = 0;
    let mut distinct_areaids: Vec<u8> = Vec::with_capacity(OBSERVER_AREAID_SAMPLE_MAX);
    let mut scan_chain_ok = false;
    if let (Some(rm), true) = (resmgr, block_count != OWN_LOAD_STREAM_FIELD_UNREAD) {
        if block_count > 0 {
            scan_chain_ok = true;
            let base_arr = rm + RESMGR_BLOCK_ARRAY_B3030_OFFSET;
            let n = block_count.min(OBSERVER_BLOCK_SCAN_CAP);
            let mut i: i64 = 0;
            while i < n {
                let slot = base_arr + (i as usize) * BLOCK_ENTRY_STRIDE;
                if let Some(block) = deref(slot) {
                    if let Some(inner) = deref(block + BLOCK_INNER_8_OFFSET) {
                        if let Some(area_u8) =
                            unsafe { safe_read_usize(inner + BLOCK_AREA_C_OFFSET) }
                                .map(|v| (v as u32 & TARGET_AREA_FROM_COORD_MASK) as u8)
                        {
                            if area_u8 == target_area {
                                target_block_count += 1;
                                // `block` IS the matched player-area (m28, 0x1c) WorldBlockRes.
                                // Drive its FD4FileCap(s) to residency via the direct-enqueue lever.
                                // Double-gated: own_dispatch armed AND our OWN-LOAD continue fired.
                                if own_dispatch_enabled()
                                    && OWN_LOAD_CONTINUE_FIRED.load(Ordering::SeqCst)
                                {
                                    unsafe {
                                        own_load_m28_dispatch(base, block, &deref);
                                    }
                                }
                            }
                            if distinct_areaids.len() < OBSERVER_AREAID_SAMPLE_MAX
                                && !distinct_areaids.contains(&area_u8)
                            {
                                distinct_areaids.push(area_u8);
                            }
                        }
                    }
                }
                i += 1;
            }
        }
    }
    let target_block_present: i64 = if scan_chain_ok {
        i64::from(target_block_count > 0)
    } else {
        OWN_LOAD_STREAM_FIELD_UNREAD
    };
    // Publish every frame (the oracle_* fields are the machine-readable progress signal); these now
    // keep updating THROUGH the loading screen because this runs in the recurring game task.
    OWN_LOAD_STREAM_OWNER_STATE.store(owner_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_OWNER_REQ_STATE.store(owner_req_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_MMS_STATE.store(mms_state, Ordering::SeqCst);
    OWN_LOAD_STREAM_BLOCK_COUNT.store(block_count, Ordering::SeqCst);
    OWN_LOAD_STREAM_REQ_COORD.store(req_coord, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_INFLIGHT.store(io_inflight, Ordering::SeqCst);
    OWN_LOAD_STREAM_IO_REQHANDLE.store(io_reqhandle, Ordering::SeqCst);
    OWN_LOAD_STREAM_C30.store(c30, Ordering::SeqCst);
    OWN_LOAD_STREAM_PLAYER_PRESENT.store(player_present_i64, Ordering::SeqCst);
    OWN_LOAD_STREAM_INGAME_PHASE.store(ingame_phase, Ordering::SeqCst);
    OWN_LOAD_STREAM_REQ_BLOCKID.store(req_blockid, Ordering::SeqCst);
    OWN_LOAD_STREAM_TARGET_BLOCK_PRESENT.store(target_block_present, Ordering::SeqCst);
    // WorldBlockRes::Update diagnostic atomics (updated by the wbr_update_hook detour). These tell us
    // whether the per-block phase machine is ticked AT ALL on our path, and how far any block's phase
    // advanced / whether the FD4 completion gate ever fired -- the cause-1-vs-cause-2 discriminator.
    let wbr_calls = OWN_LOAD_WBR_UPDATE_CALLS.load(Ordering::SeqCst);
    let wbr_max_phase = OWN_LOAD_WBR_MAX_PHASE.load(Ordering::SeqCst);
    let wbr_any_gate_set = OWN_LOAD_WBR_ANY_GATE_SET.load(Ordering::SeqCst);
    let frames = OWN_LOAD_STREAM_RECUR_FRAMES.fetch_add(1, Ordering::SeqCst);
    if frames % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        let ig = ingame.unwrap_or(null);
        let mms = movemapstep.unwrap_or(null);
        let rm = resmgr.unwrap_or(null);
        append_autoload_debug(format_args!(
            "own-load-stream: frame={frames} (recurring) owner=0x{owner:x} owner_state={owner_state} owner_req={owner_req_state} ingame=0x{ig:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{rm:x} block_count={block_count} req_coord=0x{req_coord:x} io_inflight=0x{io_inflight:x} io_reqhandle=0x{io_reqhandle:x} c30=0x{c30:x} player_present={player_present} wbr_update_calls={wbr_calls} wbr_max_phase=0x{wbr_max_phase:x} wbr_any_gate_set={wbr_any_gate_set}"
        ));
        // MENUJOB-SLOT CHECK (autoload-map-orchestrator-menujob): the native Continue installs the
        // deser->map-load CS::MenuJob at owner+0x130, which STEP_MenuJobWait ticks via ExecuteMenuJob.
        // Our SetState5 shortcut installs nothing there -> predict owner+0x130 == NULL. A null here while
        // the native path has a non-null MenuJob (MenuJobResult-family vtable) confirms the lever =
        // install a MenuJob at owner+0x130. READ-ONLY (no write, no call).
        let menujob = if owner != null {
            deref(owner + 0x130).unwrap_or(0)
        } else {
            0
        };
        let menujob_vt = if menujob != 0 {
            deref(menujob).unwrap_or(0)
        } else {
            0
        };
        // Job STATE sweep (menujob-lever-is-START-not-build): owner+0x130 is non-null on our path, so
        // the job is PRE-BUILT; the gap is whether the Continue-confirm STARTED it. Dump the job's
        // header (state field is ~+0x10 per FUN_1407915b0's dispatch on *(this+0x10); sweep neighbors to
        // be offset-robust). If the state never advances on our path (stays idle) while native advances
        // 0->1->2->3->5->6, the lever is the START, not a build. READ-ONLY.
        let js = |off: usize| {
            if menujob != 0 {
                unsafe { safe_read_i32(menujob + off) }.unwrap_or(-1)
            } else {
                -1
            }
        };
        append_autoload_debug(format_args!(
            "own-load-menujob: frame={frames} owner=0x{owner:x} owner+0x130=0x{menujob:x} vt=0x{menujob_vt:x} state[+08]={} [+10]={} [+14]={} [+18]={} [+20]={} (job pre-built; watching if it ever STARTS)",
            js(0x08),
            js(0x10),
            js(0x14),
            js(0x18),
            js(0x20)
        ));
        // Second registration-vs-streaming line: did play_game_submit's handoff run (ingame_phase /
        // req_blockid) and is the coord-derived target block REGISTERED (target_block_present) among
        // the scanned areaIds? Absent target block => registration gap; present but stuck => streaming.
        let present = target_block_present == i64::from(true);
        append_autoload_debug(format_args!(
            "own-load-blocks: frame={frames} ingame_phase={ingame_phase} req_blockid=0x{req_blockid:x} target_area=0x{target_area:x} target_block_present={present} target_block_count={target_block_count} areaids={distinct_areaids:02x?}"
        ));
    }
}

/// SAVE-SAFE verify-only OWN-LOAD buffer-feed drive (one-shot, phased). Reads the .sl2 from disk,
/// slices slot `want_slot`'s plaintext body, installs+arms the gated 0x67b100 hook, calls the native
/// parser 0x67b290(slot) in-process so it parses OUR body, then reads back GameMan+0xc30 + the
/// PlayerGameData fingerprint. NO SetState5, NO autosave, NO continue_confirm. Records presses==0.
unsafe fn own_load_drive(base: usize, gm: usize, owner: usize, want_slot: i32, n: u64) {
    const PHASE_INIT: usize = 0;
    const PHASE_DONE: usize = 1;
    const C30_ZERO: i32 = 0;
    static OWN_LOAD_PHASE: AtomicUsize = AtomicUsize::new(PHASE_INIT);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = OWN_LOAD_PHASE.load(Ordering::SeqCst);
    // Publish phase+1 so the readiness watcher tears down on terminal completion (PHASE_DONE -> 2).
    OWN_LOAD_PHASE_PUB.store(phase + 1, Ordering::SeqCst);
    if phase != PHASE_INIT {
        return;
    }
    if gm == null {
        return;
    }
    if want_slot < OWN_STEPPER_SLOT_ZERO {
        append_autoload_debug(format_args!(
            "own-load: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-effects-autoload.txt -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    // (1) Read + slice the plaintext slot body. er_save_loader::bnd4 is the only glue: the engine's
    // read path is FSM-gated, so OWN-LOAD must hand it the buffer itself (bd reuse-native-fns).
    let Some(sl2_bytes) = (unsafe { own_load_read_sl2_bytes(base) }) else {
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    };
    let body: &[u8] = match er_save_loader::bnd4::slot_body(&sl2_bytes, want_slot as usize) {
        Ok(b) => b,
        Err(e) => {
            append_autoload_debug(format_args!(
                "own-load: slot_body(slot={want_slot}) failed: {e:?} -- ABORT (no-write)"
            ));
            OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
            return;
        }
    };
    // Leak the sliced body so it outlives this frame and stays valid for the detour to memcpy. One
    // copy of the (small fraction of the) save -- never the whole file -- kept for the session.
    let leaked: &'static [u8] = Box::leak(body.to_vec().into_boxed_slice());
    OWN_LOAD_BODY_PTR.store(leaked.as_ptr() as usize, Ordering::SeqCst);
    OWN_LOAD_BODY_LEN.store(leaked.len(), Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load: sliced slot {want_slot} body len=0x{:x} (expected 0x{:x}) -> install+arm gate, call native parser 0x{:x}",
        leaked.len(),
        er_save_loader::bnd4::SLOT_BODY_LEN,
        base + DESERIALIZE_SLOT_RVA
    ));
    // (2) Install the gated 0x67b100 detour (harmless pass-through until armed).
    if !install_own_load_hook() {
        append_autoload_debug(format_args!(
            "own-load: hook install failed -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    let c30_before = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    // (3) Set the gate, call native 0x67b290(slot) in-process, clear the gate. 0x67b290 does NOT
    // re-check b80 after the read (static-confirmed), so our al=1 + body flow into the native parse.
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
    let pret = unsafe { parser(want_slot) };
    OWN_LOAD_GATE.store(false, Ordering::SeqCst);
    let fed = OWN_LOAD_FED_BYTES.load(Ordering::SeqCst);
    // (4) VERIFY (read-back only): GameMan+0xc30 (map id) + the PlayerGameData char fingerprint.
    let c30 = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    let ac0 = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
    let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO && c30 != FULLREAD_C30_M10_DEFAULT;
    if c30_real && fp_real {
        OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
    }
    append_autoload_debug(format_args!(
        "own-load: VERIFY parser 0x{:x}(slot={want_slot}) ret={pret} fed_bytes=0x{fed:x} c30 0x{c30_before:x}->0x{c30:x} c30_real={c30_real} ac0={ac0} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) presses=0 (NO SetState5/NO save write)",
        base + DESERIALIZE_SLOT_RVA
    ));
    unsafe { dump_load_correctness(base, n) };
    // OWNER DIAGNOSTIC (er-effects-rs-mr2, save-safe pure reads): the prior continue crash used the
    // WRONG owner (*(GameDataMan+0x8)). Log EVERY continue_confirm owner candidate + each one's
    // +0x284 (new-game flag) byte so a VERIFY-ONLY run reveals which is the SetState-able title
    // owner BEFORE we ever fire continue_confirm. This is independent of the gated continue step.
    //   title  = the threaded SetState-able title owner the caller validated (own_stepper_idx10),
    //   recipe = *(base + CONTINUE_MANAGER_GLOBAL_RVA + 8)  (the native-fullread COMMIT recipe's literal),
    //   mgr_vt = *(base + CONTINUE_MANAGER_GLOBAL_RVA)      (the manager object's vtable ptr),
    //   gdm8   = *(GameDataMan + 0x8)                       (the prior crash owner).
    let read284 = |obj: usize| -> u8 {
        if obj == null {
            0
        } else {
            unsafe { safe_read_usize(obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(0)
        }
    };
    let recipe_owner = unsafe {
        safe_read_usize(base + CONTINUE_MANAGER_GLOBAL_RVA + FULLREAD_OWNER_GDM_08_OFFSET)
    }
    .unwrap_or(null);
    let manager_vtable =
        unsafe { safe_read_usize(base + CONTINUE_MANAGER_GLOBAL_RVA) }.unwrap_or(null);
    let game_data_man = game_data_man_ptr_or_null();
    let gdm8 = if game_data_man == null {
        null
    } else {
        unsafe { safe_read_usize(game_data_man + FULLREAD_OWNER_GDM_08_OFFSET) }.unwrap_or(null)
    };
    append_autoload_debug(format_args!(
        "own-load-OWNER-DIAG: title=0x{owner:x} (+284={}) recipe=0x{recipe_owner:x} (+284={}) mgr_vt=0x{manager_vtable:x} gdm8=0x{gdm8:x} (+284={})",
        read284(owner),
        read284(recipe_owner),
        read284(gdm8)
    ));
    // (5) FINAL STEP. Two mutually-exclusive armed levers (both OFF by default; verify-only is the
    // default). The LoadGame-JOB INSTALL lever (own_load_install_job) takes precedence: it is the
    // SAVE-SAFE, NON-SetState5 path (build + install the LoadGame MenuJob into owner+0x130 so
    // STEP_MenuJobWait ticks it -> self-build -> deser -> world stream; no SetState5, no save write).
    // Only if it is NOT armed do we fall back to the legacy GUARDED continue_confirm/SetState5 lever
    // (own_load_continue), which is SAVE-WRITING (SetState5 autosaves) behind the hard c30/fp guard.
    // PATH B (own_load_pump) takes precedence: BUILD the LoadGame job with REAL mss-derived ctx, then
    // privately pump its Run every frame from the recurring game task to completion (deser -> m28 stream)
    // and drive the transition on Success. No owner+0x130 install, no queue, no dialog -- the proven
    // menu-free "own the load". SAVE-SAFE at build (only the final SetState5 transition writes, gated).
    if own_load_pump_enabled() {
        unsafe { own_load_pump_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    } else if own_load_install_job_enabled() {
        unsafe { own_load_install_job_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    } else if own_load_continue_enabled() {
        unsafe { own_load_continue_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
    }
    OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
    OWN_LOAD_PHASE_PUB.store(PHASE_DONE + 1, Ordering::SeqCst);
}

/// OWN-LOAD FINAL STEP (er-effects-rs-mr2): after the PROVEN verify-only parse mounted a REAL c30 +
/// real character, fire the GUARDED native `continue_confirm` 0x140b0e180 -> `SetState5` 0x140b0d960
/// to stream the character into the PLAYABLE world. `continue_confirm` reads owner = [rcx+8] off
/// the shim, reads GameMan+0xc30 (already REAL from our parse) into owner+0xbc, then
/// SetState(owner, 5) -> the per-frame title-flow step machine streams the world.
///
/// OWNER (er-effects-rs-mr2 fix): the owner MUST be the SetState-able TITLE owner threaded in from
/// `own_stepper_idx10` (the validated title-flow object), NOT *(GameDataMan+0x8). The prior crash
/// passed *(GameDataMan+0x8) (a DIFFERENT object) into continue_confirm and crashed inside
/// SetState5. The OWNER DIAGNOSTIC in the verify path logs all candidates for cross-checking.
///
/// SAVE-SAFETY ABSOLUTE (SetState5 AUTOSAVES). HARD GUARD before firing -- ABORT with a logged
/// no-write if ANY fails:
///   * `c30_real` (c30 != 0xa010000 m10-default AND != 0xffffffff unset AND != 0): same flag the
///     verify path computed -- never fire SetState5 on an unverified/default c30 (the prior crash
///     cause -- real char streamed to the wrong map then autosaved over).
///   * `fp_real`: the PlayerGameData char fingerprint is real (level/stats non-default).
///   * `title_owner` non-null AND title_owner+0x284 (new-game flag) == 0 (continue_confirm's LOAD
///     branch; non-zero would take the NewGame path -- fail closed).
/// Keeps `simulated_button_presses_total = 0`: this is a pure in-process native call, no input.
unsafe fn own_load_continue_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Hard c30 + fingerprint guard (absolute save-safety backstop). NOTE: unlike the native-fullread
    // COMMIT path (which needs a level>=10 floor to reject the level-9 NEW-GAME PREVIEW), OWN-LOAD has
    // a STRONGER per-slot signal: `c30_real` means GameMan+0xc30 became the slot's REAL map
    // (0x1c000000 etc.), NOT the new-game default 0xa010000 -- so a real save is proven directly.
    // `fp_real` already requires level>=1 AND a non-empty name (see char_fingerprint), so it admits
    // legitimate LOW-LEVEL real characters (e.g. a level-7 Hero-class save) that a >=10 floor would
    // wrongly reject. c30_real + fp_real is the correct, save-safe gate here.
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-continue: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO continue_confirm, NO SetState5, NO save write -> ABORT (save-safe)"
        ));
        return;
    }
    // OWNER = the SetState-able TITLE owner threaded in from own_stepper_idx10 (NOT *(GameDataMan+0x8),
    // which caused the prior crash). It is the validated title-flow object the DLL already SetState's.
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-continue: ABORT -- threaded title_owner is null -> no write"
        ));
        return;
    }
    let new_game_flag = match unsafe {
        safe_read_usize(title_owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET)
    } {
        Some(v) => v as u8,
        None => {
            append_autoload_debug(format_args!(
                "own-load-continue: ABORT -- title_owner+0x284 (new-game flag) unreadable (title_owner=0x{title_owner:x}) -> no write"
            ));
            return;
        }
    };
    if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
        append_autoload_debug(format_args!(
            "own-load-continue: ABORT -- title_owner+0x284={new_game_flag} != 0 (continue_confirm LOAD branch requires the new-game flag clear) -> no write"
        ));
        return;
    }
    // GUARD PASSED. Build the {[OWNER_IDX]=title_owner} shim and fire the native continue_confirm.
    let shim = &raw mut OWN_STEPPER_SHIM;
    unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = title_owner };
    let shim_ptr = shim as usize;
    let confirm: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
    append_autoload_debug(format_args!(
        "own-load-continue: *** GUARD PASS -- COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} title_owner=0x{title_owner:x}) c30=0x{c30:x} level={fp_level} title_owner+0x284=0 -- continue_confirm fires SetState5 internally (AUTOSAVES) presses=0 ***",
        base + CONTINUE_CONFIRM_RVA
    ));
    timeline_event(
        "T_own_load_continue",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    unsafe { confirm(shim_ptr) };
    // Cache the pointers the RECURRING world-stream observer needs, then arm it. own_stepper_idx10 (a
    // TITLE-PHASE task) STOPS ticking once SetState5 starts this transition, so the title `owner` and
    // its InGameStep (owner+0x2e8) will no longer be threaded in. Snapshot them HERE (InGameStep was
    // already non-null at frame 0) so the recurring game task can keep walking owner->InGameStep->
    // MoveMapStep through the whole loading screen. (own-load-stream-observer-must-be-recurring-task-2026-06-22)
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load-continue: continue_confirm returned -- native pump now streams the real world (#{n}); recurring world-stream observer ARMED (owner=0x{title_owner:x} ingame=0x{ingame_cached:x}) -> DONE"
    ));
}

/// Snapshot of the `owner+0x130` MenuJob slot for the before/after vtable-flip + self-build evidence.
/// All pure fault-tolerant reads -- never changes load behavior.
fn own_load_install_job_slot_snapshot(slot_addr: usize) -> (usize, usize, usize, u8, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // The job pointer currently in the slot.
    let job = unsafe { safe_read_usize(slot_addr) }.unwrap_or(null);
    if job == null {
        return (null, null, null, 0, null);
    }
    let vtable = unsafe { safe_read_usize(job) }.unwrap_or(null);
    let inner_seq = unsafe { safe_read_usize(job + MENUJOB_INNER_SEQ_70_OFFSET) }.unwrap_or(null);
    let built_flag = unsafe { safe_read_usize(job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    let current_job_index =
        unsafe { safe_read_usize(job + MENUJOB_CURRENT_JOB_INDEX_10_OFFSET) }.unwrap_or(null);
    (job, vtable, inner_seq, built_flag, current_job_index)
}

/// OWN-LOAD FINAL STEP -- LoadGame-JOB INSTALL lever (`own_load_install_job`). The SAVE-SAFE,
/// NON-SetState5 alternative to `own_load_continue_fire`: after the PROVEN verify-only parse mounted a
/// REAL c30 + real character, BUILD the native LoadGame `CS::MenuJobWithContext<LoadJobContext>` and
/// INSTALL it into the title owner's `+0x130` MenuJob slot, replacing the idle `IfElseJob`.
/// `CS::TitleStep::STEP_MenuJobWait` already ticks `ExecuteMenuJob(&owner->+0x130)` every frame, so the
/// installed job then self-builds (its `Run` builds the inner FixOrderJobSequence on the first tick:
/// `+0x68`/`+0x70` flip), deserializes the save, and streams the world -- WITHOUT `SetState5`.
///
/// SAVE-SAFETY ABSOLUTE: NO `SetState5`, NO autosave, NO save write. The BUILD factory only allocates +
/// copies a template; the first-tick deser step (`FUN_14082c330`) only READS the save
/// (`AllocateAligned` -> read -> `SetSaveSlot` -> decrypt -> `ReadBytes` -> dealloc) up to world-stream.
/// Static-verified against the runtime dump. Same hard c30/fp guard as the continue lever is kept as a
/// belt-and-braces precondition even though no write occurs. Keeps `simulated_button_presses_total = 0`.
///
/// ARG SOURCING (static RE, 2026-06-22): the BUILD factory `FUN_140826510(out, ctx_parent, slot,
/// owner_ctx)` needs only `out` (our local) + `slot` (the int slot) for the deser/map self-build; the
/// `ctx_parent`/`owner_ctx` args are the OUTER profile-selection UI context, stored as lambda captures
/// whose every build-path deref is null-guarded -- so we pass them as 0. RESIDUAL RISK: if the engine's
/// `EnableProfileSelection` release flag is set AND the outer sequence ticks the profile-selection
/// sub-job, a captured-null deref could fault -- watch the install-fire log for that. The two native
/// calls are wrapped in `catch_unwind` (catches a Rust-unwinding panic; a hardware AV is NOT caught).
unsafe fn own_load_install_job_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    const NO_CTX: usize = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Belt-and-braces guard (no write occurs, but never act on an unverified parse).
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-install-job: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO build/install -> ABORT (save-safe)"
        ));
        return;
    }
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-install-job: ABORT -- threaded title_owner is null -> no install (save-safe)"
        ));
        return;
    }
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst) as i32;
    let slot_addr = title_owner + TITLE_OWNER_MENUJOB_SLOT_130_OFFSET;
    // BEFORE: dump owner+0x130 (the idle IfElseJob it replaces). Pure reads.
    let (b_job, b_vt, b_seq, b_built, b_idx) = own_load_install_job_slot_snapshot(slot_addr);
    append_autoload_debug(format_args!(
        "own-load-install-job: BEFORE slot=owner+0x130=0x{slot_addr:x} job=0x{b_job:x} vt=0x{b_vt:x} (expect IfElseJob dump 0x{:x}) +0x68_built={b_built} +0x70_seq=0x{b_seq:x} +0x10_idx=0x{b_idx:x} -- BUILD 0x{:x}(out,ctx=0,slot={want_slot},owner_ctx=0) presses=0",
        MENUJOB_IFELSE_VTABLE_DUMP_VA,
        base + LOADGAME_JOB_BUILD_RVA,
    ));
    // (a) BUILD the LoadGame MenuJobWithContext into a local DLRefCountPtr (the factory writes the job
    //     ptr into *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_BUILD_RVA) };
    let mut built_job: usize = 0;
    let build_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        build(&raw mut built_job, NO_CTX, want_slot, NO_CTX)
    }));
    match build_ret {
        Ok(_) => {}
        Err(_) => {
            append_autoload_debug(format_args!(
                "own-load-install-job: BUILD PANICKED (caught) -- NO install -> ABORT (save-safe)"
            ));
            return;
        }
    }
    if built_job == null || built_job == 0 {
        append_autoload_debug(format_args!(
            "own-load-install-job: BUILD returned a NULL job (built_job=0x{built_job:x}) -- NO install -> ABORT (save-safe)"
        ));
        return;
    }
    let built_vt = unsafe { safe_read_usize(built_job) }.unwrap_or(null);
    append_autoload_debug(format_args!(
        "own-load-install-job: BUILD OK job=0x{built_job:x} vt=0x{built_vt:x} (expect LoadGame dump 0x{:x}) -- INSTALL via assign 0x{:x}(slot=0x{slot_addr:x}, src=&job)",
        MENUJOB_LOADGAME_VTABLE_DUMP_VA,
        base + MENUJOB_ASSIGN_RVA,
    ));
    // (b) APPEND our built job into the owner+0x130 MenuJobQueue via PushBackJob (NOT a slot-overwrite).
    //     owner+0x130 is a CS::MenuJobQueue (active job +0x130, ring +0x138, count +0x178). The prior
    //     move-assign overwrite ORPHANED the title IfElseJob's sibling CS::MenuWindowJobs -> AV at
    //     CS::DLFixedVector::push_back 0x140733fea. PushBackJob(queue_base=&owner+0x130, src=&built_job)
    //     appends behind the still-active IfElseJob (no tear, AtomicIncrements the job, does not zero
    //     src); STEP_MenuJobWait's ExecuteMenuJob then pops + ticks our queued job.
    // Justify the transmute: MENUJOB_PUSHBACK_RVA is the prologue-grounded live entry of
    // CS::MenuJobQueue::PushBackJob (FUN_1407a9254).
    let queue_count_before =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    let pushback: unsafe extern "system" fn(*mut usize, *mut usize) -> *mut usize =
        unsafe { std::mem::transmute(base + MENUJOB_PUSHBACK_RVA) };
    let install_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        pushback(slot_addr as *mut usize, &raw mut built_job)
    }));
    if install_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-install-job: PUSHBACK PANICKED (caught) after build (job=0x{built_job:x}) -> ABORT"
        ));
        return;
    }
    // AFTER: the active job at owner+0x130 should be UNCHANGED (still the IfElseJob) -- our job is in the
    // ring; the queue count at +0x178 should have grown by 1. Pure reads.
    let (a_job, a_vt, a_seq, a_built, a_idx) = own_load_install_job_slot_snapshot(slot_addr);
    let queue_count_after =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    OWN_LOAD_INSTALL_JOB_FIRED.fetch_add(1, Ordering::SeqCst);
    // Cache the owner so the recurring world-stream observer keeps logging through the loading screen
    // (own_stepper_idx10 stops once the title transitions). Mirror own_load_continue_fire's caching.
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    timeline_event(
        "T_own_load_install_job",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    append_autoload_debug(format_args!(
        "own-load-install-job: *** APPENDED -- AFTER queue=owner+0x130=0x{slot_addr:x} active_job=0x{a_job:x} vt=0x{a_vt:x} (active stays IfElseJob dump 0x{:x}, NOT torn) active+0x68={a_built} +0x70=0x{a_seq:x} +0x10_idx=0x{a_idx:x} | queue_count {queue_count_before}->{queue_count_after} (expect +1) | our_job=0x{built_job:x} (LoadGame dump 0x{:x}) ingame=0x{ingame_cached:x} -- STEP_MenuJobWait pops+ticks queued job -> self-build -> deser -> world stream (NO SetState5/NO save write) presses=0 (#{n}) -> DONE ***",
        MENUJOB_IFELSE_VTABLE_DUMP_VA, MENUJOB_LOADGAME_VTABLE_DUMP_VA,
    ));
    let _ = (b_seq, b_idx, b_built, b_vt, b_job);
}

/// Resolve `mss = GameDataMan->menuSystemSaveLoad = *(*(base + GAME_DATA_MAN_GLOBAL_RVA) +
/// GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET)` (static-verified: `GetMenuSystemSaveLoad` 0x140256410 is
/// exactly `GLOBAL_GameDataMan->menuSystemSaveLoad`). Returns `None` (never `null`/`0`) on any
/// fault-tolerant read failure. Pure reads.
unsafe fn resolve_menu_system_save_load(base: usize) -> Option<usize> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gdm = unsafe { safe_read_usize(base + GAME_DATA_MAN_GLOBAL_RVA) }
        .filter(|&v| v != null && v != 0)?;
    unsafe { safe_read_usize(gdm + GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET) }
        .filter(|&v| v != null && v != 0)
}

/// The "engine filled enough to drive our own load" gate -- distinct from "GameMan instance pointer
/// resolved" (`game_man_instance_resolved`), which flips true at BootPhase4, LONG before the load
/// machinery is usable. True iff GameDataMan + menuSystemSaveLoad (mss) resolve AND the TitleFlowContext
/// at `mss+0xa38` is a PLAUSIBLE heap pointer. The plausibility range matters: before the GameFlow
/// constructs the TitleFlowContext it reads back as uninitialized garbage (e.g. 0x8080808080808080),
/// which a `!= 0` check would wrongly accept -- then the LoadGame job's first `Run` derefs it and
/// access-violates (the ~25s AV observed when arming at the bare title). When this returns true, the
/// native LoadGame job (`own_load_pump_fire`) can be built + pumped without that crash. The bypass arms
/// its own-load on THIS, not on `game_man_instance_resolved`.
/// (loadgame-build-ctx-ready-precondition-2026-06-22)
pub(crate) unsafe fn loadgame_build_ctx_ready(base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // CORRECTED (bd loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22): the buildable
    // TitleFlowContext is `*(CS::TitleTopDialog+0xa38)`, NOT `*(mss+0xa38)` (the mss reading was a red
    // herring -- r13 at the golden factory site is the dialog). Read it off the live dialog
    // (owner+0xe0, vtable-gated) via the cached title owner, so this arming signal matches exactly the
    // ctx `own_load_pump_fire` builds with.
    let owner = TITLE_OWNER_PTR.load(Ordering::SeqCst);
    if owner == null || owner == 0 {
        return false;
    }
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    if dialog == 0 {
        return false;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(0);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return false;
    }
    let ctx = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR
}

/// PATH B "OWN THE LOAD" -- BUILD the LoadGame job with REAL mss-derived ctx, store its pointer for the
/// recurring per-frame private pump. The menu-free alternative to BOTH the owner+0x130 install (a
/// proven dead end) and the SetState5-only continue (reached the loading screen but never mounted m28).
///
/// We BUILD via `FUN_140826510(out, ctx_parent=mss+0x50, save_slot, owner_ctx=*(mss+0xa38))` -- the REAL
/// non-null ctx from the golden Continue trace (the prior ctx=0 build AV'd when the outer
/// profile-selection sub-job dereffed the captured null). We do NOT install the job anywhere (no
/// owner+0x130, no MenuJobQueue, no CSMenuMan dialog). Instead the recurring game task ticks its `Run`
/// privately every frame (see `own_load_pump_tick`) until it self-builds + deserializes + map-streams
/// (m28 mount) and reaches `state==Success`, then drives the title->ingame transition once.
///
/// SAVE-SAFETY ABSOLUTE: BUILD only allocates + copies a template (no save write); the first-tick deser
/// step (`FUN_14082c330`) only READS the save up to world-stream. NO SetState5 here. The same hard
/// c30/fp guard as the other levers is kept as a belt-and-braces precondition even though no write
/// occurs at build time. The transition (the only save-writing step) is separately gated in
/// `own_load_pump_tick`. Keeps `simulated_button_presses_total = 0`.
unsafe fn own_load_pump_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Belt-and-braces guard (no write occurs at build, but never act on an unverified parse).
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-pump: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO build -> ABORT (save-safe)"
        ));
        return;
    }
    if title_owner == null {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- threaded title_owner is null -> no build (save-safe)"
        ));
        return;
    }
    if OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst) != 0 {
        // Already built+armed (own_load_drive is one-shot, but guard against a re-entrant fire).
        return;
    }
    // CORRECTED ctx source (bd loadgame-owner-ctx-is-DIALOG-a38-not-mss-CORRECTION-2026-06-22): the
    // LoadGame factory's owner_ctx (r9) and ctx_parent (rdx) come from the live CS::TitleTopDialog,
    // NOT from CSMenuSystemSaveLoad. The golden factory site reads `mov 0xa38(%r13),%r9` where r13 IS
    // the dialog (the prior mss+0xa38 reading misidentified r13 as mss and read back garbage -> the AV).
    // Locate the live dialog at owner+0xe0 (vtable-gated, same recipe as locate_live_loadgame_node).
    let dialog = unsafe { safe_read_usize(title_owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }
        .filter(|&v| v != null && v != 0)
        .unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- live TitleTopDialog not up (owner+0x{:x}=0x{dialog:x} vt=0x{dialog_vt:x} want 0x{:x}) -> no build (save-safe)",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return;
    }
    let ctx_parent = dialog + DIALOG_CTX_PARENT_50_OFFSET;
    // owner_ctx = *(dialog+0xa38) = CS::TitleFlowContext (written UNCONDITIONALLY by the dialog ctor
    // 0x1409a82d0, so it is valid at the settled press-any-button title -- unlike mss+0xa38 which read
    // back uninitialized garbage). FAIL CLOSED (no build) if it is not a plausible heap pointer:
    // passing NULL is exactly what AV'd before, and a real ctx is the whole point of the correction.
    let raw_owner_ctx =
        unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(raw_owner_ctx > OWNER_CTX_MIN_PLAUSIBLE_PTR && raw_owner_ctx < OWNER_CTX_MAX_PLAUSIBLE_PTR)
    {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- owner_ctx *(dialog+0x{:x})=0x{raw_owner_ctx:x} is not a plausible TitleFlowContext (dialog=0x{dialog:x}) -> no build (save-safe)",
            DIALOG_OWNER_CTX_A38_OFFSET
        ));
        return;
    }
    let owner_ctx = raw_owner_ctx;
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "own-load-pump: BUILD 0x{:x}(out, ctx_parent=dialog+0x{:x}=0x{ctx_parent:x}, slot={want_slot}, owner_ctx=*(dialog+0x{:x})=0x{owner_ctx:x}) dialog=0x{dialog:x} -- CORRECTED dialog-derived ctx (golden Continue args) presses=0",
        base + LOADGAME_JOB_BUILD_RVA,
        DIALOG_CTX_PARENT_50_OFFSET,
        DIALOG_OWNER_CTX_A38_OFFSET,
    ));
    // BUILD the LoadGame MenuJobWithContext into a local DLRefCountPtr (factory writes the job ptr into
    // *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot:i32, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_BUILD_RVA) };
    let mut built_job: usize = 0;
    let build_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        build(&raw mut built_job, ctx_parent, want_slot, owner_ctx)
    }));
    if build_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-pump: BUILD PANICKED (caught) -- NO pump -> ABORT (save-safe)"
        ));
        return;
    }
    if built_job == null || built_job == 0 {
        append_autoload_debug(format_args!(
            "own-load-pump: BUILD returned a NULL job (built_job=0x{built_job:x}) -- NO pump -> ABORT (save-safe)"
        ));
        return;
    }
    let built_vt = unsafe { safe_read_usize(built_job) }.unwrap_or(null);
    let built_flag = unsafe { safe_read_usize(built_job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    // Arm the recurring private pump: publish the job ptr + cache owner/InGameStep (mirror the other
    // levers) so the recurring observer keeps logging through the loading screen, and set
    // OWN_LOAD_CONTINUE_FIRED so own_load_stream_observe_recurring runs each frame. Do NOT install the
    // job anywhere -- the recurring task pumps Run directly.
    OWN_LOAD_PUMP_JOB.store(built_job, Ordering::SeqCst);
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    timeline_event(
        "T_own_load_pump_build",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    append_autoload_debug(format_args!(
        "own-load-pump: *** BUILT job=0x{built_job:x} vt=0x{built_vt:x} (expect LoadGame dump 0x{:x}) +0x68_built={built_flag} -- ARMED private per-frame pump (NO owner+0x130 install, NO queue, NO dialog) ingame=0x{ingame_cached:x} -- recurring task will tick Run each frame -> self-build -> deser -> m28 stream -> SetState5 transition on Success presses=0 (#{n}) ***",
        MENUJOB_LOADGAME_VTABLE_DUMP_VA,
    ));
}

/// PATH B per-frame PRIVATE PUMP (runs from the recurring game task each frame, gated). If a LoadGame
/// job was built+armed by `own_load_pump_fire`, tick its `Run` exactly the way the native
/// `ExecuteMenuJob` does -- a zero-init `MenuJobResult` and an `FD4Time` carrying the frame delta -- so
/// the job self-builds, deserializes, and map-streams the world WITHOUT the menu system. When the job
/// reaches `state==Success` (deser+map done, m28 mounted), drive the title->ingame transition ONCE via
/// the guarded `continue_confirm`/SetState5 (the same save-safe guard as `own_load_continue_fire`), then
/// latch `OWN_LOAD_PUMP_DONE` so we never re-pump or re-transition.
///
/// SAVE-SAFETY: the pump itself (build+deser+map-stream) is READ-only up to world-stream. The ONLY
/// save-writing step is the final SetState5 transition, which stays HARD-gated on the verified parse
/// (`c30_real && fp_real`, re-checked from the live GameMan+0xc30 and char fingerprint) + the title
/// owner's new-game flag clear -- mirroring `own_load_continue_fire`. No save write before the world is
/// confirmed loading. Every native call is wrapped in `catch_unwind` (a Rust panic is caught; a hardware
/// AV is not). Keeps `simulated_button_presses_total = 0`.
pub(crate) unsafe fn own_load_pump_tick(base: usize, gm: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if OWN_LOAD_PUMP_DONE.load(Ordering::SeqCst) {
        return;
    }
    // Build the call buffers exactly as native ExecuteMenuJob/STEP_MenuJobWait do: a zero-init
    // MenuJobResult (8 bytes) and an FD4Time (16 bytes) whose +0x8 f32 holds the frame delta (Run only
    // reads time+8; it writes the FD4Time vtable into time+0 itself). We over-size both buffers to a
    // qword to keep them aligned and writable.
    let mut result: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE]; // >= MENUJOB_RESULT_SIZE; zero state.
    let mut time: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE];
    // Write the f32 frame delta at time+0x8 (Run advances the map-stream sub-job on this).
    time[FD4_TIME_DELTA_8_OFFSET..FD4_TIME_DELTA_8_OFFSET + core::mem::size_of::<f32>()]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let result_ptr = result.as_mut_ptr() as usize;
    let time_ptr = time.as_mut_ptr() as usize;
    // Run(this /*rcx*/, result /*rdx*/, time /*r8*/, param4 /*r9*/) -> *MenuJobResult.
    // Justify the transmute: LOADGAME_JOB_RUN_RVA is the prologue-grounded live entry of the LoadGame
    // MenuJobWithContext::Run (vtable+0x10), signature per the static decompile of FUN_140826e40.
    let run: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + LOADGAME_JOB_RUN_RVA) };
    let run_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        run(job, result_ptr, time_ptr, 0)
    }));
    let fired = OWN_LOAD_PUMP_FIRED.fetch_add(1, Ordering::SeqCst) + 1;
    if run_ret.is_err() {
        // A Rust-level panic in Run -> stop pumping (latch done) so we do not re-fault every frame.
        OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own-load-pump: Run PANICKED (caught) at pump #{fired} (job=0x{job:x}) -> latch DONE, no transition (save-safe)"
        ));
        return;
    }
    // Read back the result state (+0x0) and the inner deser sub-code (+0x4).
    let state = i32::from_le_bytes([
        result[MENUJOB_RESULT_STATE_0_OFFSET],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 1],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 2],
        result[MENUJOB_RESULT_STATE_0_OFFSET + 3],
    ]);
    let subcode = i32::from_le_bytes([
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 1],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 2],
        result[MENUJOB_RESULT_SUBCODE_4_OFFSET + 3],
    ]);
    OWN_LOAD_PUMP_STATE.store(i64::from(state), Ordering::SeqCst);
    OWN_LOAD_PUMP_SUBCODE.store(i64::from(subcode), Ordering::SeqCst);
    // Job header diagnostics: +0x68 built flag flips 0->1 on self-build, +0x70 inner-seq ptr 0->built.
    let built_flag = unsafe { safe_read_usize(job + MENUJOB_BUILT_FLAG_68_OFFSET) }
        .map(|v| v as u8)
        .unwrap_or(0);
    let inner_seq = unsafe { safe_read_usize(job + MENUJOB_INNER_SEQ_70_OFFSET) }.unwrap_or(null);
    // Throttled log (every OWN_LOAD_STREAM_LOG_INTERVAL pumps), plus the first pump.
    if fired == 1 || fired % OWN_LOAD_STREAM_LOG_INTERVAL == 0 {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} Run(job=0x{job:x}) state={state} (1=Continue 2=Success 3=Failed) subcode={subcode} (deser 5/2/6) +0x68_built={built_flag} +0x70_seq=0x{inner_seq:x} delta={frame_delta}"
        ));
    }
    if state <= MENUJOB_STATE_CONTINUE {
        // Still working (Continue) -- keep pumping next frame.
        return;
    }
    // Terminal: Success (2) or Failed (3). Latch DONE so we stop pumping regardless of the transition.
    OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
    if state == MENUJOB_STATE_FAILED {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} reached state=Failed(3) subcode={subcode} -- deser/map FAILED -> NO transition, latch DONE (save-safe)"
        ));
        return;
    }
    // state == Success: the job deserialized + map-streamed (m28). Drive the title->ingame transition
    // ONCE via the guarded SetState5. RE-VERIFY the parse from LIVE state (the build+pump can change
    // GameMan+0xc30) so the save-write transition is gated exactly like own_load_continue_fire.
    let owner = OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst);
    let c30_live = if gm != null && gm != 0 {
        unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }.unwrap_or(0)
    } else {
        0
    };
    let c30_real =
        c30_live != GAME_MAN_C30_UNSET && c30_live != 0 && c30_live != FULLREAD_C30_M10_DEFAULT;
    let (fp_real, fp_level, _fp_name_len) = unsafe { char_fingerprint(base) };
    append_autoload_debug(format_args!(
        "own-load-pump: *** pump #{fired} reached state=Success(2) subcode={subcode} -- deser+map-stream DONE (m28 mounted); driving title->ingame transition ONCE (owner=0x{owner:x} c30_live=0x{c30_live:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) ***"
    ));
    // SAVE-SAFE PROBE: if the verify-only gate is set, the pump has proven the corrected dialog-ctx
    // build reached Success (no AV) with the world map-streamed -- STOP HERE without the save-writing
    // SetState5 transition, so this can run against the real save with zero write risk.
    if own_load_pump_verify_only() {
        append_autoload_debug(format_args!(
            "own-load-pump: VERIFY-ONLY gate set -- reached Success(2) subcode={subcode} (corrected dialog-ctx build+pump OK, no AV); SKIPPING SetState5 transition -> NO save write, latch DONE (save-safe)"
        ));
        return;
    }
    // The transition is the SAME guarded continue_confirm/SetState5 path the legacy lever uses; it
    // re-checks c30_real && fp_real + the owner new-game flag internally and ABORTs (no write) on any
    // failure. Pass the live-re-verified c30 so the guard reflects the post-pump state.
    unsafe {
        own_load_continue_fire(base, owner, c30_live, c30_real, fp_real, fp_level, fired);
    }
}

/// AUTONOMOUS press-any-button -> open-menu (zero-input): drive the title to the open main menu
/// OURSELVES so a run needs no real button press. When the live TitleTopDialog (owner+0xe0) is settled
/// in the FD4 `Loop` state with the menu-opened latch (dialog+0xa40) still 0, call the native open-menu
/// registrar `0x1409b24e0(rcx=dialog)` -- the exact action a button press triggers -- to open the menu
/// (sets a40=1). Requires online-disable (`er-effects-offline.txt`) so the connection modal is skipped
/// and the SM reaches Loop. One-shot. Then `maybe_fire_tfc_continue` (gated a40==1) fires Continue. No
/// input. (Same self-fire the own_stepper STAGE1d uses, extracted for the tfc flow.)
pub(crate) unsafe fn maybe_auto_open_menu(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if TFC_AUTO_MENU_OPENED.load(Ordering::SeqCst) != 0 {
        return;
    }
    let Some(owner_ptr) = (unsafe { title_owner(base) }) else {
        return;
    };
    let owner = owner_ptr as usize;
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    let a40 = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(1);
    if a40 != OWN_STEPPER_MENU_OPENED_NO {
        // Menu already open (a real press or a prior call) -> nothing to do.
        TFC_AUTO_MENU_OPENED.store(1, Ordering::SeqCst);
        return;
    }
    // Require the dialog SETTLED in Loop: the registrar internally set_state(TextFadeOut) re-checks
    // node flags&0x8f>=2 and bails if not settled (FadeIn would no-op / corrupt). Read-only probe.
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    if !in_loop {
        return;
    }
    // ROUTE THE REGISTRAR IN-PLACE (zero-input): the native open-menu call sites write a "mode" byte at
    // [*(base+TITLE_MENU_TRANSITION_SINGLETON_RVA)]+0 BEFORE jumping to the registrar -- press-accept
    // 0x1409b1260 sets it =1 (open main menu IN PLACE), pump/back paths set it =0. A bare open_menu with
    // the byte left STALE may route the registrar into an error-modal branch. Replicate the press-accept
    // set (subagent-C static RE: product native-open with this byte set reached the menu with 0 msgbox).
    // Null-/readability-guarded; no save write, no input. bd er-effects-rs-0ye + title-accept-to-registrar-narrow-path-143d5dea8.
    let transition_singleton =
        unsafe { safe_read_usize(base + TITLE_MENU_TRANSITION_SINGLETON_RVA) }.unwrap_or(null);
    if transition_singleton != null && unsafe { safe_read_usize(transition_singleton) }.is_some() {
        unsafe { *(transition_singleton as *mut u8) = TITLE_MENU_TRANSITION_FLAG_SET_VALUE };
        append_autoload_debug(format_args!(
            "tfc-auto-open: set menu-transition mode byte [*(0x{:x})]+0=1 before open-menu (route registrar in-place)",
            base + TITLE_MENU_TRANSITION_SINGLETON_RVA
        ));
    }
    let open_menu: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_OPEN_MENU_RVA) };
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        open_menu(dialog)
    }));
    TFC_AUTO_MENU_OPENED.store(1, Ordering::SeqCst);
    let _ = null;
    append_autoload_debug(format_args!(
        "tfc-auto-open: fired open-menu registrar 0x{:x}(dialog=0x{dialog:x}) on Loop+a40==0 (panicked={}) -- autonomous press-any-button equivalent, NO input",
        base + TITLE_TOP_DIALOG_OPEN_MENU_RVA,
        r.is_err()
    ));
}

/// One-shot log latch for `force_offline_connection_bytes` (only logs the first 1->0 clear).
pub(crate) static FORCE_OFFLINE_BYTES_CLEARED: AtomicUsize = AtomicUsize::new(0);

/// Connection-state OFFLINE lever (zero-input, save-safe) -- the milestone-3 fix. The title's
/// network/session event handlers (`CSLuaEventScriptImitation::On{LanCutError,DisconnectGameServer,
/// FailedGetBlockNum,NpServerSignOut,DisconnectEOSServer,...}`) build the "Cannot connect to network /
/// connection lost / network error" `GR_System_Message` MessageBoxDialogs that our offline pab boot
/// raises at menu-open. Each handler is guarded by `if (IsInOnlineMode()) { if
/// (IsServerConnectionEnabled() && ...) { build popup } }`, which reduces to two `GameMan` bytes:
/// `isInOnlineMode = [GameMan+0xBC8]`, `serverConnectionEnabled = [GameMan+0xBC9]`
/// (`GameMan = *(base+GAME_SAVE_SLOT_SINGLETON_RVA)`; getter `0x14067a030` is `mov rax,[0x143d69918];
/// movzx eax,[rax+0xBC8]; ret` -- VERIFIED by deobf disasm). NOTE the existing online-disable patches
/// that getter's RETURN value, but the handlers consult the BYTES (directly / via getters our patch
/// does not cover), so the patch alone does not gate them. Forcing both bytes to 0 each title frame
/// short-circuits the whole connection-loss family at the source (the guard fails -> no popup is ever
/// enqueued -- not suppressed, not dismissed). Pure offline state, no save write, no input. Readable-
/// guarded so a not-yet-initialized GameMan can never fault the game thread. bd er-effects-rs-0ye
/// (subagent-D GR_System_Message gate, subagent-B premise: modals are network notices not SaveRetry).
pub(crate) unsafe fn force_offline_connection_bytes(base: usize) {
    const IS_IN_ONLINE_MODE_BC8_OFFSET: usize = 0xBC8;
    const SERVER_CONNECTION_ENABLED_BC9_OFFSET: usize = 0xBC9;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let game_man = unsafe { safe_read_usize(base + GAME_SAVE_SLOT_SINGLETON_RVA) }.unwrap_or(null);
    if game_man == null {
        return;
    }
    let (Some(online), Some(server)) = (
        unsafe { safe_read_u8(game_man + IS_IN_ONLINE_MODE_BC8_OFFSET) },
        unsafe { safe_read_u8(game_man + SERVER_CONNECTION_ENABLED_BC9_OFFSET) },
    ) else {
        return;
    };
    if online == 0 && server == 0 {
        return;
    }
    unsafe {
        *((game_man + IS_IN_ONLINE_MODE_BC8_OFFSET) as *mut u8) = 0;
        *((game_man + SERVER_CONNECTION_ENABLED_BC9_OFFSET) as *mut u8) = 0;
    }
    if FORCE_OFFLINE_BYTES_CLEARED.fetch_add(1, Ordering::SeqCst) == 0 {
        append_autoload_debug(format_args!(
            "force-offline: cleared GameMan+0xBC8 (isInOnlineMode {online}->0) +0xBC9 (serverConnectionEnabled {server}->0) gm=0x{game_man:x} -- gate connection-loss GR_System_Message popups at source"
        ));
    }
}

/// See `fire_tfc_continue_enabled`. Runs from the recurring game task; self-gates and fires ONCE.
/// Pure in-process field writes (NO input, NO native call) -- the native menu pump's selector
/// (`0x1409a8eb0`) picks up `tfc+0x14c==1` on its next tick and dispatches the load through the
/// engine's own job pump (the proven user-Continue path, which avoids the FixOrderJobSequence
/// overflow that killed the factory-direct `own_load_pump`). Logs before/after so a probe sees the
/// exact write.
pub(crate) unsafe fn maybe_fire_tfc_continue(base: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !fire_tfc_continue_enabled() {
        return;
    }
    if TFC_CONTINUE_FIRED.load(Ordering::SeqCst) != 0 {
        return;
    }
    // Resolve+cache the SimpleTitleStep owner (throttled full scan); bail until it exists.
    let Some(owner_ptr) = (unsafe { title_owner(base) }) else {
        return;
    };
    let owner = owner_ptr as usize;
    // Require the SETTLED main-menu state (STEP_MenuJobWait), i.e. press-any-button -> BeginLogo done.
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    if committed != TITLE_STEP_MENU_JOB_WAIT {
        return;
    }
    // Require "the rest of GameMan is set up": the GetSaveSlot singleton (*(base+0x3d69918)) non-null.
    let gm_singleton = unsafe { safe_read_usize(base + GAME_SAVE_SLOT_SINGLETON_RVA) }.unwrap_or(0);
    if gm_singleton == null || gm_singleton == 0 {
        return;
    }
    // Live TitleTopDialog (owner+0xe0, vtable-gated) -> CS::TitleFlowContext at +0xa38.
    let dialog = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(0);
    let dialog_vt = if dialog != 0 {
        unsafe { safe_read_usize(dialog) }.unwrap_or(0)
    } else {
        0
    };
    if dialog == 0 || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    // Require the MAIN MENU to be OPEN, not the bare press-any-button screen. State 10
    // (MenuJobWait) occurs at BOTH; the open-menu registrar 0x1409b24e0 sets the menu-opened latch
    // [dialog+0xa40]=1. Firing the bit at the closed press-any-button screen is dormant (the selector
    // that consumes tfc+0x14c is a Continue-item funclet not pumped until the menu is open) -- bd
    // tfc-14c-bit-dormant-without-menu-open-or-selector-invoke-2026-06-22.
    let menu_opened = unsafe {
        safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET)
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(0)
    };
    if menu_opened != OWN_STEPPER_CALL_INC {
        return;
    }
    let tfc = unsafe { safe_read_usize(dialog + DIALOG_OWNER_CTX_A38_OFFSET) }.unwrap_or(0);
    if !(tfc > OWNER_CTX_MIN_PLAUSIBLE_PTR && tfc < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        return;
    }
    // READINESS GATE on dialog+0x50 (the load MenuWindowJob's push target -- selector sets r8=dialog+
    // 0x50). A run showed its count field (dialog+0x50+0x48 = dialog+0x98) can hold GARBAGE (a pointer
    // ~0x7fff..., not a small count) -> dialog+0x50 is not yet a valid/ready vector in our self-opened
    // flow, and firing then crashes (insert reads garbage count -> 'out of memory'). Fire ONLY when the
    // count is a plausible small value WITH ROOM (< 8); else WAIT (do NOT consume the one-shot) and
    // retry next frame. If it never becomes valid we simply never fire (no crash). bd
    // dialog-plus0x50-NOT-a-vector-built-job-miscontextualized-2026-06-23.
    let load_vec_count = unsafe {
        safe_read_usize(dialog + DIALOG_MENUWINDOW_VEC_50_OFFSET + DLFIXEDVECTOR_COUNT_48_OFFSET)
    }
    .unwrap_or(usize::MAX);
    if load_vec_count >= 8 {
        let waits = TFC_LOAD_VEC_WAIT_TICKS.fetch_add(1, Ordering::SeqCst);
        if waits % 120 == 0 {
            append_autoload_debug(format_args!(
                "fire-tfc-continue: WAIT -- dialog+0x50 load vector not ready (count@dialog+0x98=0x{load_vec_count:x} >= 8, likely uninitialized/garbage) dialog=0x{dialog:x} waits={waits}; not firing"
            ));
        }
        return;
    }
    append_autoload_debug(format_args!(
        "fire-tfc-continue: dialog+0x50 load vector READY (count={load_vec_count} < 8) -- proceeding to fire (dialog=0x{dialog:x})"
    ));
    let before = unsafe { safe_read_i32(tfc + TFC_DISPATCH_STATE_14C_OFFSET) }.unwrap_or(-1);
    // Set the save slot on mss FIRST (builder reads mss+0x1200 as the factory r8), then the dispatch
    // bit -- mirroring the native confirm handler 0x1409a9250's two key writes.
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let mss = unsafe { resolve_menu_system_save_load(base) };
    if let Some(mss) = mss {
        unsafe { *((mss + MSS_SAVE_SLOT_1200_OFFSET) as *mut i32) = want_slot };
    }
    unsafe { *((tfc + TFC_DISPATCH_STATE_14C_OFFSET) as *mut i32) = TFC_DISPATCH_STATE_LOAD };
    // Force the dispatcher's BUILD branch: clear tfc+0x18c (IsNotReleaseFlag55 0x14082cd60 `cmpb
    // $0,0x18c(rcx)`). The open-menu path sets this nonzero AFTER press-any-button, which makes the
    // load dispatcher 0x1409b3070 take its ABORT branch (empty job, no load -- the builder 0x9ac760
    // never fired). Clearing it guarantees the real LoadGame build. bd dispatcher-abort-branch-force-
    // tfc-18c-zero-2026-06-23.
    let nrf_before = unsafe { safe_read_usize(tfc + TFC_NOT_RELEASE_FLAG_18C_OFFSET) }
        .map(|v| (v & 0xff) as u8)
        .unwrap_or(0xff);
    unsafe { *((tfc + TFC_NOT_RELEASE_FLAG_18C_OFFSET) as *mut u8) = TFC_NOT_RELEASE_FLAG_CLEAR };
    TFC_CONTINUE_FIRED.store(1, Ordering::SeqCst);
    // Let the recurring world-stream observer log THROUGH the loading screen.
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "fire-tfc-continue: SET *(tfc+0x{:x})=1 (was {before}) + mss+0x{:x}=slot {want_slot} (tfc=0x{tfc:x} dialog=0x{dialog:x} owner=0x{owner:x} mss={mss:?} gm_singleton=0x{gm_singleton:x}) -- now INVOKING selector 0x{:x} (NO input)",
        TFC_DISPATCH_STATE_14C_OFFSET,
        MSS_SAVE_SLOT_1200_OFFSET,
        base + TITLE_CONTINUE_SELECTOR_RVA
    ));
    // INVOKE the Continue-item selector that consumes tfc+0x14c (it is NOT pumped from the idle menu).
    // Selector 0x1409a8eb0(rcx = &dialog_slot = owner+0xe0, rdx = out MenuJobResult*): reads
    // *(rcx)->dialog, *(dialog+0xa38)->tfc, *(tfc+0x14c)==1 -> LOAD branch -> sets r8=dialog+0x50 +
    // calls the load dispatcher 0x1409b3070 (proper ChainMenuJobs enqueue). Wrapped in catch_unwind
    // (a Rust panic is caught; a hardware AV is not). Keeps simulated_button_presses_total = 0.
    let dialog_slot = owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    let mut out_job: [usize; 4] = [0; 4];
    let out_ptr = out_job.as_mut_ptr() as usize;
    let selector: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + TITLE_CONTINUE_SELECTOR_RVA) };
    let sel_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        selector(dialog_slot, out_ptr)
    }));
    if sel_ret.is_err() {
        append_autoload_debug(format_args!(
            "fire-tfc-continue: selector call PANICKED (caught) rcx=owner+0xe0=0x{dialog_slot:x} -- no dispatch (investigate ABI)"
        ));
        return;
    }
    append_autoload_debug(format_args!(
        "fire-tfc-continue: selector returned 0x{:x} out=[0x{:x},0x{:x},0x{:x},0x{:x}] -- LOAD branch dispatched 0x{:x}; now POSTING the built job",
        sel_ret.unwrap_or(0),
        out_job[0],
        out_job[1],
        out_job[2],
        out_job[3],
        base + 0x9b3070usize
    ));
    // INSTALL the built job as currentTopMenuJob (CSPopupMenu+0xB0) via CS::MenuJob::Assign, so the
    // NATIVE per-frame menu pump runs its Run IN CONTEXT -- the fix for the self-pump menu-jumping (our
    // ExecuteMenuJob/drain-wrapper attempts ran the job out of context and never deserialized). The
    // selector/dispatcher only BUILD + return the job (out_job[0]); the native flow normally installs
    // it into a pump-drained slot. We replicate that install. bd menu-job-install-mechanism-2026-06-23
    // + inject-job-into-native-pump-slots-recipe-2026-06-23. NO input.
    let job = out_job[0];
    if !(job > OWNER_CTX_MIN_PLAUSIBLE_PTR && job < OWNER_CTX_MAX_PLAUSIBLE_PTR) {
        append_autoload_debug(format_args!(
            "fire-tfc-continue: selector out[0]=0x{job:x} is not a plausible built MenuJob -> nothing to install (dispatcher took the abort/noop branch?)"
        ));
        return;
    }
    let _ = MENU_PUMP_KICK_PTR_RVA;
    let _ = TITLE_OWNER_MENU_LIST_130_OFFSET;
    let _ = DIALOG_MENU_QUEUE_10_OFFSET;
    let _ = MENUJOB_PUSHBACK_RVA;
    let _ = MENU_DRAIN_WRAPPER_RVA;
    let _ = EXECUTE_MENU_JOB_RVA;
    // (REMOVED the dialog+0x50 count-reset hack: a live TitleTopDialog's +0x98 count is provably
    // always 0..8 -- the garbage we saw means we read a NON-LIVE/transient object, so zeroing it just
    // masks the real lifecycle problem and would CORRUPT a valid dialog's window list. The readiness
    // gate above (count<8) + the vtable/a40 gates are the correct fail-closed guard. bd
    // forge-breaks-lifecycle-native-confirm-is-correct-context-2026-06-23.)
    let _ = DIALOG_MENUWINDOW_VEC_50_OFFSET;
    let _ = DLFIXEDVECTOR_COUNT_48_OFFSET;
    // TARGET = owner+0x130, the title flow's ACTIVE MenuJob slot that STEP_MenuJobWait runs
    // ExecuteMenuJob(&owner+0x130) on EVERY frame (the title's own per-frame pump, definitely live at
    // the title menu -- unlike currentTopMenuJob+0xB0 which a run showed is EMPTY/unused by the title).
    // owner+0x130 is a MenuJob* slot (PushBackJob AV'd there because it is NOT a FixOrderJobSequence;
    // Assign -- a slot replace -- is the right primitive). bd currenttopjob-B0-empty-not-drained.
    let _ = GLOBAL_CSMENUMAN_RVA;
    let _ = CSMENUMAN_POPUP_80_OFFSET;
    let _ = CSPOPUP_TOP_JOB_B0_OFFSET;
    let dest = owner + TITLE_OWNER_MENU_LIST_130_OFFSET;
    let old_top = unsafe { safe_read_usize(dest) }.unwrap_or(0);
    // Pre-bump the job refcount (+0x8) so it survives the Assign regardless of the wrap's count.
    if let Some(rc) = unsafe { safe_read_usize(job + MENU_JOB_REFCOUNT_8_OFFSET) } {
        unsafe { *((job + MENU_JOB_REFCOUNT_8_OFFSET) as *mut usize) = rc.wrapping_add(1) };
    }
    // Assign(rcx = dest=&owner+0x130 active slot, rdx = &scratch, r8 = &src): unref old, install ours.
    let mut scratch: usize = 0;
    let mut src: usize = job;
    let assign: unsafe extern "system" fn(usize, usize, usize) =
        unsafe { std::mem::transmute(base + MENU_JOB_ASSIGN3_RVA) };
    let scratch_ptr = (&raw mut scratch) as usize;
    let src_ptr = (&raw mut src) as usize;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        assign(dest, scratch_ptr, src_ptr)
    }));
    let new_top = unsafe { safe_read_usize(dest) }.unwrap_or(0);
    append_autoload_debug(format_args!(
        "fire-tfc-continue: *** INSTALLED job=0x{job:x} into owner+0x130 (STEP_MenuJobWait active slot) via Assign 0x{:x} (tfc+0x18c was {nrf_before}->0; owner=0x{owner:x} dest=0x{dest:x} old_top=0x{old_top:x} new_top=0x{new_top:x} panicked={}) -- STEP_MenuJobWait should pump it IN CONTEXT. Watch oracle: c30 real, player present, now_loading ***",
        base + MENU_JOB_ASSIGN3_RVA,
        r.is_err()
    ));
}

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

/// Install the TitleTopDialog::update hook ONCE so the Continue build runs in the pump's live frame.
/// minhook on 0x1409aac10, mirroring install_continue_trace_hooks (queue_enable + MH_ApplyQueued +
/// mem::forget to keep the hook alive). Gated by `fire_tfc_continue_enabled` at the call site.
pub(crate) unsafe fn install_title_update_hook(base: usize) {
    if TITLE_UPDATE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "title-update-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "titletopdialog_update_9aac10",
            TITLE_TOP_DIALOG_UPDATE_RVA as u32,
            title_update_detour as *mut c_void,
            &TITLE_UPDATE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "title-update-hook: INSTALLED on TitleTopDialog::update 0x{:x} -- in-context Continue build armed",
            base + TITLE_TOP_DIALOG_UPDATE_RVA
        )),
        status => append_autoload_debug(format_args!(
            "title-update-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
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
const PAB_JOB_SLOT_130_OFFSET: usize = 0x130;
/// The job's completion press-count the predicate 0x1407a9200 reads (>=2 == complete).
const PAB_JOB_PRESS_COUNT_1E8_OFFSET: usize = 0x1e8;
/// The job's bound keycode (logged for identity validation + the documented fallback input bit).
const PAB_JOB_KEYCODE_180_OFFSET: usize = 0x180;
/// The "pressed" value the predicate treats as complete.
const PAB_PRESS_COUNT_SATISFIED: u32 = 2;
/// Upper sanity bound for a plausible press-count (reject garbage/unreadable reads -> keep waiting).
const PAB_COUNT_SANITY_MAX: u32 = 8;
/// Frames the press-any-button job must be built+valid before we advance (screen settle).
const PAB_ADVANCE_SETTLE_FRAMES: usize = 10;
/// Minimum plausible heap pointer (reject not-yet-built / garbage job slots).
const PAB_MIN_HEAP_PTR: usize = 0x10000;

/// Trampoline to the original PAB node-update. 0 = not hooked.
pub(crate) static PAB_ADVANCE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for installing the PAB node-update hook.
pub(crate) static PAB_ADVANCE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch: the readiness advance has fired (0 = not yet).
pub(crate) static PAB_ADVANCE_FIRED: AtomicUsize = AtomicUsize::new(0);
/// Valid-job-frame settle counter for the readiness advance.
pub(crate) static PAB_ADVANCE_SETTLE: AtomicUsize = AtomicUsize::new(0);

/// Arm the readiness-gated press-any-button advance. ENV `ER_EFFECTS_PAB_ADVANCE=1` or GAME_DIR file
/// `er-effects-pab-advance.txt`. DECOUPLED from `fire_tfc_continue_enabled` (that gate previously also
/// drove `maybe_auto_open_menu`, so removing it stranded a probe at press-any-button).
pub(crate) fn pab_advance_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_PAB_ADVANCE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-pab-advance.txt")
            .exists()
}

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
    ret
}

/// Gated, fail-closed, one-shot readiness advance past press-any-button. Reads the built job at
/// `[step+0x130]`; once it is a valid in-image job (we are at press-any-button) and has settled, sets
/// `[job+0x1e8]=2` so the job's own predicate (0x1407a9200) completes it through the native path. Logs
/// the job struct on first sighting so the run self-confirms the offsets. ZERO input.
unsafe fn pab_advance_try(step: usize) {
    if !pab_advance_enabled() || PAB_ADVANCE_FIRED.load(Ordering::SeqCst) != 0 {
        return;
    }
    if step <= PAB_MIN_HEAP_PTR {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    // The press-any-button job the native node-update builds/holds.
    let job = unsafe { safe_read_usize(step + PAB_JOB_SLOT_130_OFFSET) }.unwrap_or(0);
    if job <= PAB_MIN_HEAP_PTR || (job & (core::mem::size_of::<usize>() - 1)) != 0 {
        return; // job not built yet (pre-press-any-button) -> wait
    }
    // Identity: a valid in-image vtable (fail closed -> never write a wrong/garbage object).
    let vt = unsafe { safe_read_usize(job) }.unwrap_or(0);
    if !vtable_in_game_image(vt, base) {
        return;
    }
    let count = unsafe { safe_read_i32(job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) }.unwrap_or(-1) as u32;
    let keycode = unsafe { safe_read_i32(job + PAB_JOB_KEYCODE_180_OFFSET) }.unwrap_or(-1) as u32;
    let settle = PAB_ADVANCE_SETTLE.fetch_add(1, Ordering::SeqCst) + 1;
    if settle == 1 {
        append_autoload_debug(format_args!(
            "pab-advance: press-any-button job READY step=0x{step:x} job=0x{job:x} vt=0x{vt:x} [+0x1e8]count={count} [+0x180]keycode=0x{keycode:x} -- settling {PAB_ADVANCE_SETTLE_FRAMES} frames"
        ));
    }
    if settle < PAB_ADVANCE_SETTLE_FRAMES {
        return;
    }
    if count > PAB_COUNT_SANITY_MAX {
        return; // unreadable/garbage press-count -> do NOT write or latch; keep waiting
    }
    if count >= PAB_PRESS_COUNT_SATISFIED {
        // Already satisfied (a real press or prior advance) -> latch, nothing to do.
        PAB_ADVANCE_FIRED.store(1, Ordering::SeqCst);
        return;
    }
    // READINESS ADVANCE (zero-input): satisfy the job's own completion predicate.
    unsafe {
        *((job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) as *mut u32) = PAB_PRESS_COUNT_SATISFIED;
    }
    PAB_ADVANCE_FIRED.store(1, Ordering::SeqCst);
    let after = unsafe { safe_read_i32(job + PAB_JOB_PRESS_COUNT_1E8_OFFSET) }.unwrap_or(-1) as u32;
    append_autoload_debug(format_args!(
        "pab-advance: *** SET [job+0x1e8]={PAB_PRESS_COUNT_SATISFIED} (was {count}, now {after}) job=0x{job:x} keycode=0x{keycode:x} settle={settle} -- readiness-gated press-any-button advance, ZERO input ***"
    ));
}

/// Install the press-any-button node-update hook ONCE (minhook, mirroring `install_title_update_hook`).
/// Gated by `pab_advance_enabled` at the call site; the detour self-gates too (pass-through until armed).
pub(crate) unsafe fn install_pab_advance_hook(base: usize) {
    if PAB_ADVANCE_HOOK_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "pab-advance-hook: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "pab_node_update_7ad1c0",
            PAB_NODE_UPDATE_RVA,
            pab_node_update_detour as *mut c_void,
            &PAB_ADVANCE_ORIG,
        );
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => append_autoload_debug(format_args!(
            "pab-advance-hook: INSTALLED on PAB node-update 0x{:x} -- readiness press-any-button advance armed (zero-input)",
            base + PAB_NODE_UPDATE_RVA as usize
        )),
        status => append_autoload_debug(format_args!(
            "pab-advance-hook: MH_ApplyQueued failed: {status:?}"
        )),
    }
    std::mem::forget(hooks);
}

/// Per-frame PUMP for the built LoadGame job (bd drain-dialog-plus8-not-menujob-pump-our-job-directly).
/// Runs from the recurring game task once `maybe_fire_tfc_continue` armed `TFC_DRAIN_JOB`. Calls
/// `ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time)` DIRECTLY on our built job -- it invokes the job's
/// own `vtable[2]` (the LoadGame chain's Execute), advancing deser/world-stream, and zeroes the slot
/// when done (`ShouldContinue==false`). We pump OUR job (not the dialog's `+0x8` slot, which is not a
/// MenuJob and AV'd the queue-drain wrapper). Pure native call (no input). Stops on completion (slot
/// cleared), in-world, panic, or the tick cap. Every call is `catch_unwind`-guarded.
pub(crate) unsafe fn tfc_continue_drain_tick(base: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = TFC_DRAIN_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: in-world reached -> stop pumping (load complete)"
        ));
        return;
    }
    let ticks = TFC_DRAIN_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if ticks > TFC_DRAIN_TICK_CAP {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: tick cap {TFC_DRAIN_TICK_CAP} hit -> stop pumping (job never completed)"
        ));
        return;
    }
    // FD4Time: ExecuteMenuJob reads only +0x8 (f32 delta). Pass a 16-byte buffer with the frame delta.
    let mut time: [u8; FD4_TIME_SIZE] = [0u8; FD4_TIME_SIZE];
    time[FD4_TIME_DELTA_8_OFFSET..FD4_TIME_DELTA_8_OFFSET + core::mem::size_of::<f32>()]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let time_ptr = time.as_mut_ptr() as usize;
    // ExecuteMenuJob(rcx = &job_slot, rdx = &FD4Time): cur=*rcx; AtomicInc(cur+8); cur->vtable[2](...);
    // if done -> *rcx=0. Pass a local slot (job ptr persists in TFC_DRAIN_JOB across frames).
    let mut job_slot: usize = job;
    let slot_ptr = (&raw mut job_slot) as usize;
    let exec: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(base + EXECUTE_MENU_JOB_RVA) };
    let exec_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        exec(slot_ptr, time_ptr)
    }));
    if exec_ret.is_err() {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: ExecuteMenuJob 0x{:x}(rcx=&job=0x{job:x}) PANICKED (caught) at tick {ticks} -> stop pumping",
            base + EXECUTE_MENU_JOB_RVA
        ));
        return;
    }
    if job_slot == 0 {
        TFC_DRAIN_JOB.store(0, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "tfc-drain: job 0x{job:x} COMPLETED (slot cleared by ExecuteMenuJob) at tick {ticks} -> done pumping"
        ));
        return;
    }
    if ticks == 1 || ticks % (OWN_LOAD_STREAM_LOG_INTERVAL as usize) == 0 {
        append_autoload_debug(format_args!(
            "tfc-drain: tick {ticks} ExecuteMenuJob(job=0x{job:x}) delta={frame_delta} (pumping)"
        ));
    }
}

/// The D-pad Down button mask to inject for poll-frame `n` (counted from the first poll after
/// menu-open), per the INJECT_NAV schedule: settle, then `INJECT_NAV_MAX_CYCLES` tap+gap cycles
/// with Down asserted for the first `INJECT_NAV_TAP_LEN` frames of each cycle. Returns 0 (no
/// input) during settle, gaps, and after the cycles complete.
pub(crate) fn inject_nav_buttons(n: usize) -> u16 {
    const NONE: u16 = 0;
    if n < INJECT_NAV_SETTLE_FRAMES {
        return NONE;
    }
    let m = n - INJECT_NAV_SETTLE_FRAMES;
    if m >= INJECT_NAV_MAX_CYCLES * INJECT_NAV_CYCLE {
        return NONE;
    }
    if m % INJECT_NAV_CYCLE < INJECT_NAV_TAP_LEN {
        XINPUT_GAMEPAD_DPAD_DOWN
    } else {
        NONE
    }
}

/// AUTO-CONFIRM observe mode (er-effects-auto-confirm.txt): drive the game's OWN natural title
/// flow with Confirm input-taps so we can finally observe the view PAST the modal. No SetState
/// forcing, no input block, no custom dismiss -- just the press the game polls for.
pub(crate) fn auto_confirm_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_AUTO_CONFIRM").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-auto-confirm.txt")
            .exists()
}

/// Tap Confirm (inputmgr+0x90+0x3d, edge) to walk the NATURAL flow:
/// press-any-button -> [confirm] -> connection-error modal -> [confirm] -> MAIN MENU.
/// STOPS once the modal has been SEEN and is now GONE, so we never confirm a main-menu item
/// (Continue = load most-recent = SetState(5) save-write risk). Pure observation of the post-modal
/// view. Uses the builder capture hook only to know when the modal is up.
pub(crate) fn auto_confirm_tap() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Ok(base) = game_module_base() else {
        return;
    };
    install_auto_accept_hook();
    let modal_now = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst) != null;
    if modal_now {
        AUTO_CONFIRM_MODAL_SEEN.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
    let seen = AUTO_CONFIRM_MODAL_SEEN.load(Ordering::SeqCst) != null;
    if seen && !modal_now {
        // Past the modal -> stop tapping (do NOT confirm Continue on the main menu).
        return;
    }
    let inputmgr =
        unsafe { safe_read_usize(base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) }.unwrap_or(null);
    if inputmgr == null {
        return;
    }
    let frame = AUTO_CONFIRM_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    if frame % AUTO_CONFIRM_CYCLE_FRAMES < AUTO_CONFIRM_SET_FRAMES {
        unsafe {
            *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_CONFIRM_3D) as *mut u8) |=
                MENU_EVENT_PRESSED_BIT;
        }
    }
    if frame % AUTO_CONFIRM_LOG_INTERVAL == null as u64 {
        append_autoload_debug(format_args!(
            "auto-confirm: tap frame={frame} modal_now={modal_now} seen={seen} inputmgr=0x{inputmgr:x}"
        ));
    }
}

/// Whether STAGE 1d should SELF-FIRE the TitleTopDialog open-menu registrar (0x1409b24e0).
/// DEFAULT OFF (file-gated): with the connection-error modal now handled (clean headless boot),
/// the NATURAL Continue/Load main menu builds from SetState(2)=BeginLogo, and force-firing the
/// TitleTopDialog registrar opens a COMPETING dialog that prevents the natural menu's Load-Game
/// item d180 from ticking through the capture hooks. Off => let the natural menu surface d180.
pub(crate) fn own_stepper_selffire_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SELFFIRE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-selffire.txt")
            .exists()
}

/// Decode one x86-64 jmp-thunk hop. Matches either `add rcx,8 ; jmp rel32` (the MSVC
/// `std::function` `_Do_call` thunk family the FD4 menu-item action functor routes
/// through) or a bare `jmp rel32`, returning the absolute jump target. Returns `None`
/// when `addr` is not such a thunk (i.e. it is the real lambda body). Fault-tolerant:
/// reads via `safe_read_*`, never faults on unmapped code.
unsafe fn decode_thunk_hop(addr: usize) -> Option<usize> {
    // Low 5 bytes `48 83 C1 08 E9` = `add rcx,8 ; jmp` (little-endian in the qword).
    const ADDRCX8_JMP_PREFIX: usize = 0xE9_08C1_8348;
    const PREFIX_MASK_40: usize = 0xFF_FFFF_FFFF;
    const ADDRCX8_REL_OFF: usize = 5;
    const ADDRCX8_NEXT_OFF: i64 = 9;
    const JMP_OPCODE: usize = 0xE9;
    const JMP_OPCODE_MASK: usize = 0xFF;
    const JMP_REL_OFF: usize = 1;
    const JMP_NEXT_OFF: i64 = 5;
    let w0 = unsafe { safe_read_usize(addr) }?;
    if (w0 & PREFIX_MASK_40) == ADDRCX8_JMP_PREFIX {
        let rel = unsafe { safe_read_i32(addr + ADDRCX8_REL_OFF) }? as i64;
        Some((addr as i64 + ADDRCX8_NEXT_OFF + rel) as usize)
    } else if (w0 & JMP_OPCODE_MASK) == JMP_OPCODE {
        let rel = unsafe { safe_read_i32(addr + JMP_REL_OFF) }? as i64;
        Some((addr as i64 + JMP_NEXT_OFF + rel) as usize)
    } else {
        None
    }
}

/// STAGE 1 (strictly NO-WRITE): walk the title menu-item container at `owner+0x138` and
/// log each item, so we can (a) confirm the live FD4 SBO pointer-vector layout matches
/// the static RE (the captured recipe pointers were suspiciously low, so VERIFY before
/// any call) and (b) identify the Load-Game leaf by its `+0xa8` action functor's
/// `_Do_call` jmp-chain resolving to `dialog_factory 0x14081ead0` (Continue's instead
/// routes to confirm `0x140b0e180`, no dialog). All reads go through fault-tolerant
/// ReadProcessMemory -- NO writes, NO native calls, NO SetState -> save-safe at the
/// parked title. Tries both container interpretations (inline SBO vs base-pointer at
/// `+0x18`) and reports which yields valid menu-item vtables. Runs once.
unsafe fn diagnostic_menu_walk(
    owner: usize,
    module_base: usize,
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const ITEM_CONTAINER_138: usize = 0x138;
    const CONT_CURSOR_10: usize = 0x10;
    const CONT_ELEM0_18: usize = 0x18;
    const CONT_COUNT_60: usize = 0x60;
    const MENU_JOB_HOLDER_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const ITEM_VTABLE_RVA: usize = 0x02aa97e8;
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_DESC_58: usize = 0x58;
    const ITEM_RESULT_130: usize = 0x130;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const COUNT_SANITY_MIN: i32 = 1;
    const COUNT_SANITY_MAX: i32 = 32;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const INTERP_INLINE: usize = 0;
    const INTERP_BASE_PTR: usize = 1;
    const INTERP_COUNT: usize = 2;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let item_vtable_abs = module_base + ITEM_VTABLE_RVA;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    let container = owner + ITEM_CONTAINER_138;

    let state = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let cursor =
        unsafe { safe_read_i32(container + CONT_CURSOR_10) }.unwrap_or(TITLE_STATE_OWNER_GONE);
    let count =
        unsafe { safe_read_i32(container + CONT_COUNT_60) }.unwrap_or(TITLE_STATE_OWNER_GONE);
    let holder = unsafe { safe_read_usize(owner + MENU_JOB_HOLDER_E0) }.unwrap_or(null);
    let elem0_raw = unsafe { safe_read_usize(container + CONT_ELEM0_18) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "menu-walk[{tag}]: owner=0x{owner:x} state={state} container=0x{container:x} cursor={cursor} count={count} holder=0x{holder:x} elem0_raw=0x{elem0_raw:x} item_vt=0x{item_vtable_abs:x} dialog_factory=0x{dialog_factory_abs:x}"
        ));
    }
    if !(COUNT_SANITY_MIN..=COUNT_SANITY_MAX).contains(&count) {
        if verbose {
            append_autoload_debug(format_args!(
                "menu-walk[{tag}]: count={count} out of sane range -- container layout unverified (NO-WRITE)"
            ));
        }
        return None;
    }
    let count_usize = count as usize;

    let mut load_game_item: Option<usize> = None;
    let mut interp = INTERP_INLINE;
    while interp < INTERP_COUNT {
        let label = if interp == INTERP_INLINE {
            "inline"
        } else {
            "baseptr"
        };
        let base_ptr = if interp == INTERP_BASE_PTR {
            elem0_raw
        } else {
            null
        };
        if interp == INTERP_BASE_PTR && base_ptr == null {
            interp += WALK_STEP;
            continue;
        }
        let mut menu_items_found = WALK_START;
        let mut i = WALK_START;
        while i < count_usize {
            let item = if interp == INTERP_INLINE {
                unsafe { safe_read_usize(container + CONT_ELEM0_18 + i * PTR_STRIDE) }
            } else {
                unsafe { safe_read_usize(base_ptr + i * PTR_STRIDE) }
            }
            .unwrap_or(null);
            if item == null {
                i += WALK_STEP;
                continue;
            }
            let vtable = unsafe { safe_read_usize(item) }.unwrap_or(null);
            let is_menu_item = vtable == item_vtable_abs;
            if is_menu_item {
                menu_items_found += WALK_STEP;
            }
            let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
            let ctx = unsafe { safe_read_usize(item + ITEM_CTX_10) }.unwrap_or(null);
            let result = unsafe { safe_read_usize(item + ITEM_RESULT_130) }.unwrap_or(null);
            let desc_lo = unsafe { safe_read_usize(item + ITEM_DESC_58) }.unwrap_or(null);
            let desc_hi =
                unsafe { safe_read_usize(item + ITEM_DESC_58 + PTR_STRIDE) }.unwrap_or(null);
            // Follow the action functor's _Do_call jmp-chain; if it reaches the dialog
            // factory this is the Load-Game item.
            let mut is_load_game = false;
            let mut chain = String::new();
            if functor != null {
                let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
                let mut docall = if functor_vtable != null {
                    unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }
                        .unwrap_or(null)
                } else {
                    null
                };
                chain.push_str(&format!("docall=0x{docall:x}"));
                let mut hop = WALK_START;
                while hop < JMP_CHAIN_MAX_HOPS && docall != null {
                    if docall == dialog_factory_abs {
                        is_load_game = true;
                        break;
                    }
                    match unsafe { decode_thunk_hop(docall) } {
                        Some(next) => {
                            chain.push_str(&format!("->0x{next:x}"));
                            docall = next;
                        }
                        None => break,
                    }
                    hop += WALK_STEP;
                }
                if docall == dialog_factory_abs {
                    is_load_game = true;
                }
            }
            if is_menu_item && is_load_game && load_game_item.is_none() {
                load_game_item = Some(item);
            }
            if verbose {
                append_autoload_debug(format_args!(
                    "menu-walk[{tag}/{label}] i={i} item=0x{item:x} vt=0x{vtable:x} menu_item={is_menu_item} functor=0x{functor:x} ctx=0x{ctx:x} result=0x{result:x} desc=0x{desc_hi:016x}{desc_lo:016x} {chain} LOAD_GAME={is_load_game}"
                ));
            }
            i += WALK_STEP;
        }
        if verbose {
            append_autoload_debug(format_args!(
                "menu-walk[{tag}/{label}] summary: menu_items_found={menu_items_found}/{count_usize}"
            ));
        }
        interp += WALK_STEP;
    }
    load_game_item
}

/// Does `item`'s action functor at `+0xa8` resolve (through its `_Do_call` jmp-chain) to
/// the dialog factory 0x14081ead0? That uniquely marks the Load-Game leaf (Continue's
/// functor instead routes to the c30->SetState(5) confirm 0x140b0e180). Appends the decoded
/// chain to `chain` for logging. Fault-tolerant reads; never faults.
/// Does a std::function `functor` (the pointer ITSELF, not item+offset) resolve through its
/// `_Do_call` jmp-chain to the dialog factory 0x14081ead0? Used for the TitleTopDialog ROW entries
/// whose action functor lives at `[entry+0xf8]` (vs the MenuWindowJob `[item+0xa8]`). Fault-tolerant.
unsafe fn functor_ptr_hits_factory(functor: usize, module_base: usize, chain: &mut String) -> bool {
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    if functor == null {
        return false;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if functor_vtable == null {
        return false;
    }
    let mut docall =
        unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(null);
    chain.push_str(&format!("functor=0x{functor:x} docall=0x{docall:x}"));
    let mut hop = HOP_START;
    while hop < JMP_CHAIN_MAX_HOPS && docall != null {
        if docall == dialog_factory_abs {
            return true;
        }
        match unsafe { decode_thunk_hop(docall) } {
            Some(next) => {
                chain.push_str(&format!("->0x{next:x}"));
                docall = next;
            }
            None => break,
        }
        hop += HOP_STEP;
    }
    docall == dialog_factory_abs
}

unsafe fn functor_chain_hits_factory(item: usize, module_base: usize, chain: &mut String) -> bool {
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const JMP_CHAIN_MAX_HOPS: usize = 4;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog_factory_abs = module_base + DIALOG_FACTORY_RVA;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }.unwrap_or(null);
    if functor == null {
        return false;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if functor_vtable == null {
        return false;
    }
    let mut docall =
        unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(null);
    chain.push_str(&format!("functor=0x{functor:x} docall=0x{docall:x}"));
    let mut hop = HOP_START;
    while hop < JMP_CHAIN_MAX_HOPS && docall != null {
        if docall == dialog_factory_abs {
            return true;
        }
        match unsafe { decode_thunk_hop(docall) } {
            Some(next) => {
                chain.push_str(&format!("->0x{next:x}"));
                docall = next;
            }
            None => break,
        }
        hop += HOP_STEP;
    }
    docall == dialog_factory_abs
}

/// READ-ONLY enumerator of the TitleTopDialog's REALIZED selectable-entry vector -- the actual
/// Continue/Load-Game/New-Game rows the user navigates. These are NOT FD4 MenuWindowJobs in the
/// Sequence tree (which is why every job-tree walk + the 0x1407ad1c0 Update hook miss them); they
/// live in the dialog's own CSMenu sub-object (menu = dialog+0xa38) as a vector
/// `[menu+0x1290]..[menu+0x1298]` stride 0x210, cursor `[dialog+0xb0c]`, bound `[dialog+0xb08]`
/// (mainmenu-items-are-titletopdialog-widgets-not-fd4-jobs-2026). The confirm router 0x14078e1c0
/// fires an entry via `rax=[entry]; call [rax+0x10]` when `[entry+0xf8]!=0`. For each entry this
/// logs the vtable, its action method `[vtable+0x10]`, the `+0xf8` action-functor + its decoded
/// `_Do_call` jmp-chain, and whether either resolves to dialog_factory 0x14081ead0 (Load-Game) or
/// continue_confirm 0x140b0e180 (Continue). Pure vector math + reads (no game call) -> save-safe.
/// Returns (load_game_entry, continue_entry, cursor) for STAGE 2 to drive.
/// ZERO-INPUT title-menu Load fire (STATIC-RE validated, NO input injection). Replicates the
/// confirm router 0x14078e1c0's entry-action call directly (decoded: resolver 0x14078fbd0 returns
/// entry=[dialog+0x1290]+idx*0x210; if [entry+0xf8]!=0 -> rcx=[entry+0xf8]; call [[rcx]+0x10]).
/// Scans the realized TitleTopDialog row vector for the entry whose action functor [entry+0xf8]
/// chains to dialog_factory 0x14081ead0 (= Load Game; found empirically, NOT assumed by index),
/// sets cursor [dialog+0xb0c], and fires its _Do_call(rcx=action) -> builds the ProfileLoadDialog.
/// SELF-VALIDATING + FAIL-CLOSED: asserts the dialog vtable, that the row vector is populated, and
/// that a Load-Game entry was found, BEFORE firing -- so a non-realized/contaminated state is
/// caught, not absorbed. Build-only; the sole save-write is downstream (gated continue_confirm).
/// Returns true iff it fired.
unsafe fn fire_titletop_load_entry(dialog: usize, base: usize) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const VEC_BEGIN_1290: usize = 0x1290;
    const VEC_END_1298: usize = 0x1298;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_F8: usize = 0xf8;
    const CURSOR_B0C: usize = 0xb0c;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MAX_ENTRIES: usize = 16;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    // VALIDATE 1: dialog identity (runtime vtable 0x142b26468).
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "titletop-fire: dialog=0x{dialog:x} vt=0x{vt:x} != TitleTopDialog 0x{:x} -- ABORT (no fire)",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return false;
    }
    // VALIDATE 2: row vector realized/populated.
    let begin = unsafe { safe_read_usize(dialog + VEC_BEGIN_1290) }.unwrap_or(NULL);
    let end = unsafe { safe_read_usize(dialog + VEC_END_1298) }.unwrap_or(NULL);
    if begin == NULL || end <= begin {
        append_autoload_debug(format_args!(
            "titletop-fire: row vector EMPTY/unrealized vec=[0x{begin:x}..0x{end:x}] -- ABORT (rows not populated)"
        ));
        return false;
    }
    let count = (end - begin) / ENTRY_STRIDE_210;
    // VALIDATE 3: find Load-Game by action->dialog_factory (NOT assumed index).
    let mut found: Option<(usize, usize)> = None;
    let mut idx = IDX_START;
    while idx < count && idx < MAX_ENTRIES {
        let entry = begin + idx * ENTRY_STRIDE_210;
        let action = unsafe { safe_read_usize(entry + ENTRY_ACTION_F8) }.unwrap_or(NULL);
        if action != NULL {
            let mut chain = String::new();
            if unsafe { functor_ptr_hits_factory(action, base, &mut chain) } {
                found = Some((idx, action));
                append_autoload_debug(format_args!(
                    "titletop-fire: LOAD-GAME entry idx={idx} entry=0x{entry:x} action=0x{action:x} {chain}"
                ));
                break;
            }
        }
        idx += IDX_STEP;
    }
    let (load_idx, action) = match found {
        Some(v) => v,
        None => {
            append_autoload_debug(format_args!(
                "titletop-fire: NO Load-Game entry (action->dialog_factory) in {count} rows -- ABORT"
            ));
            return false;
        }
    };
    // All validated -> set cursor + fire the action's _Do_call(rcx=action) == the router's confirm.
    unsafe {
        *((dialog + CURSOR_B0C) as *mut i32) = load_idx as i32;
    }
    let vtable = unsafe { safe_read_usize(action) }.unwrap_or(NULL);
    let do_call = if vtable != NULL {
        unsafe { safe_read_usize(vtable + DOCALL_VTABLE_SLOT_10) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if do_call == NULL {
        append_autoload_debug(format_args!(
            "titletop-fire: action=0x{action:x} has no _Do_call -- ABORT"
        ));
        return false;
    }
    let f: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(do_call) };
    unsafe { f(action) };
    append_autoload_debug(format_args!(
        "titletop-fire: FIRED Load-Game idx={load_idx} do_call=0x{do_call:x} -- ProfileLoadDialog should now build at owner+0xe0"
    ));
    true
}

/// Baseline snapshot of the TitleTopDialog dword window, captured before the one deterministic
/// Down so the post-Down pass can diff against it and name the cursor field precisely.
static CURSOR_PROBE_BASELINE: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());

/// CURSOR-OFFSET PROBE (read-only, save-safe). `baseline=true`: snapshot the live TitleTopDialog
/// (owner+0xe0) dword window (cursor=0=Continue). `baseline=false` (after exactly one deterministic
/// Down, cursor=1=Load Game): re-read and log every offset whose value CHANGED, flagging the
/// 0->1 transition = the cursor field. Also logs the unverified static candidate [dialog+0xb0c] to
/// confirm/refute it. Pure reads via safe_read_usize -> never AVs.
unsafe fn cursor_offset_probe(owner: usize, base: usize, baseline: bool) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const DWORD_LO_MASK: usize = 0xffffffff;
    const DWORD_BYTES: usize = 4;
    const SCAN_START: usize = 0;
    const SCAN_STEP: usize = 1;
    const CURSOR_FROM: u32 = 0;
    const CURSOR_TO: u32 = 1;
    let tag = if baseline { "baseline" } else { "postdown" };
    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    let cand_b0c = unsafe { safe_read_usize(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }
        .map(|v| (v & DWORD_LO_MASK) as u32)
        .unwrap_or(u32::MAX);
    append_autoload_debug(format_args!(
        "cursor-probe[{tag}]: dialog=0x{dialog:x} vt=0x{dialog_vt:x}(want base+0x{:x}) candidate[+0xb0c]={cand_b0c}",
        TITLE_TOP_DIALOG_VTABLE_RVA
    ));
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return;
    }
    let read_dword = |off: usize| -> u32 {
        unsafe { safe_read_usize(dialog + off) }
            .map(|w| (w & DWORD_LO_MASK) as u32)
            .unwrap_or(u32::MAX)
    };
    if baseline {
        let mut snap = Vec::with_capacity(CURSOR_PROBE_SCAN_DWORDS);
        let mut i = SCAN_START;
        while i < CURSOR_PROBE_SCAN_DWORDS {
            snap.push(read_dword(i * DWORD_BYTES));
            i += SCAN_STEP;
        }
        if let Ok(mut b) = CURSOR_PROBE_BASELINE.lock() {
            *b = snap;
        }
        return;
    }
    let baseline_snap = match CURSOR_PROBE_BASELINE.lock() {
        Ok(b) if b.len() == CURSOR_PROBE_SCAN_DWORDS => b.clone(),
        _ => {
            append_autoload_debug(format_args!(
                "cursor-probe[postdown]: no baseline captured -- skip diff"
            ));
            return;
        }
    };
    let mut logged = SCAN_START;
    let mut i = SCAN_START;
    while i < CURSOR_PROBE_SCAN_DWORDS && logged < CURSOR_PROBE_LOG_CAP {
        let off = i * DWORD_BYTES;
        let old = baseline_snap[i];
        let new = read_dword(off);
        if old != new && new < CURSOR_PROBE_SMALL_MAX {
            let is_cursor = old == CURSOR_FROM && new == CURSOR_TO;
            append_autoload_debug(format_args!(
                "cursor-probe[postdown] CHANGED off=0x{off:x} {old}->{new}{}",
                if is_cursor { "  <== CURSOR (0->1)" } else { "" }
            ));
            logged += SCAN_STEP;
        }
        i += SCAN_STEP;
    }
    append_autoload_debug(format_args!(
        "cursor-probe[postdown]: diff complete ({logged} changed small dwords)"
    ));
}

unsafe fn dump_titletop_menu_entries(
    owner: usize,
    base: usize,
) -> (Option<usize>, Option<usize>, i32) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const MENU_SUBOBJ_A38: usize = DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    const ENTRY_VEC_BEGIN_1290: usize = 0x1290;
    const ENTRY_VEC_END_1298: usize = 0x1298;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_VT_SLOT_10: usize = 0x10;
    const ENTRY_FUNCTOR_F8: usize = 0xf8;
    const ENTRY_RESULT_130: usize = 0x130;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const MAX_ENTRIES: usize = 16;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    const JMP_HOPS: usize = 5;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const BAD_I32: i32 = -1;
    let ri32 = |addr: usize| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(BAD_I32)
    };
    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    let dialog_vt = if dialog != NULL {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let cursor = if dialog != NULL {
        ri32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET)
    } else {
        BAD_I32
    };
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "titletop-entries: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} (expect 0x{:x}) -- not the TitleTopDialog, skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return (None, None, cursor);
    }
    // The selectable-row vector does NOT live on the TitleTopDialog -- [dialog+0x1290] is GFx
    // markup text (runtime read = ASCII). The rows live on a SEPARATE title CSMenu controller
    // ("router_this", runtime vtable base+0x2afa070, ctor 0x1409060d8): the select router
    // 0x14078e1c0 calls the resolver 0x14078fbd0 with rcx=router_this, reading [router_this+0x1290]
    // /[+0x1298] (stride 0x210); cursor [+0xb0c], bound [+0xb08]. Locate router_this by scanning
    // the TitleTopDialog's fields for a pointer to an object whose [0] == that vtable. Pure reads
    // (safe_read_usize tolerates bad derefs) -> save-safe.
    const ROUTER_VTABLE_RVA: usize = 0x02afa070;
    const ROUTER_SCAN_QWORDS: usize = 0x400;
    const PTR_ALIGN_MASK: usize = 0x7;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const PTR_SZ: usize = 8;
    let router_vt = base + ROUTER_VTABLE_RVA;
    // Prefer the ctor-latched router_this (cap_csmenu_ctor_hook captures it at construction --
    // it is NOT field-linked from the TitleTopDialog). Fall back to a dialog-field scan.
    let mut router_this = MENU_ROUTER_THIS.load(Ordering::SeqCst);
    if router_this == NULL {
        let mut q = QW_START;
        while q < ROUTER_SCAN_QWORDS {
            let p = unsafe { safe_read_usize(dialog + q * PTR_SZ) }.unwrap_or(NULL);
            if p != NULL
                && (p & PTR_ALIGN_MASK) == QW_START
                && unsafe { safe_read_usize(p) }.unwrap_or(NULL) == router_vt
            {
                router_this = p;
                break;
            }
            q += QW_STEP;
        }
    }
    if router_this == NULL {
        append_autoload_debug(format_args!(
            "titletop-entries: dialog=0x{dialog:x} -- router_this (CSMenu vt=0x{router_vt:x}) NOT found in dialog fields; cursor={cursor} (rows unreachable via this path)"
        ));
        return (None, None, cursor);
    }
    let menu = router_this + MENU_SUBOBJ_A38;
    let cursor = ri32(router_this + DIALOG_SLOT_CURSOR_B0C_OFFSET);
    let vec_begin = unsafe { safe_read_usize(router_this + ENTRY_VEC_BEGIN_1290) }.unwrap_or(NULL);
    let vec_end = unsafe { safe_read_usize(router_this + ENTRY_VEC_END_1298) }.unwrap_or(NULL);
    let bound = ri32(router_this + DIALOG_SLOT_BOUND_B08_OFFSET);
    if vec_begin == NULL || vec_end <= vec_begin {
        append_autoload_debug(format_args!(
            "titletop-entries: router_this=0x{router_this:x} vec=[0x{vec_begin:x}..0x{vec_end:x}] EMPTY -- rows NOT populated headless; cursor={cursor} bound={bound}"
        ));
        return (None, None, cursor);
    }
    let count = (vec_end - vec_begin) / ENTRY_STRIDE_210;
    append_autoload_debug(format_args!(
        "titletop-entries: dialog=0x{dialog:x} menu=0x{menu:x} count={count} cursor={cursor} bound={bound} vec=[0x{vec_begin:x}..0x{vec_end:x}]"
    ));
    let factory_abs = base + DIALOG_FACTORY_RVA;
    let confirm_abs = base + CONTINUE_CONFIRM_RVA;
    let continue_wrapper_abs = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    // Decode a function/thunk address forward through up to JMP_HOPS jmp-thunks, reporting if it
    // reaches the Load-Game factory, Continue confirm, or native Continue wrapper. (Full-function
    // actions that only CALL the factory internally won't chain-resolve -- the raw action address is
    // logged regardless.)
    let classify = |start: usize, chain: &mut String| -> (bool, bool) {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == factory_abs {
                return (true, false);
            }
            if tgt == confirm_abs || tgt == continue_wrapper_abs {
                return (false, true);
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => {
                    chain.push_str(&format!("->0x{next:x}"));
                    tgt = next;
                }
                None => break,
            }
            hop += HOP_STEP;
        }
        (
            tgt == factory_abs,
            tgt == confirm_abs || tgt == continue_wrapper_abs,
        )
    };
    let mut load_game: Option<usize> = None;
    let mut continue_entry: Option<usize> = None;
    let mut idx = IDX_START;
    while idx < count && idx < MAX_ENTRIES {
        let entry = vec_begin + idx * ENTRY_STRIDE_210;
        let evt = unsafe { safe_read_usize(entry) }.unwrap_or(NULL);
        let action = if evt != NULL {
            unsafe { safe_read_usize(evt + ENTRY_ACTION_VT_SLOT_10) }.unwrap_or(NULL)
        } else {
            NULL
        };
        let functor = unsafe { safe_read_usize(entry + ENTRY_FUNCTOR_F8) }.unwrap_or(NULL);
        let result = unsafe { safe_read_usize(entry + ENTRY_RESULT_130) }.unwrap_or(NULL);
        // Classify the vtable action method, and (if present) the +0xf8 std::function's _Do_call.
        let mut action_chain = String::new();
        let (a_load, a_cont) = classify(action, &mut action_chain);
        let mut f_chain = String::new();
        let f_docall = if functor != NULL {
            let fvt = unsafe { safe_read_usize(functor) }.unwrap_or(NULL);
            if fvt != NULL {
                unsafe { safe_read_usize(fvt + ENTRY_ACTION_VT_SLOT_10) }.unwrap_or(NULL)
            } else {
                NULL
            }
        } else {
            NULL
        };
        let (f_load, f_cont) = if f_docall != NULL {
            classify(f_docall, &mut f_chain)
        } else {
            (false, false)
        };
        let is_load = a_load || f_load;
        let is_cont = a_cont || f_cont;
        append_autoload_debug(format_args!(
            "titletop-entry #{idx} entry=0x{entry:x} vt=0x{evt:x} action=0x{action:x}{action_chain} f8=0x{functor:x} f8_docall=0x{f_docall:x}{f_chain} result=0x{result:x} LOAD_GAME={is_load} CONTINUE={is_cont}"
        ));
        if is_load && load_game.is_none() {
            load_game = Some(entry);
        }
        if is_cont && continue_entry.is_none() {
            let receiver = if f_cont { functor } else { entry };
            let do_call = if f_cont { f_docall } else { action };
            continue_entry = Some(entry);
            MENU_CONTINUE_ENTRY.store(entry, Ordering::SeqCst);
            MENU_CONTINUE_FUNCTOR.store(receiver, Ordering::SeqCst);
            MENU_CONTINUE_DOCALL.store(do_call, Ordering::SeqCst);
            MENU_CONTINUE_ROUTER.store(router_this, Ordering::SeqCst);
            MENU_CONTINUE_INDEX.store(idx, Ordering::SeqCst);
        }
        idx += IDX_STEP;
    }
    (load_game, continue_entry, cursor)
}

/// SAVE-SAFE READ-ONLY structural scan of the OPEN TitleTopDialog for the Load-Game entry,
/// using the two RTTI fingerprints from the 2026-06-18 reconciliation
/// (bd title-load-is-profileloaddialog-NOT-movemapliststep-b78-dead-2026):
///   * d180 std::function `_Func_impl` vtable = `base+0x2ac3ea8` (its `_Do_call` 0x140820c60
///     `add rcx,8; jmp dialog_factory 0x14081ead0`), held at a MenuWindowJob's `+0xa8`;
///   * `CS::MenuMemberFuncJob<TitleTopDialog>` vtable = `base+0x2b265d0` (run 0x1409aaba0),
///     the entries the registrar 0x1409b24e0 registers into `[dialog+0xa48]`.
/// The prior d180-locate walked the FD4 MenuJobSequence tree (owner+0xe0/0x130/0x138) and never
/// surfaced the item, because the title rows are TitleTopDialog REGISTRY entries, not Sequence
/// children, AND `[dialog+0xa48]` is an opaque FD4 delegate registry (insert 0x1407a6c00, vcall
/// node-build -- not statically walkable). This instead does a BOUNDED flat scan of the dialog
/// object's own fields for any pointer to either fingerprint (and any object whose `+0xa8` holds
/// the d180 functor = a MenuWindowJob d180). Pure ReadProcessMemory (safe_read_usize tolerates bad
/// derefs) -> NO writes, NO native calls -> save-safe. RECON-ONLY: logs every hit and RETURNS
/// `(member_node, window_item)`: `member_node` = the first Load-Game CS::MenuMemberFuncJob node
/// (vt MEMBERFUNCJOB_VTABLE_RVA, member_fn reaches the dialog factory) -- this is the node the
/// native run 0x1409aaba0 is fired against; `window_item` = the first d180 MenuWindowJob item
/// (whose +0xa8 holds the d180 functor). It does NOT latch/advance (the caller decides) so a first
/// run stays NO-WRITE at the menu. (Extended 2026-06-18 to also return the MenuMemberFuncJob node
/// so native_load_enabled() can fire its run; previously it returned only the window item.)
unsafe fn scan_dialog_for_loadgame(owner: usize, base: usize) -> (Option<usize>, Option<usize>) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const ENTRY_REGISTRY_A48: usize = 0xa48;
    const ENTRY_SOURCE_A38: usize = DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    // d180 std::function _Func_impl vtable (user-capture-confirmed); MenuMemberFuncJob vtable.
    const FUNCTOR_VTABLE_RVA: usize = 0x02ac3ea8;
    const MEMBERFUNCJOB_VTABLE_RVA: usize = 0x02b265d0;
    const FACTORY_RVA: usize = 0x0081ead0;
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_ADJ_20: usize = 0x20;
    const SCAN_QWORDS: usize = 0x500;
    const PTR_SZ: usize = core::mem::size_of::<usize>();
    const PTR_ALIGN_MASK: usize = 0x7;
    const HEAP_LO: usize = 0x10000;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const HIT_CAP: usize = 24;
    const HIT_START: usize = 0;
    const HIT_STEP: usize = 1;

    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return (None, None);
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "loadgame-scan: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} != TitleTopDialog 0x{:x} -- skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return (None, None);
    }
    let functor_vt = base + FUNCTOR_VTABLE_RVA;
    let memberjob_vt = base + MEMBERFUNCJOB_VTABLE_RVA;
    let factory_abs = base + FACTORY_RVA;
    // Resolve a (member-)fn forward through up to JMP_HOPS jmp-thunks; true if it reaches the
    // Load-Game dialog_factory. (A full member fn that only CALLs the factory internally won't
    // chain-resolve; the raw fn VA is logged regardless for offline disasm.)
    let reaches_factory = |start: usize| -> bool {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == factory_abs {
                return true;
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => tgt = next,
                None => break,
            }
            hop += HOP_STEP;
        }
        tgt == factory_abs
    };
    let registry = unsafe { safe_read_usize(dialog + ENTRY_REGISTRY_A48) }.unwrap_or(NULL);
    let source = unsafe { safe_read_usize(dialog + ENTRY_SOURCE_A38) }.unwrap_or(NULL);
    append_autoload_debug(format_args!(
        "loadgame-scan: dialog=0x{dialog:x} registry(0xa48)=0x{registry:x} source(0xa38)=0x{source:x} functor_vt=0x{functor_vt:x} memberjob_vt=0x{memberjob_vt:x} -- scanning {SCAN_QWORDS} qwords"
    ));
    // DIRECT-BUILD r8 (ctor owner-obj) candidate validation (2026-06-18 breakthrough: the
    // ProfileLoadDialog ctor 0x1409a3d90 is COLD-VIABLE -- it builds router_this + slot rows
    // inline, no session/PGD/input-focus deps). dialog_factory 0x14081ead0 passes the ctor
    // r8 = *(capture+8); the gold capture showed that = owner+0x138, and the ctor reads the
    // profile ROW-VECTOR COUNT at [r8+0xa60]. Validate READ-ONLY which candidate has a plausible
    // vtable [+0] + a small row count [+0xa60] BEFORE any native build call (look before acting).
    const OWNER_MENU_OBJ_138: usize =
        TITLE_OWNER_MENU_LIST_130_OFFSET + core::mem::size_of::<usize>();
    const CTOR_ROW_COUNT_A60: usize = 0xa60;
    const CTOR_ROW_VEC_BEGIN_A58: usize = 0xa58;
    const R8_CAND_N: usize = 2;
    let cand_a = owner + OWNER_MENU_OBJ_138;
    let cand_b = unsafe { safe_read_usize(cand_a) }.unwrap_or(NULL);
    let cands: [(&str, usize); R8_CAND_N] = [("owner+0x138", cand_a), ("*(owner+0x138)", cand_b)];
    for (tag, c) in cands.iter() {
        if *c == NULL {
            continue;
        }
        let cvt = unsafe { safe_read_usize(*c) }.unwrap_or(NULL);
        let cnt = unsafe { safe_read_usize(*c + CTOR_ROW_COUNT_A60) }.unwrap_or(NULL);
        let vbeg = unsafe { safe_read_usize(*c + CTOR_ROW_VEC_BEGIN_A58) }.unwrap_or(NULL);
        append_autoload_debug(format_args!(
            "loadgame-scan: r8-cand[{tag}]=0x{c:x} vt=0x{cvt:x} rowvec_begin[+0xa58]=0x{vbeg:x} rowcount[+0xa60]=0x{cnt:x}"
        ));
    }
    let mut found_item: Option<usize> = None;
    let mut found_member_node: Option<usize> = None;
    let mut hits = HIT_START;
    let mut q = QW_START;
    while q < SCAN_QWORDS {
        let off = q * PTR_SZ;
        let p = unsafe { safe_read_usize(dialog + off) }.unwrap_or(NULL);
        if p != NULL && (p & PTR_ALIGN_MASK) == QW_START && p >= HEAP_LO {
            let vt = unsafe { safe_read_usize(p) }.unwrap_or(NULL);
            if vt == memberjob_vt {
                // (a) a MenuMemberFuncJob registry entry node.
                let mfn = unsafe { safe_read_usize(p + MEMBER_FN_18) }.unwrap_or(NULL);
                let mdlg = unsafe { safe_read_usize(p + MEMBER_DIALOG_10) }.unwrap_or(NULL);
                let madj = unsafe { safe_read_usize(p + MEMBER_ADJ_20) }.unwrap_or(NULL);
                let rf = reaches_factory(mfn);
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "loadgame-scan: dialog+0x{off:x} MenuMemberFuncJob node=0x{p:x} member_fn=0x{mfn:x} reaches_factory={rf} back=0x{mdlg:x} adj=0x{madj:x}"
                    ));
                }
                // The Load-Game run target: a MenuMemberFuncJob whose member_fn chains to the
                // dialog factory. Latch the FIRST such node (run 0x1409aaba0 fires against it).
                if rf && found_member_node.is_none() {
                    found_member_node = Some(p);
                }
                hits += HIT_STEP;
            } else if vt == functor_vt {
                // (b) the d180 functor object itself.
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "loadgame-scan: dialog+0x{off:x} -> d180 FUNCTOR object=0x{p:x} (vt 0x2ac3ea8)"
                    ));
                }
                hits += HIT_STEP;
            } else {
                // (c) a MenuWindowJob whose +0xa8 holds the d180 functor = the Load-Game item.
                let fa8 = unsafe { safe_read_usize(p + ITEM_FUNCTOR_A8) }.unwrap_or(NULL);
                if fa8 != NULL && (fa8 & PTR_ALIGN_MASK) == QW_START && fa8 >= HEAP_LO {
                    let fvt = unsafe { safe_read_usize(fa8) }.unwrap_or(NULL);
                    if fvt == functor_vt {
                        append_autoload_debug(format_args!(
                            "loadgame-scan: dialog+0x{off:x} -> d180 MenuWindowJob item=0x{p:x} item_vt=0x{vt:x} functor=0x{fa8:x} -- LOAD-GAME candidate"
                        ));
                        if found_item.is_none() {
                            found_item = Some(p);
                        }
                        hits += HIT_STEP;
                    }
                }
            }
        }
        q += QW_STEP;
    }
    append_autoload_debug(format_args!(
        "loadgame-scan: done hits={hits} found_member_node=0x{:x} found_item=0x{:x}",
        found_member_node.unwrap_or(NULL),
        found_item.unwrap_or(NULL)
    ));
    (found_member_node, found_item)
}

/// MODEL B (FACTORY-HOOK LATCH RECIPE 2026-06-18, bd
/// live-dialog-menuwindow-latch-via-factory-hook-0x14081e5e0-2026): READ-ONLY deterministic
/// acquisition of the two LIVE args the Load-Game dialog factory 0x14081ead0 needs -- the live
/// TitleTopDialog* (the factory rcx = its [+0xa38] TitleFlowContext capture) and the live host
/// MenuWindow* (the factory rdx). The MenuWindow is NOT persistently readable at the parked title
/// (probe-5 proved [td+0xa38] is a CS::TitleFlowContext, NOT a SceneObjProxy, and there is no
/// persistent SceneObjProxy to read the +0x20 back-ref from). Instead the host MenuWindow is
/// LATCHED at boot from rdx of the SceneObjProxy ctor 0x14074a700
/// (`scene_obj_proxy_ctor_hook` -> LATCHED_MENU_WINDOW; probe-6: the OLD TitleTopDialog-factory rdx
/// was a std::function delegate, NOT the MenuWindow).
///
/// CONVERGED recipe (all pure safe_read_usize / atomic load -> NO writes, NO native calls, never
/// AVs -> save-safe; fail-closed at every step, every step logged via append_autoload_debug):
///   1. td = *(owner+0xe0); require *(td) == base+TITLE_TOP_DIALOG_VTABLE_RVA (else fail-closed).
///   2. SELF-DIAGNOSTIC: read + LOG the TitleFlowContext capture *(td+0xa38) + its vtable (context
///      only; it is the factory rcx, never gates acquisition).
///   3. menu_window = LATCHED_MENU_WINDOW (SeqCst); fail-closed if 0 (factory not yet hit) or not a
///      canonical heap pointer. Read mwvt = *(menu_window); LOG menu_window + mwvt; if mwvt is
///      neither MenuWindow nor MenuWindowProxy LOG loudly but STILL return it (probe visibility).
///   4. Return (td, menu_window).
unsafe fn locate_live_loadgame_node(owner: usize, base: usize) -> Option<(usize, usize)> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;

    let title_vt = base + TITLE_TOP_DIALOG_VTABLE_RVA;
    let scene_proxy_vt = base + SCENE_OBJ_PROXY_VTABLE_RVA;
    let menu_vt = base + MENU_WINDOW_VTABLE_RVA;
    let menu_proxy_vt = base + MENU_WINDOW_PROXY_VTABLE_RVA;

    // (1) TitleTopDialog: owner+0xe0, vtable-gated (probe-2/3 runtime-confirmed).
    let td = unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(NULL);
    let tdvt = if td != NULL {
        unsafe { safe_read_usize(td) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if tdvt != title_vt {
        append_autoload_debug(format_args!(
            "live-dialog: owner+0x{:x}=0x{td:x} vt=0x{tdvt:x} != TitleTopDialog 0x{title_vt:x} -- title not up, fail-closed",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET
        ));
        return None;
    }
    append_autoload_debug(format_args!(
        "live-dialog: TitleTopDialog acquired owner+0x{:x}=0x{td:x} (vt 0x{tdvt:x})",
        TITLE_OWNER_MENU_HOLDER_E0_OFFSET
    ));

    // (2) SELF-DIAGNOSTIC (context only): the TitleFlowContext capture at td+0xa38. Probe-5 proved
    // this is a CS::TitleFlowContext (vt 0x142ac7f20), NOT a persistent SceneObjProxy, so it does
    // NOT yield the MenuWindow -- but it IS the correct factory rcx (= td+0xa38). LOG it for
    // context; it never gates acquisition.
    let capture =
        unsafe { safe_read_usize(td + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET) }.unwrap_or(NULL);
    let cvt = if capture != NULL {
        unsafe { safe_read_usize(capture) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_autoload_debug(format_args!(
        "live-dialog: capture *(td+0x{:x})=0x{capture:x} vt=0x{cvt:x} (TitleFlowContext; factory rcx) (probe scene_proxy_vt 0x{scene_proxy_vt:x})",
        DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET
    ));

    // (3) MenuWindow: READ the boot-latched host MenuWindow* (latched as rdx of the TitleTopDialog
    // ctor 0x14074a700 by `scene_obj_proxy_ctor_hook`). The MenuWindow is NOT persistently
    // readable at the parked title, so the latch is the only headless source. Fail-closed if 0.
    let menu_window = LATCHED_MENU_WINDOW.load(Ordering::SeqCst);
    if menu_window == NULL {
        append_autoload_debug(format_args!(
            "live-dialog: LATCHED_MENU_WINDOW is 0 (SceneObjProxy ctor 0x14074a700 not yet hit) -- fail-closed, no factory call"
        ));
        return None;
    }
    if menu_window < HEAP_LO || (menu_window & PTR_ALIGN_MASK) != NULL {
        append_autoload_debug(format_args!(
            "live-dialog: latched MenuWindow 0x{menu_window:x} is not a valid heap pointer -- fail-closed, no factory call"
        ));
        return None;
    }
    let mwvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(NULL);
    append_autoload_debug(format_args!(
        "live-dialog: latched MenuWindow=0x{menu_window:x} vt=0x{mwvt:x} (want MenuWindow 0x{menu_vt:x} or MenuWindowProxy 0x{menu_proxy_vt:x})"
    ));
    if mwvt != menu_vt && mwvt != menu_proxy_vt {
        // Loud log but STILL return it (probe visibility) -- the pointer is heap-canonical above.
        append_autoload_debug(format_args!(
            "live-dialog: unexpected latched MenuWindow vtable 0x{mwvt:x} (neither 0x{menu_vt:x} nor 0x{menu_proxy_vt:x}) -- returning anyway for probe visibility"
        ));
    }
    append_autoload_debug(format_args!(
        "live-dialog: ACQUIRED title_dialog=0x{td:x} (vt 0x{title_vt:x}) menu_window=0x{menu_window:x} via boot factory-hook latch"
    ));
    Some((td, menu_window))
}

/// MODEL B (FINAL RECIPE 2026-06-18): build the LIVE registered ProfileLoadDialog by calling the
/// dialog factory 0x14081ead0 WITH THE LIVE CALL-FRAME ARGS -- the only way the dialog becomes
/// live + pumped (the parameterless node-run builds a NON-LIVE dialog and discards it). The factory
/// reads the SceneProxy from [rcx] (r8 = *(dialog+0xa38), the live SceneProxy* the TitleTopDialog
/// ctor stored there at 0x1409a8213) and takes the live MenuWindow* as rdx. So:
///   factory(rcx = title_dialog + 0xa38, rdx = menu_window) -> ProfileLoadDialog* in rax.
/// This builds + registers the dialog into the menu group 0x143d87350 + active-screen set
/// intrinsically (registration is folded into the factory invocation under live args), which the
/// native pump then drives. We FAIL-CLOSED: re-validate the title_dialog vtable (0x142b26468) and
/// that its SceneProxy capture [+0xa38] + the menu_window are non-null heap BEFORE the call; a
/// mismatch returns false with NO native call. Zero-input (the game's own factory, no synthesis).
/// Returns true if the factory was invoked.
unsafe fn fire_live_loadgame_node(
    title_dialog: usize,
    menu_window: usize,
    base: usize,
    enter_stage2: bool,
) -> bool {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const HEAP_LO: usize = 0x10000;
    if title_dialog == NULL || menu_window == NULL {
        return false;
    }
    let dvt = unsafe { safe_read_usize(title_dialog) }.unwrap_or(NULL);
    let capture_slot = title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    let scene_proxy = unsafe { safe_read_usize(capture_slot) }.unwrap_or(NULL);
    if dvt != base + TITLE_TOP_DIALOG_VTABLE_RVA || scene_proxy < HEAP_LO || menu_window < HEAP_LO {
        append_autoload_debug(format_args!(
            "live-dialog: FIRE ABORT (fail-closed, NO native call) title_dialog=0x{title_dialog:x} vt=0x{dvt:x}(want 0x{:x}) scene_proxy([+0xa38])=0x{scene_proxy:x} menu_window=0x{menu_window:x}",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return false;
    }
    const RECORD_BASE_18: usize = 0x18;
    const RECORD_STATE_44: usize = 0x44;
    const RECORD_STRIDE_2A0: usize = 0x2a0;
    const RECORD_VALID_295: usize = 0x295;
    const RECORD_STATE_LOADABLE: i32 = 2;
    const RECORD_VALID_SET: u8 = 1;
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let gdm = game_data_man_ptr_or_null();
    let profile_summary = if gdm != NULL {
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if profile_summary != NULL && want_slot >= OWN_STEPPER_SLOT_ZERO {
        let activate: unsafe extern "system" fn(usize, i32) =
            unsafe { std::mem::transmute(base + PROFILE_SLOT_ACTIVATE_RVA) };
        unsafe { activate(profile_summary, want_slot) };
        let rec = profile_summary + RECORD_BASE_18 + (want_slot as usize) * RECORD_STRIDE_2A0;
        unsafe { *((rec + RECORD_VALID_295) as *mut u8) = RECORD_VALID_SET };
        unsafe { *((rec + RECORD_STATE_44) as *mut i32) = RECORD_STATE_LOADABLE };
        append_autoload_debug(format_args!(
            "live-dialog: pre-activated profile_summary=0x{profile_summary:x} slot={want_slot} rec=0x{rec:x} before factory so ProfileLoadDialog rows populate"
        ));
    }
    let factory: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(base + LIVE_DIALOG_FACTORY_RVA) };
    append_autoload_debug(format_args!(
        "live-dialog: FIRE factory 0x{:x}(rcx=title_dialog+0xa38=0x{capture_slot:x} [SceneProxy=0x{scene_proxy:x}], rdx=menu_window=0x{menu_window:x}) -- building LIVE registered ProfileLoadDialog",
        base + LIVE_DIALOG_FACTORY_RVA
    ));
    let dialog = unsafe { factory(capture_slot, menu_window) };
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    let dialog_vt = if dialog >= HEAP_LO {
        unsafe { safe_read_usize(dialog) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_autoload_debug(format_args!(
        "live-dialog: factory returned dialog=0x{dialog:x} vt=0x{dialog_vt:x} (want ProfileLoadDialog 0x{pld_vt:x})"
    ));
    // FIX 2 (probe-6): drive the RETURNED dialog directly -- do NOT scan the active-screen array
    // 0x143d6d8d0 (probe-2 proved it is MODEL-RENDERERS, never the PLD). If the returned vtable is
    // the ProfileLoadDialog, the normal autoload path stores it + transitions own_stepper to STAGE2
    // ACTIVATE on THAT pointer. The invalid/empty Continue UX fallback deliberately stops here so
    // the user sees the native Load Game menu instead of any automatic load/confirm.
    if dialog_vt != pld_vt {
        append_autoload_debug(format_args!(
            "live-dialog: returned dialog vtable 0x{dialog_vt:x} != ProfileLoadDialog 0x{pld_vt:x} -- fail-closed, STAY (NO-WRITE, no STAGE2)"
        ));
        return false;
    }
    if !enter_stage2 {
        append_autoload_debug(format_args!(
            "live-dialog: LIVE ProfileLoadDialog=0x{dialog:x} (vt 0x{pld_vt:x}) from factory return -- menu-only fallback, no STAGE2/no confirm"
        ));
        return true;
    }
    OWN_STEPPER_DIALOG.store(dialog, Ordering::SeqCst);
    own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
    append_autoload_debug(format_args!(
        "live-dialog: LIVE ProfileLoadDialog=0x{dialog:x} (vt 0x{pld_vt:x}) from factory return -- entering STAGE2 ACTIVATE (slot={})",
        OWN_STEPPER_SLOT.load(Ordering::SeqCst)
    ));
    true
}

#[derive(Clone, Copy)]
struct MenuActionNode {
    node: usize,
    node_vt: usize,
    registry: usize,
    member_dialog: usize,
    member_fn: usize,
    member_adjust: usize,
    window_item: usize,
}

#[derive(Clone, Copy)]
struct NativeContinueEntry {
    entry: usize,
    functor: usize,
    do_call: usize,
    router: usize,
    index: usize,
    cursor: i32,
}

#[derive(Clone, Copy)]
struct NativeContinueItemAction {
    item: usize,
    result: usize,
    result_vt: usize,
    functor: usize,
    do_call: usize,
}

#[derive(Clone, Copy)]
struct LiveDialogFireReady {
    title_dialog: usize,
    title_dialog_vt: usize,
    capture_slot: usize,
    capture: usize,
    capture_vt: usize,
    registry_vt: usize,
    menu_opened_latch: usize,
    menu_window: usize,
    menu_window_vt: usize,
}

#[derive(Clone, Copy)]
struct ProfileLoadDialogReady {
    dialog: usize,
    dvt: usize,
    bound: i32,
    cursor_now: i32,
    cursor_target: i32,
    expected_slot: i32,
    load_activate: usize,
    load_job_ctx: usize,
    load_job_ctx_vt: usize,
    player_game_data: usize,
}

#[derive(Clone, Copy)]
enum StartupModalBlockingState {
    Clear,
    Blocking {
        dialog: usize,
        vtable: usize,
        closing_latch: usize,
    },
}

struct ProductCoreAutoloadReady {
    committed: i32,
    requested: i32,
    table: usize,
    session: usize,
    game_data_man: usize,
    profile_summary: usize,
    iodev: usize,
    heap_allocator: usize,
    title_dialog: usize,
    title_in_loop: bool,
    title_in_textfadeout: bool,
    menu_opened_latch: usize,
    press_start_proxy: usize,
    press_start_context: usize,
}

struct TitlePressButtonComponent {
    proxy: usize,
    context: usize,
}

struct TitleDialogState {
    in_loop: bool,
    in_textfadeout: bool,
    menu_opened_latch: usize,
}

unsafe fn is_heap_aligned_ptr(ptr: usize) -> bool {
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    ptr >= HEAP_LO && (ptr & PTR_ALIGN_MASK) == TITLE_OWNER_SCAN_START_ADDRESS
}

fn vtable_in_game_image(vtable: usize, base: usize) -> bool {
    const MODULE_MIN_OFFSET: usize = 0x1000;
    const MODULE_SPAN_FALLBACK: usize = 0x3000000;
    vtable >= base + MODULE_MIN_OFFSET && vtable < base + MODULE_SPAN_FALLBACK
}

unsafe fn title_press_button_component_ready(
    dialog: usize,
    base: usize,
) -> Option<TitlePressButtonComponent> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
    let proxy_vt = unsafe { safe_read_usize(proxy) }.unwrap_or(null);
    if proxy_vt != base + SCENE_OBJ_PROXY_VTABLE_RVA {
        return None;
    }
    let context =
        unsafe { safe_read_usize(proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }.unwrap_or(null);
    if context == null {
        return None;
    }
    Some(TitlePressButtonComponent { proxy, context })
}

unsafe fn title_dialog_state(dialog: usize, base: usize) -> TitleDialogState {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    let menu_opened_latch =
        unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(null);
    TitleDialogState {
        in_loop,
        in_textfadeout,
        menu_opened_latch,
    }
}

unsafe fn title_boot_ready(owner: usize, base: usize) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let table =
        unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
    let session =
        unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }.unwrap_or(null);
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let dialog_vt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    if committed != TITLE_STEP_MENU_JOB_WAIT
        || requested != TITLE_STEP_MENU_JOB_WAIT
        || table != base + INNER_TITLE_STATE_TABLE_RVA
        || session == null
        || dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA
        || unsafe { title_press_button_component_ready(dialog, base) }.is_none()
    {
        return false;
    }
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    let in_textfadeout =
        unsafe { is_in_state(sm, base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) } != OWN_STEPPER_FALSE;
    in_loop || in_textfadeout
}

unsafe fn title_scheduler_ready(owner: usize, base: usize) -> bool {
    unsafe { title_boot_ready(owner, base) }
}

unsafe fn product_core_autoload_ready(
    owner: usize,
    base: usize,
    gm: usize,
    slot: i32,
) -> Option<ProductCoreAutoloadReady> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if slot < OWN_STEPPER_SLOT_ZERO || gm == null {
        return None;
    }
    let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
        .unwrap_or(TITLE_STATE_OWNER_GONE);
    let table =
        unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
    let session =
        unsafe { safe_read_usize(base + SESSION_SINGLETON_144588E98_RVA) }.unwrap_or(null);
    let game_data_man = game_data_man_ptr_or_null();
    let profile_summary = if game_data_man != null {
        unsafe { safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    let iodev = unsafe { safe_read_usize(base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
    let heap_allocator = crate::runtime_heap_allocator_ptr_or_null();
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let dialog_vt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    let press_start = if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
        unsafe { title_press_button_component_ready(dialog, base) }
    } else {
        None
    };
    let title_state = if dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA {
        Some(unsafe { title_dialog_state(dialog, base) })
    } else {
        None
    };
    if committed != TITLE_STEP_MENU_JOB_WAIT
        || requested != TITLE_STEP_MENU_JOB_WAIT
        || table != base + INNER_TITLE_STATE_TABLE_RVA
        || session == null
        || game_data_man == null
        || profile_summary == null
        || iodev == null
        || heap_allocator == null
        || press_start.is_none()
        || title_state.is_none()
    {
        return None;
    }
    let press_start = press_start?;
    let title_state = title_state?;
    Some(ProductCoreAutoloadReady {
        committed,
        requested,
        table,
        session,
        game_data_man,
        profile_summary,
        iodev,
        heap_allocator,
        title_dialog: dialog,
        title_in_loop: title_state.in_loop,
        title_in_textfadeout: title_state.in_textfadeout,
        menu_opened_latch: title_state.menu_opened_latch,
        press_start_proxy: press_start.proxy,
        press_start_context: press_start.context,
    })
}

pub(crate) unsafe fn product_core_autoload_tick(module_base: usize, slot: i32, tick: u64) -> bool {
    if !product_autoload_enabled() {
        return false;
    }
    PRODUCT_CORE_AUTOLOAD_TICKS.fetch_add(1, Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    PRODUCT_CORE_LAST_PHASE.store(phase, Ordering::SeqCst);
    if phase == OWN_STEPPER_PHASE_DONE {
        return true;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Some(owner_ptr) = (unsafe { title_owner(module_base) }) else {
        PRODUCT_CORE_READY_BLOCKS.fetch_add(1, Ordering::SeqCst);
        PRODUCT_CORE_LAST_BLOCKER.store(PRODUCT_CORE_BLOCKER_NO_TITLE_OWNER, Ordering::SeqCst);
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "product-core-autoload: waiting for title owner before native save-load core tick={tick}"
            ));
        }
        return true;
    };
    let owner = owner_ptr as usize;
    PRODUCT_CORE_OWNER_TICKS.fetch_add(1, Ordering::SeqCst);
    PRODUCT_CORE_LAST_OWNER.store(owner, Ordering::SeqCst);
    let gm = game_man_ptr_or_null();
    if phase == OWN_STEPPER_PHASE_S2_INVOKE
        || phase == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        unsafe { own_stepper_stage2(owner, module_base, gm, slot, tick, null) };
        return true;
    }
    if phase == OWN_STEPPER_PHASE_MENU
        && FULLREAD_PHASE.load(Ordering::SeqCst) == FULLREAD_PHASE_GUARD
    {
        // Native Continue can reset title-menu visual latches while its modal-confirm branch waits.
        // The product intent is to disable that confirm wait after the native load has produced
        // loaded-slot evidence, so keep the post-submit guard running instead of re-gating on title
        // visuals that are no longer authoritative.
        let guard_ready = ProductCoreAutoloadReady {
            committed: TITLE_STATE_OWNER_GONE,
            requested: TITLE_STATE_OWNER_GONE,
            table: null,
            session: null,
            game_data_man: null,
            profile_summary: null,
            iodev: null,
            heap_allocator: null,
            title_dialog: null,
            title_in_loop: false,
            title_in_textfadeout: false,
            menu_opened_latch: null,
            press_start_proxy: null,
            press_start_context: null,
        };
        unsafe { product_continue_autoload_tick(owner, module_base, gm, slot, tick, &guard_ready) };
        return true;
    }
    let Some(ready) = (unsafe { product_core_autoload_ready(owner, module_base, gm, slot) }) else {
        let committed = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) }
            .unwrap_or(TITLE_STATE_OWNER_GONE);
        let requested = unsafe { safe_read_i32(owner + TITLE_OWNER_STATE_OFFSET) }
            .unwrap_or(TITLE_STATE_OWNER_GONE);
        let table =
            unsafe { safe_read_usize(owner + TITLE_OWNER_INSTANCE_TABLE_OFFSET) }.unwrap_or(null);
        let session = unsafe { safe_read_usize(module_base + SESSION_SINGLETON_144588E98_RVA) }
            .unwrap_or(null);
        let game_data_man = game_data_man_ptr_or_null();
        let profile_summary = if game_data_man != null {
            unsafe { safe_read_usize(game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) }
                .unwrap_or(null)
        } else {
            null
        };
        let iodev = unsafe { safe_read_usize(module_base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
        let heap_allocator = crate::runtime_heap_allocator_ptr_or_null();
        let dialog =
            unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
        let dialog_vt = if dialog != null {
            unsafe { safe_read_usize(dialog) }.unwrap_or(null)
        } else {
            null
        };
        let press_start_proxy = dialog + TITLE_PRESS_START_SCENE_PROXY_B78_OFFSET;
        let press_start_vt = if dialog != null {
            unsafe { safe_read_usize(press_start_proxy) }.unwrap_or(null)
        } else {
            null
        };
        let press_start_context = if press_start_vt == module_base + SCENE_OBJ_PROXY_VTABLE_RVA {
            unsafe { safe_read_usize(press_start_proxy + SCENE_OBJ_PROXY_CONTEXT_20_OFFSET) }
                .unwrap_or(null)
        } else {
            null
        };
        let (title_loop, title_textfadeout, menu_opened_latch) =
            if dialog_vt == module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                let state = unsafe { title_dialog_state(dialog, module_base) };
                (state.in_loop, state.in_textfadeout, state.menu_opened_latch)
            } else {
                (false, false, null)
            };
        PRODUCT_CORE_LAST_TITLE_DIALOG.store(dialog, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_DIALOG_VT.store(dialog_vt, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_IN_LOOP.store(title_loop as usize, Ordering::SeqCst);
        PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT.store(title_textfadeout as usize, Ordering::SeqCst);
        PRODUCT_CORE_LAST_MENU_OPENED_LATCH.store(menu_opened_latch, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_PROXY.store(press_start_proxy, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_VT.store(press_start_vt, Ordering::SeqCst);
        PRODUCT_CORE_LAST_PRESS_START_CONTEXT.store(press_start_context, Ordering::SeqCst);
        let blocker =
            if committed != TITLE_STEP_MENU_JOB_WAIT || requested != TITLE_STEP_MENU_JOB_WAIT {
                PRODUCT_CORE_BLOCKER_TITLE_OWNER_STATE
            } else if table != module_base + INNER_TITLE_STATE_TABLE_RVA {
                PRODUCT_CORE_BLOCKER_TITLE_TABLE
            } else if session == null {
                PRODUCT_CORE_BLOCKER_SESSION
            } else if game_data_man == null {
                PRODUCT_CORE_BLOCKER_GAME_DATA_MAN
            } else if profile_summary == null {
                PRODUCT_CORE_BLOCKER_PROFILE_SUMMARY
            } else if iodev == null {
                PRODUCT_CORE_BLOCKER_IODEV
            } else if heap_allocator == null {
                PRODUCT_CORE_BLOCKER_HEAP_ALLOCATOR
            } else if dialog_vt != module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                PRODUCT_CORE_BLOCKER_TITLE_DIALOG
            } else if press_start_vt != module_base + SCENE_OBJ_PROXY_VTABLE_RVA
                || press_start_context == null
            {
                PRODUCT_CORE_BLOCKER_PRESS_START
            } else if !title_loop
                && !title_textfadeout
                && menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
            {
                PRODUCT_CORE_BLOCKER_TITLE_STATE
            } else {
                PRODUCT_CORE_BLOCKER_UNKNOWN
            };
        PRODUCT_CORE_READY_BLOCKS.fetch_add(1, Ordering::SeqCst);
        PRODUCT_CORE_LAST_BLOCKER.store(blocker, Ordering::SeqCst);
        if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
            append_autoload_debug(format_args!(
                "product-core-autoload: waiting for core readiness owner=0x{owner:x} state={committed}/{requested} table=0x{table:x} session=0x{session:x} gm=0x{gm:x} gdm=0x{game_data_man:x} profile=0x{profile_summary:x} iodev=0x{iodev:x} heap=0x{heap_allocator:x} title_loop={title_loop} title_textfadeout={title_textfadeout} menu_latch={menu_opened_latch} press_start_proxy=0x{press_start_proxy:x} press_start_vt=0x{press_start_vt:x} press_start_ctx=0x{press_start_context:x} slot={slot} tick={tick}"
            ));
        }
        return true;
    };
    PRODUCT_CORE_LAST_TITLE_DIALOG.store(ready.title_dialog, Ordering::SeqCst);
    PRODUCT_CORE_LAST_TITLE_DIALOG_VT.store(
        unsafe { safe_read_usize(ready.title_dialog) }.unwrap_or(null),
        Ordering::SeqCst,
    );
    PRODUCT_CORE_LAST_TITLE_IN_LOOP.store(ready.title_in_loop as usize, Ordering::SeqCst);
    PRODUCT_CORE_LAST_TITLE_IN_TEXTFADEOUT
        .store(ready.title_in_textfadeout as usize, Ordering::SeqCst);
    PRODUCT_CORE_LAST_MENU_OPENED_LATCH.store(ready.menu_opened_latch, Ordering::SeqCst);
    PRODUCT_CORE_LAST_PRESS_START_PROXY.store(ready.press_start_proxy, Ordering::SeqCst);
    PRODUCT_CORE_LAST_PRESS_START_VT.store(
        unsafe { safe_read_usize(ready.press_start_proxy) }.unwrap_or(null),
        Ordering::SeqCst,
    );
    PRODUCT_CORE_LAST_PRESS_START_CONTEXT.store(ready.press_start_context, Ordering::SeqCst);
    PRODUCT_CORE_READY_SUCCESSES.fetch_add(1, Ordering::SeqCst);
    PRODUCT_CORE_LAST_BLOCKER.store(PRODUCT_CORE_BLOCKER_READY, Ordering::SeqCst);
    if phase == OWN_STEPPER_PHASE_MENU {
        if ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
            && OWN_STEPPER_MENU_OPENED
                .compare_exchange(
                    OWN_STEPPER_MENU_OPENED_NO,
                    OWN_STEPPER_CALL_INC,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            // Lever-3 (narrow registrar advance): the native title press-accept handler 0x1409b1260
            // sets the menu-system singleton's +0 byte to 1 BEFORE tail-jumping to this same
            // registrar -- the missing piece that makes it open the menu IN PLACE rather than
            // spawning the competing dialog a bare self-fire produced (and the route that reaches
            // the main menu without the language/ToS the broad global accept byte over-triggers).
            // Replicate that flag set, gated, just before the (already vtable-validated) open_menu.
            // Zero-input, no save write.
            if title_registrar_advance_gate_enabled() {
                let singleton = unsafe {
                    *((module_base + TITLE_MENU_TRANSITION_SINGLETON_RVA) as *const usize)
                };
                if singleton != TITLE_OWNER_SCAN_START_ADDRESS && singleton != null {
                    unsafe { *(singleton as *mut u8) = TITLE_MENU_TRANSITION_FLAG_SET_VALUE };
                    append_autoload_debug(format_args!(
                        "title_registrar_advance: set menu-transition singleton [0x{:x}]->+0=1 before open-menu",
                        module_base + TITLE_MENU_TRANSITION_SINGLETON_RVA
                    ));
                }
            }
            let open_menu: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_OPEN_MENU_RVA) };
            unsafe { open_menu(ready.title_dialog) };
            timeline_event(
                "T_menu_open",
                tick,
                format_args!(
                    "product-core dialog=0x{:x} press_start_proxy=0x{:x}",
                    ready.title_dialog, ready.press_start_proxy
                ),
            );
            append_autoload_debug(format_args!(
                "product-core-autoload: PRESS BUTTON component ready; self-fire native open-menu 0x{:x}(dialog=0x{:x}) on validated title dialog + latch-clear before native save-load core; TitleTopDialog::open_menu writes latch and does not require Loop/TextFadeout state",
                module_base + TITLE_TOP_DIALOG_OPEN_MENU_RVA,
                ready.title_dialog
            ));
            return true;
        }
        if !ready.title_in_textfadeout && ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for title open-menu semantic confirmation dialog=0x{:x} loop={} textfadeout={} latch={} press_start_proxy=0x{:x} slot={slot} tick={tick}",
                    ready.title_dialog,
                    ready.title_in_loop,
                    ready.title_in_textfadeout,
                    ready.menu_opened_latch,
                    ready.press_start_proxy
                ));
            }
            return true;
        }
        if !unsafe { product_continue_action_ready(&ready, module_base, gm, slot) } {
            if tick % OWN_STEPPER_LOG_INTERVAL == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native Continue action readiness owner=0x{owner:x} state={}/{} dialog=0x{:x} menu_latch={} press_start_proxy=0x{:x} slot={slot} -- no direct_build/input fallback",
                    ready.committed,
                    ready.requested,
                    ready.title_dialog,
                    ready.menu_opened_latch,
                    ready.press_start_proxy
                ));
            }
            return true;
        }
        unsafe { product_continue_autoload_tick(owner, module_base, gm, slot, tick, &ready) };
    }
    let phase_now = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    if phase_now == OWN_STEPPER_PHASE_S2_INVOKE
        || phase_now == OWN_STEPPER_PHASE_S2_ACTIVATE
        || phase_now == OWN_STEPPER_PHASE_S2_MOUNT_POLL
        || phase_now == OWN_STEPPER_PHASE_S2_CONFIRM
    {
        unsafe { own_stepper_stage2(owner, module_base, gm, slot, tick, null) };
    }
    true
}

unsafe fn product_continue_action_ready(
    ready: &ProductCoreAutoloadReady,
    base: usize,
    gm: usize,
    slot: i32,
) -> bool {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if slot < OWN_STEPPER_SLOT_ZERO
        || gm == null
        || ready.menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO
    {
        return false;
    }
    let dialog_vt = unsafe { safe_read_usize(ready.title_dialog) }.unwrap_or(null);
    dialog_vt == base + TITLE_TOP_DIALOG_VTABLE_RVA
}

fn record_continue_candidate(item: usize, accept_predicate: usize, base: usize) {
    const MENU_ITEM_ACCEPT_IDLE_RVA: usize = 0x007add70;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if item == null {
        return;
    }
    MENU_CONTINUE_CANDIDATE_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_CONTINUE_CANDIDATE_ITEM.store(item, Ordering::SeqCst);
    let prior = MENU_CONTINUE_CANDIDATE_LAST_ACCEPT.swap(accept_predicate, Ordering::SeqCst);
    if prior != null && prior != accept_predicate {
        MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES.fetch_add(1, Ordering::SeqCst);
        append_continue_trace(format_args!(
            "MENU-CONTINUE-CANDIDATE accept predicate changed item=0x{item:x} prior=0x{prior:x} now=0x{accept_predicate:x}"
        ));
    }
    if base != null && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA {
        MENU_CONTINUE_CANDIDATE_NATIVE_ACCEPT_HITS.fetch_add(1, Ordering::SeqCst);
    } else if base != null && accept_predicate == base + MENU_ITEM_ACCEPT_IDLE_RVA {
        MENU_CONTINUE_CANDIDATE_IDLE_ACCEPT_HITS.fetch_add(1, Ordering::SeqCst);
    } else {
        MENU_CONTINUE_CANDIDATE_OTHER_ACCEPT_HITS.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe fn product_continue_item_action(base: usize) -> Option<NativeContinueItemAction> {
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let item = match MENU_CONTINUE_ITEM.load(Ordering::SeqCst) {
        TITLE_OWNER_SCAN_START_ADDRESS => MENU_CONTINUE_CANDIDATE_ITEM.load(Ordering::SeqCst),
        item => item,
    };
    if item == null {
        return None;
    }
    let item_vt = unsafe { safe_read_usize(item) }?;
    if item_vt != base + MENU_WINDOW_JOB_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} vt=0x{item_vt:x} expected=0x{:x}",
            base + MENU_WINDOW_JOB_VTABLE_RVA
        ));
        return None;
    }
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }?;
    if functor == null {
        return None;
    }
    let functor_vt = unsafe { safe_read_usize(functor) }?;
    let do_call = unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }?;
    if do_call != base + MENU_TITLE_CONTINUE_DOCALL_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} functor=0x{functor:x} docall=0x{do_call:x} expected=0x{:x}",
            base + MENU_TITLE_CONTINUE_DOCALL_RVA
        ));
        return None;
    }
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_IDLE_RVA: usize = 0x007add70;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }?;
    record_continue_candidate(item, accept_predicate, base);
    if accept_predicate == base + MENU_ITEM_ACCEPT_IDLE_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} accept_predicate=0x{accept_predicate:x} (constant false idle predicate) -- not a semantic accept-ready Continue item"
        ));
        return None;
    }
    if accept_predicate != base + MENU_ITEM_ACCEPT_NATIVE_RVA {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} accept_predicate=0x{accept_predicate:x} expected native accept predicate 0x{:x}",
            base + MENU_ITEM_ACCEPT_NATIVE_RVA
        ));
        return None;
    }
    if MENU_CONTINUE_ITEM
        .compare_exchange(
            TITLE_OWNER_SCAN_START_ADDRESS,
            item,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_ok()
    {
        append_autoload_debug(format_args!(
            "product-core-autoload: promoted candidate native Continue MenuWindowJob item=0x{item:x} accept_predicate=0x{accept_predicate:x}"
        ));
    }
    let result = unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }?;
    if result == null {
        return None;
    }
    let result_vt = unsafe { safe_read_usize(result) }?;
    if !vtable_in_game_image(result_vt, base) {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue MenuWindowJob rejected item=0x{item:x} result=0x{result:x} result_vt=0x{result_vt:x}"
        ));
        return None;
    }
    Some(NativeContinueItemAction {
        item,
        result,
        result_vt,
        functor,
        do_call,
    })
}

unsafe fn submit_native_continue_item_action(
    action: NativeContinueItemAction,
    base: usize,
) -> Option<i32> {
    const MENU_ITEM_RESULT_MODE_UNKNOWN: i32 = i32::MIN;
    let diagnostic_mode = unsafe { safe_read_i32(action.result + MENU_ITEM_RESULT_MODE_58_OFFSET) }
        .unwrap_or(MENU_ITEM_RESULT_MODE_UNKNOWN);
    let event_handler =
        unsafe { safe_read_usize(action.result_vt + MENU_ITEM_RESULT_EVENT_SLOT_60_OFFSET) }?;
    if !vtable_in_game_image(event_handler, base) {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue submit ABI rejected item=0x{:x} result=0x{:x} result_vt=0x{:x} event_handler=0x{event_handler:x} diagnostic_mode={diagnostic_mode}",
            action.item, action.result, action.result_vt
        ));
        return None;
    }
    const CONTINUE_WRAPPER_EVENT_WORDS: usize = 2;
    const CONTINUE_WRAPPER_EVENT_CODE_INDEX: usize = 0;
    const CONTINUE_WRAPPER_EVENT_PAYLOAD_INDEX: usize = 1;
    let native_submit = base + MENU_ITEM_SUBMIT_RVA;
    let fd4_event_constructor = base + FD4_EVENT_CONSTRUCTOR_RVA;
    let native_submit_fn: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(native_submit) };
    append_autoload_debug(format_args!(
        "product-core-autoload: native Continue submit ABI proven item=0x{:x} result=0x{:x} result_vt=0x{:x} event_handler=0x{event_handler:x} native_submit=0x{native_submit:x} fd4_event_ctor=0x{fd4_event_constructor:x} diagnostic_mode={diagnostic_mode} -- result+0x58 logged only, never used as readiness",
        action.item, action.result, action.result_vt
    ));
    unsafe { native_submit_fn(action.result) };
    append_autoload_debug(format_args!(
        "product-core-autoload: native Continue submit dispatcher returned after event_handler=0x{event_handler:x} -- modal-confirm wait remains disabled downstream until loaded evidence"
    ));
    Some(diagnostic_mode)
}

unsafe fn product_continue_entry_action(owner: usize, base: usize) -> Option<NativeContinueEntry> {
    const ROUTER_CURSOR_OFFSET: usize = DIALOG_SLOT_CURSOR_B0C_OFFSET;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let (_, continue_entry, cursor) = unsafe { dump_titletop_menu_entries(owner, base) };
    let entry = continue_entry.unwrap_or_else(|| MENU_CONTINUE_ENTRY.load(Ordering::SeqCst));
    let mut functor = MENU_CONTINUE_FUNCTOR.load(Ordering::SeqCst);
    let mut do_call = MENU_CONTINUE_DOCALL.load(Ordering::SeqCst);
    let mut router = MENU_CONTINUE_ROUTER.load(Ordering::SeqCst);
    let mut index = MENU_CONTINUE_INDEX.load(Ordering::SeqCst);
    let mut entry = entry;
    if entry == null || functor == null || do_call == null || index == null {
        return None;
    }
    let do_call_vtable = unsafe { safe_read_usize(functor) }.unwrap_or(null);
    if do_call_vtable == null || !vtable_in_game_image(do_call_vtable, base) {
        append_autoload_debug(format_args!(
            "product-core-autoload: native Continue row rejected functor=0x{functor:x} vt=0x{do_call_vtable:x} entry=0x{entry:x}"
        ));
        return None;
    }
    let live_cursor = unsafe { safe_read_i32(router + ROUTER_CURSOR_OFFSET) }.unwrap_or(cursor);
    Some(NativeContinueEntry {
        entry,
        functor,
        do_call,
        router,
        index,
        cursor: live_cursor,
    })
}

unsafe fn captured_continue_task_node(base: usize) -> usize {
    let node = MENU_CONTINUE_TASK_NODE.load(Ordering::SeqCst);
    if node == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let update_rva = unsafe { task_node_update_rva(base, node) };
    if update_rva != TRACE_MENU_CONTINUE_WRAPPER_RVA as usize {
        append_autoload_debug(format_args!(
            "product-core-autoload: captured Continue task node 0x{node:x} rejected update_rva=0x{update_rva:x} expected=0x{:x}",
            TRACE_MENU_CONTINUE_WRAPPER_RVA as usize
        ));
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    node
}

unsafe fn drive_product_continue_post_click_dispatchers(base: usize, slot: i32) {
    let synth = &raw mut SYNTH_MMS_OWNER as *mut u8;
    unsafe {
        *synth.add(SYNTH_MMS_SKIP_APPLY_12A_OFFSET) = SYNTH_MMS_SKIP_APPLY_ON;
        *(synth.add(SYNTH_MMS_DESER_SLOT_12C_OFFSET) as *mut i32) = slot;
    }
    let synth_ptr = synth as usize;
    let dispatcher1: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + B80_DISPATCHER1_RVA) };
    let dispatcher2: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + B80_DISPATCHER2_RVA) };
    unsafe { dispatcher1(synth_ptr) };
    unsafe { dispatcher2(synth_ptr) };
}

unsafe fn product_continue_autoload_tick(
    owner: usize,
    base: usize,
    gm: usize,
    slot: i32,
    tick: u64,
    ready: &ProductCoreAutoloadReady,
) {
    const PRODUCT_CONTINUE_C30_ZERO: i32 = 0;
    const PRODUCT_CONTINUE_B80_MODAL_WAIT: i32 = 1;
    const PRODUCT_CONTINUE_NEW_GAME_BLOCKED: u8 = 1;
    const PRODUCT_CONTINUE_WAIT_LOG_TICKS: u64 = 30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = FULLREAD_PHASE.load(Ordering::SeqCst);
    let read_i32 = |off: usize| unsafe { safe_read_i32(gm + off) }.unwrap_or(GAME_MAN_C30_UNSET);

    if phase == FULLREAD_PHASE_DONE {
        return;
    }

    if phase == FULLREAD_PHASE_SUBMIT {
        if !unsafe { product_continue_action_ready(ready, base, gm, slot) } {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: Continue submit gated off dialog=0x{:x} menu_latch={} slot={slot} -- semantic menu readiness not stable",
                    ready.title_dialog, ready.menu_opened_latch
                ));
            }
            return;
        }
        let b80_before = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        if b80_before != OWN_STEPPER_B80_IDLE {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native preview/load b80={b80_before} to become idle before Continue row fire -- no SetState5"
                ));
            }
            return;
        }
        let (profile_real, profile_map, profile_level, profile_name_len) =
            unsafe { profile_slot_fingerprint(slot) };
        if !profile_real {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: Continue slot profile is empty-like (slot={slot} map=0x{profile_map:x} level={profile_level} name_len={profile_name_len}); fail-closed with no native Load Game fallback, no legal-popup auto-accept, no Continue submit, and no input"
                ));
            }
            return;
        }
        let Some(action) = (unsafe { product_continue_item_action(base) }) else {
            if tick % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 {
                append_autoload_debug(format_args!(
                    "product-core-autoload: waiting for native Continue MenuWindowJob result after open-menu dialog=0x{:x} slot={slot} -- no direct_load/direct_build/input fallback",
                    ready.title_dialog
                ));
            }
            return;
        };
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = slot };
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        OWN_STEPPER_EXPECTED_SLOT.store(slot, Ordering::SeqCst);
        OWN_STEPPER_CONFIRMED.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
        OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
        OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
        let Some(result_mode) = (unsafe { submit_native_continue_item_action(action, base) })
        else {
            return;
        };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        append_autoload_debug(format_args!(
            "product-core-autoload: *** SUBMITTED native Continue MenuWindowJob result mode={result_mode} submit=0x{:x}(result=0x{:x}, result_vt=0x{:x}, item=0x{:x}, functor=0x{:x}, docall=0x{:x}) after set_save_slot({slot}) b78={b78} ac0={ac0} c30=0x{c30:x} b80={b80} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) dialog=0x{:x} menu_latch={} tick={tick} -- no input/direct_load/direct_build/raw deserialize/direct_confirm ***",
            base + MENU_ITEM_SUBMIT_RVA,
            action.result,
            action.result_vt,
            action.item,
            action.functor,
            action.do_call,
            ready.title_dialog,
            ready.menu_opened_latch
        ));
        timeline_event(
            "T_native_continue_action",
            tick,
            format_args!(
                "slot={slot} item=0x{:x} result=0x{:x} b80={b80}",
                action.item, action.result
            ),
        );
        FULLREAD_DRAIN_WAITS.store(null, Ordering::SeqCst);
        FULLREAD_PHASE.store(FULLREAD_PHASE_GUARD, Ordering::SeqCst);
        return;
    }

    if phase == FULLREAD_PHASE_GUARD {
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let latched = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let deser_ok = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst) == OWN_STEPPER_DESER_FIRED_OK;
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        let slot_identity = unsafe { requested_slot_identity(expected, c30) };
        let waits = FULLREAD_DRAIN_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
        let c30_available =
            c30 == latched && c30 != GAME_MAN_C30_UNSET && c30 != PRODUCT_CONTINUE_C30_ZERO;
        let c30_sane = c30_available && (c30 != GAME_MAN_NEWGAME_DEFAULT_MAP || fp_real);
        let c30_loaded = c30 != GAME_MAN_C30_UNSET && c30 != PRODUCT_CONTINUE_C30_ZERO;
        let c30_loaded_sane = c30_loaded && (c30 != GAME_MAN_NEWGAME_DEFAULT_MAP || fp_real);
        let new_game_flag =
            unsafe { safe_read_usize(owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(PRODUCT_CONTINUE_NEW_GAME_BLOCKED);
        let commit = native_fullread_commit_enabled();
        let b80_idle = b80 == OWN_STEPPER_B80_IDLE;
        let b80_modal_wait = b80 == PRODUCT_CONTINUE_B80_MODAL_WAIT;
        let native_confirmed =
            OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS;
        let modal_disable_ready = commit
            && !native_confirmed
            && b80_modal_wait
            && fp_real
            && slot_identity.matches
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30_loaded_sane
            && new_game_flag == FULLREAD_OWNER_NEW_GAME_OK;
        if modal_disable_ready {
            let shim = &raw mut OWN_STEPPER_SHIM;
            unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner };
            let shim_ptr = shim as usize;
            let confirm: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
            append_autoload_debug(format_args!(
                "product-core-autoload: MODAL-CONFIRM-DISABLED loaded evidence ac0={ac0} expected={expected} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity=true(profile=0x{:x} profile_map=0x{:x} profile_level={} profile_name_len={}) b80={b80} owner+0x284={new_game_flag} -> continue_confirm shim=0x{shim_ptr:x} owner=0x{owner:x} (no confirm input)",
                slot_identity.profile_summary,
                slot_identity.profile_map,
                slot_identity.profile_level,
                slot_identity.profile_name_len
            ));
            timeline_event(
                "T_modal_confirm_disabled",
                tick,
                format_args!("ac0={ac0} c30=0x{c30:x} b80={b80}"),
            );
            unsafe { confirm(shim_ptr) };
            OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "product-core-autoload: STAGE2-SETSTATE5 fired via disabled modal confirm owner=0x{owner:x} -- native pump now streams the real world"
            ));
        }
        let native_confirmed =
            OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS;
        let proceed = commit
            && (deser_ok || modal_disable_ready)
            && native_confirmed
            && fp_real
            && slot_identity.matches
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && (c30_sane || c30_loaded_sane)
            && (b80_idle || modal_disable_ready)
            && new_game_flag == FULLREAD_OWNER_NEW_GAME_OK;
        if waits % PRODUCT_CONTINUE_WAIT_LOG_TICKS == null as u64 || proceed {
            append_autoload_debug(format_args!(
                "product-core-autoload: Continue post-click GUARD waits={waits} commit={commit} deser_ok={deser_ok} native_confirmed={native_confirmed} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} c30_sane={c30_sane} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity={} profile=0x{:x} profile_map=0x{:x} profile_level={} profile_name_len={} pgd_level={} pgd_name_len={} owner+0x284={new_game_flag} b80={b80} proceed={proceed} -- waiting for requested-slot native b80/c30 writer + native continue_confirm/SetState5",
                slot_identity.matches,
                slot_identity.profile_summary,
                slot_identity.profile_map,
                slot_identity.profile_level,
                slot_identity.profile_name_len,
                slot_identity.pgd_level,
                slot_identity.pgd_name_len
            ));
        }
        if !proceed {
            if waits >= FULLREAD_DRAIN_MAX {
                append_autoload_debug(format_args!(
                    "product-core-autoload: Continue post-click GUARD timeout waits={waits} commit={commit} deser_ok={deser_ok} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} c30_sane={c30_sane} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity={} profile=0x{:x} profile_map=0x{:x} profile_level={} profile_name_len={} pgd_level={} pgd_name_len={} owner+0x284={new_game_flag} b80={b80} -- DONE (NO SetState5)",
                    slot_identity.matches,
                    slot_identity.profile_summary,
                    slot_identity.profile_map,
                    slot_identity.profile_level,
                    slot_identity.profile_name_len,
                    slot_identity.pgd_level,
                    slot_identity.pgd_name_len
                ));
                FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        append_autoload_debug(format_args!(
            "product-core-autoload: STAGE2-MOUNT-COMMIT native Continue row guard pass ac0={ac0} expected={expected} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) slot_identity=true owner+0x284={new_game_flag} b80={b80} -- native continue_confirm/SetState5 already fired"
        ));
        timeline_event("T_playgame", tick, format_args!("ac0={ac0} c30=0x{c30:x}"));
        FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}

unsafe fn fire_product_title_load_action(
    action: MenuActionNode,
    base: usize,
    tick: u64,
    slot: i32,
) {
    if OWN_STEPPER_TITLE_FIRED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let node = action.node;
    let node_vt = action.node_vt;
    let member_dialog = action.member_dialog;
    let member_fn = action.member_fn;
    let member_adjust = action.member_adjust;
    let window_item = action.window_item;
    OWN_STEPPER_EXPECTED_SLOT.store(slot, Ordering::SeqCst);
    OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
    OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
    OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
    OWN_STEPPER_DIALOG.store(null, Ordering::SeqCst);
    OWN_STEPPER_SELECTOR_STEP.store(null, Ordering::SeqCst);
    OWN_STEPPER_SELECTOR_CTX.store(null, Ordering::SeqCst);
    reset_phase_timer(&OWN_STEPPER_S2_PHASE_STARTED_MS);
    let run: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(usize)>(
            base + MENU_MEMBER_FUNC_JOB_RUN_RVA,
        )
    };
    append_autoload_debug(format_args!(
        "product-core-autoload: *** FIRING native TitleTopDialog Load-Game run 0x{:x}(rcx=node=0x{node:x}) vt=0x{node_vt:x} member_dialog=0x{member_dialog:x} member_fn=0x{member_fn:x} member_adjust=0x{member_adjust:x} window_item=0x{window_item:x} slot={slot} tick={tick} -- no direct_build/forged ctx ***",
        base + MENU_MEMBER_FUNC_JOB_RUN_RVA
    ));
    timeline_event(
        "T_native_load_action",
        tick,
        format_args!("node=0x{node:x} member_fn=0x{member_fn:x}"),
    );
    unsafe { run(node) };
    append_autoload_debug(format_args!(
        "product-core-autoload: native TitleTopDialog Load-Game run returned; waiting for ProfileLoadDialog factory hook capture"
    ));
}

unsafe fn title_menu_action_ready(owner: usize, base: usize) -> Option<MenuActionNode> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if dialog == null {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let registry =
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if !vtable_in_game_image(registry, base) {
        return None;
    }
    let (member_node, window_item) = unsafe { scan_dialog_for_loadgame(owner, base) };
    let node = member_node?;
    let node_vt = unsafe { safe_read_usize(node) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return None;
    }
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let member_dialog = unsafe { safe_read_usize(node + MEMBER_DIALOG_10) }.unwrap_or(null);
    let member_fn = unsafe { safe_read_usize(node + MEMBER_FN_18) }.unwrap_or(null);
    let member_adjust = unsafe { safe_read_usize(node + MEMBER_ADJ_20) }.unwrap_or(null);
    if member_fn == null {
        return None;
    }
    let factory_abs = base + LIVE_DIALOG_FACTORY_RVA;
    let mut target = member_fn;
    let mut hop = HOP_START;
    while hop < JMP_HOPS && target != null {
        if target == factory_abs {
            return Some(MenuActionNode {
                node,
                node_vt,
                registry,
                member_dialog,
                member_fn,
                member_adjust,
                window_item: window_item.unwrap_or(null),
            });
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += HOP_STEP;
    }
    None
}

unsafe fn title_live_dialog_fire_ready(owner: usize, base: usize) -> Option<LiveDialogFireReady> {
    const TITLE_FLOW_CONTEXT_VTABLE_RVA: usize = 0x2ac7f20;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if !unsafe { title_scheduler_ready(owner, base) } {
        return None;
    }
    let title_dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if title_dialog == null {
        return None;
    }
    let title_dialog_vt = unsafe { safe_read_usize(title_dialog) }.unwrap_or(null);
    if title_dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let menu_opened_latch = unsafe {
        safe_read_usize(title_dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET)
            .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
            .unwrap_or(null)
    };
    if menu_opened_latch == OWN_STEPPER_MENU_OPENED_NO {
        return None;
    }
    let registry_vt =
        unsafe { safe_read_usize(title_dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if registry_vt != base + SCENE_OBJ_PROXY_VTABLE_RVA {
        return None;
    }
    let capture_slot = title_dialog + DIALOG_SCENE_PROXY_CAPTURE_A38_OFFSET;
    let capture = unsafe { safe_read_usize(capture_slot) }.unwrap_or(null);
    if !unsafe { is_heap_aligned_ptr(capture) } {
        return None;
    }
    let capture_vt = unsafe { safe_read_usize(capture) }.unwrap_or(null);
    if capture_vt != base + TITLE_FLOW_CONTEXT_VTABLE_RVA {
        return None;
    }
    let menu_window = LATCHED_MENU_WINDOW.load(Ordering::SeqCst);
    if !unsafe { is_heap_aligned_ptr(menu_window) } {
        return None;
    }
    let menu_window_vt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
    Some(LiveDialogFireReady {
        title_dialog,
        title_dialog_vt,
        capture_slot,
        capture,
        capture_vt,
        registry_vt,
        menu_opened_latch,
        menu_window,
        menu_window_vt,
    })
}

/// True if `vt` is a startup MessageBoxDialog the auto-accept should drive: the base MessageBoxDialog
/// vtable OR the CS::SaveRetryDialog subclass vtable (the wrapper 0x1407af9a0 overrides base ->
/// SaveRetryDialog AFTER the builder, so a base-only check bails once the override lands). bd
/// offline-title-modal-is-saveretrydialog.
fn is_startup_msgbox_vtable(vt: usize, base: usize) -> bool {
    vt == base + MSGBOX_DIALOG_VTABLE_RVA || vt == base + SAVE_RETRY_DIALOG_VTABLE_RVA
}

fn startup_modal_blocking_state() -> StartupModalBlockingState {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        return StartupModalBlockingState::Clear;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != null {
            own
        } else {
            game_module_base().unwrap_or(null)
        }
    };
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if base == null || !is_startup_msgbox_vtable(vt, base) {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return StartupModalBlockingState::Clear;
    }
    let closing = unsafe { safe_read_usize(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }
        .map(|v| v & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES);
    if closing == MSGBOX_CLOSING_YES {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return StartupModalBlockingState::Clear;
    }
    StartupModalBlockingState::Blocking {
        dialog,
        vtable: vt,
        closing_latch: closing,
    }
}

unsafe fn profile_load_dialog_ready(
    base: usize,
    dialog: usize,
    want_slot: i32,
    log_pending: bool,
) -> Option<ProfileLoadDialogReady> {
    const PROFILE_LOAD_ACTIVATE_RVA: usize = 0x009a4670;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    let dvt = if dialog != null {
        unsafe { safe_read_usize(dialog) }.unwrap_or(null)
    } else {
        null
    };
    if dvt != pld_vt {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: waiting for ProfileLoadDialog dialog=0x{dialog:x} vt=0x{dvt:x} want=0x{pld_vt:x}"
            ));
        }
        return None;
    }
    let lav =
        unsafe { safe_read_usize(dvt + DIALOG_LOAD_ACTIVATE_VTSLOT_A0_OFFSET) }.unwrap_or(null);
    if lav != base + PROFILE_LOAD_ACTIVATE_RVA {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_activate slot not ready lav=0x{lav:x} want=0x{:x} dvt=0x{dvt:x}",
                base + PROFILE_LOAD_ACTIVATE_RVA
            ));
        }
        return None;
    }
    let gdm = game_data_man_ptr_or_null();
    let player_game_data = if gdm != null {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(null)
    } else {
        null
    };
    if player_game_data == null {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: PlayerGameData null gdm=0x{gdm:x} -- load_activate would assert"
            ));
        }
        return None;
    }
    let bound = unsafe { safe_read_i32(dialog + DIALOG_SLOT_BOUND_B08_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let cursor_now = unsafe { safe_read_i32(dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) }
        .unwrap_or(OWN_STEPPER_SLOT_NONE);
    let expected_slot = if want_slot == OWN_STEPPER_SLOT_NONE {
        cursor_now
    } else {
        want_slot
    };
    let cursor_target = if want_slot == OWN_STEPPER_SLOT_NONE {
        cursor_now
    } else if bound == OWN_STEPPER_CALL_INC as i32 {
        OWN_STEPPER_SLOT_ZERO
    } else {
        want_slot
    };
    if expected_slot < OWN_STEPPER_SLOT_ZERO
        || bound <= OWN_STEPPER_SLOT_ZERO
        || cursor_target < OWN_STEPPER_SLOT_ZERO
        || cursor_target >= bound
    {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: slot rows not ready/valid want={want_slot} expected={expected_slot} cursor_target={cursor_target} cursor={cursor_now} bound={bound} dialog=0x{dialog:x}"
            ));
        }
        return None;
    }
    let load_job_ctx = unsafe {
        safe_read_usize(dialog + core::mem::offset_of!(ProfileLoadDialogLayout, load_job_ctx))
    }
    .unwrap_or(null);
    if !unsafe { is_heap_aligned_ptr(load_job_ctx) } {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_job_ctx not ready dialog=0x{dialog:x} ctx=0x{load_job_ctx:x}"
            ));
        }
        return None;
    }
    let load_job_ctx_vt = unsafe { safe_read_usize(load_job_ctx) }.unwrap_or(null);
    if !vtable_in_game_image(load_job_ctx_vt, base) {
        if log_pending {
            append_autoload_debug(format_args!(
                "profile-load-ready: load_job_ctx vtable invalid ctx=0x{load_job_ctx:x} vt=0x{load_job_ctx_vt:x} base=0x{base:x}"
            ));
        }
        return None;
    }
    Some(ProfileLoadDialogReady {
        dialog,
        dvt,
        bound,
        cursor_now,
        cursor_target,
        expected_slot,
        load_activate: lav,
        load_job_ctx,
        load_job_ctx_vt,
        player_game_data,
    })
}

/// MODEL B orchestrator (gated by live_dialog_enabled(), OFF by default). At the rendered title
/// menu: (1) do the wall-clock-bounded active-screen scan to acquire the live TitleTopDialog* +
/// MenuWindow*, (2) call the dialog factory 0x14081ead0(rcx=title_dialog+0xa38, rdx=menu_window)
/// ONCE -- which builds + registers the LIVE ProfileLoadDialog into the active-screen set, then (3)
/// wait for that ProfileLoadDialog (vtable 0x142b229f8) to appear in the active-screen array, latch
/// it as OWN_STEPPER_DIALOG, and hand it to STAGE2 ACTIVATE (which fires load_activate -> native pump
/// mount -> guarded, char-fingerprint-gated continue_confirm). One-shot fire latch; bounded wait.
/// FAIL-CLOSED at every step (no acquisition -> stay; bad vtable -> no call; dialog not live yet ->
/// wait then DONE on timeout). The forge path is untouched.
unsafe fn own_stepper_live_dialog_fire(
    owner: usize,
    base: usize,
    waits: u64,
    timed_out: bool,
    elapsed_ms: u64,
) {
    // FIX 2 (probe-6): the factory 0x14081ead0 RETURNS the new dialog in rax. fire_live_loadgame_node
    // validates that return == ProfileLoadDialog (vt 0x142b229f8) and, on a match, stores it as
    // OWN_STEPPER_DIALOG + transitions own_stepper to STAGE2 ACTIVATE on THAT pointer. We no longer
    // scan the active-screen array 0x143d6d8d0 here (probe-2 proved it holds MODEL-RENDERERS, never
    // the PLD -> it would never confirm). Once fired+verified the orchestrator routes to STAGE2.
    if OWN_STEPPER_LIVE_FIRED.load(Ordering::SeqCst) == OWN_STEPPER_LIVE_FIRED_NO {
        let Some(ready) = (unsafe { title_live_dialog_fire_ready(owner, base) }) else {
            if timed_out {
                append_autoload_debug(format_args!(
                    "live-dialog: factory args never became semantically ready after {waits} polls/{elapsed_ms}ms -- STAY at menu (NO-WRITE), DONE"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        };
        append_autoload_debug(format_args!(
            "live-dialog: factory args ready title_dialog=0x{:x} vt=0x{:x} capture_slot=0x{:x} capture=0x{:x} capture_vt=0x{:x} registry_vt=0x{:x} latch={} menu_window=0x{:x} menu_window_vt=0x{:x} -- firing live factory",
            ready.title_dialog,
            ready.title_dialog_vt,
            ready.capture_slot,
            ready.capture,
            ready.capture_vt,
            ready.registry_vt,
            ready.menu_opened_latch,
            ready.menu_window,
            ready.menu_window_vt
        ));
        // fire_live_loadgame_node returns true ONLY when the factory returned a verified
        // ProfileLoadDialog (it has already stored it + set STAGE2 ACTIVATE on success).
        if unsafe { fire_live_loadgame_node(ready.title_dialog, ready.menu_window, base, true) } {
            OWN_STEPPER_LIVE_FIRED.store(OWN_STEPPER_LIVE_FIRED_YES, Ordering::SeqCst);
        } else if timed_out {
            append_autoload_debug(format_args!(
                "live-dialog: factory returned non-PLD (or fail-closed) after {waits} polls/{elapsed_ms}ms -- STAY at menu (NO-WRITE), DONE"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
    // Fired + verified: own_stepper is already in STAGE2 ACTIVATE driving the returned PLD. If we are
    // somehow still here (phase not advanced), bound the wait and stop without writing.
    if timed_out {
        append_autoload_debug(format_args!(
            "live-dialog: fired factory but STAGE2 did not advance after {waits} polls/{elapsed_ms}ms -- STAY (NO-WRITE), DONE"
        ));
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}

/// Fire a captured MenuWindowJob's `+0xa8` action std::function in-context, mirroring the
/// native leaf Update's functor-invoke at `0x1407ad2b9`:
///   rcx = `[item+0xa8]` (the std::function obj); rax = `[rcx]` (`_Func_impl_no_alloc`
///   vtable, no RTTI); rdx = `item+0x10` (the dialog ctx out-slot, the single arg);
///   call `[rax+0x10]` (`_Do_call`: `add rcx,8; jmp <lambda>`).
/// Returns the lambda result (e.g. the built dialog), which the native Update stores to
/// `[item+0x130]`. Guarded EXACTLY like the native BUILD path: only fires when
/// `[item+0xa8]!=0` AND `[item+0x10]==0`, so we never re-invoke an already-built item
/// (which would leak/overwrite `item+0x130`). This is the game's OWN menu-action functor
/// (NOT input synthesis) -- compliant with the zero-input standard. NOTE: this performs a
/// native call, so it is only used once the live item/owner are validated; it is NOT a
/// save-write by itself (the Load-entry/dialog functors build UI, not save state).
unsafe fn invoke_menu_item_functor(item: usize) -> Option<usize> {
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const ITEM_CTX_10: usize = 0x10;
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }?;
    if functor == null {
        return None;
    }
    // BUILD-path precondition: the native Update fires the functor only when item+0x10==0.
    let ctx_slot = unsafe { safe_read_usize(item + ITEM_CTX_10) }?;
    if ctx_slot != null {
        return None;
    }
    let functor_vtable = unsafe { safe_read_usize(functor) }?;
    if functor_vtable == null {
        return None;
    }
    let do_call = unsafe { safe_read_usize(functor_vtable + DOCALL_VTABLE_SLOT_10) }?;
    if do_call == null {
        return None;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize =
        unsafe { std::mem::transmute(do_call) };
    let ctx_out = item + ITEM_CTX_10;
    Some(unsafe { f(functor, ctx_out) })
}

/// Drive the NATIVE MenuWindowJob::Update 0x1407ad1c0(rcx=item, rdx=&out, r8=framectx) once to
/// BUILD the item's dialog the way the game does. Unlike a bare functor invoke, the native Update
/// WIRES the ctx (item+0x10) from the descriptor (item+0x58 -> resolved window item+0x68 via
/// 0x140d6a8e0 + window-mgr 0x143d83148) BEFORE firing the functor -- so it needs NO synthetic ctx
/// (the prior wall). It is idempotent (returns early if item+0x130 already holds a dialog) and the
/// Load-Game item only builds a ProfileLoadDialog -> BUILD-ONLY, no save write. Guarded by the
/// native BUILD precondition (mirrors 0x1407ad1ec/1fa/208): [item+0x130]==0 && [item+0xa8]!=0 &&
/// [item+0x10]==0. `framectx` is the live FD4Time passed to our idx10 step (the same ctx the native
/// pump feeds the leaf). Returns the built dialog at [item+0x130], if any.
unsafe fn drive_menu_item_update(item: usize, base: usize, framectx: usize) -> Option<usize> {
    const ITEM_FUNCTOR_A8: usize = MENU_ITEM_FUNCTOR_A8_OFFSET;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const OUT_ZERO: u64 = 0;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let functor = unsafe { safe_read_usize(item + ITEM_FUNCTOR_A8) }?;
    let ctx = unsafe { safe_read_usize(item + ITEM_CTX_10) }?;
    let pre130 = unsafe { safe_read_usize(item + ITEM_RESULT_130) }?;
    // Native BUILD precondition: dialog not yet built, functor present, ctx not yet wired.
    if functor == null || ctx != null || pre130 != null {
        return None;
    }
    let update: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(base + MENU_ITEM_UPDATE_RVA as usize) };
    // 16-byte writable StepResult out-slot ([0]=status, [4]=payload) the leaf Update writes.
    let mut out = [OUT_ZERO, OUT_ZERO];
    let _ = unsafe { update(item, out.as_mut_ptr() as usize, framectx) };
    let _ = &out;
    unsafe { safe_read_usize(item + ITEM_RESULT_130) }.filter(|&d| d != null)
}

/// Decode a single-child FD4 job decorator's forwarded-child offset from its Update fn
/// prologue. Every decorator in the owner+0x130 menu chain forwards Update to one wrapped
/// child via `mov rcx,[node+disp]; mov rax,[rcx]; call [rax+0x10]`, but the child offset
/// varies per type (0x48, 0x40, ...). Rather than tabulate each, we read the Update fn's
/// first bytes and return the disp of the FIRST `mov rcx,[rcx+disp]`:
///   `48 8b 49 <disp8>`              -> disp8
///   `48 8b 89 <disp32 le>`          -> disp32
/// Returns None if no such load appears in the scanned prologue (not a forwarding decorator).
/// Pure code read via `safe_read_usize`; never faults.
unsafe fn decorator_child_offset(update_fn: usize) -> Option<usize> {
    const SCAN_LEN: usize = 0x28;
    const REXW: usize = 0x48;
    const MOV_RM_OPCODE: usize = 0x8b;
    const MODRM_RCX_RCX_DISP8: usize = 0x49;
    const MODRM_RCX_RCX_DISP32: usize = 0x89;
    const BYTE_MASK: usize = 0xff;
    const B1_SHIFT: usize = 8;
    const B2_SHIFT: usize = 16;
    const B3_SHIFT: usize = 24;
    const DISP32_LEN: usize = 4;
    // bytes consumed by `48 8b 89` before the disp32 immediate begins.
    const DISP32_PREFIX_LEN: usize = 3;
    const SCAN_START: usize = 0;
    const SCAN_STEP: usize = 1;
    let mut i = SCAN_START;
    while i < SCAN_LEN {
        let word = unsafe { safe_read_usize(update_fn + i) }?;
        let b0 = word & BYTE_MASK;
        let b1 = (word >> B1_SHIFT) & BYTE_MASK;
        let b2 = (word >> B2_SHIFT) & BYTE_MASK;
        let b3 = (word >> B3_SHIFT) & BYTE_MASK;
        if b0 == REXW && b1 == MOV_RM_OPCODE {
            if b2 == MODRM_RCX_RCX_DISP8 {
                return Some(b3);
            }
            if b2 == MODRM_RCX_RCX_DISP32 {
                let mut disp = SCAN_START;
                let mut k = SCAN_START;
                while k < DISP32_LEN {
                    let byte = unsafe { safe_read_usize(update_fn + i + DISP32_PREFIX_LEN + k) }?
                        & BYTE_MASK;
                    disp |= byte << (k * B1_SHIFT);
                    k += SCAN_STEP;
                }
                return Some(disp);
            }
        }
        i += SCAN_STEP;
    }
    None
}

/// STAGE 1b (strictly NO-WRITE): recursive bounded walk of the title menu JOB tree rooted
/// at `[owner+0xe0]` (the FD4 multicast/job holder -- runtime proved the real menu lives
/// here, NOT the empty `owner+0x138`). Classifies each node by its Update slot
/// `[vtable+0x10]`: 0x1407aa1f0 = Sequence/IfElse container (children at `[node+0x18]` base,
/// count `[node+0x60]`, stride 8), 0x1407ad1c0 = MenuWindowJob leaf (action functor
/// `[node+0xa8]`). Logs the structure and returns the Load-Game leaf (functor -> dialog
/// factory). Both child-pointer interpretations (base-deref and inline) are enqueued; a
/// visited-set + node/depth caps bound it; fault-tolerant reads never AV. NO writes/calls.
unsafe fn diagnostic_job_tree_walk(
    owner: usize,
    module_base: usize,
    holder_offset: usize,
    tag: &str,
    verbose: bool,
) -> Option<usize> {
    const VTABLE_UPDATE_SLOT_10: usize = 0x10;
    const NODE_CHILDREN_BASE_18: usize = 0x18;
    const NODE_COUNT_60: usize = 0x60;
    const NODE_HOLDER_ROOT_18: usize = 0x18;
    const SEQ_UPDATE_RVA: usize = 0x07aa1f0;
    const LEAF_UPDATE_RVA: usize = 0x07ad1c0;
    // IfElseJob combiner (vt 0x142aa2c38). Its child jobs are NOT at the sequence
    // [+0x18]/[+0x60] layout; that mis-read is the "garbage count" the generic walk hit.
    // Decoded from selector 0x140793390: inline entry array at [node+0x18], stride 0x10,
    // each entry = {predicate@+0, child_job@+0x8}; entry count at [node+0xa0]; default/else
    // child at [node+0xa8]; runtime-active child at [node+0xb0]. Entry + default child jobs
    // are pre-built/retained at BUILD time, so reading them needs no pump.
    const IFELSE_UPDATE_RVA: usize = 0x07931e0;
    // Single-child wrapper (vt 0x142a93af8, update 0x140745510): `mov rcx,[node+0x48];
    // call [rcx]->vt[+0x10]` -- forwards Update to one wrapped child at [node+0x48]. The
    // IfElseJob entry child jobs are these wrappers, not MenuWindowJobs directly.
    const WRAP_UPDATE_RVA: usize = 0x0745510;
    const WRAP_CHILD_48: usize = 0x48;
    const IFELSE_ENTRY_STRIDE_10: usize = 0x10;
    const IFELSE_ENTRY_JOB_8: usize = 0x8;
    const IFELSE_COUNT_A0: usize = 0xa0;
    const IFELSE_DEFAULT_A8: usize = 0xa8;
    const IFELSE_ACTIVE_B0: usize = 0xb0;
    const ITEM_CTX_10: usize = 0x10;
    const ITEM_RESULT_130: usize = 0x130;
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const COUNT_MIN: usize = 1;
    const COUNT_MAX: usize = 32;
    const MAX_NODES: usize = 256;
    const MAX_DEPTH: usize = 8;
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    // Generic decorator descent. The owner+0x130 menu tree threads d180 through a chain of
    // single-child FD4 job decorators (vt 0x142a93af8 child@+0x48, vt 0x142a93d18 child@+0x40,
    // ...) with per-type child offsets. Rather than decode each, for any node that is none of
    // the known container/leaf kinds we scan a bounded field window and enqueue every qword
    // that points at an in-module job object (its vtable AND that vtable's Update slot both
    // land inside the game image). Fault-tolerant reads; visited-set + node budget bound it.
    const GEN_SCAN_LO: usize = 0x10;
    const GEN_SCAN_HI: usize = 0xc0;
    // PE image bounds (for the in-module pointer test): SizeOfImage at NT+0x50, e_lfanew at
    // base+0x3c. Both are u32; mask the low dword off the qword read.
    const PE_E_LFANEW_OFFSET: usize = 0x3c;
    const PE_SIZE_OF_IMAGE_FROM_NT: usize = 0x50;
    const PE_U32_MASK: usize = 0xffffffff;
    const MODULE_SPAN_FALLBACK: usize = 0x3000000;
    const MODULE_MIN_OFFSET: usize = 0x1000;

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let seq_update_abs = module_base + SEQ_UPDATE_RVA;
    let leaf_update_abs = module_base + LEAF_UPDATE_RVA;
    let ifelse_update_abs = module_base + IFELSE_UPDATE_RVA;
    let wrap_update_abs = module_base + WRAP_UPDATE_RVA;

    let e_lfanew = unsafe { safe_read_usize(module_base + PE_E_LFANEW_OFFSET) }
        .map(|v| v & PE_U32_MASK)
        .unwrap_or(null);
    let image_span = if e_lfanew != null {
        unsafe { safe_read_usize(module_base + e_lfanew + PE_SIZE_OF_IMAGE_FROM_NT) }
            .map(|v| v & PE_U32_MASK)
            .filter(|&s| s != null)
            .unwrap_or(MODULE_SPAN_FALLBACK)
    } else {
        MODULE_SPAN_FALLBACK
    };
    let module_lo = module_base + MODULE_MIN_OFFSET;
    let module_hi = module_base + image_span;
    let in_module = |p: usize| p >= module_lo && p < module_hi;

    let holder = unsafe { safe_read_usize(owner + holder_offset) }.unwrap_or(null);
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}]: owner=0x{owner:x} holder(owner+0x{holder_offset:x})=0x{holder:x} seq_update=0x{seq_update_abs:x} leaf_update=0x{leaf_update_abs:x}"
        ));
    }
    if holder == null {
        return None;
    }
    let root = unsafe { safe_read_usize(holder + NODE_HOLDER_ROOT_18) }.unwrap_or(null);

    let mut load_game: Option<usize> = None;
    let mut visited: Vec<usize> = Vec::new();
    let mut stack: Vec<(usize, usize)> = Vec::new();
    stack.push((holder, WALK_START));
    if root != null {
        stack.push((root, WALK_START));
    }
    let mut node_budget = MAX_NODES;
    while let Some((node, depth)) = stack.pop() {
        if node_budget == WALK_START {
            break;
        }
        node_budget -= WALK_STEP;
        if node == null || visited.contains(&node) {
            continue;
        }
        visited.push(node);
        let vtable = unsafe { safe_read_usize(node) }.unwrap_or(null);
        let update = if vtable != null {
            unsafe { safe_read_usize(vtable + VTABLE_UPDATE_SLOT_10) }.unwrap_or(null)
        } else {
            null
        };
        let count = unsafe { safe_read_usize(node + NODE_COUNT_60) }.unwrap_or(null);
        let base = unsafe { safe_read_usize(node + NODE_CHILDREN_BASE_18) }.unwrap_or(null);
        let is_leaf = update == leaf_update_abs;
        let is_container = update == seq_update_abs;
        let is_ifelse = update == ifelse_update_abs;
        let is_wrap = update == wrap_update_abs;
        let wrap_child = unsafe { safe_read_usize(node + WRAP_CHILD_48) }.unwrap_or(null);
        let ife_count = unsafe { safe_read_usize(node + IFELSE_COUNT_A0) }.unwrap_or(null);
        let ife_default = unsafe { safe_read_usize(node + IFELSE_DEFAULT_A8) }.unwrap_or(null);
        let ife_active = unsafe { safe_read_usize(node + IFELSE_ACTIVE_B0) }.unwrap_or(null);
        let mut chain = String::new();
        let is_load_game = if update != null {
            unsafe { functor_chain_hits_factory(node, module_base, &mut chain) }
        } else {
            false
        };
        if is_load_game && load_game.is_none() {
            load_game = Some(node);
        }
        let ctx = unsafe { safe_read_usize(node + ITEM_CTX_10) }.unwrap_or(null);
        let result = unsafe { safe_read_usize(node + ITEM_RESULT_130) }.unwrap_or(null);
        if verbose {
            append_autoload_debug(format_args!(
                "job-tree[{tag}] d={depth} node=0x{node:x} vt=0x{vtable:x} update=0x{update:x} leaf={is_leaf} container={is_container} ifelse={is_ifelse} wrap={is_wrap} count=0x{count:x} base=0x{base:x} ife_count=0x{ife_count:x} ife_default=0x{ife_default:x} ife_active=0x{ife_active:x} wrap_child=0x{wrap_child:x} ctx=0x{ctx:x} result=0x{result:x} {chain} LOAD_GAME={is_load_game}"
            ));
        }
        if depth < MAX_DEPTH && is_wrap {
            // Single-child wrapper: descend into its one forwarded child.
            if wrap_child != null {
                stack.push((wrap_child, depth + WALK_STEP));
            }
        } else if depth < MAX_DEPTH && is_ifelse {
            // IfElseJob (selector 0x140793390): a case vector at [node+0x18], stride 0x10, each
            // case = {predicate@+0, child_job@+0x8}; the main-menu branch (holding d180) binds its
            // child to [node+0xb0] ONLY when its input-gated predicate flips (so headless d180 is
            // present-but-unbound). The case COUNT offset is ambiguous across memos (+0xa0 vs +0x88
            // = capacity vs size), so rather than trust a count we do a bounded LAYOUT-AGNOSTIC
            // scan of the case slots and enqueue every child_job (and predicate slot) that points
            // at an in-module job object -- this reaches d180's case child whether or not its
            // branch is bound, with no pump. Pure reads; visited-set + node budget bound it.
            let _ = (ife_count, IFELSE_COUNT_A0, COUNT_MIN, IFELSE_ENTRY_JOB_8);
            let mut i = WALK_START;
            while i < COUNT_MAX {
                let case = node + NODE_CHILDREN_BASE_18 + i * IFELSE_ENTRY_STRIDE_10;
                for slot in [WALK_START, IFELSE_ENTRY_JOB_8] {
                    let child = unsafe { safe_read_usize(case + slot) }.unwrap_or(null);
                    if child != null && child != node {
                        let cvt = unsafe { safe_read_usize(child) }.unwrap_or(null);
                        if in_module(cvt) {
                            stack.push((child, depth + WALK_STEP));
                        }
                    }
                }
                i += WALK_STEP;
            }
            if ife_default != null {
                stack.push((ife_default, depth + WALK_STEP));
            }
            if ife_active != null && ife_active != ife_default {
                stack.push((ife_active, depth + WALK_STEP));
            }
        } else if depth < MAX_DEPTH && is_container && (COUNT_MIN..=COUNT_MAX).contains(&count) {
            let mut i = WALK_START;
            while i < count {
                let child_b = if base != null {
                    unsafe { safe_read_usize(base + i * PTR_STRIDE) }.unwrap_or(null)
                } else {
                    null
                };
                let child_i =
                    unsafe { safe_read_usize(node + NODE_CHILDREN_BASE_18 + i * PTR_STRIDE) }
                        .unwrap_or(null);
                if child_b != null {
                    stack.push((child_b, depth + WALK_STEP));
                }
                if child_i != null && child_i != child_b {
                    stack.push((child_i, depth + WALK_STEP));
                }
                i += WALK_STEP;
            }
        } else if depth < MAX_DEPTH && !is_leaf && in_module(vtable) && in_module(update) {
            // Unknown FD4 decorator: decode the single forwarded-child offset from its Update
            // prologue (`mov rcx,[node+disp]`) and descend into [node+disp] ONLY -- a precise
            // single-child follow, never a field scan (which wandered into the GUI graph).
            if let Some(off) = unsafe { decorator_child_offset(update) } {
                if (GEN_SCAN_LO..=GEN_SCAN_HI).contains(&off) {
                    let child = unsafe { safe_read_usize(node + off) }.unwrap_or(null);
                    if child != null && child != node {
                        let cvt = unsafe { safe_read_usize(child) }.unwrap_or(null);
                        if in_module(cvt) {
                            stack.push((child, depth + WALK_STEP));
                        }
                    }
                }
            }
        }
    }
    if verbose {
        append_autoload_debug(format_args!(
            "job-tree[{tag}] summary: nodes_visited={} load_game=0x{:x}",
            visited.len(),
            load_game.unwrap_or(null)
        ));
    }
    load_game
}

/// DETERMINISTIC MENU INPUT PROBE driver. Runs each frame (in PHASE_MENU_BUILD, after the menu is
/// open) when `input_probe_enabled()`. Schedule (probe-frame `f`, see lib.rs consts):
///   [0, DOWN_START)                 SETTLE   -- baseline, no input (rows empty headless?)
///   [DOWN_START, +DOWN_TAP_FRAMES)  DOWN     -- inject one Down (Continue->Load Game)
///   [DOWN_START, CONFIRM_START)     HIGHLIGHT-- NO input; watch MENU_D180_LEAF_TICKED grow?
///   [CONFIRM_START, +CONFIRM_TAP)   CONFIRM  -- inject Confirm; native load fires (captured)
/// The decisive signal is whether the genuine d180 leaf-Update tick count grows during HIGHLIGHT
/// (before Confirm). Pure reads + the two keystate-bit writes; no SetState here (the Confirm drives
/// the native load). `dump_titletop_menu_entries` logs the live router_this row vector each interval.
unsafe fn menu_input_probe(owner: usize, base: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    INPUT_PROBE_ACTIVE.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    let inputmgr =
        unsafe { safe_read_usize(base + SELECTBOT_INPUT_MANAGER_GLOBAL_RVA) }.unwrap_or(NULL);
    let f = INPUT_PROBE_FRAME.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let item = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
    let leaf_ticks = MENU_D180_LEAF_TICKED.load(Ordering::SeqCst);

    let in_down =
        f >= INPUT_PROBE_DOWN_START && f < INPUT_PROBE_DOWN_START + INPUT_PROBE_DOWN_TAP_FRAMES;
    let in_highlight = f >= INPUT_PROBE_DOWN_START && f < INPUT_PROBE_CONFIRM_START;
    let in_confirm = f >= INPUT_PROBE_CONFIRM_START
        && f < INPUT_PROBE_CONFIRM_START + INPUT_PROBE_CONFIRM_TAP_FRAMES;

    if inputmgr != NULL {
        if in_down {
            // Inject BOTH vertical-move events (one is Down, one Up; Up saturates at the top so
            // from Continue only Down moves -> lands on Load Game). Edge-triggered &1.
            unsafe {
                *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_MOVE_A_00) as *mut u8) |=
                    MENU_EVENT_PRESSED_BIT;
                *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_MOVE_B_45) as *mut u8) |=
                    MENU_EVENT_PRESSED_BIT;
            }
        }
        if in_confirm {
            unsafe {
                *((inputmgr + INPUTMGR_BITMAP_90_OFFSET + MENU_EVENT_CONFIRM_3D) as *mut u8) |=
                    MENU_EVENT_PRESSED_BIT;
            }
        }
    }

    // DECISIVE one-shot: d180's leaf Update ticked during the highlight window (after Down, before
    // Confirm). Snapshot taken at DOWN_START; any growth here means highlight ALONE ticks d180.
    if in_highlight
        && leaf_ticks > INPUT_PROBE_DOWN_LEAF_BASELINE.load(Ordering::SeqCst)
        && INPUT_PROBE_D180_PRECONFIRM.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == NULL
    {
        let (l, c, cur) = unsafe { dump_titletop_menu_entries(owner, base) };
        append_autoload_debug(format_args!(
            "INPUT-PROBE: *** d180 LEAF-TICKED during HIGHLIGHT (pre-confirm) f={f} ticks={leaf_ticks} item=0x{item:x} cursor={cur} load_entry=0x{:x} cont_entry=0x{:x} *** -> highlight ALONE ticks d180; zero-input functor-invoke route VIABLE",
            l.unwrap_or(NULL),
            c.unwrap_or(NULL)
        ));
    }

    if f == INPUT_PROBE_DOWN_START {
        // Latch the leaf-tick baseline at the moment Down begins, so HIGHLIGHT growth is measured
        // strictly from here (ignores any pre-Down ticks).
        INPUT_PROBE_DOWN_LEAF_BASELINE.store(leaf_ticks, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "INPUT-PROBE: DOWN inject f={f} inputmgr=0x{inputmgr:x} leaf_baseline={leaf_ticks} -- highlight window [{}..{}) before Confirm",
            INPUT_PROBE_DOWN_START, INPUT_PROBE_CONFIRM_START
        ));
    }
    if f == INPUT_PROBE_CONFIRM_START {
        let pre = INPUT_PROBE_D180_PRECONFIRM.load(Ordering::SeqCst) != NULL;
        append_autoload_debug(format_args!(
            "INPUT-PROBE: CONFIRM inject f={f} d180_leaf_ticked_on_highlight={pre} ticks_now={leaf_ticks} -- {} (load now fires via Confirm)",
            if pre {
                "highlight WAS sufficient"
            } else {
                "highlight did NOT tick d180 -> needs static walk / focus is required"
            }
        ));
    }
    if f % INPUT_PROBE_LOG_INTERVAL == NULL as u64 {
        let phase = if in_down {
            "DOWN"
        } else if in_confirm {
            "CONFIRM"
        } else if in_highlight {
            "HIGHLIGHT"
        } else if f < INPUT_PROBE_DOWN_START {
            "SETTLE"
        } else {
            "POST"
        };
        append_autoload_debug(format_args!(
            "INPUT-PROBE: f={f} phase={phase} d180_item=0x{item:x} leaf_ticks={leaf_ticks}"
        ));
        let _ = unsafe { dump_titletop_menu_entries(owner, base) };
    }
}

/// OBSERVE-ONLY NATIVE-LOAD tick (native_load_enabled(), gated OFF by default). Runs each frame
/// INSTEAD of the own_stepper forcing logic, then the caller pass-throughs to OWN_STEPPER_ORIG_IDX10
/// so the NATIVE title machine advances untouched (the user drives past press-any-button + modals).
/// KEEP vs the normal own_stepper: it does NOT SetState(owner,2/3), does NOT clear the beginlogo
/// gate, does NOT self-fire the registrar 0x1409b24e0, does NOT run direct_build / cold_char_mount.
/// It ONLY: (1) read-only checks whether the live TitleTopDialog menu/action is rendered and
/// semantically validated (TitleTopDialog vtable, [dialog+0xa48] registry, Load-Game
/// MenuMemberFuncJob node/action chain); (2) ONE-SHOT: fires that native run
/// MENU_MEMBER_FUNC_JOB_RUN_RVA (0x1409aaba0, rcx=node) -- which builds the LIVE registered
/// ProfileLoadDialog the native pump drives. After firing it observes (the caller keeps writing the
/// golden oracle as the native pump hopefully loads the char). Pure read-only until the single fire.
unsafe fn native_load_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    // Already fired: keep observing (oracle written by the caller's pass-through telemetry).
    if NATIVE_LOAD_FIRED.load(Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-load: FIRED -- observing native pump (#{n}); golden oracle written via telemetry"
            ));
        }
        return;
    }
    let Some(action) = (unsafe { title_menu_action_ready(owner, base) }) else {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-load: waiting for semantic Load-Game action readiness (#{n}) -- TitleTopDialog/registry/node/action not all validated yet"
            ));
        }
        return;
    };
    // ONE-SHOT fire. The semantic readiness helper already validated the node vtable, registry,
    // member fn, and factory chain; latch only after that validation succeeds.
    if NATIVE_LOAD_FIRED.swap(NATIVE_LOAD_FIRED_YES, Ordering::SeqCst) != NATIVE_LOAD_FIRED_NO {
        return;
    }
    let node = action.node;
    let node_vt = action.node_vt;
    let m_dlg = action.member_dialog;
    let m_fn = action.member_fn;
    let m_adj = action.member_adjust;
    let run: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(usize)>(
            base + MENU_MEMBER_FUNC_JOB_RUN_RVA,
        )
    };
    append_autoload_debug(format_args!(
        "native-load: *** FIRING native Load-Game run 0x{:x}(rcx=node=0x{node:x}) vt=0x{node_vt:x} [+0x10]=0x{m_dlg:x} [+0x18]=0x{m_fn:x} [+0x20]=0x{m_adj:x} #{n} -- building LIVE ProfileLoadDialog in the NATURAL menu (zero forcing) ***",
        base + MENU_MEMBER_FUNC_JOB_RUN_RVA
    ));
    timeline_event(
        "T_native_load_fire",
        n,
        format_args!("node=0x{node:x} member_fn=0x{m_fn:x}"),
    );
    unsafe { run(node) };
    append_autoload_debug(format_args!(
        "native-load: native Load-Game run returned -- observing native pump for golden oracle (#{n})"
    ));
}

/// SAVE-SAFE READ-ONLY structural scan of the OPEN TitleTopDialog for the **Continue**
/// (load-most-recent) `MenuMemberFuncJob` registry node -- the PATH B analog of
/// `scan_dialog_for_loadgame`. Identical bounded flat scan of the dialog object's own fields, but the
/// MenuMemberFuncJob it latches is the one whose member-fn (node+0x18) chains through the thunk hops
/// to the native Continue wrapper `TRACE_MENU_CONTINUE_WRAPPER_RVA` (0x14082bac0), NOT the Load-Game
/// ProfileLoadDialog factory `LIVE_DIALOG_FACTORY_RVA` (0x14081ead0). This is the SAME discriminator
/// already proven at runtime by `capture_continue_member_node_candidate` (which captures the
/// registered Continue MenuMemberFuncJob off the registrar's task-enqueue path); here we resolve it
/// statically from the live dialog so the native_continue tick can fire its run with no enqueue hook.
/// Pure ReadProcessMemory (`safe_read_usize` tolerates bad derefs) -> NO writes, NO native calls.
/// Returns the first matching Continue MenuMemberFuncJob node, or None.
unsafe fn scan_dialog_for_continue(owner: usize, base: usize) -> Option<usize> {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const DIALOG_E0: usize = TITLE_OWNER_MENU_HOLDER_E0_OFFSET;
    const MEMBERFUNCJOB_VTABLE_RVA_LOCAL: usize = MEMBERFUNCJOB_VTABLE_RVA;
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_ADJ_20: usize = 0x20;
    const SCAN_QWORDS: usize = 0x500;
    const PTR_SZ: usize = core::mem::size_of::<usize>();
    const PTR_ALIGN_MASK: usize = 0x7;
    const HEAP_LO: usize = 0x10000;
    const QW_START: usize = 0;
    const QW_STEP: usize = 1;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    const HIT_CAP: usize = 24;
    const HIT_START: usize = 0;
    const HIT_STEP: usize = 1;

    let dialog = unsafe { safe_read_usize(owner + DIALOG_E0) }.unwrap_or(NULL);
    if dialog == NULL {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(NULL);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        append_autoload_debug(format_args!(
            "continue-scan: owner+0xe0=0x{dialog:x} vt=0x{dialog_vt:x} != TitleTopDialog 0x{:x} -- skip",
            base + TITLE_TOP_DIALOG_VTABLE_RVA
        ));
        return None;
    }
    let memberjob_vt = base + MEMBERFUNCJOB_VTABLE_RVA_LOCAL;
    let continue_wrapper_abs = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    let dialog_factory_abs = base + LIVE_DIALOG_FACTORY_RVA;
    // Resolve a (member-)fn forward through up to JMP_HOPS jmp-thunks; report whether it reaches the
    // native Continue wrapper (Continue) or the Load-Game dialog factory (so we can log BOTH and so a
    // probe reveals every node's discriminator even if our hop budget misses one).
    let classify = |start: usize| -> (bool, bool) {
        let mut tgt = start;
        let mut hop = HOP_START;
        while hop < JMP_HOPS && tgt != NULL {
            if tgt == continue_wrapper_abs {
                return (true, false);
            }
            if tgt == dialog_factory_abs {
                return (false, true);
            }
            match unsafe { decode_thunk_hop(tgt) } {
                Some(next) => tgt = next,
                None => break,
            }
            hop += HOP_STEP;
        }
        (tgt == continue_wrapper_abs, tgt == dialog_factory_abs)
    };
    append_autoload_debug(format_args!(
        "continue-scan: dialog=0x{dialog:x} memberjob_vt=0x{memberjob_vt:x} continue_wrapper=0x{continue_wrapper_abs:x} dialog_factory=0x{dialog_factory_abs:x} -- scanning {SCAN_QWORDS} qwords"
    ));
    let mut found_continue_node: Option<usize> = None;
    let mut hits = HIT_START;
    let mut q = QW_START;
    while q < SCAN_QWORDS {
        let off = q * PTR_SZ;
        let p = unsafe { safe_read_usize(dialog + off) }.unwrap_or(NULL);
        if p != NULL && (p & PTR_ALIGN_MASK) == QW_START && p >= HEAP_LO {
            let vt = unsafe { safe_read_usize(p) }.unwrap_or(NULL);
            if vt == memberjob_vt {
                let mfn = unsafe { safe_read_usize(p + MEMBER_FN_18) }.unwrap_or(NULL);
                let mdlg = unsafe { safe_read_usize(p + MEMBER_DIALOG_10) }.unwrap_or(NULL);
                let madj = unsafe { safe_read_usize(p + MEMBER_ADJ_20) }.unwrap_or(NULL);
                let (is_continue, is_load) = classify(mfn);
                if hits < HIT_CAP {
                    append_autoload_debug(format_args!(
                        "continue-scan: dialog+0x{off:x} MenuMemberFuncJob node=0x{p:x} member_fn=0x{mfn:x} CONTINUE={is_continue} LOAD_GAME={is_load} back=0x{mdlg:x} adj=0x{madj:x}"
                    ));
                }
                // The Continue run target: a MenuMemberFuncJob whose member_fn chains to the native
                // Continue wrapper. Latch the FIRST such node (run 0x1409aaba0 fires against it).
                if is_continue && found_continue_node.is_none() {
                    found_continue_node = Some(p);
                }
                hits += HIT_STEP;
            }
        }
        q += QW_STEP;
    }
    append_autoload_debug(format_args!(
        "continue-scan: done hits={hits} found_continue_node=0x{:x}",
        found_continue_node.unwrap_or(NULL)
    ));
    found_continue_node
}

/// PATH B readiness gate for the native Continue node -- mirror of `title_menu_action_ready` but for
/// the **Continue** (load-most-recent) MenuMemberFuncJob whose member-fn chains to the native
/// Continue wrapper (`TRACE_MENU_CONTINUE_WRAPPER_RVA`) instead of the Load-Game dialog factory.
/// Validates: live TitleTopDialog vtable, [dialog+0xa48] in-image registry, the Continue node's
/// MenuMemberFuncJob vtable, and the member-fn -> Continue-wrapper thunk chain. Returns the fully
/// populated `MenuActionNode` (reusing the same struct as Load-Game) so `native_continue_tick` can
/// fire run 0x1409aaba0 against it with full node telemetry.
unsafe fn title_menu_continue_action_ready(owner: usize, base: usize) -> Option<MenuActionNode> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if dialog == null {
        return None;
    }
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    let registry =
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if !vtable_in_game_image(registry, base) {
        return None;
    }
    let node = unsafe { scan_dialog_for_continue(owner, base) }?;
    let node_vt = unsafe { safe_read_usize(node) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return None;
    }
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    let member_dialog = unsafe { safe_read_usize(node + MEMBER_DIALOG_10) }.unwrap_or(null);
    let member_fn = unsafe { safe_read_usize(node + MEMBER_FN_18) }.unwrap_or(null);
    let member_adjust = unsafe { safe_read_usize(node + MEMBER_ADJ_20) }.unwrap_or(null);
    if member_fn == null {
        return None;
    }
    let continue_wrapper_abs = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    let mut target = member_fn;
    let mut hop = HOP_START;
    while hop < JMP_HOPS && target != null {
        if target == continue_wrapper_abs {
            return Some(MenuActionNode {
                node,
                node_vt,
                registry,
                member_dialog,
                member_fn,
                member_adjust,
                window_item: null,
            });
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += HOP_STEP;
    }
    None
}

/// Resolve the slot the native Continue should load, or None to leave the game's own most-recent
/// selection untouched. The native Continue primitive `continue_load 0x14067b750(rcx=-1)` resolves
/// "-1 == most-recent" by reading `slot = *(GameMan+0xac0)` (verified by disasm), and the game's own
/// `set_save_slot 0x14067a810` writes exactly that field. So when a slot is explicitly configured we
/// call `set_save_slot(slot)` just before firing the Continue node -- steering which character the
/// short Continue path loads WITHOUT touching the save buffer (a transient runtime field, no
/// checksums, nothing persisted; save-safe). Sources, in order: OWN_STEPPER_SLOT (trigger-file
/// "slot=N"), then env ER_EFFECTS_AUTOLOAD_SLOT. Unset (< 0 / absent) => None => true most-recent.
fn native_continue_target_slot() -> Option<i32> {
    let configured = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    if configured >= OWN_STEPPER_SLOT_ZERO {
        return Some(configured);
    }
    if let Ok(v) = std::env::var("ER_EFFECTS_AUTOLOAD_SLOT") {
        if let Ok(slot) = v.trim().parse::<i32>() {
            if slot >= OWN_STEPPER_SLOT_ZERO {
                return Some(slot);
            }
        }
    }
    None
}

/// Crash-on-not-loaded watchdog (privacy-policy-gated-on-character-presence-CONFIRMED-2026-06-23):
/// the Bandai-Namco privacy policy / new-game state shows ONLY when the active profile has no
/// character (profile_slot_active == 0). When a load is expected (not telemetry-only) and the profile
/// summary has been present but reports ZERO active slots for a settle window, the gold save did NOT
/// load -> abort instantly so the failure is loud + fast (no stall on the policy). profile_slot_active
/// != 0 is the single "save loaded" semaphore (redirect fired AND char present AND policy never builds).
unsafe fn save_load_watchdog() {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    if save_override_telemetry_only() {
        return;
    }
    let gdm = crate::game_data_man_ptr_or_null();
    if gdm == NULL {
        return;
    }
    let summary =
        unsafe { safe_read_usize(gdm + crate::SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    if summary == NULL {
        return; // profile summary not loaded yet -> still booting, do not count
    }
    // Profile-summary slot-active array offset == size_of::<usize>() (matches telemetry's read).
    let active = unsafe { safe_read_usize(summary + core::mem::size_of::<usize>()) }.unwrap_or(0);
    if active != 0 {
        SAVE_WATCHDOG_ZERO_FRAMES.store(0, Ordering::SeqCst); // char present -> save loaded
        // First gold load done: stop redirecting %APPDATA% so writes + later loads go to the real
        // default C: dir (the Z: write fails + would mutate the gold). One-shot.
        if !SAVE_FIRST_LOAD_DONE.swap(true, Ordering::SeqCst) {
            append_autoload_debug(format_args!(
                "save-override: FIRST-LOAD-DONE (profile_slot_active=0x{active:x}) -- reverting %APPDATA% redirect to the real default dir for writes + subsequent loads"
            ));
        }
        return;
    }
    let n = SAVE_WATCHDOG_ZERO_FRAMES.fetch_add(1, Ordering::SeqCst) + 1;
    if n == 1 {
        append_autoload_debug(format_args!(
            "save-override: watchdog -- profile summary present but ZERO active slots (no character); counting toward abort budget {SAVE_WATCHDOG_ZERO_BUDGET}"
        ));
    }
    if n >= SAVE_WATCHDOG_ZERO_BUDGET {
        append_autoload_debug(format_args!(
            "save-override: WATCHDOG ABORT -- profile summary reports ZERO active slots after {n} frames; the gold save did NOT load (no character -> privacy policy / new-game). Aborting."
        ));
        eprintln!(
            "er-effects: WATCHDOG ABORT -- gold save not loaded (no character in active profile); aborting."
        );
        std::process::abort();
    }
}

/// OBSERVE-ONLY NATIVE-CONTINUE tick (PATH B, native_continue_enabled(), gated OFF by default). The
/// Continue analog of `native_load_tick`: runs each frame INSTEAD of the own_stepper forcing logic,
/// then the caller pass-throughs to OWN_STEPPER_ORIG_IDX10 so the NATIVE title machine advances
/// untouched (the user/own_stepper opens the menu). KEEP vs the normal own_stepper: it does NOT
/// SetState(owner,2/3), does NOT clear the beginlogo gate, does NOT self-fire the registrar, does NOT
/// run direct_build / cold_char_mount. It ONLY: (1) read-only checks whether the live TitleTopDialog
/// menu is rendered and the Continue MenuMemberFuncJob node/action chain is semantically validated;
/// (2) ONE-SHOT fires that native run MENU_MEMBER_FUNC_JOB_RUN_RVA (0x1409aaba0, rcx=continue_node)
/// -- which drives the FULL native load (parse + world-asset streaming + spawn). After firing it
/// observes (the caller keeps writing the golden oracle + the world-stream telemetry as the native
/// pump streams the world). Pure read-only until the single fire; NO SetState forcing.
unsafe fn native_continue_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    // Fast-fail: if the gold save never loads (no character -> privacy policy), abort instead of
    // stalling the whole run waiting for a Continue menu that the policy gate never lets appear.
    unsafe { save_load_watchdog() };
    // Already fired: keep observing (oracle written by the caller's pass-through telemetry).
    if NATIVE_CONTINUE_FIRED.load(Ordering::SeqCst) != NATIVE_CONTINUE_FIRED_NO {
        if n % NATIVE_CONTINUE_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-continue: FIRED -- observing native pump (#{n}); golden oracle + world-stream telemetry written via pass-through"
            ));
        }
        return;
    }
    let Some(action) = (unsafe { title_menu_continue_action_ready(owner, base) }) else {
        if n % NATIVE_CONTINUE_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-continue: waiting for semantic Continue action readiness (#{n}) -- TitleTopDialog/registry/Continue node/member-fn->wrapper chain not all validated yet"
            ));
        }
        return;
    };
    // ONE-SHOT fire. The semantic readiness helper already validated the node vtable, registry,
    // member fn, and Continue-wrapper chain; latch only after that validation succeeds.
    if NATIVE_CONTINUE_FIRED.swap(NATIVE_CONTINUE_FIRED_YES, Ordering::SeqCst)
        != NATIVE_CONTINUE_FIRED_NO
    {
        return;
    }
    let node = action.node;
    let node_vt = action.node_vt;
    let m_dlg = action.member_dialog;
    let m_fn = action.member_fn;
    let m_adj = action.member_adjust;
    let run: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(usize)>(
            base + MENU_MEMBER_FUNC_JOB_RUN_RVA,
        )
    };
    // Slot steering (save-override-slot-via-setsaveslot): if a slot is explicitly configured, write it
    // into GameMan+0xac0 via the game's own set_save_slot BEFORE firing, so the Continue node's
    // `continue_load(-1)` resolves "-1 == most-recent" to OUR slot (verified: 0x14067b790 reads
    // *(GameMan+0xac0)). Transient runtime field -- no save-buffer edit, no checksum, save-safe. Unset
    // => leave the game's true most-recent selection.
    if let Some(slot) = native_continue_target_slot() {
        let set_save_slot: unsafe extern "system" fn(i32) = unsafe {
            std::mem::transmute::<usize, unsafe extern "system" fn(i32)>(
                base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
            )
        };
        unsafe { set_save_slot(slot) };
        let gm = game_man_ptr_or_null();
        let ac0 = if gm != NULL {
            unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) }
        } else {
            -1
        };
        append_autoload_debug(format_args!(
            "native-continue: slot-steer set_save_slot({slot}) -> GameMan+0xac0={ac0} so Continue's -1 resolves to slot {slot} (#{n})"
        ));
    }
    append_autoload_debug(format_args!(
        "native-continue: *** FIRING native Continue run 0x{:x}(rcx=node=0x{node:x}) vt=0x{node_vt:x} [+0x10]=0x{m_dlg:x} [+0x18]=0x{m_fn:x} [+0x20]=0x{m_adj:x} #{n} -- driving the FULL native load (parse+stream+spawn) in the NATURAL menu (zero forcing) ***",
        base + MENU_MEMBER_FUNC_JOB_RUN_RVA
    ));
    timeline_event(
        "T_native_continue_fire",
        n,
        format_args!("node=0x{node:x} member_fn=0x{m_fn:x}"),
    );
    // Anchor the readiness harness to THIS fire: the watcher's world-load deadline + world-stream
    // stall semaphore + `t_continue_fired` timeline all key off `oracle_own_load_continue_fired`
    // (OWN_LOAD_CONTINUE_FIRED), not the native_continue-private latch. Set it the instant we fire so
    // the world-load is timed from the load actually starting (not launch), and the world-stream
    // telemetry published below by `own_load_stream_telemetry` is interpreted as a live load.
    OWN_LOAD_CONTINUE_FIRED.store(true, Ordering::SeqCst);
    unsafe { run(node) };
    append_autoload_debug(format_args!(
        "native-continue: native Continue run returned -- observing native pump for golden oracle + world-stream (#{n})"
    ));
}

/// Resolve the full-read target slot: a configured OWN_STEPPER_SLOT (>=0, from the trigger-file
/// "slot=N"), else ER_EFFECTS_AUTOLOAD_SLOT (>=0), else FULLREAD_DEFAULT_SLOT (Banon = 0).
fn native_fullread_slot() -> i32 {
    let configured = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    if configured >= OWN_STEPPER_SLOT_ZERO {
        return configured;
    }
    if let Ok(v) = std::env::var("ER_EFFECTS_AUTOLOAD_SLOT") {
        if let Ok(slot) = v.trim().parse::<i32>() {
            if slot >= OWN_STEPPER_SLOT_ZERO {
                return slot;
            }
        }
    }
    FULLREAD_DEFAULT_SLOT
}

/// OBSERVE-ONLY NATIVE FULL-SAVE-READ tick (native_fullread_enabled(), gated OFF by default). Runs
/// each frame INSTEAD of the own_stepper forcing logic (no SetState forcing for boot); the caller
/// pass-throughs to OWN_STEPPER_ORIG_IDX10 so the NATIVE title machine advances untouched. Once the
/// live TitleTopDialog menu action is semantically validated (same readiness helper as
/// native_load_tick: TitleTopDialog vtable, [dialog+0xa48] registry, Load-Game node/action chain),
/// it runs the full-save-read load chain as a per-frame phase
/// machine at the LIVE menu (where the FD4 IO worker pool 0x144853048 is live so the submit drains):
///   SUBMIT: set GameMan+0xb78=slot (step 1, NEW), set_save_slot 0x14067a810 (step 2 -> GameMan+0xac0),
///           submit full read 0x14067b1a0 (step 3, type-0xa).
///   DRAIN:  tick lane 0x140679510 + poll 0x140679180 each frame until GameMan+0xb80==3 (step 4).
///   DESER:  deserialize 0x14067b290(slot) ONCE at b80==3 (step 5 -> GameMan+0xc30 = real map).
///   GUARD:  c30 != 0xa010000 (m10 default) AND char fingerprint present (level>=10 + name) (step 6).
///   CONFIRM (step 7, the SOLE save write): ONLY if the guard passes AND native_fullread_commit_enabled():
///           continue_confirm 0x140b0e180(rcx=shim{[OWNER]=owner}) where owner=*(base+0x3d5df38+8);
///           it checks owner+0x284==0 -> sets owner+0xbc=c30 + SetState5 (AUTOSAVES). Without the
///           commit sub-gate, stops at GUARD (VERIFY-ONLY: log only, NO continue_confirm/NO SetState5).
/// Reuses cold_char_mount_drive's submit/lane/poll/deser CALLS (exact RVAs) but builds/pumps NO
/// selector step (probe-12 crash) and forces NO SetState for boot. Logs b80/c30/level each frame.
unsafe fn native_fullread_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const WAIT_INC: usize = 1;
    let gm = game_man_ptr_or_null();
    let phase = FULLREAD_PHASE.load(Ordering::SeqCst);
    // Already finished: keep observing (the golden oracle is written by the caller's telemetry once
    // the native pump streams the world).
    if phase == FULLREAD_PHASE_DONE {
        if n % FULLREAD_LOG_INTERVAL == NULL as u64 {
            let c30 = if gm != NULL {
                unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) }
            } else {
                GAME_MAN_C30_UNSET
            };
            let (_fp_real, level, _name_len) = unsafe { char_fingerprint(base) };
            append_autoload_debug(format_args!(
                "native-fullread: DONE -- observing native pump (#{n}) c30=0x{c30:x} level={level}"
            ));
        }
        return;
    }
    let Some(action) = (unsafe { title_menu_action_ready(owner, base) }) else {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for semantic Load-Game action readiness (#{n}) gm=0x{gm:x} -- TitleTopDialog/registry/node/action not all validated yet"
            ));
        }
        return;
    };
    if gm == NULL {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for GameMan after menu action ready node=0x{:x} registry=0x{:x} (#{n})",
                action.node, action.registry
            ));
        }
        return;
    }
    let slot = native_fullread_slot();
    let read_i32 = |off: usize| unsafe { *((gm + off) as *const i32) };

    if phase == FULLREAD_PHASE_SUBMIT {
        // Step 1 (NEW): set the slot-resolve global GameMan+0xb78=slot (resolver 0x1406793c0 returns
        // *(u32*)(gm+0xb78)) so the native chain resolves OUR slot. Save-safe (an in-memory selector).
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = slot };
        // Step 2: set_save_slot 0x14067a810(slot) -> GameMan+0xac0=slot.
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        // Step 3: submit the full read 0x14067b1a0(slot) (type-0xa; sets GameMan+0xb80=2, the
        // deserialize arm). At the LIVE menu the FD4 IO worker pool is live so this DRAINS.
        let submit: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + B80_FULL_LOAD_INITIATOR_RVA) };
        let sret = unsafe { submit(slot) };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        append_autoload_debug(format_args!(
            "native-fullread: SUBMIT slot={slot} b78={b78} (0x{:x} write) set_save_slot 0x{:x} ac0={ac0} submit 0x{:x} ret={sret} b80={b80} -> DRAIN",
            base,
            base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
            base + B80_FULL_LOAD_INITIATOR_RVA
        ));
        timeline_event(
            "T_fullread_submit",
            n,
            format_args!("slot={slot} b80={b80}"),
        );
        FULLREAD_DRAIN_WAITS.store(NULL, Ordering::SeqCst);
        FULLREAD_PHASE.store(FULLREAD_PHASE_DRAIN, Ordering::SeqCst);
        return;
    }

    if phase == FULLREAD_PHASE_DRAIN {
        // Step 4: tick lane 0x140679510 (b80==1/2 IO tick) + poll 0x140679180 each frame until
        // GameMan+0xb80==3 (RESIDENT, the 0x280000 buffer drained). Reuses cold_char_mount's calls.
        let lane: unsafe extern "system" fn() -> i32 =
            unsafe { std::mem::transmute(base + B80_LANE1_DRIVER_RVA) };
        let _ = unsafe { lane() };
        let poll: unsafe extern "system" fn(u8, u8) -> i32 =
            unsafe { std::mem::transmute(base + B80_POLL_RVA) };
        let _ = unsafe { poll(FULLREAD_POLL_ARG, FULLREAD_POLL_ARG) };
        let b80 = read_i32(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let w = FULLREAD_DRAIN_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst) as u64;
        if w % FULLREAD_LOG_INTERVAL == NULL as u64 {
            let (_fp, level, _nl) = unsafe { char_fingerprint(base) };
            append_autoload_debug(format_args!(
                "native-fullread: DRAIN waits={w} b80={b80} c30=0x{c30:x} level={level}"
            ));
        }
        if b80 == FULLREAD_B80_RESIDENT {
            append_autoload_debug(format_args!(
                "native-fullread: b80 reached RESIDENT(3) after {w} drain ticks -- the LIVE worker pool DRAINED the full read -> DESER"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DESER, Ordering::SeqCst);
        } else if w >= FULLREAD_DRAIN_MAX {
            append_autoload_debug(format_args!(
                "native-fullread: b80 STUCK at {b80} after {w} drain ticks (full read never resident) -- TIMEOUT (no write) -> DONE"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == FULLREAD_PHASE_DESER {
        // Step 5: deserialize 0x14067b290(slot) ONCE at b80==3 -> writes GameMan+0xc30 = real map.
        let deser: unsafe extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(base + DESERIALIZE_SLOT_RVA) };
        let dret = unsafe { deser(slot) };
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let (_fp, level, _nl) = unsafe { char_fingerprint(base) };
        append_autoload_debug(format_args!(
            "native-fullread: DESER slot={slot} ret={dret} c30=0x{c30:x} ac0={ac0} level={level} -> GUARD"
        ));
        timeline_event(
            "T_fullread_deser",
            n,
            format_args!("c30=0x{c30:x} level={level}"),
        );
        FULLREAD_PHASE.store(FULLREAD_PHASE_GUARD, Ordering::SeqCst);
        return;
    }

    if phase == FULLREAD_PHASE_GUARD {
        // Step 6: GUARD. c30 != 0xa010000 (m10 default) AND char fingerprint present (level>=10 +
        // non-empty name). This is the HARD gate for the only save write.
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let (fp_real, level, name_len) = unsafe { char_fingerprint(base) };
        let c30_real = c30 != FULLREAD_C30_M10_DEFAULT && c30 != GAME_MAN_C30_UNSET;
        let level_real = level >= FULLREAD_MIN_REAL_LEVEL;
        let guard_pass = c30_real && fp_real && level_real;
        let commit = native_fullread_commit_enabled();
        append_autoload_debug(format_args!(
            "native-fullread: GUARD c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={level} level_real={level_real} name_len={name_len} -> guard_pass={guard_pass} commit_gate={commit}"
        ));
        if !guard_pass {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD FAIL (c30=0x{c30:x} level={level}) -- NO continue_confirm, NO SetState5, NO save write -> DONE (save-safe)"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // Step 7 is HARD-gated behind BOTH the guard above AND the commit sub-gate (default off):
        // VERIFY-ONLY by default -- stop here (log only, NO continue_confirm/NO SetState5).
        if !commit {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD PASS (c30=0x{c30:x} level={level}) but VERIFY-ONLY (commit sub-gate OFF) -- NO continue_confirm, NO SetState5 -> DONE (save-safe). Set ER_EFFECTS_FULLREAD_COMMIT=1 / er-effects-fullread-commit.txt to commit."
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // COMMIT: continue_confirm 0x140b0e180(rcx=&shim{[OWNER]=owner}), owner=*(base+0x3d5df38+8).
        // It checks owner+0x284==0 -> sets owner+0xbc=c30 + SetState5 (AUTOSAVES). Look before acting:
        // resolve owner read-only + confirm owner+0x284==0 before the native call (fail-closed).
        let game_data_man = game_data_man_ptr_or_null();
        let owner_obj = if game_data_man == NULL {
            NULL
        } else {
            unsafe { safe_read_usize(game_data_man + FULLREAD_OWNER_GDM_08_OFFSET) }.unwrap_or(NULL)
        };
        if owner_obj == NULL {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- continue_confirm owner (GameDataMan=0x{game_data_man:x}, offset=0x{:x}) is null -> DONE (no write)",
                FULLREAD_OWNER_GDM_08_OFFSET
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let new_game_flag =
            unsafe { *((owner_obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *const u8) };
        if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- owner+0x284={new_game_flag} != 0 (continue_confirm requires the new-game flag clear) -> DONE (no write)"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let shim = &raw mut OWN_STEPPER_SHIM;
        unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner_obj };
        let shim_ptr = shim as usize;
        let confirm: unsafe extern "system" fn(usize) =
            unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
        append_autoload_debug(format_args!(
            "native-fullread: *** COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} owner=0x{owner_obj:x}) c30=0x{c30:x} level={level} owner+0x284=0 -- SetState5 (AUTOSAVES) ***",
            base + CONTINUE_CONFIRM_RVA
        ));
        timeline_event(
            "T_fullread_confirm",
            n,
            format_args!("c30=0x{c30:x} level={level}"),
        );
        unsafe { confirm(shim_ptr) };
        append_autoload_debug(format_args!(
            "native-fullread: continue_confirm returned -- native pump now streams the real world (#{n}) -> DONE"
        ));
        FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        return;
    }
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
        unsafe { native_continue_tick(owner, base, n) };
        // Per-frame world-stream telemetry (pure reads, save-safe). native_continue_tick's one-shot
        // latch fast-forwards to FIRED after the Continue run fires, so this runs EVERY native_continue
        // frame -- including all the post-fire loading-screen frames where the title owner is still
        // ticked -- publishing the deepest world-load pump values (mms_state, block_count,
        // io_inflight, player_present) so a probe log shows whether the world STREAMS (mms_state
        // advancing past 3, player_present=true) after the Continue fire. Gated to native_continue.
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

/// STAGE 2 in-context load drive (see the lib.rs STAGE-2 const block). Runs each frame while
/// `OWN_STEPPER_PHASE` is one of the four S2 phases, sequencing:
///   INVOKE  -> hand-fire d180's `+0xa8` functor to build the ProfileLoadDialog
///   ACTIVATE-> write slot cursor `[dialog+0xb0c]=N`, call vtable-slot-20 `load_activate(dialog)`
///   MOUNT_POLL -> let the native pump tick the selector; detect the mount (`ac0==N` + io
///               request set->cleared); latch the real `c30`
///   CONFIRM -> guard (`ac0==N && c30==latched`) then `continue_confirm` -> SetState(5)
/// Every cross-into-game call is gated by read-only preconditions; the ONLY save-write risk is
/// the CONFIRM SetState(5), gated entirely by a verified real mount (fail-closed otherwise:
/// stay at the menu, NO SetState(5), NO save write).
unsafe fn own_stepper_stage2(
    owner: usize,
    base: usize,
    gm: usize,
    want_slot: i32,
    n: u64,
    framectx: usize,
) {
    const S2_LOG_INTERVAL: u64 = 30;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let waits = OWN_STEPPER_S2_WAITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let s2_elapsed_ms = own_stepper_s2_elapsed_ms();
    let s2_timed_out = own_stepper_s2_timed_out();
    let item = MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst);
    let pld_vt = base + PROFILE_LOAD_DIALOG_VTABLE_RVA;
    // 32-bit GameMan field read (low dword of the 8-byte safe read; little-endian).
    let ri32 = |addr: usize, dflt: i32| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(dflt)
    };
    let c30 = if gm != null {
        ri32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET, GAME_MAN_C30_UNSET)
    } else {
        GAME_MAN_C30_UNSET
    };
    let ac0 = if gm != null {
        ri32(
            gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET,
            OWN_STEPPER_SLOT_NONE,
        )
    } else {
        OWN_STEPPER_SLOT_NONE
    };
    let b80 = if gm != null {
        ri32(
            gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET,
            OWN_STEPPER_B80_IDLE,
        )
    } else {
        OWN_STEPPER_B80_IDLE
    };
    let iodev = unsafe { safe_read_usize(base + IODEV_GLOBAL_RVA) }.unwrap_or(null);
    let (io10, io18, io20) = if iodev != null {
        (
            unsafe { safe_read_usize(iodev + IODEV_INFLIGHT_10_OFFSET) }.unwrap_or(null),
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_18_OFFSET) }.unwrap_or(null),
            unsafe { safe_read_usize(iodev + IODEV_REQHANDLE_20_OFFSET) }.unwrap_or(null),
        )
    } else {
        (null, null, null)
    };
    // A dialog candidate is valid iff its vtable == ProfileLoadDialog.
    let valid_dialog =
        |d: usize| -> bool { d != null && unsafe { safe_read_usize(d) }.unwrap_or(null) == pld_vt };

    if phase == OWN_STEPPER_PHASE_S2_INVOKE {
        if item == null {
            if s2_timed_out {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-INVOKE-TIMEOUT no item after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        let dlg130 =
            unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }.unwrap_or(null);
        let ctx10 = unsafe { safe_read_usize(item + MENU_ITEM_CTX_10_OFFSET) }.unwrap_or(null);
        let functor =
            unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }.unwrap_or(null);
        // If the native pump already built the dialog (focused on Load), use it.
        let existing = if valid_dialog(dlg130) {
            dlg130
        } else if valid_dialog(ctx10) {
            ctx10
        } else {
            null
        };
        if existing != null {
            OWN_STEPPER_DIALOG.store(existing, Ordering::SeqCst);
            timeline_event(
                "T_dialog",
                n,
                format_args!("dialog=0x{existing:x} via=native"),
            );
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE-OK (native-built) dialog=0x{existing:x} dvt=0x{pld_vt:x} item=0x{item:x}"
            ));
            own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
            return;
        }
        // Drive d180's NATIVE Update once as soon as the item exists and its native build
        // preconditions are true. d180 lives at owner+0x130 under an input-gated IfElseJob branch
        // (its case child is never bound headless), so the native pump never ticks it -- but the
        // item is fully built, so calling its own MenuWindowJob::Update 0x1407ad1c0 (which wires
        // the ctx item+0x10 from the descriptor item+0x58 before firing the functor) builds the
        // ProfileLoadDialog with a NATIVE ctx (no synthesis) and zero input. Build-only;
        // idempotent; no save write.
        if OWN_STEPPER_INVOKED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS as usize {
            let ret = unsafe { drive_menu_item_update(item, base, framectx) }.unwrap_or(null);
            OWN_STEPPER_INVOKED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            let dlg130b = unsafe { safe_read_usize(item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) }
                .unwrap_or(null);
            let ctx10b = unsafe { safe_read_usize(item + MENU_ITEM_CTX_10_OFFSET) }.unwrap_or(null);
            let candidate = if valid_dialog(ret) {
                ret
            } else if valid_dialog(dlg130b) {
                dlg130b
            } else if valid_dialog(ctx10b) {
                ctx10b
            } else {
                null
            };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE hand-fired item=0x{item:x} functor=0x{functor:x} ret=0x{ret:x} dlg130(pre=0x{dlg130:x},post=0x{dlg130b:x}) ctx10(pre=0x{ctx10:x},post=0x{ctx10b:x}) candidate=0x{candidate:x}"
            ));
            if candidate != null {
                // Mirror native bookkeeping: stash the built dialog at item+0x130 if empty so a
                // later native leaf-Update does not re-build it.
                if dlg130b == null {
                    unsafe {
                        *((item + MENU_ITEM_DIALOG_RESULT_130_OFFSET) as *mut usize) = candidate;
                    }
                }
                OWN_STEPPER_DIALOG.store(candidate, Ordering::SeqCst);
                timeline_event(
                    "T_dialog",
                    n,
                    format_args!("dialog=0x{candidate:x} via=invoke"),
                );
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
                return;
            }
        }
        if s2_timed_out {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-INVOKE-TIMEOUT dialog not built after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_ACTIVATE {
        let dialog = OWN_STEPPER_DIALOG.load(Ordering::SeqCst);
        if gm == null {
            if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-ACTIVATE waiting for GameMan before load_activate dialog=0x{dialog:x}"
                ));
            }
            if s2_timed_out {
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        }
        let log_pending = waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64;
        let Some(ready) =
            (unsafe { profile_load_dialog_ready(base, dialog, want_slot, log_pending) })
        else {
            if s2_timed_out {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-ACTIVATE-TIMEOUT profile_load_dialog_ready stayed false after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
            return;
        };
        let dialog = ready.dialog;
        let dvt = ready.dvt;
        let bound = ready.bound;
        let cursor_now = ready.cursor_now;
        let expected_slot = ready.expected_slot;
        let cursor_target = ready.cursor_target;
        let lav = ready.load_activate;
        // For a fixed slot, write the dialog row cursor (UI state, not a save write); for
        // most-recent, leave the dialog's own highlight untouched.
        if want_slot != OWN_STEPPER_SLOT_NONE {
            unsafe {
                *((dialog + DIALOG_SLOT_CURSOR_B0C_OFFSET) as *mut i32) = cursor_target;
            }
        }
        OWN_STEPPER_EXPECTED_SLOT.store(expected_slot, Ordering::SeqCst);
        if (live_dialog_enabled() || product_autoload_enabled())
            && expected_slot != OWN_STEPPER_SLOT_NONE
        {
            let set_save_slot: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
            unsafe { set_save_slot(expected_slot) };
            let slot_after = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-ACTIVATE native-selector set_save_slot({expected_slot}) after profile_load_dialog_ready -> ac0={slot_after}"
            ));
        }
        OWN_STEPPER_SELECTOR_STEP.store(null, Ordering::SeqCst);
        OWN_STEPPER_SELECTOR_CTX.store(null, Ordering::SeqCst);
        let activate: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(lav) };
        let r = unsafe { activate(dialog) };
        append_autoload_debug(format_args!(
            "own_stepper: STAGE2-ACTIVATE profile_load_dialog_ready opened want={want_slot} expected={expected_slot} cursor_target={cursor_target} cursor_now={cursor_now} bound={bound} dvt=0x{dvt:x} lav=0x{lav:x} ret={r} dialog=0x{dialog:x} ctx=0x{:x} ctx_vt=0x{:x} pgd=0x{:x} io18=0x{io18:x} io20=0x{io20:x} -- MOUNT via live selector tick plus direct submit+drain+deser",
            ready.load_job_ctx, ready.load_job_ctx_vt, ready.player_game_data
        ));
        // Reset the shared mount latches so the MOUNT phase's delegate (cold_char_mount_drive) and
        // the mount-done gate observe a clean slate for this drive.
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_NOT_FIRED, Ordering::SeqCst);
        OWN_STEPPER_MOUNT_C30.store(GAME_MAN_C30_UNSET, Ordering::SeqCst);
        OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_NO, Ordering::SeqCst);
        own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_MOUNT_POLL);
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_MOUNT_POLL {
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        // Product/live-dialog path: once load_activate builds the real selector step, self-pump
        // that native selector instead of jumping straight to the cold full-read helper. This is the
        // proper Load-Game beginning: profile rows/record state -> load_activate -> selector tick ->
        // menu_deser/mount. The cold helper remains for the older non-selector diagnostic paths.
        let native_selector_path = live_dialog_enabled() || product_autoload_enabled();
        if native_selector_path {
            const SELECTOR_TICK_RVA: usize = PROFILE_LOAD_SELECTOR_TICK_RVA;
            #[repr(C)]
            struct SelectorTickResultLayout {
                qwords: [usize; 4],
            }
            const SELECTOR_RESULT_QWORDS: usize =
                core::mem::size_of::<SelectorTickResultLayout>() / core::mem::size_of::<usize>();
            let step = OWN_STEPPER_SELECTOR_STEP.load(Ordering::SeqCst);
            let selector_ctx = OWN_STEPPER_SELECTOR_CTX.load(Ordering::SeqCst);
            if step != null && selector_ctx != null {
                let tick: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
                    unsafe { std::mem::transmute(base + SELECTOR_TICK_RVA) };
                let mut result = [TITLE_OWNER_SCAN_START_ADDRESS; SELECTOR_RESULT_QWORDS];
                let result_ptr = result.as_mut_ptr() as usize;
                let tick_ret = unsafe { tick(step, selector_ctx, result_ptr, null) };
                if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "own_stepper: native selector self-pump step=0x{step:x} ctx=0x{selector_ctx:x} result=0x{result_ptr:x} ret=0x{tick_ret:x}"
                    ));
                }
            } else if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                append_autoload_debug(format_args!(
                    "own_stepper: native selector self-pump waiting for selector step/ctx step=0x{step:x} ctx=0x{selector_ctx:x}"
                ));
            }
        } else {
            unsafe { cold_char_mount_drive(base, gm, want_slot, n) };
        }
        // io18/io20 both non-null => the request was started; latch it.
        if io18 != null && io20 != null {
            OWN_STEPPER_IO_WAS_SET.store(OWN_STEPPER_IO_WAS_SET_YES, Ordering::SeqCst);
        }
        let io_was_set =
            OWN_STEPPER_IO_WAS_SET.load(Ordering::SeqCst) == OWN_STEPPER_IO_WAS_SET_YES;
        let io_consumed = io18 == null && io20 == null;
        // Mount signal = the deserialize 0x67b290 SUCCEEDED (ret==1), which proves it wrote c30 from
        // the save header + applied the real char. c30 itself is ambiguous (the char's real early map
        // 0xa010000 collides with the new-game default), so the reliable signal is deser-success +
        // a SANE latched c30 (not the unset sentinel, not zero). (setstate5-is-save-safe-c30-from-save)
        const C30_ZERO: i32 = 0;
        let _ = (io_was_set, io_consumed);
        let mut latched_c30 = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let mut deser_state = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst);
        if native_selector_path
            && deser_state == OWN_STEPPER_DESER_NOT_FIRED
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30 != GAME_MAN_C30_UNSET
            && c30 != C30_ZERO
        {
            let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
            if fp_real {
                OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
                OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
                latched_c30 = c30;
                deser_state = OWN_STEPPER_DESER_FIRED_OK;
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-MOUNT-LATCH native-selector ac0={ac0} expected={expected} c30=0x{c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len})"
                ));
            }
        }
        let deser_ok = deser_state == OWN_STEPPER_DESER_FIRED_OK;
        let deser_done = deser_state != OWN_STEPPER_DESER_NOT_FIRED;
        let (fp_real_mount, _fp_level_mount, _fp_name_len_mount) =
            unsafe { char_fingerprint(base) };
        let c30_available = latched_c30 != GAME_MAN_C30_UNSET && latched_c30 != C30_ZERO;
        let c30_sane =
            c30_available && (latched_c30 != GAME_MAN_NEWGAME_DEFAULT_MAP || fp_real_mount);
        let mount_done =
            deser_ok && c30_sane && ac0 == expected && expected != OWN_STEPPER_SLOT_NONE;
        if waits % S2_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 || deser_done {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-POLL waits={waits} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} deser_ok={deser_ok} c30_sane={c30_sane} b80={b80} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        // Default VERIFY-ONLY: stop at deserialize. With the explicit fullread commit gate enabled,
        // a verified mount advances to CONFIRM, whose independent guard re-checks deser_ok,
        // fp_real, expected slot, and c30 latch before continue_confirm/SetState5.
        if deser_done {
            let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
            timeline_event(
                "T_mount",
                n,
                format_args!("ac0={ac0} c30=0x{latched_c30:x} waits={waits}"),
            );
            let commit = native_fullread_commit_enabled();
            if mount_done && commit {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-MOUNT-COMMIT deser_ok={deser_ok} mount_done={mount_done} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) b80={b80} -- entering CONFIRM"
                ));
                own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_CONFIRM);
            } else {
                append_autoload_debug(format_args!(
                    "own_stepper: STAGE2-MOUNT-VERIFY deser_ok={deser_ok} mount_done={mount_done} commit={commit} ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched_c30:x} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) b80={b80} -- VERIFY-ONLY (NO SetState5/NO save write) -> DONE"
                ));
                OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            }
        } else if s2_timed_out {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-MOUNT-POLL-TIMEOUT ac0={ac0} want={want_slot} c30=0x{c30:x} io_was_set={io_was_set} after {waits} polls/{s2_elapsed_ms}ms -- STAGE2-NOWRITE-ABORT (stay at menu)"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }

    if phase == OWN_STEPPER_PHASE_S2_CONFIRM {
        let latched = OWN_STEPPER_MOUNT_C30.load(Ordering::SeqCst);
        let expected = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
        // HARD save-write guard: only SetState(5) when the real char is still mounted. Require the
        // mount latch, c30 unchanged since the mount and present, the slot match, and the decisive
        // PlayerGameData character fingerprint. c30 may legitimately equal the m10_01 default for
        // saves parked there, and the UTF-16 name field can be empty/unknown, so neither is a hard
        // failure when the level/stat fingerprint is real.
        const DESER_FIRED_OK_CONFIRM: usize = 2;
        const C30_ZERO_CONFIRM: i32 = 0;
        let deser_ok = OWN_STEPPER_DESER_FIRED.load(Ordering::SeqCst) == DESER_FIRED_OK_CONFIRM;
        // CHAR-FINGERPRINT gate (MODEL B): SetState(5) ONLY when a REAL character is mounted in
        // PlayerGameData (level>=1). Runtime direct-build evidence showed the mounted target slot
        // has real stats/level while the name field remains empty/unknown, so name is diagnostic
        // only. The new-game default remains level 0, so level>=1 still fail-closes safely.
        let (fp_real, fp_level, fp_name_len) = unsafe { char_fingerprint(base) };
        let c30_available = c30 == latched && c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO_CONFIRM;
        let proceed = deser_ok
            && fp_real
            && ac0 == expected
            && expected != OWN_STEPPER_SLOT_NONE
            && c30_available;
        if !proceed {
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-CONFIRM-GUARD-FAIL ac0={ac0} expected={expected} c30=0x{c30:x} latched=0x{latched:x} deser_ok={deser_ok} fp_real={fp_real}(level={fp_level} name_len={fp_name_len}) -- STAGE2-NOWRITE-ABORT"
            ));
            OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        if OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS as usize {
            let shim = &raw mut OWN_STEPPER_SHIM;
            unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner };
            let shim_ptr = shim as usize;
            let confirm: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(base + CONTINUE_CONFIRM_RVA) };
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-CONFIRM-GUARD-PASS ac0={ac0} c30=0x{c30:x} -> continue_confirm shim=0x{shim_ptr:x} owner=0x{owner:x}"
            ));
            timeline_event("T_playgame", n, format_args!("ac0={ac0} c30=0x{c30:x}"));
            unsafe { confirm(shim_ptr) };
            OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "own_stepper: STAGE2-SETSTATE5 fired owner=0x{owner:x} -- native pump now streams the real world"
            ));
        }
        OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_DONE, Ordering::SeqCst);
    }
}

fn utf16_name_empty_like(units: &[u16], len: usize) -> bool {
    const NAME_LEN_NONE: usize = 0;
    const NAME_LEN_SINGLE: usize = 1;
    const NAME_UNDERSCORE: u16 = '_' as u16;
    const NAME_SPACE: u16 = ' ' as u16;
    if len == NAME_LEN_NONE {
        return true;
    }
    if len == NAME_LEN_SINGLE && units.first().copied() == Some(NAME_UNDERSCORE) {
        return true;
    }
    units.iter().take(len).all(|unit| *unit == NAME_SPACE)
}

fn utf16_names_equal(left: &[u16], right: &[u16], len: usize) -> bool {
    left.get(..len) == right.get(..len)
}

unsafe fn read_utf16_name_units(addr: usize) -> ([u16; PGD_NAME_LEN_U16], usize) {
    const ZERO_U16: u16 = 0;
    const U16_STRIDE: usize = 2;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    let mut units = [ZERO_U16; PGD_NAME_LEN_U16];
    let mut len = IDX_START;
    while len < PGD_NAME_LEN_U16 {
        let unit = unsafe { safe_read_usize(addr + len * U16_STRIDE) }
            .map(|value| value as u16)
            .unwrap_or(ZERO_U16);
        units[len] = unit;
        if unit == ZERO_U16 {
            break;
        }
        len += IDX_STEP;
    }
    (units, len)
}

#[derive(Clone, Copy)]
struct RequestedSlotIdentity {
    matches: bool,
    profile_summary: usize,
    profile_map: i32,
    profile_level: u32,
    profile_name_len: usize,
    pgd_level: u32,
    pgd_name_len: usize,
}

unsafe fn profile_slot_fingerprint(slot: i32) -> (bool, i32, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U32: u32 = 0;
    const NAME_LEN_NONE: usize = 0;
    const MIN_REAL_LEVEL: u32 = 1;
    const PROFILE_RECORD_BASE: usize = 0x18;
    const PROFILE_RECORD_STRIDE: usize = 0x2a0;
    const PROFILE_RECORD_LEVEL_OFFSET: usize = 0x24;
    const PROFILE_RECORD_MAP_OFFSET: usize = 0x30;
    if slot < OWN_STEPPER_SLOT_ZERO {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == NULL {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    if profile_summary == NULL {
        return (false, BAD_I32, ZERO_U32, NAME_LEN_NONE);
    }
    let slot_index = slot as usize;
    let rec = profile_summary + PROFILE_RECORD_BASE + slot_index * PROFILE_RECORD_STRIDE;
    let profile_map = unsafe { safe_read_usize(rec + PROFILE_RECORD_MAP_OFFSET) }
        .map(|value| value as u32 as i32)
        .unwrap_or(BAD_I32);
    let profile_level = unsafe { safe_read_usize(rec + PROFILE_RECORD_LEVEL_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (profile_name, profile_name_len) = unsafe { read_utf16_name_units(rec) };
    let profile_name_empty = utf16_name_empty_like(&profile_name, profile_name_len);
    (
        profile_level >= MIN_REAL_LEVEL && !profile_name_empty,
        profile_map,
        profile_level,
        profile_name_len,
    )
}

unsafe fn requested_slot_identity(slot: i32, c30: i32) -> RequestedSlotIdentity {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U32: u32 = 0;
    const NAME_LEN_NONE: usize = 0;
    const PROFILE_RECORD_BASE: usize = 0x18;
    const PROFILE_RECORD_STRIDE: usize = 0x2a0;
    const PROFILE_RECORD_LEVEL_OFFSET: usize = 0x24;
    const PROFILE_RECORD_MAP_OFFSET: usize = 0x30;
    let mut result = RequestedSlotIdentity {
        matches: false,
        profile_summary: NULL,
        profile_map: BAD_I32,
        profile_level: ZERO_U32,
        profile_name_len: NAME_LEN_NONE,
        pgd_level: ZERO_U32,
        pgd_name_len: NAME_LEN_NONE,
    };
    if slot < OWN_STEPPER_SLOT_ZERO {
        return result;
    }
    let gdm = game_data_man_ptr_or_null();
    if gdm == NULL {
        return result;
    }
    let pgd =
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL);
    let profile_summary =
        unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL);
    result.profile_summary = profile_summary;
    if pgd == NULL || profile_summary == NULL {
        return result;
    }
    let slot_index = slot as usize;
    let rec = profile_summary + PROFILE_RECORD_BASE + slot_index * PROFILE_RECORD_STRIDE;
    let profile_map = unsafe { safe_read_usize(rec + PROFILE_RECORD_MAP_OFFSET) }
        .map(|value| value as u32 as i32)
        .unwrap_or(BAD_I32);
    let profile_level = unsafe { safe_read_usize(rec + PROFILE_RECORD_LEVEL_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (profile_name, profile_name_len) = unsafe { read_utf16_name_units(rec) };
    let pgd_level = unsafe { safe_read_usize(pgd + PGD_LEVEL_68_OFFSET) }
        .map(|value| value as u32)
        .unwrap_or(ZERO_U32);
    let (pgd_name, pgd_name_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let profile_name_empty = utf16_name_empty_like(&profile_name, profile_name_len);
    let pgd_name_empty = utf16_name_empty_like(&pgd_name, pgd_name_len);
    result.profile_map = profile_map;
    result.profile_level = profile_level;
    result.profile_name_len = profile_name_len;
    result.pgd_level = pgd_level;
    result.pgd_name_len = pgd_name_len;
    result.matches = profile_map == c30
        && profile_level == pgd_level
        && profile_name_len == pgd_name_len
        && !profile_name_empty
        && !pgd_name_empty
        && utf16_names_equal(&profile_name, &pgd_name, pgd_name_len);
    result
}

/// CHAR-FINGERPRINT save-write gate: returns (is_real, level, name_len) by reading the live
/// CS::PlayerGameData (GameDataMan `[base+0x3d5df38]` -> +0x08 -> PlayerGameData), the validated
/// reading (the same chain dump_load_correctness uses). A REAL mounted character has level>=1 AND
/// a non-empty-like 16-bit name (`"_"`, empty, and all-spaces are empty-like). Pure
/// fault-tolerant safe_read_usize -> never faults. Used to FAIL-CLOSED SetState(5): the c30
/// oracle is ambiguous (m10_01 collision), so the character actually present in PlayerGameData is
/// the decisive signal.
unsafe fn char_fingerprint(base: usize) -> (bool, u32, usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ZERO_U32: u32 = 0;
    const MIN_REAL_LEVEL: u32 = 1;
    const NAME_LEN_NONE: usize = 0;
    let gdm = game_data_man_ptr_or_null();
    let pgd = if gdm != NULL {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if pgd == NULL {
        return (false, ZERO_U32, NAME_LEN_NONE);
    }
    let level = unsafe { safe_read_usize(pgd + PGD_LEVEL_68_OFFSET) }
        .map(|v| v as u32)
        .unwrap_or(ZERO_U32);
    let (name_units, name_len) = unsafe { read_utf16_name_units(pgd + PGD_NAME_9C_OFFSET) };
    let is_real = level >= MIN_REAL_LEVEL && !utf16_name_empty_like(&name_units, name_len);
    (is_real, level, name_len)
}

/// Read the load-correctness invariants at the in-world transition and log a single greppable
/// `LOAD-CORRECTNESS` record: GameMan c30/ac0/name_is_empty + the CS::PlayerGameData
/// (`[base+0x4588268]`) character fingerprint (name, level, runes, rune-memory, chr_type,
/// 8-stat block). A native-menu load and a DLL-driven load produce comparable records;
/// correctness == field-for-field match (name non-empty, level/runes/stats equal). Pure reads,
/// fault-tolerant; safe to call once at the first in-world frame.
pub(crate) unsafe fn dump_load_correctness(base: usize, frame: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const BAD_I32: i32 = -1;
    const ZERO_U16: u16 = 0;
    const ZERO_U32: u32 = 0;
    const NAME_UNKNOWN: u8 = 0xff;
    const U16_STRIDE: usize = 2;
    const U32_STRIDE: usize = 4;
    const IDX_START: usize = 0;
    const IDX_STEP: usize = 1;
    let gm = game_man_ptr_or_null();
    let ri32 = |addr: usize| -> i32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32 as i32)
            .unwrap_or(BAD_I32)
    };
    let ru32 = |addr: usize| -> u32 {
        unsafe { safe_read_usize(addr) }
            .map(|v| v as u32)
            .unwrap_or(ZERO_U32)
    };
    let (c30, ac0, name_empty) = if gm != NULL {
        (
            ri32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET),
            ri32(gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET),
            unsafe { safe_read_usize(gm + GAME_MAN_NAME_IS_EMPTY_E70_OFFSET) }
                .map(|v| v as u8)
                .unwrap_or(NAME_UNKNOWN),
        )
    } else {
        (BAD_I32, BAD_I32, NAME_UNKNOWN)
    };
    // [0x144588268] -> GameDataMan; PlayerGameData (the save data) = [GameDataMan + 0x08].
    let gdm = game_data_man_ptr_or_null();
    let pgd = if gdm != NULL {
        unsafe { safe_read_usize(gdm + GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET) }.unwrap_or(NULL)
    } else {
        NULL
    };
    if pgd == NULL {
        append_autoload_debug(format_args!(
            "LOAD-CORRECTNESS frame={frame} pgd=NULL gm_c30=0x{c30:x} gm_ac0={ac0} name_empty={name_empty}"
        ));
        return;
    }
    let level = ru32(pgd + PGD_LEVEL_68_OFFSET);
    let runes = ru32(pgd + PGD_RUNE_COUNT_6C_OFFSET);
    let rune_mem = ru32(pgd + PGD_RUNE_MEMORY_70_OFFSET);
    let chr_type = ru32(pgd + PGD_CHR_TYPE_98_OFFSET);
    // character_name: up to 17 UTF-16LE units, to the first NUL.
    let mut name_units = [ZERO_U16; PGD_NAME_LEN_U16];
    let mut i = IDX_START;
    while i < PGD_NAME_LEN_U16 {
        name_units[i] = unsafe { safe_read_usize(pgd + PGD_NAME_9C_OFFSET + i * U16_STRIDE) }
            .map(|v| v as u16)
            .unwrap_or(ZERO_U16);
        i += IDX_STEP;
    }
    let mut nlen = IDX_START;
    while nlen < PGD_NAME_LEN_U16 && name_units[nlen] != ZERO_U16 {
        nlen += IDX_STEP;
    }
    let name = String::from_utf16(&name_units[..nlen]).unwrap_or_default();
    let mut stats = [ZERO_U32; PGD_STAT_COUNT];
    let mut s = IDX_START;
    while s < PGD_STAT_COUNT {
        stats[s] = ru32(pgd + PGD_STAT_BASE_3C_OFFSET + s * U32_STRIDE);
        s += IDX_STEP;
    }
    append_autoload_debug(format_args!(
        "LOAD-CORRECTNESS frame={frame} gm_c30=0x{c30:x} gm_ac0={ac0} name_empty={name_empty} pgd=0x{pgd:x} chr_type={chr_type} name={name:?} level={level} runes={runes} rune_mem={rune_mem} stats={stats:?}"
    ));
}

/// OWN-THE-STEPPER idx6 (STEP_GameStepWait) handler: runs IN-CONTEXT after idx10's
/// placeholder SetState(5) builds the MoveMapStep, whose native update 0x140aff640 ticks
/// the b80 dispatchers (disp1 0x140afbad0 + disp2 0x140afb880). idx6 does NOT call the
/// deserialize itself -- it keeps the b78-route armed (re-plant GameMan+0xb78=slot, clear
/// b72, only while b80 is idle) so the NATIVE disp2 b78-route initiates and disp1
/// deserializes the real slot into GameMan+0xc30. When c30 turns real, idx6 re-targets
/// owner+0xbc to that map and SetState(5) ONCE so the load streams the character's real
/// world instead of the m60 placeholder. Pass-through (watch+log) otherwise.
pub(crate) unsafe extern "system" fn own_stepper_idx6(owner: usize, framectx: usize) {
    let base = OWN_STEPPER_BASE.load(Ordering::SeqCst);
    let phase = OWN_STEPPER_PHASE.load(Ordering::SeqCst);
    let gm = game_man_ptr_or_null();
    let csfeman = unsafe { *((base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let read_gm = |off: usize| {
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let n = OWN_STEPPER_IDX6_CALLS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) as u64;
    let pass6 = || {
        let orig = OWN_STEPPER_ORIG_IDX6.load(Ordering::SeqCst);
        if orig != TITLE_OWNER_SCAN_START_ADDRESS {
            let f: unsafe extern "system" fn(usize, usize) = unsafe { std::mem::transmute(orig) };
            unsafe { f(owner, framectx) };
        }
    };
    let _ = phase;
    // NO-WRITE CHECKPOINT. The Path A re-target (re-plant b78 / re-SetState(5) on c30=real)
    // is REMOVED: it MISFIRED on the native new-game default c30=0xa010000 and reloaded an
    // m10 null character (pathA-b78-route-falsified-b80-stuck-latch-gate-2026). idx10 no
    // longer SetState(5)s, so this idx6 (state 6) is not reached in normal flow; it remains a
    // pure read-only watcher (no writes) for any future in-context load comparison.
    let _ = (
        &OWN_STEPPER_RETARGETED,
        OWN_STEPPER_RETARGET_NO,
        OWN_STEPPER_RETARGET_YES,
        OWN_STEPPER_SLOT_NONE,
        OWN_STEPPER_B80_IDLE,
        GAME_MAN_C30_UNSET,
        DEFAULT_PLAY_GAME_MAP,
        GAME_MAN_REQUESTED_SLOT_B78_OFFSET,
        GAME_MAN_ARM_FLAG_B72_OFFSET,
        TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET,
        TITLE_OWNER_PLAY_GAME_SLOT_OFFSET,
        TITLE_SET_STATE_RVA,
        TITLE_STEP_PLAY_GAME,
        &OWN_STEPPER_SLOT,
    );
    // WATCH the native load that the idx10 Continue confirm kicked off (state 6
    // GameStepWait). Mirrors the observe snapshot so the in-context load can be compared
    // directly to the real user-driven load: csfeman + MoveMapStep build, mms_state
    // advance (1 MsbLoad -> 2 MsbLoadWait -> 3 WorldResWait), b80 deserialize, c30 -> real
    // map, resmgr + b7c1 (the streaming-enable the real flow sets natively at mms_state=2).
    if n % OWN_STEPPER_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
        let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let ingame = unsafe { *((owner + TITLE_OWNER_JOB_OFFSET) as *const usize) };
        let mms = if ingame != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let mms_state = if mms != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((mms + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        let wrm = if mms != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let resmgr = if wrm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let b7c1 = if resmgr != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        append_autoload_debug(format_args!(
            "own_stepper: idx6 watch #{n} csfeman=0x{csfeman:x} c30=0x{c30:x} ac0={ac0} b80={b80} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{resmgr:x} b7c1={b7c1}"
        ));
    }
    pass6();
}

/// Patch the writable .data idx10 step-fn slot to our handler once the FE-host is at
/// committed state 10. Same thread as the dispatch (game-task), so no race.
pub(crate) unsafe fn own_stepper_patch_once(module_base: usize) {
    if OWN_STEPPER_PATCHED.load(Ordering::SeqCst) != OWN_STEPPER_PATCHED_NO {
        return;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let owner = owner as usize;
    if unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
        != TITLE_STEP_MENU_JOB_WAIT
    {
        return;
    }
    // Optional slot override from the trigger file ("slot=N"); -1/absent => the game's
    // own most-recent selection.
    if let Some(dir) = game_directory_path() {
        if let Ok(content) = std::fs::read_to_string(dir.join("er-effects-own-stepper.txt")) {
            for line in content.lines() {
                if let Some(rest) = line.trim().strip_prefix("slot=") {
                    if let Ok(v) = rest.trim().parse::<i32>() {
                        OWN_STEPPER_SLOT.store(v, Ordering::SeqCst);
                    }
                }
            }
        }
    }
    let slot = module_base + TITLE_STEP_IDX10_SLOT_RVA;
    let orig = unsafe { *(slot as *const usize) };
    OWN_STEPPER_ORIG_IDX10.store(orig, Ordering::SeqCst);
    OWN_STEPPER_BASE.store(module_base, Ordering::SeqCst);
    // Own idx6 (STEP_GameStepWait) too, for the post-SetState(5) deserialize + re-target.
    let slot6 = module_base + TITLE_STEP_IDX6_SLOT_RVA;
    let orig6 = unsafe { *(slot6 as *const usize) };
    OWN_STEPPER_ORIG_IDX6.store(orig6, Ordering::SeqCst);
    unsafe { *(slot6 as *mut usize) = own_stepper_idx6 as usize };
    unsafe { *(slot as *mut usize) = own_stepper_idx10 as usize };
    OWN_STEPPER_PATCHED.store(OWN_STEPPER_PATCHED_YES, Ordering::SeqCst);
    let handler = own_stepper_idx10 as usize;
    let _ = TITLE_STEP_PLAY_GAME;
    append_autoload_debug(format_args!(
        "own_stepper: PATCHED idx10 slot=0x{slot:x} orig=0x{orig:x} -> handler=0x{handler:x} owner=0x{owner:x}"
    ));
}

/// Pure read-only observation (NO forcing, NO SetState) of the title -> menu -> load
/// transition. Logs a full snapshot every OBSERVE_INTERVAL ticks so we can capture
/// exactly what the REAL button press does: the title state sequence, when CSFeMan /
/// session build, when the save mounts (GameMan+0xc30 changes from the default), the
/// InGameStep/MoveMapStep appearance. Ground-truths the menu-build the static RE
/// kept mis-identifying.
pub(crate) unsafe fn title_observe_tick(module_base: usize, tick: u64) {
    let _ = OBSERVE_INTERVAL;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let owner = unsafe { title_owner(module_base) }.map(|p| p as usize);
    let state = match owner {
        Some(o) => unsafe { *((o + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) },
        None => TITLE_STATE_OWNER_GONE,
    };
    // Title->menu timing baseline (works for BOTH a true-vanilla user run and the DLL run):
    // T0 = first frame parked at the title (state 10); T_menu_open = when the TitleTopDialog SM
    // reaches TextFadeOut (menu open -- by the user's presses+modal-dismissals in vanilla). The
    // delta is the apples-to-apples title->ready-menu time to compare against the DLL's headless
    // 3.1s. Read-only (is_in_state is a pure state query).
    if state == TITLE_STEP_MENU_JOB_WAIT
        && owner.is_some()
        && OBSERVE_T0_EMITTED.swap(OBSERVE_MARKER_EMITTED, Ordering::SeqCst)
            == OBSERVE_MARKER_NOT_EMITTED
    {
        timeline_event("T0", tick, format_args!("state10 observe-baseline"));
    }
    if let Some(o) = owner {
        if OBSERVE_MENU_OPEN_EMITTED.load(Ordering::SeqCst) == OBSERVE_MARKER_NOT_EMITTED {
            let dialog =
                unsafe { safe_read_usize(o + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
            let dialog_vt = if dialog != null {
                unsafe { safe_read_usize(dialog) }.unwrap_or(null)
            } else {
                null
            };
            if dialog_vt == module_base + TITLE_TOP_DIALOG_VTABLE_RVA {
                let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
                let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
                    unsafe { std::mem::transmute(module_base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
                let textfadeout =
                    unsafe { is_in_state(sm, module_base + TITLE_STATE_DESC_TEXTFADEOUT_RVA) }
                        != OWN_STEPPER_FALSE;
                if textfadeout
                    && OBSERVE_MENU_OPEN_EMITTED.swap(OBSERVE_MARKER_EMITTED, Ordering::SeqCst)
                        == OBSERVE_MARKER_NOT_EMITTED
                {
                    timeline_event(
                        "T_menu_open",
                        tick,
                        format_args!("dialog=0x{dialog:x} observe-baseline"),
                    );
                }
            }
        }
    }
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let session = unsafe { *((module_base + SESSION_SINGLETON_RVA) as *const usize) };
    let gm = game_man_ptr_or_null();
    let read_gm = |off: usize| {
        if gm != null {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let b78 = read_gm(GAME_MAN_REQUESTED_SLOT_B78_OFFSET);
    // Frame-level save-IO orchestration capture (menu-b80-mount-orchestration-sequence):
    // the iodev request handle pair [iodev+0x18]/[iodev+0x20] + [iodev+0x10] inflight.
    // Only 0x14067b4e0's preview read populates these; logging them across a real
    // load pins EXACTLY when the read goes in-flight/resident vs when b80 flips.
    let iodev = unsafe { *((module_base + IODEV_GLOBAL_RVA) as *const usize) };
    let read_iodev = |off: usize| {
        if iodev != null {
            unsafe { *((iodev + off) as *const usize) }
        } else {
            null
        }
    };
    let iodev10 = read_iodev(IODEV_INFLIGHT_10_OFFSET);
    let iodev18 = read_iodev(IODEV_REQHANDLE_18_OFFSET);
    let iodev20 = read_iodev(IODEV_REQHANDLE_20_OFFSET);
    let ingame = match owner {
        Some(o) => unsafe { *((o + TITLE_OWNER_JOB_OFFSET) as *const usize) },
        None => null,
    };
    let mms = if ingame != null {
        unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) }
    } else {
        null
    };
    let mms_state = if mms != null {
        unsafe { *((mms + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    let slotmgr = game_data_man_ptr_or_null();
    // World-resource streaming enable-state (the WorldResWait resolution gate):
    // resmgr = deref(deref(MoveMapStep+0xf0)+0x10); b7c1 = its streaming-enable flag;
    // driver = the streaming/session driver singleton 0x143d7c088. Capture what the
    // REAL load has enabled during mms_state=3 that our forced load lacks.
    let wrm = if mms != null {
        unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
    } else {
        null
    };
    let resmgr = if wrm != null {
        unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
    } else {
        null
    };
    let b7c1 = if resmgr != null {
        unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    let driver = unsafe { *((module_base + STREAMING_DRIVER_SINGLETON_RVA) as *const usize) };
    // Change-detection: only log when the signature changes (full granularity, no
    // per-frame file I/O). Captures every transition incl. the mms_state 3 -> resolve.
    let csf_nz = (csfeman != null) as i64;
    let sess_nz = (session != null) as i64;
    let ingame_nz = (ingame != null) as i64;
    let driver_nz = (driver != null) as i64;
    let mut sig = state as i64;
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add(mms_state as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(csf_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(sess_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(ingame_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(c30 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b80 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(ac0 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b7c1 as i64);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(driver_nz);
    sig = sig.wrapping_mul(OBSERVE_SIG_MULT).wrapping_add(b78 as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev10 != null) as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev18 != null) as i64);
    sig = sig
        .wrapping_mul(OBSERVE_SIG_MULT)
        .wrapping_add((iodev20 != null) as i64);
    if OBSERVE_LAST_SIG.swap(sig, Ordering::SeqCst) == sig {
        return;
    }
    append_autoload_debug(format_args!(
        "observe: state={state} csfeman=0x{csfeman:x} session=0x{session:x} c30=0x{c30:x} ac0={ac0} b80={b80} b78={b78} iodev=0x{iodev:x} io10=0x{iodev10:x} io18=0x{iodev18:x} io20=0x{iodev20:x} mms=0x{mms:x} mms_state={mms_state} resmgr=0x{resmgr:x} b7c1={b7c1} driver=0x{driver:x} slotmgr=0x{slotmgr:x} tick={tick}"
    ));
}

pub(crate) fn submit_play_game_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_SUBMIT_PLAY_GAME").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-submit-play-game.txt")
        .exists()
}

/// Corrected native play-game submit (play-game-submit-and-continue-load-recipe-2026).
/// On the live FE-host SimpleTitleStep (committed state 10), replicate the Continue/
/// Load handler 0x140b0e180's load branch WITHOUT forcing state: set the slot, clear
/// the new-game flag owner+0x284, write a packed map to owner+0xbc, and call the
/// game's own SetState 0x140b0d960(owner, 5=PlayGame). The existing per-frame pump
/// then runs PlayGame -> child MoveMap_Init -> builds CSFeMan -> loads. Zero input.
/// (force_play_game wrote owner+0x4c=5 raw + a raw slot in +0xbc -> orphaned.)
pub(crate) unsafe fn submit_play_game_once(
    module_base: usize,
    slot: i32,
    tick: u64,
    task_data: &FD4TaskData,
) -> bool {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return false;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let gm = game_man_ptr_or_null();
    let read_c30 = || {
        if gm != null {
            unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let set_state: unsafe extern "system" fn(usize, i32) =
        unsafe { std::mem::transmute(module_base + TITLE_SET_STATE_RVA) };
    match SUBMIT_PLAY_GAME_PHASE.load(Ordering::SeqCst) {
        SUBMIT_PHASE_INIT => {
            // Phase A: deserialize slot N (CSFeMan-less at the title) to set its map,
            // then SetState(5)=PlayGame so the pump builds CSFeMan + the MoveMapStep.
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return false;
            };
            let owner = owner as usize;
            if unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
                != TITLE_STEP_MENU_JOB_WAIT
            {
                return false;
            }
            let set_save_slot: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
            unsafe { set_save_slot(slot) };
            let deserialize: unsafe extern "system" fn(i32) =
                unsafe { std::mem::transmute(module_base + DESERIALIZE_SLOT_RVA) };
            unsafe { deserialize(slot) };
            let c30 = read_c30();
            unsafe {
                *((owner + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *mut u8) =
                    MOVIE_SKIP_FLAG_CLEAR;
                *((owner + TITLE_OWNER_PLAY_GAME_SLOT_OFFSET) as *mut i32) = c30;
            }
            unsafe { set_state(owner, TITLE_STEP_PLAY_GAME) };
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_BUILT, Ordering::SeqCst);
            let _ = TITLE_STEP_BEGIN_TITLE;
            append_autoload_debug(format_args!(
                "submit_play_game: phaseA deserialize+SetState(5) slot={slot} c30=0x{c30:x} tick={tick}"
            ));
        }
        SUBMIT_PHASE_DESER => {
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_BUILT, Ordering::SeqCst);
        }
        SUBMIT_PHASE_BUILT => {
            // Phase C: close the two world-streaming gaps (worldres-loadstate-creator-
            // and-streaming-enable-gate-2026). Gap 1: the spawner built its block-load
            // request from [InGameStep+0x100], which held the wrong coord, so slot 9's
            // m10 load-states were never created -- set the real coord + re-submit via
            // 0x140aed820 so the builder creates them. Gap 2: world-res streaming is
            // disabled ([resmgr+0xb7c1]==0) -- call the virtual enabler 0x14066e2e4 to
            // set it + build the session singletons + start the IO job machine.
            if csfeman == null {
                return true;
            }
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return true;
            };
            let owner = owner as usize;
            let ingame = unsafe { *((owner + TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if ingame == null {
                return true;
            }
            let coord = read_c30();
            unsafe {
                *((ingame + INGAMESTEP_TARGET_COORD_100_OFFSET) as *mut i32) = coord;
            }
            // CORRECT resmgr = deref(deref(MoveMapStep+0xf0)+0x10), vtable 0x142a7e030
            // (NOT InGameStep+0x250, which is the WorldRes-OWNER, vtable 0x142a7de60 --
            // passing that was the prior crash).
            let mms = unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) };
            let wrm = if mms != null {
                unsafe { *((mms + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
            } else {
                null
            };
            let resmgr = if wrm != null {
                unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
            } else {
                null
            };
            // TIMING FIX: the resmgr only exists once the MoveMapStep has spun up
            // (~mms_state 2 in the real load). WAIT for it -- our prior attempts ran
            // at phaseC with resmgr=0x0 and silently skipped the enable.
            if resmgr == null {
                return true;
            }
            let resmgr_vt = unsafe { *(resmgr as *const usize) };
            let b7c1_before =
                unsafe { *((resmgr + RESMGR_STREAM_ENABLE_B7C1_OFFSET) as *const u8) as i32 };
            // Defensive: build the streaming/session driver singleton if somehow null
            // (it is normally built from boot).
            let driver_before =
                unsafe { *((module_base + STREAMING_DRIVER_SINGLETON_RVA) as *const usize) };
            if driver_before == null {
                let build_driver: unsafe extern "system" fn() -> usize =
                    unsafe { std::mem::transmute(module_base + STREAMING_DRIVER_BUILDER_RVA) };
                let _ = unsafe { build_driver() };
            }
            // ENABLE streaming on the live heap resmgr (the one WorldResWait checks) if
            // not already enabled. The REAL load has b7c1=1 here; ours is missing only
            // this bit. 0x14066e2e4 sets +0xb7c1 + builds the 2 session singletons +
            // starts the IO jobs.
            let mut enabled = DIAG_COUNT_ZERO;
            if b7c1_before == DIAG_COUNT_ZERO {
                let enable: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(module_base + STREAMING_ENABLE_RVA) };
                unsafe { enable(resmgr) };
                enabled = DIAG_COUNT_ONE;
            }
            // Re-submit so the builder (re)creates the block load-states.
            let submit_req: unsafe extern "system" fn(usize) =
                unsafe { std::mem::transmute(module_base + REQUEST_SUBMIT_RVA) };
            unsafe { submit_req(ingame) };
            let _ = (
                RESMGR_EXPECTED_VTABLE_RVA,
                INGAMESTEP_RESMGR_250_OFFSET,
                SESSION_SINGLETON_A_RVA,
                SESSION_SINGLETON_B_RVA,
                TITLE_PROCEED_GATE_SET_VALUE,
                LOAD_INITIATOR_RVA,
                WORLD_WORKER_BUILD_RVA,
                SYNTHETIC_STEP_THIS_SIZE,
                SYNTHETIC_STEP_STATE_OFFSET,
                WORLD_WORKER_BUILD_STATE,
                crate::runtime_heap_allocator_ptr_or_null as fn() -> usize,
            );
            SUBMIT_PLAY_GAME_PHASE.store(SUBMIT_PHASE_DONE, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "submit_play_game: phaseC ENABLE resmgr=0x{resmgr:x} vt=0x{resmgr_vt:x} b7c1={b7c1_before} driver=0x{driver_before:x} enabled={enabled} coord=0x{coord:x} tick={tick}"
            ));
        }
        _ => {
            // Phase D (observe): the scheduler ticks CSTaskGroup 20 (MoveMapStep)
            // every frame, so after phaseC initiated the b80 load the game's own
            // b80 machine + MsbLoad drive the stream to resident natively. Watch
            // b80 advance, mms_state -> -1, and child+0xd8 drain 1->2->0. No pumping
            // (direct-pump of 0x140aff640 crashes: movemapstep-direct-pump-crashes).
            let _ = (
                task_data,
                MOVEMAPSTEP_UPDATE_RVA,
                INGAMESTEP_PENDING_D8_PENDING,
            );
            if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL != null as u64 {
                return true;
            }
            let Some(owner) = (unsafe { title_owner(module_base) }) else {
                return true;
            };
            let owner = owner as usize;
            let ingame = unsafe { *((owner + TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if ingame == null {
                return true;
            }
            let d8 = unsafe { *((ingame + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
            let movemapstep =
                unsafe { *((ingame + INGAMESTEP_MOVEMAPSTEP_PTR_OFFSET) as *const usize) };
            let state = unsafe { *((owner + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) };
            let mms_state = if movemapstep != null {
                unsafe { *((movemapstep + TITLE_OWNER_STATE_COMMITTED_OFFSET) as *const i32) }
            } else {
                TITLE_STATE_OWNER_GONE
            };
            let b80 = if gm != null {
                unsafe { *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const i32) }
            } else {
                TITLE_STATE_OWNER_GONE
            };
            let world_a = unsafe { *((module_base + WORLD_SINGLETON_A_RVA) as *const usize) };
            // STEP_WorldResWait inputs: the requested coord [[MoveMapStep+0xf0]+0x2c]
            // (byte3 = target area; 0x0a == m10 requested) and the resmgr loaded-block
            // count [[[MoveMapStep+0xf0]+0x10]+0xb3140].
            let wrm = if movemapstep != null {
                unsafe { *((movemapstep + MOVEMAPSTEP_WORLDRES_F0_OFFSET) as *const usize) }
            } else {
                null
            };
            let coord = if wrm != null {
                unsafe { *((wrm + WORLDRES_COORD_2C_OFFSET) as *const i32) }
            } else {
                DIAG_NULL_CHAIN
            };
            let resmgr = if wrm != null {
                unsafe { *((wrm + WORLDRES_RESMGR_10_OFFSET) as *const usize) }
            } else {
                null
            };
            let blocks = if resmgr != null {
                unsafe { *((resmgr + RESMGR_BLOCK_COUNT_B3140_OFFSET) as *const i32) }
            } else {
                DIAG_NULL_CHAIN
            };
            // Scan the block array for slot 9's target area 0x0a (m10): found10 says
            // whether the block is registered (streaming gap) vs absent (loader gap);
            // sample is the first few blocks' area bytes (likely the title's scene).
            let mut found10 = DIAG_COUNT_ZERO;
            let mut sample = DIAG_SAMPLE_ZERO;
            let mut m10phase = DIAG_PHASE_NONE;
            let mut m10flag = DIAG_PHASE_NONE;
            if resmgr != null && blocks > DIAG_COUNT_ZERO {
                let arr = resmgr + WORLDRES_BLOCK_ARRAY_B3030_OFFSET;
                let n = blocks.min(BLOCK_SCAN_MAX);
                for i in DIAG_COUNT_ZERO..n {
                    let entry =
                        unsafe { *((arr + (i as usize) * BLOCK_ENTRY_STRIDE) as *const usize) };
                    if entry == null {
                        continue;
                    }
                    let areaobj =
                        unsafe { *((entry + BLOCK_ENTRY_AREAOBJ_8_OFFSET) as *const usize) };
                    if areaobj == null {
                        continue;
                    }
                    let area = unsafe { *((areaobj + BLOCK_AREAOBJ_AREA_C_OFFSET) as *const i32) };
                    if area == TARGET_AREA_M10 {
                        found10 += DIAG_COUNT_ONE;
                        // load-state = entry->vtable[+0x10](entry); phase = [+0x35].
                        let vt = unsafe { *(entry as *const usize) };
                        if vt != null {
                            let getter: unsafe extern "system" fn(usize) -> usize = unsafe {
                                std::mem::transmute(
                                    *((vt + BLOCK_LOADSTATE_GETTER_VT_10_OFFSET) as *const usize),
                                )
                            };
                            let ls = unsafe { getter(entry) };
                            if ls != null {
                                m10flag = unsafe {
                                    *((ls + BLOCK_LOADSTATE_FLAG_2D_OFFSET) as *const u8) as i32
                                };
                                m10phase = unsafe {
                                    *((ls + BLOCK_LOADSTATE_PHASE_35_OFFSET) as *const u8) as i32
                                };
                            }
                        }
                    }
                    if (i as usize) < BLOCK_SAMPLE_COUNT {
                        sample |= ((area as u32) & BLOCK_AREA_BYTE_MASK)
                            << ((i as u32) * BLOCK_SAMPLE_SHIFT);
                    }
                }
            }
            append_autoload_debug(format_args!(
                "submit_play_game: phaseD state={state} mms_state={mms_state} blocks={blocks} found10={found10} m10phase={m10phase} m10flag={m10flag} sample=0x{sample:x} reqcoord=0x{coord:x} child_d8={d8} csfeman=0x{csfeman:x} tick={tick}"
            ));
            let _ = (world_a, b80);
        }
    }
    true
}

pub(crate) fn ingameinit_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMEINIT_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingameinit-drive.txt")
        .exists()
}

pub(crate) fn continue_drive_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_CONTINUE_DRIVE").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-continue-drive.txt")
        .exists()
}

pub(crate) fn arm_probe_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_ARM_PROBE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-arm-probe.txt")
            .exists()
}

pub(crate) fn native_arm_loop_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_NATIVE_ARM_LOOP").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-native-arm-loop.txt")
        .exists()
}

pub(crate) fn title_accept_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_TITLE_ACCEPT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-title-accept.txt")
            .exists()
}

pub(crate) fn title_accept_inject_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TITLE_ACCEPT_INJECT").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-title-accept-inject.txt")
        .exists()
}

pub(crate) fn splash_skip_enabled() -> bool {
    // Splash-skip is a MAIN PRODUCT FEATURE (faster boot to the title), on for every load path, not a
    // manual toggle. It is safe because the "jumped too far" failure mode -- the BeginLogo branch-flip
    // also skips the main-menu list build, leaving an empty menu -- only matters when a path NEEDS the
    // main menu. The product's load paths do not: product autoload rebuilds the menu itself
    // (SetState(2) + clear [owner+0xb8]), and own-load BYPASSES the menu entirely (it slices the .sl2
    // and calls the native parser directly). So enabling splash-skip whenever a load path is armed
    // speeds up our runs without re-introducing the empty-menu break. Plain vanilla play (no load path,
    // no env/file) is unaffected and still builds the full menu.
    product_autoload_enabled()
        || own_load_enabled()
        || matches!(std::env::var("ER_EFFECTS_SPLASH_SKIP").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-splash-skip.txt")
            .exists()
}

/// Force OFFLINE boot (no online login attempt -> no "Unable to start in online mode" modal),
/// so the headless autoload reaches the real title/main-menu directly. Auto-on whenever the
/// own-stepper drives the front-end (the autoload runs vanilla-OFFLINE), plus explicit overrides.
/// Gated (not always-on) so it never forces offline on a co-op/online launch that wants the
/// getter live.
pub(crate) fn online_disable_enabled() -> bool {
    own_stepper_enabled()
        || matches!(std::env::var("ER_EFFECTS_OFFLINE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-offline.txt")
            .exists()
}

/// Patch the `GameMan::IsOnlineMode` getter 0x14067a030 to `xor eax,eax; ret` so it always
/// reports OFFLINE. Validates the expected first opcode byte (aborts if the binary differs),
/// VirtualProtects the 3-byte stub region RWX, writes the stub, restores protection, and
/// flushes the instruction cache. Spawned early at DLL attach (timing-independent: it changes
/// what the function RETURNS, not a data field, so it works whether GameMan is constructed yet
/// or not). Mirrors `apply_splash_skip`. Equivalent to the player choosing "Play Offline" --
/// no save access, no struct mutation, no crash risk.
pub(crate) fn apply_online_disable() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("online-disable: module base unavailable"));
        return;
    };
    // Patch the IsOnlineMode getter (consumers read offline). NOTE: the login-readiness predicate
    // patch (0x140cab230) was REVERTED -- it did not prevent the modal (the offline fork shows it
    // too) AND it broke the OnDecide OK-dispatch (the modal stuck instead of proceeding).
    apply_xor_ret_stub(base, ONLINE_DISABLE_RVA, "IsOnlineMode getter");
    let _ = ONLINE_PREDICATE_DISABLE_RVA;
}

/// Force `CS::CSWindowImp::IsGameInForeground` (0x14266def0) to always return true (`mov al,1; ret`)
/// so the engine's flip pacer never applies the unfocused-window fps throttle -- the probe boots at
/// full speed regardless of focus (bd runtime-probe-unfocused-window-throttle). Same RWX/flush
/// pattern as the online-disable patch; validates the expected 0x40 prologue first.
pub(crate) fn apply_foreground_force() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("foreground-force: module base unavailable"));
        return;
    };
    let target = (base + FOREGROUND_FORCE_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != FOREGROUND_FORCE_EXPECTED_FIRST {
        append_autoload_debug(format_args!(
            "foreground-force: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{FOREGROUND_FORCE_EXPECTED_FIRST:x}",
            base + FOREGROUND_FORCE_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("foreground-force: VirtualProtect failed"));
        return;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = FOREGROUND_FORCE_STUB[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    append_autoload_debug(format_args!(
        "foreground-force: patched IsGameInForeground 0x{:x} -> mov al,1;ret (no unfocused fps throttle)",
        base + FOREGROUND_FORCE_RVA
    ));
}

/// Write a self-contained 3-byte return stub at `base+rva` after validating the expected first
/// byte. RWX via VirtualProtect, write, restore, icache flush. Returns true on success. Shared by
/// the gate-force patches (foreground / sign-in / user-index).
fn patch_3byte_stub(
    base: usize,
    rva: usize,
    expected_first: u8,
    stub: [u8; 3],
    label: &str,
) -> bool {
    let target = (base + rva) as *mut u8;
    let existing = unsafe { *target };
    if existing != expected_first {
        append_autoload_debug(format_args!(
            "{label}: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{expected_first:x}",
            base + rva
        ));
        return false;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("{label}: VirtualProtect failed"));
        return false;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = stub[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    true
}

/// Force the SaveLoad2 storage-select op gate to pass cold (bd b80-ROOTCAUSE-cold-no-user-signin):
/// patch the sign-in check to always return true and the user-index resolver to return 0, so the
/// select-op ctor (0x14240f1b0) builds the runnable and the load proceeds to SLLoadSession -> read
/// -> b80 RESIDENT. Save-safe (in-memory code patch; no save write). Called once from the cold-mount
/// attempt so normal play is unaffected unless a cold mount is requested.
pub(crate) fn apply_signin_force(base: usize) {
    let s = patch_3byte_stub(
        base,
        SIGNIN_FORCE_RVA,
        SIGNIN_FORCE_EXPECTED_FIRST,
        SIGNIN_FORCE_STUB,
        "signin-force",
    );
    let u = patch_3byte_stub(
        base,
        USERINDEX_FORCE_RVA,
        USERINDEX_FORCE_EXPECTED_FIRST,
        USERINDEX_FORCE_STUB,
        "userindex-force",
    );
    append_autoload_debug(format_args!(
        "signin-force: signin@0x{:x} ok={s} -> mov al,1;ret | userindex@0x{:x} ok={u} -> xor eax,eax;ret (select-op gate now passes: signed-in as user 0)",
        base + SIGNIN_FORCE_RVA,
        base + USERINDEX_FORCE_RVA
    ));
}

/// Patch a 0x48-prologue function body to `xor eax,eax; ret` (return 0) at `base+rva`. Validates
/// the expected first byte, VirtualProtects RWX, writes the 3-byte stub, restores protection, and
/// flushes the icache. Used to force-offline the IsOnlineMode getter + login-readiness predicate.
fn apply_xor_ret_stub(base: usize, rva: usize, label: &str) {
    let target = (base + rva) as *mut u8;
    let existing = unsafe { *target };
    if existing != ONLINE_DISABLE_EXPECTED_FIRST {
        append_autoload_debug(format_args!(
            "online-disable: ABORT {label} -- byte at 0x{:x} is 0x{existing:x}, expected 0x{ONLINE_DISABLE_EXPECTED_FIRST:x}",
            base + rva
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!(
            "online-disable: VirtualProtect failed for {label}"
        ));
        return;
    }
    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
    while i < ONLINE_DISABLE_PATCH_LEN {
        unsafe { *target.add(i) = ONLINE_DISABLE_STUB[i] };
        i += ONLINE_DISABLE_BYTE_STEP;
    }
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            ONLINE_DISABLE_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    unsafe {
        FlushInstructionCache(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            target as *const c_void,
            ONLINE_DISABLE_PATCH_LEN,
        )
    };
    append_autoload_debug(format_args!(
        "online-disable: patched {label} 0x{:x} -> xor eax,eax;ret (forces offline)",
        base + rva
    ));
}

// (The 0x1407b0cf0 "finished-poll" auto-accept hook was removed: RE showed 0x1407b0cf0 is a
// "has >= 2 buttons" layout query, not a finished-poll -- it is never called for the
// connection-error dialog, and writing +0x25e0/+0x25e8 corrupts the dialog (+0x25e8 is the
// button COUNT). The dismiss is force_dismiss_startup_dialog -> OnDecide 0x140927ba0.)

/// DIAGNOSTIC detour for the dialog builder 0x1409275b0 (4 register args rcx/rdx/r8/r9 -> dialog
/// in rax). Calls the original, then (pre-world, capped) logs the BUILT dialog's vtable/class +
/// the 4 args (the FMG message id is one of them) + caller, so we can identify the actual
/// connection-error dialog without guessing. Read-only; never mutates the dialog.
unsafe fn policy_tos_record_fields(record: usize) -> (usize, usize, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if record == null {
        return (null, null, null);
    }
    let record_id = unsafe { safe_read_i32(record) }
        .map(|value| value.max(0) as usize)
        .unwrap_or(null);
    let stack_arg0 = unsafe { safe_read_i32(record + 0x4) }
        .map(|value| value.max(0) as usize)
        .unwrap_or(null);
    let backing_flag_ptr = unsafe { safe_read_usize(record + 0x8) }.unwrap_or(null);
    (record_id, stack_arg0, backing_flag_ptr)
}

/// Operator gate for zero-input ToS-modal suppression. Default OFF: the wrapper builds the
/// TosMultiLangDialog as the game normally would. When enabled (only on a profile where the
/// Terms of Service is already accepted), `policy_tos_title_ctor_wrapper_hook` skips the
/// build and returns null, so the unnecessary startup ToS modal is never constructed -- no
/// input, no auto-accept of an un-accepted policy, no MessageBox.
pub(crate) fn policy_tos_suppress_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_POLICY_TOS_SUPPRESS").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-policy-tos-suppress.txt")
        .exists()
}

pub(crate) unsafe extern "system" fn policy_tos_title_ctor_wrapper_hook(
    record: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let (record_id, stack_arg0, backing_flag_ptr) = unsafe { policy_tos_record_fields(record) };
    let original_this = record.saturating_sub(POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST);
    let original_vtable = if original_this != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(original_this) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let caller_rva = trace_first_game_caller_rva();
    let backing_flag_value = if backing_flag_ptr != null {
        unsafe { safe_read_usize(backing_flag_ptr) }.unwrap_or(0)
    } else {
        0
    };
    let orig = POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG.load(Ordering::SeqCst);
    let ret = if policy_tos_suppress_enabled() {
        // Replace the native "show ToS" stepper with our own no-op: skip building the
        // TosMultiLangDialog and return null, mimicking the wrapper's native allocation-
        // failure path (caller-tolerated). The ToS ctor 0x1409b5970 -- whose only caller is
        // this wrapper -- never runs, so the policy/ToS ctor hook never fires and
        // POLICY_TOS_TITLE_TOTAL_BUILDS stays 0: the unnecessary startup modal is never
        // constructed. Zero input, no auto-accept.
        POLICY_TOS_TITLE_SUPPRESSED_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "policy-oracle: SUPPRESSED TosMultiLangDialog build (wrapper 0x{:x}) -> returned null (native alloc-fail path) record=0x{record:x} backing_flag_ptr=0x{backing_flag_ptr:x} backing_flag_value={backing_flag_value} -- zero-input ToS-modal suppression",
            game_module_base().unwrap_or(null) + POLICY_TOS_TITLE_CTOR_WRAPPER_RVA as usize,
        ));
        POLICY_TOS_MODAL_SUPPRESSED_RETURN
    } else if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(record, rdx, r8) }
    };
    POLICY_TOS_TITLE_WRAPPER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_RECORD.store(record, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS.store(original_this, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE.store(original_vtable, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_RECORD_ID.store(record_id, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_STACK_ARG0.store(stack_arg0, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR.store(backing_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_RET.store(ret, Ordering::SeqCst);
    POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn policy_tos_selector_wrapper_hook(record: usize) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let owner = if record != null {
        unsafe { safe_read_usize(record) }.unwrap_or(null)
    } else {
        null
    };
    let requested_flag = if owner != null {
        unsafe { safe_read_i32(owner + 0x29c8) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    let selector_arg = if owner != null { owner + 0x29d0 } else { null };
    let original_this = record.saturating_sub(POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST);
    let original_vtable = if original_this != null {
        unsafe { safe_read_usize(original_this) }.unwrap_or(null)
    } else {
        null
    };
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_SELECTOR_WRAPPER_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize) -> usize = unsafe { std::mem::transmute(orig) };
        unsafe { f(record) }
    };
    POLICY_TOS_SELECTOR_WRAPPER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_RECORD.store(record, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_THIS.store(original_this, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_ORIGINAL_VTABLE.store(original_vtable, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG.store(requested_flag, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG.store(selector_arg, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_RET.store(ret, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn policy_tos_selector_ctor_hook(
    this: usize,
    rdx: usize,
    r8: usize,
    selector_arg: usize,
    requested_flag_ptr: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let requested_flag_value = if requested_flag_ptr != null {
        unsafe { safe_read_i32(requested_flag_ptr) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    let owner = selector_arg.saturating_sub(0x29d0);
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_SELECTOR_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize, usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, rdx, r8, selector_arg, requested_flag_ptr) }
    };
    let object = if ret != null { ret } else { this };
    let vt = if object != null {
        unsafe { safe_read_usize(object) }.unwrap_or(null)
    } else {
        null
    };
    let stored_selector_arg = if object != null {
        unsafe { safe_read_usize(object + 0x1260) }.unwrap_or(null)
    } else {
        null
    };
    let stored_requested_flag_ptr = if object != null {
        unsafe { safe_read_usize(object + 0x1268) }.unwrap_or(null)
    } else {
        null
    };
    POLICY_TOS_SELECTOR_CTOR_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_THIS.store(object, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_VTABLE.store(vt, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR.store(requested_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_VALUE
        .store(requested_flag_value, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_SELECTOR_ARG.store(selector_arg, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_STORED_SELECTOR_ARG.store(stored_selector_arg, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR
        .store(stored_requested_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_RET.store(ret, Ordering::SeqCst);
    POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

unsafe fn policy_tos_flag_value(owner: usize) -> (usize, usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let flag_ptr = if owner != null {
        unsafe { safe_read_usize(owner + 0x29c0) }.unwrap_or(null)
    } else {
        null
    };
    let flag_value = if flag_ptr != null {
        unsafe { safe_read_i32(flag_ptr) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    (flag_ptr, flag_value)
}

pub(crate) unsafe extern "system" fn policy_tos_flag_setter_hook(
    owner: usize,
    value: i32,
    force: u8,
) {
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_FLAG_SETTER_ORIG.load(Ordering::SeqCst);
    let (_, before) = unsafe { policy_tos_flag_value(owner) };
    if orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn(usize, i32, u8) = unsafe { std::mem::transmute(orig) };
        unsafe { f(owner, value, force) };
    }
    let (_, after) = unsafe { policy_tos_flag_value(owner) };
    POLICY_TOS_FLAG_SETTER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_VALUE.store(value.max(0) as usize, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_FORCE.store(force as usize, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_BEFORE.store(before, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_AFTER.store(after, Ordering::SeqCst);
    POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
}

pub(crate) unsafe extern "system" fn policy_tos_status_predicate_hook(this: usize) -> u8 {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_STATUS_PREDICATE_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        0
    } else {
        let f: unsafe extern "system" fn(usize) -> u8 = unsafe { std::mem::transmute(orig) };
        unsafe { f(this) }
    };
    let owner = unsafe { safe_read_usize(this + core::mem::size_of::<usize>()) }.unwrap_or(null);
    let (flag_ptr, flag_value) = unsafe { policy_tos_flag_value(owner) };
    POLICY_TOS_STATUS_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_THIS.store(this, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_OWNER.store(owner, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_FLAG_PTR.store(flag_ptr, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_FLAG_VALUE.store(flag_value, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_RET.store(ret as usize, Ordering::SeqCst);
    POLICY_TOS_STATUS_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    ret
}

pub(crate) unsafe extern "system" fn policy_tos_title_ctor_hook(
    this: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
    stack_arg0: usize,
    backing_flag_ptr: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let caller_rva = trace_first_game_caller_rva();
    let orig = POLICY_TOS_TITLE_CTOR_ORIG.load(Ordering::SeqCst);
    let ret = if orig == HOOK_ORIGINAL_UNSET {
        null
    } else {
        let f: unsafe extern "system" fn(usize, usize, usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(this, rdx, r8, r9, stack_arg0, backing_flag_ptr) }
    };
    let base = game_module_base().unwrap_or(null);
    let object = if ret != null { ret } else { this };
    let vt = if object != null {
        unsafe { safe_read_usize(object) }.unwrap_or(null)
    } else {
        null
    };
    let stored_backing_flag_ptr = if object != null {
        unsafe { safe_read_usize(object + 0x29c0) }.unwrap_or(null)
    } else {
        null
    };
    let backing_flag_value = if stored_backing_flag_ptr != null {
        unsafe { safe_read_i32(stored_backing_flag_ptr) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    let requested_flag_value = if object != null {
        unsafe { safe_read_i32(object + 0x29c8) }
            .map(|value| value.max(0) as usize)
            .unwrap_or(null)
    } else {
        null
    };
    POLICY_TOS_TITLE_LAST_THIS.store(object, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_VTABLE.store(vt, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_ARG_RDX.store(rdx, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_ARG_R8.store(r8, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_ARG_R9.store(r9, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_STACK_ARG0.store(stack_arg0, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR.store(backing_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR.store(stored_backing_flag_ptr, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE.store(backing_flag_value, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE.store(requested_flag_value, Ordering::SeqCst);
    POLICY_TOS_TITLE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    POLICY_TOS_TITLE_TOTAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    write_policy_oracle_snapshot("tos_title_ctor");
    append_autoload_debug(format_args!(
        "policy-oracle: TosTitle ctor 0x{:x} built object=0x{object:x} vt=0x{vt:x} expected_vt=0x{:x} args(rdx=0x{rdx:x} r8=0x{r8:x} r9=0x{r9:x} stack0=0x{stack_arg0:x} backing_flag_ptr=0x{backing_flag_ptr:x}) stored_backing_flag_ptr=0x{stored_backing_flag_ptr:x} backing_flag_value={backing_flag_value} requested_flag_value={requested_flag_value} text_path=0x{:x} -- native/asset-backed Privacy/ToS surface regression",
        base + POLICY_TOS_TITLE_CTOR_RVA as usize,
        base + POLICY_TOS_TITLE_VTABLE_RVA,
        base + POLICY_TOS_TITLE_TEXT_PATH_RVA
    ));
    ret
}

pub(crate) fn install_policy_tos_title_hook() {
    if POLICY_TOS_TITLE_HOOK_INSTALLED.load(Ordering::SeqCst) != POLICY_TOS_TITLE_HOOK_NOT_INSTALLED
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "policy-oracle: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(wrapper_addr) = game_rva(POLICY_TOS_TITLE_CTOR_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS ctor wrapper rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            wrapper_addr as *mut c_void,
            policy_tos_title_ctor_wrapper_hook as *mut c_void,
        )
    } {
        POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS ctor wrapper failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(selector_wrapper_addr) = game_rva(POLICY_TOS_SELECTOR_WRAPPER_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS selector wrapper rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            selector_wrapper_addr as *mut c_void,
            policy_tos_selector_wrapper_hook as *mut c_void,
        )
    } {
        POLICY_TOS_SELECTOR_WRAPPER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS selector wrapper failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(selector_ctor_addr) = game_rva(POLICY_TOS_SELECTOR_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS selector ctor rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            selector_ctor_addr as *mut c_void,
            policy_tos_selector_ctor_hook as *mut c_void,
        )
    } {
        POLICY_TOS_SELECTOR_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS selector ctor failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(predicate_addr) = game_rva(POLICY_TOS_STATUS_PREDICATE_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS status predicate rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            predicate_addr as *mut c_void,
            policy_tos_status_predicate_hook as *mut c_void,
        )
    } {
        POLICY_TOS_STATUS_PREDICATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS status predicate failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(flag_setter_addr) = game_rva(POLICY_TOS_FLAG_SETTER_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve ToS flag setter rva"
        ));
        return;
    };
    if let Ok(hook) = unsafe {
        MhHook::new(
            flag_setter_addr as *mut c_void,
            policy_tos_flag_setter_hook as *mut c_void,
        )
    } {
        POLICY_TOS_FLAG_SETTER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
        if let Err(status) = unsafe { hook.queue_enable() } {
            append_autoload_debug(format_args!(
                "policy-oracle: queue_enable ToS flag setter failed: {status:?}"
            ));
        } else {
            std::mem::forget(hook);
        }
    }
    let Ok(ctor_addr) = game_rva(POLICY_TOS_TITLE_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "policy-oracle: failed to resolve TosTitle ctor rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            policy_tos_title_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            POLICY_TOS_TITLE_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "policy-oracle: queue_enable TosTitle ctor failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    POLICY_TOS_TITLE_HOOK_INSTALLED
                        .store(POLICY_TOS_TITLE_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "policy-oracle: hooked TosTitle ctor 0x{ctor_addr:x}, ctor wrapper 0x{wrapper_addr:x}, selector wrapper 0x{selector_wrapper_addr:x}, selector ctor 0x{selector_ctor_addr:x}, status predicate 0x{predicate_addr:x}, and flag setter 0x{flag_setter_addr:x} (native Privacy/ToS surface oracle)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "policy-oracle: MH_ApplyQueued TosTitle ctor failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "policy-oracle: MhHook::new TosTitle ctor failed: {status:?}"
        )),
    }
}

pub(crate) fn server_status_text_id_is_product_failure(text_id: usize) -> bool {
    matches!(
        text_id,
        SERVER_STATUS_CHECKING_NETWORK_TEXT_ID
            | SERVER_STATUS_LOGGING_IN_TEXT_ID
            | SERVER_STATUS_RETRIEVING_DATA_TEXT_ID
            | SERVER_STATUS_SAVING_DATA_TEXT_ID
    )
}

pub(crate) unsafe extern "system" fn server_status_formatter_hook(
    record_slot: usize,
    out_text: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let record = unsafe { safe_read_usize(record_slot) }.unwrap_or(null);
    if record != null {
        let state = unsafe { safe_read_i32(record + SERVER_STATUS_RECORD_STATE_OFFSET) }
            .unwrap_or(-1)
            .max(0) as usize;
        let text_id = unsafe { safe_read_i32(record + SERVER_STATUS_RECORD_TEXT_ID_OFFSET) }
            .unwrap_or(-1)
            .max(0) as usize;
        if server_status_text_id_is_product_failure(text_id) {
            SERVER_STATUS_TOTAL_SEEN.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            SERVER_STATUS_LAST_STATE.store(state, Ordering::SeqCst);
            SERVER_STATUS_LAST_TEXT_ID.store(text_id, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "server-status-oracle: state={state} text_id={text_id} via formatter 0x{:x} -- invalid online/login status semaphore {}",
                game_module_base().unwrap_or(null) + SERVER_STATUS_FORMATTER_RVA as usize,
                trace_callers_summary()
            ));
        }
    }
    let orig = SERVER_STATUS_FORMATTER_ORIG.load(Ordering::SeqCst);
    if orig == null {
        return out_text;
    }
    let f: unsafe extern "system" fn(usize, usize) -> usize = unsafe { std::mem::transmute(orig) };
    unsafe { f(record_slot, out_text) }
}

pub(crate) fn install_server_status_hook() {
    if SERVER_STATUS_HOOK_INSTALLED.load(Ordering::SeqCst) != SERVER_STATUS_HOOK_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "server-status-oracle: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(formatter_addr) = game_rva(SERVER_STATUS_FORMATTER_RVA) else {
        append_autoload_debug(format_args!(
            "server-status-oracle: failed to resolve formatter rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            formatter_addr as *mut c_void,
            server_status_formatter_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SERVER_STATUS_FORMATTER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "server-status-oracle: queue_enable formatter failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    SERVER_STATUS_HOOK_INSTALLED
                        .store(SERVER_STATUS_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "server-status-oracle: hooked formatter 0x{formatter_addr:x} (server/login semaphore oracle)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "server-status-oracle: MH_ApplyQueued formatter failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "server-status-oracle: MhHook::new formatter failed: {status:?}"
        )),
    }
}

/// Read a DLW (UTF-16 / char16_t) `basic_string` at `s` and return up to `max_chars` of its text.
/// Layout: [+0x10]=length (chars), [+0x18]=capacity (chars); the text is inline at `s` when capacity
/// < 8, else `*(s)` points at the heap buffer. Every read is fault-guarded so a garbage Spec field can
/// never AV the game thread. UTF-16 lossy decode (the repo no-lossy lint targets from_utf8_lossy only).
unsafe fn read_dlw_string(s: usize, max_chars: usize) -> Option<String> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if s <= null {
        return None;
    }
    let length = unsafe { safe_read_usize(s + 0x10) }?;
    let capacity = unsafe { safe_read_usize(s + 0x18) }?;
    if length == null || length > 4096 {
        return None;
    }
    let take = length.min(max_chars);
    let text_ptr = if capacity < 8 {
        s
    } else {
        unsafe { safe_read_usize(s) }?
    };
    if text_ptr <= null {
        return None;
    }
    let mut buf: Vec<u16> = Vec::with_capacity(take);
    for i in 0..take {
        let w = (unsafe { safe_read_usize(text_ptr + i * 2) }? & 0xffff) as u16;
        if w == 0 {
            break;
        }
        buf.push(w);
    }
    if buf.is_empty() {
        return None;
    }
    Some(String::from_utf16_lossy(&buf))
}

/// Diagnostic: dump the MessageBoxDialog builder Spec (`r8`) to NAME the modal's message. The text id
/// is NOT in rdx/r9 (a pointer pair 0x40 apart) and is NOT fetched via GetGR_System_Message at build
/// time, so read it straight from the Spec. Tries the reported MenuString offset (+0x8e0) plus a scan
/// of early offsets for any embedded/pointed-to DLW string. Read-only; logs each decoded string.
unsafe fn dump_msgbox_spec(c: usize, n: usize) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if c <= null {
        return;
    }
    if let Some(text) =
        unsafe { read_dlw_string(unsafe { safe_read_usize(c + 0x8e0) }.unwrap_or(null), 80) }
    {
        append_autoload_debug(format_args!("spec #{n}: text@*(r8+0x8e0)=\"{text}\""));
    }
    let mut off = 0usize;
    while off < 0x120 {
        // Inline DLW string at r8+off.
        if let Some(text) = unsafe { read_dlw_string(c + off, 80) } {
            append_autoload_debug(format_args!("spec #{n}: inline[r8+0x{off:x}]=\"{text}\""));
        }
        // Pointer-to-DLW-string at r8+off.
        if let Some(ptr) = unsafe { safe_read_usize(c + off) } {
            if let Some(text) = unsafe { read_dlw_string(ptr, 80) } {
                append_autoload_debug(format_args!("spec #{n}: *[r8+0x{off:x}]=\"{text}\""));
            }
        }
        off += 8;
    }
}

pub(crate) unsafe extern "system" fn msgbox_builder_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if product_autoload_enabled() {
        MSGBOX_LAST_ARG_RCX.store(a, Ordering::SeqCst);
        MSGBOX_LAST_ARG_RDX.store(b, Ordering::SeqCst);
        MSGBOX_LAST_ARG_R8.store(c, Ordering::SeqCst);
        MSGBOX_LAST_ARG_R9.store(d, Ordering::SeqCst);
        MSGBOX_TOTAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES {
            MSGBOX_POSTLOAD_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        }
        let n = MSGBOX_BUILDER_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < MSGBOX_BUILDER_LOG_MAX {
            append_autoload_debug(format_args!(
                "msgbox-skip #{n}: product autoload suppressed MessageBoxDialog builder before UI allocation but counted it as oracle failure args(rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x}) {}",
                trace_callers_summary()
            ));
        }
        return null;
    }
    let orig = MSGBOX_BUILDER_ORIG.load(Ordering::SeqCst);
    let ret = if orig != null {
        let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(a, b, c, d) }
    } else {
        null
    };
    if ret != null {
        let base = {
            let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
            if own != null {
                own
            } else {
                game_module_base().unwrap_or(null)
            }
        };
        let vt = unsafe { safe_read_usize(ret) }.unwrap_or(null);
        let is_msgbox = vt == base + MSGBOX_DIALOG_VTABLE_RVA;
        let in_world = IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES;
        // CAPTURE the startup MessageBoxDialog (connection-error / EULA / warning) pre-world so
        // the game task can dismiss it via the real OK handler. Post-load/in-world dialogs are
        // NEVER auto-dismissed; they are only latched for telemetry so the oracle fails instead of
        // reporting a false 1400 when a blocking popup remains on screen.
        if is_msgbox {
            MSGBOX_LAST_DIALOG.store(ret, Ordering::SeqCst);
            MSGBOX_LAST_ARG_RCX.store(a, Ordering::SeqCst);
            MSGBOX_LAST_ARG_RDX.store(b, Ordering::SeqCst);
            MSGBOX_LAST_ARG_R8.store(c, Ordering::SeqCst);
            MSGBOX_LAST_ARG_R9.store(d, Ordering::SeqCst);
            MSGBOX_TOTAL_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            if in_world {
                MSGBOX_POSTLOAD_BUILDS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            } else {
                CONNECTION_ERROR_DIALOG.store(ret, Ordering::SeqCst);
            }
        }
        let n = MSGBOX_BUILDER_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < MSGBOX_BUILDER_LOG_MAX {
            let vt_rva = vt.wrapping_sub(base);
            append_autoload_debug(format_args!(
                "msgbox-builder #{n}: dialog=0x{ret:x} vt=0x{vt:x} vt_rva=0x{vt_rva:x} captured={is_msgbox} in_world={in_world} args(rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x}) {}",
                trace_callers_summary()
            ));
            // NAME the modal: read its message text straight from the Spec (r8=c).
            unsafe { dump_msgbox_spec(c, n) };
        }
    }
    ret
}

/// Dismiss the captured startup MessageBoxDialog (connection-error / EULA / warning) by calling
/// its verified OnDecide/finalize 0x140927ba0(rcx=dialog) -- the genuine OK handler that
/// dispatches the chosen button (builder-defaulted to OK) and drives the dialog to emit "stop"
/// so the parent MenuWindowJob tears it down. Called each frame pre-in-world from the game task
/// (the menu/game thread, where OnDecide's input-registrar singleton access is valid) UNTIL the
/// closing latch [dialog+0x3b0]==1 or the dialog is freed/reused (vtable mismatch) -- both stop
/// the calls, avoiding re-dispatch / UAF. Fault-tolerant reads never AV.
pub(crate) fn force_dismiss_startup_dialog() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let dialog = CONNECTION_ERROR_DIALOG.load(Ordering::SeqCst);
    if dialog == null {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != null {
            own
        } else {
            game_module_base().unwrap_or(null)
        }
    };
    let vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    if base == null || !is_startup_msgbox_vtable(vt, base) {
        // Dialog consumed/freed/reused -> stop (and let the builder hook re-capture a new one).
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        return;
    }
    // Stop once the dialog has begun teardown (EmitResult set the closing latch) -- calling
    // OnDecide again risks re-dispatch / UAF as the job frees it.
    let closing = unsafe { safe_read_usize(dialog + MSGBOX_CLOSING_LATCH_3B0_OFFSET) }
        .map(|v| v & MSGBOX_LATCH_BYTE_MASK)
        .unwrap_or(MSGBOX_CLOSING_YES);
    if closing == MSGBOX_CLOSING_YES {
        CONNECTION_ERROR_DIALOG.store(null, Ordering::SeqCst);
        let n = DISMISS_WRITE_LOG.load(Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "auto-accept: MessageBoxDialog 0x{dialog:x} closing (latch+0x3b0=1) after {n} OnDecide calls -- dismissed"
        ));
        return;
    }
    // Drive the dialog Decided + OK + fade-complete BEFORE the OK-handler so (a) the title-flow's
    // modal-build poll ([dialog+0x25e8]>0 at 0x1407b04f5) treats it as resolved and PROCEEDS to the
    // menu, and (b) the OK-handler's fade gate (commit only when fade_current<=fade_target) fires THIS
    // frame -> instant commit/close, no fade-in render = no flash (vs the ~20 OnDecide frames before).
    // The dialog is vtable-validated above (base MessageBoxDialog OR SaveRetryDialog). bd
    // press-any-button-golden-lever-job1e8-readiness-2026-06-23 + offline-title-modal-is-saveretrydialog.
    unsafe {
        *((dialog + MSGBOX_STATE_25E8_OFFSET) as *mut i32) = MSGBOX_STATE_DECIDED;
        *((dialog + MSGBOX_RESULT_BUTTON_25E0_OFFSET) as *mut i32) = MSGBOX_OK_BUTTON;
    }
    if let Some(fade_target_bits) =
        unsafe { safe_read_i32(dialog + MSGBOX_FADE_TARGET_2300_OFFSET) }
    {
        unsafe {
            *((dialog + MSGBOX_FADE_CURRENT_1278_OFFSET) as *mut i32) = fade_target_bits;
        }
    }
    // PROPER OK (NOT force-stop): OnDecide 0x140927ba0 branches on the chosen button [dialog+0x25e0]
    // -- if == -1 it calls 0x14078dfd0 (the CANCEL/notify-closed path, which kicks the title flow
    // BACK to PRESS-ANY-BUTTON); if != -1 it DISPATCHES that button (= press OK -> proceed to the
    // main menu offline). The prior force-stop 0x14078dfd0 was exactly the cancel path, so the game
    // bounced back to press-any-button. Fix: set the chosen button to OK (index 0), then OnDecide.
    // Press OK EVERY FRAME (runtime-confirmed: one-shot only HIGHLIGHTS OK; the modal needs the
    // per-frame re-dispatch to progress its decide animation -> activate -> close -> proceed to
    // the main menu). [dialog+0x25e0]=0 selects OK so OnDecide takes the dispatch (NOT cancel) arm.
    // Call THE REAL OK-BUTTON HANDLER 0x14078e030(rcx=dialog) -- captured from a live OK-press.
    // It reads the dialog cursor, gets the OK callback, and COMMITS (0x14078ef20) which actually
    // CLOSES the dialog and emits its result so the title flow PROCEEDS. This is what a real OK
    // does; OnDecide/field-writes/input-injection all failed to close it. Runs each frame on every
    // captured MessageBoxDialog -> skips ALL of them (connection-error, starting-offline, ...).
    let ok_handler: unsafe extern "system" fn(usize) =
        unsafe { std::mem::transmute(base + MSGBOX_OK_HANDLER_RVA) };
    unsafe { ok_handler(dialog) };
    let n = DISMISS_WRITE_LOG.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n % AUTO_ACCEPT_LOG_INTERVAL == null {
        append_autoload_debug(format_args!(
            "auto-accept: OK-handler 0x{:x}(MessageBoxDialog 0x{dialog:x}) -- real OK-press to close + proceed #{n}",
            base + MSGBOX_OK_HANDLER_RVA
        ));
    }
    let _ = (
        &LAST_ONDECIDE_DIALOG,
        MSGBOX_RESULT_BUTTON_25E0_OFFSET,
        MSGBOX_OK_BUTTON,
        MSGBOX_CONFIRM_LATCH_1BC0_OFFSET,
        MSGBOX_CONFIRM_LATCH_SET,
        MSGBOX_ONDECIDE_RVA,
        INPUTMGR_BITMAP_90_OFFSET,
        MENU_EVENT_CONFIRM_3D,
        MENU_EVENT_PRESSED_BIT,
    );
}

/// Install the startup-popup capture hook once (minhook on the MessageBoxDialog builder
/// 0x1409275b0). The builder hook captures each created MessageBoxDialog into
/// CONNECTION_ERROR_DIALOG; `force_dismiss_startup_dialog` then dismisses it via OnDecide each
/// frame. Idempotent; safe to call every frame from the game task until it succeeds.
pub(crate) fn install_auto_accept_hook() {
    if AUTO_ACCEPT_INSTALLED.load(Ordering::SeqCst) != AUTO_ACCEPT_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "auto-accept: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(builder_addr) = game_rva(MSGBOX_BUILDER_RVA) else {
        append_autoload_debug(format_args!("auto-accept: failed to resolve builder rva"));
        return;
    };
    match unsafe {
        MhHook::new(
            builder_addr as *mut c_void,
            msgbox_builder_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            MSGBOX_BUILDER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "auto-accept: queue_enable builder failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    AUTO_ACCEPT_INSTALLED.store(AUTO_ACCEPT_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "auto-accept: hooked MessageBoxDialog builder 0x{builder_addr:x} (capture -> OnDecide dismiss)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "auto-accept: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "auto-accept: MhHook::new builder failed: {status:?}"
        )),
    }
}

/// Diagnostic gate (GAME_DIR file `er-effects-grsysmsg-log.txt` or `ER_EFFECTS_GRSYSMSG_LOG=1`):
/// arm the GR_System_Message id-logger so a probe can DEFINITIVELY name which message(s) the
/// menu-open MessageBoxDialogs carry (instead of guessing connection vs save). Reusable tool.
pub(crate) fn grsysmsg_log_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_GRSYSMSG_LOG").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-grsysmsg-log.txt")
            .exists()
}

static GR_SYSMSG_LOG_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static GR_SYSMSG_LOG_ORIG: AtomicUsize = AtomicUsize::new(0);
static GR_SYSMSG_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
/// `CS::GetGR_System_Message` (deobf entry 0x140762e30): `MenuString* (rcx=out, edx=int messageId)`.
/// The dump labels it 0x140762e40 but that is MID-INSTRUCTION (inside `movq $-2,[rsp+0x28]`); the real
/// MSVC prologue (`mov [rsp+8],rcx; push rdi; sub rsp,0x30`) is at 0x140762e30 -- VERIFIED by deobf
/// boundary disasm (prev fn ret+int3 at 0x140762e26/27, then this prologue). Body reads FMG repo
/// [0x143d7d4f8], applies the +0x384 variant, builds the MenuString.
// CORRECTED 2026-06-23 (corrupted-save-re-findings): 0x762e30 is GetTextEmbedImageName (it does
// id += 900, uses a different singleton) -- NOT GetGR_System_Message. The real getter is deobf
// 0x140762d50 (dump 0x140762e40 - 0xf0 region shift): it loads L"GR_System_Message"+L"SM" and calls
// MsgRepository::GetAndFormat with the id in edx. Hooking the WRONG fn is why the 401106 corrupted-
// save id was never seen (oracle stayed 0). This RVA must be the real getter for the semaphore.
const GR_SYSTEM_MESSAGE_RVA: u32 = 0x762d50;
const GR_SYSMSG_LOG_MAX: usize = 64;

/// DIAGNOSTIC detour for GetGR_System_Message 0x140762e40. Once the main menu has opened (skip the
/// boot-time message flood), log the integer message id (the `edx`/`rdx` arg) + first game caller RVA
/// for each call, capped. The id maps 1:1 to GR_System_Message_win64 (e.g. 4101 "Cannot connect to
/// network", 4102 "connection to game server lost", 4190 "network error", 70000 save-data notice,
/// 4191 "Failed to save game"), so the menu-open modals can be named without guessing. Read-only
/// passthrough; never mutates.
/// GR_System_Message ids the game fetches when it builds a "save data is corrupted" dialog (verified
/// from menu.msgbnd GR_System_Message_win64.fmg). 4191/4192/4193/401106 = "Failed to save game --
/// save data is corrupted"; 401721 = "Failed to load save data -- corrupted"; 401107 = "delete
/// corrupted data and create a new save?". Detecting any of these in GetGR_System_Message IS the
/// memory-read semaphore for the corrupted-save popup (privacy-policy/char-presence-CONFIRMED loop).
pub(crate) const CORRUPTED_SAVE_MSG_IDS: &[i32] = &[4191, 4192, 4193, 401106, 401107, 401721];
/// The corrupted-save message id last seen (0 = none). Exposed as `oracle_corrupted_save_seen_id`.
pub(crate) static CORRUPTED_SAVE_SEEN_ID: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);

pub(crate) unsafe extern "system" fn gr_sysmsg_log_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    // Corrupted-save SEMAPHORE: always check (independent of the menu-open-gated logging below) so a
    // load probe records the corrupted-save popup as RAM-read telemetry, not just an on-screen image.
    let msg_id_now = (rdx & 0xffff_ffff) as i32;
    if CORRUPTED_SAVE_MSG_IDS.contains(&msg_id_now)
        && CORRUPTED_SAVE_SEEN_ID.swap(msg_id_now, Ordering::SeqCst) != msg_id_now
    {
        append_autoload_debug(format_args!(
            "save-override: CORRUPTED-SAVE SEMAPHORE -- GetGR_System_Message id={msg_id_now} (save data is corrupted dialog); the gold save was read but rejected on validate/write"
        ));
    }
    if TFC_AUTO_MENU_OPENED.load(Ordering::SeqCst) != 0 {
        let n = GR_SYSMSG_LOG_COUNT.fetch_add(1, Ordering::SeqCst);
        if n < GR_SYSMSG_LOG_MAX {
            let msg_id = (rdx & 0xffff_ffff) as i32;
            let caller_rva = trace_first_game_caller_rva();
            append_autoload_debug(format_args!(
                "grsysmsg #{n}: id={msg_id} caller_rva=0x{caller_rva:x} out=0x{rcx:x}"
            ));
        }
    }
    let orig = GR_SYSMSG_LOG_ORIG.load(Ordering::SeqCst);
    if orig == TITLE_OWNER_SCAN_START_ADDRESS {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

/// Install the GR_System_Message id-logger once (MinHook on 0x140762e40), mirroring the auto-accept
/// builder-hook precedent. Caller-gated by `grsysmsg_log_enabled()`.
pub(crate) fn install_gr_sysmsg_log_hook() {
    if GR_SYSMSG_LOG_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "grsysmsg-log: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(GR_SYSTEM_MESSAGE_RVA) else {
        append_autoload_debug(format_args!("grsysmsg-log: failed to resolve rva"));
        return;
    };
    match unsafe { MhHook::new(addr as *mut c_void, gr_sysmsg_log_hook as *mut c_void) } {
        Ok(hook) => {
            GR_SYSMSG_LOG_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "grsysmsg-log: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "grsysmsg-log: hooked GetGR_System_Message 0x{addr:x} (log id+caller after menu-open)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "grsysmsg-log: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("grsysmsg-log: MhHook::new failed: {status:?}"))
        }
    }
}

/// CS::NetworkCheckJob::Run RVA (deobf entry 0x140821310). Signature
/// `MenuJobResult*(rcx=job, rdx=MenuJobResult* result, r8=FD4Time*)`. Entry prologue
/// (push rbp/rsi/rdi/r14/r15; lea rbp; sub rsp) is a clean MinHook target (disasm-verified).
const NETWORK_CHECK_JOB_RUN_RVA: u32 = 0x821310;
/// `FD4::FD4TimeTemplate<float>::vftable` (deobf 0x1429c8e48) -- the value Run's common-return path
/// writes to `*(param_3)` in every leaf (RVA read from the deobf disasm of the clean leaf).
const FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA: usize = 0x29c8e48;
/// `MenuJobState::Continue` (the no-modal result), verified from the deobf clean leaf (`lea edx,[r8+1]`).
const MENU_JOB_STATE_CONTINUE: i32 = 1;

static NETWORK_CHECK_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static NETWORK_CHECK_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// THE MILESTONE-3 FIX (zero-input, save-safe). `CS::NetworkCheckJob::Run` is a title-flow MenuJob the
/// TitleTopDialog registrar chains UNCONDITIONALLY at menu-open. Offline, its Steam-holder check
/// (FUN_140cab320: all 3 holders field@0x10==2) and EOS check (FUN_140ddfb90) never pass, so every
/// decision-tree leaf builds a GR_System_Message MessageBoxDialog -- EXCEPT one leaf that does
/// `MenuJobResult::SetResult(Continue)` with no modal (decompile-verified). This detour REPLACES Run
/// with exactly that clean leaf, skipping the entire tree, so ZERO modals are ever enqueued regardless
/// of CSNetMan/CSCheatEOS readiness. The original is never called (its only outputs are the result +
/// the FD4Time vtable, both replicated). No input, no save write; only armed when offline is forced,
/// so it never alters an online (Seamless Co-op) network check. bd er-effects-rs-0ye.
pub(crate) unsafe extern "system" fn network_check_job_run_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let result = rdx;
    // MenuJobResult::SetResult(result, Continue, 0): state @ +0 (i32), field1 @ +4 (i32). The native
    // SetResult 0x1407a91e0 only writes these two fields, so replicate inline. Readability-guarded.
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_CONTINUE;
            *((result + 4) as *mut i32) = 0;
        }
    }
    // param_3->base._vfptr = FD4::FD4TimeTemplate<float>::vftable (Run's common-return sets this).
    if let Ok(base) = game_module_base() {
        if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
            unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
        }
    }
    if NETWORK_CHECK_SHORTCIRCUIT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == null {
        append_autoload_debug(format_args!(
            "network-check-shortcircuit: forced CS::NetworkCheckJob::Run -> MenuJobResult(Continue) result=0x{rdx:x} fd4time=0x{r8:x} -- no GR_System_Message modal enqueued (offline)"
        ));
    }
    let _ = (rcx, r9);
    result
}

/// Install the NetworkCheckJob::Run short-circuit ONCE (MinHook on 0x140821310), mirroring the
/// auto-accept builder-hook precedent. Must arm before menu-open; caller-gated (offline only).
pub(crate) fn install_network_check_shortcircuit_hook() {
    if NETWORK_CHECK_SHORTCIRCUIT_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "network-check-shortcircuit: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(NETWORK_CHECK_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "network-check-shortcircuit: failed to resolve rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            network_check_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "network-check-shortcircuit: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "network-check-shortcircuit: hooked CS::NetworkCheckJob::Run 0x{addr:x} -- offline modal suppression armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "network-check-shortcircuit: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "network-check-shortcircuit: MhHook::new failed: {status:?}"
        )),
    }
}

/// CS::ShowProgressJob::Run RVA (deobf entry 0x1408349c0; dump 0x140834ab0, region shift -0xf0,
/// clean prologue disasm-verified). Signature `MenuJobResult*(rcx=ShowProgressJob, rdx=MenuJobResult*
/// result, r8=FD4Time*)` -- IDENTICAL to NetworkCheckJob::Run.
const SHOW_PROGRESS_JOB_RUN_RVA: u32 = 0x8349c0;
/// `MenuJobState::Success` (=2; Continue=1). Verified from FUN_1407a7340's `SetResult(.,Success,0)`
/// clean leaf (deobf `lea edx,[r8+2]`). A passing check returns Success -> `ShouldContinue` (state>1)
/// true -> ShowProgressJob::Run propagates it -> flow ADVANCES (no modal). Forcing Continue(1) would
/// loop the timed job; Success(2) completes it cleanly.
const MENU_JOB_STATE_SUCCESS: i32 = 2;

static SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED: AtomicUsize = AtomicUsize::new(0);
static SHOW_PROGRESS_SHORTCIRCUIT_COUNT: AtomicUsize = AtomicUsize::new(0);
/// Original CS::ShowProgressJob::Run trampoline (MinHook). Needed so the SAVE-data progressType can be
/// PASSED THROUGH to its real delegate -- that delegate IS the boot ProfileSummary read (SLLoadSession
/// -> ER0000.sl2). Blanket-suppressing every type (the prior behavior) killed the save read, leaving
/// an empty profile -> Bandai privacy policy. bd boot-profile-read-STEP_InitMenu-blocked-by-showprogress-shortcircuit-2026-06-23.
static SHOW_PROGRESS_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ShowProgressJob progressType at [job+0x18] (RE-confirmed). 10 = save-data check/load (MUST run its
/// delegate); 20=network, 30/31=sign-in, 60=login (offline-modal types we still short-circuit).
const SHOW_PROGRESS_TYPE_OFFSET: usize = 0x18;
const SHOW_PROGRESS_SAVE_TYPE: u32 = 10;
static SHOW_PROGRESS_TYPE_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// THE MILESTONE-3 FIX, part 2 (zero-input, save-safe). `CS::ShowProgressJob::Run` (deobf 0x1408349c0)
/// is the SHARED Run for the offline title-flow check steps (save=10/network=20/sign-in=30,31/
/// login=60) the registrar chains at menu-open. Each runs a check delegate (job+0x20, slot +0x10);
/// offline the delegate returns an ERROR result, which ShowProgressJob::Run propagates so the pump
/// enqueues a GR_System_Message MessageBox. The 3 observed menu-open modals all come from these
/// ShowProgressJobs (NOT NetworkCheckJob, which is a separate job already hooked). This detour REPLACES
/// Run with a passing-check exit: result = {state=Success, field1=0} (exactly what FUN_1407a7340's
/// SetResult(Success) clean leaf yields) + the FD4Time vtable, skipping the delegate -> the job
/// completes successfully, the flow advances, and ZERO modals are enqueued. One hook covers all the
/// check steps. Offline-gated (no effect on an online Seamless Co-op check). bd er-effects-rs-0ye.
pub(crate) unsafe extern "system" fn show_progress_job_run_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let result = rdx;
    // progressType ([job+0x18], low 32 bits). 10 = the SAVE-data check/load: its delegate is the boot
    // ProfileSummary read, so it MUST run -- pass it through to the original. Suppressing it (as the
    // prior blanket short-circuit did) leaves the profile empty -> privacy policy, and the save is
    // never read. All other types (network/sign-in/login) still get the Success short-circuit so the
    // offline connection modals stay suppressed.
    let ptype = if rcx > null {
        unsafe { safe_read_usize(rcx + SHOW_PROGRESS_TYPE_OFFSET) }
            .map(|v| (v & 0xffff_ffff) as u32)
    } else {
        None
    };
    let raw10 = if rcx > null {
        unsafe { safe_read_usize(rcx + 0x10) }
    } else {
        None
    };
    let d = SHOW_PROGRESS_TYPE_LOGGED.fetch_add(1, Ordering::SeqCst);
    if d < 16 {
        append_autoload_debug(format_args!(
            "show-progress: progressType[+0x18]={ptype:?} field[+0x10]={raw10:x?} result=0x{rdx:x} (save_type={SHOW_PROGRESS_SAVE_TYPE})"
        ));
    }
    if ptype == Some(SHOW_PROGRESS_SAVE_TYPE) {
        let orig = SHOW_PROGRESS_ORIG.load(Ordering::SeqCst);
        if orig != HOOK_ORIGINAL_UNSET {
            if d < 16 {
                append_autoload_debug(format_args!(
                    "show-progress: PASS-THROUGH save-data progressType {SHOW_PROGRESS_SAVE_TYPE} -> original delegate (boot ProfileSummary read fires)"
                ));
            }
            let call: unsafe extern "system" fn(usize, usize, usize, usize) -> usize = unsafe {
                std::mem::transmute::<
                    usize,
                    unsafe extern "system" fn(usize, usize, usize, usize) -> usize,
                >(orig)
            };
            return unsafe { call(rcx, rdx, r8, r9) };
        }
    }
    if result > null && unsafe { safe_read_usize(result) }.is_some() {
        unsafe {
            *(result as *mut i32) = MENU_JOB_STATE_SUCCESS;
            *((result + 4) as *mut i32) = 0;
        }
    }
    if let Ok(base) = game_module_base() {
        if r8 > null && unsafe { safe_read_usize(r8) }.is_some() {
            unsafe { *(r8 as *mut usize) = base + FD4_TIME_TEMPLATE_FLOAT_VFTABLE_RVA };
        }
    }
    if SHOW_PROGRESS_SHORTCIRCUIT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) == null {
        append_autoload_debug(format_args!(
            "show-progress-shortcircuit: forced CS::ShowProgressJob::Run -> MenuJobResult(Success) result=0x{rdx:x} fd4time=0x{r8:x} -- offline title-flow check modal(s) suppressed at the shared chokepoint"
        ));
    }
    let _ = (rcx, r9);
    result
}

/// Install the ShowProgressJob::Run short-circuit ONCE (MinHook on 0x1408349c0). Must arm before
/// menu-open; caller-gated (offline only).
pub(crate) fn install_show_progress_shortcircuit_hook() {
    if SHOW_PROGRESS_SHORTCIRCUIT_INSTALLED.swap(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        != TITLE_OWNER_SCAN_START_ADDRESS
    {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "show-progress-shortcircuit: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(addr) = game_rva(SHOW_PROGRESS_JOB_RUN_RVA) else {
        append_autoload_debug(format_args!(
            "show-progress-shortcircuit: failed to resolve rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            addr as *mut c_void,
            show_progress_job_run_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            // Store the trampoline BEFORE enabling so the SAVE-data progressType can be passed through
            // to the original delegate (the boot ProfileSummary read).
            SHOW_PROGRESS_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "show-progress-shortcircuit: queue_enable failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "show-progress-shortcircuit: hooked CS::ShowProgressJob::Run 0x{addr:x} -- save-type passthrough + offline-check modal suppression armed"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "show-progress-shortcircuit: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "show-progress-shortcircuit: MhHook::new failed: {status:?}"
        )),
    }
}

/// LATCH detour for the CS::SceneObjProxy ctor 0x14074a700 (rcx=proxy[this], rdx=MenuWindow*,
/// r8/r9 forwarded). Disasm-verified: the ctor does `mov %rdx,%rbx` (0x14074a720) then
/// `mov %rbx,0x20(%rsi)` (0x14074a735) -- so the incoming RDX is the engine-verified MenuWindow it
/// stores at proxy+0x20 (probe-6 proved the OLD TitleTopDialog-factory rdx was a std::function
/// delegate, NOT the MenuWindow). Runtime showed the old MenuWindow/MenuWindowProxy vtable constants
/// are stale for this ctor's engine-provided rdx, but static disassembly still proves the game stores
/// rdx as proxy+0x20. Treat the engine-provided heap-aligned rdx as the trust boundary and OVERWRITE
/// LATCHED_MENU_WINDOW on EVERY valid call (most-recent live host window wins -- the title's host
/// window is latched by the time STAGE2 runs). Then pure passthrough: call the original trampoline
/// with ALL args preserved + return its result, never perturbing the build.
/// bd live-dialog-probe6-factory-fires-returns-dialog-rdx-not-menuwindow-2026.
pub(crate) unsafe extern "system" fn scene_obj_proxy_ctor_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> usize {
    const CANDIDATE_ALIGNED: usize = 0;
    const HEAP_LO: usize = 0x10000;
    const PTR_ALIGN_MASK: usize = 0x7;
    const SCENE_OBJ_PROXY_CTOR_LOG_MAX: usize = 32;
    const SCENE_OBJ_PROXY_CTOR_HIT_INC: usize = 1;
    static SCENE_OBJ_PROXY_CTOR_HITS: AtomicUsize = AtomicUsize::new(0);

    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let menu_window = rdx;
    let hit = SCENE_OBJ_PROXY_CTOR_HITS.fetch_add(SCENE_OBJ_PROXY_CTOR_HIT_INC, Ordering::SeqCst);
    let pvt = unsafe { safe_read_usize(menu_window) }.unwrap_or(null);
    if menu_window != null
        && menu_window >= HEAP_LO
        && (menu_window & PTR_ALIGN_MASK) == CANDIDATE_ALIGNED
    {
        LATCHED_MENU_WINDOW.store(menu_window, Ordering::SeqCst);
        if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
            append_autoload_debug(format_args!(
                "menuwindow-latch: 0x14074a700 ACCEPT #{hit} rdx=0x{menu_window:x} first=0x{pvt:x} (engine-stored proxy+0x20 candidate)"
            ));
        }
    } else if hit < SCENE_OBJ_PROXY_CTOR_LOG_MAX {
        append_autoload_debug(format_args!(
            "menuwindow-latch: 0x14074a700 REJECT #{hit} rdx=0x{menu_window:x} first=0x{pvt:x} (not heap-aligned)"
        ));
    }
    let orig = SCENE_OBJ_PROXY_CTOR_ORIG.load(Ordering::SeqCst);
    if orig == null {
        return null;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(orig) };
    unsafe { f(rcx, rdx, r8, r9) }
}

/// Install the MenuWindow-latch hook once (MinHook on the SceneObjProxy ctor 0x14074a700),
/// matching the auto-accept builder-hook precedent exactly (MhHook::new + queue_enable +
/// MH_ApplyQueued). Must run at process attach BEFORE the title builds during boot so the ctor's
/// rdx (the validated host MenuWindow*) is latched. Idempotent + harmless (latch + passthrough).
pub(crate) fn install_menu_window_latch_hook() {
    if MENU_WINDOW_LATCH_INSTALLED.load(Ordering::SeqCst) != MENU_WINDOW_LATCH_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "menuwindow-latch: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let Ok(ctor_addr) = game_rva(SCENE_OBJ_PROXY_CTOR_RVA) else {
        append_autoload_debug(format_args!(
            "menuwindow-latch: failed to resolve SceneObjProxy ctor rva"
        ));
        return;
    };
    match unsafe {
        MhHook::new(
            ctor_addr as *mut c_void,
            scene_obj_proxy_ctor_hook as *mut c_void,
        )
    } {
        Ok(hook) => {
            SCENE_OBJ_PROXY_CTOR_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "menuwindow-latch: queue_enable ctor failed: {status:?}"
                ));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    MENU_WINDOW_LATCH_INSTALLED
                        .store(MENU_WINDOW_LATCH_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "menuwindow-latch: hooked SceneObjProxy ctor 0x{ctor_addr:x} (latch rdx=MenuWindow*)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "menuwindow-latch: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "menuwindow-latch: MhHook::new ctor failed: {status:?}"
        )),
    }
}

/// Install the SAVE-SAFE c30-writer diagnostic hook once (MinHook on the SOLE
/// GameMan+0xc30 writer 0x14067bd70), mirroring the MenuWindow-latch precedent exactly
/// (MH_Initialize + MhHook::new + queue_enable + MH_ApplyQueued). Installed
/// UNCONDITIONALLY at process attach. The hook (`c30_writer_hook`) is a pure
/// passthrough that forwards all args + returns the original's result; it only logs the
/// c30-write gate, c30 before/after, and a window of the resident save buffer so we can
/// diagnose why c30 stays default cold. NO SetState5, NO save write -- harmless.
pub(crate) fn install_c30_writer_hook() {
    if C30_WRITER_HOOK_INSTALLED.load(Ordering::SeqCst) != C30_WRITER_HOOK_NOT_INSTALLED {
        return;
    }
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!("c30-writer: MH_Initialize failed: {status:?}"));
            return;
        }
    }
    let Ok(writer_addr) = game_rva(C30_WRITER_RVA as u32) else {
        append_autoload_debug(format_args!("c30-writer: failed to resolve 0x67bd70 rva"));
        return;
    };
    match unsafe { MhHook::new(writer_addr as *mut c_void, c30_writer_hook as *mut c_void) } {
        Ok(hook) => {
            C30_WRITER_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!("c30-writer: queue_enable failed: {status:?}"));
                return;
            }
            match unsafe { MH_ApplyQueued() } {
                MH_STATUS::MH_OK => {
                    std::mem::forget(hook);
                    C30_WRITER_HOOK_INSTALLED
                        .store(C30_WRITER_HOOK_INSTALLED_YES, Ordering::SeqCst);
                    append_autoload_debug(format_args!(
                        "c30-writer: hooked 0x{writer_addr:x} (SAVE-SAFE c30-write diagnostic; gate + c30 before/after + buffer window)"
                    ));
                }
                status => append_autoload_debug(format_args!(
                    "c30-writer: MH_ApplyQueued failed: {status:?}"
                )),
            }
        }
        Err(status) => {
            append_autoload_debug(format_args!("c30-writer: MhHook::new failed: {status:?}"))
        }
    }
}

/// Clean static splash-skip patch (flip je->jg in STEP_BeginLogo) so the game's
/// own flow advances past the logo via SetState instead of playing it. Validates
/// the expected opcode first (aborts if the binary differs), and restores page
/// protection after. Spawned early at DLL attach so it lands before state 2 runs.
pub(crate) fn apply_splash_skip() {
    let Ok(base) = game_module_base() else {
        append_autoload_debug(format_args!("splash-skip: module base unavailable"));
        return;
    };
    let target = (base + SPLASH_SKIP_RVA) as *mut u8;
    let existing = unsafe { *target };
    if existing != SPLASH_SKIP_EXPECTED_JE {
        append_autoload_debug(format_args!(
            "splash-skip: ABORT -- byte at 0x{:x} is 0x{existing:x}, expected 0x{SPLASH_SKIP_EXPECTED_JE:x}",
            base + SPLASH_SKIP_RVA
        ));
        return;
    }
    let mut old_protect = PAGE_PROTECT_UNSET;
    let protect_ok = unsafe {
        VirtualProtect(
            target as *mut c_void,
            SPLASH_PATCH_LEN,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    };
    if protect_ok == HOOK_FALSE_RETURN as i32 {
        append_autoload_debug(format_args!("splash-skip: VirtualProtect failed"));
        return;
    }
    unsafe { *target = SPLASH_SKIP_REPLACEMENT_JG };
    let mut restored = PAGE_PROTECT_UNSET;
    unsafe {
        VirtualProtect(
            target as *mut c_void,
            SPLASH_PATCH_LEN,
            old_protect,
            &mut restored,
        )
    };
    append_autoload_debug(format_args!(
        "splash-skip: patched 0x{:x} 0x{SPLASH_SKIP_EXPECTED_JE:x}->0x{SPLASH_SKIP_REPLACEMENT_JG:x}",
        base + SPLASH_SKIP_RVA
    ));
}

/// Render-thread liveness + bootstrap probe. Runs from the ImGui render loop (a
/// separate thread from the game-task scheduler), so it keeps reporting after the
/// title->menu phase transition stops the title CSTask. Distinguishes "the title
/// advanced (render alive + CSFeMan builds)" from "the game hung (render frozen)".
#[allow(dead_code)]
/// When set, ALL game input is hard-blocked at the API layer (see `enforce_input_block`):
/// DInput8 keyboard+mouse (state zeroed by the `debug::InputBlocker` hook) AND XInput
/// gamepad (this module's hook). Read by `xinput_get_state_hook` each poll so the block is
/// authoritative regardless of window focus.
pub(crate) static BLOCK_INPUT_ACTIVE: AtomicUsize = AtomicUsize::new(0);
const BLOCK_INPUT_ON: usize = 1;
/// Original `XInputGetState` (minhook trampoline). 0 until the hook installs.
pub(crate) static XINPUT_GET_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);

/// STAY-ACTIVE gate (`ER_EFFECTS_STAY_ACTIVE=1` / `er-effects-stay-active.txt`). When set, keep ER's
/// input-accept flag `[DLUID+0x88d]` forced to 1 every tick so a virtual gamepad keeps driving the
/// menus while ER is UNFOCUSED -- letting the user work in another window during a golden capture.
/// Decoded: ER clears that flag each frame when it isn't `GetActiveWindow` (`0x141f292bd`); we re-set
/// it. Touches ONLY focus-input gating, never the sim/save/load.
pub(crate) fn stay_active_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_STAY_ACTIVE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-stay-active.txt")
            .exists()
}

/// True when the autoload/own-stepper probe must run UNCONTAMINATED -- no real keyboard,
/// mouse (move/click), or gamepad input may reach the game even if the user focuses the
/// window. Auto-on whenever the own-stepper drives the front-end (the whole point of that
/// probe is a zero-input load), plus an explicit env/file override for standalone use.
pub(crate) fn block_input_enabled() -> bool {
    // FORCE-BLOCK override (env/file): block UNCONDITIONALLY, even past menu-open. Used to
    // FALSIFY -- runtime-proven 2026-06-17 that blocking through menu-open lets the menu OPEN
    // (self-fire) but starves the post-open navigation, so the load never selects.
    if matches!(std::env::var("ER_EFFECTS_BLOCK_INPUT").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-block-input.txt")
            .exists()
    {
        return true;
    }
    // INJECT-NAV instrument-capture: keep the block ON past menu-open so the user's input is
    // suppressed while the XInput hook fabricates the cursor nav (so nothing pollutes the
    // capture). The fabricated Down is written INTO the otherwise-blocked gamepad state, so the
    // menu still gets a live (synthesized) input each frame -- it does not stall.
    if own_stepper_enabled() && !own_stepper_passive_enabled() && inject_nav_enabled() {
        return true;
    }
    // PASSIVE mode never blocks. Otherwise keep the block engaged through the ENTIRE headless
    // drive -- boot -> menu-open -> zero-input title-confirm Load fire -> mount -> confirm --
    // releasing ONLY once in-world (the user takes over) or on abort (phase DONE). Product
    // autoload keeps blocking after the guarded SetState5 until the in-world oracle fires, so the
    // world-stream interval cannot be contaminated by user input.
    let product_world_stream_pending = product_autoload_enabled()
        && OWN_STEPPER_CONFIRMED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES;
    // ZERO-INPUT INVARIANT (always-block-input-zero-input-invariant-2026-06-22): block ALL foreign
    // input whenever ANY automated load lever is armed (not just own_stepper) until in-world, so no
    // probe can be contaminated and no path can secretly rely on input. own_load covers its pump/
    // continue/install sub-levers (they all ride on own_load being armed). Normal play and user-driven
    // golden traces (no lever armed) never block; the in-world release lets the user play after load.
    (own_stepper_enabled() || own_load_enabled() || product_autoload_enabled())
        && !own_stepper_passive_enabled()
        && IN_WORLD_REACHED.load(Ordering::SeqCst) != IN_WORLD_REACHED_YES
        && (OWN_STEPPER_PHASE.load(Ordering::SeqCst) != OWN_STEPPER_PHASE_DONE
            || product_world_stream_pending)
}

/// Release the input block (DInput + XInput) once `block_input_enabled()` flips false mid-run.
/// The hooks stay installed but pass input through when `BLOCK_INPUT_ACTIVE` is clear; the
/// DInput blocker also needs its own flags cleared. Acts once on the ON->off transition.
pub(crate) fn release_input_block_now() {
    if BLOCK_INPUT_ACTIVE.swap(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst) == BLOCK_INPUT_ON {
        InputBlocker::get_instance().block_only(InputFlags::empty());
        // Release the cursor confinement (paired with the ClipCursor lockdown in enforce).
        let _ = unsafe { ClipCursor(None) };
        append_autoload_debug(format_args!(
            "input-block: RELEASED (in-world / abort) -- keyboard/mouse/gamepad + cursor live"
        ));
    }
}

/// XInput `XInputGetState(user_index, *mut XINPUT_STATE) -> DWORD` detour. Calls the real
/// function, then -- while the block is active -- zeroes the XINPUT_GAMEPAD sub-struct
/// (buttons + triggers + thumbsticks) so the game reads a connected-but-idle pad (no
/// "controller disconnected" popup, but zero input). Leaves the disconnected return code
/// untouched so a genuinely absent pad still reads absent.
pub(crate) unsafe extern "system" fn xinput_get_state_hook(user_index: u32, state: *mut u8) -> u32 {
    const XINPUT_SUCCESS: u32 = 0;
    const XINPUT_ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
    // XINPUT_STATE = { DWORD dwPacketNumber; XINPUT_GAMEPAD Gamepad; }; the gamepad sub-struct
    // (wButtons,bLeftTrigger,bRightTrigger,sThumbLX/LY/RX/RY) starts at +4 and is 12 bytes.
    const XINPUT_GAMEPAD_OFFSET: usize = 4;
    const XINPUT_GAMEPAD_SIZE: usize = 12;
    const ZERO_FILL_BYTE: u8 = 0;
    let orig = XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst);
    let hr = if orig != TITLE_OWNER_SCAN_START_ADDRESS {
        let f: unsafe extern "system" fn(u32, *mut u8) -> u32 =
            unsafe { std::mem::transmute(orig) };
        unsafe { f(user_index, state) }
    } else {
        XINPUT_ERROR_DEVICE_NOT_CONNECTED
    };
    const XINPUT_PACKET_OFFSET: usize = 0;
    const WBUTTONS_OFFSET_IN_GAMEPAD: usize = 0;
    if !state.is_null() && BLOCK_INPUT_ACTIVE.load(Ordering::SeqCst) == BLOCK_INPUT_ON {
        let inject = inject_nav_enabled()
            && OWN_STEPPER_MENU_OPENED.load(Ordering::SeqCst) != OWN_STEPPER_MENU_OPENED_NO;
        if inject {
            // Fabricate the gamepad state at the poll source from the schedule driven each frame
            // by own_stepper idx10 (this hook may never be polled if no controller, so the
            // schedule does NOT live here). Force SUCCESS + a fresh packet number so a live pad is
            // simulated; write the scheduled D-pad Down. Harmless if the game ignores XInput.
            let buttons = INJECT_NAV_CUR_BUTTONS.load(Ordering::SeqCst) as u16;
            let pkt = INJECT_NAV_FRAME.load(Ordering::SeqCst) as u32;
            unsafe {
                std::ptr::write_bytes(
                    state.add(XINPUT_GAMEPAD_OFFSET),
                    ZERO_FILL_BYTE,
                    XINPUT_GAMEPAD_SIZE,
                );
                *(state.add(XINPUT_PACKET_OFFSET) as *mut u32) = pkt;
                *(state.add(XINPUT_GAMEPAD_OFFSET + WBUTTONS_OFFSET_IN_GAMEPAD) as *mut u16) =
                    buttons;
            }
            let _ = user_index;
            return XINPUT_SUCCESS;
        }
        if hr == XINPUT_SUCCESS {
            unsafe {
                std::ptr::write_bytes(
                    state.add(XINPUT_GAMEPAD_OFFSET),
                    ZERO_FILL_BYTE,
                    XINPUT_GAMEPAD_SIZE,
                )
            };
        }
    }
    hr
}

/// Install the XInput gamepad block once. Hooks `XInputGetState` (and ordinal-100
/// `XInputGetStateEx`, used by Steam Input) in whichever xinput runtime DLL is loaded.
/// minhook-based, mirroring `create_continue_trace_hook`.
unsafe fn install_xinput_block() {
    const XINPUT_DLLS: [&[u8]; 5] = [
        b"xinput1_4.dll\0",
        b"xinput1_3.dll\0",
        b"xinput9_1_0.dll\0",
        b"xinput1_2.dll\0",
        b"xinput1_1.dll\0",
    ];
    const XINPUT_GET_STATE_EX_ORDINAL: usize = 100;
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_autoload_debug(format_args!(
                "xinput-block: MH_Initialize failed: {status:?}"
            ));
            return;
        }
    }
    let mut hooked_any = false;
    for name in XINPUT_DLLS {
        let hmod = match unsafe { GetModuleHandleA(PCSTR(name.as_ptr())) } {
            Ok(h) if !h.is_invalid() => h,
            _ => continue,
        };
        let proc = unsafe { GetProcAddress(hmod, PCSTR(b"XInputGetState\0".as_ptr())) };
        let Some(addr) = proc else { continue };
        let addr = addr as usize;
        match unsafe { MhHook::new(addr as *mut c_void, xinput_get_state_hook as *mut c_void) } {
            Ok(hook) => {
                XINPUT_GET_STATE_ORIG.store(hook.trampoline() as usize, Ordering::SeqCst);
                if let Err(status) = unsafe { hook.queue_enable() } {
                    append_autoload_debug(format_args!(
                        "xinput-block: queue_enable XInputGetState failed: {status:?}"
                    ));
                } else {
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetState at 0x{addr:x}"
                    ));
                    std::mem::forget(hook);
                    hooked_any = true;
                }
            }
            Err(status) => append_autoload_debug(format_args!(
                "xinput-block: MhHook::new XInputGetState failed: {status:?}"
            )),
        }
        // Steam Input routes the guide button through ordinal-100 XInputGetStateEx; neuter it
        // too so a focused pad cannot drive menus through that path. Same zeroing detour.
        let ex = unsafe { GetProcAddress(hmod, PCSTR(XINPUT_GET_STATE_EX_ORDINAL as *const u8)) };
        if let Some(ex_addr) = ex {
            let ex_addr = ex_addr as usize;
            if ex_addr != addr {
                if let Ok(hook) = unsafe {
                    MhHook::new(ex_addr as *mut c_void, xinput_get_state_hook as *mut c_void)
                } {
                    let _ = unsafe { hook.queue_enable() };
                    std::mem::forget(hook);
                    append_autoload_debug(format_args!(
                        "xinput-block: hooked XInputGetStateEx(ord 100) at 0x{ex_addr:x}"
                    ));
                }
            }
        }
        break;
    }
    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {}
        status => append_autoload_debug(format_args!(
            "xinput-block: MH_ApplyQueued failed: {status:?}"
        )),
    }
    if !hooked_any {
        append_autoload_debug(format_args!(
            "xinput-block: no xinput DLL with XInputGetState found yet (will retry next frame)"
        ));
    }
}

/// Tracks whether the DInput keyboard+mouse `install_hooks` has run (once).
static DINPUT_BLOCK_INSTALLED: AtomicUsize = AtomicUsize::new(0);

/// Enforce the comprehensive input block for this frame. Self-contained (no args) so it can
/// run from EITHER the game task OR the render loop -- critical because under the offline
/// launcher the hudhook render loop does NOT execute at the title, so the render-loop call
/// alone never engaged the block (that was the contamination hole). Driven every frame from
/// the game task while `block_input_enabled()`:
///   1. ONCE: install the DInput8 keyboard+mouse `GetDeviceState` block (panics on probe
///      failure -> contained with catch_unwind so the FD4 task never unwinds into C++).
///   2. EVERY frame: assert the block-all flag (sticky, overriding any overlay want-capture
///      clear) and install/retry the XInput gamepad hook until the xinput DLL is present.
/// Genuinely zero-input: it only SUPPRESSES device reads -- it never synthesizes any input.
pub(crate) fn enforce_input_block_now() {
    let blocker = InputBlocker::get_instance();
    if DINPUT_BLOCK_INSTALLED.swap(BLOCK_INPUT_ON, Ordering::SeqCst)
        == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            blocker.install_hooks()
        }));
        match result {
            Ok(Ok(())) => {
                append_autoload_debug(format_args!(
                    "input-block: DInput keyboard+mouse GetDeviceState hook installed"
                ));
            }
            Ok(Err(status)) => append_autoload_debug(format_args!(
                "input-block: DInput install_hooks failed: {status:?} (XInput still hooks)"
            )),
            Err(_) => append_autoload_debug(format_args!(
                "input-block: DInput install_hooks panicked (contained; XInput still hooks)"
            )),
        }
    }
    BLOCK_INPUT_ACTIVE.store(BLOCK_INPUT_ON, Ordering::SeqCst);
    blocker.block_only(InputFlags::all());
    if XINPUT_GET_STATE_ORIG.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS {
        // Not yet hooked (xinput DLL may load late): retry each frame until it sticks.
        unsafe { install_xinput_block() };
    }
    // Lock down MOUSE MOVEMENT: the DInput GetDeviceState block zeroes keyboard + mouse buttons +
    // DInput mouse deltas, but ER moves the MENU cursor via the OS cursor position (GetCursorPos),
    // which DInput blocking does NOT cover -- so the user can still move the cursor. Confine the OS
    // cursor to a 1x1 rect every frame: it physically cannot move regardless of which API reads it,
    // making the run uncontaminatable by the mouse. Released (ClipCursor(None)) when the block lifts.
    const CLIP_ORIGIN: i32 = 0;
    const CLIP_EDGE: i32 = 1;
    let clip = RECT {
        left: CLIP_ORIGIN,
        top: CLIP_ORIGIN,
        right: CLIP_EDGE,
        bottom: CLIP_EDGE,
    };
    let _ = unsafe { ClipCursor(Some(&clip)) };
}

pub(crate) fn render_liveness_probe() {
    if !title_accept_enabled() {
        return;
    }
    let frame = RENDER_FRAME_COUNT.fetch_add(AV_LOG_LINE_INCREMENT, Ordering::SeqCst);
    if frame % RENDER_PROBE_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let Ok(base) = game_module_base() else {
        return;
    };
    let csfeman = unsafe { *((base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let latch = unsafe { *((base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    append_autoload_debug(format_args!(
        "render_probe: frame={frame} csfeman=0x{csfeman:x} latch={latch}"
    ));
}

/// Boot-level title-accept (genuine zero input). The press-any-button wall is the
/// boot intro/movie thread parked in its movie-wait loop; the latch 0x143d856a0
/// (sole writer 0x140c8ff41) is set only AFTER that loop finishes, which is what
/// lets the inner MenuJobWait advance 10->11. The movie-dismiss gate 0x140e90820
/// has NO input check -- it finishes on decode completion or the skip-flag byte
/// 0x14458b8a5. So writing the skip-flag makes the intro thread complete its REAL
/// fade-out + teardown + latch LEGITIMATELY (proper bookkeeping, unlike the bare
/// latch poke that crashes), driving the native title-accept with zero input.
/// Watch CSFeMan 0x143d6b880 for the bootstrap.
pub(crate) unsafe fn title_accept_tick(module_base: usize, tick: u64, do_write: bool) {
    if tick < ARM_PROBE_MIN_TICK {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Module-base globals -- always safe committed reads. NO title_owner scan:
    // its full-memory VirtualQuery+deref walk raced the booting game (region freed
    // mid-scan -> AV, the boot-crash). The autoload needs none of it -- the movie
    // singleton and GameMan are fixed globals.
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    let latch = unsafe { *((module_base + TITLE_ACCEPT_LATCH_RVA) as *const u8) };
    let movie = unsafe { *((module_base + MOVIE_SINGLETON_RVA) as *const usize) };
    let skip = unsafe { *((module_base + MOVIE_SKIP_FLAG_RVA) as *const u8) };
    let gm = game_man_ptr_or_null();
    let session = unsafe { *((module_base + SESSION_SINGLETON_RVA) as *const usize) };
    let log_now = (tick % ARM_PROBE_TICK_INTERVAL == null as u64)
        || (skip == MOVIE_SKIP_FLAG_SET && csfeman == null);
    // Scan-free native movie dismiss: gated on the movie singleton being present
    // with the expected vtable (= the title bg movie is up at press-any-button,
    // since splash-skip removed the logos) + a tick floor + skip-flag clear.
    if do_write && tick >= DISMISS_MIN_TICK && skip == MOVIE_SKIP_FLAG_CLEAR && movie != null {
        let movie_vtable = unsafe { *(movie as *const usize) };
        let hwnd = unsafe { *((movie + MOVIE_HWND_OFFSET) as *const usize) };
        if movie_vtable == module_base + MOVIE_VTABLE_RVA && hwnd != null {
            let hwnd_ptr = hwnd as *mut c_void;
            unsafe {
                let menu = GetSystemMenu(hwnd_ptr, WND_GET_SYSTEM_MENU_KEEP);
                if !menu.is_null() {
                    DeleteMenu(menu, WND_SC_CLOSE, WND_MF_BYCOMMAND);
                }
                ShowWindow(hwnd_ptr, WND_SW_HIDE);
                UpdateWindow(hwnd_ptr);
                *((module_base + MOVIE_SKIP_FLAG_RVA) as *mut u8) = MOVIE_SKIP_FLAG_SET;
            }
            append_autoload_debug(format_args!(
                "title_accept: native movie dismiss (movie=0x{movie:x} hwnd=0x{hwnd:x} latch={latch} tick={tick})"
            ));
        }
    }
    // Observability: GameMan load fields + session + csfeman, to see the post-
    // dismiss bootstrap/load trajectory (drives where to arm the load recipe).
    if log_now {
        let (cmd, force, slot, loading) = if gm != null {
            unsafe {
                (
                    *((gm + GAME_MAN_REQUESTED_SLOT_B78_OFFSET) as *const i32),
                    *((gm + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8),
                    *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32),
                    *((gm + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8),
                )
            }
        } else {
            (
                TITLE_STATE_OWNER_GONE,
                MOVIE_SKIP_FLAG_CLEAR,
                TITLE_STATE_OWNER_GONE,
                MOVIE_SKIP_FLAG_CLEAR,
            )
        };
        append_autoload_debug(format_args!(
            "title_accept: skip={skip} movie=0x{movie:x} latch={latch} csfeman=0x{csfeman:x} session=0x{session:x} gm=0x{gm:x} cmd={cmd} force={force} slot={slot} loading={loading} tick={tick}"
        ));
    }
}

/// Per-frame native autoload arm. Recipe A set the slot once and the title reset
/// it to -1 before the save-mgr update could arm, so the latch fired Finish with
/// nothing armed -> null deref. This re-sets the slot EVERY frame (against the
/// title's reset) and sets the latch, giving the native update 0x14067f5d0 a
/// chance to arm GameMan+0xb72 before Finish. Observes b72 / b80 / CSFeMan to see
/// if the arm + bootstrap take. Crash logger should run alongside.
pub(crate) unsafe fn native_arm_loop_tick(module_base: usize, slot: i32, tick: u64) {
    if tick < ARM_PROBE_MIN_TICK {
        return;
    }
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let game_man = game_man_ptr_or_null();
    if game_man == null {
        return;
    }
    let load_in_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    let armed = unsafe { *((game_man + GAME_MAN_ARM_FLAG_B72_OFFSET) as *const u8) };
    let csfeman = unsafe { *((module_base + CSFEMAN_SINGLETON_RVA) as *const usize) };
    if load_in_progress == TITLE_NATIVE_JOB_TASK_DATA_ZERO {
        // Re-arm each frame: persist the slot against the title's reset, set latch.
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        unsafe {
            *((module_base + SELECTBOT_LOAD_GATE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
        }
    }
    if tick % ARM_PROBE_TICK_INTERVAL == null as u64 {
        let ac0 = unsafe { *((game_man + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "native_arm_loop tick={tick} ac0={ac0} b72={armed} b80={load_in_progress} csfeman=0x{csfeman:x}"
        ));
    }
}

/// Read-only probe of the native autoload-arm preconditions at the title. The
/// decisive unknown is `[slotmgr+0x8]` (the loaded slot-record container): the
/// native save-mgr update arms autoload only when it is populated. Logs the
/// GameMan flow flags, slot manager + its data/container pointers, and whether
/// CSFeMan / the input manager exist yet. Touches no state.
pub(crate) unsafe fn arm_precondition_probe(module_base: usize, tick: u64) {
    if tick < ARM_PROBE_MIN_TICK
        || tick % ARM_PROBE_TICK_INTERVAL != TITLE_OWNER_SCAN_START_ADDRESS as u64
    {
        return;
    }
    let read_ptr = |rva: usize| unsafe { *((module_base + rva) as *const usize) };
    let game_man = game_man_ptr_or_null();
    let slot_mgr = game_data_man_ptr_or_null();
    let csfeman = read_ptr(CSFEMAN_SINGLETON_RVA);
    let input_mgr = read_ptr(TITLE_INPUT_MANAGER_RVA);
    let latch = unsafe { *((module_base + SELECTBOT_LOAD_GATE_RVA) as *const u8) };
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let gm_byte = |off: usize| {
        if game_man != null {
            i64::from(unsafe { *((game_man + off) as *const u8) })
        } else {
            ARM_PROBE_FIELD_ABSENT
        }
    };
    let gm_i32 = |off: usize| {
        if game_man != null {
            i64::from(unsafe { *((game_man + off) as *const i32) })
        } else {
            ARM_PROBE_FIELD_ABSENT
        }
    };
    let (slot_data, slot_container) = if slot_mgr != null {
        (
            unsafe { *((slot_mgr + SLOT_MANAGER_DATA_OFFSET) as *const usize) },
            unsafe { *((slot_mgr + SLOT_MANAGER_CONTAINER_OFFSET) as *const usize) },
        )
    } else {
        (null, null)
    };
    append_autoload_debug(format_args!(
        "arm_probe tick={tick} gm=0x{game_man:x} slotmgr=0x{slot_mgr:x} slotmgr+8=0x{slot_data:x} slotmgr+78=0x{slot_container:x} csfeman=0x{csfeman:x} input_mgr=0x{input_mgr:x} latch={latch} b80={} ac0={} b72={} b73={} b75={} b78={} bc4={}",
        gm_byte(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET),
        gm_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET),
        gm_byte(GAME_MAN_ARM_FLAG_B72_OFFSET),
        gm_byte(GAME_MAN_FLAG_B73_PROBE_OFFSET),
        gm_byte(GAME_MAN_FLAG_B75_PROBE_OFFSET),
        gm_i32(GAME_MAN_REQUESTED_SLOT_B78_OFFSET),
        gm_byte(GAME_MAN_FLAG_BC4_OFFSET),
    ));
}

/// Recipe Option 1 (genuine offline continue, flagless): drive the MoveMapList
/// dispatcher 0x140afb880 each frame with GameMan b73 set so it begins
/// current_slot_load and deserializes the REAL slot character (sets
/// GameMan+0x10=1), also building the world singletons. owner is a synthetic
/// buffer with +0x12c = slot. Never writes the force flag 0x143d856a0.
pub(crate) unsafe fn continue_drive_tick(module_base: usize, slot: i32, tick: u64) {
    // Log readiness before the fixed drive gate: recent runs exit before the
    // drive can fire, so the next runtime must tell us when GameMan first became
    // available instead of turning the gate into another blind threshold knob.
    let game_man = game_man_ptr_or_null();
    if game_man == TITLE_OWNER_SCAN_START_ADDRESS {
        return;
    }
    let first_seen_tick = match CONTINUE_DRIVE_GM_FIRST_SEEN_TICK.compare_exchange(
        CONTINUE_DRIVE_GM_FIRST_SEEN_UNSET,
        tick,
        Ordering::SeqCst,
        Ordering::SeqCst,
    ) {
        Ok(_) => {
            append_autoload_debug(format_args!(
                "continue_drive: GameMan first_seen tick={tick} gm=0x{game_man:x} after_gm_gate={CONTINUE_DRIVE_AFTER_GAME_MAN_TICKS}"
            ));
            tick
        }
        Err(existing) => existing,
    };
    let game_man_relative_gate =
        first_seen_tick.saturating_add(CONTINUE_DRIVE_AFTER_GAME_MAN_TICKS);
    let drive_gate_tick = core::cmp::max(CONTINUE_DRIVE_MIN_TICK, game_man_relative_gate);
    if tick < drive_gate_tick {
        return;
    }
    let real_done = unsafe { *((game_man + GAME_MAN_REAL_LOAD_DONE_OFFSET) as *const i32) };
    let load_progress =
        unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
    let map14 = unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
    if real_done == GAME_MAN_REAL_LOAD_DONE_VALUE {
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            append_autoload_debug(format_args!(
                "continue_drive: REAL LOAD DONE gm+0x10=1 map14={map14} b80={load_progress} tick={tick}"
            ));
        }
        return;
    }
    // Synthetic MoveMapList owner: the offline-continue path reads owner+0x12c
    // (slot) and +0x12a. A persistent zeroed buffer suffices.
    let mut owner_ptr = CONTINUE_OWNER_PTR.load(Ordering::SeqCst);
    if owner_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
        let buf = vec![SYNTHETIC_ZERO_QWORD; CONTINUE_OWNER_QWORDS].into_boxed_slice();
        owner_ptr = Box::leak(buf).as_mut_ptr() as usize;
        CONTINUE_OWNER_PTR.store(owner_ptr, Ordering::SeqCst);
    }
    let owner = owner_ptr as *mut u8;
    unsafe {
        *(owner.add(CONTINUE_OWNER_SLOT_OFFSET) as *mut i32) = slot;
        *(owner.add(CONTINUE_OWNER_FLAG_12A_OFFSET)) = CONTINUE_OWNER_FLAG_12A_VALUE;
    }
    // Until the async load has begun (b80 != 0), arm the slot + b73 so the
    // dispatcher selects current_slot_load and begins. The begin is gated on
    // b80==0, so re-arming after it starts cannot re-submit.
    if !CONTINUE_DRIVE_BEGUN.load(Ordering::SeqCst) {
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        unsafe {
            *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *mut u8) = GAME_MAN_B73_FLAG_SET;
        }
        if load_progress != TITLE_NATIVE_JOB_TASK_DATA_ZERO {
            CONTINUE_DRIVE_BEGUN.store(true, Ordering::SeqCst);
        }
    }
    let first_attempt = !CONTINUE_DRIVE_FIRST_ATTEMPT_LOGGED.swap(true, Ordering::SeqCst);
    if first_attempt {
        let b73_before = unsafe { *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *const u8) };
        append_autoload_debug(format_args!(
            "continue_drive: FIRST dispatcher before slot={slot} b80={load_progress} b73={b73_before} real_done={real_done} map14={map14} tick={tick} gate_tick={drive_gate_tick}"
        ));
    }
    let dispatcher: unsafe extern "system" fn(*mut u8) -> usize =
        unsafe { std::mem::transmute(module_base + MOVEMAP_DISPATCHER_RVA) };
    let _ = unsafe { dispatcher(owner) };
    if first_attempt
        || tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64
    {
        let real_after = unsafe { *((game_man + GAME_MAN_REAL_LOAD_DONE_OFFSET) as *const i32) };
        let b80_after =
            unsafe { *((game_man + GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET) as *const u8) };
        let b73_after = unsafe { *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *const u8) };
        let map14_after =
            unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "continue_drive: drove dispatcher slot={slot} b80={b80_after} b73={b73_after} real_done={real_after} map14={map14_after} tick={tick}"
        ));
    }
}

/// Recipe B (flagless): drive the outer SimpleTitleStep IngameInit once to prime
/// the world subsystems and submit the load, then pump the InGameStep each frame
/// to completion. Never touches the force flag 0x143d856a0. Replaces
/// force_play_game (which double-submits). Locates the outer object via scan,
/// arms the staging slot the same frame (IngameInit's descriptor builder reads
/// GameMan+0xac0), calls IngameInit(outer, &FD4TaskData) once, then ticks the
/// InGameStep pump and observes the load cascade.
pub(crate) unsafe fn ingameinit_drive_tick(
    module_base: usize,
    slot: i32,
    tick: u64,
    task_data: &FD4TaskData,
) {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return;
    };
    let ingame = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const usize) };
    let owner_state = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    if ingame == TITLE_OWNER_SCAN_START_ADDRESS {
        if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
            append_autoload_debug(format_args!(
                "ingameinit_drive: ingame(owner+0x2e8) is NULL, owner={owner:p} state={owner_state} tick={tick}"
            ));
        }
        return;
    }
    let _ = owner_state;
    if !INGAMEINIT_DRIVE_DONE.swap(true, Ordering::SeqCst) {
        // Arm the staging slot this frame (the descriptor builder 0x140aea590
        // reads GameMan+0xac0).
        let set_save_slot: unsafe extern "system" fn(i32) =
            unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
        unsafe { set_save_slot(slot) };
        // Compute a valid (non -1) map id so IngameInit takes the continue
        // variant (variant 2 / -1 is the new-game path). Parse the same default
        // map string the new-game path uses.
        let map_parser: unsafe extern "system" fn(*const c_void) -> i32 =
            unsafe { std::mem::transmute(module_base + INGAMEINIT_MAP_PARSER_RVA) };
        let map_id = unsafe { map_parser((module_base + DEFAULT_MAP_STRING_RVA) as *const c_void) };
        // The SimpleTitleStep container is never instantiated in this build, so we
        // call IngameInit with a SYNTHETIC `this`: it only reads +0xc0 (InGameStep)
        // and +0x130 (map), and its tail 0x140b0a980 inc's +0x4c (safe while
        // +0x48 <= 6). A persistent zeroed buffer satisfies all of that.
        let mut synth_ptr = SYNTHETIC_OUTER_PTR.load(Ordering::SeqCst);
        if synth_ptr == TITLE_OWNER_SCAN_START_ADDRESS {
            let buf = vec![SYNTHETIC_ZERO_QWORD; INGAMEINIT_SYNTHETIC_QWORDS].into_boxed_slice();
            synth_ptr = Box::leak(buf).as_mut_ptr() as usize;
            SYNTHETIC_OUTER_PTR.store(synth_ptr, Ordering::SeqCst);
        }
        let synth = synth_ptr as *mut u8;
        unsafe {
            *(synth.add(OUTER_STEP_INGAMESTEP_OFFSET) as *mut usize) = ingame;
            *(synth.add(OUTER_STEP_MAP_OVERRIDE_130_OFFSET) as *mut i32) = map_id;
        }
        let ingame_init: unsafe extern "system" fn(*mut u8, *const FD4TaskData) -> usize =
            unsafe { std::mem::transmute(module_base + INGAMEINIT_HANDLER_RVA) };
        append_autoload_debug(format_args!(
            "ingameinit_drive: calling IngameInit synth={synth:p} slot={slot} map_id={map_id} ingame={ingame:#x}"
        ));
        let _ = unsafe { ingame_init(synth, task_data as *const FD4TaskData) };
        let ingame_d8 = unsafe { *((ingame + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
        let ingame_cur = unsafe { *((ingame + INGAMESTEP_STEP_STATE_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "ingameinit_drive: IngameInit returned ingame_d8={ingame_d8} ingame_cur={ingame_cur}"
        ));
        return;
    }
    // After priming+submit: pump the InGameStep each frame so step 7 observes the
    // (now primed) stream reach resident and sets d8=2 -> load completes.
    let ingame_ptr = ingame as *mut u8;
    let cur = unsafe { *(ingame_ptr.add(INGAMESTEP_STEP_STATE_OFFSET) as *const i32) };
    let d8 = unsafe { *(ingame_ptr.add(TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
    let last_next = INGAMESTEP_PUMP_LAST_NEXT.swap(cur, Ordering::SeqCst);
    let last_d8 = INGAMESTEP_PUMP_LAST_D8.swap(d8, Ordering::SeqCst);
    if cur != last_next || d8 != last_d8 {
        append_autoload_debug(format_args!(
            "ingameinit_drive: pump cur={cur} d8={d8} ingame={ingame:#x}"
        ));
    }
    if cur == INGAMESTEP_FINISHED_SENTINEL || d8 == INGAMESTEP_LOAD_DONE {
        return;
    }
    let Ok(pump) = game_rva(STEP_PUMP_DRIVER_RVA) else {
        return;
    };
    let pump: unsafe extern "system" fn(*mut u8, *const FD4TaskData) -> usize =
        unsafe { std::mem::transmute(pump) };
    let _ = unsafe { pump(ingame_ptr, task_data as *const FD4TaskData) };
}

pub(crate) fn ingamestep_unpin_enabled() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_INGAMESTEP_UNPIN").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-ingamestep-unpin.txt")
        .exists()
}

/// Drives the native TitleStep state machine to `STEP_PlayGame` once.
///
/// Live zero-input probes showed the game parks at `STEP_BeginTitle`
/// (PRESS ANY BUTTON) with GameMan ready but the MoveMapList load dispatcher
/// inactive, so directly setting the continue flags is a no-op. Static RE maps
/// the TitleStep handler table: index 5 (`STEP_PlayGame`, 0x140b0d5b0) reads the
/// selected save slot and submits the native load job. This selects slot `slot`
/// via the menu set-slot primitive and advances the owner's state field so the
/// game's own title task dispatches `STEP_PlayGame` on the next frame — no host
/// input and no synthetic load-primitive calls. We only act once the owner has
/// reached `STEP_BeginTitle`, which guarantees `STEP_InitMenu` already built the
/// menu object `STEP_PlayGame` depends on.
pub(crate) unsafe fn call_force_play_game_once(module_base: usize, slot: i32, tick: u64) -> bool {
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        return false;
    };
    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    // Log every TitleStep state transition so we can see whether the forced
    // STEP_PlayGame write sticks and advances (5 -> 6 GameStepWait -> load) or
    // gets reset by the title task / a different owner instance.
    let last_state = FORCE_PLAY_GAME_LAST_STATE.swap(state_before, Ordering::SeqCst);
    if state_before != last_state {
        // Read GameMan+0x14 (the load value pair writes) each transition: if it
        // becomes nonnegative when PlayGame runs (5 -> 6), the pair chain
        // succeeded and the gap is downstream (GameStepWait/job); if it stays -1,
        // submit/validate/pair never wrote it.
        let gm = game_man_ptr_or_null();
        let load14 = if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((gm + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) }
        } else {
            DIRECT_INPUT_FAILURE_HRESULT
        };
        append_autoload_debug(format_args!(
            "force_play_game: observed state {last_state}->{state_before} load14={load14} tick={tick}"
        ));
    }
    if FORCE_PLAY_GAME_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        // Already drove the state once; keep observing transitions (logged above).
        // While parked in GameStepWait, periodically report the load job's pending
        // field so we can see whether anything drains it.
        if state_before == TITLE_STEP_GAME_STEP_WAIT {
            let job = unsafe { *(owner.add(TITLE_OWNER_JOB_OFFSET) as *const usize) };
            if job != TITLE_OWNER_SCAN_START_ADDRESS {
                let pending = unsafe { *((job + TITLE_OWNER_JOB_PENDING_OFFSET) as *const i32) };
                if tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64 {
                    append_autoload_debug(format_args!(
                        "force_play_game: gamestepwait job={job:#x} job_d8={pending} tick={tick}"
                    ));
                }
                // NOTE: calling the menu-task update wrapper (0x82a0f0) directly on
                // this job crashed the game (autoload-live-playgame-v10) -- the job
                // is not the right `this` / reentrancy. Pumping must go through the
                // game's own task runner; do not force-orphan the job.
            }
        }
        return true;
    }
    // The live title idles at STEP_MenuJobWait (the input-wait state shown as
    // PRESS ANY BUTTON); STEP_BeginTitle is the alternate stable pre-load step.
    // Both run after STEP_InitMenu built the menu object PlayGame needs.
    if state_before != TITLE_STEP_BEGIN_TITLE && state_before != TITLE_STEP_MENU_JOB_WAIT {
        return false;
    }
    let set_save_slot: unsafe extern "system" fn(i32) =
        unsafe { std::mem::transmute(module_base + FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA) };
    unsafe { set_save_slot(slot) };
    // Read-only diagnostic: log the PlayGame load-pair preconditions so we can
    // see which one blocks (pair skips writing GameMan+0x14 unless b28==0; the
    // validate step gates on 12d/12e).
    let game_man_ptr = game_man_ptr_or_null();
    if game_man_ptr != TITLE_OWNER_SCAN_START_ADDRESS {
        let gm = game_man_ptr as *const u8;
        let ac0 = unsafe { *(gm.add(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        let load14 = unsafe { *(gm.add(FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        let b28 = unsafe { *gm.add(FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET) };
        let f12d = unsafe { *gm.add(FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET) };
        let f12e = unsafe { *gm.add(FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET) };
        append_autoload_debug(format_args!(
            "force_play_game: gm={game_man_ptr:#x} ac0={ac0} load14={load14} b28={b28} f12d={f12d} f12e={f12e}"
        ));
    }
    unsafe {
        *(owner.add(TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_OFFSET) as *mut u8) =
            TITLE_OWNER_PLAY_GAME_REQUEST_FLAG_SET;
    }
    // Select the slot STEP_PlayGame loads: its handler reads owner+0xbc and the
    // pair step writes it to GameMan+0x14. Without this it stays -1 and pair bails.
    unsafe { *(owner.add(TITLE_OWNER_PLAY_GAME_SLOT_OFFSET) as *mut i32) = slot };
    unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *mut i32) = TITLE_STEP_PLAY_GAME };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    FORCE_PLAY_GAME_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "force_play_game: set slot={slot} state {state_before}->{state_after} (STEP_PlayGame) tick={tick}"
    ));
    true
}

/// Pseudo-handle for the current process (GetCurrentProcess() is constant -1).
pub(crate) const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
/// Bytes read per ReadProcessMemory call when scanning a region for the title
/// vtable. One syscall per 64KB chunk (then an in-process buffer scan) keeps the
/// fault-tolerant scan fast -- a syscall per 8-byte cursor would stall the thread.
pub(crate) const SCAN_CHUNK_SIZE: usize = 0x10000;

/// Fault-tolerant pointer-sized read via ReadProcessMemory: returns None on
/// unmapped/freed memory instead of raising an access violation. Used by the
/// title-owner scan to survive the TOCTOU race against the booting game.
pub(crate) unsafe fn safe_read_usize(addr: usize) -> Option<usize> {
    let mut value: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut usize as *mut c_void,
            std::mem::size_of::<usize>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<usize>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant i32 read via ReadProcessMemory (None on unmapped memory).
pub(crate) unsafe fn safe_read_i32(addr: usize) -> Option<i32> {
    let mut value: i32 = TITLE_OWNER_SCAN_START_ADDRESS as i32;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut i32 as *mut c_void,
            std::mem::size_of::<i32>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<i32>() {
        Some(value)
    } else {
        None
    }
}

/// Fault-tolerant single-byte read via ReadProcessMemory (None on unmapped memory). Used by the
/// WorldBlockRes::Update diagnostic detour to sample the phase ([+0x35]) and gate ([+0x2f]) bytes
/// without ever dereferencing a raw pointer into possibly-unmapped block memory.
pub(crate) unsafe fn safe_read_u8(addr: usize) -> Option<u8> {
    let mut value: u8 = 0;
    let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    let ok = unsafe {
        ReadProcessMemory(
            CURRENT_PROCESS_PSEUDO_HANDLE,
            addr as *const c_void,
            &mut value as *mut u8 as *mut c_void,
            std::mem::size_of::<u8>(),
            &mut read,
        )
    };
    if ok != HOOK_FALSE_RETURN as i32 && read == std::mem::size_of::<u8>() {
        Some(value)
    } else {
        None
    }
}

pub(crate) unsafe fn find_title_owner_by_vtable(module_base: usize) -> Option<*mut u8> {
    TITLE_OWNER_SCAN_ATTEMPTS.fetch_add(1, Ordering::SeqCst);
    let target_vtable = module_base.checked_add(TITLE_OWNER_VTABLE_RVA)?;
    let mut scan_buf = vec![MOVIE_SKIP_FLAG_CLEAR; SCAN_CHUNK_SIZE];
    let mut address = TITLE_OWNER_SCAN_START_ADDRESS;
    while address < TITLE_OWNER_SCAN_MAX_ADDRESS {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe {
            VirtualQuery(
                Some(address as *const c_void),
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried == TITLE_OWNER_QUERY_FAILED_BYTES {
            break;
        }

        let base = info.BaseAddress as usize;
        let size = info.RegionSize;
        let next = base.saturating_add(size);
        let state = info.State.0;
        let protect = info.Protect.0;
        if state == MEM_COMMIT_NUMERIC
            && protect & (PAGE_NOACCESS_NUMERIC | PAGE_GUARD_NUMERIC) == PAGE_PROTECTION_NO_FLAGS
            && size >= TITLE_OWNER_STATE_OFFSET + std::mem::size_of::<i32>()
        {
            // Read the region in chunks via ReadProcessMemory (a chunk freed by
            // the booting game returns FALSE instead of faulting), then scan each
            // buffer in-process. One syscall per 64KB keeps the scan fast.
            let mut region_off = TITLE_OWNER_SCAN_START_ADDRESS;
            while region_off < size {
                let chunk = (size - region_off).min(SCAN_CHUNK_SIZE);
                let chunk_base = base + region_off;
                let mut read: usize = TITLE_OWNER_SCAN_START_ADDRESS;
                let ok = unsafe {
                    ReadProcessMemory(
                        CURRENT_PROCESS_PSEUDO_HANDLE,
                        chunk_base as *const c_void,
                        scan_buf.as_mut_ptr() as *mut c_void,
                        chunk,
                        &mut read,
                    )
                };
                if ok != HOOK_FALSE_RETURN as i32 && read >= std::mem::size_of::<usize>() {
                    let mut i = TITLE_OWNER_SCAN_START_ADDRESS;
                    while i + std::mem::size_of::<usize>() <= read {
                        let vtable = usize::from_le_bytes(
                            scan_buf[i..i + std::mem::size_of::<usize>()]
                                .try_into()
                                .unwrap(),
                        );
                        if vtable == target_vtable {
                            TITLE_OWNER_SCAN_VTABLE_HITS.fetch_add(1, Ordering::SeqCst);
                            let cursor = chunk_base + i;
                            TITLE_OWNER_SCAN_LAST_CANDIDATE.store(cursor, Ordering::SeqCst);
                            // Validate the per-instance state-table pointer (rejects
                            // the stray .data match 0x1000ffc58); fault-tolerant.
                            let instance_table = unsafe {
                                safe_read_usize(cursor + TITLE_OWNER_INSTANCE_TABLE_OFFSET)
                            };
                            let state_value =
                                unsafe { safe_read_i32(cursor + TITLE_OWNER_STATE_OFFSET) };
                            TITLE_OWNER_SCAN_LAST_TABLE.store(
                                instance_table.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS),
                                Ordering::SeqCst,
                            );
                            TITLE_OWNER_SCAN_LAST_STATE_BITS.store(
                                state_value.map_or(usize::MAX, |s| s as u32 as usize),
                                Ordering::SeqCst,
                            );
                            let table_ok =
                                instance_table == Some(module_base + INNER_TITLE_STATE_TABLE_RVA);
                            let state_ok = state_value.is_some_and(|s| {
                                (TITLE_OWNER_MIN_STATE..=TITLE_OWNER_MAX_STATE).contains(&s)
                            });
                            if table_ok && state_ok {
                                return Some(cursor as *mut u8);
                            }
                            if !table_ok {
                                TITLE_OWNER_SCAN_TABLE_REJECTS.fetch_add(1, Ordering::SeqCst);
                            } else if !state_ok {
                                TITLE_OWNER_SCAN_STATE_REJECTS.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                        i += TITLE_OWNER_SCAN_ALIGNMENT;
                    }
                }
                region_off = region_off.saturating_add(chunk);
            }
        }

        if next <= address {
            break;
        }
        address = next;
    }
    None
}

pub(crate) unsafe fn title_owner(module_base: usize) -> Option<*mut u8> {
    let cached = TITLE_OWNER_PTR.load(Ordering::SeqCst) as *mut u8;
    if !cached.is_null() {
        return Some(cached);
    }
    // Throttle the full-memory scan: until the owner exists it would otherwise
    // run every frame and cripple FPS (observed ~2 task ticks/s).
    let countdown = TITLE_OWNER_SCAN_COUNTDOWN.load(Ordering::SeqCst);
    if countdown > TITLE_OWNER_SCAN_COUNTDOWN_READY {
        TITLE_OWNER_SCAN_COUNTDOWN.fetch_sub(TITLE_OWNER_SCAN_COUNTDOWN_STEP, Ordering::SeqCst);
        return None;
    }
    TITLE_OWNER_SCAN_COUNTDOWN.store(TITLE_OWNER_SCAN_CALL_INTERVAL, Ordering::SeqCst);
    let found = unsafe { find_title_owner_by_vtable(module_base) }?;
    TITLE_OWNER_PTR.store(found as usize, Ordering::SeqCst);
    let state_value = unsafe { *(found.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    append_autoload_debug(format_args!(
        "native_title_job: captured title owner={found:p} state={state_value}"
    ));
    Some(found)
}

pub(crate) unsafe fn call_native_title_job_once(module_base: usize, tick: u64) -> bool {
    if TITLE_NATIVE_JOB_CALLED.load(Ordering::SeqCst) != TITLE_NATIVE_JOB_NOT_CALLED {
        return true;
    }
    if tick < TITLE_NATIVE_JOB_MIN_TICK {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for min tick tick={tick} target={TITLE_NATIVE_JOB_MIN_TICK}"
            ));
        }
        return false;
    }
    let Some(owner) = (unsafe { title_owner(module_base) }) else {
        let count = TITLE_OWNER_TRACE_COUNT
            .fetch_add(TITLE_TRACE_SEQUENCE_INCREMENT, Ordering::SeqCst)
            + TITLE_TRACE_SEQUENCE_INCREMENT;
        if count <= TITLE_OWNER_TRACE_LIMIT {
            append_autoload_debug(format_args!(
                "native_title_job: waiting for title owner at tick={tick}"
            ));
        }
        return false;
    };

    let state_before = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    let mut task_data = [TITLE_NATIVE_JOB_TASK_DATA_ZERO; TITLE_NATIVE_JOB_TASK_DATA_BYTES];
    let frame_delta = TITLE_NATIVE_JOB_FRAME_DELTA_NUMERATOR / TITLE_NATIVE_JOB_FRAME_RATE;
    task_data[TITLE_NATIVE_JOB_DELTA_OFFSET_START..TITLE_NATIVE_JOB_DELTA_OFFSET_END]
        .copy_from_slice(&frame_delta.to_le_bytes());
    let title_menu_job: unsafe extern "system" fn(*mut u8, *mut c_void) =
        unsafe { std::mem::transmute(module_base + TITLE_MENU_JOB_WAIT_RVA) };
    append_autoload_debug(format_args!(
        "native_title_job: ENTER owner={owner:p} state_before={state_before} tick={tick}"
    ));
    unsafe { title_menu_job(owner, task_data.as_mut_ptr().cast()) };
    let state_after = unsafe { *(owner.add(TITLE_OWNER_STATE_OFFSET) as *const i32) };
    TITLE_NATIVE_JOB_CALLED.store(TITLE_NATIVE_JOB_CALLED_VALUE, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native_title_job: LEAVE owner={owner:p} state_after={state_after} tick={tick}"
    ));
    true
}

#[derive(Clone, Copy)]
pub(crate) struct MenuTraceSnapshot {
    pub(crate) seq: usize,
    pub(crate) hook_rva: usize,
    pub(crate) table_rva: usize,
    pub(crate) this_ptr: usize,
    pub(crate) state_qword: usize,
    pub(crate) payload_ptr: usize,
}

impl MenuTraceSnapshot {
    pub(crate) fn advanced_from(self, previous: Self) -> bool {
        self.seq != previous.seq
            || self.hook_rva != previous.hook_rva
            || self.table_rva != previous.table_rva
            || self.this_ptr != previous.this_ptr
            || self.state_qword != previous.state_qword
            || self.payload_ptr != previous.payload_ptr
    }

    pub(crate) fn barrier_id(self) -> String {
        format!(
            "hook_0x{:x}/table_{}",
            self.hook_rva,
            trace_rva_label(self.table_rva)
        )
    }

    pub(crate) fn summary(self) -> String {
        format!(
            "last_menu_seq={} hook_rva=0x{:x} table_rva={} this=0x{:x} state_qword=0x{:x} payload_ptr=0x{:x}",
            self.seq,
            self.hook_rva,
            trace_rva_label(self.table_rva),
            self.this_ptr,
            self.state_qword,
            self.payload_ptr
        )
    }
}

pub(crate) fn menu_trace_snapshot() -> MenuTraceSnapshot {
    MenuTraceSnapshot {
        seq: MENU_TRACE_LAST_SEQ.load(Ordering::SeqCst),
        hook_rva: MENU_TRACE_LAST_HOOK_RVA.load(Ordering::SeqCst),
        table_rva: MENU_TRACE_LAST_TABLE_RVA.load(Ordering::SeqCst),
        this_ptr: MENU_TRACE_LAST_THIS.load(Ordering::SeqCst),
        state_qword: MENU_TRACE_LAST_STATE_QWORD.load(Ordering::SeqCst),
        payload_ptr: MENU_TRACE_LAST_PAYLOAD_PTR.load(Ordering::SeqCst),
    }
}

pub(crate) fn trace_rva_label(rva: usize) -> String {
    if rva == TRACE_UNKNOWN_TABLE_RVA as usize {
        "unknown".to_owned()
    } else {
        format!("0x{rva:x}")
    }
}

pub(crate) fn append_confirm_probe(
    phase: &str,
    pulse_seq: usize,
    tick: u64,
    snapshot: MenuTraceSnapshot,
    advanced_after_pulse: Option<bool>,
) {
    let advanced =
        advanced_after_pulse.map_or_else(|| "unknown".to_owned(), |value| value.to_string());
    let line = format!(
        "confirm_probe phase={phase} pulse={pulse_seq} tick={tick} menu_condition[unknown_confirmable_modal] barrier_id={} observed_after_pulse={advanced} confirm_active={} {} {}",
        snapshot.barrier_id(),
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES,
        snapshot.summary(),
        game_man_trace_summary()
    );
    append_autoload_debug(format_args!("{line}"));
    append_continue_trace(format_args!("{line}"));
}

pub(crate) unsafe fn menu_task_state_summary(this: *mut c_void) -> (usize, usize, String) {
    if this.is_null() {
        return (
            MENU_TASK_NULL_STATE_QWORD,
            MENU_TASK_NULL_PAYLOAD_PTR,
            "task_state{null=true}".to_owned(),
        );
    }
    let base = this.cast::<u8>();
    let state_qword = unsafe { *(base.cast::<usize>()) };
    let state_code = unsafe { *(base.cast::<i32>()) };
    let state_payload = unsafe { *(base.add(MENU_TASK_STATE_PAYLOAD_CODE_OFFSET).cast::<i32>()) };
    let delay_bits = unsafe { *(base.add(MENU_TASK_STATE_DELAY_OFFSET).cast::<u32>()) };
    let payload_ptr = unsafe { *(base.add(MENU_TASK_STATE_PAYLOAD_PTR_OFFSET).cast::<usize>()) };
    (
        state_qword,
        payload_ptr,
        format!(
            "task_state{{qword=0x{state_qword:x},code={state_code},payload={state_payload},delay_bits=0x{delay_bits:x},payload_ptr=0x{payload_ptr:x}}}"
        ),
    )
}

pub(crate) fn record_menu_trace_snapshot(
    seq: usize,
    hook_rva: u32,
    table_rva: u32,
    this: *mut c_void,
    state_qword: usize,
    payload_ptr: usize,
) {
    MENU_TRACE_LAST_SEQ.store(seq, Ordering::SeqCst);
    MENU_TRACE_LAST_HOOK_RVA.store(hook_rva as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_TABLE_RVA.store(table_rva as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_THIS.store(this as usize, Ordering::SeqCst);
    MENU_TRACE_LAST_STATE_QWORD.store(state_qword, Ordering::SeqCst);
    MENU_TRACE_LAST_PAYLOAD_PTR.store(payload_ptr, Ordering::SeqCst);
}

pub(crate) unsafe fn append_menu_semaphore_trace(
    hook_name: &str,
    phase: &str,
    hook_rva: u32,
    table_rva: u32,
    this: *mut c_void,
) {
    let seq = MENU_TRACE_EVENT_SEQ.fetch_add(MENU_TRACE_EVENT_INCREMENT, Ordering::SeqCst)
        + MENU_TRACE_EVENT_INCREMENT;
    let (state_qword, payload_ptr, task_state) = unsafe { menu_task_state_summary(this) };
    record_menu_trace_snapshot(seq, hook_rva, table_rva, this, state_qword, payload_ptr);
    append_continue_trace(format_args!(
        "menu_semaphore seq={seq} phase={phase} hook={hook_name} hook_rva=0x{hook_rva:x} table_rva={} this={this:p} barrier_id=hook_0x{hook_rva:x}/table_{} confirm_active={} pulse={} {} {} {}",
        trace_rva_label(table_rva as usize),
        trace_rva_label(table_rva as usize),
        SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst) > NO_SAFE_INPUT_CONFIRM_FRAMES,
        SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
        task_state,
        trace_callers_summary(),
        game_man_trace_summary()
    ));
}

pub(crate) fn game_man_trace_summary() -> String {
    // Named GameMan fields bound to the upstream typed layout (self-validating, dedups the
    // crate-level consts). The b73/b74/b75/bb8/bbc/bc0/bc4 flags read upstream-unnamed regions,
    // so they stay hand-decoded.
    const GAME_MAN_SAVE_SLOT_OFFSET: usize = core::mem::offset_of!(GameMan, save_slot);
    const GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET: usize =
        core::mem::offset_of!(GameMan, requested_save_slot_load_index);
    const GAME_MAN_SAVE_STATE_OFFSET: usize = core::mem::offset_of!(GameMan, save_state);
    const GAME_MAN_FLAG_B72_OFFSET: usize = core::mem::offset_of!(GameMan, save_requested);
    const GAME_MAN_FLAG_B73_OFFSET: usize = GAME_MAN_FLAG_B73_PROBE_OFFSET;
    const GAME_MAN_FLAG_B74_OFFSET: usize = GAME_MAN_FLAG_B73_OFFSET + core::mem::size_of::<u8>();
    const GAME_MAN_FLAG_B75_OFFSET: usize = GAME_MAN_FLAG_B75_PROBE_OFFSET;
    const GAME_MAN_FLAG_BC4_OFFSET: usize = crate::GAME_MAN_FLAG_BC4_OFFSET;
    const GAME_MAN_FLAG_BB8_OFFSET: usize = GAME_MAN_FLAG_BC4_OFFSET
        - core::mem::size_of::<u32>()
        - core::mem::size_of::<u32>()
        - core::mem::size_of::<u32>();
    const GAME_MAN_FLAG_BBC_OFFSET: usize = GAME_MAN_FLAG_BB8_OFFSET + core::mem::size_of::<u32>();
    const GAME_MAN_FLAG_BC0_OFFSET: usize = GAME_MAN_FLAG_BBC_OFFSET + core::mem::size_of::<u32>();

    unsafe {
        let game_man = game_man_ptr_or_null() as *const u8;
        if game_man.is_null() {
            return "gm=null".to_owned();
        }

        let read_i32 = |offset: usize| *(game_man.add(offset) as *const i32);
        let read_u8 = |offset: usize| *game_man.add(offset);
        let requested_slot_index = read_i32(GAME_MAN_REQUESTED_SAVE_SLOT_LOAD_INDEX_OFFSET);
        let save_state = read_i32(GAME_MAN_SAVE_STATE_OFFSET);
        format!(
            "gm={game_man:p} slot={} req_idx={} b78={} state={} b80={} flags{{b72={},b73={},b74={},b75={},bb8={}}} bbc={} bc0={} bc4={}",
            read_i32(GAME_MAN_SAVE_SLOT_OFFSET),
            requested_slot_index,
            requested_slot_index,
            save_state,
            save_state,
            read_u8(GAME_MAN_FLAG_B72_OFFSET),
            read_u8(GAME_MAN_FLAG_B73_OFFSET),
            read_u8(GAME_MAN_FLAG_B74_OFFSET),
            read_u8(GAME_MAN_FLAG_B75_OFFSET),
            read_u8(GAME_MAN_FLAG_BB8_OFFSET),
            read_i32(GAME_MAN_FLAG_BBC_OFFSET),
            read_i32(GAME_MAN_FLAG_BC0_OFFSET),
            read_i32(GAME_MAN_FLAG_BC4_OFFSET),
        )
    }
}

pub(crate) unsafe fn create_continue_trace_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    rva: u32,
    hook_impl: *mut c_void,
    original: &AtomicUsize,
) {
    let Ok(addr) = game_rva(rva) else {
        append_continue_trace(format_args!("hook {name}: failed to resolve rva=0x{rva:x}"));
        return;
    };

    match unsafe { MhHook::new(addr as *mut c_void, hook_impl) } {
        Ok(hook) => {
            original.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_continue_trace(format_args!("hook {name}: queue_enable failed: {status:?}"));
            } else {
                append_continue_trace(format_args!(
                    "hook {name}: target=0x{addr:x} trampoline={:p}",
                    hook.trampoline()
                ));
                hooks.push(hook);
            }
        }
        Err(status) => append_continue_trace(format_args!(
            "hook {name}: create failed at 0x{addr:x}: {status:?}"
        )),
    }
}

pub(crate) fn install_continue_trace_hooks() {
    write_bootstrap_event(
        BOOTSTRAP_EVENT_CONTINUE_TRACE_STARTED,
        BOOTSTRAP_DETAIL_START,
    );
    // Local Proton executable RVAs. The shared Ghidra 1.16.1 function starts are
    // currently +0xf0 for these text symbols; these RVAs are verified against
    // /home/banon/.local/share/Steam/.../eldenring.exe sha256
    // 34102b1c08bb5f769a724427a6f70fe29b3b732c31cf73693f861c48d3492ddb.
    const MENU_CONTINUE_WRAPPER_RVA: u32 = TRACE_MENU_CONTINUE_WRAPPER_RVA;
    const MENU_NEW_OR_LOAD_WRAPPER_RVA: u32 = TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA;
    const MENU_OTHER_LOAD_WRAPPER_RVA: u32 = er_save_loader::MENU_OTHER_LOAD_WRAPPER_RVA;
    const SET_SAVE_SLOT_RVA: u32 = er_save_loader::SET_SAVE_SLOT_RVA;
    const SAVE_REQUEST_PROFILE_RVA: u32 = er_save_loader::SAVE_REQUEST_PROFILE_RVA;
    const REQUEST_SAVE_RVA: u32 = er_save_loader::REQUEST_SAVE_RVA;
    const CURRENT_SLOT_LOAD_RVA: u32 = 0x0067b570;
    const CONTINUE_LOAD_RVA: u32 = 0x0067b750;
    const COMBINED_LOAD_RVA: u32 = 0x0067b940;
    const MAP_LOAD_RVA: u32 = 0x0067bc10;
    const SAVE_LOAD_STATE_INIT_RVA: u32 = er_save_loader::SAVE_LOAD_STATE_INIT_RVA;

    append_continue_trace(format_args!(
        "install_continue_trace_hooks begin {}",
        game_man_trace_summary()
    ));

    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        status => {
            append_continue_trace(format_args!("MH_Initialize failed: {status:?}"));
            return;
        }
    }

    let mut hooks = Vec::new();
    unsafe {
        create_continue_trace_hook(
            &mut hooks,
            "menu_continue_wrapper",
            MENU_CONTINUE_WRAPPER_RVA,
            menu_continue_wrapper_hook as *mut c_void,
            &MENU_CONTINUE_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "menu_new_or_load_wrapper",
            MENU_NEW_OR_LOAD_WRAPPER_RVA,
            menu_new_or_load_wrapper_hook as *mut c_void,
            &MENU_NEW_OR_LOAD_WRAPPER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "menu_other_load_wrapper",
            MENU_OTHER_LOAD_WRAPPER_RVA,
            menu_other_load_wrapper_hook as *mut c_void,
            &MENU_OTHER_LOAD_WRAPPER_ORIG,
        );
        if trace_menu_task_update_enabled() {
            create_continue_trace_hook(
                &mut hooks,
                "menu_task_update_wrapper",
                TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
                menu_task_update_wrapper_hook as *mut c_void,
                &MENU_TASK_UPDATE_WRAPPER_ORIG,
            );
        } else {
            append_continue_trace(format_args!(
                "menu_task_update_wrapper trace skipped by default; set ER_EFFECTS_TRACE_MENU_TASK_UPDATE=1 for invasive pump diagnostics"
            ));
        }
        create_continue_trace_hook(
            &mut hooks,
            "native_submit_7ac890",
            MENU_ITEM_SUBMIT_RVA as u32,
            native_submit_hook as *mut c_void,
            &NATIVE_SUBMIT_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "result_event_handler_746e80",
            RESULT_EVENT_HANDLER_RVA,
            result_event_handler_hook as *mut c_void,
            &RESULT_EVENT_HANDLER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "result_action_builder_746a00",
            RESULT_ACTION_BUILDER_RVA,
            result_action_builder_hook as *mut c_void,
            &RESULT_ACTION_BUILDER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "result_event_wrapper_builder_744a60",
            RESULT_EVENT_WRAPPER_BUILDER_RVA,
            result_event_wrapper_builder_hook as *mut c_void,
            &RESULT_EVENT_WRAPPER_BUILDER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "task_enqueue_7a7b60",
            TRACE_TASK_ENQUEUE_RVA,
            task_enqueue_hook as *mut c_void,
            &TASK_ENQUEUE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "set_save_slot",
            SET_SAVE_SLOT_RVA,
            set_save_slot_hook as *mut c_void,
            &SET_SAVE_SLOT_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "save_request_profile",
            SAVE_REQUEST_PROFILE_RVA,
            save_request_profile_hook as *mut c_void,
            &SAVE_REQUEST_PROFILE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "request_save",
            REQUEST_SAVE_RVA,
            request_save_hook as *mut c_void,
            &REQUEST_SAVE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "current_slot_load_67b570",
            CURRENT_SLOT_LOAD_RVA,
            current_slot_load_hook as *mut c_void,
            &CURRENT_SLOT_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "continue_load_67b750",
            CONTINUE_LOAD_RVA,
            continue_load_hook as *mut c_void,
            &CONTINUE_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "combined_load_67b940",
            COMBINED_LOAD_RVA,
            combined_load_hook as *mut c_void,
            &COMBINED_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "map_load_67bc10",
            MAP_LOAD_RVA,
            map_load_hook as *mut c_void,
            &MAP_LOAD_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "save_load_state_init_67b030",
            SAVE_LOAD_STATE_INIT_RVA,
            save_load_state_init_hook as *mut c_void,
            &SAVE_LOAD_STATE_INIT_ORIG,
        );
        // b80 save-mount capture: the 5 functions that drive the slot deserialize. A real
        // user-driven .co2 load through these pins the exact call order + args + which fn
        // populates io18/io20 + which transitions b80 + which applies the character, so we
        // can replicate it with slot-int primitives (no synthetic-owner save-write).
        create_continue_trace_hook(
            &mut hooks,
            "b80_preview_67b4e0",
            LOAD_INITIATOR_RVA as u32,
            b80_preview_initiator_hook as *mut c_void,
            &B80_PREVIEW_INITIATOR_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_loadsavedata_67b200",
            B80_LOAD_SAVE_DATA_INITIATOR_RVA as u32,
            b80_loadsavedata_hook as *mut c_void,
            &B80_LOAD_SAVE_DATA_INITIATOR_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_fullload_67b1a0",
            B80_FULL_LOAD_INITIATOR_RVA as u32,
            b80_fullload_hook as *mut c_void,
            &B80_FULL_LOAD_INITIATOR_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_poll_679180",
            B80_POLL_RVA as u32,
            b80_poll_hook as *mut c_void,
            &B80_POLL_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_deserialize_67b290",
            DESERIALIZE_SLOT_RVA as u32,
            b80_deserialize_hook as *mut c_void,
            &B80_DESERIALIZE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "b80_dispatcher2_afb880_observe",
            B80_DISPATCHER2_RVA as u32,
            b80_dispatcher2_observe_hook as *mut c_void,
            &B80_DISPATCHER2_OBSERVE_ORIG,
        );
        // NOTE: the c30_writer 0x67bd70 hook is NOT installed here. It is installed
        // UNCONDITIONALLY at process attach via install_c30_writer_hook (mirroring the
        // MenuWindow-latch precedent) so the SAVE-SAFE c30-write diagnostic is always
        // armed without requiring the continue-trace path. Installing it twice on the
        // same address would make the second MhHook::new fail, so it lives only there.
        // MENU-UI capture (Path B state-stepper). One real navigation through these pins the
        // this-pointers + construction order + call sequence for the 4 user interactions:
        // SetState (state machine), Continue confirm, ProfileLoadDialog activate (both
        // variants), the enter-Load-Game builder, the selector-step tick, and the mount.
        const CAP_SETSTATE_RVA: u32 = 0x00b0d960;
        const CAP_CONTINUE_CONFIRM_RVA: u32 = 0x00b0e180;
        const CAP_LOAD_ACTIVATE_RVA: u32 = 0x009a4670;
        const CAP_LOAD_ACTIVATE2_RVA: u32 = 0x009ac760;
        const CAP_BUILDER_RVA: u32 = 0x00826510;
        const CAP_SELECTOR_TICK_RVA: u32 = PROFILE_LOAD_SELECTOR_TICK_RVA as u32;
        const CAP_MENU_DESER_RVA: u32 = ProfileLoadMenuRva::MenuDeser as u32;
        const CAP_DIALOG_FACTORY_RVA: u32 = LIVE_DIALOG_FACTORY_RVA as u32;
        create_continue_trace_hook(
            &mut hooks,
            "cap_setstate_b0d960",
            CAP_SETSTATE_RVA,
            cap_setstate_hook as *mut c_void,
            &CAP_SETSTATE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_continue_confirm_b0e180",
            CAP_CONTINUE_CONFIRM_RVA,
            cap_continue_confirm_hook as *mut c_void,
            &CAP_CONTINUE_CONFIRM_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_load_activate_9a4670",
            CAP_LOAD_ACTIVATE_RVA,
            cap_load_activate_hook as *mut c_void,
            &CAP_LOAD_ACTIVATE_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_load_activate2_9ac760",
            CAP_LOAD_ACTIVATE2_RVA,
            cap_load_activate2_hook as *mut c_void,
            &CAP_LOAD_ACTIVATE2_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_builder_826510",
            CAP_BUILDER_RVA,
            cap_builder_hook as *mut c_void,
            &CAP_BUILDER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_selector_tick_826d50",
            CAP_SELECTOR_TICK_RVA,
            cap_selector_tick_hook as *mut c_void,
            &CAP_SELECTOR_TICK_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_deser_82c240",
            CAP_MENU_DESER_RVA,
            cap_menu_deser_hook as *mut c_void,
            &CAP_MENU_DESER_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_dialog_factory_81ead0",
            CAP_DIALOG_FACTORY_RVA,
            cap_dialog_factory_hook as *mut c_void,
            &CAP_DIALOG_FACTORY_ORIG,
        );
        // MenuWindowJob ctor 0x1407ac8c0: latch semantic Continue items at construction before
        // the first updated/idle title input leaf can poison MENU_CONTINUE_ITEM.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_window_job_ctor_7ac8c0",
            MENU_WINDOW_JOB_CTOR_RVA,
            menu_window_job_ctor_hook as *mut c_void,
            &MENU_WINDOW_JOB_CTOR_ORIG,
        );
        // MenuWindowJob native-accept ctor variant 0x1407acb00: observe/latch semantic Continue
        // rows built by the sibling constructor that also installs native accept 0x1407ad810.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_window_job_native_ctor_b_7acb00",
            MENU_WINDOW_JOB_NATIVE_CTOR_B_RVA,
            menu_window_job_native_ctor_b_hook as *mut c_void,
            &MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG,
        );
        // MenuWindowJob idle ctor 0x1407acf80: static RE shows this neighboring constructor
        // installs the constant-false accept predicate 0x1407add70. Observe it separately so a
        // Continue-looking row with idle accept can be attributed to the disabled native path.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_window_job_idle_ctor_7acf80",
            MENU_WINDOW_JOB_IDLE_CTOR_RVA,
            menu_window_job_idle_ctor_hook as *mut c_void,
            &MENU_WINDOW_JOB_IDLE_CTOR_ORIG,
        );
        // Title native-ready predicate 0x140733150: the native title builder calls this on
        // title_dialog+0x2610 before constructing native-accept rows. Observe the exact result
        // and state flags so product-core can wait for the native condition instead of promoting
        // idle rows.
        create_continue_trace_hook(
            &mut hooks,
            "cap_title_native_ready_733150",
            TITLE_NATIVE_READY_PREDICATE_RVA,
            title_native_ready_predicate_hook as *mut c_void,
            &TITLE_NATIVE_READY_PREDICATE_ORIG,
        );
        // Menu-item Update 0x1407ad1c0: capture the live Load-Game item (functor ->
        // dialog_factory) by letting the native pump walk its own CSMenu tree.
        create_continue_trace_hook(
            &mut hooks,
            "cap_menu_item_update_7ad1c0",
            MENU_ITEM_UPDATE_RVA,
            cap_menu_item_update_hook as *mut c_void,
            &MENU_ITEM_UPDATE_ORIG,
        );
        // Sequence child-iterator 0x1407aa1f0: enumerate every Sequence's children to capture
        // the Load-Game leaf d180 even though it does not tick (only the focused entry ticks
        // the leaf Update above).
        create_continue_trace_hook(
            &mut hooks,
            "cap_sequence_iter_7aa1f0",
            SEQUENCE_ITER_RVA,
            cap_sequence_iter_hook as *mut c_void,
            &SEQUENCE_ITER_ORIG,
        );
        // CSMenu controller ctor 0x1409060d0: latch router_this (owns the selectable-row vector
        // at +0x1290) -- it is NOT field-linked from the TitleTopDialog, so capturing it at
        // construction is how the own-stepper reaches the Continue/Load rows zero-input.
        create_continue_trace_hook(
            &mut hooks,
            "cap_csmenu_ctor_9060d8",
            CSMENU_CTOR_RVA,
            cap_csmenu_ctor_hook as *mut c_void,
            &CAP_CSMENU_CTOR_ORIG,
        );
        // Row-push functions (reliable .text): if either fires headless the rows materialize
        // zero-input; if neither does, the interactive menu controller is input-instantiated.
        create_continue_trace_hook(
            &mut hooks,
            "cap_rebuild_rows_78d2c0",
            REBUILD_ROWS_RVA,
            cap_rebuild_rows_hook as *mut c_void,
            &CAP_REBUILD_ROWS_ORIG,
        );
        create_continue_trace_hook(
            &mut hooks,
            "cap_append_one_78eea0",
            APPEND_ONE_RVA,
            cap_append_one_hook as *mut c_void,
            &CAP_APPEND_ONE_ORIG,
        );
    }

    match unsafe { MH_ApplyQueued() } {
        MH_STATUS::MH_OK => {
            write_bootstrap_event(
                BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLIED,
                BOOTSTRAP_DETAIL_DONE,
            );
            append_continue_trace(format_args!(
                "install_continue_trace_hooks applied count={} {}",
                hooks.len(),
                game_man_trace_summary()
            ));
        }
        status => {
            let detail = format!("MH_ApplyQueued failed: {status:?}");
            write_bootstrap_event(BOOTSTRAP_EVENT_CONTINUE_TRACE_APPLY_FAILED, &detail);
            append_continue_trace(format_args!("{detail}"));
        }
    }

    std::mem::forget(hooks);
}

pub(crate) unsafe fn call_wrapper_original(
    original: &AtomicUsize,
    this: *mut c_void,
) -> Option<*mut c_void> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(this) })
}

pub(crate) unsafe fn call_bool3_original(
    original: &AtomicUsize,
    arg0: i32,
    arg1: u8,
    arg2: u8,
) -> Option<u8> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(i32, u8, u8) -> u8 =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(arg0, arg1, arg2) })
}

pub(crate) unsafe fn call_task_enqueue_original(
    arg0: *mut c_void,
    arg1: *mut c_void,
) -> Option<*mut c_void> {
    let original = TASK_ENQUEUE_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(arg0, arg1) })
}

pub(crate) unsafe fn call_result_void1_original(
    original: &AtomicUsize,
    result: usize,
) -> Option<()> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(usize) = unsafe { std::mem::transmute(original) };
    unsafe { original(result) };
    Some(())
}

pub(crate) unsafe fn call_result_void2_original(
    original: &AtomicUsize,
    result: usize,
    event: usize,
) -> Option<()> {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(usize, usize) =
        unsafe { std::mem::transmute(original) };
    unsafe { original(result, event) };
    Some(())
}

pub(crate) unsafe fn call_wrapper_builder_original(
    rcx: usize,
    rdx: usize,
    r8: usize,
) -> Option<usize> {
    let original = RESULT_EVENT_WRAPPER_BUILDER_ORIG.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return None;
    }
    let original: unsafe extern "system" fn(usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    Some(unsafe { original(rcx, rdx, r8) })
}

/// Defensive default when a b80 trampoline is somehow unset (dead branch: if our hook
/// runs, MhHook installed and the trampoline is set).
const B80_HOOK_DEFAULT_RET: i32 = 0;

/// State snapshot for the b80 save-mount capture: the GameMan load-phase fields plus the
/// iodev request-handle pair the poll keys on. Logged at ENTER and LEAVE of each hooked
/// b80 function so a real user-driven load pins which fn populates io18/io20, transitions
/// b80 0->1/2->3, and writes c30/ac0 (the character-apply). io18 && io20 set == the
/// deserialize-ready signature (real-load-c30-mount-write-confirmed-seamless-2026).
pub(crate) fn b80_mount_trace_summary() -> String {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Ok(base) = game_module_base() else {
        return "base_unresolved".to_owned();
    };
    let gm = game_man_ptr_or_null();
    let read_gm = |off: usize| {
        if gm != null {
            unsafe { *((gm + off) as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        }
    };
    let b80 = read_gm(GAME_MAN_LOAD_IN_PROGRESS_B80_OFFSET);
    let ac0 = read_gm(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
    let c30 = read_gm(GAME_MAN_SAVED_MAP_C30_OFFSET);
    let b78 = read_gm(GAME_MAN_REQUESTED_SLOT_B78_OFFSET);
    let iodev = unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) };
    let read_io = |off: usize| {
        if iodev != null {
            unsafe { *((iodev + off) as *const usize) }
        } else {
            null
        }
    };
    let io10 = read_io(IODEV_INFLIGHT_10_OFFSET);
    let io18 = read_io(IODEV_REQHANDLE_18_OFFSET);
    let io20 = read_io(IODEV_REQHANDLE_20_OFFSET);
    format!(
        "b80={b80} ac0={ac0} c30=0x{c30:x} b78={b78} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x}"
    )
}

/// Call an original slot-int b80 initiator/deserialize (fastcall, ecx=slot). Returns the
/// full eax the original produced so the game's caller sees the unmodified result.
unsafe fn call_b80_initiator_original(original: &AtomicUsize, slot: i32) -> i32 {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return B80_HOOK_DEFAULT_RET;
    }
    let original: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(original) };
    unsafe { original(slot) }
}

/// Call the original b80 poll 0x140679180(cl,dl). Returns its full eax (0 ready /
/// 1 in-progress / else error) so the dispatcher's switch is unchanged.
unsafe fn call_b80_poll_original(original: &AtomicUsize, arg0: u8, arg1: u8) -> i32 {
    let original = original.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return B80_HOOK_DEFAULT_RET;
    }
    let original: unsafe extern "system" fn(u8, u8) -> i32 =
        unsafe { std::mem::transmute(original) };
    unsafe { original(arg0, arg1) }
}

pub(crate) unsafe extern "system" fn b80_preview_initiator_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_preview_67b4e0 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_PREVIEW_INITIATOR_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_preview_67b4e0 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_loadsavedata_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_loadsavedata_67b200 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_LOAD_SAVE_DATA_INITIATOR_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_loadsavedata_67b200 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_fullload_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_fullload_67b1a0 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_FULL_LOAD_INITIATOR_ORIG, slot) };
    append_continue_trace(format_args!(
        "b80_fullload_67b1a0 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_poll_hook(arg0: u8, arg1: u8) -> i32 {
    append_continue_trace(format_args!(
        "b80_poll_679180 ENTER arg0={arg0} arg1={arg1} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_poll_original(&B80_POLL_ORIG, arg0, arg1) };
    append_continue_trace(format_args!(
        "b80_poll_679180 LEAVE ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn b80_dispatcher2_observe_hook(this: usize) -> u8 {
    if this != TITLE_OWNER_SCAN_START_ADDRESS {
        B80_NATIVE_DISPATCHER_OWNER.store(this, Ordering::SeqCst);
    }
    let count = B80_DISPATCHER2_OBSERVE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    let before = b80_mount_trace_summary();
    let ret = unsafe {
        let orig = B80_DISPATCHER2_OBSERVE_ORIG.load(Ordering::SeqCst);
        if orig == HOOK_ORIGINAL_UNSET {
            TITLE_OWNER_SCAN_START_ADDRESS as u8
        } else {
            let f: unsafe extern "system" fn(usize) -> u8 = std::mem::transmute(orig);
            f(this)
        }
    };
    if count < MENU_ITEM_UPDATE_LOG_MAX
        || before.contains("b80=1")
        || before.contains("b80=2")
        || before.contains("b80=3")
    {
        append_continue_trace(format_args!(
            "b80_dispatcher2_afb880 OBS this=0x{this:x} ret={ret} before{{{before}}} after{{{}}} {}",
            b80_mount_trace_summary(),
            trace_callers_summary()
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn b80_deserialize_hook(slot: i32) -> i32 {
    append_continue_trace(format_args!(
        "b80_deserialize_67b290 ENTER slot={slot} {}",
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_b80_initiator_original(&B80_DESERIALIZE_ORIG, slot) };
    const B80_DESERIALIZE_SUCCESS_RET: i32 = 1;
    const C30_ZERO: i32 = 0;
    let gm = game_man_ptr_or_null();
    if ret == B80_DESERIALIZE_SUCCESS_RET && gm != TITLE_OWNER_SCAN_START_ADDRESS {
        let c30 = unsafe { *((gm + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
        let ac0 = unsafe { *((gm + FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET) as *const i32) };
        if c30 != GAME_MAN_C30_UNSET && c30 != C30_ZERO {
            OWN_STEPPER_MOUNT_C30.store(c30, Ordering::SeqCst);
            OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
            append_autoload_debug(format_args!(
                "b80_deserialize_67b290: latched native post-click deserialize success slot={slot} ac0={ac0} c30=0x{c30:x}"
            ));
        }
    }
    append_continue_trace(format_args!(
        "b80_deserialize_67b290 LEAVE slot={slot} ret={ret} {}",
        b80_mount_trace_summary()
    ));
    ret
}

/// The SOLE GameMan+0xc30 writer 0x14067bd70(rcx=GameMan, rdx=buf, r8d=size). Logs the
/// CALLER STACK (which deserializer drove the c30 write -- the Wine-safe replacement
/// for the hardware watchpoint) + the mount state, then chains the original. If this
/// never fires during a Seamless .co2 load, ERSC writes c30 from its own module.
pub(crate) unsafe extern "system" fn c30_writer_hook(
    game_man: usize,
    buffer: usize,
    size: u32,
) -> usize {
    // SAVE-SAFE diagnostic (NO SetState5, NO save write): a pure passthrough that forwards
    // ALL args + returns the original's result. Rate-limited to the first few calls (the cold
    // deserialize drives a small bounded number of c30-writer entries). On ENTER we log the gate
    // [0x143d68078] (null -> writer returns without writing), c30 BEFORE, and a window of the
    // resident save buffer (rdx) so the REAL target map record can be spotted offline. On LEAVE
    // we log the return (al) + c30 AFTER, so we can see whether 0x67bd70 ran, whether it changed
    // c30, and to what. (coldmount-c30-is-the-single-key-write-conditions-and-recipe-2026)
    const C30_LOG_INC: usize = 1;
    const HEX_BYTES_PER_LINE: usize = 16;
    let log_n = C30_WRITER_LOG_COUNT.fetch_add(C30_LOG_INC, Ordering::SeqCst);
    let do_log = log_n < C30_WRITER_LOG_MAX;
    if do_log {
        let gate = game_module_base()
            .ok()
            .map(|base| unsafe { *((base + SAVE_DATA_SUBSYSTEM_GATE_RVA) as *const usize) })
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let c30_before = unsafe { *((game_man + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
        // Hex window of the resident 0x280000 save buffer header so the map record is visible.
        let mut hex = String::new();
        const BUFFER_DUMP_START: usize = 0;
        for i in BUFFER_DUMP_START..C30_WRITER_BUFFER_DUMP_BYTES {
            if i % HEX_BYTES_PER_LINE == TITLE_OWNER_SCAN_START_ADDRESS {
                hex.push(' ');
            }
            let byte = unsafe { *((buffer + i) as *const u8) };
            let _ = write!(hex, "{byte:02x}");
        }
        append_continue_trace(format_args!(
            "c30_writer_67bd70 ENTER#{log_n} game_man=0x{game_man:x} buf=0x{buffer:x} size=0x{size:x} gate(0x143d68078)=0x{gate:x} c30_before=0x{c30_before:x} buf[0..0x{:x}]={hex} {} {}",
            C30_WRITER_BUFFER_DUMP_BYTES,
            b80_mount_trace_summary(),
            trace_callers_summary()
        ));
    }
    let original = C30_WRITER_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        B80_HOOK_DEFAULT_RET as usize
    } else {
        let original: unsafe extern "system" fn(usize, usize, u32) -> usize =
            unsafe { std::mem::transmute(original) };
        unsafe { original(game_man, buffer, size) }
    };
    const C30_WRITER_FULL_SAVE_SIZE: u32 = 0x280000;
    const C30_WRITER_SUCCESS_RET: usize = 1;
    const C30_AFTER_ZERO: i32 = 0;
    let c30_after = unsafe { *((game_man + GAME_MAN_SAVED_MAP_C30_OFFSET) as *const i32) };
    if ret == C30_WRITER_SUCCESS_RET
        && size == C30_WRITER_FULL_SAVE_SIZE
        && c30_after != C30_AFTER_ZERO
    {
        OWN_STEPPER_MOUNT_C30.store(c30_after, Ordering::SeqCst);
        OWN_STEPPER_DESER_FIRED.store(OWN_STEPPER_DESER_FIRED_OK, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "c30_writer_67bd70: latched full-save native deser success c30=0x{c30_after:x} size=0x{size:x}"
        ));
    }
    if do_log {
        append_continue_trace(format_args!(
            "c30_writer_67bd70 LEAVE#{log_n} ret=0x{ret:x} c30_after=0x{c30_after:x} {}",
            b80_mount_trace_summary()
        ));
    }
    ret
}

pub(crate) unsafe extern "system" fn menu_continue_wrapper_hook(this: *mut c_void) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_continue_wrapper",
            "ENTER",
            TRACE_MENU_CONTINUE_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_CONTINUE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_continue_wrapper",
            "LEAVE",
            TRACE_MENU_CONTINUE_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

pub(crate) unsafe extern "system" fn menu_new_or_load_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_new_or_load_wrapper",
            "ENTER",
            TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_NEW_OR_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_new_or_load_wrapper",
            "LEAVE",
            TRACE_MENU_NEW_OR_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

pub(crate) unsafe extern "system" fn menu_other_load_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_other_load_wrapper",
            "ENTER",
            TRACE_MENU_OTHER_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_OTHER_LOAD_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_other_load_wrapper",
            "LEAVE",
            TRACE_MENU_OTHER_LOAD_WRAPPER_RVA,
            TRACE_UNKNOWN_TABLE_RVA,
            result,
        )
    };
    result
}

/// Forward a captured menu-UI call through its trampoline. Uniform 4-arg fastcall: the
/// integer arg registers (rcx/rdx/r8/r9) pass through; callees taking fewer args ignore the
/// rest, and none of the captured targets take >4 integer args or float args. Returns rax.
unsafe fn call_cap_original(orig: &AtomicUsize, a: usize, b: usize, c: usize, d: usize) -> usize {
    let original = orig.load(Ordering::SeqCst);
    if original == HOOK_ORIGINAL_UNSET {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    }
    let f: unsafe extern "system" fn(usize, usize, usize, usize) -> usize =
        unsafe { std::mem::transmute(original) };
    unsafe { f(a, b, c, d) }
}

/// Title CSMenu controller ctor 0x1409060d0 (real prologue entry; doc's 0x9060d8 was mid-
/// prologue): latches `router_this` (the object owning the
/// selectable Continue/Load/NewGame row vector at +0x1290) when its primary vtable
/// (runtime `base+0x2afa070`) is installed. router_this is NOT field-linked from the
/// TitleTopDialog, so this ctor capture is how the own-stepper obtains it. Pure observe +
/// pass-through; latches the first matching controller.
pub(crate) unsafe extern "system" fn cap_csmenu_ctor_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ROUTER_VEC_BEGIN_1290: usize = 0x1290;
    const ROUTER_VEC_END_1298: usize = 0x1298;
    let ret = unsafe { call_cap_original(&CAP_CSMENU_CTOR_ORIG, this, b, c, d) };
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    if this != NULL && base != NULL {
        let vt = unsafe { safe_read_usize(this) }.unwrap_or(NULL);
        let vt_rva = vt.wrapping_sub(base);
        let matched = vt == base + ROUTER_THIS_VTABLE_RVA;
        if matched {
            MENU_ROUTER_THIS.store(this, Ordering::SeqCst);
        }
        // Log the first N constructions REGARDLESS of match: reveals whether this ctor fires
        // headless at all and the ACTUAL installed runtime vtable (vt_rva), so the inferred
        // ROUTER_THIS_VTABLE_RVA=0x2afa070 (derived via a +0xe00 dump skew, not measured) can be
        // corrected if wrong.
        let n = CAP_CSMENU_CTOR_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if n < CAP_CSMENU_CTOR_LOG_FIRST {
            let vb = unsafe { safe_read_usize(this + ROUTER_VEC_BEGIN_1290) }.unwrap_or(NULL);
            let ve = unsafe { safe_read_usize(this + ROUTER_VEC_END_1298) }.unwrap_or(NULL);
            append_continue_trace(format_args!(
                "CAP csmenu_ctor #{n} this=0x{this:x} vt=0x{vt:x} vt_rva=0x{vt_rva:x} matched={matched} vec=[0x{vb:x}..0x{ve:x}] {}",
                trace_callers_summary()
            ));
        }
    }
    ret
}

/// Post-build scan of a row container (`rebuild_rows`/`append_one` rcx). The generic FD4 list
/// builder fires for EVERY menu list, so the title menu is identified by CONTENT: a row whose
/// action functor ([entry+0xf8] -> [+0] vtable -> [+0x10] _Do_call) chains to dialog_factory
/// 0x14081ead0 (Load-Game) or continue_confirm 0x140b0e180 (Continue). Captures the Load-Game /
/// Continue ROW ENTRIES (and router_this = container-0x1290) when found. Pure reads + classify
/// (the original already ran) -> save-safe. Called AFTER the original builds the rows.
unsafe fn inspect_row_container(tag: &str, container: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ENTRY_STRIDE_210: usize = 0x210;
    const ENTRY_ACTION_F8: usize = 0xf8;
    const ACTION_DOCALL_10: usize = 0x10;
    const ROW_VEC_OFFSET_1290: usize = 0x1290;
    const DIALOG_FACTORY_RVA: usize = LIVE_DIALOG_FACTORY_RVA;
    const PROBE_ENTRIES: usize = 8;
    const PROBE_START: usize = 0;
    const PROBE_STEP: usize = 1;
    const JMP_HOPS: usize = 5;
    const HOP_START: usize = 0;
    const HOP_STEP: usize = 1;
    if container == NULL {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    if base == NULL {
        return;
    }
    let factory = base + DIALOG_FACTORY_RVA;
    let confirm = base + CONTINUE_CONFIRM_RVA;
    let begin = unsafe { safe_read_usize(container) }.unwrap_or(NULL);
    if begin == NULL {
        return;
    }
    let mut load_entry: usize = NULL;
    let mut cont_entry: usize = NULL;
    let mut i = PROBE_START;
    while i < PROBE_ENTRIES {
        let entry = begin + i * ENTRY_STRIDE_210;
        let action = unsafe { safe_read_usize(entry + ENTRY_ACTION_F8) }.unwrap_or(NULL);
        if action != NULL {
            let avt = unsafe { safe_read_usize(action) }.unwrap_or(NULL);
            if avt != NULL {
                let mut tgt = unsafe { safe_read_usize(avt + ACTION_DOCALL_10) }.unwrap_or(NULL);
                let mut hop = HOP_START;
                while hop < JMP_HOPS && tgt != NULL {
                    if tgt == factory {
                        load_entry = entry;
                        break;
                    }
                    if tgt == confirm {
                        cont_entry = entry;
                        break;
                    }
                    match unsafe { decode_thunk_hop(tgt) } {
                        Some(next) => tgt = next,
                        None => break,
                    }
                    hop += HOP_STEP;
                }
            }
        }
        i += PROBE_STEP;
    }
    if load_entry == NULL && cont_entry == NULL {
        return;
    }
    // This container IS the title menu row list. Latch the entries + a router_this candidate.
    if load_entry != NULL {
        MENU_LOADGAME_ROW_ENTRY.store(load_entry, Ordering::SeqCst);
    }
    if cont_entry != NULL {
        MENU_CONTINUE_ROW_ENTRY.store(cont_entry, Ordering::SeqCst);
    }
    let router_this = container.wrapping_sub(ROW_VEC_OFFSET_1290);
    MENU_ROUTER_THIS.store(router_this, Ordering::SeqCst);
    let n = CAP_ROW_PUSH_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_ROW_PUSH_LOG_FIRST {
        let rvt = unsafe { safe_read_usize(router_this) }.unwrap_or(NULL);
        append_continue_trace(format_args!(
            "CAP row_push[{tag}] TITLE-MENU container=0x{container:x} begin=0x{begin:x} load_entry=0x{load_entry:x} cont_entry=0x{cont_entry:x} router_this?=0x{router_this:x} rvt=0x{rvt:x} {}",
            trace_callers_summary()
        ));
    }
}

/// rebuild_rows 0x14078d2c0(rcx=list-model container, rdx=src iterator pair): bulk-emplaces the
/// Continue/Load/NewGame rows. Firing headless proves the rows materialize zero-input; the
/// post-build scan isolates the title menu by row CONTENT.
pub(crate) unsafe extern "system" fn cap_rebuild_rows_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&CAP_REBUILD_ROWS_ORIG, a, b, c, d) };
    unsafe { log_row_push_caller("rebuild", a) };
    unsafe { inspect_row_container("rebuild", a) };
    ret
}

/// append_one 0x14078eea0(rcx=list-model, r8=&idx): single-row emplace.
pub(crate) unsafe extern "system" fn cap_append_one_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&CAP_APPEND_ONE_ORIG, a, b, c, d) };
    unsafe { log_row_push_caller("append", a) };
    unsafe { inspect_row_container("append", a) };
    ret
}

/// UNCONDITIONAL instrument-capture: log container + row-vector size + caller stack for the
/// first N rebuild_rows/append_one fires, regardless of content. This pins WHAT triggers the
/// TitleTopDialog CSMenu row populate (the input/focus-gated step confirmed missing zero-input).
/// Pure reads; the original already ran -> save-safe.
unsafe fn log_row_push_caller(tag: &str, container: usize) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const ROW_VEC_BEGIN_1290: usize = 0x1290;
    const ROW_VEC_END_1298: usize = 0x1298;
    let n = CAP_ROW_PUSH_ALLFIRE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n >= CAP_ROW_PUSH_ALLFIRE_LOG_FIRST {
        return;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != NULL {
            own
        } else {
            game_module_base().unwrap_or(NULL)
        }
    };
    // container is the list-model; router_this back-ptr at [container+8], its row vector lives at
    // router_this+0x1290. Also probe the container itself in case it IS router_this.
    let backptr = unsafe { safe_read_usize(container + ROW_CONTAINER_BACKPTR_8) }.unwrap_or(NULL);
    let vb = unsafe { safe_read_usize(container + ROW_VEC_BEGIN_1290) }.unwrap_or(NULL);
    let ve = unsafe { safe_read_usize(container + ROW_VEC_END_1298) }.unwrap_or(NULL);
    let cvt = unsafe { safe_read_usize(container) }.unwrap_or(NULL);
    let cvt_rva = if base != NULL {
        cvt.wrapping_sub(base)
    } else {
        cvt
    };
    append_continue_trace(format_args!(
        "CAP row_push_ALL[{tag}] #{n} container=0x{container:x} cvt=0x{cvt:x}(rva 0x{cvt_rva:x}) backptr=0x{backptr:x} vec=[0x{vb:x}..0x{ve:x}] {}",
        trace_callers_summary()
    ));
}

/// Menu/FD4 insertion helper 0x1407a7b60(rcx=registry/builder, rdx=descriptor): passive capture of
/// the exact objects TitleTopDialog::open_menu inserts. This is intentionally generic: log the
/// original return plus a few qwords around rcx/rdx so the next static/runtime step can identify the
/// registry storage without guessing dialog fields or generic Sequence trees.
unsafe fn log_menu_insert_details(a: usize, b: usize, c: usize, d: usize, ret: usize) {
    let n = CAP_MENU_INSERT_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_MENU_INSERT_LOG_FIRST {
        let q = |addr: usize, off: usize| -> usize {
            if addr == TITLE_OWNER_SCAN_START_ADDRESS {
                TITLE_OWNER_SCAN_START_ADDRESS
            } else {
                unsafe { safe_read_usize(addr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            }
        };
        let base = {
            let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
            if own != TITLE_OWNER_SCAN_START_ADDRESS {
                own
            } else {
                game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            }
        };
        let avt = q(a, CAP_MENU_INSERT_VTABLE_OFFSET);
        let bvt = q(b, CAP_MENU_INSERT_VTABLE_OFFSET);
        let rvt = q(ret, CAP_MENU_INSERT_VTABLE_OFFSET);
        let arva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            avt.wrapping_sub(base)
        } else {
            avt
        };
        let brva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            bvt.wrapping_sub(base)
        } else {
            bvt
        };
        let rrva = if base != TITLE_OWNER_SCAN_START_ADDRESS {
            rvt.wrapping_sub(base)
        } else {
            rvt
        };
        append_continue_trace(format_args!(
            "CAP menu_insert #{} rcx=0x{a:x} vt=0x{avt:x}(rva 0x{arva:x}) a8=0x{:x} a10=0x{:x} a18=0x{:x} a38=0x{:x} a50=0x{:x} rdx=0x{b:x} vt=0x{bvt:x}(rva 0x{brva:x}) b8=0x{:x} b10=0x{:x} b18=0x{:x} b38=0x{:x} r8=0x{c:x} r9=0x{d:x} ret=0x{ret:x} ret_vt=0x{rvt:x}(rva 0x{rrva:x}) ret8=0x{:x} ret10=0x{:x} ret18=0x{:x} {}",
            n,
            q(a, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_18_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_38_OFFSET),
            q(a, CAP_MENU_INSERT_QWORD_50_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_18_OFFSET),
            q(b, CAP_MENU_INSERT_QWORD_38_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_8_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_10_OFFSET),
            q(ret, CAP_MENU_INSERT_QWORD_18_OFFSET),
            trace_callers_summary()
        ));
    }
}

/// SetState 0x140b0d960(this, state): the title state machine setter. Logging every call
/// reveals the press-any-key advance + Continue's SetState(5) sequence.
pub(crate) unsafe extern "system" fn cap_setstate_hook(
    this: usize,
    state: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP setstate this=0x{this:x} state={} {} {}",
        state as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_SETSTATE_ORIG, this, state, c, d) }
}

/// Continue confirm 0x140b0e180(this): reads GameMan+0xc30 into owner+0xbc then SetState(5).
pub(crate) unsafe extern "system" fn cap_continue_confirm_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let owner = if this != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe {
            *((this + OWN_STEPPER_SHIM_OWNER_IDX * core::mem::size_of::<usize>()) as *const usize)
        }
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    append_continue_trace(format_args!(
        "CAP continue_confirm this=0x{this:x} owner=0x{owner:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    OWN_STEPPER_CONFIRMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    unsafe { call_cap_original(&CAP_CONTINUE_CONFIRM_ORIG, this, b, c, d) }
}

/// Load activate 0x1409a4670 = CS::ProfileLoadDialog vtable slot 20 (this = the dialog).
pub(crate) unsafe extern "system" fn cap_load_activate_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    append_continue_trace(format_args!(
        "CAP load_activate(slot20) dialog_this=0x{this:x} a1=0x{b:x} a2=0x{c:x} a3=0x{d:x} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_LOAD_ACTIVATE_ORIG, this, b, c, d) }
}

/// Load activate variant 0x1409ac760 (global-slot path).
pub(crate) unsafe extern "system" fn cap_load_activate2_hook(
    this: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const ARG_Q0_OFFSET: usize = 0x0;
    const ARG_Q8_OFFSET: usize = 0x8;
    const ARG_Q10_OFFSET: usize = 0x10;
    const ARG_Q18_OFFSET: usize = 0x18;
    let q = |ptr: usize, off: usize| -> usize {
        if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        }
    };
    append_continue_trace(format_args!(
        "CAP load_activate2 this=0x{this:x}[0x{:x},0x{:x},0x{:x},0x{:x}] a1=0x{b:x}[0x{:x},0x{:x}] a2=0x{c:x}[0x{:x},0x{:x},0x{:x},0x{:x}] a3=0x{d:x}[0x{:x},0x{:x}] {} {}",
        q(this, ARG_Q0_OFFSET),
        q(this, ARG_Q8_OFFSET),
        q(this, ARG_Q10_OFFSET),
        q(this, ARG_Q18_OFFSET),
        q(b, ARG_Q0_OFFSET),
        q(b, ARG_Q8_OFFSET),
        q(c, ARG_Q0_OFFSET),
        q(c, ARG_Q8_OFFSET),
        q(c, ARG_Q10_OFFSET),
        q(c, ARG_Q18_OFFSET),
        q(d, ARG_Q0_OFFSET),
        q(d, ARG_Q8_OFFSET),
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    unsafe { call_cap_original(&CAP_LOAD_ACTIVATE2_ORIG, this, b, c, d) }
}

/// Enter-Load-Game builder 0x140826510(owner, rdx, r8d=slot, r9) -> selector step.
pub(crate) unsafe extern "system" fn cap_builder_hook(
    owner: usize,
    rdx: usize,
    slot: usize,
    r9: usize,
) -> usize {
    let slot_i32 = slot as i32;
    let expected_slot = OWN_STEPPER_EXPECTED_SLOT.load(Ordering::SeqCst);
    let effective_slot = slot;
    append_continue_trace(format_args!(
        "CAP builder owner=0x{owner:x} slot={} effective_slot={} rdx=0x{rdx:x} r9=0x{r9:x} {} {}",
        slot_i32,
        effective_slot as i32,
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_BUILDER_ORIG, owner, rdx, effective_slot, r9) };
    if (live_dialog_enabled() || product_autoload_enabled())
        && ret != TITLE_OWNER_SCAN_START_ADDRESS
    {
        #[repr(C)]
        struct SelectorBuilderOwnerLayout {
            unknown_000: [u8; 0xf8],
            selector_ctx: usize,
        }
        const SELECTOR_CTX_OFFSET_F8: usize =
            core::mem::offset_of!(SelectorBuilderOwnerLayout, selector_ctx);
        const SELECTOR_STEP_VTABLE_RVA: usize = ProfileLoadMenuRva::SelectorStepVtable as usize;
        let step = unsafe { safe_read_usize(ret) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let step_vt = if step != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(step) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let ctx = ret + SELECTOR_CTX_OFFSET_F8;
        if game_module_base()
            .ok()
            .is_some_and(|base| step_vt == base + SELECTOR_STEP_VTABLE_RVA)
        {
            OWN_STEPPER_SELECTOR_STEP.store(step, Ordering::SeqCst);
            OWN_STEPPER_SELECTOR_CTX.store(ctx, Ordering::SeqCst);
        }
        append_autoload_debug(format_args!(
            "own_stepper: builder ret(owner)=0x{ret:x} step=[owner]=0x{step:x} step_vt=0x{step_vt:x} ctx(owner+0xf8)=0x{ctx:x} slot={} effective_slot={} for native selector self-pump",
            slot_i32, effective_slot as i32
        ));
    }
    ret
}

/// Selector-owner step tick 0x140826d50(step, ctx, result). Rate-limited (it ticks every
/// frame). Logs the step this, its +0x68 install flag, and the slot at ctx[0].
pub(crate) unsafe extern "system" fn cap_selector_tick_hook(
    step: usize,
    ctx: usize,
    result: usize,
    d: usize,
) -> usize {
    let n = CAP_SELECTOR_TICK_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    if n < CAP_SELECTOR_TICK_LOG_FIRST
        || n % CAP_SELECTOR_TICK_LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let installed = if step != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *((step + SELECTOR_STEP_INSTALL_FLAG_68_OFFSET) as *const u8) as i32 }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        let ctx_slot = if ctx != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { *(ctx as *const i32) }
        } else {
            TITLE_STATE_OWNER_GONE
        };
        const SELECTOR_STEP_Q10_OFFSET: usize =
            core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q18_OFFSET: usize =
            SELECTOR_STEP_Q10_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q20_OFFSET: usize =
            SELECTOR_STEP_Q18_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q28_OFFSET: usize =
            SELECTOR_STEP_Q20_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q30_OFFSET: usize =
            SELECTOR_STEP_Q28_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q38_OFFSET: usize =
            SELECTOR_STEP_Q30_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q50_OFFSET: usize = SELECTOR_STEP_Q38_OFFSET
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q58_OFFSET: usize =
            SELECTOR_STEP_Q50_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_Q60_OFFSET: usize =
            SELECTOR_STEP_Q58_OFFSET + core::mem::size_of::<usize>();
        const SELECTOR_STEP_TASK_OFFSET: usize = SELECTOR_STEP_Q60_OFFSET
            + core::mem::size_of::<usize>()
            + core::mem::size_of::<usize>();
        let step_q = |off: usize| -> usize {
            if step != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(step + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let step_task = step_q(SELECTOR_STEP_TASK_OFFSET);
        let step_task_vt = if step_task != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(step_task) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        const PTR_Q0_OFFSET: usize = 0x0;
        const PTR_Q8_OFFSET: usize = 0x8;
        const PTR_Q10_OFFSET: usize = 0x10;
        const PTR_Q18_OFFSET: usize = 0x18;
        let ptr_q = |ptr: usize, off: usize| -> usize {
            if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let step_q10 = step_q(SELECTOR_STEP_Q10_OFFSET);
        let step_q18 = step_q(SELECTOR_STEP_Q18_OFFSET);
        let step_q20 = step_q(SELECTOR_STEP_Q20_OFFSET);
        let step_q28 = step_q(SELECTOR_STEP_Q28_OFFSET);
        let step_q30 = step_q(SELECTOR_STEP_Q30_OFFSET);
        let step_q38 = step_q(SELECTOR_STEP_Q38_OFFSET);
        let step_q50 = step_q(SELECTOR_STEP_Q50_OFFSET);
        let step_q58 = step_q(SELECTOR_STEP_Q58_OFFSET);
        let step_q60 = step_q(SELECTOR_STEP_Q60_OFFSET);
        append_continue_trace(format_args!(
            "CAP selector_tick #{n} step=0x{step:x} ctx=0x{ctx:x} installed={installed} ctx_slot={ctx_slot} task=0x{step_task:x} task_vt=0x{step_task_vt:x} step_q=[0x{step_q10:x},0x{step_q18:x},0x{step_q20:x},0x{step_q28:x},0x{step_q30:x},0x{step_q38:x},0x{step_q50:x},0x{step_q58:x},0x{step_q60:x}] q50_obj=[0x{:x},0x{:x},0x{:x},0x{:x}] q60_obj=[0x{:x},0x{:x},0x{:x},0x{:x}] {}",
            ptr_q(step_q50, PTR_Q0_OFFSET),
            ptr_q(step_q50, PTR_Q8_OFFSET),
            ptr_q(step_q50, PTR_Q10_OFFSET),
            ptr_q(step_q50, PTR_Q18_OFFSET),
            ptr_q(step_q60, PTR_Q0_OFFSET),
            ptr_q(step_q60, PTR_Q8_OFFSET),
            ptr_q(step_q60, PTR_Q10_OFFSET),
            ptr_q(step_q60, PTR_Q18_OFFSET),
            b80_mount_trace_summary()
        ));
    }
    unsafe { call_cap_original(&CAP_SELECTOR_TICK_ORIG, step, ctx, result, d) }
}

/// ProfileLoadDialog factory 0x14081ead0(rcx=ctx, rdx): builds the Load-Game dialog when the
/// main-menu "Load Game" item is activated. The caller backtrace pins the navigation chain.
pub(crate) unsafe extern "system" fn cap_dialog_factory_hook(
    a: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Capture ALL four register args (rcx/rdx/r8/r9) AND a window of the rcx capture object so the
    // headless PATH-3-direct replay can reconstruct the exact factory invocation. The native
    // _Do_call thunk 0x140820c60 does `add rcx,8` before jmping here, so rcx (=a) is the lambda
    // capture state past the _Func_impl header; the ctor reads the owner from a field of it. Pure
    // reads + pass-through -> save-safe.
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const CAP_START: usize = 0;
    const CAP_WINDOW: usize = 7;
    const CAP_STEP: usize = 1;
    const PTR_SIZE: usize = 8;
    let mut capdump = String::new();
    // Dump [a-8 .. a+0x30] (the _Func_impl vtable at a-8, then capture fields).
    let mut i: usize = CAP_START;
    while i < CAP_WINDOW {
        let off = i * PTR_SIZE;
        let addr = a.wrapping_sub(PTR_SIZE).wrapping_add(off);
        let v = unsafe { safe_read_usize(addr) }.unwrap_or(NULL);
        capdump.push_str(&format!(" [rcx-8+0x{off:x}]=0x{v:x}"));
        i += CAP_STEP;
    }
    let rdx0 = unsafe { safe_read_usize(b) }.unwrap_or(NULL);
    let rdx8 = unsafe { safe_read_usize(b.wrapping_add(PTR_SIZE)) }.unwrap_or(NULL);
    append_continue_trace(format_args!(
        "CAP dialog_factory ENTER rcx=0x{a:x} rdx=0x{b:x} r8=0x{c:x} r9=0x{d:x} [rdx]=0x{rdx0:x} [rdx+8]=0x{rdx8:x}{capdump} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    let ret = unsafe { call_cap_original(&CAP_DIALOG_FACTORY_ORIG, a, b, c, d) };
    let ret_vt = if ret != NULL {
        unsafe { safe_read_usize(ret) }.unwrap_or(NULL)
    } else {
        NULL
    };
    append_continue_trace(format_args!(
        "CAP dialog_factory LEAVE dialog_this=0x{ret:x} dialog_vt=0x{ret_vt:x}"
    ));
    let base = game_module_base().unwrap_or(NULL);
    if product_autoload_enabled()
        && base != NULL
        && OWN_STEPPER_TITLE_FIRED.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && OWN_STEPPER_PHASE.load(Ordering::SeqCst) == OWN_STEPPER_PHASE_MENU
        && ret_vt == base + PROFILE_LOAD_DIALOG_VTABLE_RVA
    {
        OWN_STEPPER_DIALOG.store(ret, Ordering::SeqCst);
        own_stepper_enter_s2_phase(OWN_STEPPER_PHASE_S2_ACTIVATE);
        append_autoload_debug(format_args!(
            "product-core-autoload: native TitleTopDialog Load-Game factory returned ProfileLoadDialog=0x{ret:x} vt=0x{ret_vt:x}; captured by factory hook -> STAGE2 ACTIVATE"
        ));
    }
    ret
}

/// Menu deserialize 0x14082c240(this, ctx): the real mount (writes GameMan+0xc30 + char).
pub(crate) unsafe extern "system" fn cap_menu_deser_hook(
    this: usize,
    ctx: usize,
    c: usize,
    d: usize,
) -> usize {
    let ctx_slot = if ctx != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { *(ctx as *const i32) }
    } else {
        TITLE_STATE_OWNER_GONE
    };
    append_continue_trace(format_args!(
        "CAP menu_deser ENTER this=0x{this:x} ctx=0x{ctx:x} ctx_slot={ctx_slot} {} {}",
        trace_callers_summary(),
        b80_mount_trace_summary()
    ));
    {
        const Q0: usize = 0x0;
        const Q1: usize = 0x8;
        const Q2: usize = 0x10;
        const Q3: usize = 0x18;
        let q = |ptr: usize, off: usize| -> usize {
            if ptr != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            }
        };
        let io = game_module_base()
            .ok()
            .map(|base| unsafe { *((base + IODEV_GLOBAL_RVA) as *const usize) })
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let io18 = q(io, IODEV_REQHANDLE_18_OFFSET);
        let io20 = q(io, IODEV_REQHANDLE_20_OFFSET);
        append_continue_trace(format_args!(
            "CAP menu_deser RAW this=[0x{:x},0x{:x},0x{:x},0x{:x}] ctx=[0x{:x},0x{:x},0x{:x},0x{:x}] io18=0x{io18:x}[0x{:x},0x{:x},0x{:x},0x{:x}] io20=0x{io20:x}[0x{:x},0x{:x},0x{:x},0x{:x}]",
            q(this, Q0),
            q(this, Q1),
            q(this, Q2),
            q(this, Q3),
            q(ctx, Q0),
            q(ctx, Q1),
            q(ctx, Q2),
            q(ctx, Q3),
            q(io18, Q0),
            q(io18, Q1),
            q(io18, Q2),
            q(io18, Q3),
            q(io20, Q0),
            q(io20, Q1),
            q(io20, Q2),
            q(io20, Q3),
        ));
    }
    let ret = unsafe { call_cap_original(&CAP_MENU_DESER_ORIG, this, ctx, c, d) };
    append_continue_trace(format_args!(
        "CAP menu_deser LEAVE ret=0x{ret:x} {}",
        b80_mount_trace_summary()
    ));
    ret
}

/// Title native-ready predicate 0x140733150 hook. Static RE shows the original body is:
/// `state = this->vtable[0](this); return (state->flags_20 & 0x8f) != 0`. Re-implement that tiny
/// body exactly so the hook can record the returned state object/flags without making a second
/// native getter call or changing success semantics.
pub(crate) unsafe extern "system" fn title_native_ready_predicate_hook(this: usize) -> usize {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const STATE_FLAGS_20_OFFSET: usize = 0x20;
    const READY_MASK_8F: usize = 0x8f;
    type StateGetter = unsafe extern "system" fn(usize) -> usize;

    let caller_rva = trace_first_game_caller_rva();
    let vtable = unsafe { safe_read_usize(this) }.unwrap_or(NULL);
    let getter = if vtable != NULL {
        unsafe { safe_read_usize(vtable) }.unwrap_or(NULL)
    } else {
        NULL
    };
    let state = if getter != NULL {
        let f: StateGetter = unsafe { std::mem::transmute(getter) };
        unsafe { f(this) }
    } else {
        NULL
    };
    let flags = if state != NULL {
        unsafe { safe_read_usize(state + STATE_FLAGS_20_OFFSET) }.unwrap_or(0) & 0xff
    } else {
        0
    };
    let masked = flags & READY_MASK_8F;
    let ret = if masked != 0 { 1 } else { 0 };

    TITLE_NATIVE_READY_PREDICATE_HITS.fetch_add(1, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_THIS.store(this, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_VTABLE.store(vtable, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_GETTER.store(getter, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_OBJECT.store(state, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_FLAGS.store(flags, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_MASKED.store(masked, Ordering::SeqCst);
    TITLE_NATIVE_READY_PREDICATE_LAST_RET.store(ret, Ordering::SeqCst);

    ret
}

/// MenuWindowJob ctor 0x1407ac8c0 hook: observe constructed menu jobs and latch the semantic
/// Continue item only when both the Continue action and native accept predicate are installed.
/// This avoids poisoning MENU_CONTINUE_ITEM with the first updated title input leaf, whose
/// accept predicate is the constant-false 0x1407add70 diagnostic dead end.
pub(crate) unsafe extern "system" fn menu_window_job_ctor_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_CTOR_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_CTOR_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_CTOR_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let continue_candidate =
        vt == base + MENU_WINDOW_JOB_VTABLE_RVA && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
    if continue_candidate {
        record_continue_candidate(item, accept_predicate, base);
    }
    let semantic_continue_item =
        continue_candidate && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
    if semantic_continue_item {
        MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS.fetch_add(1, Ordering::SeqCst);
    }
    if semantic_continue_item
        && MENU_CONTINUE_ITEM
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                item,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    {
        append_continue_trace(format_args!(
            "MENU-WINDOW-CTOR captured semantic native Continue item=0x{item:x} out=0x{out_slot:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
            unsafe { menu_item_action_summary(item) },
            trace_callers_summary()
        ));
        append_autoload_debug(format_args!(
            "product-core-autoload: constructor captured semantic native Continue MenuWindowJob item=0x{item:x} vt=0x{vt:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x}"
        ));
    }
    ret
}

/// MenuWindowJob native-accept ctor variant 0x1407acb00 hook: observe constructed menu jobs from
/// the sibling constructor that static RE shows installs the native accept predicate 0x1407ad810.
/// This is passive except for the same semantic pointer latch used by the existing 0x1407ac8c0
/// constructor hook: if the item is a Continue row with native accept, record its pointer so the
/// product path can later submit through native semantics.
pub(crate) unsafe extern "system" fn menu_window_job_native_ctor_b_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let caller_rva = trace_first_game_caller_rva();
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_NATIVE_CTOR_B_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_NATIVE_CTOR_B_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let semantic_continue_item = vt == base + MENU_WINDOW_JOB_VTABLE_RVA
        && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA
        && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
    if semantic_continue_item {
        MENU_WINDOW_JOB_NATIVE_CTOR_B_CONTINUE_HITS.fetch_add(1, Ordering::SeqCst);
        record_continue_candidate(item, accept_predicate, base);
        if MENU_CONTINUE_ITEM
            .compare_exchange(
                TITLE_OWNER_SCAN_START_ADDRESS,
                item,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            append_continue_trace(format_args!(
                "MENU-WINDOW-NATIVE-CTOR-B captured semantic native Continue item=0x{item:x} caller_rva=0x{caller_rva:x} out=0x{out_slot:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
            append_autoload_debug(format_args!(
                "product-core-autoload: native ctor B captured semantic native Continue MenuWindowJob item=0x{item:x} caller_rva=0x{caller_rva:x} accept_predicate=0x{accept_predicate:x}"
            ));
        }
    }
    ret
}

/// MenuWindowJob disabled/idle ctor 0x1407acf80 hook: observe constructed menu jobs whose accept
/// functor is the constant-false 0x1407add70 variant. Static RE of the constructor shows it builds
/// the same MenuWindowJob vtable but installs the idle predicate into item+0xf0/+0xf8; this hook
/// attributes Continue-looking candidates to that disabled native path without promoting or
/// submitting them.
pub(crate) unsafe extern "system" fn menu_window_job_idle_ctor_hook(
    out_slot: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    let caller_rva = trace_first_game_caller_rva();
    let ret = unsafe { call_cap_original(&MENU_WINDOW_JOB_IDLE_CTOR_ORIG, out_slot, b, c, d) };
    if !product_autoload_enabled() || out_slot == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if base == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    const DOCALL_VTABLE_SLOT_10: usize = 0x10;
    const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
    let item = unsafe { safe_read_usize(out_slot) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    if item == TITLE_OWNER_SCAN_START_ADDRESS {
        return ret;
    }
    let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let accept_predicate = unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    MENU_WINDOW_JOB_IDLE_CTOR_HITS.fetch_add(1, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_ITEM.store(item, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_VT.store(vt, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_DOCALL.store(do_call, Ordering::SeqCst);
    MENU_WINDOW_JOB_IDLE_CTOR_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
    let continue_candidate =
        vt == base + MENU_WINDOW_JOB_VTABLE_RVA && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
    if continue_candidate {
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS.fetch_add(1, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.store(item, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.store(out_slot, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_DOCALL.store(do_call, Ordering::SeqCst);
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
        record_continue_candidate(item, accept_predicate, base);
        append_continue_trace(format_args!(
            "MENU-WINDOW-IDLE-CTOR observed Continue-looking disabled item=0x{item:x} caller_rva=0x{caller_rva:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
            unsafe { menu_item_action_summary(item) },
            trace_callers_summary()
        ));
    }
    ret
}

/// MenuWindowJob::Update 0x1407ad1c0 hook: the native menu pump calls this with rcx = a
/// menu-item each tick. We let the game walk its own (CSMenu) tree and CAPTURE the item
/// whose +0xa8 action functor's _Do_call chain resolves to dialog_factory 0x14081ead0 (=
/// the Load-Game item) into MENU_LOAD_GAME_ITEM, so the own-stepper can drive it
/// zero-input without guessing the container layout. Pure observe + pass-through (no
/// behaviour change). Logs the first distinct items to map the live title menu.
pub(crate) unsafe extern "system" fn cap_menu_item_update_hook(
    item: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    // Module base independent of the own-stepper (so this hook also works during a
    // user-driven trace with the own-stepper off): own-stepper base if set, else resolve it.
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if product_autoload_enabled()
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
    {
        const DOCALL_VTABLE_SLOT_10: usize = 0x10;
        const MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET: usize = 0xf8;
        const MENU_ITEM_ACCEPT_NATIVE_RVA: usize = 0x007ad810;
        let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let functor = unsafe { safe_read_usize(item + MENU_ITEM_FUNCTOR_A8_OFFSET) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let functor_vt = if functor != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(functor) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let do_call = if functor_vt != TITLE_OWNER_SCAN_START_ADDRESS {
            unsafe { safe_read_usize(functor_vt + DOCALL_VTABLE_SLOT_10) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        } else {
            TITLE_OWNER_SCAN_START_ADDRESS
        };
        let accept_predicate =
            unsafe { safe_read_usize(item + MENU_ITEM_ACCEPT_PREDICATE_F8_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        MENU_ITEM_UPDATE_HITS.fetch_add(1, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_ITEM.store(item, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_VT.store(vt, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_FUNCTOR.store(functor, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_DOCALL.store(do_call, Ordering::SeqCst);
        MENU_ITEM_UPDATE_LAST_ACCEPT.store(accept_predicate, Ordering::SeqCst);
        let continue_candidate = vt == base + MENU_WINDOW_JOB_VTABLE_RVA
            && do_call == base + MENU_TITLE_CONTINUE_DOCALL_RVA;
        if continue_candidate {
            record_continue_candidate(item, accept_predicate, base);
        }
        let semantic_continue_item =
            continue_candidate && accept_predicate == base + MENU_ITEM_ACCEPT_NATIVE_RVA;
        if semantic_continue_item {
            MENU_ITEM_UPDATE_SEMANTIC_HITS.fetch_add(1, Ordering::SeqCst);
        }
        if semantic_continue_item
            && MENU_CONTINUE_ITEM
                .compare_exchange(
                    TITLE_OWNER_SCAN_START_ADDRESS,
                    item,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
        {
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured semantic native Continue item=0x{item:x} vt=0x{vt:x} functor=0x{functor:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
            append_autoload_debug(format_args!(
                "product-core-autoload: captured semantic native Continue MenuWindowJob item=0x{item:x} vt=0x{vt:x} docall=0x{do_call:x} accept_predicate=0x{accept_predicate:x}"
            ));
        }
    }
    if product_autoload_enabled()
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && item == MENU_CONTINUE_ITEM.load(Ordering::SeqCst)
    {
        let n =
            MENU_CONTINUE_ITEM_FIELD_LOG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        const FIELD_LOG_0: usize = 0;
        const FIELD_LOG_8: usize = 8;
        const FIELD_LOG_30: usize = 30;
        const FIELD_LOG_60: usize = 60;
        const FIELD_LOG_120: usize = 120;
        if n == FIELD_LOG_0
            || n == FIELD_LOG_8
            || n == FIELD_LOG_30
            || n == FIELD_LOG_60
            || n == FIELD_LOG_120
        {
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE Continue candidate fields tick_count={n} item=0x{item:x} item_fields{{{}}} {}",
                unsafe { menu_item_action_summary(item) },
                trace_callers_summary()
            ));
        }
    }
    // While the deterministic input probe is active, count GENUINE d180 leaf-Update ticks (this
    // leaf fn 0x1407ad1c0 actually running for the Load-Game item) even after MENU_LOAD_GAME_ITEM
    // is already latched -- so the probe can tell "d180 leaf ticked" from "static walk found it".
    if INPUT_PROBE_ACTIVE.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
        && item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) != TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        if unsafe { functor_chain_hits_factory(item, base, &mut chain) } {
            MENU_D180_LEAF_TICKED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        }
    }
    if item != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let mut chain = String::new();
        let is_load_game = unsafe { functor_chain_hits_factory(item, base, &mut chain) };
        if is_load_game {
            MENU_LOAD_GAME_ITEM.store(item, Ordering::SeqCst);
            MENU_D180_LEAF_TICKED.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            append_continue_trace(format_args!(
                "MENU-ITEM-UPDATE captured LOAD-GAME item=0x{item:x} {chain} {}",
                trace_callers_summary()
            ));
        } else if MENU_ITEM_UPDATE_LAST.swap(item, Ordering::SeqCst) != item {
            // New distinct item ticked: log it once. CAPPED -- with a few items rotating
            // each frame this otherwise floods the size-capped trace and rolls the early
            // SEQ-ITER-CHILD enumeration off. The capture (MENU_LOAD_GAME_ITEM) is unaffected.
            let n =
                MENU_ITEM_UPDATE_CAPTURE_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
            if n < MENU_ITEM_UPDATE_LOG_MAX {
                let vt = unsafe { safe_read_usize(item) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if product_autoload_enabled() {
                    append_continue_trace(format_args!(
                        "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} item_fields{{{}}} {chain} load_game=false {}",
                        unsafe { menu_item_action_summary(item) },
                        trace_callers_summary()
                    ));
                } else {
                    append_continue_trace(format_args!(
                        "MENU-ITEM-UPDATE #{n} item=0x{item:x} vt=0x{vt:x} {chain} load_game=false {}",
                        trace_callers_summary()
                    ));
                }
            }
        }
    }
    unsafe { call_cap_original(&MENU_ITEM_UPDATE_ORIG, item, b, c, d) }
}

/// FD4 Sequence::Update / child-iterator 0x1407aa1f0 hook. The opened main-menu registers the
/// Load-Game leaf d180 but it does NOT tick (only the focused entry ticks the leaf Update, so
/// `cap_menu_item_update_hook` misses d180). This iterator runs on every Sequence node; we
/// walk its inline child array ([seq+0x18 + i*8], count [seq+0x60]) and classify each child by
/// the action-functor `_Do_call` chain (`functor_chain_hits_factory` -> dialog_factory
/// 0x14081ead0). The unique hit is d180 / Load-Game -- captured regardless of focus, then read
/// by own_stepper idx10 (MENU_LOAD_GAME_ITEM) for the Stage-2 functor invoke. Early-outs once
/// found (the iterator is hot); fault-tolerant reads never AV; pure read, NO writes/calls into
/// the game beyond the original.
pub(crate) unsafe extern "system" fn cap_sequence_iter_hook(
    seq: usize,
    b: usize,
    c: usize,
    d: usize,
) -> usize {
    const PTR_STRIDE: usize = core::mem::size_of::<usize>();
    const WALK_START: usize = 0;
    const WALK_STEP: usize = 1;
    let base = {
        let own = OWN_STEPPER_BASE.load(Ordering::SeqCst);
        if own != TITLE_OWNER_SCAN_START_ADDRESS {
            own
        } else {
            game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
        }
    };
    if seq != TITLE_OWNER_SCAN_START_ADDRESS
        && base != TITLE_OWNER_SCAN_START_ADDRESS
        && MENU_LOAD_GAME_ITEM.load(Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
    {
        let count = unsafe { safe_read_usize(seq + SEQUENCE_COUNT_60_OFFSET) }
            .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        // Unconditional structural dump (first N calls): what does the iterator walk?
        let ndbg = SEQ_ITER_DEBUG_COUNT.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
        if ndbg < SEQ_ITER_DEBUG_MAX {
            let seq_vt = unsafe { safe_read_usize(seq) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let child0 = unsafe { safe_read_usize(seq + SEQUENCE_CHILDREN_BASE_18_OFFSET) }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
            let child0_vt = if child0 != TITLE_OWNER_SCAN_START_ADDRESS {
                unsafe { safe_read_usize(child0) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
            } else {
                TITLE_OWNER_SCAN_START_ADDRESS
            };
            append_continue_trace(format_args!(
                "SEQ-ITER-DBG #{ndbg} seq=0x{seq:x} seqvt=0x{seq_vt:x} count={count} child0=0x{child0:x} child0vt=0x{child0_vt:x}"
            ));
        }
        if (SEQUENCE_CHILD_COUNT_MIN..=SEQUENCE_CHILD_COUNT_MAX).contains(&count) {
            let mut i = WALK_START;
            while i < count {
                let child = unsafe {
                    safe_read_usize(seq + SEQUENCE_CHILDREN_BASE_18_OFFSET + i * PTR_STRIDE)
                }
                .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                if child != TITLE_OWNER_SCAN_START_ADDRESS {
                    let mut chain = String::new();
                    let child_vt =
                        unsafe { safe_read_usize(child) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
                    if unsafe { functor_chain_hits_factory(child, base, &mut chain) } {
                        MENU_LOAD_GAME_ITEM.store(child, Ordering::SeqCst);
                        append_continue_trace(format_args!(
                            "SEQ-ITER captured LOAD-GAME child=0x{child:x} vt=0x{child_vt:x} seq=0x{seq:x} count={count} idx={i} {chain}"
                        ));
                        break;
                    }
                    // A MenuWindowJob child means the main menu actually opened (its entries
                    // are registered into a Sequence the iterator walks) -- signal the STAGE1d
                    // retry loop to stop. The title views tick via a different pump, so this
                    // fires ONLY on the real main-menu entries.
                    if child_vt == base + MENU_WINDOW_JOB_VTABLE_RVA {
                        MENU_ENTRIES_SEEN.store(MENU_ENTRIES_SEEN_YES, Ordering::SeqCst);
                    }
                    // Diagnostic: surface distinct MenuWindowJob children (the registered menu
                    // entries, ticking or not) with their docall chain so one run reveals the
                    // opened-menu structure (which entry is Load-Game). Capped to avoid flooding.
                    if child_vt == base + MENU_WINDOW_JOB_VTABLE_RVA
                        && SEQ_ITER_CHILD_LAST.swap(child, Ordering::SeqCst) != child
                    {
                        let nlog = SEQ_ITER_CHILD_LOG_COUNT
                            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
                        if nlog < SEQ_ITER_CHILD_LOG_MAX {
                            append_continue_trace(format_args!(
                                "SEQ-ITER-CHILD #{nlog} child=0x{child:x} seq=0x{seq:x} count={count} idx={i} {chain}"
                            ));
                        }
                    }
                }
                i += WALK_STEP;
            }
        }
    }
    unsafe { call_cap_original(&SEQUENCE_ITER_ORIG, seq, b, c, d) }
}

fn format_optional_usize_hex(value: usize) -> String {
    if value == TITLE_OWNER_SCAN_START_ADDRESS {
        "null".to_owned()
    } else {
        format!("0x{value:x}")
    }
}

unsafe fn result_built_flag(result: usize) -> usize {
    const RESULT_BUILT_3B0_OFFSET: usize = 0x3b0;
    const U8_MASK: usize = 0xff;
    if result == TITLE_OWNER_SCAN_START_ADDRESS {
        TITLE_OWNER_SCAN_START_ADDRESS
    } else {
        unsafe { safe_read_usize(result + RESULT_BUILT_3B0_OFFSET) }
            .map_or(TITLE_OWNER_SCAN_START_ADDRESS, |value| value & U8_MASK)
    }
}

unsafe fn native_result_event_words(event: usize) -> (usize, usize) {
    const EVENT_WORD0_OFFSET: usize = 0;
    const EVENT_WORD1_OFFSET: usize = core::mem::size_of::<usize>();
    if event == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
        );
    }
    let word0 = unsafe { safe_read_usize(event + EVENT_WORD0_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    let word1 = unsafe { safe_read_usize(event + EVENT_WORD1_OFFSET) }
        .unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
    (word0, word1)
}

fn fd4_event_code_arg(raw_qword0: usize) -> (usize, usize) {
    const U32_MASK: usize = 0xffff_ffff;
    if raw_qword0 == TITLE_OWNER_SCAN_START_ADDRESS {
        return (
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
        );
    }
    (raw_qword0 & U32_MASK, (raw_qword0 >> 32) & U32_MASK)
}

pub(crate) unsafe extern "system" fn native_submit_hook(result: usize) {
    const TRACE_FIRST: usize = 16;
    let seq =
        NATIVE_SUBMIT_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst) + OWN_STEPPER_CALL_INC;
    NATIVE_SUBMIT_LAST_RESULT.store(result, Ordering::SeqCst);
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "native_submit_7ac890 seq={seq} phase=ENTER result=0x{result:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void1_original(&NATIVE_SUBMIT_ORIG, result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "native_submit_7ac890 seq={seq} phase=LEAVE result=0x{result:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn result_event_handler_hook(result: usize, event: usize) {
    const TRACE_FIRST: usize = 16;
    let seq = RESULT_EVENT_HANDLER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    RESULT_EVENT_LAST_RESULT.store(result, Ordering::SeqCst);
    RESULT_EVENT_LAST_EVENT.store(event, Ordering::SeqCst);
    let (event_raw_qword0, _) = unsafe { native_result_event_words(event) };
    let (fd4_code, fd4_arg) = fd4_event_code_arg(event_raw_qword0);
    RESULT_EVENT_LAST_RAW_QWORD0.store(event_raw_qword0, Ordering::SeqCst);
    RESULT_EVENT_LAST_FD4_CODE.store(fd4_code, Ordering::SeqCst);
    RESULT_EVENT_LAST_FD4_ARG.store(fd4_arg, Ordering::SeqCst);
    let built_before = unsafe { result_built_flag(result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_event_handler_746e80 seq={seq} phase=ENTER result=0x{result:x} event=0x{event:x} event_raw_qword0={} fd4_code={} fd4_arg={} built_before={} {}",
            format_optional_usize_hex(event_raw_qword0),
            format_optional_usize_hex(fd4_code),
            format_optional_usize_hex(fd4_arg),
            format_optional_usize_hex(built_before),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void2_original(&RESULT_EVENT_HANDLER_ORIG, result, event) };
    let built_after = unsafe { result_built_flag(result) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_event_handler_746e80 seq={seq} phase=LEAVE result=0x{result:x} event=0x{event:x} built_after={} {}",
            format_optional_usize_hex(built_after),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn result_event_wrapper_builder_hook(
    rcx: usize,
    rdx: usize,
    r8: usize,
) -> usize {
    const TRACE_FIRST: usize = 16;
    const RESULT_ACTION_BUILDER_TRACE_SIZE: usize = 0x360;
    let from_result_action_builder = callstack_contains_game_rva(
        RESULT_ACTION_BUILDER_RVA as usize,
        RESULT_ACTION_BUILDER_RVA as usize + RESULT_ACTION_BUILDER_TRACE_SIZE,
    );
    let result = unsafe { call_wrapper_builder_original(rcx, rdx, r8) }.unwrap_or(rcx);
    if from_result_action_builder {
        let seq = RESULT_ACTION_WRAPPER_BUILDER_HITS
            .fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            + OWN_STEPPER_CALL_INC;
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result) }
        };
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX.store(rcx, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX.store(rdx, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_R8.store(r8, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RET.store(result, Ordering::SeqCst);
        RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if seq <= TRACE_FIRST {
            append_continue_trace(format_args!(
                "result_event_wrapper_builder_744a60 seq={seq} rcx=0x{rcx:x} rdx=0x{rdx:x} r8=0x{r8:x} ret=0x{result:x} ret_update_rva={} -- passive wrapper-builder call from result action builder",
                format_optional_usize_hex(ret_update_rva)
            ));
        }
    }
    result
}

pub(crate) unsafe extern "system" fn result_action_builder_hook(result: usize, event: usize) {
    const TRACE_FIRST: usize = 16;
    let seq = RESULT_ACTION_BUILDER_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
        + OWN_STEPPER_CALL_INC;
    RESULT_ACTION_LAST_RESULT.store(result, Ordering::SeqCst);
    RESULT_ACTION_LAST_EVENT.store(event, Ordering::SeqCst);
    let (event_word0, event_word1) = unsafe { native_result_event_words(event) };
    RESULT_ACTION_LAST_WORD0.store(event_word0, Ordering::SeqCst);
    RESULT_ACTION_LAST_WORD1.store(event_word1, Ordering::SeqCst);
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_action_builder_746a00 seq={seq} phase=ENTER result=0x{result:x} event=0x{event:x} event_word0={} event_word1={} built={} {}",
            format_optional_usize_hex(event_word0),
            format_optional_usize_hex(event_word1),
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            trace_callers_summary()
        ));
    }
    let _ = unsafe { call_result_void2_original(&RESULT_ACTION_BUILDER_ORIG, result, event) };
    if seq <= TRACE_FIRST {
        append_continue_trace(format_args!(
            "result_action_builder_746a00 seq={seq} phase=LEAVE result=0x{result:x} event=0x{event:x} built={} {}",
            format_optional_usize_hex(unsafe { result_built_flag(result) }),
            game_man_trace_summary()
        ));
    }
}

pub(crate) unsafe extern "system" fn menu_task_update_wrapper_hook(
    this: *mut c_void,
) -> *mut c_void {
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "ENTER",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            this,
        )
    };
    let result =
        unsafe { call_wrapper_original(&MENU_TASK_UPDATE_WRAPPER_ORIG, this) }.unwrap_or(this);
    unsafe {
        append_menu_semaphore_trace(
            "menu_task_update_wrapper",
            "LEAVE",
            TRACE_MENU_TASK_UPDATE_WRAPPER_RVA,
            TRACE_MENU_TASK_UPDATE_TABLE_RVA,
            result,
        )
    };
    result
}

unsafe fn text_section_bounds(base: usize) -> Option<(usize, usize)> {
    let e_lfanew = unsafe { safe_read_usize(base + PE_DOS_LFANEW_OFFSET) }? & PE_U32_MASK;
    let nt = base + e_lfanew;
    let num_sections = unsafe { safe_read_usize(nt + PE_FILE_NUM_SECTIONS_OFFSET) }? & PE_U16_MASK;
    let size_opt = unsafe { safe_read_usize(nt + PE_FILE_SIZE_OPT_HEADER_OFFSET) }? & PE_U16_MASK;
    let sections = nt + PE_OPT_HEADER_OFFSET + size_opt;
    let mut index = PE_SECTION_SCAN_START;
    while index < num_sections {
        let header = sections + index * PE_SECTION_HEADER_SIZE;
        let name = unsafe { safe_read_usize(header) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if name.to_le_bytes().starts_with(PE_TEXT_SECTION_NAME) {
            let vsize = unsafe { safe_read_usize(header + PE_SECTION_VSIZE_OFFSET) }? & PE_U32_MASK;
            let vaddr = unsafe { safe_read_usize(header + PE_SECTION_VADDR_OFFSET) }? & PE_U32_MASK;
            return Some((base + vaddr, vsize));
        }
        index += OWN_STEPPER_CALL_INC;
    }
    None
}

unsafe fn update_target_in_text(base: usize, update: usize) -> bool {
    if update < base {
        return false;
    }
    let Some((text_start, text_len)) = (unsafe { text_section_bounds(base) }) else {
        return false;
    };
    update >= text_start && update < text_start.saturating_add(text_len)
}

unsafe fn raw_task_node_update_rva(base: usize, node: usize) -> usize {
    const TASK_NODE_UPDATE_VTABLE_SLOT: usize = 0x10;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let Some(vtable) = (unsafe { safe_read_usize(node) }) else {
        return null;
    };
    let Some(update) = (unsafe { safe_read_usize(vtable + TASK_NODE_UPDATE_VTABLE_SLOT) }) else {
        return null;
    };
    if unsafe { update_target_in_text(base, update) } {
        update - base
    } else {
        null
    }
}

unsafe fn task_node_update_rva(base: usize, node: usize) -> usize {
    let direct = unsafe { raw_task_node_update_rva(base, node) };
    if direct != TITLE_OWNER_SCAN_START_ADDRESS {
        return direct;
    }
    let Some(shared_pointee) = (unsafe { safe_read_usize(node) }) else {
        return TITLE_OWNER_SCAN_START_ADDRESS;
    };
    unsafe { raw_task_node_update_rva(base, shared_pointee) }
}

unsafe fn qword_window_summary(ptr: usize) -> String {
    const QWORDS: usize = 6;
    const START: usize = 0;
    const STEP: usize = 1;
    const STRIDE: usize = core::mem::size_of::<usize>();
    let mut out = String::new();
    let mut i = START;
    while i < QWORDS {
        let off = i * STRIDE;
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        i += STEP;
    }
    out
}

unsafe fn menu_item_action_summary(ptr: usize) -> String {
    const OFFSETS: [usize; 14] = [
        0x0, 0x8, 0x10, 0x40, 0x50, 0x68, 0xa8, 0xb0, 0xe8, 0xf0, 0xf8, 0x100, 0x130, 0x138,
    ];
    let mut out = String::new();
    for off in OFFSETS {
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        if value != TITLE_OWNER_SCAN_START_ADDRESS {
            let _ = core::fmt::write(
                &mut out,
                format_args!(" ->{{{}}}", unsafe { qword_window_summary(value) }),
            );
        }
    }
    out
}

unsafe fn task_node_raw_summary(ptr: usize) -> String {
    const QWORDS: usize = 8;
    const START: usize = 0;
    const STEP: usize = 1;
    const STRIDE: usize = core::mem::size_of::<usize>();
    let mut out = String::new();
    let mut first = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut second = TITLE_OWNER_SCAN_START_ADDRESS;
    let mut i = START;
    while i < QWORDS {
        let off = i * STRIDE;
        let value = unsafe { safe_read_usize(ptr + off) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        if i == START {
            first = value;
        } else if i == STEP {
            second = value;
        }
        let _ = core::fmt::write(&mut out, format_args!(" +0x{off:x}=0x{value:x}"));
        i += STEP;
    }
    if first != TITLE_OWNER_SCAN_START_ADDRESS {
        let _ = core::fmt::write(
            &mut out,
            format_args!(" | *q0{{{}}}", unsafe { qword_window_summary(first) }),
        );
    }
    if second != TITLE_OWNER_SCAN_START_ADDRESS {
        let _ = core::fmt::write(
            &mut out,
            format_args!(" | *q8{{{}}}", unsafe { qword_window_summary(second) }),
        );
    }
    out
}

unsafe fn capture_continue_task_node_candidate(base: usize, candidate: usize, label: &str) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if candidate == null {
        return;
    }
    let update_rva = unsafe { task_node_update_rva(base, candidate) };
    if update_rva != TRACE_MENU_CONTINUE_WRAPPER_RVA as usize {
        return;
    }
    if MENU_CONTINUE_TASK_NODE
        .compare_exchange(null, candidate, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        append_continue_trace(format_args!(
            "CAP continue_task_node {label}=0x{candidate:x} update_rva=0x{update_rva:x} -- captured native Continue menu task wrapper"
        ));
        append_autoload_debug(format_args!(
            "product-core-autoload: captured native Continue task node from {label}=0x{candidate:x} update_rva=0x{update_rva:x}"
        ));
    }
}

unsafe fn capture_continue_member_node_candidate(base: usize, candidate: usize, label: &str) {
    const MEMBER_DIALOG_10: usize = core::mem::size_of::<usize>() + core::mem::size_of::<usize>();
    const MEMBER_FN_18: usize = 0x18;
    const MEMBER_ADJ_20: usize = 0x20;
    const JMP_HOPS: usize = 6;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if candidate == null {
        return;
    }
    let node_vt = unsafe { safe_read_usize(candidate) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return;
    }
    let member_fn = unsafe { safe_read_usize(candidate + MEMBER_FN_18) }.unwrap_or(null);
    if member_fn == null {
        return;
    }
    let continue_wrapper = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    let mut target = member_fn;
    let mut hop = 0;
    while hop < JMP_HOPS && target != null {
        if target == continue_wrapper {
            let member_dialog =
                unsafe { safe_read_usize(candidate + MEMBER_DIALOG_10) }.unwrap_or(null);
            let member_adjust =
                unsafe { safe_read_usize(candidate + MEMBER_ADJ_20) }.unwrap_or(null);
            if MENU_CONTINUE_MEMBER_NODE
                .compare_exchange(null, candidate, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                append_continue_trace(format_args!(
                    "CAP continue_member_node {label}=0x{candidate:x} node_vt=0x{node_vt:x} member_dialog=0x{member_dialog:x} member_fn=0x{member_fn:x} member_adjust=0x{member_adjust:x} -- captured registered TitleTopDialog Continue MenuMemberFuncJob"
                ));
                append_autoload_debug(format_args!(
                    "product-core-autoload: captured registered TitleTopDialog Continue MenuMemberFuncJob from {label}=0x{candidate:x} member_fn=0x{member_fn:x}"
                ));
            }
            return;
        }
        match unsafe { decode_thunk_hop(target) } {
            Some(next) => target = next,
            None => break,
        }
        hop += 1;
    }
}

pub(crate) unsafe extern "system" fn task_enqueue_hook(
    arg0: *mut c_void,
    arg1: *mut c_void,
) -> *mut c_void {
    let caller_rva = trace_first_game_caller_rva();
    let trace_index = TASK_ENQUEUE_TRACE_COUNT
        .fetch_add(TASK_ENQUEUE_TRACE_INCREMENT, Ordering::SeqCst)
        + TASK_ENQUEUE_TRACE_INCREMENT;
    let should_trace = trace_index <= TASK_ENQUEUE_TRACE_LIMIT
        || SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
            > NO_SAFE_INPUT_CONFIRM_FRAMES;
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=ENTER hook_rva=0x{:x} list={arg0:p} node={arg1:p} node_{} raw{{{}}} confirm_active={} pulse={} {} {}",
            TRACE_TASK_ENQUEUE_RVA,
            unsafe { object_vtable_summary(arg1) },
            unsafe { task_node_raw_summary(arg1 as usize) },
            SAFE_INPUT_CONFIRM_FRAMES_REMAINING.load(Ordering::SeqCst)
                > NO_SAFE_INPUT_CONFIRM_FRAMES,
            SAFE_INPUT_CONFIRM_PULSE_SEQ.load(Ordering::SeqCst),
            trace_callers_summary(),
            game_man_trace_summary()
        ));
    }
    let result = unsafe { call_task_enqueue_original(arg0, arg1) }.unwrap_or(arg1);
    let arg0_pointee = if arg0 as usize != TITLE_OWNER_SCAN_START_ADDRESS {
        unsafe { safe_read_usize(arg0 as usize) }.unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS)
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let generic_hit = TASK_ENQUEUE_GENERIC_HITS.fetch_add(1, Ordering::SeqCst) + 1;
    TASK_ENQUEUE_GENERIC_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG0.store(arg0 as usize, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_ARG1.store(arg1 as usize, Ordering::SeqCst);
    TASK_ENQUEUE_GENERIC_LAST_RET.store(result as usize, Ordering::SeqCst);
    match generic_hit {
        1 => {
            TASK_ENQUEUE_GENERIC_SAMPLE0_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0.store(arg0 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_ARG1.store(arg1 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE0_RET.store(result as usize, Ordering::SeqCst);
        }
        2 => {
            TASK_ENQUEUE_GENERIC_SAMPLE1_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0.store(arg0 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG0_POINTEE.store(arg0_pointee, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_ARG1.store(arg1 as usize, Ordering::SeqCst);
            TASK_ENQUEUE_GENERIC_SAMPLE1_RET.store(result as usize, Ordering::SeqCst);
        }
        _ => {}
    }
    const MENU_CONTINUE_IDLE_INSERT_CALLER_RVA: usize = 0x0076432c;
    const MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA: usize = 0x007642b0;
    const MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA: usize = 0x007643c0;
    let idle_ctor_out_slot =
        MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_OUT_SLOT.load(Ordering::SeqCst);
    let idle_ctor_item = MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_ITEM.load(Ordering::SeqCst);
    let arg0_points_to_idle_item = arg0_pointee == idle_ctor_item;
    const TASK_ENQUEUE_IDLE_MATCH_CALLER_EXACT: usize = 1;
    const TASK_ENQUEUE_IDLE_MATCH_CALLER_RANGE: usize = 2;
    const TASK_ENQUEUE_IDLE_MATCH_ARG0_OUT_SLOT: usize = 3;
    const TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE: usize = 4;
    const TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM: usize = 5;
    let stack_contains_idle_caller = callstack_contains_game_rva(
        MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA,
        MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA,
    );
    let idle_match_kind = if caller_rva == MENU_CONTINUE_IDLE_INSERT_CALLER_RVA {
        TASK_ENQUEUE_IDLE_MATCH_CALLER_EXACT
    } else if stack_contains_idle_caller {
        TASK_ENQUEUE_IDLE_MATCH_CALLER_RANGE
    } else if idle_ctor_out_slot != TITLE_OWNER_SCAN_START_ADDRESS
        && arg0 as usize == idle_ctor_out_slot
    {
        TASK_ENQUEUE_IDLE_MATCH_ARG0_OUT_SLOT
    } else if idle_ctor_item != TITLE_OWNER_SCAN_START_ADDRESS && arg0_points_to_idle_item {
        TASK_ENQUEUE_IDLE_MATCH_ARG0_POINTEE
    } else if idle_ctor_item != TITLE_OWNER_SCAN_START_ADDRESS && arg1 as usize == idle_ctor_item {
        TASK_ENQUEUE_IDLE_MATCH_ARG1_ITEM
    } else {
        TITLE_OWNER_SCAN_START_ADDRESS
    };
    let idle_continue_insert_match = idle_match_kind != TITLE_OWNER_SCAN_START_ADDRESS;
    if idle_continue_insert_match {
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_MATCH_HITS.fetch_add(1, Ordering::SeqCst);
        TASK_ENQUEUE_GENERIC_IDLE_ITEM_LAST_MATCH_KIND.store(idle_match_kind, Ordering::SeqCst);
    }
    if idle_continue_insert_match {
        let hit = MENU_CONTINUE_IDLE_INSERT_HITS.fetch_add(1, Ordering::SeqCst) + 1;
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let arg1_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, arg1 as usize) }
        };
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result as usize) }
        };
        MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA.store(caller_rva, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG0.store(arg0 as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG1.store(arg1 as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_RET.store(result as usize, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_ARG1_UPDATE_RVA.store(arg1_update_rva, Ordering::SeqCst);
        MENU_CONTINUE_IDLE_INSERT_LAST_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if hit <= CAP_MENU_INSERT_LOG_FIRST as u64 {
            append_continue_trace(format_args!(
                "MENU-CONTINUE-IDLE-INSERT seq={hit} caller_rva=0x{caller_rva:x} arg0={arg0:p} arg1={arg1:p} arg1_update_rva={} ret={result:p} ret_update_rva={} -- passive disabled Continue insert edge via 0x{:x}",
                format_optional_usize_hex(arg1_update_rva),
                format_optional_usize_hex(ret_update_rva),
                TRACE_TASK_ENQUEUE_RVA
            ));
        }
    }
    const RESULT_ACTION_BUILDER_TRACE_SIZE: usize = 0x360;
    if callstack_contains_game_rva(
        RESULT_ACTION_BUILDER_RVA as usize,
        RESULT_ACTION_BUILDER_RVA as usize + RESULT_ACTION_BUILDER_TRACE_SIZE,
    ) {
        let hit = RESULT_ACTION_INSERT_HITS.fetch_add(OWN_STEPPER_CALL_INC, Ordering::SeqCst)
            + OWN_STEPPER_CALL_INC;
        RESULT_ACTION_LAST_INSERT_ARG0.store(arg0 as usize, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_ARG1.store(arg1 as usize, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_RET.store(result as usize, Ordering::SeqCst);
        let base = game_module_base().unwrap_or(TITLE_OWNER_SCAN_START_ADDRESS);
        let arg1_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, arg1 as usize) }
        };
        let ret_update_rva = if base == TITLE_OWNER_SCAN_START_ADDRESS {
            TITLE_OWNER_SCAN_START_ADDRESS
        } else {
            unsafe { task_node_update_rva(base, result as usize) }
        };
        RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA.store(arg1_update_rva, Ordering::SeqCst);
        RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA.store(ret_update_rva, Ordering::SeqCst);
        if hit <= CAP_MENU_INSERT_LOG_FIRST {
            append_continue_trace(format_args!(
                "result_action_builder_insert seq={hit} arg0={arg0:p} arg1={arg1:p} arg1_update_rva={} ret={result:p} ret_update_rva={} -- passive downstream action node insert via 0x{:x}",
                format_optional_usize_hex(arg1_update_rva),
                format_optional_usize_hex(ret_update_rva),
                TRACE_TASK_ENQUEUE_RVA
            ));
        }
    }
    if let Ok(base) = game_module_base() {
        unsafe { capture_continue_task_node_candidate(base, arg1 as usize, "arg1") };
        unsafe { capture_continue_task_node_candidate(base, result as usize, "ret") };
        unsafe { capture_continue_member_node_candidate(base, arg1 as usize, "arg1") };
        unsafe { capture_continue_member_node_candidate(base, result as usize, "ret") };
    }
    unsafe {
        log_menu_insert_details(
            arg0 as usize,
            arg1 as usize,
            TITLE_OWNER_SCAN_START_ADDRESS,
            TITLE_OWNER_SCAN_START_ADDRESS,
            result as usize,
        );
    }
    if should_trace {
        append_continue_trace(format_args!(
            "menu_task_enqueue seq={trace_index} phase=LEAVE ret={result:p} ret_{} raw{{{}}} {}",
            unsafe { object_vtable_summary(result) },
            unsafe { task_node_raw_summary(result as usize) },
            game_man_trace_summary()
        ));
    }
    result
}

pub(crate) unsafe extern "system" fn set_save_slot_hook(slot: i32) {
    append_continue_trace(format_args!(
        "ENTER set_save_slot slot={slot} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SET_SAVE_SLOT_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(i32) = unsafe { std::mem::transmute(original) };
        unsafe { original(slot) };
    }
    append_continue_trace(format_args!(
        "LEAVE set_save_slot {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn save_request_profile_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER save_request_profile enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_REQUEST_PROFILE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE save_request_profile {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn request_save_hook(enabled: u8) {
    append_continue_trace(format_args!(
        "ENTER request_save enabled={enabled} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = REQUEST_SAVE_ORIG.load(Ordering::SeqCst);
    if original != HOOK_ORIGINAL_UNSET {
        let original: unsafe extern "system" fn(u8) = unsafe { std::mem::transmute(original) };
        unsafe { original(enabled) };
    }
    append_continue_trace(format_args!(
        "LEAVE request_save {}",
        game_man_trace_summary()
    ));
}

pub(crate) unsafe extern "system" fn current_slot_load_hook(arg0: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER current_slot_load_67b570 arg0={arg0} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CURRENT_SLOT_LOAD_ORIG, arg0, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE current_slot_load_67b570 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn continue_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER continue_load_67b750 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&CONTINUE_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE continue_load_67b750 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn combined_load_hook(slot: i32, arg1: u8, arg2: u8) -> u8 {
    append_continue_trace(format_args!(
        "ENTER combined_load_67b940 slot={slot} arg1={arg1} arg2={arg2} {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let ret = unsafe { call_bool3_original(&COMBINED_LOAD_ORIG, slot, arg1, arg2) }
        .unwrap_or(HOOK_FALSE_RETURN);
    append_continue_trace(format_args!(
        "LEAVE combined_load_67b940 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn map_load_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER map_load_67bc10 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = MAP_LOAD_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    if ret != HOOK_FALSE_RETURN {
        TITLE_HANDOFF_COMPLETE.store(TITLE_HANDOFF_COMPLETE_VALUE, Ordering::SeqCst);
    }
    append_continue_trace(format_args!(
        "LEAVE map_load_67bc10 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

pub(crate) unsafe extern "system" fn save_load_state_init_hook() -> u8 {
    append_continue_trace(format_args!(
        "ENTER save_load_state_init_67b030 {} {}",
        trace_callers_summary(),
        game_man_trace_summary()
    ));
    let original = SAVE_LOAD_STATE_INIT_ORIG.load(Ordering::SeqCst);
    let ret = if original == HOOK_ORIGINAL_UNSET {
        HOOK_FALSE_RETURN
    } else {
        let original: unsafe extern "system" fn() -> u8 = unsafe { std::mem::transmute(original) };
        unsafe { original() }
    };
    append_continue_trace(format_args!(
        "LEAVE save_load_state_init_67b030 ret={ret} {}",
        game_man_trace_summary()
    ));
    ret
}

// ===========================================================================
// SAVE-SOURCE OVERRIDE (no-default-fallback, env-mandated)
// ===========================================================================
//
// USER HARD CONSTRAINT (save-override-no-default-fallback-mandatory-env-2026-06-23):
// while the DLL is loaded it MUST NOT assume / read the default user save directory
// (%APPDATA%/EldenRing/<SteamID64>/ER0000.sl2). There is NO escape hatch back to the
// default dir. The ONLY exemption is a pure telemetry/observe-only mode that loads
// nothing. In every other case the save source is MANDATORY and supplied via env
// `ER_EFFECTS_SAVE_FILE` (an absolute path to the save file the game should open);
// if it is unset/blank/not a readable real save the process ABORTS early at DLL init,
// before the game opens any save -- never a silent fallback.
//
// Mechanism: a scoped MinHook on the Win32 `CreateFileW` (and `CopyFileW`) chokepoint
// through which the game opens EVERY save artifact (verified RE: vanilla `.sl2`,
// Seamless `.co2`, `.bak`, all funnel `MicrosoftDiskFileOperator::OpenFile` ->
// `CreateFileW`; reads/writes reuse the returned HANDLE so redirecting the open covers
// both). The hook rewrites only the DIRECTORY portion of paths that match the save
// signature (a `\EldenRing\` segment + a save basename), keeping the game's chosen
// basename, so `.sl2`/`.co2`/`.bak` reroute together and vanilla + Seamless both work.
// Non-save opens pass through unchanged. Stable Win32 ABI; no fixed-offset code poke;
// mod-compatible (ERSC does not replace this open). See target/save-io-re-findings.md.

/// Minimum plausible size (bytes) of a real ER0000.sl2/.co2: the fixed-slot BND4 container
/// is ~28 MB even with empty slots, so anything under 1 MB is missing/truncated/garbage.
pub(crate) const SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES: u64 = 0x10_0000;

/// Telemetry/observe-only exemption: env `ER_EFFECTS_TELEMETRY_ONLY=1` OR GAME_DIR file
/// `er-effects-telemetry-only.txt`. The SOLE case the DLL may run without an env-provided
/// save source, because it loads no character (pure observation).
pub(crate) fn save_override_telemetry_only() -> bool {
    matches!(
        std::env::var("ER_EFFECTS_TELEMETRY_ONLY").as_deref(),
        Ok("1")
    ) || game_directory_path()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("er-effects-telemetry-only.txt")
        .exists()
}

/// Save-IO TRACE gate (ER_EFFECTS_SAVE_TRACE=1 / er-effects-save-trace.txt). When set, install the
/// save-redirect hooks for their DIAGNOSTICS ONLY (CreateFileW + NtCreateFile path logging) even with
/// NO redirect dir set -- so we can trace how the WORKING vanilla case (a char-present save in the
/// real appdata, no redirect) opens ER0000.sl2. No redirect, no abort; pure observation.
pub(crate) fn save_trace_enabled() -> bool {
    matches!(std::env::var("ER_EFFECTS_SAVE_TRACE").as_deref(), Ok("1"))
        || game_directory_path()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("er-effects-save-trace.txt")
            .exists()
}

/// Redirect directory (UTF-16, NUL-free, no trailing separator) computed from the parent of
/// `ER_EFFECTS_SAVE_FILE`. Set once at init, BEFORE the CreateFileW hook is armed.
static SAVE_REDIRECT_DIR_W: OnceLock<Vec<u16>> = OnceLock::new();
/// Original CreateFileW / CopyFileW (MinHook trampolines). 0 = not hooked.
static SAVE_REDIRECT_ORIG_CREATEFILEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_ORIG_COPYFILEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Save-existence-check redirects: the game stats/enumerates the save file BEFORE opening it; if
/// these hit the (wiped) default dir the game concludes "no save" and never CreateFileW's it.
static SAVE_REDIRECT_ORIG_GETATTRW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_ORIG_GETATTREXW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_ORIG_FINDFIRSTW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// PRIMARY redirect: the save-dir builder (FUN_140e0e680) calls SHGetFolderPathW(CSIDL_APPDATA) to
/// get %APPDATA%, then formats `%APPDATA%/EldenRing/<steamid>/`. Returning OUR staged root here makes
/// the game build AND open the full save path under our tree NATIVELY (Wine does case-insensitive
/// resolution), so the character is read without depending on intercepting each handle-relative open.
static SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_REDIRECT_SHGFP_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// One-shot redirect latch (user design 2026-06-23): the gold is provided via the Z: staged dir for
/// the FIRST load (reading from Z: works), but writing to Z: fails (Wine free-space) AND would mutate
/// the user's save. So once the gold profile is loaded (profile_slot_active != 0), we STOP redirecting
/// -- SHGetFolderPathW reverts to the real %APPDATA% so the system-save WRITE and all subsequent
/// load/save paths land on the proper default C: dir (write works, gold never touched).
static SAVE_FIRST_LOAD_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// ntdll NtCreateFile diagnostic: the boot save read happens BELOW Win32 (no CreateFileW/
/// GetFileAttributesW/FindFirstFileW hit the save), so hook the ntdll chokepoint to SEE the actual
/// open of ER0000.sl2 -- its NT path form and whether it is relative to a RootDirectory handle.
static SAVE_REDIRECT_ORIG_NTCREATEFILE: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_NTCREATE_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
const SAVE_NTCREATE_DIAG_MAX: usize = 120;
/// THE corruption fix (corrupted-save-re-findings): the save commit prechecks free space via
/// GetDiskFreeSpaceExW(saveDir), which on the Wine Z:->/home drive mapping returns bogus/ZERO free
/// space -> `free < needed` -> the write aborts BEFORE any byte ("Failed to save game / corrupted").
/// We hook it to report ample free space for the save dir so the game's OWN save flow writes our
/// staged save (no hardcoded paths, no Steam Cloud).
static SAVE_REDIRECT_ORIG_GETDISKFREEW: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_DISKFREE_LOGGED: AtomicUsize = AtomicUsize::new(0);
/// The game doesn't call kernel32!GetDiskFreeSpaceExW from our hook (no fire) -- under Wine all
/// free-space queries funnel to ntdll!NtQueryVolumeInformationFile. Override the AVAILABLE allocation
/// units for FileFsSizeInformation(3)/FileFsFullSizeInformation(7) so the save-commit free-space
/// precheck sees ample space regardless of the bogus Z:-drive report. THE corruption fix, robust.
static SAVE_REDIRECT_ORIG_NTQUERYVOLINFO: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
static SAVE_VOLINFO_LOGGED: AtomicUsize = AtomicUsize::new(0);
static SAVE_REDIRECT_INSTALL_ONCE: Once = Once::new();
/// Count of save-path opens we have redirected, logged for the first few so a probe can CONFIRM the
/// game actually opened our staged save through the redirect (not the default dir). Capped so a
/// busy IO loop cannot spam the debug log.
static SAVE_REDIRECT_HITS: AtomicUsize = AtomicUsize::new(0);
const SAVE_REDIRECT_LOG_MAX: usize = 8;
/// Diagnostic: total CreateFileW calls our detour observed (proves the hook is live at all under
/// Wine's kernel32->kernelbase forwarding), and a bounded log of save-LIKE paths so we can see the
/// exact path form the game opens the save with (to fix the filter or confirm a missed hook).
static SAVE_CREATEFILEW_CALLS: AtomicUsize = AtomicUsize::new(0);
static SAVE_CREATEFILEW_DIAG_LOGGED: AtomicUsize = AtomicUsize::new(0);
const SAVE_CREATEFILEW_DIAG_MAX: usize = 200;
/// DEDICATED budget for save-FILE queries (paths ending .sl2 / .co2 or containing ER0000): the shared
/// CreateFileW/existence-check diag cap above is exhausted by early-boot `eldenring\` dir churn
/// (GraphicsConfig.xml etc.) BEFORE the actual save read, hiding whether/with-what-steamid the game
/// ever queries ER0000.sl2. This separate counter guarantees those queries are always logged. Reveals
/// the exact `EldenRing\<steamid>\ER0000.sl2` path the game builds (steamid match vs the staged 766).
static SAVE_SL2_QUERY_LOGGED: AtomicUsize = AtomicUsize::new(0);
const SAVE_SL2_QUERY_MAX: usize = 40;
/// Log EVERY CreateFileW path for the first N calls (the whole early-boot save-detection window), so
/// we can see exactly what the game opens after our staged EldenRing\ dir (why it never reads the
/// save). Beyond this, only save-LIKE paths are logged.
const SAVE_CREATEFILEW_DIAG_ALL_BELOW: usize = 120;

/// Frames of "profile summary present but ZERO active slots" tolerated before the save-load watchdog
/// aborts. ~15s at 60fps -- long enough to ignore the boot transient before the summary is parsed,
/// short enough to fast-fail well under the runtime cap instead of stalling on the privacy policy.
static SAVE_WATCHDOG_ZERO_FRAMES: AtomicUsize = AtomicUsize::new(0);
const SAVE_WATCHDOG_ZERO_BUDGET: usize = 900;

/// Convert a Unix absolute path (e.g. `/home/banon/.../save`) to the Wine drive form the in-process
/// `CreateFileW` accepts -- `Z:` maps to `/` under Proton/Wine (confirmed: the game opens our log as
/// `\\?\Z:\home\...`). Backslash separators, no trailing separator. Returns a wide string.
fn unix_path_to_wine_wide(root: &std::path::Path) -> Vec<u16> {
    // to_string_lossy: building a path string, not decoding game memory (the from_utf8_lossy ban
    // targets in-process telemetry; OsStr->String here is fine).
    let win: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c == '/' { '\\' } else { c })
        .collect();
    let mut out: Vec<u16> = "Z:".encode_utf16().chain(win.encode_utf16()).collect();
    while matches!(out.last(), Some(&c) if c == b'\\' as u16) {
        out.pop();
    }
    out
}

/// Resolve `ER_EFFECTS_SAVE_FILE` -> the staged save ROOT (the ancestor directory that CONTAINS the
/// `EldenRing` folder) in Wine `Z:\...` wide form, or None if the env is unset/blank/not a readable
/// plausibly-sized save / not staged under an `EldenRing` directory component. The redirect rewrites
/// the game's `...\Roaming\EldenRing\<rest>` to `<root>\EldenRing\<rest>`, so the staged save MUST
/// live at `<root>/EldenRing/<steamid>/ER0000.sl2`.
fn save_override_redirect_root_w() -> Option<Vec<u16>> {
    let raw = std::env::var("ER_EFFECTS_SAVE_FILE").ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    let meta = std::fs::metadata(&path).ok()?;
    if !meta.is_file() || meta.len() < SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES {
        return None;
    }
    let mut root = PathBuf::new();
    let mut found = false;
    for comp in path.components() {
        if comp
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("EldenRing")
        {
            found = true;
            break;
        }
        root.push(comp);
    }
    if !found {
        return None;
    }
    Some(unix_path_to_wine_wide(&root))
}

/// Outcome of `enforce_save_override_or_abort`. The abort path does not return.
pub(crate) enum SaveOverrideMode {
    /// Pure telemetry/observe-only: no save source required, no redirect installed.
    TelemetryOnly,
    /// A valid env save source was resolved; the redirect hook should be installed.
    Redirect,
}

/// Called EARLY in `DllMain` (before any save IO). Enforces the no-default-fallback rule:
/// unless telemetry-only, a valid `ER_EFFECTS_SAVE_FILE` MUST be present, else the process is
/// aborted immediately. On success it stashes the redirect directory for the CreateFileW hook.
/// NEVER returns on the fail-closed path.
pub(crate) fn enforce_save_override_or_abort() -> SaveOverrideMode {
    if save_override_telemetry_only() {
        append_autoload_debug(format_args!(
            "save-override: TELEMETRY-ONLY mode -- save source not enforced (loads nothing; no default-dir read for a character)"
        ));
        return SaveOverrideMode::TelemetryOnly;
    }
    match save_override_redirect_root_w() {
        Some(root_w) => {
            // UTF-8 Lossy: log-only decode of the staged root for probe confirmation.
            let shown = String::from_utf16_lossy(&root_w);
            let _ = SAVE_REDIRECT_DIR_W.set(root_w);
            append_autoload_debug(format_args!(
                "save-override: ENFORCED -- redirecting the whole %APPDATA%\\Roaming\\EldenRing save subtree to staged root '{shown}' (expects <root>\\EldenRing\\<steamid>\\ER0000.sl2)"
            ));
            SaveOverrideMode::Redirect
        }
        None => {
            // FAIL CLOSED. The DLL must never assume the default user save directory.
            append_autoload_debug(format_args!(
                "save-override: FATAL -- ER_EFFECTS_SAVE_FILE is unset/blank/not a readable save (>= {} bytes) staged under an EldenRing dir, and ER_EFFECTS_TELEMETRY_ONLY is not set. Refusing to assume the default user save directory. ABORTING.",
                SAVE_OVERRIDE_MIN_PLAUSIBLE_BYTES
            ));
            eprintln!(
                "er-effects: FATAL -- no env-provided save source (ER_EFFECTS_SAVE_FILE) and not telemetry-only; refusing to assume the default user save directory. Aborting."
            );
            std::process::abort();
        }
    }
}

/// Length of a NUL-terminated UTF-16 string at `ptr` (excludes the NUL). 0 on null pointer.
unsafe fn wide_len(ptr: *const u16) -> usize {
    if ptr.is_null() {
        return 0;
    }
    let mut len = 0usize;
    // Bounded scan: a path longer than this is not a real Windows path; stop to stay safe.
    const WIDE_SCAN_MAX: usize = 0x8000;
    while len < WIDE_SCAN_MAX {
        if unsafe { *ptr.add(len) } == 0 {
            break;
        }
        len += 1;
    }
    len
}

/// ASCII-lowercase a UTF-16 code unit (leaves non-ASCII untouched).
fn wide_ascii_lower(c: u16) -> u16 {
    if (b'A' as u16..=b'Z' as u16).contains(&c) {
        c + 0x20
    } else {
        c
    }
}

/// True if `hay` contains `needle` (ASCII, case-insensitive). `needle` must be ASCII lowercase.
fn wide_contains_ci_ascii(hay: &[u16], needle: &[u16]) -> bool {
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let last = hay.len() - needle.len();
    (0..=last).any(|start| {
        needle
            .iter()
            .enumerate()
            .all(|(i, &n)| wide_ascii_lower(hay[start + i]) == n)
    })
}

/// First index in `hay` where `needle` occurs (ASCII, case-insensitive). `needle` must be ASCII
/// lowercase. None if absent.
fn wide_find_ci_ascii(hay: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() || needle.len() > hay.len() {
        return None;
    }
    let last = hay.len() - needle.len();
    (0..=last).find(|&start| {
        needle
            .iter()
            .enumerate()
            .all(|(i, &n)| wide_ascii_lower(hay[start + i]) == n)
    })
}

/// True if `hay` ends with `suffix` (ASCII, case-insensitive). `suffix` must be ASCII lowercase.
fn wide_ends_with_ci_ascii(hay: &[u16], suffix: &[u16]) -> bool {
    if suffix.len() > hay.len() {
        return false;
    }
    let start = hay.len() - suffix.len();
    suffix
        .iter()
        .enumerate()
        .all(|(i, &s)| wide_ascii_lower(hay[start + i]) == s)
}

/// Index just after the last path separator in `path` (0 if none) -- the basename start.
fn wide_basename_start(path: &[u16]) -> usize {
    let mut start = 0usize;
    for (i, &c) in path.iter().enumerate() {
        if c == b'\\' as u16 || c == b'/' as u16 {
            start = i + 1;
        }
    }
    start
}

/// If `path` is anywhere under the game's `%APPDATA%\Roaming\EldenRing` save root, return its
/// redirected (NUL-terminated) wide path under our staged EldenRing tree. None = not the save root.
///
/// We redirect the ENTIRE EldenRing-appdata SUBTREE (the `...\Roaming\EldenRing` directory handle and
/// everything under it), not just `*.sl2` files: the game decides "save present?" by ENUMERATING the
/// `EldenRing\` directory handle (Wine NtQueryDirectoryFile), never opening `<steamid>\ER0000.sl2` by
/// path -- so a per-file redirect can't be seen. By rewriting the directory open itself, the
/// handle-relative enumeration lists OUR staged `EldenRing\<steamid>\ER0000.sl2`.
///
/// `SAVE_REDIRECT_DIR_W` holds the staged ROOT that CONTAINS the `EldenRing` folder, in Wine form
/// (`Z:\home\...\save`). The redirect keeps the `EldenRing\<rest>` suffix: game
/// `C:\users\steamuser\AppData\Roaming\EldenRing\<id>\ER0000.sl2` -> `<root>\EldenRing\<id>\ER0000.sl2`.
fn save_redirect_path(path: &[u16]) -> Option<Vec<u16>> {
    let root = SAVE_REDIRECT_DIR_W.get()?;
    const ELDENRING: &[u16] = &[
        b'e' as u16,
        b'l' as u16,
        b'd' as u16,
        b'e' as u16,
        b'n' as u16,
        b'r' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    const ROAMING: &[u16] = &[
        b'r' as u16,
        b'o' as u16,
        b'a' as u16,
        b'm' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    // Anchor on `Roaming` + `EldenRing` so a coincidental "eldenring" elsewhere -- and our already-
    // redirected target (`Z:\...\save\EldenRing\...`, no "Roaming") -- never re-redirects.
    if !wide_contains_ci_ascii(path, ROAMING) {
        return None;
    }
    let idx = wide_find_ci_ascii(path, ELDENRING)?;
    let suffix = &path[idx..]; // "EldenRing\<id>\ER0000.sl2" (or "EldenRing\" for the dir open)
    let mut out = Vec::with_capacity(root.len() + 1 + suffix.len() + 1);
    out.extend_from_slice(root);
    out.push(b'\\' as u16);
    // ASCII-lowercase the suffix: the game opens the save root in MIXED case ("EldenRing\" for the
    // dir handle, "eldenring\graphicsconfig.xml" elsewhere). Our staged tree is on a CASE-SENSITIVE
    // Linux filesystem, so we normalize every case-variant to lowercase and stage the tree lowercase
    // (eldenring/<steamid>/er0000.sl2). The game reads through the returned HANDLE -- it does not care
    // about the redirected filename's case; the Windows-side case-insensitive name compare still
    // matches the enumerated lowercase entries.
    for &c in suffix {
        out.push(wide_ascii_lower(c));
    }
    out.push(0);
    Some(out)
}

type CreateFileWFn =
    unsafe extern "system" fn(*const u16, u32, u32, *const c_void, u32, u32, isize) -> isize;
type CopyFileWFn = unsafe extern "system" fn(*const u16, *const u16, i32) -> i32;

/// CreateFileW detour: redirect save-file opens to the env dir; pass everything else through.
/// Covers BOTH read and write (the returned HANDLE is reused by ReadFile/WriteFile).
unsafe extern "system" fn save_redirect_createfilew_hook(
    lp_file_name: *const u16,
    access: u32,
    share: u32,
    security: *const c_void,
    disposition: u32,
    flags: u32,
    template: isize,
) -> isize {
    let orig = SAVE_REDIRECT_ORIG_CREATEFILEW.load(Ordering::SeqCst);
    let call: CreateFileWFn = unsafe { std::mem::transmute::<usize, CreateFileWFn>(orig) };
    let len = unsafe { wide_len(lp_file_name) };
    let calls = SAVE_CREATEFILEW_CALLS.fetch_add(1, Ordering::SeqCst);
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        // Diagnostic: confirm the hook is live (log the very first call), then log save-LIKE paths
        // (contain "eldenring" or end .sl2/.co2/.bak) so we can see the exact save path form even when
        // the redirect filter does NOT match -- distinguishes "hook never fires" from "filter misses".
        const ELDENRING_SEG: &[u16] = &[
            b'e' as u16,
            b'l' as u16,
            b'd' as u16,
            b'e' as u16,
            b'n' as u16,
            b'r' as u16,
            b'i' as u16,
            b'n' as u16,
            b'g' as u16,
        ];
        const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
        const CO2D: &[u16] = &[b'.' as u16, b'c' as u16, b'o' as u16, b'2' as u16];
        const BAKD: &[u16] = &[b'.' as u16, b'b' as u16, b'a' as u16, b'k' as u16];
        let save_like = wide_contains_ci_ascii(path, ELDENRING_SEG)
            || wide_ends_with_ci_ascii(path, SL2D)
            || wide_ends_with_ci_ascii(path, CO2D)
            || wide_ends_with_ci_ascii(path, BAKD);
        if calls == 0 || save_like {
            let d = SAVE_CREATEFILEW_DIAG_LOGGED.load(Ordering::SeqCst);
            if d < SAVE_CREATEFILEW_DIAG_MAX {
                SAVE_CREATEFILEW_DIAG_LOGGED.store(d + 1, Ordering::SeqCst);
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
                let p = String::from_utf16_lossy(path);
                append_autoload_debug(format_args!(
                    "save-override: CreateFileW diag call#{calls} save_like={save_like} '{p}'"
                ));
            }
        }
        if let Some(redirected) = save_redirect_path(path) {
            let ret = unsafe {
                call(
                    redirected.as_ptr(),
                    access,
                    share,
                    security,
                    disposition,
                    flags,
                    template,
                )
            };
            let hit = SAVE_REDIRECT_HITS.fetch_add(1, Ordering::SeqCst);
            if hit < SAVE_REDIRECT_LOG_MAX {
                // UTF-8 Lossy: log-only decode of a Windows wide path for probe confirmation.
                let from = String::from_utf16_lossy(path);
                let to_end = redirected
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(redirected.len());
                // UTF-8 Lossy: log-only decode of the redirected wide path.
                let to = String::from_utf16_lossy(&redirected[..to_end]);
                // ret == -1 (INVALID_HANDLE_VALUE) means the redirected path did NOT resolve (Wine
                // path/case miss) -> the game falls back to no-save. ok=true means our file opened.
                let ok = ret != -1;
                append_autoload_debug(format_args!(
                    "save-override: REDIRECT #{hit} access=0x{access:x} disp={disposition} ok={ok} ret=0x{ret:x} '{from}' -> '{to}'"
                ));
            }
            return ret;
        }
    }
    unsafe {
        call(
            lp_file_name,
            access,
            share,
            security,
            disposition,
            flags,
            template,
        )
    }
}

/// CopyFileW detour: redirect either endpoint that is a save artifact (the `.bak` backup routine
/// copies ER0000.sl2 -> ER0000.sl2.bak), so backups follow the save into the env dir and never
/// touch the default user directory.
unsafe extern "system" fn save_redirect_copyfilew_hook(
    existing: *const u16,
    new_file: *const u16,
    fail_if_exists: i32,
) -> i32 {
    let orig = SAVE_REDIRECT_ORIG_COPYFILEW.load(Ordering::SeqCst);
    let call: CopyFileWFn = unsafe { std::mem::transmute::<usize, CopyFileWFn>(orig) };
    let existing_red = {
        let len = unsafe { wide_len(existing) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(existing, len) })
            .and_then(save_redirect_path)
    };
    let new_red = {
        let len = unsafe { wide_len(new_file) };
        (len != 0)
            .then(|| unsafe { std::slice::from_raw_parts(new_file, len) })
            .and_then(save_redirect_path)
    };
    let existing_ptr = existing_red.as_ref().map_or(existing, |v| v.as_ptr());
    let new_ptr = new_red.as_ref().map_or(new_file, |v| v.as_ptr());
    unsafe { call(existing_ptr, new_ptr, fail_if_exists) }
}

/// Shared diag + redirect decision for a save-existence-check API taking a wide path arg1. Logs
/// "eldenring"-containing paths (capped, shared budget) so we see the exact existence-check path
/// form, and returns the redirected NUL-terminated path when the save filter matches (else None).
fn save_path_api_redirect(api: &str, path: &[u16]) -> Option<Vec<u16>> {
    const ELDENRING_SEG: &[u16] = &[
        b'e' as u16,
        b'l' as u16,
        b'd' as u16,
        b'e' as u16,
        b'n' as u16,
        b'r' as u16,
        b'i' as u16,
        b'n' as u16,
        b'g' as u16,
    ];
    let redirected = save_redirect_path(path);
    // DEDICATED save-FILE query log (own budget; immune to the early-boot churn that exhausts the
    // shared cap below) -- captures the exact ER0000.sl2 existence/enum path + its <steamid> component.
    const ER0000: &[u16] = &[
        b'e' as u16,
        b'r' as u16,
        b'0' as u16,
        b'0' as u16,
        b'0' as u16,
        b'0' as u16,
    ];
    if wide_contains_ci_ascii(path, ER0000) {
        let d = SAVE_SL2_QUERY_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < SAVE_SL2_QUERY_MAX {
            // UTF-8 Lossy: log-only decode of the save-file query path for probe diagnosis.
            let p = String::from_utf16_lossy(path);
            let did = if redirected.is_some() {
                "REDIRECT"
            } else {
                "pass"
            };
            append_autoload_debug(format_args!("save-override: {api} SL2-QUERY {did} '{p}'"));
        }
    }
    if wide_contains_ci_ascii(path, ELDENRING_SEG) {
        let d = SAVE_CREATEFILEW_DIAG_LOGGED.load(Ordering::SeqCst);
        if d < SAVE_CREATEFILEW_DIAG_MAX {
            SAVE_CREATEFILEW_DIAG_LOGGED.store(d + 1, Ordering::SeqCst);
            // UTF-8 Lossy: log-only decode of a Windows wide path for probe diagnosis.
            let p = String::from_utf16_lossy(path);
            let did = if redirected.is_some() {
                "REDIRECT"
            } else {
                "pass"
            };
            append_autoload_debug(format_args!("save-override: {api} diag {did} '{p}'"));
        }
    }
    redirected
}

/// GetFileAttributesW detour: redirect save-path existence checks to the env dir.
unsafe extern "system" fn save_redirect_getattrw_hook(lp_file_name: *const u16) -> u32 {
    let orig = SAVE_REDIRECT_ORIG_GETATTRW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16) -> u32 =
        unsafe { std::mem::transmute::<usize, unsafe extern "system" fn(*const u16) -> u32>(orig) };
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("GetFileAttributesW", path) {
            return unsafe { call(red.as_ptr()) };
        }
    }
    unsafe { call(lp_file_name) }
}

/// GetFileAttributesExW detour: same redirect for the Ex existence check.
unsafe extern "system" fn save_redirect_getattrexw_hook(
    lp_file_name: *const u16,
    info_level: i32,
    info: *mut c_void,
) -> i32 {
    let orig = SAVE_REDIRECT_ORIG_GETATTREXW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32 = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32>(
            orig,
        )
    };
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("GetFileAttributesExW", path) {
            return unsafe { call(red.as_ptr(), info_level, info) };
        }
    }
    unsafe { call(lp_file_name, info_level, info) }
}

/// FindFirstFileW detour: redirect save-path enumeration/existence checks to the env dir.
unsafe extern "system" fn save_redirect_findfirstw_hook(
    lp_file_name: *const u16,
    find_data: *mut c_void,
) -> isize {
    let orig = SAVE_REDIRECT_ORIG_FINDFIRSTW.load(Ordering::SeqCst);
    let call: unsafe extern "system" fn(*const u16, *mut c_void) -> isize = unsafe {
        std::mem::transmute::<usize, unsafe extern "system" fn(*const u16, *mut c_void) -> isize>(
            orig,
        )
    };
    let len = unsafe { wide_len(lp_file_name) };
    if len != 0 {
        let path = unsafe { std::slice::from_raw_parts(lp_file_name, len) };
        if let Some(red) = save_path_api_redirect("FindFirstFileW", path) {
            return unsafe { call(red.as_ptr(), find_data) };
        }
    }
    unsafe { call(lp_file_name, find_data) }
}

type ShGetFolderPathWFn = unsafe extern "system" fn(isize, i32, isize, u32, *mut u16) -> i32;

/// SHGetFolderPathW detour: for CSIDL_APPDATA, return our staged ROOT instead of the real %APPDATA%,
/// so the game's save-dir builder produces `<our_root>\EldenRing\<steamid>\...` and reads our gold
/// save's character natively. All other folders pass through unchanged.
unsafe extern "system" fn save_redirect_shgetfolderpathw_hook(
    hwnd: isize,
    csidl: i32,
    token: isize,
    flags: u32,
    path: *mut u16,
) -> i32 {
    const CSIDL_APPDATA: i32 = 0x1a;
    const CSIDL_FOLDER_MASK: i32 = 0xff; // low byte = folder id; high bits = CSIDL_FLAG_*
    const S_OK: i32 = 0;
    const MAX_PATH_W: usize = 259;
    // One-shot: after the first gold load, revert to the real %APPDATA% so writes + subsequent loads
    // use the proper default C: dir (the Z: redirect only serves the first read of the gold).
    if (csidl & CSIDL_FOLDER_MASK) == CSIDL_APPDATA
        && !path.is_null()
        && !SAVE_FIRST_LOAD_DONE.load(Ordering::SeqCst)
    {
        if let Some(root) = SAVE_REDIRECT_DIR_W.get() {
            let n = root.len().min(MAX_PATH_W);
            for i in 0..n {
                unsafe { *path.add(i) = root[i] };
            }
            unsafe { *path.add(n) = 0 };
            let prev = SAVE_REDIRECT_SHGFP_LOGGED.swap(1, Ordering::SeqCst);
            if prev == 0 {
                // UTF-8 Lossy: log-only decode of the staged root for probe confirmation.
                let shown = String::from_utf16_lossy(&root[..n]);
                append_autoload_debug(format_args!(
                    "save-override: SHGetFolderPathW(CSIDL_APPDATA) -> staged root '{shown}' (game now builds all save paths under our tree)"
                ));
            }
            return S_OK;
        }
    }
    let orig = SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW.load(Ordering::SeqCst);
    let call: ShGetFolderPathWFn =
        unsafe { std::mem::transmute::<usize, ShGetFolderPathWFn>(orig) };
    unsafe { call(hwnd, csidl, token, flags, path) }
}

type NtCreateFileFn = unsafe extern "system" fn(
    *mut isize,
    u32,
    *const u8,
    *mut u8,
    *const i64,
    u32,
    u32,
    u32,
    u32,
    *const c_void,
    u32,
) -> i32;

/// NtCreateFile DIAGNOSTIC detour: logs save-LIKE opens (path contains "eldenring" or ends .sl2),
/// including whether the open is RELATIVE to a RootDirectory handle (the invisible-to-Win32 path the
/// game uses for the boot save read). Pure logging -- always calls the original unchanged.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn save_ntcreatefile_diag_hook(
    handle: *mut isize,
    access: u32,
    object_attributes: *const u8,
    iosb: *mut u8,
    alloc: *const i64,
    file_attrs: u32,
    share: u32,
    disposition: u32,
    options: u32,
    ea: *const c_void,
    ea_len: u32,
) -> i32 {
    // OBJECT_ATTRIBUTES (x64): +0x08 RootDirectory (HANDLE), +0x10 ObjectName (PUNICODE_STRING).
    // UNICODE_STRING (x64): +0x00 Length(u16 bytes), +0x08 Buffer(PWSTR).
    // Captured pre-call (path, is_sl2); logged with the NTSTATUS result after the original returns so
    // a FAILING save-commit open is unambiguous (the prior diag logged only the request, never ret).
    let mut save_diag: Option<(String, bool)> = None;
    if !object_attributes.is_null() {
        let objname = unsafe { *(object_attributes.add(0x10) as *const usize) } as *const u8;
        if !objname.is_null() {
            let len_bytes = unsafe { *(objname as *const u16) } as usize;
            let buf = unsafe { *(objname.add(0x08) as *const usize) } as *const u16;
            if !buf.is_null() && len_bytes >= 2 && len_bytes < 0x2000 {
                let nwch = len_bytes / 2;
                let path = unsafe { std::slice::from_raw_parts(buf, nwch) };
                const ELDENRING_SEG: &[u16] = &[
                    b'e' as u16,
                    b'l' as u16,
                    b'd' as u16,
                    b'e' as u16,
                    b'n' as u16,
                    b'r' as u16,
                    b'i' as u16,
                    b'n' as u16,
                    b'g' as u16,
                ];
                const SL2D: &[u16] = &[b'.' as u16, b's' as u16, b'l' as u16, b'2' as u16];
                // Focus the (capped) budget on ER0000.sl2 opens ONLY -- early boot churns hundreds
                // of "eldenring"-dir opens (graphicsconfig.xml, etc.) that otherwise exhaust the cap
                // before the boot save READ/WRITE we care about. The .sl2 opens ARE the save commit.
                let _ = ELDENRING_SEG;
                let is_sl2 = wide_ends_with_ci_ascii(path, SL2D);
                if is_sl2
                    && SAVE_NTCREATE_DIAG_LOGGED.load(Ordering::SeqCst) < SAVE_NTCREATE_DIAG_MAX
                {
                    // UTF-8 Lossy: log-only decode of an NT path for probe diagnosis.
                    save_diag = Some((String::from_utf16_lossy(path), is_sl2));
                }
            }
        }
    }
    let orig = SAVE_REDIRECT_ORIG_NTCREATEFILE.load(Ordering::SeqCst);
    let call: NtCreateFileFn = unsafe { std::mem::transmute::<usize, NtCreateFileFn>(orig) };
    let ret = unsafe {
        call(
            handle,
            access,
            object_attributes,
            iosb,
            alloc,
            file_attrs,
            share,
            disposition,
            options,
            ea,
            ea_len,
        )
    };
    if let Some((p, is_sl2)) = save_diag {
        let d = SAVE_NTCREATE_DIAG_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < SAVE_NTCREATE_DIAG_MAX {
            // ret is NTSTATUS (0 == STATUS_SUCCESS). is_write keys off GENERIC_WRITE (0x40000000)
            // or FILE_WRITE_DATA (0x2) so a failing save COMMIT is unambiguous in the log.
            let is_write = access & 0x4000_0000 != 0 || access & 0x2 != 0;
            append_autoload_debug(format_args!(
                "save-override: NtCreateFile diag access=0x{access:x} disp={disposition} opts=0x{options:x} write={is_write} sl2={is_sl2} ret=0x{ret:x} '{p}'"
            ));
        }
    }
    ret
}

type GetDiskFreeSpaceExWFn =
    unsafe extern "system" fn(*const u16, *mut u64, *mut u64, *mut u64) -> i32;

/// GetDiskFreeSpaceExW detour: for the EldenRing save dir, report ample free space (Wine returns
/// bogus 0 on the Z:->/home drive, which fails the save-commit free-space precheck -> corrupted-save
/// loop). Everything else passes through unchanged.
unsafe extern "system" fn save_redirect_getdiskfreew_hook(
    lp_dir: *const u16,
    free_avail: *mut u64,
    total: *mut u64,
    total_free: *mut u64,
) -> i32 {
    // Override EVERY call (the game's save-commit precheck may pass the bare drive root, not an
    // EldenRing path -- diag showed it never matched the eldenring filter). Returning ample free is
    // benign for a probe and guarantees the `free < needed` precheck passes. Log the first few paths.
    const AMPLE_FREE: u64 = 0x10_0000_0000; // 64 GiB
    if !free_avail.is_null() {
        unsafe { *free_avail = AMPLE_FREE };
    }
    if !total.is_null() {
        unsafe { *total = AMPLE_FREE };
    }
    if !total_free.is_null() {
        unsafe { *total_free = AMPLE_FREE };
    }
    let d = SAVE_DISKFREE_LOGGED.fetch_add(1, Ordering::SeqCst);
    if d < 6 {
        let len = unsafe { wide_len(lp_dir) };
        // UTF-8 Lossy: log-only decode of the free-space query path for probe confirmation.
        let p = if len != 0 {
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(lp_dir, len) })
        } else {
            String::new()
        };
        append_autoload_debug(format_args!(
            "save-override: GetDiskFreeSpaceExW #{d} '{p}' -> ample free (unblock save-commit precheck)"
        ));
    }
    1 // TRUE
}

type NtQueryVolumeInfoFn = unsafe extern "system" fn(isize, *mut u8, *mut u8, u32, u32) -> i32;

/// NtQueryVolumeInformationFile detour: override the AVAILABLE free-space units for the size info
/// classes so the save-commit precheck passes (Wine reports bogus 0 free on the Z: staged drive).
unsafe extern "system" fn save_redirect_ntqueryvolinfo_hook(
    handle: isize,
    iosb: *mut u8,
    fs_info: *mut u8,
    length: u32,
    fs_class: u32,
) -> i32 {
    const FILE_FS_SIZE_INFORMATION: u32 = 3;
    const FILE_FS_FULL_SIZE_INFORMATION: u32 = 7;
    const AMPLE_UNITS: i64 = 0x1000_0000; // ~268M allocation units -> ample free regardless of unit size
    let orig = SAVE_REDIRECT_ORIG_NTQUERYVOLINFO.load(Ordering::SeqCst);
    let call: NtQueryVolumeInfoFn =
        unsafe { std::mem::transmute::<usize, NtQueryVolumeInfoFn>(orig) };
    let ret = unsafe { call(handle, iosb, fs_info, length, fs_class) };
    // DIAGNOSTIC: log only the FREE-SPACE classes (3/7), capped. Logging every class exhausts the cap
    // on early-boot class=1 spam before the save-time free-space precheck fires; the precheck is the
    // only thing that matters for the corrupted-save loop. pre_avail_units = the bogus Wine value.
    if fs_class == FILE_FS_SIZE_INFORMATION || fs_class == FILE_FS_FULL_SIZE_INFORMATION {
        let d = SAVE_VOLINFO_LOGGED.load(Ordering::SeqCst);
        if d < 40 {
            SAVE_VOLINFO_LOGGED.store(d + 1, Ordering::SeqCst);
            let avail = if ret == 0 && !fs_info.is_null() && length >= 16 {
                unsafe { *(fs_info.add(8) as *const i64) }
            } else {
                -1
            };
            append_autoload_debug(format_args!(
                "save-override: NtQueryVolumeInformationFile diag class={fs_class} len={length} ret=0x{ret:x} pre_avail_units={avail}"
            ));
        }
    }
    if ret == 0 && !fs_info.is_null() {
        if fs_class == FILE_FS_SIZE_INFORMATION && length >= 16 {
            // [+0] TotalAllocationUnits (i64), [+8] AvailableAllocationUnits (i64).
            unsafe {
                *(fs_info.add(0) as *mut i64) = AMPLE_UNITS;
                *(fs_info.add(8) as *mut i64) = AMPLE_UNITS;
            }
        } else if fs_class == FILE_FS_FULL_SIZE_INFORMATION && length >= 24 {
            // [+0] Total, [+8] CallerAvailable, [+16] ActualAvailable (all i64).
            unsafe {
                *(fs_info.add(0) as *mut i64) = AMPLE_UNITS;
                *(fs_info.add(8) as *mut i64) = AMPLE_UNITS;
                *(fs_info.add(16) as *mut i64) = AMPLE_UNITS;
            }
        } else {
            return ret;
        }
        let d = SAVE_VOLINFO_LOGGED.fetch_add(1, Ordering::SeqCst);
        if d < 4 {
            append_autoload_debug(format_args!(
                "save-override: NtQueryVolumeInformationFile class={fs_class} -> ample free units (unblock save-commit precheck) #{d}"
            ));
        }
    }
    ret
}

/// True when running under Wine/Proton (ntdll exports `wine_get_version`, which native Windows does
/// not). The free-space-precheck workaround is a Wine-specific bug fix (Wine reports bogus 0 free for
/// the Z:->/home drive mapping); on native Windows it must NOT run (it would mask a real disk-full).
pub(crate) fn running_under_wine() -> bool {
    unsafe { module_proc(b"ntdll.dll\0", b"wine_get_version\0") != HOOK_ORIGINAL_UNSET }
}

/// Resolve an export address from an already-loaded module (NUL-terminated ASCII names). 0 if the
/// module isn't loaded or the export is absent.
unsafe fn module_proc(module_name: &[u8], proc_name: &[u8]) -> usize {
    let module = match unsafe { GetModuleHandleA(PCSTR::from_raw(module_name.as_ptr())) } {
        Ok(m) => m,
        Err(_) => return HOOK_ORIGINAL_UNSET,
    };
    match unsafe { GetProcAddress(module, PCSTR::from_raw(proc_name.as_ptr())) } {
        Some(p) => p as usize,
        None => HOOK_ORIGINAL_UNSET,
    }
}

/// Resolve a kernel32 export address by name (NUL-terminated ASCII). 0 if unavailable.
unsafe fn kernel32_proc(name: &[u8]) -> usize {
    unsafe { module_proc(b"kernel32.dll\0", name) }
}

/// Install the save-redirect hooks (CreateFileW + CopyFileW) ONCE. Idempotent. Must run while the
/// redirect dir is already stashed (after `enforce_save_override_or_abort` -> Redirect). Mirrors the
/// thread-spawn install pattern of the other early DllMain subsystems.
/// Queue one kernel32 export hook (resolve by name, store trampoline, queue-enable). Best-effort:
/// logs and skips on any failure. Used for the save-redirect existence-check APIs.
unsafe fn queue_save_redirect_hook(
    hooks: &mut Vec<MhHook>,
    name: &str,
    proc_name: &[u8],
    detour: *mut c_void,
    orig: &AtomicUsize,
) {
    let addr = unsafe { kernel32_proc(proc_name) };
    if addr == HOOK_ORIGINAL_UNSET {
        append_autoload_debug(format_args!(
            "save-override: could not resolve kernel32!{name}"
        ));
        return;
    }
    match unsafe { MhHook::new(addr as *mut c_void, detour) } {
        Ok(hook) => {
            orig.store(hook.trampoline() as usize, Ordering::SeqCst);
            if let Err(status) = unsafe { hook.queue_enable() } {
                append_autoload_debug(format_args!(
                    "save-override: {name} queue_enable failed: {status:?}"
                ));
            } else {
                hooks.push(hook);
            }
        }
        Err(status) => append_autoload_debug(format_args!(
            "save-override: MhHook::new {name} failed at 0x{addr:x}: {status:?}"
        )),
    }
}

pub(crate) fn install_save_redirect_hooks() {
    SAVE_REDIRECT_INSTALL_ONCE.call_once(|| {
        if SAVE_REDIRECT_DIR_W.get().is_none() && !save_trace_enabled() {
            append_autoload_debug(format_args!(
                "save-override: install skipped -- redirect dir not set (enforce did not run / telemetry-only)"
            ));
            return;
        }
        match unsafe { MH_Initialize() } {
            MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
            status => {
                append_autoload_debug(format_args!(
                    "save-override: MH_Initialize failed: {status:?}"
                ));
                return;
            }
        }
        append_autoload_debug(format_args!(
            "save-override: install begin -- running_under_wine={} (Wine-only free-space overrides {})",
            running_under_wine(),
            if running_under_wine() { "ARMED" } else { "SKIPPED" }
        ));
        let mut hooks = Vec::new();
        let create_addr = unsafe { kernel32_proc(b"CreateFileW\0") };
        if create_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    create_addr as *mut c_void,
                    save_redirect_createfilew_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_CREATEFILEW
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: CreateFileW queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new CreateFileW failed at 0x{create_addr:x}: {status:?}"
                )),
            }
        } else {
            append_autoload_debug(format_args!(
                "save-override: could not resolve kernel32!CreateFileW"
            ));
        }
        let copy_addr = unsafe { kernel32_proc(b"CopyFileW\0") };
        if copy_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    copy_addr as *mut c_void,
                    save_redirect_copyfilew_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_COPYFILEW.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: CopyFileW queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new CopyFileW failed at 0x{copy_addr:x}: {status:?}"
                )),
            }
        }
        // Existence-check redirects: the game stats/enumerates ER0000.sl2 before opening it; without
        // these the wiped default dir reads as "no save" and CreateFileW is never reached.
        unsafe {
            queue_save_redirect_hook(
                &mut hooks,
                "GetFileAttributesW",
                b"GetFileAttributesW\0",
                save_redirect_getattrw_hook as *mut c_void,
                &SAVE_REDIRECT_ORIG_GETATTRW,
            );
            queue_save_redirect_hook(
                &mut hooks,
                "GetFileAttributesExW",
                b"GetFileAttributesExW\0",
                save_redirect_getattrexw_hook as *mut c_void,
                &SAVE_REDIRECT_ORIG_GETATTREXW,
            );
            queue_save_redirect_hook(
                &mut hooks,
                "FindFirstFileW",
                b"FindFirstFileW\0",
                save_redirect_findfirstw_hook as *mut c_void,
                &SAVE_REDIRECT_ORIG_FINDFIRSTW,
            );
            // THE corruption fix (WINE ONLY): ample free space for the save dir (Wine Z: drive reports
            // bogus 0). Native Windows reports correctly, so this must not run there.
            if running_under_wine() {
                queue_save_redirect_hook(
                    &mut hooks,
                    "GetDiskFreeSpaceExW",
                    b"GetDiskFreeSpaceExW\0",
                    save_redirect_getdiskfreew_hook as *mut c_void,
                    &SAVE_REDIRECT_ORIG_GETDISKFREEW,
                );
            }
        }
        // PRIMARY: redirect the %APPDATA% root via SHGetFolderPathW (shell32) so the game builds and
        // opens the full save path under our staged tree natively -- this is what actually makes the
        // character load (the per-file kernel32 hooks above are a fallback for the real default dir).
        let shgfp_addr = unsafe { module_proc(b"shell32.dll\0", b"SHGetFolderPathW\0") };
        if shgfp_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    shgfp_addr as *mut c_void,
                    save_redirect_shgetfolderpathw_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_SHGETFOLDERPATHW
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: SHGetFolderPathW queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new SHGetFolderPathW failed at 0x{shgfp_addr:x}: {status:?}"
                )),
            }
        } else {
            append_autoload_debug(format_args!(
                "save-override: could not resolve shell32!SHGetFolderPathW (shell32 not loaded yet?)"
            ));
        }
        // THE corruption fix at the lowest layer (WINE ONLY): ntdll!NtQueryVolumeInformationFile
        // free-space override (the game's free-space precheck never reaches our kernel32 hook). Native
        // Windows reports free space correctly, so this Wine-bug workaround must not run there.
        let ntqvi_addr = if running_under_wine() {
            unsafe { module_proc(b"ntdll.dll\0", b"NtQueryVolumeInformationFile\0") }
        } else {
            HOOK_ORIGINAL_UNSET
        };
        if ntqvi_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    ntqvi_addr as *mut c_void,
                    save_redirect_ntqueryvolinfo_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_NTQUERYVOLINFO
                        .store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: NtQueryVolumeInformationFile queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new NtQueryVolumeInformationFile failed at 0x{ntqvi_addr:x}: {status:?}"
                )),
            }
        } else {
            append_autoload_debug(format_args!(
                "save-override: could not resolve ntdll!NtQueryVolumeInformationFile"
            ));
        }
        // DIAGNOSTIC: ntdll!NtCreateFile -- see the boot save read that is invisible to Win32.
        let ntcf_addr = unsafe { module_proc(b"ntdll.dll\0", b"NtCreateFile\0") };
        if ntcf_addr != HOOK_ORIGINAL_UNSET {
            match unsafe {
                MhHook::new(
                    ntcf_addr as *mut c_void,
                    save_ntcreatefile_diag_hook as *mut c_void,
                )
            } {
                Ok(hook) => {
                    SAVE_REDIRECT_ORIG_NTCREATEFILE.store(hook.trampoline() as usize, Ordering::SeqCst);
                    if let Err(status) = unsafe { hook.queue_enable() } {
                        append_autoload_debug(format_args!(
                            "save-override: NtCreateFile queue_enable failed: {status:?}"
                        ));
                    } else {
                        hooks.push(hook);
                    }
                }
                Err(status) => append_autoload_debug(format_args!(
                    "save-override: MhHook::new NtCreateFile failed at 0x{ntcf_addr:x}: {status:?}"
                )),
            }
        }
        match unsafe { MH_ApplyQueued() } {
            MH_STATUS::MH_OK => append_autoload_debug(format_args!(
                "save-override: INSTALLED SHGetFolderPathW(0x{shgfp_addr:x})+CreateFileW(0x{create_addr:x})+CopyFileW(0x{copy_addr:x})+GetFileAttributesW/ExW+FindFirstFileW save-path redirect -- default user save dir is now never read"
            )),
            status => append_autoload_debug(format_args!(
                "save-override: MH_ApplyQueued failed: {status:?}"
            )),
        }
        std::mem::forget(hooks);
    });
}
