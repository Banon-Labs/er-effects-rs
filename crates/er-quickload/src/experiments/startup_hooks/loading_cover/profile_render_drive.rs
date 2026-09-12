//! Making the profile model render, so the loading screen has a portrait to show.
//!
//! `force_profile_render_tick` and the three hooks beside it stood in
//! `quit_menu/save_swap_profile_table.rs`, sharing a file with the save-swap ledger they have
//! nothing to do with. They resolve the offscreen render target for the slot being loaded, drive
//! the native profile renderer until it has built a head, and keep the spare renderer and the
//! table builder from tearing that work down underneath it. The callers are `bootstrap.rs`,
//! `task_registration.rs`, `loading_cover_save_slot.rs` and `profile_table_gfx_files.rs` -- not a
//! cloned row among them.
//!
//! Pure move, no behaviour change.

use super::*;

/// Patch the target slot's profile offscreen RT size before any post-Continue profile renderer is
/// constructed. The constructor snapshots this table; patching after `PROFILE_TABLE_BUILDER_RVA` runs is
/// too late and produces the 256x256 loading-screen portrait (Bug A). Returns true only when the target
/// slot is known and its row is confirmed at the configured target size.
///
/// Target slot, not loaded slot (2026-07-30, deterministic different-slot no-portrait root cause).
/// During a System->Quit->Load-Profile switch the confirmed loaded slot still names the old character,
/// whose row was already patched at boot -- so this function silently early-returned true while the
/// newly-selected slot's row stayed native 128 (x2 supersample = the observed 256x256 capture, run
/// 20260730-202840: kick #2/#3 renderers for slot 1 both built 256 while `portrait-res` only logged
/// slot 2 at boot and slot 1 too late at +41246ms, after both builds). Every switch-window capture was
/// then small -> pixelated when published, rejected (no portrait at all) once the small-capture gate
/// landed. Resolve the row for the selected slot as soon as the switch names it, so every later build
/// (our loading-owned rebuild and the native mid-window TitleTopDialog rebuild) constructs the target
/// RT at full size.
pub(crate) unsafe fn patch_profile_offscreen_size_for_loaded_slot(base: usize) -> bool {
    if !portrait_real_pixels_enabled() {
        return true;
    }
    let target = if SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst)
        >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
    {
        portrait_target_slot()
    } else {
        let Some(loaded) = portrait_loaded_slot_confirmed() else {
            return false;
        };
        loaded
    };
    unsafe { patch_profile_offscreen_size_for_slot(base, target) }
}

/// Row-level worker for [`patch_profile_offscreen_size_for_loaded_slot`]; also called eagerly from the
/// switch retarget (`portrait_retarget_and_rearm_for_switch`) the moment the selected slot is known.
/// Idempotent per slot via `PROFILE_SIZE_PATCHED`.
pub(crate) unsafe fn patch_profile_offscreen_size_for_slot(base: usize, target: i32) -> bool {
    if !portrait_real_pixels_enabled() {
        return true;
    }
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&target) {
        return false;
    }
    let bit = 1usize << (target as usize);
    if PROFILE_SIZE_PATCHED.load(Ordering::SeqCst) & bit != 0 {
        return true;
    }
    // Resolved, not added (2026-08-30): this is a `.data` table and it is written below. A stale
    // RVA on 1.17 does not fault -- the `cur == PROFILE_OFFSCREEN_SIZE_INIT` check would have to
    // false-positive first -- but the value check is a filter, not a build gate, and the cost of
    // it passing on the wrong table is a 16-byte store into an unknown global. `_offset` keeps the
    // refusal answering 0 for every row rather than turning it into the address `row_index * 16`.
    let row = er_game_base::mem::game_data_addr_offset(
        base,
        PROFILE_OFFSCREEN_SIZE_TABLE_RVA,
        "PROFILE_OFFSCREEN_SIZE_TABLE_RVA",
        target as usize * PROFILE_OFFSCREEN_SIZE_TABLE_STRIDE,
    );
    if row == 0 {
        append_autoload_debug(format_args!(
            "portrait-res: REFUSED -- PROFILE_OFFSCREEN_SIZE_TABLE_RVA 0x{PROFILE_OFFSCREEN_SIZE_TABLE_RVA:x} \
             has no verified mapping for this build; the offscreen size stays native for slot {target}"
        ));
        return false;
    }
    let cur = unsafe { safe_read_usize(row) }.unwrap_or(0);
    let size_target = profile_offscreen_size_target();
    let patched = if cur == size_target {
        true
    } else if cur == PROFILE_OFFSCREEN_SIZE_INIT {
        unsafe {
            core::ptr::write_volatile(row as *mut u64, size_target as u64);
            core::ptr::write_volatile(
                (row + PROFILE_OFFSCREEN_SIZE_SUPERSAMPLE_FLAG_OFFSET) as *mut u8,
                0,
            );
        }
        true
    } else {
        false
    };
    if patched {
        PROFILE_SIZE_PATCHED.fetch_or(bit, Ordering::SeqCst);
    }
    let target_w = size_target & 0xffff_ffff;
    let target_h = (size_target >> 32) & 0xffff_ffff;
    append_autoload_debug(format_args!(
        "portrait-res: pre-builder target slot {target} row=0x{cur:x} patched={} -> base {target_w}x{target_h}, native supersample off (expected RT {target_w}x{target_h}); other slots left native 128",
        if patched { 1 } else { 0 }
    ));
    patched
}

fn incoming_portrait_slot_pending(phase: usize, selected: usize, fresh_deser: usize) -> bool {
    phase >= SYSTEM_QUIT_QUICKLOAD_PHASE_RETURN_TITLE_REQUESTED
        || (selected < TITLE_PROFILE_SLOT_COUNT && fresh_deser == 0)
}

