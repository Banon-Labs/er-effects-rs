use std::{
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Instant,
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
// The 2026-06-18 direct-build drive (`own_stepper_direct_build`) stood here: it constructed a
// CS::ProfileLoadDialog straight from dialog_factory 0x14081ead0 at the open menu, bypassing
// the input-gated router_this/d180-on-confirm layer, then handed off to stage 2. Its only
// caller was the `direct_build_enabled()` branch in
// product_core_own_stepper/fallback_drives.rs, and that gate has returned a literal `false`
// since it was written, so the build never ran. Deleted with the branch rather than left as an
// orphan that reads like a live dialog-construction path.
/// Multi-frame cold char-mount drive (gated, save-safe). Sequence (worker registered): build+register
/// the FD4 stream worker (0xb0a980 stub) so the scheduler ticks it and drains the save-IO read; set
/// the slot; Preview 0x67b4e0 (b80=1 + starts the iodev read); poll 0x679180 each frame until
/// GameMan+0xb80==3 (the make-or-break -- the registered+ticked worker draining the read); then
/// deserialize 0x67b290 (mounts GameMan+0xc30=real map + applies the char to PlayerGameData).
/// No SetState / no save write. dump_load_correctness verifies the mounted char.
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
    pub(crate) use er_telemetry_core::counters::MOUNT_WAITS;
    // Fire the warm FD4 worker-kick (0x67b4e0) at most once per process.
    pub(crate) use er_telemetry_core::counters::WARM_KICK_FIRED;
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if gm == null {
        return;
    }
    let read_i32 = |off: usize| unsafe { *((gm + off) as *const i32) };
    let iodev_summary = || -> (usize, usize, usize) {
        let iodev = er_game_base::mem::read_global_ptr(base, IODEV_GLOBAL_RVA, "IODEV_GLOBAL_RVA");
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
                "cold-char-mount: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-quickload-own-stepper.txt -- ABORT (no-write)"
            ));
            MOUNT_PHASE.store(PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // (-2) sign-in force (bd b80-ROOTCAUSE-cold-no-user-signin). The SaveLoad2 storage-select op
        // ctor (0x14240f1b0) builds its runnable only if the sign-in check returns true and the user
        // index is <= 3; cold (no signed-in user) both fail -> the op is null and the load FSM parks
        // at idx 0x16 (the b80 wall). Patch the two gate fns (deobf-verified live entries) so the
        // cold path loads as if signed in as user 0. Save-safe (in-memory code patch). Done here, in
        // PHASE_INIT, before the submit so the select op the load triggers sees the patched gates.
        apply_signin_force(base);
        // (-1.5) source probe (read-only) for a future controlled public-requestLoad (0x14240ac00):
        // the dead load builder reads source globals that may be invalid cold (it crashed). Before
        // ever calling requestLoad, log the candidate sources so we know a valid one: SLLoadContent
        // *0x143d87358, the main heap allocator *0x143d872e0, and owner+8 (what the dead builder
        // passed as the requestLoad source). Pure reads -- no calls into risky fns.
        //
        // 0x3d872e0 was named SLLOAD_SRC2_RVA here, which read as a save-load-specific "second
        // source". It is not: the 1.16.2 dump shows GLOBAL_MainHeapAllocator, 1821 xrefs across
        // CSTaskImp/CSWindowImp/CSEzWork, and GameMan::WriteSaveToSlot derefs it as
        // `GLOBAL_MainHeapAllocator->_vfptr->AllocateAligned`. Renamed 2026-08-01.
        const SLLOADCONTENT_SRC_RVA: usize = 0x3d87358;
        let src1 = unsafe {
            safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                SLLOADCONTENT_SRC_RVA,
                "SLLOADCONTENT_SRC_RVA",
            ))
        }
        .unwrap_or(null);
        let src2 = unsafe {
            safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                GLOBAL_MAIN_HEAP_ALLOCATOR_RVA,
                "GLOBAL_MAIN_HEAP_ALLOCATOR_RVA",
            ))
        }
        .unwrap_or(null);
        let owner_probe =
            er_game_base::mem::read_global_ptr(base, IODEV_GLOBAL_RVA, "IODEV_GLOBAL_RVA");
        let owner8 = if owner_probe != null {
            unsafe { safe_read_usize(owner_probe + 8) }.unwrap_or(null)
        } else {
            null
        };
        append_autoload_debug(format_args!(
            "cold-char-mount: SOURCE-PROBE SLLoadContent[*0x143d87358]=0x{src1:x} src2[*0x143d872e0]=0x{src2:x} owner=0x{owner_probe:x} owner8=0x{owner8:x} (non-null source needed for a safe public requestLoad 0x14240ac00)"
        ));
        // SLSYS-probe (read-only): is the SaveLoad2 SLSystemImpl + its session manager built cold? If
        // the session manager (sysimpl+0x8) is NULL, requestLoad derefs null -> that explains the
        // off-thread crash, and the narrow menu-free fix is to call SaveLoad2 initialize first (build
        // the manager) before any load. If it's already built+ready (sysimpl+0x19!=0), the crash is a
        // deeper threading issue and the synthetic path is a real dead end. *0x144852f88 = SLSystemImpl
        // ptr; +0x8 = SLSessionManager; +0x10 = device/result table; +0x19 = manager-ready flag.
        // This file had the identity right all along; `constants.rs` called the same address
        // an FD4 IO worker manager until 2026-08-01. Ghidra confirms this reading.
        const SLSYSTEMIMPL_PTR_RVA: usize = RuntimeGlobalRva::SaveLoad2SlSystemImpl as usize;
        let sysimpl = unsafe {
            safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                SLSYSTEMIMPL_PTR_RVA,
                "SLSYSTEMIMPL_PTR_RVA",
            ))
        }
        .unwrap_or(null);
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
        // (-1) Set the save-file path/name on the container so the device read returns slot N's real
        // .sl2 bytes. The native Continue handler runs this slot-mgr peek 0x140678a50 first (reads
        // [GameDataMan+0x8] container, sync-reads the save path token 0x47054, copies the name to
        // container+0x94, sets GameMan+0xe70=1) before the load. The prior cold attempt skipped it,
        // so the device read an empty buffer (deserialize gave c30=0xffffffff + garbage char).
        // Save-safe (sets a path + reads metadata; No save write).
        const SLOT_MGR_PEEK_RVA: usize = 0x678a50;
        let peek: unsafe extern "system" fn() = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(SLOT_MGR_PEEK_RVA, "SLOT_MGR_PEEK_RVA") {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        unsafe { peek() };
        append_autoload_debug(format_args!(
            "cold-char-mount: slot-mgr peek 0x{:x}() -> set save-file path before mount (GameMan+0xe70 ready)",
            er_game_base::mem::game_data_addr(base, SLOT_MGR_PEEK_RVA, "SLOT_MGR_PEEK_RVA")
        ));
        // (0) REFRAME (2026-06-18, REFRAME-io-subsystem-present-cold-blocker-is-just-the-active-byte):
        // the FD4 IO subsystem (pool/task/iodev) is already present + clean cold (snapshot-proven).
        // 0x67b200 fails cold only because its slot-check 0x140261cd0 reads [ProfileSummary+8+slot]==0
        // (the session/ProfileSummary is present). Set that byte directly via activate 0x140262250
        // (byte[profile+slot+8]=1) so 0x67b200 passes its slot-check and submits the read onto the
        // present subsystem. Save-safe (sets an in-memory flag; the deserialize only reads the .sl2).
        const SLOT_ACTIVE_BYTE_BASE: usize = 0x8;
        let game_data_man = game_data_man_ptr_or_null();
        let profile_summary = if game_data_man != null {
            unsafe { *((game_data_man + SLOT_MANAGER_CONTAINER_OFFSET) as *const usize) }
        } else {
            null
        };
        if profile_summary != null {
            let activate: unsafe extern "system" fn(usize, i32) = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        PROFILE_SLOT_ACTIVATE_RVA,
                        "PROFILE_SLOT_ACTIVATE_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
            unsafe { activate(profile_summary, want_slot) };
            let abyte = unsafe {
                *((profile_summary + SLOT_ACTIVE_BYTE_BASE + want_slot as usize) as *const u8)
            };
            append_autoload_debug(format_args!(
                "cold-char-mount: ACTIVATE 0x{:x}(profile=0x{profile_summary:x}, slot={want_slot}) -> [profile+8+{want_slot}]={abyte} (so 0x67b200 slot-check 0x140261cd0 passes)",
                er_game_base::mem::game_data_addr(
                    base,
                    PROFILE_SLOT_ACTIVATE_RVA,
                    "PROFILE_SLOT_ACTIVATE_RVA"
                )
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
        let worker_build: unsafe extern "system" fn(usize) -> usize = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    WORLD_WORKER_BUILD_RVA,
                    "WORLD_WORKER_BUILD_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        unsafe { worker_build(stub_ptr) };
        let worker = crate::runtime_heap_allocator_ptr_or_null();
        // (1.5) device MOUNT/BIND (b80-mount-routine-0x140e6e8d0-recipe-...). Root cause of
        // the cold full-read wall: the save IO device is UNMOUNTED cold -- [iodev+0x40]==0
        // (the device-ready flag the async router 0x140e6eb80 tests) and [iodev+0x30]==
        // 0xffffffff (no OS handle), so the full read takes the cold async branch that
        // completes empty (b80 2->0). The native title->Continue boot binds the device via
        // mount 0x140e6e8d0(iodev); the menu-free path skips it. Self-validating: log the
        // actual cold device state (we have never read +0x40/+0x30 at runtime -- the unbound
        // conclusion was static inference), call the native mount, log the post-state, then
        // submit. The mount is internally guarded by 0x14240acd0([0x143d872e0]) which needs
        // the IO worker registry [0x144843038+0x18]!=0; if it bails (al=0) the log shows it.
        // Save-SAFE: the mount only opens a handle + registers paths for read; no save write.
        let iodev_before =
            er_game_base::mem::read_global_ptr(base, IODEV_GLOBAL_RVA, "IODEV_GLOBAL_RVA");
        let registry = unsafe {
            *((er_game_base::mem::game_data_addr(
                base,
                IO_WORKER_REGISTRY_RVA,
                "IO_WORKER_REGISTRY_RVA",
            )) as *const usize)
        };
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
        let iodev_getter: unsafe extern "system" fn() -> usize = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(IODEV_GETTER_RVA, "IODEV_GETTER_RVA") {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let iodev = unsafe { iodev_getter() };
        let mount: unsafe extern "system" fn(usize) -> u8 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    IODEV_MOUNT_OPEN_RVA,
                    "IODEV_MOUNT_OPEN_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let mount_al = if iodev != null {
            unsafe { mount(iodev) }
        } else {
            0
        };
        let (dev40_after, dev30_after) = read_dev(iodev);
        append_autoload_debug(format_args!(
            "cold-char-mount: MOUNT 0x{:x}(iodev=0x{iodev:x}) al={mount_al} | registry=0x{registry:x} reg_count={reg_count} | dev40 {dev40_before}->{dev40_after} dev30 0x{dev30_before:x}->0x{dev30_after:x} (al=1 & dev40->nonzero = device bound; submit should now route to the BOUND read)",
            er_game_base::mem::game_data_addr(base, IODEV_MOUNT_OPEN_RVA, "IODEV_MOUNT_OPEN_RVA")
        ));
        // Worker-gate diagnostic (b80-device-mount-refuted-...). The read drops b80 2->0 in
        // one frame = the enqueue 0x14240e420 DISCARDS the request (no-op completion). Two
        // discard gates: (1) [worker+0x19]!=0 (no-accept/shutdown byte); (2) the registry
        // intrusive list [registry+0x28] does not contain the caller's key (0x141ee1240).
        // Read both (no call) to pin which gate fires cold. reg_list_empty when [[+0x28]]==[+0x28].
        let worker_mgr = er_game_base::mem::read_global_ptr(
            base,
            FD4_IO_WORKER_MGR_RVA,
            "FD4_IO_WORKER_MGR_RVA",
        );
        let worker_noaccept = if worker_mgr != null {
            unsafe { *((worker_mgr + FD4_IO_WORKER_NOACCEPT_19_OFFSET) as *const u8) }
        } else {
            0xff
        };
        let io_pool = unsafe {
            *((er_game_base::mem::game_data_addr(base, FD4_IO_POOL_RVA, "FD4_IO_POOL_RVA"))
                as *const usize)
        };
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
        // Worker queue snapshot before submit (b80-device-mount-refuted-...). Compared against the
        // after-submit snapshot below: if [worker+0x8]/[worker+0x10] change, the read was ENQUEUED
        // (so the wall is the worker not processing / read-fail); if unchanged, it was discarded at
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
        // the deref before/after disambiguates ENQUEUED (deref changes) from discarded (no change).
        let qd8_before = unsafe { safe_read_usize(q8_before) }.unwrap_or(null);
        let qd10_before = unsafe { safe_read_usize(q10_before) }.unwrap_or(null);
        // (1.75) save-directory -- pre-submit population is refuted (bd b80-cold-fix-refuted-pathdb-
        // transient-setter-wants-char16ptr-2026-06-21). The original plan was to call SETTER
        // 0x14240a2a0([iodev+0x20], 0, &dir) before submit so the request copy-ctor would inherit a
        // real directory. Runtime proof it cannot work: [iodev+0x20] is 0 before submit (it only
        // becomes the request handle io20 after submit). Static PROOF: the live opcode-0x17/0x18
        // handler 0x140e6ded0 calls the setter with rcx=[this+0x20] where `this` is a transient
        // per-request command object (the pump 0x140e6e080 bails when [this+0x20]==0), and the setter
        // wants a raw char16_t* in r8 (not a std::u16string). So the directory is filled on a
        // per-request object during its state-machine pump, not on a pokable global. The real fix
        // needs the request copy-ctor template source (request ctor 0x14240a850 forwards rdx to
        // copy-ctor 0x1424085b0 -- trace one frame up) or a post-submit, non-racy write to the live
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
        // (2) Resolve + set the slot, then submit the full save read (b80=2). The old
        // preview+LoadSaveData path drained but only left metadata resident, so 0x67b290 could
        // report success while c30 stayed at the default map and the strict world oracle caught a
        // false positive. The live native_fullread recipe also writes GameMan+0xb78 before
        // set_save_slot because resolver 0x1406793c0 reads that selector; direct-build previously
        // omitted it and reached b80==3 but deserialized the wrong/default buffer. Use the
        // runtime-pinned full-read initiator 0x67b1a0, then co-drive lane+poll in PHASE_POLL until
        // b80 reaches resident before deserializing.
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = want_slot };
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        let set_save_slot: unsafe extern "system" fn(i32) = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
                    "FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        unsafe { set_save_slot(want_slot) };
        let submit: unsafe extern "system" fn(i32) -> i32 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    B80_FULL_LOAD_INITIATOR_RVA,
                    "B80_FULL_LOAD_INITIATOR_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        // Not `submit(want_slot)`: the argument is a flag the game always passes as 0, and the
        // slot was already set by `set_save_slot` above. See `B80_FULL_LOAD_SUBMIT_FLAG`.
        let sret = unsafe { submit(B80_FULL_LOAD_SUBMIT_FLAG) };
        let (io10, io18, io20) = iodev_summary();
        let q8_after = read_q(FD4_IO_WORKER_QUEUE_08_OFFSET);
        let q10_after = read_q(FD4_IO_WORKER_QUEUE_10_OFFSET);
        let qd8_after = unsafe { safe_read_usize(q8_after) }.unwrap_or(null);
        let qd10_after = unsafe { safe_read_usize(q10_after) }.unwrap_or(null);
        append_autoload_debug(format_args!(
            "cold-char-mount: FULL-INIT slot={want_slot} b78={b78} worker=0x{worker:x} submit_ret={sret} b80={} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} | q8 0x{q8_before:x}->0x{q8_after:x} [q8] 0x{qd8_before:x}->0x{qd8_after:x} q10 0x{q10_before:x}->0x{q10_after:x} [q10] 0x{qd10_before:x}->0x{qd10_after:x} (any change=ENQUEUED; none=DISCARDED) -> POLL",
            read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET)
        ));
        // (2.4) save-DIR read-only verify (bd b80-cold-exact-dir-field-slot3-0x142410c60). The worker
        // (SLLoadSession::_Func02 0x142410cd0) -> name-builder FUN_14240d5b0 -> slot-3 0x142410c60
        // reads the dir std::u16string from [SLLoadSession+0xe0] == io18, at io18+0xe8 (data/SSO),
        // size io18+0xf8, cap io18+0x100 (cap>=8 => data is a heap ptr at io18+0xe8, else SSO inline).
        // Empty cold => slot-3 returns empty => builder ret 0 => _Func02 code 8 => no open. Confirm the
        // field+emptiness here (pure reads) before any write into this transient request object.
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
        // (2.5) save-directory post-submit install (bd savedir-CONFIG-lever-setter-0x14240a2a0-...).
        // The cold full read completes empty because the path-DB's slot-0 directory std::u16string
        // is unset, so the worker formats a bare `.sl2` that fails to open. The live Continue boot
        // fills it via the opcode-0x17/0x18 pump handler 0x140e6ded0; the menu-free cold path never
        // dispatches that opcode, so we replay its two native steps here -- on the live io20
        // (=[iodev+0x20], which only exists after submit) in this same task invocation, the tightest
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
        // The builder assumes a pre-constructed empty string, so install the arena allocator at +0
        // and cap=7 (empty SSO) first. [u64;8] guarantees 8-byte alignment for the field writes.
        let mut wrapper = [0u64; 8];
        let wbase = wrapper.as_mut_ptr() as usize;
        let alloc_getter: unsafe extern "system" fn() -> usize = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    SAVE_DIR_ALLOC_GETTER_RVA,
                    "SAVE_DIR_ALLOC_GETTER_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let allocator = unsafe { alloc_getter() };
        unsafe {
            *((wbase + U16STRING_ALLOC_OFFSET) as *mut usize) = allocator;
            *((wbase + U16STRING_CAP_OFFSET) as *mut usize) = U16STRING_SSO_CAP;
        }
        // Guard: two frames down the builder calls through the qword at 0x143b48ff0 (0x140e8d550
        // -> 0x140e8d510 -> `MOV RAX,[0x143b48ff0]; CALL RAX`), so a null there is `CALL 0`. Skip
        // the call and log the cause -- that would be hypothesis-2 (Steam not live).
        let steam_id_call_slot = unsafe {
            safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                STEAM_ID_ACCESSOR_CALL_SLOT_RVA,
                "STEAM_ID_ACCESSOR_CALL_SLOT_RVA",
            ))
        }
        .unwrap_or(null);
        if steam_id_call_slot != null && allocator != null {
            let builder: unsafe extern "system" fn(usize) = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        SAVE_DIR_BUILDER_RVA,
                        "SAVE_DIR_BUILDER_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
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
            "cold-char-mount: SAVE-DIR BUILD steam_id_call_slot=0x{steam_id_call_slot:x} allocator=0x{allocator:x} cap={dir_cap} size={dir_size} data=0x{dir_data:x} text=\"{built_text}\" (size>0 & real path = builder works cold = hypothesis-1 handler-never-ran; size=0 = Steam not live cold = hypothesis-2)"
        ));
        // Install on the live path-DB slot-0 directory. The setter copies our buffer into the slot
        // entry's std::u16string at entry+0xb0 (via 0x14240dce0), so our stack wrapper can be dropped.
        let setter: unsafe extern "system" fn(usize, i32, usize) = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(SAVE_DIR_SETTER_RVA, "SAVE_DIR_SETTER_RVA")
                {
                    Some(address) => address,
                    None => return,
                },
            )
        };
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
            let lookup: unsafe extern "system" fn(usize, i32) -> usize = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        SAVE_DIR_SLOT_LOOKUP_RVA,
                        "SAVE_DIR_SLOT_LOOKUP_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
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
        // Owner-FSM gate measurement (bd b80-owner-FSM-lifecycle-gates-2026-06-21). Runtime data
        // refuted the static "empty registry / null early-out" story: reg_count=16 (non-empty) and
        // io18/io20 (=owner+0x18/+0x20) persist non-null, so the poll's early-out is not the wall.
        // The real bounce is inside the native FSM tick setter 0x140679180: with df0==0 it polls the
        // owner FSM 0x140e6e080(owner); Only state-index 0x14 returns 0 (-> b80=3), any index>=2 (18
        // ->3, 0x19->2+teardown, 0x19... ) resets b80=0. The index comes from the pure getter
        // 0x14240a1f0([owner+0x20]): returns 0x19 when the handle's container ([o20]) is null, else a
        // real node index; 0x14 only when idle-ready (container built, current-node null, deep gate 0).
        // Read the handle internals + index here while b80 is still 2, to pin the exact failing gate
        // before building any fix. All reads are fault-safe; the getter is a read-only status query.
        const STATE_INDEX_GETTER_RVA: usize = 0x240a1f0;
        const OWNER_HANDLE_CONTAINER_OFFSET: usize = 0x0;
        const OWNER_HANDLE_H10_OFFSET: usize = 0x10;
        // Not the owner FSM's field and not a handle: `GameMan + 0xdf0` is the length of the
        // `DLString<wchar_t>` inside the `FD4FilePathBase` at `GameMan + 0xdd0` (Ghidra's 1.16.2
        // `GameMan` type; the ctor's `lea rdi,[rsi+0xdd0]` at 0x14067644b / 1.17 0x14067729b).
        // `0x140679180` spells its own gate `(GLOBAL_GameMan->field479_0xdd0).string.length != 0`,
        // so "df0!=0 = warm fast-path" below means "a save path is already set". A count, printed
        // in decimal -- rendered hex beside `owner=0x..`/`o20=0x..` it reads as an address.
        const GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET: usize = 0xdf0;
        let owner_fsm =
            er_game_base::mem::read_global_ptr(base, IODEV_GLOBAL_RVA, "IODEV_GLOBAL_RVA");
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
            let idx_getter: unsafe extern "system" fn(usize) -> i32 = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        STATE_INDEX_GETTER_RVA,
                        "STATE_INDEX_GETTER_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
            unsafe { idx_getter(io20) }
        } else {
            -1
        };
        let path_len =
            unsafe { *((gm + GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET) as *const usize) };
        let b80_at_init = read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET);
        append_autoload_debug(format_args!(
            "cold-char-mount: OWNER-FSM owner=0x{owner_fsm:x} o18=0x{io18:x} o20=0x{io20:x} container=[o20]=0x{container:x} h10=[o20+0x10]=0x{h10:x} h10_deep=[h10+0x10]=0x{h10_deep:x} fsm_index=0x{fsm_index:x} path_len[gm+0xdf0]={path_len} b80={b80_at_init} (idx 0x14=idle->b80=3; 0x19=container-null; path_len!=0=save path set=warm fast-path)"
        ));
        MOUNT_WAITS.store(TITLE_OWNER_SCAN_START_ADDRESS, Ordering::SeqCst);
        MOUNT_PHASE.store(PHASE_POLL, Ordering::SeqCst);
        return;
    }
    if phase == PHASE_LANE {
        // While b80==1, tick the b80==1 lane driver 0x679510 (IO tick) to drive the preview read to
        // resident. It keeps b80=1 while in-progress and resets b80=0 once the read completes (the
        // registered+ticked worker is what makes that completion happen). When b80==0, the iodev
        // request is resident; fire LoadSaveData 0x67b200 to re-enter the b80=2 lane (populates io18).
        let lane: unsafe extern "system" fn() -> i32 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    B80_LANE1_DRIVER_RVA,
                    "B80_LANE1_DRIVER_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let _ = unsafe { lane() };
        let b80 = read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET);
        let w = MOUNT_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst);
        if w % LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS {
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: LANE waits={w} b80={b80} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x}"
            ));
        }
        if b80 == B80_IDLE {
            let loadsave: unsafe extern "system" fn(i32) -> i32 = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        B80_LOAD_SAVE_DATA_INITIATOR_RVA,
                        "B80_LOAD_SAVE_DATA_INITIATOR_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
            let lret = unsafe { loadsave(want_slot) };
            let (io10, io18, io20) = iodev_summary();
            append_autoload_debug(format_args!(
                "cold-char-mount: preview read RESIDENT (b80->0 after {w} lane ticks) -> LoadSaveData 0x67b200 ret={lret} b80={} io10=0x{io10:x} io18=0x{io18:x} io20=0x{io20:x} -> POLL",
                read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET)
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
        // NOTE (b80-fullread-correction-...): a lane-skip A/B run FALSIFIED the "lane 0x679510
        // prematurely completes the read" hypothesis -- with lane() removed, b80 was already 0 at
        // poll waits=0 (it drops 2->0 in the native frame right after submit, before cold_char_mount
        // ticks anything). So the recipe-aligned lane+poll drain is restored; the real wall is that
        // the cold async full read completes empty (b80->0, never resident=3) -- the worker is
        // registered+scheduler-ticked but does no actual 0x280000 disk IO. Next suspect: the df0
        // fast-path ([mgr+0xdf0]!=0 -> 0x67b100 skips the read).
        let lane: unsafe extern "system" fn() -> i32 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    B80_LANE1_DRIVER_RVA,
                    "B80_LANE1_DRIVER_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let _ = unsafe { lane() };
        let poll: unsafe extern "system" fn(u8, u8) -> i32 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(B80_POLL_RVA, "B80_POLL_RVA") {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let _ = unsafe { poll(POLL_ARG, POLL_ARG) };
        let b80 = read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET);
        let w = MOUNT_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst);
        // Warm worker-kick (bd b80-warm-kick-0x14067b4e0-worker-0x140e6ec80). The cold submit
        // 0x67b1a0 only request_transitions state 0xa, so the owner-FSM node parks at idx 0x16 (an
        // async device-read node) and nothing pumps it: the node advances only via the FD4 worker
        // that the warm Continue step (0x14082ba30) builds by calling 0x67b4e0(cl=0). That kick mints
        // a handle (0x141ed5fe0), captures it to GameMan+0xb98/0xba0, then 0x140e6ec80 subscribes the
        // node-advance callback to events 0x7..0x12 and submits the real save-read as an FD4 job-pool
        // job (engine-wide, not menu-gated). On the menu-free cold path that kick never runs. Fire it
        // once here -- b80 has bounced to 0, satisfying 0x67b4e0's b80==0 guard -- to pump the parked
        // node to completion. Save-SAFE: it submits a read job; no save write. The single warm caller
        // passes cl=0 (xor ecx,ecx at 0x14082ba39).
        if b80 == B80_IDLE
            && WARM_KICK_FIRED.swap(WAIT_INC, Ordering::SeqCst) == TITLE_OWNER_SCAN_START_ADDRESS
        {
            const NODE_FINALIZER_RVA: usize = er_game_base::rva::SL_RELEASE_REQUEST_RVA;
            // Not a "warm load kick": 0x67b4e0 blanks the whole save container. See
            // BLANK_SAVE_CONTAINER_REQUEST_RVA. Only referenced below to suppress an unused warning.
            const WARM_LOAD_KICK_RVA: usize = BLANK_SAVE_CONTAINER_REQUEST_RVA;
            // Not a load handle pair. `GameMan + 0xb98` is a `DLDateTime` and 0xba0 is its
            // upper half; a second `DLDateTime` follows at 0xba8. The constructor writes both
            // halves through a register, not through `this` --
            //   1406761a3  lea   rbx, [rsi+0xb98]     ; 1.17 0x140676ff3
            //   1406761aa  mov   qword [rbx], r14     ; GameMan+0xb98
            //   1406761ad  and   qword [rbx+8], r12   ; GameMan+0xba0
            //   1406761d7  lea   rbx, [rsi+0xba8]     ; the second DLDateTime
            // -- so a `this`-relative census sees 0xb98 and 0xba8 and is silent at 0xba0, which
            // is how a wrong meaning survives a drift check. Ghidra's 1.16.2 `GameMan` type names
            // both members `DLDateTime`, and the ctor pairs 1296/1296 with zero moved offsets.
            // Nothing here reads them; they are kept as the measured RE fact.
            const GAME_MAN_DLDATETIME_B98_OFFSET: usize = 0xb98;
            const GAME_MAN_DLDATETIME_B98_UPPER_BA0_OFFSET: usize = 0xba0;
            // Runtime-proven cold gate (bd b80-warm-kick-runtime-0x140e6ec80-returns0-cold): the
            // worker-builder 0x140e6ec80 (inside the kick) returns al=0 unless both [owner+0x10]==0
            // (worker) and [owner+0x20]==0 (node) -- it only builds when nothing exists yet. In the
            // warm path the worker is built before the node; our cold flow built the parked node
            // first (owner+0x20 = io20, non-null), so the kick bailed (ret=0, no FD4 job). Clear the
            // parked node via the finalizer 0x140e6f200 (zeroes owner+0x10/+0x18/+0x20 -- the same
            // teardown the idx-0x14 success path runs) so the kick rebuilds worker+node cleanly and
            // submits the real FD4 read job. owner = iodev = *0x144589390.
            let owner =
                er_game_base::mem::read_global_ptr(base, IODEV_GLOBAL_RVA, "IODEV_GLOBAL_RVA");
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
                let finalizer: unsafe extern "system" fn(usize) = unsafe {
                    std::mem::transmute(
                        match crate::experiments::gated_game_fn(
                            NODE_FINALIZER_RVA,
                            "NODE_FINALIZER_RVA",
                        ) {
                            Some(address) => address,
                            None => return,
                        },
                    )
                };
                unsafe { finalizer(owner) };
            }
            let o20_post = if owner != null {
                unsafe { safe_read_usize(owner + IODEV_REQHANDLE_20_OFFSET) }.unwrap_or(null)
            } else {
                null
            };
            let _ = (
                WARM_LOAD_KICK_RVA,
                GAME_MAN_DLDATETIME_B98_OFFSET,
                GAME_MAN_DLDATETIME_B98_UPPER_BA0_OFFSET,
                o10_pre,
                o20_pre,
                o20_post,
            );
            // Proper-load (off-thread). Calling the load builder (deobf entry 0x140e6da42) inline on
            // the game task hung it -- requestLoad (0x14240ac00) blocks on async machinery (FD4 job
            // pool / session-manager tick) that needs the game task to keep pumping; blocking the game
            // task in requestLoad deadlocks it. Fix: run the load builder on a separate thread so the
            // game task stays free to pump the async read to completion. Also safer than inline: a hang
            // on this thread doesn't freeze the game (teardown cleans it). Preconditions: finalize
            // (above, game thread) cleared owner+0x10/0x18/0x20; signin forced; source validated
            // non-null. Save-SAFE: requestLoad is a read. Watch owner+0x20 / b80 in the poll below.
            // Proper-load disabled -- Dead end confirmed (3 attempts, all save-safe): the SaveLoad2
            // load builder (deobf 0x140e6da42) is uncallable in the cold menu-free context. Inline on
            // the game task hangs (requestLoad deadlocks); on a separate thread it crashes
            // (process_exited). Wrong dump addr 0x140e6da37 crashed (misaligned). Sources were
            // validated non-null, so this is a fundamental boot/session/threading-context mismatch, not
            // a bad arg. The dead requestLoad path needs the engine's full boot+session-manager+worker
            // context that the menu-free path lacks. The realistic drive is to let the engine's boot/
            // session machinery run the load (input-blocked), not synthetic primitive calls -- a major
            // redesign that revisits the menu-Continue/save-write-risk constraint. finalize kept
            // (harmless). See bd b80-load-builder-hangs-inline-async-needed + the off-thread crash.
            append_autoload_debug(format_args!(
                "cold-char-mount: PROPER-LOAD disabled (load builder uncallable cold: inline hangs, off-thread crashes) -- finalize 0x{:x}(owner=0x{owner:x}) done, no load call",
                er_game_base::mem::game_data_addr(base, NODE_FINALIZER_RVA, "NODE_FINALIZER_RVA")
            ));
        }
        // (select-node pump removed with the PIVOT: it was for the low-level select-node hypothesis
        // and dereferenced owner+0x20 as a select container; owner+0x20 is now a proper requestLoad
        // handle, so that deref/advance is wrong and unsafe. The proper requestLoad's SLLoadSession is
        // driven autonomously by the SaveLoad2 session manager + FD4 job pool, like the warm path.)
        if w % LOG_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS {
            let (io10, io18, io20) = iodev_summary();
            // Pure-read trajectory telemetry across poll frames (no function calls -- io20 is now a
            // requestLoad handle of unknown internal type, so we only safe-read raw fields): the
            // handle's [o20+0] and [[o20+0x10]+0x10]. Combined with b80 + the char fingerprint below,
            // this shows whether the proper requestLoad drives the load to resident.
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
        // Diagnostic (char-apply debug, cold-B80-wall-broken-...): before the deserialize, read the
        // suspects for why c30/char did not apply: the save-path length at GameMan+0xdf0 (if
        // non-zero, `0x67b100` takes the fast-path and does not read into 0x67b290's buffer = lane
        // mismatch / empty parse); [mgr+0x18] (the async load job 0x140e6eb80 queued);
        // [0x143d68078] (the c30-write gate that gates 0x67bd70 inside 0x67b290).
        //
        // It was `DF0_OFFSET`, described as "deserialize-ready", logged as `0x{:x}`. It is the
        // length of the `DLString<wchar_t>` in the `FD4FilePathBase` at GameMan+0xdd0 -- both
        // gates that read it, `0x140679180` and `0x14067b100`, decompile to
        // `(GLOBAL_GameMan->field479_0xdd0).string.length != 0`. A character count, so: decimal.
        const GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET: usize = 0xdf0;
        const ASYNC_JOB_18_OFFSET: usize = 0x18;
        const C30_WRITE_GATE_RVA: usize = er_game_base::rva::SAVE_DATA_SUBSYSTEM_GATE_RVA;
        let path_len =
            unsafe { *((gm + GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET) as *const usize) };
        let job18 = unsafe { *((gm + ASYNC_JOB_18_OFFSET) as *const usize) };
        let c30_gate =
            er_game_base::mem::read_global_ptr(base, C30_WRITE_GATE_RVA, "C30_WRITE_GATE_RVA");
        let deser: unsafe extern "system" fn(i32) -> i32 = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    DESERIALIZE_SLOT_RVA,
                    "DESERIALIZE_SLOT_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let dret = unsafe { deser(want_slot) };
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        append_autoload_debug(format_args!(
            "cold-char-mount: DESERIALIZE slot={want_slot} ret={dret} c30=0x{c30:x} ac0={ac0} | pre-deser path_len(gm+0xdf0)={path_len} async_job(mgr+0x18)=0x{job18:x} c30_gate(0x143d68078)=0x{c30_gate:x} (path_len!=0 -> 0x67b100 fast-path skips the read = empty parse). NO SetState/NO save write:"
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
    }
}
