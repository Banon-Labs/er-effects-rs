use super::*;

/// Save-safe verify-only own-load buffer-feed drive (one-shot, phased). Reads the .sl2 from disk,
/// slices slot `want_slot`'s plaintext body, installs+arms the gated 0x67b100 hook, calls the native
/// parser 0x67b290(slot) in-process so it parses our body, then reads back GameMan+0xc30 + the
/// PlayerGameData fingerprint. No SetState5, no autosave, no continue_confirm. Records presses==0.
pub(crate) unsafe fn own_load_drive(base: usize, gm: usize, owner: usize, want_slot: i32, n: u64) {
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
            "own-load: needs an EXPLICIT slot (slot={want_slot}); set slot=N in er-quickload-autoload.txt -- ABORT (no-write)"
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        return;
    }
    // (1) Read + slice the plaintext slot body. er_save_loader::bnd4 is the only glue: the engine's
    // read path is FSM-gated, so own-load must hand it the buffer itself (bd reuse-native-fns).
    // A prior unresolvable staged-source verdict is terminal for the process. Consume this driver's
    // phase exactly once instead of calling the resolver again and recreating the per-frame loop.
    if own_load_save_rejection_terminal() {
        append_autoload_debug(format_args!(
            "own-load: terminal save rejection already published (fingerprint=0x{:016x}) -- probe transitions to PHASE_DONE without a resolver retry",
            own_load_save_rejection_fingerprint()
        ));
        OWN_LOAD_PHASE.store(PHASE_DONE, Ordering::SeqCst);
        OWN_LOAD_PHASE_PUB.store(PHASE_DONE + 1, Ordering::SeqCst);
        return;
    }
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
        er_game_base::mem::game_data_addr(base, DESERIALIZE_SLOT_RVA, "DESERIALIZE_SLOT_RVA")
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
    // (3) Set the gate, call native 0x67b290(slot) in-process, clear the gate. 0x67b290 does not
    // re-check b80 after the read (static-confirmed), so our al=1 + body flow into the native parse.
    OWN_LOAD_GATE.store(true, Ordering::SeqCst);
    let parser: unsafe extern "system" fn(i32) -> i32 = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(DESERIALIZE_SLOT_RVA, "DESERIALIZE_SLOT_RVA") {
                Some(address) => address,
                None => return,
            },
        )
    };
    let pret = unsafe { parser(want_slot) };
    OWN_LOAD_GATE.store(false, Ordering::SeqCst);
    let fed = OWN_LOAD_FED_BYTES.load(Ordering::SeqCst);
    // (4) verify (read-back only): GameMan+0xc30 (map id) + the PlayerGameData char fingerprint.
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
        er_game_base::mem::game_data_addr(base, DESERIALIZE_SLOT_RVA, "DESERIALIZE_SLOT_RVA")
    ));
    unsafe { dump_load_correctness(base, n) };
    // Owner diagnostic (er-effects-rs-mr2, save-safe pure reads): the prior continue crash used the
    // wrong owner (*(GameDataMan+0x8)). Log every continue_confirm owner candidate + each one's
    // +0x284 (new-game flag) byte so a verify-only run reveals which is the SetState-able title
    // owner before we ever fire continue_confirm. This is independent of the gated continue step.
    //   title  = the threaded SetState-able title owner the caller validated (own_stepper_idx10),
    //   recipe = *(base + CONTINUE_MANAGER_GLOBAL_RVA + 8)  (the native-fullread commit recipe's literal),
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
    // Resolved, through the indexed form. `GAME_DATA_MAN_GLOBAL_RVA` moved +0x4060 on 1.17
    // (0x3d5df38 -> 0x3d61f98), so the raw read succeeded and handed back whatever now sits at the
    // old address + 8, which became the recipe owner. `game_data_addr_offset` keeps a refusal a
    // refusal: plain `+ FULLREAD_OWNER_GDM_08` would turn 0 into the address 8.
    let recipe_owner = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr_offset(
            base,
            CONTINUE_MANAGER_GLOBAL_RVA,
            "CONTINUE_MANAGER_GLOBAL_RVA",
            FULLREAD_OWNER_GDM_08_OFFSET,
        ))
    }
    .unwrap_or(null);
    let manager_vtable = unsafe {
        safe_read_usize(er_game_base::mem::game_data_addr(
            base,
            CONTINUE_MANAGER_GLOBAL_RVA,
            "CONTINUE_MANAGER_GLOBAL_RVA",
        ))
    }
    .unwrap_or(null);
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
    // (5) final step. Two mutually-exclusive armed levers (both off by default; verify-only is the
    // default). The LoadGame-job install lever (own_load_install_job) takes precedence: it is the
    // save-safe, non-SetState5 path (build + install the LoadGame MenuJob into owner+0x130 so
    // STEP_MenuJobWait ticks it -> self-build -> deser -> world stream; no SetState5, no save write).
    // Only if it is not armed do we fall back to the legacy guarded continue_confirm/SetState5 lever
    // (own_load_continue), which is save-writing (SetState5 autosaves) behind the hard c30/fp guard.
    // Path B (own_load_pump) takes precedence: Build the LoadGame job with real mss-derived ctx, then
    // privately pump its Run every frame from the recurring game task to completion (deser -> m28 stream)
    // and drive the transition on Success. No owner+0x130 install, no queue, no dialog -- the proven
    // menu-free "own the load". Save-safe at build (only the final SetState5 transition writes, gated).
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

