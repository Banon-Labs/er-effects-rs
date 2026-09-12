// Engine-loop oracles: frame pacing, the GameMan save-manager lanes, and the switch/step/flip/
// present/composite timing semaphores.
//
// Split out of `write_game_module_oracles.rs` when that file passed the hard size gate. The seam
// is the one the source already had: everything here is a self-contained block that reads a
// counter or a singleton and emits it, with no local flowing to a later subsystem -- so the whole
// group lifts out without threading a single value back. [`write_frame_pacing_oracles`] stays a
// separate function because it is the one block that must run even when the game module cannot be
// resolved; folding it in with the rest would silently gate the FPS oracle on `game_module_base`.

/// Emitted even without a resolved game module -- it reads only our own frame-time counters.
fn write_frame_pacing_oracles(body: &mut String) {
    // FPS oracle (goal 2026-07-19: stable, load1-baseline-comparable framerate). Current EMA fps + the
    // per-epoch worst-frame fps (min), written each game-task frame by lifecycle from delta_time.
    {
        use std::sync::atomic::Ordering;
        let ema_us = crate::constants::FRAME_TIME_EMA_US
            .load(Ordering::Relaxed)
            .max(1);
        let worst_us = crate::constants::FRAME_TIME_WORST_US.load(Ordering::Relaxed);
        let fps = 1_000_000.0f32 / ema_us as f32;
        let min_fps = if worst_us > 0 {
            1_000_000.0f32 / worst_us as f32
        } else {
            fps
        };
        body.push_str(&format!(
            "  \"oracle_fps\": {fps:.1},\n  \"oracle_min_fps\": {min_fps:.1},\n  \"oracle_frame_ms\": {:.2},\n",
            ema_us as f32 / 1000.0
        ));
    }
}