pub(crate) unsafe fn force_profile_render_tick(base: usize, _slot: i32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if base == 0 || base == null {
        return;
    }
    // RE-engage on every loading screen (subsequent-character-load fix): pause the build pipeline only
    // during active gameplay, not permanently after the first world -- so a System Quit character switch's
    // loading screen re-builds + re-captures the new character's portrait.
    // Native portrait (2026-07-15): but keep running while the native now-loading screen is actively
    // rendering, even after PlayerIns resolves -- IN_WORLD_REACHED flips ~1.7s early on a fast load, so
    // portrait_pipeline_idle_in_gameplay went true mid-load and this tick returned before building the table
    // (run32: force_profile_render_tick never reached maybe_build). The model must build + render during the
    // loading screen we own, so gate the idle return on the native loading screen being gone.
    if unsafe { portrait_pipeline_idle_in_gameplay(base) } && !native_loading_screen_active() {
        return;
    }
    let valid = |p: usize| p != 0 && p != null;
    // Post-continue PORTRAIT: before the table-ready guard below (which would early-return on the
    // torn-down post-Continue table), repopulate the table during now-loading so the rest of this tick
    // (mark+refresh feed) and the draw/oracle run on the loading screen.
    unsafe { maybe_build_profile_table_for_loading(base) };
    // Build the player-stats text bitmap (game menu font) once the stats + font are readable, for the
    // loading cover to composite alongside the portrait in place of the native tips.
    if portrait_overlay_enabled() {
        unsafe { maybe_build_stats_text() };
    }
    // Product source ownership: the pre-Continue/ProfileSelect renderer is not our loading portrait
    // source. Ignore it completely (no kick, no spare candidate, no bake-capture/dump) until the
    // loading-screen-owned table has been built by maybe_build_profile_table_for_loading above.
    if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) == 0 {
        return;
    }
    // ProfileSummary = GameDataMan -> slot-manager container.
    let gdm = game_data_man_ptr_or_null();
    if !valid(gdm) {
        return;
    }
    let summary = unsafe { safe_read_usize(gdm + SLOT_MANAGER_CONTAINER_OFFSET) }.unwrap_or(0);
    if !valid(summary) {
        return;
    }
    // Slot->name dump, once per run (er-effects-rs-hi2 attribution): the anomaly hypothesis is
    // character-specific (Patches' boot/menu-path lifecycle differs on reload), so per-window
    // anomalies must be joinable to which character each retarget slot holds -- readable here from
    // the ProfileSummary records the pipeline already uses.
    if PROFILE_SLOT_NAMES_DUMPED.load(Ordering::SeqCst) == 0 {
        // Only consume the one-shot once at least one real name is readable: this runs before the
        // boot ProfileSummary save read (~+16s), and latching on the pre-read table logged ten
        // "(empty)" slots (run 2026-07-03 ~21:14). Keep retrying until the records are populated.
        let mut names: Vec<String> = Vec::with_capacity(TITLE_PROFILE_SLOT_COUNT);
        let mut any_real = false;
        for s in 0..TITLE_PROFILE_SLOT_COUNT {
            let rec = profile_summary_record_address(summary, s);
            let (units, len) = unsafe { read_utf16_name_units(rec) };
            let name = if utf16_name_empty_like(&units, len) {
                "(empty)".to_owned()
            } else {
                any_real = true;
                String::from_utf16_lossy(&units[..len])
            };
            names.push(format!("{s}={name}"));
        }
        if any_real {
            PROFILE_SLOT_NAMES_DUMPED.store(1, Ordering::SeqCst);
            append_autoload_debug(format_args!("profile-slot-names: {}", names.join(" ")));
        }
    }
    // Guard (crash fix): only call refresh once the renderer table is live -- it is populated at
    // TitleTopDialog ctor (main menu), not at early title. Calling refresh before the table exists
    // AVs inside refresh (observed crash rva 0x9aa6d4 = refresh+0x54 at +8939ms). Require slot-0's
    // table entry to be a valid CSMenuProfModelRend before marking/refreshing.
    let probe = unsafe { safe_read_usize(portrait_renderer_table_entry(base, 0)) }.unwrap_or(0);
    if !valid(probe)
        || unsafe { safe_read_usize(probe) }.unwrap_or(0)
            != er_game_base::mem::game_data_addr(
                base,
                TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
            )
    {
        return;
    }
    // Immediate build kick (regression fix -- goal issue 1, grounded in the 06-29 vs 06-30 capture diff):
    // the 240-tick / feed cadence below can fire before the native boot ProfileSummary read makes the
    // autoload target slot real (~+17s). When it does, the mark loop marks 0 real slots, refresh requests
    // nothing, the renderer's +0x754 "load-requested" latch stays 0, and the model never builds in the
    // brief now-loading window -> nothing to capture (06-30 runs: req754=0 req755=0 model=0x0). 06-29 runs
    // that captured a portrait marked the slot while refresh ran (req755=1 -> model=0x<nonzero>); the
    // all-slots-mark removal (correctly gated on a real fingerprint to avoid contaminating empty slots'
    // saveSlotsStates) lost that build-request for slot 0 because the cadence no longer coincides with the
    // moment the slot goes real. So here, edge-triggered: the instant a slot's fingerprint is real and its
    // renderer's +0x754 is still 0, mark + refresh it immediately (off-cadence) and open the feed window to
    // drive the async build to completion. Idempotent -- once +0x754 latches to 1 this no-ops, so no churn.
    // Only marks real slots (post-read), identical to the cadence loop's gate, so it can't pre-empt the read.
    // Only the target slot (2026-07-30): during System->Quit->Load Profile, `GameMan.save_slot`
    // still names the old resident character when the loading-cover portrait must build the newly
    // selected incoming row. The 19:54 softlock repro proved the drift: target slot=1 but this path
    // kicked "LOADED slot 2", so the first other-slot load displayed no matching profile render. Use the
    // selected switch target when present, otherwise fall back to the confirmed loaded slot for boot/load1.
    let quickload_phase = SYSTEM_QUIT_QUICKLOAD_PHASE.load(Ordering::SeqCst);
    let selected_slot = SYSTEM_QUIT_QUICKLOAD_SELECTED_SLOT.load(Ordering::SeqCst);
    let fresh_deser = SYSTEM_QUIT_CONTINUE_CONFIRM_FRESH_DESER_DONE.load(Ordering::SeqCst);
    // The phase can transiently return to idle during the clean-title handoff, before the selected
    // slot's deserialize. The old phase-only gate then fell back to ac0 (the outgoing character)
    // and latched that stale slot for the entire incoming loading window. Keep the explicit selected
    // slot authoritative until its fresh deserialize completes, regardless of phase churn.
    let target_slot = if incoming_portrait_slot_pending(quickload_phase, selected_slot, fresh_deser)
    {
        portrait_target_slot()
    } else {
        let Some(loaded) = portrait_loaded_slot_confirmed() else {
            return;
        };
        loaded
    };
    if !(0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&target_slot) {
        return;
    }
    // Fail-fast SEMAPHORE: only compare against the live loaded character once a load has actually
    // completed. The old gate was `portrait_loaded_slot_confirmed() == Some(target_slot)` -- i.e.
    // it asked ac0 (`GameMan.save_slot`) whether the target slot was resident. That gate is
    // defeated by our own write: `own_load/loaders.rs` calls the native `SetSaveSlot(picked)`
    // (Ghidra 0x14067a810 -- a pure field store with no load semantics) before submitting, ~5s
    // before the deserialize. Three milliseconds later ac0 already equalled the target, the gate
    // opened, and the semaphore compared our incoming record against the character still resident
    // from the previous session. Gate on the deserialize instead:
    //   * switch: the picked slot's fresh deserialize completed;
    //   * boot:   c30 is a real saved map (not the m10 new-game default) and the native slot
    //             request register (`GameMan+0xb78`) is back at its no-request sentinel, i.e. no
    //             load is still in flight.
    let deserialize_completed =
        if incoming_portrait_slot_pending(quickload_phase, selected_slot, fresh_deser) {
            fresh_deser == 1
        } else {
            let gm = game_man_ptr_or_null();
            gm != TITLE_OWNER_SCAN_START_ADDRESS
                && unsafe { safe_read_i32(gm + GAME_MAN_SAVED_MAP_C30_OFFSET) }
                    .is_some_and(|c30| c30 != FULLREAD_C30_M10_DEFAULT && c30 != 0)
                && unsafe { safe_read_i32(gm + GAME_MAN_SLOT_SELECT_B78_OFFSET) }
                    .is_some_and(|b78| b78 < 0)
        };
    if deserialize_completed {
        unsafe { portrait_render_slot_semaphore(base, target_slot) };
    }
    // Armor-resolution oracle (bd er-effects-rs-91l5 Layer 1). Every tick, read the live stage-0
    // ChrAsm of the renderer this tick is driving and publish the four EquipParamProtector rows the
    // model build will actually request. It reads the same `target_slot` the kick and the bake capture
    // use, so it can never score a renderer other than the displayed one. Read-only, fault-guarded,
    // and it re-resolves the pool pointer itself on every call.
    unsafe { portrait_equip_oracle_sample(base, summary, target_slot) };
    {
        let mark: unsafe extern "system" fn(usize, i32) -> u8 = unsafe {
            core::mem::transmute(
                match crate::experiments::gated_game_fn(
                    PROFILE_MARK_SLOT_USED_RVA,
                    "PROFILE_MARK_SLOT_USED_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let mut kicked = 0u32;
        let mut kicked_mask = 0u32;
        for s in 0..10i32 {
            // One slot (GX-overflow revert): immediate-kick only the target (see cadence loop).
            if s != target_slot {
                continue;
            }
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != er_game_base::mem::game_data_addr(
                        base,
                        TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                        "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                    )
            {
                continue;
            }
            // +0x754 = the refresh's "load-requested" idempotency latch. 0 = the async model build was
            // never kicked for this slot -> kick it now. Non-zero -> already requested, skip.
            if unsafe { safe_read_u8(r + 0x754) }.unwrap_or(0xff) != 0 {
                continue;
            }
            let _ = unsafe { mark(summary, s) };
            // Per-slot kick replica (not the engine's global refresh, which would kick every marked
            // slot and build all the save's characters mid-load -> the cross-slot portrait swap).
            if unsafe { kick_target_profile_slot(base, summary, r, s) } {
                kicked += 1;
                kicked_mask |= 1 << s;
            }
        }
        if kicked > 0 {
            // Drive the freshly-requested build to completion + keep it latched through the loading screen.
            PROFILE_LOADSCREEN_FEED_TICKS
                .store(PROFILE_LOADSCREEN_FEED_WINDOW_TICKS, Ordering::SeqCst);
            if PROFILE_REAL_SLOT_KICK_LOGGED.swap(1, Ordering::SeqCst) == 0 {
                append_autoload_debug(format_args!(
                    "force-profile-render: IMMEDIATE build kick -- {kicked} real slot(s) (mask=0x{kicked_mask:x}) became available with req754=0; marked + per-slot kicked off-cadence + opened feed window (summary=0x{summary:x})"
                ));
            }
        }
    }
    // Model BUILD: every ~240 ticks, mark all 10 profile slots used + call the refresh that kicks the
    // async character-model build. refresh is IDEMPOTENT per slot via the +0x754 "load-requested" latch,
    // so by default this builds each model once and then leaves it -- the model stays live every frame,
    // which is what the realtime look-at draw needs (an invalid/rebuilding pose-holder fails the draw).
    let counter = PROFILE_FORCE_TICK_COUNTER.fetch_add(1, Ordering::SeqCst);
    // Post-Continue feed window: while it is open, run the (idempotent) mark+refresh every 8 ticks so the
    // freshly-built renderers' async model build is driven to completion and stays latched -- the once-per-
    // 240 baseline is too sparse for the brief now-loading window. Outside the window keep the 240 cadence.
    let feed_window = PROFILE_LOADSCREEN_FEED_TICKS.load(Ordering::SeqCst) > 0;
    if feed_window {
        PROFILE_LOADSCREEN_FEED_TICKS.fetch_sub(1, Ordering::SeqCst);
    }
    if counter.is_multiple_of(240) || (feed_window && counter.is_multiple_of(8)) {
        let log_this = counter.is_multiple_of(240); // throttle the in-window feed log to once per 240
        let mark: unsafe extern "system" fn(usize, i32) -> u8 = unsafe {
            core::mem::transmute(
                match crate::experiments::gated_game_fn(
                    PROFILE_MARK_SLOT_USED_RVA,
                    "PROFILE_MARK_SLOT_USED_RVA",
                ) {
                    Some(address) => address,
                    None => return,
                },
            )
        };
        let mut marked = 0u32;
        for s in 0..10i32 {
            // One slot (GX-overflow revert, user 2026-07-03): build only the autoload target. Rendering
            // every saved slot overran the 192-slot GX command queue (0x1aeaf05 null-slot-write crash) --
            // 10 concurrent live renderers' draw tasks, independent of RT size (target-only 1024 didn't
            // help). All-slots menu portraits will be handled at the GFX/surface layer (remove the
            // DummyProfileFace surface) instead of driving all 10 renderers.
            if s != target_slot {
                continue;
            }
            // Real-character gate (per the native boot ProfileSummary read: level>=1 + non-empty name).
            // Never mark before the read populates the slot (can't pre-empt it / contaminate saveSlotsStates).
            if !unsafe { profile_slot_fingerprint(s).0 } {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            let r_valid = valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == er_game_base::mem::game_data_addr(
                        base,
                        TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                        "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                    );
            let _ = unsafe { mark(summary, s) };
            // Per-slot kick replica in place of the engine's global refresh: the global form kicked
            // every marked slot (all the save's characters) -- the cross-slot portrait swap source.
            // Idempotent via the +0x754/+0x755 gate inside, so the feed cadence just re-tries until
            // the record is real and then no-ops.
            if r_valid {
                let _ = unsafe { kick_target_profile_slot(base, summary, r, s) };
            }
            marked += 1;
        }
        // TRIPWIRE oracle: count non-target renderers holding a live model during our feed window.
        // Expected 0 with one-slot render -- any foreign live model is the swap-bug precondition.
        let mut foreign = 0usize;
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            if s == target_slot {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == er_game_base::mem::game_data_addr(
                        base,
                        TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                        "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                    )
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0)
                    != 0
            {
                foreign += 1;
            }
        }
        PROFILE_FOREIGN_MODELS_MAX.fetch_max(foreign, Ordering::SeqCst);
        if log_this {
            append_autoload_debug(format_args!(
                "force-profile-render: build cycle (counter={counter}) feed_window={feed_window} -- marked {marked} real slot(s) + per-slot kicked (summary=0x{summary:x} foreign_models={foreign})"
            ));
        }
        // No forced rebuild happens any more, so the model (and its skeleton) persist -> keep the
        // cached look-at indices/base; the look-at keeps driving every frame with no re-resolve gap.
    }
    // ~80 ticks after each rebuild kick, reset the dump mask so the freshly-rebuilt models (not the
    // stale pre-clear model_ins) get re-dumped. Each cycle's dumps overwrite the per-slot files.
    if counter % 240 == 80 {
        PROFILE_SLOT_DUMP_MASK.store(0, Ordering::SeqCst);
    }
    // Camera LEVER: every tick, override each live renderer's orbit camera with our custom viewport.
    // Re-applied so a refresh that re-runs the engine camera setup can't win; the dump loop below then
    // captures the custom-framed RT. Gated under the same `portrait_real_pixels` diagnostic as the dump.
    if portrait_real_pixels_enabled() {
        for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if valid(r)
                && unsafe { safe_read_usize(r) }.unwrap_or(0)
                    == er_game_base::mem::game_data_addr(
                        base,
                        TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                        "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                    )
            {
                unsafe { apply_profile_camera_override(base, r, s) };
            }
        }
    }
    // Cursor/head tracking is intentionally retired. Keep the loading portrait renderer alive and
    // refreshed, but do not rotate character bones toward the mouse. Still pre-record the target
    // renderer as the teardown spare once its model is built so the loading portrait survives Continue.
    for s in 0..TITLE_PROFILE_SLOT_COUNT as i32 {
        let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
        if valid(r)
            && unsafe { safe_read_usize(r) }.unwrap_or(0)
                == er_game_base::mem::game_data_addr(
                    base,
                    TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                    "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                )
        {
            let target = portrait_target_slot();
            if s == target
                && PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst) == 0
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                    .map(&valid)
                    .unwrap_or(false)
            {
                PROFILE_SPARE_CANDIDATE.store(r, Ordering::SeqCst);
                let model =
                    unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
                PROFILE_SPARE_CANDIDATE_MODEL.store(model, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "loading-portrait: pre-recorded spare candidate renderer=0x{r:x} slot={s} model_ins=0x{model:x} (loading-screen-owned renderer)"
                ));
            }
        }
    }
    // Per-slot: once a slot's model (+0x778) has built, readback its color offscreen RT and dump to
    // portrait-capture-slot{N}.bin once (tracked via PROFILE_SLOT_DUMP_MASK). Inspect the 10 dumps
    // offline and match to the known disk characters to map renderer-slot -> character.
    if portrait_real_pixels_enabled() {
        for s in 0..10i32 {
            let bit = 1usize << s;
            if PROFILE_SLOT_DUMP_MASK.load(Ordering::SeqCst) & bit != 0 {
                continue;
            }
            let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s)) }.unwrap_or(0);
            if !valid(r)
                || unsafe { safe_read_usize(r) }.unwrap_or(0)
                    != er_game_base::mem::game_data_addr(
                        base,
                        TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                        "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                    )
            {
                continue;
            }
            let model =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }.unwrap_or(0);
            if !valid(model) {
                continue;
            }
            let off = unsafe {
                safe_read_usize(r + TITLE_CUSTOM_COVER_PROFILE_RENDERER_OFFSCREEN_REND_OFFSET)
            }
            .unwrap_or(0);
            if !valid(off) {
                continue;
            }
            // Lighting residency oracle: envObj = renderer+0x760; *(envObj) is the registered IBL
            // env-region id, non-zero only if the GILM env map was resident when the IBL built.
            let env_obj =
                unsafe { safe_read_usize(r + PROFILE_RENDERER_ENV_REGION_OFFSET) }.unwrap_or(0);
            let ibl_region = if valid(env_obj) {
                unsafe { safe_read_usize(env_obj) }.unwrap_or(0)
            } else {
                0
            };
            if let Some((w, h, px)) = unsafe { readback_offscreen_rgba8(off) } {
                let nb = portrait_center_nonblack(w, h, &px);
                let checker = portrait_looks_like_checker(w, h, &px);
                // Bake SOURCE: store the target slot's menu portrait into LOADING_BG_PORTRAIT_RGBA so the
                // now-loading forge bakes it into the static TPF (the proven decode-time display path) and the
                // present-overlay composite (gated on PROFILE_BAKE_RGBA_CAPTURED) displays it. Only latch on a
                // real FACE: nonblack alone false-passes the magenta/white checker (an unrendered RT or our
                // cover placeholder) -- latching that is exactly what put a center checker square on screen and
                // made oracle_..._gx_nonblack a false success. Requiring !checker means we keep re-checking each
                // dump cycle and latch only once a real shaded head has actually rendered into the offscreen
                // (which needs the render-thread offscreen drive -- see portrait_render_drive). One-shot via swap.
                // No silent slot-0 FALLBACK (bd er-effects-rs-91zb step 3). This used to read
                // `portrait_loaded_slot()`, which collapses "no source names a slot" to 0 via
                // `unwrap_or(0)` -- so with nothing confirmed, slot 0's head was published as
                // though it were the loaded character. Publish nothing instead: a missing portrait
                // is a visible, diagnosable absence; a confidently wrong one is not.
                if portrait_loaded_slot_confirmed() == Some(s)
                    && nb
                    && !checker
                    && PROFILE_BAKE_RGBA_CAPTURED.swap(1, Ordering::SeqCst) == 0
                {
                    let _ = ibl_region;
                    dump_portrait_rgba(110, w, h, &px);
                    // Mask gate (user 2026-08-21: "do not render the portrait until we mask out the
                    // background"). This writer is the one that put an unmasked head on screen.
                    //
                    // `readback_offscreen_rgba8` reads the colour offscreen and nothing else -- it never
                    // touches the depth sibling, so `apply_depth_alpha_key` never runs on this path and
                    // every texel it returns has alpha 255. The gates above are all colour tests
                    // (`nonblack`, `!checker`) and cannot see that; the resulting fully opaque buffer went
                    // into LOADING_BG_PORTRAIT_RGBA, and the compositor drew the character together with
                    // its whole scene background. It does not even bump LOADING_BG_PORTRAIT_RGBA_VERSION,
                    // so no reader downstream could have noticed the buffer had changed.
                    //
                    // Refuse rather than key it here. Masking on this path would mean acquiring and
                    // reading back the matching depth target on the game thread at FrameBegin, i.e.
                    // duplicating the worker's staged colour+depth pipeline on the wrong thread -- and
                    // the worker publishes a correctly keyed frame moments later anyway. A refused bake
                    // costs only that the portrait appears when it is actually ready.
                    //
                    // Un-consume the one-shot on refusal, exactly as the readiness gate below does, so a
                    // later capture can still bake if one ever arrives already keyed.
                    if !portrait_frame_is_masked(&px) {
                        PORTRAIT_BAKE_PUBLISH_REFUSED_UNMASKED.fetch_add(1, Ordering::SeqCst);
                        PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "loading-portrait: BAKE REFUSED slot={s} dims={w}x{h} -- colour-only readback is unmasked (alpha cut < {PORTRAIT_MIN_TRANSPARENT_PCT}%); holding for the depth-keyed worker publish"
                        ));
                    } else if note_ls_portrait_capture(w, h, &px) {
                        // Readiness gate: hold back neutral/too-small transient captures (Bug A/B). On
                        // rejection, un-consume the one-shot (the swap fired in the condition above) so a
                        // later full-size head still bake-captures.
                        if let Ok(mut g) = LOADING_BG_PORTRAIT_RGBA.lock() {
                            *g = Some((w, h, px.clone()));
                        }
                        // Identity tag rides with every bridge write (bd er-effects-rs-dpf6 Phase 1).
                        // Game thread: hash slot `s`'s summary record directly.
                        LS_PORTRAIT_PUBLISHED_SLOT.store((s + 1) as usize, Ordering::SeqCst);
                        LS_PORTRAIT_PUBLISHED_NAME_HASH
                            .store(unsafe { portrait_slot_name_hash(s) }, Ordering::SeqCst);
                        append_autoload_debug(format_args!(
                            "loading-portrait: BAKE-CAPTURED real menu portrait slot={s} dims={w}x{h} ibl_region=0x{ibl_region:x} -> LOADING_BG_PORTRAIT_RGBA (forge will bake it)"
                        ));
                    } else {
                        PROFILE_BAKE_RGBA_CAPTURED.store(0, Ordering::SeqCst);
                    }
                }
                dump_portrait_rgba(s, w, h, &px);
                PROFILE_SLOT_DUMP_MASK.fetch_or(bit, Ordering::SeqCst);
                append_autoload_debug(format_args!(
                    "profile-slot-dump: slot={s} renderer=0x{r:x} model=0x{model:x} dims={w}x{h} nonblack={} env_obj=0x{env_obj:x} ibl_region=0x{ibl_region:x}",
                    nb as u8
                ));
            }
        }
    }
}