/// Publish the arguments a deferred `own_load_continue_fire` needs so the game task can call back.
///
/// `c30_real` and `fp_real` are not stored: both are recomputed by the retry from the values it
/// passes, and `own_load_continue_fire` re-checks them itself before it writes anything.
fn defer_own_load_continue(base: usize, title_owner: usize, c30: i32, fp_level: u32) {
    OWN_LOAD_CONTINUE_DEFER_BASE.store(base, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_DEFER_OWNER.store(title_owner, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_DEFER_C30.store(c30 as usize, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_DEFER_LEVEL.store(fp_level as usize, Ordering::SeqCst);
    OWN_LOAD_CONTINUE_DEFERRED.store(true, Ordering::SeqCst);
}

/// Call `own_load_continue_fire` again for a commit that was held back waiting on the title
/// teardown. Does nothing unless a commit is actually deferred.
///
/// # Safety
///
/// Game-task context. The callee re-checks every save-safety condition and aborts without a write
/// on any failure, so a stale owner cannot turn into a save.
pub(crate) unsafe fn own_load_continue_retry_deferred(n: u64) {
    if !OWN_LOAD_CONTINUE_DEFERRED.load(Ordering::SeqCst) {
        return;
    }
    let base = OWN_LOAD_CONTINUE_DEFER_BASE.load(Ordering::SeqCst);
    let owner = OWN_LOAD_CONTINUE_DEFER_OWNER.load(Ordering::SeqCst);
    let c30 = OWN_LOAD_CONTINUE_DEFER_C30.load(Ordering::SeqCst) as i32;
    let fp_level = OWN_LOAD_CONTINUE_DEFER_LEVEL.load(Ordering::SeqCst) as u32;
    let c30_real = c30 != GAME_MAN_C30_UNSET && c30 != 0 && c30 != FULLREAD_C30_M10_DEFAULT;
    let (fp_real, _live_level, _name_len) = unsafe { char_fingerprint(base) };
    unsafe { own_load_continue_fire(base, owner, c30, c30_real, fp_real, fp_level, n) };
}

static OWN_LOAD_CONTINUE_DEFERRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static OWN_LOAD_CONTINUE_DEFER_BASE: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_CONTINUE_DEFER_OWNER: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_CONTINUE_DEFER_C30: AtomicUsize = AtomicUsize::new(0);
static OWN_LOAD_CONTINUE_DEFER_LEVEL: AtomicUsize = AtomicUsize::new(0);

/// Ask the title to close the menu the switch made it rebuild, and answer whether it is gone.
///
/// Which window may be asked is `crate::orphan_title_window`'s judgement and nothing else's: this
/// calls `MENU_WINDOW_CLOSE_AS_FAILED_RVA`, which closes any `MenuWindow` including a
/// `CS::MessageBoxDialog`, so it is only ever pointed at the title owner's own holder slot while a
/// switch this crate armed is mid-flight.
///
/// Returns `true` when the commit may proceed: either the owner's `DLFixedVector<MenuWindow*>` at
/// `owner+0xe0` has drained to zero elements, or the budget below is spent. Returns `false` while
/// the engine is still working, which leaves `OWN_LOAD_PHASE` where it was so the next tick asks
/// again -- the phase must not advance to `PHASE_DONE` on a tick that did not commit.
///
/// The budget exists because a load must never be hostage to a teardown. If the count has not
/// drained within `TITLE_MENU_DRAIN_BUDGET_TICKS`, this gives up, says so, and lets the commit run:
/// a title window over a loaded world is the defect this is trying to remove, and it is still far
/// better than a character that never loads.
///
/// # Safety
///
/// Game-task context, and every read is a fault-tolerant `safe_read_usize`. The one call it makes
/// is the engine's own `CloseAsFailed(MenuWindow*)`, resolved through the build-verified translator.
unsafe fn title_menu_drained_for_commit(owner: usize, n: u64) -> bool {
    /// Ticks the commit will wait for the engine to finish its own teardown before giving up.
    ///
    /// Measured, not chosen. At 240 the hold outlived the teardown it was waiting inside: the 20:21
    /// run asked for the close at `+36960ms` and the log carried a second world-loss line at
    /// `+37488ms`, 528 ms later, so holding that long let the switch's teardown run to completion
    /// and drop the player on a real title screen -- a view this flow is never meant to reach. The
    /// budget is now well inside that window, so the engine gets frames to reap and the hold can
    /// never be what takes the world down.
    const TITLE_MENU_DRAIN_BUDGET_TICKS: usize = 12;
    static DRAIN_WAITED_TICKS: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_REQUESTED_FOR_WINDOW: AtomicUsize = AtomicUsize::new(0);
    static CLOSE_REQUESTS_SPENT: AtomicUsize = AtomicUsize::new(0);
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    if owner == null {
        return true;
    }
    let Some(count) = (unsafe {
        safe_read_usize(owner + crate::constants::TITLE_OWNER_MENU_WINDOW_COUNT_128_OFFSET)
    }) else {
        // The count is unreadable, so there is nothing to wait on and nothing to prove. Commit.
        return true;
    };
    if count == 0 {
        let waited = DRAIN_WAITED_TICKS.load(Ordering::SeqCst);
        if waited != 0 {
            append_autoload_debug(format_args!(
                "title-menu-drain: owner=0x{owner:x} window count reached 0 after {waited} tick(s) -- the engine reaped the title menu the switch rebuilt; commit may proceed (#{n})"
            ));
            DRAIN_WAITED_TICKS.store(0, Ordering::SeqCst);
            CLOSE_REQUESTED_FOR_WINDOW.store(0, Ordering::SeqCst);
            CLOSE_REQUESTS_SPENT.store(0, Ordering::SeqCst);
        }
        return true;
    }
    let window =
        unsafe { safe_read_usize(owner + TITLE_OWNER_MENU_HOLDER_E0_OFFSET) }.unwrap_or(null);
    let spent = CLOSE_REQUESTS_SPENT.load(Ordering::SeqCst);
    if !crate::orphan_title_window::switch_title_menu_close_required(count, spent) {
        return true;
    }
    if window != null && CLOSE_REQUESTED_FOR_WINDOW.swap(window, Ordering::SeqCst) != window {
        CLOSE_REQUESTS_SPENT.fetch_add(1, Ordering::SeqCst);
        match crate::experiments::gated_game_fn(
            MENU_WINDOW_CLOSE_AS_FAILED_RVA,
            "MENU_WINDOW_CLOSE_AS_FAILED_RVA",
        ) {
            Some(close_addr) => {
                // Justify the transmute: the address is resolved through the same build-verified
                // translator every other direct call here uses, and the signature matches the
                // static decompile of `FUN_1407ac890` -- one `MenuWindow*` in rcx, no return.
                let close: unsafe extern "system" fn(usize) =
                    unsafe { std::mem::transmute(close_addr) };
                unsafe { close(window) };
                append_autoload_debug(format_args!(
                    "title-menu-drain: asked CloseAsFailed 0x{close_addr:x} for the title window the switch rebuilt (owner=0x{owner:x} window=0x{window:x} count={count}) -- holding the commit so STEP_MenuJobWait can run FUN_1407ada40 itself (#{n})"
                ));
            }
            None => {
                append_autoload_debug(format_args!(
                    "title-menu-drain: MENU_WINDOW_CLOSE_AS_FAILED_RVA 0x{MENU_WINDOW_CLOSE_AS_FAILED_RVA:x} did not resolve on this build -- committing without a drain, so the title menu will stay over the world (#{n})"
                ));
                return true;
            }
        }
    }
    let waited = DRAIN_WAITED_TICKS.fetch_add(1, Ordering::SeqCst) + 1;
    if waited >= TITLE_MENU_DRAIN_BUDGET_TICKS {
        append_autoload_debug(format_args!(
            "title-menu-drain: GIVING UP after {waited} tick(s) with owner=0x{owner:x} count={count} window=0x{window:x} -- committing anyway; a character that loads under a stale title beats one that never loads (#{n})"
        ));
        DRAIN_WAITED_TICKS.store(0, Ordering::SeqCst);
        CLOSE_REQUESTED_FOR_WINDOW.store(0, Ordering::SeqCst);
        CLOSE_REQUESTS_SPENT.store(0, Ordering::SeqCst);
        return true;
    }
    false
}

/// Own-load final step (er-effects-rs-mr2): after the proven verify-only parse mounted a real c30 +
/// real character, fire the guarded native `continue_confirm` 0x140b0e180 -> `SetState5` 0x140b0d960
/// to stream the character into the playable world. `continue_confirm` reads owner = [rcx+8] off
/// the shim, reads GameMan+0xc30 (already real from our parse) into owner+0xbc, then
/// SetState(owner, 5) -> the per-frame title-flow step machine streams the world.
///
/// Owner (er-effects-rs-mr2 fix): the owner must be the SetState-able title owner threaded in from
/// `own_stepper_idx10` (the validated title-flow object), not *(GameDataMan+0x8). The prior crash
/// passed *(GameDataMan+0x8) (a different object) into continue_confirm and crashed inside
/// SetState5. The owner diagnostic in the verify path logs all candidates for cross-checking.
///
/// Save-SAFETY absolute (SetState5 AUTOSAVES). Hard guard before firing -- Abort with a logged
/// no-write if any fails:
///   * `c30_real` (c30 != 0xa010000 m10-default and != 0xffffffff unset and != 0): same flag the
///     verify path computed -- never fire SetState5 on an unverified/default c30 (the prior crash
///     cause -- real char streamed to the wrong map then autosaved over).
///   * `fp_real`: the PlayerGameData char fingerprint is real (level/stats non-default).
///   * `title_owner` non-null and title_owner+0x284 (new-game flag) == 0 (continue_confirm's load
///     branch; non-zero would take the NewGame path -- fail closed).
///
/// Keeps `simulated_button_presses_total = 0`: this is a pure in-process native call, no input.
pub(crate) unsafe fn own_load_continue_fire(
    base: usize,
    title_owner: usize,
    c30: i32,
    c30_real: bool,
    fp_real: bool,
    fp_level: u32,
    n: u64,
) {
    // Caller-trace DIAG (2026-07-23, bd trace own_load arming): log the full runtime caller chain each
    // time the continue actually fires, so the actual entry/arming path is captured from evidence (static
    // tracing was repeatedly wrong -- own_load fired in run71 despite no autoload file + DIAG_NO_AUTOLOAD).
    append_autoload_debug(format_args!(
        "OWN_LOAD_CONTINUE_FIRE ENTRY c30_real={c30_real} fp_real={fp_real} own_load_continue_enabled={} CALLERS: {}",
        own_load_continue_enabled(),
        crate::crashlog::trace_callers_summary(),
    ));
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // Hard c30 + fingerprint guard (absolute save-safety backstop). NOTE: unlike the native-fullread
    // commit path (which needs a level>=10 floor to reject the level-9 new-game preview), own-load has
    // a stronger per-slot signal: `c30_real` means GameMan+0xc30 became the slot's real map
    // (0x1c000000 etc.), not the new-game default 0xa010000 -- so a real save is proven directly.
    // `fp_real` already requires level>=1 and a non-empty name (see char_fingerprint), so it admits
    // legitimate low-level real characters (e.g. a level-7 Hero-class save) that a >=10 floor would
    // wrongly reject. c30_real + fp_real is the correct, save-safe gate here.
    if !(c30_real && fp_real) {
        append_autoload_debug(format_args!(
            "own-load-continue: GUARD FAIL (c30=0x{c30:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) -- NO continue_confirm, NO SetState5, NO save write -> ABORT (save-safe)"
        ));
        return;
    }
    // Owner = the SetState-able title owner threaded in from own_stepper_idx10 (not *(GameDataMan+0x8),
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
    // Hold the commit until the engine has taken down the title menu this switch made it rebuild.
    //
    // Measured three runs in a row, 2026-09-11. The switch tears the world down (`c30 0xe000000 ->
    // 0xa010000`), the game acquires `05_000_Title` and `05_001_Title_Logo` four milliseconds later,
    // and `continue_confirm` then takes `CS::TitleStep` to `STEP_PlayGame` about 200 ms after that
    // -- while the title's own job chain is still mid-flight. `STEP_MenuJobWait` never runs again,
    // so `ExecuteMenuJob` never asks the job for a result and `FUN_1407ada40`, the only thing that
    // deregisters the window from `CSMenuMan+0x90` and erases it from the owner's vector, never
    // runs. The world then arrives underneath a title menu nothing will take down, and that window
    // holds the player's input: the report each time was "everything rendered but no ability to
    // press any button", on a character telemetry showed as `T_controllable`.
    //
    // The check sits here rather than at a caller because two earlier attempts put it at call sites
    // this build does not take -- `own_load_drive`'s final step (its `own_load_continue_enabled()`
    // branch is false) and `own_load_pump_tick`'s Success arm (no `own-load-pump` Success line was
    // ever logged) -- and both produced runs with zero `title-menu-drain` lines. This function is
    // the one every path provably reaches: its own `GUARD PASS` line is in all three logs.
    //
    // Re-entry belongs to the game task, not to whoever called here: `own_load_continue_deferred`
    // publishes the arguments and `product_core_autoload_tick` calls back each frame until the
    // count drains or the budget is spent. Nothing is pumped and no field is written -- the close is
    // the engine's own `CloseAsFailed`, and the wait is on the engine's own count.
    if !unsafe { title_menu_drained_for_commit(title_owner, n) } {
        defer_own_load_continue(base, title_owner, c30, fp_level);
        return;
    }
    OWN_LOAD_CONTINUE_DEFERRED.store(false, Ordering::SeqCst);
    // Guard passed. Build the {[OWNER_IDX]=title_owner} shim and fire the native continue_confirm.
    let shim = &raw mut OWN_STEPPER_SHIM;
    unsafe { (*shim)[OWN_STEPPER_SHIM_OWNER_IDX] = title_owner };
    let shim_ptr = shim as usize;
    let confirm: unsafe extern "system" fn(usize) = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(CONTINUE_CONFIRM_RVA, "CONTINUE_CONFIRM_RVA") {
                Some(address) => address,
                None => return,
            },
        )
    };
    append_autoload_debug(format_args!(
        "own-load-continue: *** GUARD PASS -- COMMIT continue_confirm 0x{:x}(shim=0x{shim_ptr:x} title_owner=0x{title_owner:x}) c30=0x{c30:x} level={fp_level} title_owner+0x284=0 -- continue_confirm fires SetState5 internally (AUTOSAVES) presses=0 ***",
        er_game_base::mem::game_data_addr(base, CONTINUE_CONFIRM_RVA, "CONTINUE_CONFIRM_RVA")
    ));
    timeline_event(
        "T_own_load_continue",
        n,
        format_args!("c30=0x{c30:x} level={fp_level}"),
    );
    unsafe { confirm(shim_ptr) };
    // Cache the pointers the recurring world-stream observer needs, then arm it. own_stepper_idx10 (a
    // title-phase task) stops ticking once SetState5 starts this transition, so the title `owner` and
    // its InGameStep (owner+0x2e8) will no longer be threaded in. Snapshot them here (InGameStep was
    // already non-null at frame 0) so the recurring game task can keep walking owner->InGameStep->
    // MoveMapStep through the whole loading screen. (own-load-stream-observer-must-be-recurring-task-2026-06-22)
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    mark_own_load_forced_continue_handoff();
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

/// Own-load final step -- LoadGame-job install lever (`own_load_install_job`). The save-safe,
/// non-SetState5 alternative to `own_load_continue_fire`: after the proven verify-only parse mounted a
/// real c30 + real character, build the native LoadGame `CS::MenuJobWithContext<LoadJobContext>` and
/// install it into the title owner's `+0x130` MenuJob slot, replacing the idle `IfElseJob`.
/// `CS::TitleStep::STEP_MenuJobWait` already ticks `ExecuteMenuJob(&owner->+0x130)` every frame, so the
/// installed job then self-builds (its `Run` builds the inner FixOrderJobSequence on the first tick:
/// `+0x68`/`+0x70` flip), deserializes the save, and streams the world -- Without `SetState5`.
///
/// Save-SAFETY ABSOLUTE: No `SetState5`, no autosave, no save write. The build factory only allocates +
/// copies a template; the first-tick deser step (`FUN_14082c330`) only reads the save
/// (`AllocateAligned` -> read -> `SetSaveSlot` -> decrypt -> `ReadBytes` -> dealloc) up to world-stream.
/// Static-verified against the runtime dump. Same hard c30/fp guard as the continue lever is kept as a
/// belt-and-braces precondition even though no write occurs. Keeps `simulated_button_presses_total = 0`.
///
/// ARG sourcing (static RE, 2026-06-22): the build factory `FUN_140826510(out, ctx_parent, slot,
/// owner_ctx)` needs only `out` (our local) + `slot` (the int slot) for the deser/map self-build; the
/// `ctx_parent`/`owner_ctx` args are the outer profile-selection UI context, stored as lambda captures
/// whose every build-path deref is null-guarded -- so we pass them as 0. Residual RISK: if the engine's
/// `EnableProfileSelection` release flag is set and the outer sequence ticks the profile-selection
/// sub-job, a captured-null deref could fault -- watch the install-fire log for that. The two native
/// calls are wrapped in `catch_unwind` (catches a Rust-unwinding panic; a hardware AV is not caught).
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
    let want_slot = OWN_STEPPER_SLOT.load(Ordering::SeqCst);
    let slot_addr = title_owner + TITLE_OWNER_MENUJOB_SLOT_130_OFFSET;
    // BEFORE: dump owner+0x130 (the idle IfElseJob it replaces). Pure reads.
    let (b_job, b_vt, b_seq, b_built, b_idx) = own_load_install_job_slot_snapshot(slot_addr);
    append_autoload_debug(format_args!(
        "own-load-install-job: BEFORE slot=owner+0x130=0x{slot_addr:x} job=0x{b_job:x} vt=0x{b_vt:x} (expect IfElseJob dump 0x{:x}) +0x68_built={b_built} +0x70_seq=0x{b_seq:x} +0x10_idx=0x{b_idx:x} -- BUILD 0x{:x}(out,ctx=0,slot={want_slot},owner_ctx=0) presses=0",
        MENUJOB_IFELSE_VTABLE_DUMP_VA,
        er_game_base::mem::game_data_addr(base, LOADGAME_JOB_BUILD_RVA, "LOADGAME_JOB_BUILD_RVA"),
    ));
    // (a) build the LoadGame MenuJobWithContext into a local DLRefCountPtr (the factory writes the job
    //     ptr into *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                LOADGAME_JOB_BUILD_RVA,
                "LOADGAME_JOB_BUILD_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
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
        er_game_base::mem::game_data_addr(base, MENUJOB_ASSIGN_RVA, "MENUJOB_ASSIGN_RVA"),
    ));
    // (b) APPEND our built job into the owner+0x130 MenuJobQueue via PushBackJob (not a slot-overwrite).
    //     owner+0x130 is a CS::MenuJobQueue (active job +0x130, ring +0x138, count +0x178). The prior
    //     move-assign overwrite orphaned the title IfElseJob's sibling CS::MenuWindowJobs -> AV at
    //     CS::DLFixedVector::push_back 0x140733fea. PushBackJob(queue_base=&owner+0x130, src=&built_job)
    //     appends behind the still-active IfElseJob (no tear, AtomicIncrements the job, does not zero
    //     src); STEP_MenuJobWait's ExecuteMenuJob then pops + ticks our queued job.
    // Justify the transmute: MENUJOB_PUSHBACK_RVA is the prologue-grounded live entry of
    // CS::MenuJobQueue::PushBackJob (FUN_1407a9254).
    let queue_count_before =
        unsafe { safe_read_i32(slot_addr + MENUJOB_QUEUE_COUNT_178_OFFSET) }.unwrap_or(-1);
    let pushback: unsafe extern "system" fn(*mut usize, *mut usize) -> *mut usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(MENUJOB_PUSHBACK_RVA, "MENUJOB_PUSHBACK_RVA") {
                Some(address) => address,
                None => return,
            },
        )
    };
    let install_ret = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        pushback(slot_addr as *mut usize, &raw mut built_job)
    }));
    if install_ret.is_err() {
        append_autoload_debug(format_args!(
            "own-load-install-job: PUSHBACK PANICKED (caught) after build (job=0x{built_job:x}) -> ABORT"
        ));
        return;
    }
    // AFTER: the active job at owner+0x130 should be unchanged (still the IfElseJob) -- our job is in the
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
    mark_own_load_forced_continue_handoff();
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

