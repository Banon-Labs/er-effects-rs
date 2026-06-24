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

mod save_redirect;
pub(crate) use save_redirect::*;

mod trace;
pub(crate) use trace::*;

mod startup_hooks;
pub(crate) use startup_hooks::*;

mod input_block;
pub(crate) use input_block::*;

mod own_load;
pub(crate) use own_load::*;

mod menu_diag;
pub(crate) use menu_diag::*;

mod mem;
pub(crate) use mem::*;

mod gating;
pub(crate) use gating::*;
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
    // native_fullread / cold_char_mount / native-continue paths AND the experimental menu-driven
    // product_core path. Set it whenever a valid slot is configured, regardless of method, so the
    // known-good zero-input smoke path does not depend on a fragile env-method side effect.
    OWN_STEPPER_SLOT.store(slot, Ordering::SeqCst);
    if request.method() == SaveLoadMethod::DirectMenuLoad && experimental_direct_menu_load_enabled()
    {
        PRODUCT_AUTOLOAD_ARMED.store(OWN_STEPPER_CALL_INC, Ordering::SeqCst);
    }
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

/// Zero-input NATURAL menu-open (the row-building path). At the parked press-any-button title
/// (TitleTopDialog settled in "Loop", menu not yet open a40==0), set the decoded global menu-accept
/// byte 0x144589bdc=1 ONCE so the game's OWN `TitleTopDialog::update` accept-gate runs the open-menu
/// registrar in its NATIVE frame -- which POSTS the Continue/Load/NewGame MenuJob chain AND drains it
/// (MenuWindow::Update 0x140745520) in the same native flow, so the rows actually BUILD. A direct
/// registrar self-fire (`maybe_auto_open_menu`) only POSTS the chain; the native update does not drain
/// a chain it did not open itself, so the rows never build (continue-scan = 0 nodes; bd
/// rowbuild-mechanism-incontext-openmenu-2026-06-23 + title-global-accept-byte-144589bdc). This is the
/// decoded accept FLAG the input pipeline sets on press -- NOT a synthesized DInput/keystate/XInput
/// event -> still `simulated_button_presses_total == 0`. Save-safe (menu-UI build, no save write). The
/// ToS/language over-trigger this byte caused in 2026-06 is now neutralized by the offline-mode +
/// Menu_IsEnableOnlineMode patches, so it should reach the main menu cleanly; the msgbox/policy oracles
/// will catch any regression. One-shot via TITLE_ACCEPT_BYTE_GATE_FIRED, latched only after the gating
/// passes so a not-yet-settled title does not consume the shot.
pub(crate) unsafe fn maybe_set_title_accept_byte(base: usize) {
    if TITLE_ACCEPT_BYTE_GATE_FIRED.load(Ordering::SeqCst) {
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
    // Only at the parked press-any-button (menu not yet open): a40 latch == 0.
    let a40 = unsafe { safe_read_usize(dialog + TITLE_TOP_DIALOG_MENU_OPENED_A40_OFFSET) }
        .map(|v| v & TITLE_TOP_DIALOG_LATCH_BYTE_MASK)
        .unwrap_or(1);
    if a40 != OWN_STEPPER_MENU_OPENED_NO {
        TITLE_ACCEPT_BYTE_GATE_FIRED.store(true, Ordering::SeqCst); // already open -> nothing to do
        return;
    }
    // Require the dialog SETTLED in Loop so the native update's accept-gate consumes our byte on its
    // next tick (read-only probe of the live state by name, no side effects).
    let sm = dialog + TITLE_TOP_DIALOG_STATE_MACHINE_A60_OFFSET;
    let is_in_state: unsafe extern "system" fn(usize, usize) -> u8 =
        unsafe { std::mem::transmute(base + TITLE_TOP_DIALOG_IS_IN_STATE_RVA) };
    let in_loop = unsafe { is_in_state(sm, base + TITLE_STATE_DESC_LOOP_RVA) } != OWN_STEPPER_FALSE;
    if !in_loop {
        return;
    }
    if TITLE_ACCEPT_BYTE_GATE_FIRED.swap(true, Ordering::SeqCst) {
        return;
    }
    unsafe {
        *((base + TITLE_GLOBAL_ACCEPT_BYTE_RVA) as *mut u8) = TITLE_PROCEED_GATE_SET_VALUE;
    }
    append_autoload_debug(format_args!(
        "title-accept-byte: set [0x{:x}]=1 on settled TitleTopDialog (Loop, a40==0) -- zero-input NATURAL menu-open (registrar runs in native update frame -> Continue/Load/NewGame rows build + drain)",
        base + TITLE_GLOBAL_ACCEPT_BYTE_RVA
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
    // Semaphore: number of MenuMemberFuncJob nodes in the dialog. 0 == the title menu's item LIST is
    // empty/not-built (the current Continue-fire blocker) -- exposed as oracle_continue_scan_node_hits.
    CONTINUE_SCAN_NODE_HITS.store(hits, Ordering::SeqCst);
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
/// CONTINUE-READINESS SEMAPHORE (oracle, no screenshot needed). The furthest stage the Continue-action
/// readiness chain reached this frame -- so "why didn't Continue fire" is answerable from RAM telemetry:
///   0 no menu-holder | 1 holder present | 2 holder IS TitleTopDialog | 3 row-registry valid
///   | 4 a Continue node found in the dialog | 5 node vtable is MemberFuncJob | 6 member_fn present
///   | 7 member_fn->Continue-wrapper chain validated (READY to fire).
/// Stuck at 3 with CONTINUE_SCAN_NODE_HITS==0 == the dialog is the title menu but its item LIST is
/// EMPTY (not built) -- the actual current blocker. CONTINUE_DIALOG_VT_SEEN = the active holder's
/// vtable (identifies WHICH screen is up when stage<2 -- e.g. a modal instead of the title menu).
pub(crate) static CONTINUE_READY_STAGE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CONTINUE_SCAN_NODE_HITS: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CONTINUE_DIALOG_VT_SEEN: AtomicUsize = AtomicUsize::new(0);

unsafe fn title_menu_continue_action_ready(owner: usize, base: usize) -> Option<MenuActionNode> {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    CONTINUE_READY_STAGE.store(0, Ordering::SeqCst);
    let dialog =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    if dialog == null {
        return None;
    }
    CONTINUE_READY_STAGE.store(1, Ordering::SeqCst);
    let dialog_vt = unsafe { safe_read_usize(dialog) }.unwrap_or(null);
    CONTINUE_DIALOG_VT_SEEN.store(dialog_vt, Ordering::SeqCst);
    if dialog_vt != base + TITLE_TOP_DIALOG_VTABLE_RVA {
        return None;
    }
    CONTINUE_READY_STAGE.store(2, Ordering::SeqCst);
    let registry =
        unsafe { safe_read_usize(dialog + DIALOG_ROW_REGISTRY_A48_OFFSET) }.unwrap_or(null);
    if !vtable_in_game_image(registry, base) {
        return None;
    }
    CONTINUE_READY_STAGE.store(3, Ordering::SeqCst);
    let node = unsafe { scan_dialog_for_continue(owner, base) }?;
    CONTINUE_READY_STAGE.store(4, Ordering::SeqCst);
    let node_vt = unsafe { safe_read_usize(node) }.unwrap_or(null);
    if node_vt != base + MEMBERFUNCJOB_VTABLE_RVA {
        return None;
    }
    CONTINUE_READY_STAGE.store(5, Ordering::SeqCst);
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
    CONTINUE_READY_STAGE.store(6, Ordering::SeqCst);
    let continue_wrapper_abs = base + TRACE_MENU_CONTINUE_WRAPPER_RVA as usize;
    let mut target = member_fn;
    let mut hop = HOP_START;
    while hop < JMP_HOPS && target != null {
        if target == continue_wrapper_abs {
            CONTINUE_READY_STAGE.store(7, Ordering::SeqCst);
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
    // The THIRD menu-open popup ("Starting in offline mode", GR_System_Message 401170) is gated by
    // TitleFlowContext->notReleaseFlag55 = !Menu_IsEnableOnlineMode(). Force that getter false so the
    // game's own ctx-init (0x14082d0d0) writes notReleaseFlag55=1 each time, the title-flow offline step
    // (0x14082fda0) takes the clean no-popup branch, and the Continue/Load/NewGame rows build with ZERO
    // MessageBoxDialog builds. Race-free + offline-gated (Seamless online unaffected). bd
    // menu-open-3rd-popup-offline-mode-notice-2026-06-23 / er-effects-rs-yvf.
    let menu_online_off = patch_3byte_stub(
        base,
        MENU_ONLINE_MODE_DISABLE_RVA,
        MENU_ONLINE_MODE_EXPECTED_FIRST,
        ONLINE_DISABLE_STUB,
        "menu-online-mode-disable",
    );
    append_autoload_debug(format_args!(
        "online-disable: Menu_IsEnableOnlineMode@0x{:x} patched ok={menu_online_off} -> xor eax,eax;ret (notReleaseFlag55 becomes 1 -> no 'Starting in offline mode' popup -> title rows build)",
        base + MENU_ONLINE_MODE_DISABLE_RVA
    ));
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


// (The 0x1407b0cf0 "finished-poll" auto-accept hook was removed: RE showed 0x1407b0cf0 is a
// "has >= 2 buttons" layout query, not a finished-poll -- it is never called for the
// connection-error dialog, and writing +0x25e0/+0x25e8 corrupts the dialog (+0x25e8 is the
// button COUNT). The dismiss is force_dismiss_startup_dialog -> OnDecide 0x140927ba0.)

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
