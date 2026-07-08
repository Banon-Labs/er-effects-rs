/// Crash-on-not-loaded watchdog (privacy-policy-gated-on-character-presence-CONFIRMED-2026-06-23):
/// the Bandai-Namco privacy policy / new-game state shows ONLY when the active profile has no
/// character (profile_slot_active == 0). When a load is expected (not telemetry-only) and the profile
/// summary has been present but reports ZERO active slots for a settle window, the selected save did
/// NOT load -> abort instantly so the failure is loud + fast (no stall on the policy).
/// profile_slot_active != 0 is the single "save loaded" semaphore (explicit redirect/default save read
/// AND char present AND policy never builds).
pub(crate) unsafe fn save_load_watchdog() {
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
            "save-override: WATCHDOG ABORT -- profile summary reports ZERO active slots after {n} frames; the selected save did NOT load (no character -> privacy policy / new-game). Aborting."
        ));
        eprintln!(
            "er-effects: WATCHDOG ABORT -- selected save not loaded (no character in active profile); aborting."
        );
        std::process::abort();
    }
}
/// Resolve the full-read target slot: a configured OWN_STEPPER_SLOT (>=0, from the trigger-file
/// "slot=N"), else DLL config/env autoload slot (>=0), else FULLREAD_DEFAULT_SLOT (Banon = 0).
pub(crate) fn native_fullread_slot() -> i32 {
    // Missing-save picker: the user explicitly chose this slot; it wins over any configured default.
    if let Some(slot) = missing_save_picker_selected_slot() {
        return slot;
    }
    let configured = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    if configured >= OWN_STEPPER_SLOT_ZERO {
        return configured;
    }
    if let Some(slot) = configured_autoload_slot()
        && slot >= OWN_STEPPER_SLOT_ZERO
    {
        return slot;
    }
    FULLREAD_DEFAULT_SLOT
}
/// Terminal non-commit disarm for the full-read chain (bd er-effects-rs-ns4n). SUBMIT arms the
/// native slot-request register (GameMan+0xb78, `requested_save_slot_load_index`) so the native
/// chain resolves our slot. On every DONE exit that does NOT hand off to the native confirm chain
/// (continue_confirm consumes the pending request as part of its own load), the register must be
/// returned to the no-request sentinel: the in-game save manager services any >=0 request on the
/// first frames after world arrival, which runs a SECOND full deserialize into the already-live
/// world and exhausts the CSGaitemImp free queue -- the gaitemInsTable[-1] AV at live 0x67141a
/// (6/6 picker-boot crashes 2026-07-07, ~22s in, at world arrival).
unsafe fn fullread_disarm_slot_request(gm: usize, reason: &str) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    if gm == NULL {
        return;
    }
    let prev = unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *const i32) };
    if prev == OWN_STEPPER_SLOT_NONE {
        return;
    }
    unsafe {
        *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = OWN_STEPPER_SLOT_NONE;
    }
    FULLREAD_REQ_DISARM_COUNT.fetch_add(1, Ordering::SeqCst);
    FULLREAD_REQ_DISARM_LAST_PREV_SLOT.store(prev as u32 as usize, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native-fullread: DISARM req_slot {prev} -> {OWN_STEPPER_SLOT_NONE} ({reason}) -- no pending native load request may survive a non-commit exit"
    ));
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
pub(crate) unsafe fn native_fullread_tick(owner: usize, base: usize, n: u64) {
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
    // The Load-Game action-node scan is a readiness GATE (and log provenance) only -- the load chain
    // below uses slot/gm/base, never the node. For the missing-save picker the product tick has
    // already confirmed the live menu is open and the IO pool is up, so skip the scan there: it can be
    // over-strict and would otherwise stall on a menu with no separate Load-Game node.
    let missing_save = missing_save_picker_selected_slot().is_some();
    let action = unsafe { title_menu_action_ready(owner, base) };
    if action.is_none() && !missing_save {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for semantic Load-Game action readiness (#{n}) gm=0x{gm:x} -- TitleTopDialog/registry/node/action not all validated yet"
            ));
        }
        return;
    }
    if gm == NULL {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            let (node, registry) = action.as_ref().map_or((NULL, NULL), |a| (a.node, a.registry));
            append_autoload_debug(format_args!(
                "native-fullread: waiting for GameMan after menu ready node=0x{node:x} registry=0x{registry:x} (#{n})"
            ));
        }
        return;
    }
    let slot = native_fullread_slot();
    let read_i32 = |off: usize| unsafe { *((gm + off) as *const i32) };

    if phase == FULLREAD_PHASE_SUBMIT {
        // Step 0: mark the target slot occupied so the native save-load gate (0x14067b200, which reads
        // ProfileSummary->saveSlotsStates[slot]) accepts it. At a missing-save boot the boot save-check
        // has not populated ProfileSummary, so saveSlotsStates[slot]==0 and the load is refused. The
        // full-read below reads the character data itself, but the gate still needs the occupancy flag.
        // MarkProfileIndexAsUsed 0x262250(profileSummary, slot) sets it with no other side effect;
        // idempotent. Skip if ProfileSummary is not resolvable yet.
        let gdm_for_mark = game_data_man_ptr_or_null();
        let summary = if gdm_for_mark != NULL {
            unsafe { safe_read_usize(gdm_for_mark + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(NULL)
        } else {
            NULL
        };
        if summary != NULL {
            let mark: unsafe extern "system" fn(usize, i32) -> u8 =
                unsafe { std::mem::transmute(base + PROFILE_MARK_SLOT_USED_RVA) };
            let _ = unsafe { mark(summary, slot) };
        }
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
            unsafe { fullread_disarm_slot_request(gm, "drain-timeout") };
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
        // Missing-save picker: the picker already validated the chosen character (non-empty name &&
        // level>=1), so any real level is acceptable. The >=10 default is only a heuristic for the
        // diagnostic path where nothing pre-validated the slot; c30_real + fp_real still block a
        // new-game/null commit either way.
        let min_level = if missing_save_picker_selected_slot().is_some() {
            1
        } else {
            FULLREAD_MIN_REAL_LEVEL
        };
        let level_real = level >= min_level;
        let guard_pass = c30_real && fp_real && level_real;
        let commit = native_fullread_commit_enabled();
        append_autoload_debug(format_args!(
            "native-fullread: GUARD c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={level} level_real={level_real} name_len={name_len} -> guard_pass={guard_pass} commit_gate={commit}"
        ));
        if !guard_pass {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD FAIL (c30=0x{c30:x} level={level}) -- NO continue_confirm, NO SetState5, NO save write -> DONE (save-safe)"
            ));
            unsafe { fullread_disarm_slot_request(gm, "guard-fail") };
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // Step 7 is HARD-gated behind BOTH the guard above AND the commit sub-gate (default off):
        // VERIFY-ONLY by default -- stop here (log only, NO continue_confirm/NO SetState5).
        if !commit {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD PASS (c30=0x{c30:x} level={level}) but VERIFY-ONLY (commit sub-gate OFF) -- NO continue_confirm, NO SetState5 -> DONE (save-safe). Set ER_EFFECTS_FULLREAD_COMMIT=1 / er-effects-fullread-commit.txt to commit."
            ));
            unsafe { fullread_disarm_slot_request(gm, "verify-only") };
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
            unsafe { fullread_disarm_slot_request(gm, "commit-abort-owner-null") };
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        let new_game_flag =
            unsafe { *((owner_obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *const u8) };
        if new_game_flag != FULLREAD_OWNER_NEW_GAME_OK {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- owner+0x284={new_game_flag} != 0 (continue_confirm requires the new-game flag clear) -> DONE (no write)"
            ));
            unsafe { fullread_disarm_slot_request(gm, "commit-abort-new-game-flag") };
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
pub(crate) unsafe fn profile_slot_fingerprint(slot: i32) -> (bool, i32, u32, usize) {
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
/// The save slot to auto-load: the ACTIVE slot holding the most-progressed real character (highest level;
/// lowest index on a tie). "Active/real" is judged by the RECORD-based `profile_slot_fingerprint`
/// (level>=1 && non-empty name) -- NOT the `profile_summary+0x8` active byte, which the DLL writes itself
/// (PROFILE_SLOT_ACTIVATE / seed) and so reads all-active even for a NULL slot. Returns
/// `OWN_STEPPER_SLOT_NONE` (-1) when NO slot holds a real character (or the profile summary is not yet
/// populated); callers MUST refuse to load on the sentinel -- never load a null slot (which spawns the
/// new-game intro cutscene + a null character).
pub(crate) unsafe fn best_active_slot() -> i32 {
    let mut best_slot = OWN_STEPPER_SLOT_NONE;
    let mut best_level: u32 = 0;
    let mut slot: i32 = OWN_STEPPER_SLOT_ZERO;
    while (slot as usize) < TITLE_PROFILE_SLOT_COUNT {
        let (is_real, _map, level, _name_len) = unsafe { profile_slot_fingerprint(slot) };
        if is_real && level > best_level {
            best_level = level;
            best_slot = slot;
        }
        slot += 1;
    }
    best_slot
}
/// Resolve the slot to actually load under the user's guards: honor a configured slot ONLY if it holds a
/// real character; otherwise fall back to `best_active_slot()` ("whatever is indicated as an active slot on
/// disk"). Returns `OWN_STEPPER_SLOT_NONE` when nothing is loadable so the caller refuses to load.
pub(crate) unsafe fn resolve_active_load_slot(configured: i32) -> i32 {
    if configured >= OWN_STEPPER_SLOT_ZERO && unsafe { profile_slot_fingerprint(configured).0 } {
        return configured;
    }
    unsafe { best_active_slot() }
}
pub(crate) unsafe fn requested_slot_identity(slot: i32, c30: i32) -> RequestedSlotIdentity {
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
pub(crate) unsafe fn char_fingerprint(base: usize) -> (bool, u32, usize) {
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
    // Peak-load latch gate: a genuinely loaded character has level>=1 and a non-empty name.
    const MIN_REAL_LATCH_LEVEL: usize = 1;
    const NAME_LEN_EMPTY: usize = 0;
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
    // LATCH the peak-load semaphore: a REAL character (present PlayerGameData, level>=1, non-empty
    // name) confirmed in the world. Latched so a later quit-to-title -- which tears the char down and
    // resets the live oracle_char_* fields -- cannot erase the proof that the load succeeded this run.
    // Peak = highest level seen (keeps the identifying fields for that character).
    if (level as usize) >= MIN_REAL_LATCH_LEVEL && nlen > NAME_LEN_EMPTY {
        LOADED_PEAK_SEEN_COUNT.fetch_add(1, Ordering::SeqCst);
        if (level as usize) >= LOADED_PEAK_LEVEL.load(Ordering::SeqCst) {
            LOADED_PEAK_LEVEL.store(level as usize, Ordering::SeqCst);
            LOADED_PEAK_C30.store(c30, Ordering::SeqCst);
            LOADED_PEAK_NAME_LEN.store(nlen, Ordering::SeqCst);
            if let Ok(mut latched) = LOADED_PEAK_NAME.lock() {
                latched.clear();
                latched.push_str(&name);
            }
        }
    }
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