/// Path B "OWN THE LOAD" -- Build the LoadGame job with real mss-derived ctx, store its pointer for the
/// recurring per-frame private pump. The menu-free alternative to both the owner+0x130 install (a
/// proven dead end) and the SetState5-only continue (reached the loading screen but never mounted m28).
///
/// We build via `FUN_140826510(out, ctx_parent=mss+0x50, save_slot, owner_ctx=*(mss+0xa38))` -- the real
/// non-null ctx from the golden Continue trace (the prior ctx=0 build AV'd when the outer
/// profile-selection sub-job dereffed the captured null). We do not install the job anywhere (no
/// owner+0x130, no MenuJobQueue, no CSMenuMan dialog). Instead the recurring game task ticks its `Run`
/// privately every frame (see `own_load_pump_tick`) until it self-builds + deserializes + map-streams
/// (m28 mount) and reaches `state==Success`, then drives the title->ingame transition once.
///
/// Save-SAFETY ABSOLUTE: Build only allocates + copies a template (no save write); the first-tick deser
/// step (`FUN_14082c330`) only reads the save up to world-stream. No SetState5 here. The same hard
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
    // Corrected ctx source (bd loadgame-owner-ctx-is-dialog-a38-not-mss-correction-2026-06-22): the
    // LoadGame factory's owner_ctx (r9) and ctx_parent (rdx) come from the live CS::TitleTopDialog,
    // not from CSMenuSystemSaveLoad. The golden factory site reads `mov 0xa38(%r13),%r9` where r13 is
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
    if dialog == 0
        || dialog_vt
            != er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA",
            )
    {
        append_autoload_debug(format_args!(
            "own-load-pump: ABORT -- live TitleTopDialog not up (owner+0x{:x}=0x{dialog:x} vt=0x{dialog_vt:x} want 0x{:x}) -> no build (save-safe)",
            TITLE_OWNER_MENU_HOLDER_E0_OFFSET,
            er_game_base::mem::game_data_addr(
                base,
                TITLE_TOP_DIALOG_VTABLE_RVA,
                "TITLE_TOP_DIALOG_VTABLE_RVA"
            )
        ));
        return;
    }
    let ctx_parent = dialog + DIALOG_CTX_PARENT_50_OFFSET;
    // owner_ctx = *(dialog+0xa38) = CS::TitleFlowContext (written unconditionally by the dialog ctor
    // 0x1409a82d0, so it is valid at the settled press-any-button title -- unlike mss+0xa38 which read
    // back uninitialized garbage). Fail closed (no build) if it is not a plausible heap pointer:
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
        er_game_base::mem::game_data_addr(base, LOADGAME_JOB_BUILD_RVA, "LOADGAME_JOB_BUILD_RVA"),
        DIALOG_CTX_PARENT_50_OFFSET,
        DIALOG_OWNER_CTX_A38_OFFSET,
    ));
    // Build the LoadGame MenuJobWithContext into a local DLRefCountPtr (factory writes the job ptr into
    // *out with refcount 1). Win64 fastcall (out, ctx_parent, save_slot:i32, owner_ctx).
    // Justify the transmute: LOADGAME_JOB_BUILD_RVA is the prologue-grounded live entry of the menu-heap
    // LoadGame-job factory; the signature matches the static decompile of FUN_140826510.
    let build: unsafe extern "system" fn(*mut usize, usize, i32, usize) -> *mut usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(
                LOADGAME_JOB_BUILD_RVA,
                "LOADGAME_JOB_BUILD_RVA",
            ) {
                Some(address) => address,
                None => return,
            },
        )
    };
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
    // OWN_LOAD_CONTINUE_FIRED so own_load_stream_observe_recurring runs each frame. Do not install the
    // job anywhere -- the recurring task pumps Run directly.
    OWN_LOAD_PUMP_JOB.store(built_job, Ordering::SeqCst);
    OWN_LOAD_OWNER_CACHED.store(title_owner, Ordering::SeqCst);
    let ingame_cached = unsafe { safe_read_usize(title_owner + TITLE_OWNER_JOB_OFFSET) }
        .filter(|&v| v != null)
        .unwrap_or(0);
    OWN_LOAD_INGAMESTEP_CACHED.store(ingame_cached, Ordering::SeqCst);
    mark_own_load_forced_continue_handoff();
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

