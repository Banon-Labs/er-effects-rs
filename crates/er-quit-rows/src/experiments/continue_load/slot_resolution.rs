use super::*;

/// Crash-on-not-loaded watchdog (privacy-policy-gated-on-character-presence-confirmed-2026-06-23):
/// the Bandai-Namco privacy policy / new-game state shows only when the active profile has no
/// character (profile_slot_active == 0). When a load is expected (not telemetry-only) and the profile
/// summary has been present but reports zero active slots for a settle window, the selected save did
/// not load -> abort instantly so the failure is loud + fast (no stall on the policy).
/// profile_slot_active != 0 is the single "save loaded" semaphore (explicit redirect/default save read
/// and char present and policy never builds).
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
            "er-quickload: WATCHDOG ABORT -- selected save not loaded (no character in active profile); aborting."
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
/// Terminal non-commit disarm for the full-read chain (bd er-effects-rs-ns4n). Submit arms the
/// native slot-request register (GameMan+0xb78, `requested_save_slot_load_index`) so the native
/// chain resolves our slot. On every done exit, including the commit handoff, the register must be
/// returned to the no-request sentinel: the in-game save manager services any >=0 request on the
/// first frames after world arrival, which runs a second full deserialize into the already-live
/// world and exhausts the CSGaitemImp free queue -- the gaitemInsTable[-1] AV at live 0x67141a
/// (6/6 picker-boot crashes 2026-07-07; explicit save_file repro 2026-07-08, ~25s in, immediately
/// after save_state 1->2). Earlier code assumed continue_confirm consumed the pending request, but
/// runtime gm-snap proved req_slot survived as 0 until the crash, so commit must disarm too.
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
/// Observe-only native full-save-read tick, reached through the er-title-flow seam. Runs
/// each frame instead of the own_stepper forcing logic (no SetState forcing for boot); the caller
/// pass-throughs to OWN_STEPPER_ORIG_IDX10 so the native title machine advances untouched. Once the
/// live TitleTopDialog menu action is semantically validated (TitleTopDialog vtable,
/// [dialog+0xa48] registry, Load-Game node/action chain),
/// it runs the full-save-read load chain as a per-frame phase
/// machine at the live menu (where the FD4 IO worker pool 0x144853048 is live so the submit drains):
///   SUBMIT: set GameMan+0xb78=slot (step 1, new), set_save_slot 0x14067a810 (step 2 -> GameMan+0xac0),
///           submit full read 0x14067b1a0 (step 3, type-0xa).
///   DRAIN:  tick lane 0x140679510 + poll 0x140679180 each frame until GameMan+0xb80==3 (step 4).
///   DESER:  deserialize 0x14067b290(slot) once at b80==3 (step 5 -> GameMan+0xc30 = real map).
///   GUARD:  c30 != 0xa010000 (m10 default) and char fingerprint present (level>=10 + name) (step 6).
///   Confirm (step 7, the sole save write): Only if the guard passes and native_fullread_commit_enabled():
///           continue_confirm 0x140b0e180(rcx=shim{[owner]=live_title_owner});
///           it takes the non-NewGame branch when owner+0x284!=1, sets owner+0xbc=c30 + SetState5
///           (AUTOSAVES). Without the
///           commit sub-gate, stops at guard (verify-ONLY: log only, no continue_confirm/NO SetState5).
/// Reuses cold_char_mount_drive's submit/lane/poll/deser calls (exact RVAs) but builds/pumps no
/// selector step (probe-12 crash) and forces no SetState for boot. Logs b80/c30/level each frame.
/// Record that the title-time save deserialize `0x14067b290` is about to be called.
///
/// `0x14067b290` has exactly one caller in the image -- `CS::MoveMapStep::DoSaveStuff`, reachable
/// only from `MoveMapStep::Update`, the in-world step. Calling it from the boot title is calling it
/// outside every precondition that caller establishes, and a picked save died there. The routing
/// fix (`er_title_flow::autoload_route`) sends a picked save down the native Continue row instead,
/// so this must never fire on a correct run; the counter exists so a regression is loud in
/// `oracle_title_time_deser_calls` rather than silent until the next crash.
///
/// It is bumped before the call, so a run that dies inside the deserialize still leaves the count.
fn note_title_time_deser(slot: i32, reason: &str) {
    let total =
        er_telemetry_core::counters::TITLE_TIME_DESER_CALLS.fetch_add(1, Ordering::SeqCst) + 1;
    er_telemetry_core::counters::TITLE_TIME_DESER_LAST_SLOT
        .store(slot as usize + 1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "native-fullread: TITLE-TIME DESER about to call 0x{DESERIALIZE_SLOT_RVA:x}(slot={slot}) reason={reason} -- call #{total}; a correct product run reports oracle_title_time_deser_calls=0, so this is either the narrow loose-save fallback or a routing regression"
    ));
}
pub(crate) unsafe fn native_fullread_tick(owner: usize, base: usize, n: u64) {
    const NULL: usize = TITLE_OWNER_SCAN_START_ADDRESS;
    const WAIT_INC: usize = 1;
    let gm = game_man_ptr_or_null();
    let phase = FULLREAD_PHASE.load(Ordering::SeqCst);
    let system_quit_slot = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    if system_quit_slot < TITLE_PROFILE_SLOT_COUNT {
        if phase != FULLREAD_PHASE_DONE {
            append_autoload_debug(format_args!(
                "native-fullread: STAND-DOWN for System->Quit selected slot {system_quit_slot}; native b78/MoveMapStep path owns this switch (phase={phase})"
            ));
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
        }
        return;
    }
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
    // The Load-Game action-node scan is a readiness gate (and log provenance) only -- the load chain
    // below uses slot/gm/base, never the node. For direct-file save sources (missing-save picker or
    // explicit loose save_file) the product tick has already confirmed the live menu is open and the IO
    // pool is up, so skip the scan there: it can be over-strict and would otherwise stall on a menu with
    // no separate Load-Game node / stale profile summary.
    // The default boot save reaches here for the same reason a picked one does: the native Continue
    // row cannot be identified on this build, and the node this scan looks for does not live in the
    // dialog at all -- the title's rows are ref-counted CS::MenuMemberFuncJob nodes chained into a
    // FixOrderJobSequence by the registrar 0x1409b24e0 and owned by the menu manager, so the bounded
    // flat scan of the dialog object reports hits=0 every tick by construction (measured 2026-09-06,
    // 1280 qwords). Gating the chain on it would stall a default boot forever. The summary
    // fingerprint is the real precondition -- the chain loads what the record describes -- and the
    // boot container repair now satisfies it.
    let direct_file_source =
        direct_save_file_source_active() || er_profile_summary_core::boot_slot_summary_real();
    let action = unsafe { title_menu_action_ready(owner, base) };
    if action.is_none() && !direct_file_source {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            append_autoload_debug(format_args!(
                "native-fullread: waiting for semantic Load-Game action readiness (#{n}) gm=0x{gm:x} -- TitleTopDialog/registry/node/action not all validated yet"
            ));
        }
        return;
    }
    if gm == NULL {
        if n % NATIVE_LOAD_LOG_INTERVAL == NULL as u64 {
            let (node, registry) = action
                .as_ref()
                .map_or((NULL, NULL), |a| (a.node, a.registry));
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
            let mark: unsafe extern "system" fn(usize, i32) -> u8 = unsafe {
                std::mem::transmute(
                    match crate::experiments::gated_game_fn(
                        PROFILE_MARK_SLOT_USED_RVA,
                        "PROFILE_MARK_SLOT_USED_RVA",
                    ) {
                        Some(address) => address,
                        None => return,
                    },
                )
            };
            let _ = unsafe { mark(summary, slot) };
        }
        // Step 1 (new): set the slot-resolve global GameMan+0xb78=slot (resolver 0x1406793c0 returns
        // *(u32*)(gm+0xb78)) so the native chain resolves our slot. Save-safe (an in-memory selector).
        unsafe { *((gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) as *mut i32) = slot };
        // Step 2: set_save_slot 0x14067a810(slot) -> GameMan+0xac0=slot.
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
        unsafe { set_save_slot(slot) };
        // Step 3: submit the full read 0x14067b1a0(slot) (type-0xa; sets GameMan+0xb80=2, the
        // deserialize arm). At the live menu the FD4 IO worker pool is live so this drains.
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
        // Not `submit(slot)`: the argument is a flag the game always passes as 0, and the slot
        // was already set by `set_save_slot` above. See `B80_FULL_LOAD_SUBMIT_FLAG`.
        let sret = unsafe { submit(B80_FULL_LOAD_SUBMIT_FLAG) };
        let b80 = read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET);
        let ac0 = read_i32(FORCE_PLAY_GAME_GM_SLOT_AC0_OFFSET);
        let b78 = read_i32(GAME_MAN_SLOT_SELECT_B78_OFFSET);
        append_autoload_debug(format_args!(
            "native-fullread: SUBMIT slot={slot} b78={b78} (0x{:x} write) set_save_slot 0x{:x} ac0={ac0} submit 0x{:x} ret={sret} b80={b80} -> DRAIN",
            base,
            er_game_base::mem::game_data_addr(
                base,
                FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA,
                "FORCE_PLAY_GAME_SET_SAVE_SLOT_RVA"
            ),
            er_game_base::mem::game_data_addr(
                base,
                B80_FULL_LOAD_INITIATOR_RVA,
                "B80_FULL_LOAD_INITIATOR_RVA"
            )
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
        // GameMan+0xb80==3 (resident, the 0x280000 buffer drained). Reuses cold_char_mount's calls.
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
        let _ = unsafe { poll(FULLREAD_POLL_ARG, FULLREAD_POLL_ARG) };
        let b80 = read_i32(GAME_MAN_SAVE_STATE_B80_OFFSET);
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
        // Switch feed + stale-table fix (0x67141a + soft-lock, 2026-07-16, workflow wf_0a1d790f). Two bugs on a
        // 2nd+ consecutive System->Quit->Load-Profile switch, both here:
        //  (a) CRASH: the native deser's inner CSGaitemImp::Deserialize (game+0x671130) runs on the prior
        //      character's stale gaitem table -> AV at game+0x67141a. Fix: reset the singleton to pristine
        //      first (own_load_reset_gaitem_singleton, the same native per-item release continue_confirm uses).
        //  (b) soft-LOCK: the native deser(slot) reads the game's resident IO buffer, which on switch 2 comes
        //      back m10-null (c30=0xa010000, a level-9 shell) and never populates -> guard never passes ->
        //      the commit block's continue_confirm/SetState5 never fires -> done observe-loops forever.
        // Fix (b): instead of the un-gated native deser, feed our own on-disk slot bytes through the same
        // native parser (own_load_feed_deserialize arms OWN_LOAD_GATE) so c30 becomes deterministically real
        // -> guard passes -> commit fires the native continue_confirm (our hook forwards it, no re-feed) ->
        // SetState5 streams the character. Latch FRESH_DESER_DONE=1 so switch-1's own clean-title continue_confirm
        // and this path never both feed -- exactly one deserialize per switch. Boot / non-switch (picked==max,
        // or continue_confirm already fed) falls back to the native deser unchanged.
        let picked = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
        let switch_feed_case = picked < TITLE_PROFILE_SLOT_COUNT
            && gm != TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { PlayerIns::local_player_mut() }.is_err()
            && SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst) == 0;
        // The boot default save, with a real live summary record for the target slot and no player
        // in the world yet: the same feed the switch case uses, for the same measured reason.
        let boot_feed_case = !switch_feed_case
            && gm != TITLE_OWNER_SCAN_START_ADDRESS
            && unsafe { PlayerIns::local_player_mut() }.is_err()
            && er_profile_summary_core::boot_slot_summary_real();
        // Do not DESERIALIZE at the title on the boot path. Measured 2026-09-06 (two runs, 09:59
        // and 10:04): feeding the slot here makes `c30` real before `continue_confirm`, the world
        // then loads and is playable (`LOAD-CORRECTNESS name="Banon" level=150`,
        // `T_controllable player=1`), and ~2.2 s after world entry the MoveMap child finishes, the
        // advancer raises `menuData+0x5e`, `InGameStep+0xd8` drains through its session-end arm and
        // `WORLD LOST: c30 0xe000000 -> 0xa010000` fires -- the user's black screen and the looping
        // "at the end of the previous session" MessageBox. The same shape the System->Quit switch
        // hit, and bd `step-requestwait-d8-2-to-1-conversion...` records why the d8 2->1 rewrite is
        // not the answer: it re-drives a load the game had already finished and trades the black
        // screen for a permanent stall in STEP_WorldResWait.
        //
        // The repo's own routing design already says where the deserialize belongs -- In-world,
        // from `CS::MoveMapStep::DoSaveStuff`, the only native caller `0x14067b290` has. Doing it
        // at the title is what `note_title_time_deser` exists to make loud, and a correct product
        // run reports `oracle_title_time_deser_calls = 0`.
        let dret = if switch_feed_case {
            unsafe { own_load_reset_gaitem_singleton(base) };
            if unsafe { own_load_feed_deserialize(base, gm, picked as i32) } {
                SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.store(1, Ordering::SeqCst);
                // Which slot's deserialize completed (slot+1) -- ground truth for the
                // published-vs-loaded portrait oracle. See er_telemetry_core counters.
                er_telemetry_core::counters::SYSTEM_QUIT_FRESH_DESER_DONE_SLOT
                    .store(picked + 1, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "native-fullread: DESER-path FEED of picked slot {picked} OK -- c30 now real, gaitem reset; GUARD->COMMIT continue_confirm streams (FRESH_DESER_DONE=1, no double-feed)"
                ));
                1
            } else {
                append_autoload_debug(format_args!(
                    "native-fullread: DESER-path FEED of picked slot {picked} FAILED -- falling back to native deser"
                ));
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
                note_title_time_deser(slot, "switch-feed-fallback");
                unsafe { deser(slot) }
            }
        } else if boot_feed_case {
            // The boot path hits the identical failure (b) described above, so it takes the identical
            // fix. Measured run 2026-09-06 09:46:35: the drain reached resident(3) after 17 ticks --
            // the read itself is fine -- and then the native deser returned 0 with
            // `c30=0xa010000 level=9`, the m10-null level-9 shell, so guard failed and the boot went
            // to done without a character. That is the same "reads the game's resident IO buffer and
            // gets a shell" symptom the switch case above already solves by feeding our on-disk slot
            // bytes through the same native parser.
            //
            // Gated on the slot's live `CS::ProfileSummary` record being real, which the boot
            // container repair establishes before the title menu is built -- so this feeds a slot we
            // have already decoded, never a guess.
            unsafe { own_load_reset_gaitem_singleton(base) };
            if unsafe { own_load_feed_deserialize(base, gm, slot) } {
                append_autoload_debug(format_args!(
                    "native-fullread: DESER-path FEED of BOOT slot {slot} OK -- c30 now real, gaitem reset; GUARD->COMMIT continue_confirm streams the character"
                ));
                1
            } else {
                append_autoload_debug(format_args!(
                    "native-fullread: DESER-path FEED of BOOT slot {slot} FAILED -- no fallback to the title-time native deser, which is documented to return an m10-null level-9 shell here; going to GUARD so the run fails visibly instead of loading a stub"
                ));
                0
            }
        } else {
            // Step 5: deserialize 0x14067b290(slot) once at b80==3 -> writes GameMan+0xc30 = real map.
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
            note_title_time_deser(slot, "boot-fullread");
            unsafe { deser(slot) }
        };
        // Clear the warp request the DESERIALIZE just raised. `0x14067b290` ends with
        // `SetMoveMapStepBlockId(GameMan+0xc30)` and `warpRequested = true` -- correct for the
        // native flow, where this runs in-world from `CS::MoveMapStep::DoSaveStuff` and the map
        // move consumes the request on the same pass. We run it at the title, so nothing consumes
        // it, and it is still set when `CS::MoveMapStep`'s ending-request evaluator makes its first
        // call after the world arrives.
        //
        // Measured, run 2026-09-06 10:38:51, sampled at that evaluator's own entry rather than
        // downstream of it: `rt5d=0 warp=1 b7c=0 b7d=0 force=0 session_proto=6 dead_reset=0`.
        // `GameManIsWarpRequested()` is term 7 of the nine, one true term is all `cVar10` needs,
        // and the call it was read on is the one that raised `menuData+0x5e`. Everything after is
        // the documented chain: the MoveMap child leaves resident step 18, `InGameStep` drains
        // `+0xd8` through its session-end arm, `STEP_GameStepWait` does
        // `SetMapId(0xff,0xff,0xff,0xff)` -- `WORLD LOST`, the black screen, and the looping
        // "at the end of the previous session" MessageBox.
        //
        // The destination the world actually needs is not this flag: it is
        // `GameMan+0x14` (moveMapStepBlockId), which the same deserialize sets and which
        // `movemap-init-block #1` measured as the correct `0xe000000` on these runs.
        if gm != TITLE_OWNER_SCAN_START_ADDRESS {
            let warp_before = unsafe {
                er_game_base::mem::safe_read_u8(
                    gm + er_title_flow::GAME_MAN_WARP_REQUESTED_10_OFFSET,
                )
            }
            .unwrap_or(0);
            if warp_before != 0 {
                unsafe {
                    *((gm + er_title_flow::GAME_MAN_WARP_REQUESTED_10_OFFSET) as *mut u8) = 0;
                }
                append_autoload_debug(format_args!(
                    "native-fullread: cleared GameMan+0x10 warpRequested ({warp_before} -> 0) that the title-time deserialize raised -- unconsumed at the title, it is the ending-request term that tore the loaded world back to the title map"
                ));
            }
        }
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
        FULLREAD_DRAIN_WAITS.store(NULL, Ordering::SeqCst);
        FULLREAD_PHASE.store(FULLREAD_PHASE_GUARD, Ordering::SeqCst);
        return;
    }

    /// A full-read exit that refused to load anything must hand the user the save picker, not a dead
    /// title.
    ///
    /// Every caller has just decided "this save cannot be loaded" and disarmed the pending native slot
    /// request, so the boot has no remaining path to a character: no Continue row was fired, no
    /// `SetState5`, nothing further is queued. Before 2026-09-06 all three such exits simply stored
    /// `FULLREAD_PHASE_DONE` and returned, and the title then sat forever -- measured on the product
    /// path with slot 0 = "Hero" RL7, `GUARD FAIL ... -> DONE` at +16243ms and no further progress.
    ///
    /// One-shot by construction: `arm_missing_save_picker_after_boot` returns whether this call armed
    /// it, so a phase that is polled every frame cannot re-arm or spam. The refusal itself is logged by
    /// the caller; this only reports what recourse the user was given.
    fn fullread_offer_save_picker(reason: &'static str) {
        let armed = crate::experiments::arm_missing_save_picker_after_boot(reason);
        append_autoload_debug(format_args!(
            "native-fullread: armed_by_this_call={armed} -- the full read refused to load a character (reason={reason}), so the missing-save picker is offered instead of leaving the title with no way forward"
        ));
    }

    if phase == FULLREAD_PHASE_GUARD {
        // Step 6: Guard. c30 != 0xa010000 (m10 default) and char fingerprint present (level>=10 +
        // non-empty name). This is the hard gate for the only save write.
        let c30 = read_i32(GAME_MAN_SAVED_MAP_C30_OFFSET);
        let (fp_real, level, name_len) = unsafe { char_fingerprint(base) };
        let c30_real = c30 != FULLREAD_C30_M10_DEFAULT && c30 != GAME_MAN_C30_UNSET;
        // Accepted concrete source -> any real level. `c30_real` + `fp_real` are the hard
        // new-game/null blockers; the `>= 10` floor is only a heuristic for the diagnostic path
        // where nothing preselected a source, and it must not outrank a source the boot check
        // already validated.
        //
        // The default user save is such a source, and leaving it out cost a working autoload
        // (2026-09-06). The product path -- `~/Elden/launch.sh`, no `save_file` configured -- logs
        // `save-override: DEFAULT-USER-SAVE` after `active_default_save_file()` finds a readable
        // container of the expected size for the live SteamID64, which is every bit as concrete as
        // a configured file; it simply needs no staging because the game already reads that path.
        // But only `direct_save_file_source_active()` counted, so the floor stayed at 10 and slot 0
        // = "Hero" RL7 was refused: `guard c30_real=true fp_real=true level=7 level_real=false ->
        // guard_pass=false`, `GUARD FAIL -- NO continue_confirm`, character never entered the world.
        //
        // This is not the weakening that was reverted below. That one dropped `c30_real && fp_real`
        // themselves; both stay required here and both were true in the measurement above. The
        // level-9 new-game sentinel the floor was aimed at is refused independently by those two --
        // the 10:26:57 incident logged `c30_real=false fp_real=false level_real=false`, so the
        // conjunction already rejected it three times over and the floor added nothing.
        let min_level = if direct_save_file_source_active() || default_user_save_source_active() {
            1
        } else {
            FULLREAD_MIN_REAL_LEVEL
        };
        let level_real = level >= min_level;
        // Reverted 2026-09-06, and the reason is written here so nobody re-derives it. This
        // briefly accepted "the live `CS::ProfileSummary` record for the slot is real" instead of
        // `c30_real && fp_real && level_real`, on the reasoning that `c30` is written by the
        // deserialize and so cannot be a precondition for the commit that causes it. The reasoning
        // was sound and the change was still wrong: `continue_confirm` + SetState5 with no loaded
        // character does not deserialize later, it starts a new game. Measured, run 2026-09-06
        // 10:26:57 on the user's own default save: `guard ... c30_real=false fp_real=false
        // level_real=false -> guard_pass=true`, then `load-correctness name="_" level=9
        // c30=0xa010000` and the character-creation intro cutscene on screen, one autosave away
        // from overwriting slot 0 (Banon, RL150). The run was torn down and the container verified
        // intact.
        //
        // This guard is the hard gate for the only save write in the chain. It does not get
        // weakened to make a path pass; a path that cannot satisfy it has not loaded a character.
        let guard_pass = c30_real && fp_real && level_real;
        let commit = native_fullread_commit_enabled();
        let guard_waits = FULLREAD_DRAIN_WAITS.fetch_add(WAIT_INC, Ordering::SeqCst) as u64;
        append_autoload_debug(format_args!(
            "native-fullread: GUARD waits={guard_waits} c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={level} level_real={level_real} name_len={name_len} -> guard_pass={guard_pass} commit_gate={commit}"
        ));
        if !guard_pass {
            const DIRECT_FILE_GUARD_SETTLE_TICKS: u64 = 120;
            if direct_save_file_source_active() && guard_waits < DIRECT_FILE_GUARD_SETTLE_TICKS {
                if guard_waits % FULLREAD_LOG_INTERVAL == NULL as u64 {
                    append_autoload_debug(format_args!(
                        "native-fullread: GUARD settling direct-file source waits={guard_waits}/{DIRECT_FILE_GUARD_SETTLE_TICKS} c30=0x{c30:x} level={level} name_len={name_len} -- native profile/c30 writers can lag DESER by several frames; holding req_slot and rechecking"
                    ));
                }
                return;
            }
            append_autoload_debug(format_args!(
                "native-fullread: GUARD FAIL (c30=0x{c30:x} level={level}) -- NO continue_confirm, NO SetState5, NO save write -> DONE (save-safe)"
            ));
            unsafe { fullread_disarm_slot_request(gm, "guard-fail") };
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            // And give the user somewhere to go (user, 2026-09-06: "that guard definitely should
            // not have soft locked the load, if anything it should have prompted the save picker").
            //
            // Refusing the commit is correct -- this guard is the hard gate on the only save write
            // and does not get weakened. But refusing it and then doing nothing leaves the title
            // sitting there with no Continue, no picker and no explanation, which is the softlock
            // the user hit: `GUARD FAIL ... -> DONE` at +16243ms and the run never moved again.
            // "This save cannot be loaded" is exactly the condition the missing-save picker exists
            // for, and the empty-profile path one screen earlier already answers it that way.
            fullread_offer_save_picker("fullread-guard-fail");
            return;
        }
        // Step 7 is hard-gated behind both the guard above and the commit sub-gate (default off):
        // Verify-only by default -- stop here (log only, no continue_confirm/NO SetState5).
        if !commit {
            append_autoload_debug(format_args!(
                "native-fullread: GUARD PASS (c30=0x{c30:x} level={level}) but VERIFY-ONLY (commit sub-gate OFF) -- NO continue_confirm, NO SetState5 -> DONE (save-safe). Set ER_QUICKLOAD_FULLREAD_COMMIT=1 / er-quickload-fullread-commit.txt to commit."
            ));
            unsafe { fullread_disarm_slot_request(gm, "verify-only") };
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            return;
        }
        // COMMIT: continue_confirm 0x140b0e180(rcx=&shim{[owner]=title_owner}). Disasm shows it
        // reads shim+8, only takes the NewGame branch when owner+0x284 == 1, otherwise writes
        // owner+0xbc, calls SetState5, then touches owner+0x138/+0x300. The product Continue path uses
        // this live title owner; using the stale GameDataMan+8 owner here can crash at owner+0x300
        // after the direct-file fullread succeeds.
        let owner_obj = owner;
        if owner_obj == NULL {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- title owner is null -> DONE (no write)"
            ));
            unsafe { fullread_disarm_slot_request(gm, "commit-abort-owner-null") };
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            fullread_offer_save_picker("fullread-commit-abort-owner-null");
            return;
        }
        let new_game_flag =
            unsafe { *((owner_obj + TITLE_OWNER_NEW_GAME_FLAG_284_OFFSET) as *const u8) };
        const CONTINUE_CONFIRM_NEW_GAME_BRANCH_FLAG: u8 = 1;
        if new_game_flag == CONTINUE_CONFIRM_NEW_GAME_BRANCH_FLAG {
            append_autoload_debug(format_args!(
                "native-fullread: COMMIT ABORT -- owner+0x284={new_game_flag} would take continue_confirm NewGame branch -> DONE (no write)"
            ));
            unsafe { fullread_disarm_slot_request(gm, "commit-abort-new-game-flag") };
            FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
            fullread_offer_save_picker("fullread-commit-abort-new-game-flag");
            return;
        }
        let shim = &raw mut OWN_STEPPER_SHIM;
        unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = owner_obj };
        let shim_ptr = shim as usize;
        let confirm: unsafe extern "system" fn(usize) = unsafe {
            std::mem::transmute(
                match crate::experiments::gated_game_fn(
                    CONTINUE_CONFIRM_RVA,
                    "CONTINUE_CONFIRM_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        append_autoload_debug(format_args!(
            "native-fullread: *** COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} owner=0x{owner_obj:x}) c30=0x{c30:x} level={level} owner+0x284={new_game_flag} -- SetState5 (AUTOSAVES) ***",
            er_game_base::mem::game_data_addr(base, CONTINUE_CONFIRM_RVA, "CONTINUE_CONFIRM_RVA")
        ));
        timeline_event(
            "T_fullread_confirm",
            n,
            format_args!("c30=0x{c30:x} level={level}"),
        );
        unsafe { confirm(shim_ptr) };
        // continue_confirm starts the native world stream but does not reliably consume GameMan+0xb78.
        // If req_slot survives into the first post-world save state, DoSaveStuff runs a second
        // full-deserialize into the already-live PlayerGameData and crashes in CSGaitemImp::Deserialize
        // (live 0x14067141a, gaitemInsTable[-1]). Disarm immediately after the confirmed handoff;
        // GameMan+0xac0 still carries the selected save slot for the normal loaded-world state.
        unsafe { fullread_disarm_slot_request(gm, "commit-after-confirm") };
        append_autoload_debug(format_args!(
            "native-fullread: continue_confirm returned + req_slot disarmed -- native pump now streams the real world (#{n}) -> DONE"
        ));
        FULLREAD_PHASE.store(FULLREAD_PHASE_DONE, Ordering::SeqCst);
    }
}
/// The save slot to auto-load: the active slot holding the most-progressed real character (highest level;
/// lowest index on a tie). "Active/real" is judged by the record-based `profile_slot_fingerprint`
/// (level>=1 && non-empty name) -- Not the `profile_summary+0x8` active byte, which the DLL writes itself
/// (PROFILE_SLOT_ACTIVATE / seed) and so reads all-active even for a NULL slot. Returns
/// `OWN_STEPPER_SLOT_NONE` (-1) when no slot holds a real character (or the profile summary is not yet
/// populated); callers must refuse to load on the sentinel -- never load a null slot (which spawns the
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
/// Resolve the slot to actually load under the user's guards: honor a configured slot only if it holds a
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
    let rec = profile_summary_record_address(profile_summary, slot as usize);
    let profile_map = unsafe { safe_read_usize(rec + PROFILE_SUMMARY_MAP_OFFSET) }
        .map(|value| value as u32 as i32)
        .unwrap_or(BAD_I32);
    let profile_level = unsafe { safe_read_usize(rec + PROFILE_SUMMARY_LEVEL_OFFSET) }
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
// Character identity now belongs to the loading-portrait feature crate, which already owns the
// PlayerGameData layout and GameDataMan host seam. Preserve the historical flat product name.
pub(crate) use er_loading_portrait_core::char_fingerprint;
/// Read the load-correctness invariants at the in-world transition and log a single greppable
/// `LOAD-CORRECTNESS` record: GameMan c30/ac0/name_is_empty + the CS::PlayerGameData
/// (`[base+0x4588268]`) character fingerprint (name, level, runes, rune-memory, chr_type,
/// 8-stat block). A native-menu load and a DLL-driven load produce comparable records;
/// correctness == field-for-field match (name non-empty, level/runes/stats equal). Pure reads,
/// fault-tolerant; safe to call once at the first in-world frame.
pub(crate) unsafe fn dump_load_correctness(_base: usize, frame: u64) {
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
    // Latch the peak-load semaphore: a real character (present PlayerGameData, level>=1, non-empty
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
/// current_slot_load and deserializes the real slot character (sets
/// GameMan+0x10=1), also building the world singletons. owner is a synthetic
/// buffer with +0x12c = slot. Never writes the force flag 0x143d856a0.
pub(crate) unsafe fn continue_drive_tick(_module_base: usize, slot: i32, tick: u64) {
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
    let load_progress = unsafe { *((game_man + GAME_MAN_SAVE_STATE_B80_OFFSET) as *const u8) };
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
    let dispatcher: unsafe extern "system" fn(*mut u8) -> usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                MOVEMAP_DISPATCHER_RVA,
                "MOVEMAP_DISPATCHER_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
    let _ = unsafe { dispatcher(owner) };
    if first_attempt
        || tick % TITLE_JOB_OBSERVE_TICK_INTERVAL == TITLE_OWNER_SCAN_START_ADDRESS as u64
    {
        let real_after = unsafe { *((game_man + GAME_MAN_REAL_LOAD_DONE_OFFSET) as *const i32) };
        let b80_after = unsafe { *((game_man + GAME_MAN_SAVE_STATE_B80_OFFSET) as *const u8) };
        let b73_after = unsafe { *((game_man + GAME_MAN_B73_FLAG_OFFSET) as *const u8) };
        let map14_after =
            unsafe { *((game_man + FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET) as *const i32) };
        append_autoload_debug(format_args!(
            "continue_drive: drove dispatcher slot={slot} b80={b80_after} b73={b73_after} real_done={real_after} map14={map14_after} tick={tick}"
        ));
    }
}
