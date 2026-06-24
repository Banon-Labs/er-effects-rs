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

use super::*;

pub(crate) fn autoload_phase_elapsed_ms() -> u64 {
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
pub(crate) fn reset_phase_timer(timer: &AtomicU64) {
    timer.store(autoload_phase_elapsed_ms(), Ordering::SeqCst);
}
pub(crate) fn phase_elapsed_ms(timer: &AtomicU64) -> u64 {
    let started = timer.load(Ordering::SeqCst);
    if started == PHASE_TIMER_UNSET_MS {
        reset_phase_timer(timer);
        PHASE_TIMER_ZERO_MS
    } else {
        autoload_phase_elapsed_ms().saturating_sub(started)
    }
}
pub(crate) fn own_stepper_enter_menu_build_phase() {
    OWN_STEPPER_MENU_BUILD_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    reset_phase_timer(&OWN_STEPPER_MENU_BUILD_STARTED_MS);
    OWN_STEPPER_PHASE.store(OWN_STEPPER_PHASE_MENU_BUILD, Ordering::SeqCst);
}
pub(crate) fn own_stepper_menu_build_timed_out() -> bool {
    phase_elapsed_ms(&OWN_STEPPER_MENU_BUILD_STARTED_MS) >= OWN_STEPPER_MENU_BUILD_WAIT_MAX
}
pub(crate) fn own_stepper_menu_build_elapsed_ms() -> u64 {
    phase_elapsed_ms(&OWN_STEPPER_MENU_BUILD_STARTED_MS)
}
pub(crate) fn own_stepper_enter_s2_phase(phase: usize) {
    OWN_STEPPER_S2_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
    reset_phase_timer(&OWN_STEPPER_S2_PHASE_STARTED_MS);
    OWN_STEPPER_PHASE.store(phase, Ordering::SeqCst);
}
pub(crate) fn own_stepper_s2_timed_out() -> bool {
    phase_elapsed_ms(&OWN_STEPPER_S2_PHASE_STARTED_MS) >= OWN_STEPPER_S2_PHASE_MAX
}
pub(crate) fn own_stepper_s2_elapsed_ms() -> u64 {
    phase_elapsed_ms(&OWN_STEPPER_S2_PHASE_STARTED_MS)
}
/// SAVE-SAFE one-shot cold-build probe of the world-resource streaming driver. Validates the lever
/// emk-resman-streaming-driver-coldbuild-stub-lever-2026 live, WITHOUT SetState / world load.
/// The CSResStep tick getter 0x140cd6c50's body is context-free (builds the EMK resman cluster via
/// global RIP-relative stores + boot allocators; `this`/rsi is touched ONLY at prologue/tail). The
/// tail registers the stream worker when [this+0x48] >= 6. So a zeroed stub with [+0x48]=6 builds
/// the driver 0x143d7c088 + worker 0x144842d40, cold. Pure build -> read-back; no save write.
pub(crate) unsafe fn worldres_coldbuild_probe(base: usize) {
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
pub(crate) unsafe fn own_stepper_direct_build(owner: usize, base: usize) {
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
pub(crate) unsafe fn cold_char_mount_drive(base: usize, gm: usize, want_slot: i32, n: u64) {
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
/// MODEL B orchestrator (gated by live_dialog_enabled(), OFF by default). At the rendered title
/// menu: (1) do the wall-clock-bounded active-screen scan to acquire the live TitleTopDialog* +
/// MenuWindow*, (2) call the dialog factory 0x14081ead0(rcx=title_dialog+0xa38, rdx=menu_window)
/// ONCE -- which builds + registers the LIVE ProfileLoadDialog into the active-screen set, then (3)
/// wait for that ProfileLoadDialog (vtable 0x142b229f8) to appear in the active-screen array, latch
/// it as OWN_STEPPER_DIALOG, and hand it to STAGE2 ACTIVATE (which fires load_activate -> native pump
/// mount -> guarded, char-fingerprint-gated continue_confirm). One-shot fire latch; bounded wait.
/// FAIL-CLOSED at every step (no acquisition -> stay; bad vtable -> no call; dialog not live yet ->
/// wait then DONE on timeout). The forge path is untouched.
pub(crate) unsafe fn own_stepper_live_dialog_fire(
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
pub(crate) unsafe fn invoke_menu_item_functor(item: usize) -> Option<usize> {
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
pub(crate) unsafe fn drive_menu_item_update(
    item: usize,
    base: usize,
    framectx: usize,
) -> Option<usize> {
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
pub(crate) unsafe fn decorator_child_offset(update_fn: usize) -> Option<usize> {
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
pub(crate) unsafe fn diagnostic_job_tree_walk(
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
pub(crate) unsafe fn own_stepper_stage2(
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