/// Path B per-frame private pump (runs from the recurring game task each frame, gated). If a LoadGame
/// job was built+armed by `own_load_pump_fire`, tick its `Run` exactly the way the native
/// `ExecuteMenuJob` does -- a zero-init `MenuJobResult` and an `FD4Time` carrying the frame delta -- so
/// the job self-builds, deserializes, and map-streams the world without the menu system. When the job
/// reaches `state==Success` (deser+map done, m28 mounted), drive the title->ingame transition once via
/// the guarded `continue_confirm`/SetState5 (the same save-safe guard as `own_load_continue_fire`), then
/// latch `OWN_LOAD_PUMP_DONE` so we never re-pump or re-transition.
///
/// Save-SAFETY: the pump itself (build+deser+map-stream) is read-only up to world-stream. The only
/// save-writing step is the final SetState5 transition, which stays hard-gated on the verified parse
/// (`c30_real && fp_real`, re-checked from the live GameMan+0xc30 and char fingerprint) + the title
/// owner's new-game flag clear -- mirroring `own_load_continue_fire`. No save write before the world is
/// confirmed loading. Every native call is wrapped in `catch_unwind` (a Rust panic is caught; a hardware
/// AV is not). Keeps `simulated_button_presses_total = 0`.
/// Pump number logged by a commit that came from the held path, where the real number belongs to a
/// `Run` several ticks back.
const HELD_COMMIT_PUMP_NUMBER: u64 = 0;