fn write_engine_loop_oracles(body: &mut String, base: usize) {
    const GAME_MAN_SAVE_STATE_B80_OFFSET: usize = core::mem::offset_of!(GameMan, save_state);
    const GAME_MAN_SAVED_MAP_C30_OFFSET: usize =
        core::mem::offset_of!(GameMan, stay_in_multiplay_area_saved_rotation)
            + core::mem::size_of::<fromsoftware_shared::F32Vector4>()
            + core::mem::size_of::<fromsoftware_shared::F32Vector4>();
    const READ_FAIL_SENTINEL: i32 = -1;
    const NULL_PTR: usize = 0;
    // GameMan save-mgr signals: b80 (`GameMan::saveState` -- the golden-capture mash-stop signal,
    // nonzero once continue is confirmed and the deserialize kicks) + c30 (saved map id, oracle item 2).
    //
    // The two emitted keys keep their historical names on purpose (audited 2026-08-31). The field
    // constants were renamed -- b80 is `saveState`, stamped by the save lane as well as the load
    // lane, and c30 is `stayInMultipleAreaBlockId` -- but `oracle_load_in_progress_b80` and
    // `oracle_saved_map_c30` are a wire FORMAT: they are the field names inside the recorded runs
    // in `data/oracle/imprints.db` (a SQLite imprint corpus matched by key) and in the archived
    // `save-files/**/er-effects-telemetry.json` snapshots. Renaming the key here would silently
    // stop every one of those matching, i.e. it would rewrite recorded evidence to fit a new
    // label. `oracle_saved_map_c30` is also still literally accurate at the moment its consumers
    // read it: `scripts/er-readiness-watch.py` and `switch-character-oracle.py` compare it against
    // the save file's body+0x04, which is the dword the deserializer `FUN_14067bd70` writes here.
    let gm = crate::game_man_ptr_or_null();
    let read_i32 = |addr: usize| -> i32 {
        unsafe { crate::experiments::safe_read_usize(addr) }
            .map_or(READ_FAIL_SENTINEL, |v| v as u32 as i32)
    };
    let (b80, c30) = if gm == NULL_PTR {
        (READ_FAIL_SENTINEL, READ_FAIL_SENTINEL)
    } else {
        (
            read_i32(gm + GAME_MAN_SAVE_STATE_B80_OFFSET),
            read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET),
        )
    };
    body.push_str(&format!(
        "  \"oracle_load_in_progress_b80\": {b80},\n  \"oracle_saved_map_c30\": \"{c30:#x}\",\n"
    ));
    write_title_load_route_oracles(body);
    // Switch pipeline oracle (goal 2026-07-21, bd er-effects-rs-tx9n +
    // user-oracle-must-emit-teardown-and-noload-cause): make a no-load explain itself instead of
    // degrading to CAP_REACHED. These already-tracked counters expose the FD4-IO reload phase the
    // switch load walks, so the capture script can say why a load did or did not fire.
    // reload_phase = 0 idle / 1 drain / 2 commit (+ committed one-shot); player_present +
    // menu_job_present (CSMenuMan+0x798 live in-world menu job) + stable_frames are the arm gate.
    //
    // The arm/teardown/deferred/last-slot/control-file fields that used to open this block went with
    // the control-file switch driver on 2026-09-05: they counted arms of a menu-free switch, so a
    // healthy reading there was compatible with the Quit menu being completely broken.
    {
        use er_telemetry_core::counters as swctr;
        use std::sync::atomic::Ordering as SwOrd;
        body.push_str(&format!(
            "  \"oracle_switch_reload_phase\": {},\n  \"oracle_switch_reload_drain_waits\": {},\n  \"oracle_switch_reload_committed\": {},\n  \"oracle_switch_b78_guard_standdowns\": {},\n  \"oracle_switch_player_present\": {},\n  \"oracle_switch_menu_job_present\": {},\n  \"oracle_switch_stable_frames\": {},\n  \"oracle_common_finalize_count\": {},\n  \"oracle_menu_window_finalize_guards\": {},\n  \"oracle_menu_window_finalize_last_window\": \"0x{:x}\",\n  \"oracle_outgoing_teardown_baseline\": {},\n  \"oracle_outgoing_teardown_done\": {},\n  \"oracle_outgoing_teardown_wait_ticks\": {},\n  \"oracle_outgoing_teardown_failsoft\": {},\n  \"oracle_worldreswait_gate_calls\": {},\n  \"oracle_worldreswait_hold_armed\": {},\n  \"oracle_worldreswait_hold_engaged\": {},\n  \"oracle_worldreswait_held_frames\": {},\n  \"oracle_worldreswait_released_on_settle\": {},\n  \"oracle_worldreswait_released_on_failsoft\": {},\n",
            swctr::SWITCH_RELOAD_FD4IO_PHASE.load(SwOrd::SeqCst),
            swctr::SWITCH_RELOAD_FD4IO_DRAIN_WAITS.load(SwOrd::SeqCst),
            swctr::SWITCH_RELOAD_FD4IO_COMMITTED.load(SwOrd::SeqCst),
            // b78 guard engagement oracle (bd er-effects-rs-9jbe): frames the guard stood down
            // because reload_phase was non-idle and fd4io owned GameMan+0xb78 as the warp
            // target. 0 on a run means the black-screen race never presented, so that run is
            // non-regression evidence only; > 0 means the stand-down actually fired.
            swctr::SWITCH_RELOAD_B78_GUARD_STANDDOWNS.load(SwOrd::SeqCst),
            swctr::SWITCH_ORACLE_PLAYER_PRESENT.load(SwOrd::SeqCst),
            swctr::SWITCH_ORACLE_MENU_JOB_PRESENT.load(SwOrd::SeqCst),
            swctr::SWITCH_ORACLE_STABLE_FRAMES.load(SwOrd::SeqCst),
            // Phase-3 outgoing-world teardown oracles (bd PHASE3-render-release-is-CommonFinalize):
            // common_finalize_count is the render-release oracle (flat=in-place bug, +1/switch=fixed).
            swctr::COMMON_FINALIZE_CALLS.load(SwOrd::SeqCst),
            swctr::MENU_WINDOW_JOB_FINALIZE_GUARDS.load(SwOrd::SeqCst),
            swctr::MENU_WINDOW_JOB_FINALIZE_LAST_WINDOW.load(SwOrd::SeqCst),
            swctr::OUTGOING_TEARDOWN_BASELINE.load(SwOrd::SeqCst),
            swctr::OUTGOING_TEARDOWN_DONE.load(SwOrd::SeqCst),
            swctr::OUTGOING_TEARDOWN_WAIT_TICKS.load(SwOrd::SeqCst),
            swctr::OUTGOING_TEARDOWN_FAILSOFT.load(SwOrd::SeqCst),
            // WORLDRESWAIT streaming-settle hold oracles (bd reload-overlap-fix-design-worldreswait-
            // defer-release-on-streaming-settle-2026-07-24): engaged==1 means residency was reached
            // while armed and the release was deferred; released_on_settle==1 is the good outcome
            // (geometry settled), released_on_failsoft==1 means the bounded cap fell back to today's
            // in-place release; held_frames is the deferral length.
            swctr::WORLDRESWAIT_GATE_HOOK_CALLS.load(SwOrd::SeqCst),
            swctr::WORLDRESWAIT_HOLD_ARMED.load(SwOrd::SeqCst),
            swctr::WORLDRESWAIT_HOLD_ENGAGED.load(SwOrd::SeqCst),
            swctr::WORLDRESWAIT_HELD_FRAMES.load(SwOrd::SeqCst),
            swctr::WORLDRESWAIT_RELEASED_ON_SETTLE.load(SwOrd::SeqCst),
            swctr::WORLDRESWAIT_RELEASED_ON_FAILSOFT.load(SwOrd::SeqCst),
        ));
    }
    // Loading SUBSTEP oracle (bd user-loading-bar-labels-stuck): CSSystemStep (global
    // base+0x3d85680 -> instance) drives the boot/resource load; current_state names the exact
    // subsystem being waited on (WaitRes/File/Graphics/Sound/Pad), so the loading-bar sublabel can
    // track the real hanging substep instead of a stuck label. current_state is the low 4 bytes at
    // the offset owned by `er_game_base::rva` (requested_state is the adjacent +4).
    //
    // The 21 labels below are the game's own step table, not a guess: the static initializer at
    // 0x1400b16f0 `memset`s a 0x160-byte (22-slot) StepperFn array and fills 21 of its slots with
    // `CSSystemStep::STEP_Init`, `STEP_Init_forBootPhase1`, ... `STEP_Finish`, in exactly this
    // order, in both 1.16.2 (table 0x143d85760) and 1.17 (table 0x143d897e0). The short names here
    // are those `STEP_*` names with the class prefix dropped.
    //
    // This read used the wrong offset (0x40) from its introduction until 2026-08-31 and never said
    // so: 0x40 is a live pointer field, so `state` was the pointer's low half and the label fell to
    // the `"?"` arm forever. See CS_SYSTEM_STEP_CURRENT_STATE_OFFSET for the measurement.
    {
        const CS_SYSTEM_STEP_GLOBAL_RVA: usize = er_game_base::rva::CS_SYSTEM_STEP_GLOBAL_RVA;
        const CS_SYSTEM_STEP_CURRENT_STATE_OFFSET: usize =
            er_game_base::rva::CS_SYSTEM_STEP_CURRENT_STATE_OFFSET;
        const SYSTEM_STEP_LABELS: [&str; 21] = [
            "Init",
            "InitBoot1",
            "WaitBoot1",
            "InitBoot2",
            "WaitBoot2",
            "InitBoot3",
            "WaitBoot3",
            "InitBoot4",
            "WaitBoot4",
            "InitBoot5",
            "WaitBoot5",
            "InitGameFlow",
            "WaitGameFlow",
            "FinishGameFlow",
            "WaitPreGraphics",
            "WaitGraphics",
            "WaitPad",
            "WaitRes",
            "WaitSound",
            "WaitFile",
            "Finish",
        ];
        let state = unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                CS_SYSTEM_STEP_GLOBAL_RVA,
                "CS_SYSTEM_STEP_GLOBAL_RVA",
            ))
        }
        .filter(|p| *p >= 0x10000)
        .and_then(|p| unsafe {
            crate::experiments::safe_read_usize(p + CS_SYSTEM_STEP_CURRENT_STATE_OFFSET)
        })
        .map(|v| v as u32 as i32);
        let (sv, sl) = match state {
            Some(v) if (0..=20).contains(&v) => (v, SYSTEM_STEP_LABELS[v as usize]),
            Some(v) => (v, "?"),
            None => (-2, "unresolved"),
        };
        body.push_str(&format!(
            "  \"oracle_system_step_state\": {sv},\n  \"oracle_system_step_label\": \"{sl}\",\n"
        ));
    }
    // Flip-timing oracle. CSFlipperImp singleton at base+0x4589ad8 (same 0x14458_9xxx singleton
    // table as IoDevice/DELAY_DELETE/ACCEPT_BYTE). fixed_spf(+0x1c)=frame-time target,
    // task_delta(+0x268)=actual measured delta, mode_current(+0xc), use_dynamic_lock(+0x2c8).
    // Correction (bd decisive-reload-20fps-is-render-bound-not-throttle-syncinterval1-refresh4,
    // build a38dccd): the reload 20fps is not a fixedSpf=0.05 cap and not the dynamic FPS lock.
    // Measured: fixed_spf stays 0.0167 (60fps target) and use_dynamic_lock=0 through both 20fps
    // reloads; only task_delta rises to 0.05. The game passes SyncInterval=1 to Present but
    // GetFrameStatistics reports 4 refreshes/present (oracle_present_refresh_per_present_x100=400)
    // -> the frame is render-bound, not sleep-capped. The 2026-07-21 fixedspf-0.05 cap claim is
    // refuted; keep fixed_spf vs task_delta as the target-vs-actual divergence signal.
    {
        const CS_FLIPPER_SINGLETON_RVA: usize = 0x4589ad8;
        let flipper = unsafe {
            crate::experiments::safe_read_usize(er_game_base::mem::game_data_addr(
                base,
                CS_FLIPPER_SINGLETON_RVA,
                "CS_FLIPPER_SINGLETON_RVA",
            ))
        }
        .filter(|p| *p >= 0x10000)
        .unwrap_or(NULL_PTR);
        let read_flip_f32 = |off: usize| -> f32 {
            if flipper == NULL_PTR {
                -1.0
            } else {
                unsafe { crate::experiments::safe_read_usize(flipper + off) }
                    .map_or(-1.0, |v| f32::from_bits((v & 0xffff_ffff) as u32))
            }
        };
        let read_flip_i32 = |off: usize| -> i32 {
            if flipper == NULL_PTR {
                READ_FAIL_SENTINEL
            } else {
                unsafe { crate::experiments::safe_read_usize(flipper + off) }
                    .map_or(READ_FAIL_SENTINEL, |v| v as u32 as i32)
            }
        };
        const BYTE_MASK: i32 = 0xff;
        body.push_str(&format!(
            "  \"oracle_flip_fixed_spf\": {:.6},\n  \"oracle_flip_last_frame_time\": {:.6},\n  \"oracle_flip_task_delta\": {:.6},\n  \"oracle_flip_calc_fps\": {:.2},\n  \"oracle_flip_mode_current\": {},\n  \"oracle_flip_mode_initial\": {},\n  \"oracle_flip_vsync_interval\": {},\n  \"oracle_flip_use_dynamic_lock\": {},\n  \"oracle_flip_dynamic_fps_lock\": {:.2},\n  \"oracle_flip_dynamic_active\": {},\n",
            read_flip_f32(0x1c),
            read_flip_f32(0x264),
            read_flip_f32(0x268),
            read_flip_f32(0x2b8),
            read_flip_i32(0xc),
            read_flip_i32(0x8),
            read_flip_i32(0x18),
            read_flip_i32(0x2c8) & BYTE_MASK,
            read_flip_f32(0x2c4),
            read_flip_i32(0x2c9) & BYTE_MASK,
        ));
    }
    // Focus SEMAPHORE (2026-07-21, focus-controlled A/B): is the ER window the OS foreground (this
    // process)? Tests whether the load2/load3 20fps stall correlates with the surface being
    // unfocused (the surviving compositor-present-throttle theory). Under Proton/Wine this is
    // Wine's foreground; a false during a 20fps window supports the compositor theory.
    {
        let fg = crate::experiments::game_window_is_foreground();
        body.push_str(&format!("  \"oracle_window_foreground\": {fg},\n"));
    }
    // Present-duration semaphore: microseconds inside the last original Present call. Splits a
    // present-block (compositor/vsync throttle => ~tens of ms) from a real per-frame work stall
    // (present fast but frame still 50ms). bd focus-AB-falsifies...next-present-duration-2026-07-21.
    {
        use std::sync::atomic::Ordering as PsOrd;
        let present_us = er_telemetry_core::counters::PRESENT_CALL_LAST_US.load(PsOrd::SeqCst);
        body.push_str(&format!("  \"oracle_present_call_us\": {present_us},\n"));
    }
    // Present-cadence semaphores (bd GPU-timestamp-semaphore-split-reload-20fps-residual-2026-07-22):
    // the reload 20fps is 100% flip/present residual with Present() itself fast, so the frame is
    // vsync-locked to some vblank multiple or the game requests a low present interval. sync_interval
    // = the SyncInterval the game passes to Present (3 => it deliberately throttles to every 3rd
    // vblank/20fps; 1 => wants 60). refresh_per_present_x100 = observed refreshes/present from
    // GetFrameStatistics (300 => vsync-locked 1/3). qpc_delta_us = DXGI present-to-present spacing.
    {
        use std::sync::atomic::Ordering as PcOrd;
        let sync_interval =
            er_telemetry_core::counters::PRESENT_SYNC_INTERVAL_LAST.load(PcOrd::SeqCst) as i64;
        let refresh_x100 =
            er_telemetry_core::counters::PRESENT_REFRESH_PER_PRESENT_X100.load(PcOrd::SeqCst);
        let qpc_delta_us = er_telemetry_core::counters::PRESENT_QPC_DELTA_US.load(PcOrd::SeqCst);
        // gpu_frame_us (goal §3.3): per-frame GPU-busy time from the injected D3D12 timestamp pair on
        // the game queue. Large => render-bound; small with a big qpc_delta_us => present/vblank wait.
        // samples/state make a 0 attributable (oracle not live vs GPU instant). bd er-effects-rs-03ma.
        let gpu_frame_us = er_telemetry_core::counters::GPU_FRAME_US_LAST.load(PcOrd::SeqCst);
        let gpu_frame_samples =
            er_telemetry_core::counters::GPU_FRAME_ORACLE_SAMPLES.load(PcOrd::SeqCst);
        let gpu_frame_state =
            er_telemetry_core::counters::GPU_FRAME_ORACLE_STATE.load(PcOrd::SeqCst);
        body.push_str(&format!(
            "  \"oracle_present_sync_interval\": {sync_interval},\n  \"oracle_present_refresh_per_present_x100\": {refresh_x100},\n  \"oracle_present_qpc_delta_us\": {qpc_delta_us},\n  \"oracle_gpu_frame_us\": {gpu_frame_us},\n  \"oracle_gpu_frame_samples\": {gpu_frame_samples},\n  \"oracle_gpu_frame_state\": {gpu_frame_state},\n"
        ));
    }
    // Composite-duration + boot-view EPOCH: is the DLL boot-view composite still running in-world on
    // reloads? bv_epoch_live is the epoch the boot-view stop thinks is live; if it != current_epoch
    // for load2/load3 the composite never stopped for that reload. bd present-fast-work-stall...
    {
        use std::sync::atomic::Ordering as CoOrd;
        let composite_us = er_telemetry_core::counters::COMPOSITE_LAST_US.load(CoOrd::SeqCst);
        let bv_epoch = crate::constants::BOOT_VIEW_EPOCH_WORLD_LIVE.load(CoOrd::Relaxed);
        // `oracle_current_load_epoch` is not a load count. It counts fresh deserializes committed
        // inside the switch machine, so the boot load never increments it: a session that loaded
        // three worlds (boot + two reloads) reports 2. It is fine as a within-run slicing key
        // (what every analyze-* script uses it for) and as a zero-based load index while exactly
        // one non-switch load occurred -- and it says nothing about whether those loads
        // succeeded: two captured runs both read 2, one reaching world residency three times and
        // the other once before a softlock. For the total, read `oracle_total_world_loads`; for
        // agreement across every load witness, read `oracle_load_count_mismatches`.
        let cur_epoch =
            crate::constants::SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_COUNT.load(CoOrd::SeqCst);
        body.push_str(&format!(
            "  \"oracle_composite_us\": {composite_us},\n  \"oracle_boot_view_epoch_live\": {bv_epoch},\n  \"oracle_current_load_epoch\": {cur_epoch},\n"
        ));
    }
    // DLL main game-task duration: large on reloads => DLL per-frame code cost (our bug); fast =>
    // game-side loop cost (the playable-window 50ms is not the DLL). bd correction-scan-fix-didnt...
    {
        use std::sync::atomic::Ordering as GtOrd;
        let gt_us = er_telemetry_core::counters::GAME_TASK_LAST_US.load(GtOrd::SeqCst);
        let bd_us = er_telemetry_core::counters::BUILD_DRIVER_LAST_US.load(GtOrd::SeqCst);
        body.push_str(&format!(
            "  \"oracle_game_task_us\": {gt_us},\n  \"oracle_build_driver_us\": {bd_us},\n"
        ));
    }
}
