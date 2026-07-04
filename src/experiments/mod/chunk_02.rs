
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

// (The 0x1407b0cf0 "finished-poll" auto-accept hook was removed: RE showed 0x1407b0cf0 is a
// "has >= 2 buttons" layout query, not a finished-poll -- it is never called for the
// connection-error dialog, and writing +0x25e0/+0x25e8 corrupts the dialog (+0x25e8 is the
// button COUNT). The dismiss is force_dismiss_startup_dialog -> OnDecide 0x140927ba0.)

/// Pseudo-handle for the current process (GetCurrentProcess() is constant -1).
pub(crate) const CURRENT_PROCESS_PSEUDO_HANDLE: isize = -1;
/// Bytes read per ReadProcessMemory call when scanning a region for the title
/// vtable. One syscall per 64KB chunk (then an in-process buffer scan) keeps the
/// fault-tolerant scan fast -- a syscall per 8-byte cursor would stall the thread.
pub(crate) const SCAN_CHUNK_SIZE: usize = 0x10000;