pub(crate) unsafe fn own_load_pump_tick(base: usize, gm: usize, frame_delta: f32) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    let job = OWN_LOAD_PUMP_JOB.load(Ordering::SeqCst);
    if job == 0 || job == null {
        return;
    }
    if OWN_LOAD_PUMP_DONE.load(Ordering::SeqCst) {
        return;
    }
    // Success was already reached on an earlier tick and the transition is being held back until the
    // engine has taken down the title menu the switch made it rebuild. Do not run the job again --
    // it has already deserialised and streamed the map -- just re-ask the drain and, when it
    // answers, commit.
    if OWN_LOAD_PUMP_SUCCESS_HELD.load(Ordering::SeqCst) {
        if !unsafe {
            title_menu_drained_for_commit(OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst), 0)
        } {
            return;
        }
        OWN_LOAD_PUMP_SUCCESS_HELD.store(false, Ordering::SeqCst);
        OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
        unsafe { own_load_commit_after_pump(base, gm, HELD_COMMIT_PUMP_NUMBER) };
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
    let run: unsafe extern "system" fn(usize, usize, usize, usize) -> usize = unsafe {
        std::mem::transmute(
            match crate::experiments::gated_game_fn(LOADGAME_JOB_RUN_RVA, "LOADGAME_JOB_RUN_RVA") {
                Some(address) => address,
                None => return,
            },
        )
    };
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
    if fired == 1 || fired.is_multiple_of(OWN_LOAD_STREAM_LOG_INTERVAL) {
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} Run(job=0x{job:x}) state={state} (1=Continue 2=Success 3=Failed) subcode={subcode} (deser 5/2/6) +0x68_built={built_flag} +0x70_seq=0x{inner_seq:x} delta={frame_delta}"
        ));
    }
    if state <= MENUJOB_STATE_CONTINUE {
        // Still working (Continue) -- keep pumping next frame.
        return;
    }
    // Terminal: Failed (3) stops here and latches done. Success (2) does not latch yet -- see below.
    if state == MENUJOB_STATE_FAILED {
        OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
        append_autoload_debug(format_args!(
            "own-load-pump: pump #{fired} reached state=Failed(3) subcode={subcode} -- deser/map FAILED -> NO transition, latch DONE (save-safe)"
        ));
        return;
    }
    // state == Success: the job deserialized + map-streamed (m28).
    //
    // The transition is held here, not fired here, until the engine has taken down the title menu
    // this switch made it rebuild. The latch is deliberately not set on this path: `OWN_LOAD_PUMP_DONE`
    // would stop the tick, and the tick is what re-asks the drain. `OWN_LOAD_PUMP_SUCCESS_HELD` keeps
    // the tick alive while stopping the job from being run a second time.
    if !unsafe {
        title_menu_drained_for_commit(OWN_LOAD_OWNER_CACHED.load(Ordering::SeqCst), fired)
    } {
        OWN_LOAD_PUMP_SUCCESS_HELD.store(true, Ordering::SeqCst);
        return;
    }
    OWN_LOAD_PUMP_DONE.store(true, Ordering::SeqCst);
    unsafe { own_load_commit_after_pump(base, gm, fired) };
}