/// Hook on the CSMenuProfModelRend teardown-all (`FUN_1409b2f00`). One-shot: before the original
/// runs, save slot-0's renderer and null its table entry so the original's null-guarded delete
/// enqueue skips it -- sparing the loaded character's portrait renderer from the Continue teardown so
/// we can keep rendering it into the now-loading screen. The original then tears down slots 1-9.
pub(crate) unsafe extern "system" fn profile_renderer_teardown_spare_hook() {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let valid = |p: usize| p != 0 && p != null;
    // TEARDOWN fence (freeze relaxation, er-effects-rs-l1x): raise the fence before any
    // delete-enqueue below (both the orphan reclaim and the native table teardown in original()),
    // then wait out a render-thread pump caught mid-drive. The pump is one model update+draw
    // (sub-ms), so the 10ms cap is generous; a timeout is counted, not fatal -- worst case equals
    // the old per-frame TOCTOU exposure for exactly one frame instead of every frame. The fence is
    // lowered at the end of this hook, after the native teardown returns.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(1, Ordering::SeqCst);
    if PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
        PROFILE_TEARDOWN_FENCE_WAITS.fetch_add(1, Ordering::SeqCst);
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(10);
        // Backed off rather than a bare yield per attempt: this runs on the game thread while the
        // render thread finishes one pump, and a yield per attempt is a wineserver round trip per
        // attempt. `back_off` spins in user space and yields once per call, so the 10ms cap is
        // still the bound and the wait no longer costs the wineserver a syscall storm to hold it.
        let mut attempt = 0_u64;
        while PROFILE_IN_OUR_DRIVE.load(Ordering::SeqCst) {
            if std::time::Instant::now() > deadline {
                PROFILE_TEARDOWN_FENCE_TIMEOUTS.fetch_add(1, Ordering::SeqCst);
                break;
            }
            er_game_base::wait::back_off(attempt);
            attempt += 1;
        }
    }
    // Repeated-switch GX overflow fix (0x1aeaf05, ~switch #4): destroy the prior window's spared
    // renderer now, on the game thread, before sparing this switch's renderer. The load-complete
    // reset (render thread) moved it into PROFILE_SPARE_ORPHAN instead of dropping it; the spare
    // excluded it from the native delete (nulled its table slot), so without this it stayed alive
    // with its ResMan offscreen draw task filling the 192-slot GX command queue every frame,
    // accumulating +1 leaked renderer per switch. delay_delete_enqueue_renderer is the exact native
    // delete path (vtable-guarded), run here on the correct thread.
    let orphan = PROFILE_SPARE_ORPHAN.swap(0, Ordering::SeqCst);
    if orphan != 0 {
        let deleted = unsafe { delay_delete_enqueue_renderer(orphan) };
        // Ownership ledger: discharge our responsibility for the spared renderer (paired with the
        // ownership_take at the spare site). Released whether or not the enqueue took -- either we
        // handed it to delay-delete or it was already stale/gone; either way it is no longer ours.
        ownership_release(OwnedClass::SparedRenderer);
        append_autoload_debug(format_args!(
            "loading-portrait: reclaimed prior spared renderer 0x{orphan:x} via CSDelayDeleteMan enqueued={deleted} (repeated-switch GX command-queue leak fix)"
        ));
    }
    // If the native title/menu code tries to run the teardown-all again after we have already rebuilt the
    // loading-screen-owned portrait table, do not let it delete the live animated source mid-load. This is
    // the exact failure exposed by the leading-gap fix: early build at LoadingScreen start, model animates,
    // then a later stale native teardown clears DAT_143d6d8d0 and the overlay keeps displaying the last
    // snapshot frozen (r_bad climbs, drive/display diverges). The builder's own internal teardown still runs
    // normally because PROFILE_LOADSCREEN_TABLE_OWNED is set only after the builder returns.
    if PROFILE_LOADSCREEN_TABLE_OWNED.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_UPDATE_HITS.load(Ordering::SeqCst) != 0
        && LOADING_SCREEN_CLOSE_SENT_HITS.load(Ordering::SeqCst) == 0
    {
        append_autoload_debug(format_args!(
            "loading-portrait: skipped stale native profile-table teardown during active LoadingScreen -- keeping loading-owned renderer alive so the portrait keeps animating"
        ));
        PROFILE_RENDERER_TEARDOWN_FENCE.store(0, Ordering::SeqCst);
        return;
    }
    // Gate on the live-portrait overlay feature or product autoload -- the native-continue path does not set
    // PRODUCT_AUTOLOAD_ARMED, so gating on product_autoload alone never spared anything there.
    if LOADING_BG_PORTRAIT_SPARED_RENDERER.load(Ordering::SeqCst) == 0
        && (product_autoload_enabled() || portrait_overlay_enabled())
        // Disable-on-reload FALLBACK (user 2026-07-23): do not spare the portrait renderer on a
        // System->Quit->Load switch reload. A spared renderer whose GX resource goes stale across the
        // reload crashed load2 near completion (null native GX resource wrapper -> FUN_141e90290 rcx=0x20
        // AV; spared[model_ok=0]; lookat off_resource_bad climbing 68->128). Skipping the spare lets the
        // native teardown free it with the world -- no stale spared renderer, so the per-frame profile-draw
        // never runs against a dead resource. LOAD1/first-load is unaffected (switch_reload_active()==false
        // there), so the loading-portrait still shows on the initial load, just not on reloads. This is the
        // user-chosen fallback ahead of the full Root A teardown fix (unregister the ResMan draw task +
        // free per reload). bd rootB-fd4io-fix-works-load2-resubmits-but-exposes-rootA-spared-renderer-crash-2026-07-23.
        && !crate::experiments::gating::switch_reload_active()
        && let Ok(base) = game_module_base()
    {
        // The slot we render (er-effects-rs-j3r): the newly-selected character on a switch
        // (SELECTED_SLOT), else the loaded slot (ac0). portrait_target_slot() is what makes the
        // loading portrait show the character just picked, not the one still resident.
        let slot = portrait_target_slot();
        // Prefer the pre-recorded candidate (captured at the menu on a model-built frame -- robust to
        // the menu's model_ins cycling). Find its table slot and protect it. Fall back to reading
        // table[slot] + a model-built guard if no candidate was recorded.
        let candidate = PROFILE_SPARE_CANDIDATE.load(Ordering::SeqCst);
        let target_te = portrait_renderer_table_entry(base, slot);
        // Honor the pre-recorded candidate only if it still sits in the target slot. A candidate
        // captured for the old character before a switch confirm must not be spared over the
        // newly-selected one -- in that case fall back to table[target] (its model is built, the
        // menu rendered all 10 slots). Prevents the loading portrait showing the prior character.
        let candidate_in_target =
            valid(candidate) && unsafe { safe_read_usize(target_te) }.unwrap_or(0) == candidate;
        let (renderer, table, spared_slot) = if candidate_in_target {
            (candidate, target_te, slot)
        } else {
            let r = unsafe { safe_read_usize(target_te) }.unwrap_or(0);
            let model_built = valid(r)
                && unsafe { safe_read_usize(r + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                    .map(&valid)
                    .unwrap_or(false);
            (if model_built { r } else { 0 }, target_te, slot)
        };
        if valid(renderer)
            && unsafe { safe_read_usize(renderer) }.unwrap_or(0)
                == er_game_base::mem::game_data_addr(
                    base,
                    TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA,
                    "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA",
                )
        {
            LOADING_BG_PORTRAIT_SPARED_RENDERER.store(renderer, Ordering::SeqCst);
            PROFILE_RENDERER_SPARE_HITS.fetch_add(1, Ordering::SeqCst);
            // Ownership ledger: we just excluded this renderer from the native delete, so we own
            // its destruction now. Paired with the ownership_release on the drain path below.
            ownership_take(OwnedClass::SparedRenderer);
            // Null the table entry so the original's null-guarded delete-enqueue skips it.
            if table != 0 {
                unsafe { (table as *mut usize).write_volatile(0) };
            }
            // Re-latch the look-at base from the post-Continue model (a different model instance).
            if (0..TITLE_PROFILE_SLOT_COUNT as i32).contains(&spared_slot) {
                let mut guard = match PROFILE_LOOKAT_SLOTS.lock() {
                    Ok(g) => g,
                    Err(p) => p.into_inner(),
                };
                if let Some(s) = guard[spared_slot as usize].as_mut() {
                    s.base_latched = false;
                }
            }
            let model_at_spare =
                unsafe { safe_read_usize(renderer + PROFILE_RENDERER_MODEL_INS_OFFSET) }
                    .unwrap_or(0);
            append_autoload_debug(format_args!(
                "loading-portrait: SPARED slot{spared_slot} renderer=0x{renderer:x} (candidate=0x{candidate:x}) model_ins=0x{model_at_spare:x} from teardown -- drive look-at + render it post-Continue"
            ));
        }
    }
    let orig = PROFILE_RENDERER_TEARDOWN_HOOK_ORIG.load(Ordering::SeqCst);
    if orig != null && orig != HOOK_ORIGINAL_UNSET {
        let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
        unsafe { f() };
    }
    // Native teardown done -- the table entries are delete-enqueued/nulled, so the next pump
    // invocation's per-frame table re-read + vtable probe fails closed until the new window's
    // rebuild. Safe to let the drive back in.
    PROFILE_RENDERER_TEARDOWN_FENCE.store(0, Ordering::SeqCst);
}

/// Diagnostic + repair detour on the native profile-portrait builder (`FUN_1409aa7d0` =
/// `PROFILE_RENDERER_REFRESH_RVA`). The builder derefs `table[slot]+0x754` with no null check for
/// every slot whose profile record exists (Ghidra: `FUN_140261c30(summary,slot) != 0` gates the
/// walk, the entry itself is never checked), and its 10-slot table setup is called from exactly one
/// native site -- the TitleTopDialog constructor -- so our cloned in-world ProfileSelect reopens run
/// it against whatever the last teardown left; the 3rd in-session open found the table fully empty
/// and AV'd at `[null+0x754]` (er-effects-rs-j3r). Three layers, all fault-guarded + catch_unwind:
///   1. DIAG: log the full table once per distinct degraded (mask, caller) pattern.
///   2. REPAIR: a fully-empty table (the proven crash state) is rebuilt via the engine's own no-arg
///      setup (`PROFILE_TABLE_BUILDER_RVA`; its internal teardown is a no-op on an all-null table),
///      satisfying the native invariant exactly as the TitleTopDialog ctor would. Gated on
///      `PROFILE_TABLE_WAS_POPULATED` (engine/ResMan up -- the setup AVs at boot title) and on
///      fully-empty ONLY: a mixed table is the intentional teardown-spare state during Continue
///      loading and must not be rebuilt over.
///   3. GUARD: if any slot is still null/invalid after the (possible) repair, skip chaining the
///      original this call (fail-soft; the per-frame builder retries) instead of letting the native
///      walk AV.
pub(crate) unsafe extern "system" fn profile_select_table_diag_hook() {
    let chain = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let Ok(base) = game_module_base() else {
            return true;
        };
        let null = TITLE_OWNER_SCAN_START_ADDRESS;
        let scan_table = |ptrs: &mut [usize; TITLE_PROFILE_SLOT_COUNT]| -> (u32, u32) {
            let mut null_mask = 0u32;
            let mut valid_mask = 0u32;
            for (s, ptr_slot) in ptrs.iter_mut().enumerate() {
                let r = unsafe { safe_read_usize(portrait_renderer_table_entry(base, s as i32)) }
                    .unwrap_or(0);
                *ptr_slot = r;
                let is_valid = r != 0
                    && r != null
                    && unsafe { safe_read_usize(r) }.unwrap_or(0)
                        == er_game_base::mem::game_data_addr(base, TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA, "TITLE_CUSTOM_COVER_PROFILE_RENDERER_VTABLE_RVA");
                if is_valid {
                    valid_mask |= 1 << s;
                } else {
                    null_mask |= 1 << s;
                }
            }
            (valid_mask, null_mask)
        };
        let mut ptrs = [0usize; TITLE_PROFILE_SLOT_COUNT];
        let (valid_mask, mut null_mask) = scan_table(&mut ptrs);
        // Degraded = any slot lost its renderer while the builder is about to run. A healthy table
        // is all 10 valid (native setup allocs all 10 unconditionally); any null is the crash-prone
        // state, including all-null (the fully-empty table that caused the 3rd-open crash -- the
        // earlier "mixed only" check missed it). Log per distinct (mask, caller) so it never spams.
        let degraded = null_mask != 0;
        let caller_rva = crate::crashlog::trace_first_game_caller_rva();
        let key =
            ((caller_rva & 0xffffff) << 20) | ((valid_mask as usize) << 10) | null_mask as usize;
        if degraded && PROFILE_SELECT_TABLE_DIAG_LAST.swap(key, Ordering::SeqCst) != key {
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-DIAG: degraded profile-renderer table before native builder (er-effects-rs-j3r) caller_rva=0x{caller_rva:x} valid_mask=0x{valid_mask:x} null_mask=0x{null_mask:x} entries=[0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x},0x{:x}]",
                ptrs[0], ptrs[1], ptrs[2], ptrs[3], ptrs[4], ptrs[5], ptrs[6], ptrs[7], ptrs[8],
                ptrs[9]
            ));
        } else if !degraded {
            PROFILE_SELECT_TABLE_DIAG_LAST.store(0, Ordering::SeqCst);
            // A fully-valid table at builder entry proves the engine built renderers successfully --
            // the same "engine/ResMan up" evidence the loading-screen path latches; latching it here
            // too arms the repair even when the loading-portrait feature is disabled.
            PROFILE_TABLE_WAS_POPULATED.store(1, Ordering::SeqCst);
        }
        if null_mask == PROFILE_TABLE_ALL_SLOTS_MASK
            && PROFILE_TABLE_WAS_POPULATED.load(Ordering::SeqCst) != 0
            && let Some(build_addr) =
                crate::experiments::gated_game_fn(PROFILE_TABLE_BUILDER_RVA, "PROFILE_TABLE_BUILDER_RVA")
        {
            let build: unsafe extern "system" fn() = unsafe { core::mem::transmute(build_addr) };
            unsafe { build() };
            let n = PROFILE_SELECT_TABLE_REPAIR_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let (revalid_mask, renull_mask) = scan_table(&mut ptrs);
            null_mask = renull_mask;
            append_crash_log(format_args!(
                "PROFILESELECT-TABLE-REPAIR #{n}: fully-empty renderer table at native builder entry -> re-ran native table setup 0x{:x}; post-repair valid_mask=0x{revalid_mask:x} null_mask=0x{renull_mask:x} (er-effects-rs-j3r)",
                build_addr
            ));
            append_autoload_debug(format_args!(
                "profileselect-table-repair #{n}: rebuilt empty 10-slot renderer table via native setup before the native builder walked it; post-repair valid_mask=0x{revalid_mask:x} (er-effects-rs-j3r)"
            ));
        }
        if null_mask != 0 {
            let n = PROFILE_SELECT_TABLE_GUARD_SKIP_COUNT.fetch_add(1, Ordering::SeqCst) + 1;
            let skip_key = ((caller_rva & 0xffffff) << 10) | null_mask as usize;
            if PROFILE_SELECT_TABLE_GUARD_SKIP_LAST.swap(skip_key, Ordering::SeqCst) != skip_key {
                append_crash_log(format_args!(
                    "PROFILESELECT-TABLE-GUARD SKIP #{n}: null/invalid renderer slots remain (null_mask=0x{null_mask:x}) -- skipping the native builder this call so it cannot AV at [null+0x754] (er-effects-rs-j3r)"
                ));
            }
            return false;
        }
        true
    }))
    // A panicked diagnostic keeps the pre-hook behavior: chain the original.
    .unwrap_or(true);
    if !chain {
        return;
    }
    let orig = PROFILE_SELECT_TABLE_DIAG_ORIG.load(Ordering::SeqCst);
    if orig == HOOK_ORIGINAL_UNSET {
        return;
    }
    let f: unsafe extern "system" fn() = unsafe { std::mem::transmute(orig) };
    unsafe { f() };
}

#[cfg(test)]
mod portrait_target_pending_tests {
    use super::*;

    #[test]
    fn selected_slot_survives_transient_idle_phase_until_deserialize() {
        assert!(incoming_portrait_slot_pending(
            SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE,
            2,
            0
        ));
    }

    #[test]
    fn completed_selection_no_longer_overrides_resident_slot() {
        assert!(!incoming_portrait_slot_pending(
            SYSTEM_QUIT_QUICKLOAD_PHASE_IDLE,
            2,
            1
        ));
    }
}