/// Re-verify the parse from live state and fire the guarded transition.
///
/// Split out of `own_load_pump_tick` so the held path and the straight-through path commit through
/// one body rather than two copies that can drift. `subcode` is not re-read here: it belongs to the
/// `Run` that reached Success, which may have been several ticks ago.
///
/// # Safety
///
/// Game-task context. Every read is fault-tolerant; the transition itself re-checks
/// `c30_real && fp_real` and the owner's new-game flag inside `own_load_continue_fire` and aborts
/// without a write on any failure.
unsafe fn own_load_commit_after_pump(base: usize, gm: usize, fired: u64) {
    let null = TITLE_OWNER_SCAN_START_ADDRESS;
    // RE-verify the parse from live state (the build+pump can change GameMan+0xc30) so the
    // save-write transition is gated exactly like own_load_continue_fire.
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
        "own-load-pump: *** pump #{fired} reached state=Success(2) -- deser+map-stream DONE (m28 mounted); driving title->ingame transition ONCE (owner=0x{owner:x} c30_live=0x{c30_live:x} c30_real={c30_real} fp_real={fp_real} level={fp_level}) ***"
    ));
    // A `own_load_pump_verify_only()` early return used to sit here: a save-safe probe that stopped
    // at Success(2) without the save-writing SetState5. Its gate has returned a literal `false`
    // since it was introduced, so the branch never ran; it was deleted rather than left reading as
    // a live save-safety lever, which is the one thing a reader must not get wrong here.
    //
    // The transition is the same guarded continue_confirm/SetState5 path the legacy lever uses; it
    // re-checks c30_real && fp_real + the owner new-game flag internally and ABORTs (no write) on any
    // failure. Pass the live-re-verified c30 so the guard reflects the post-pump state.
    unsafe {
        own_load_continue_fire(base, owner, c30_live, c30_real, fp_real, fp_level, fired);
    }
}
