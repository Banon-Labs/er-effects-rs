use super::*;

// The MISSING-SAVE BOOT picker: its one-shot open, and the OS-native arm of it.
//
// WHAT THIS FIXES. `er-effects.toml`'s `os_native_save_picker` governed the two System>Quit
// pickers and nothing else. The boot arm -- armed by `enforce_save_override_or_abort` when no
// usable save resolves -- called the DLL-drawn overlay browser directly, so no code on that path
// ever read the key and a user who asked for the OS dialog still got the in-game browser at boot.
// The open now routes through `open_picker_for_intent`, the same function the System>Quit intents
// use, and `save_picker_surface.rs`'s table test covers the boot intent alongside them.
//
// ============================================================================================
// WHICH THREAD BLOCKS, AND WHY IT IS NOT ONE OF THE GAME'S
// ============================================================================================
//
// `GetOpenFileNameW` is synchronous. Whichever thread calls it is gone for as long as the user
// browses -- measured at 17.2 seconds in one live System>Quit run. At System>Quit that is the
// POINT: the blocked thread is the menu pump, and a frozen menu pump is the modality the dialog
// wants. At boot there is no menu pump, and the two threads that actually reach the boot picker
// are both the game's own:
//
//   * the D3D12 Present hook (`present_overlay.rs`), which is where the Wine build drives the boot
//     picker because it is the only thread that reads OS keys under Wine (bd
//     `save-picker-input-wine-background-thread-broken-2026-07-07`). Blocking it stops the
//     swapchain: the boot loading bar, the picker overlay and every Present-driven surface freeze,
//     and we would be parked inside the game's render submission with command lists in flight.
//   * the CSTaskImp recurring game task (`lifecycle.rs`, the native-Windows path and the thread
//     that completes every pick). It is pumped by the engine's frame loop; a callback that never
//     returns stalls that task group for the dialog's whole lifetime.
//
// Blocking either one produces the "not responding" window `save_picker_dim_overlay.rs` exists to
// explain -- and at boot it would freeze the very overlay that IS the alternative picker. So this
// arm opens the dialog on a thread WE own. Nothing of the game's stalls, Present keeps running,
// the boot bar keeps animating, and `no_picker_cover` follows from that: there is no frozen game
// for a cover to account for.
//
// PROVEN 2026-07-30 (run pr109-boot-oscancel-20260730-110704): comdlg32 DOES render and take input
// from this thread under Wine/Proton. The dialog opened at +4277ms, the log records its real child
// windows being created (`class=Button name=Cancel`, the file ListBox, the ComboBoxEx32 path bar),
// the user browsed it for 24.5 seconds, and it returned `result=cancelled`. The thread choice was
// the one thing this file was least sure of and it is now settled.
//
// The failure signature that used to be documented here -- "open_count advances, boot_state stuck at
// OPEN, no CLOSED line" -- WAS WRONG and is deliberately not repeated: that same run matched its
// telemetry half exactly while working perfectly, because the telemetry file had gone stale. See
// `write_telemetry.rs`, where `oracle_save_picker_boot_telemetry_flushed` now discriminates the two.
//
// ============================================================================================
// WHAT CANCEL MEANS HERE, AND WHY IT DIFFERS FROM SYSTEM>QUIT
// ============================================================================================
//
// A cancelled System>Quit picker discharges its open request and returns to the System menu (the
// #107 fix). A cancelled BOOT picker QUITS THE GAME. That is not a new invention: it is the
// contract `path_hooks.rs` has documented since before the in-game browser existed -- "OK ->
// choose a save, Cancel -> exit" -- and at a missing-save boot it is the only bounded terminal
// outcome available. There is no menu to fall back to, and `missing_save_selection_pending()`
// denies world entry (`title_tick_cover.rs` refuses `SetState(4/5)`) until a save is chosen, so
// "return to what you were doing" would mean returning to a title that can never be left.
//
// THE ARMS DISAGREE, AND THIS IS THE LOUD PART OF SAYING SO. The in-game overlay browser has no
// cancel AT ALL and this change does not give it one: its `PICKER_ACT_BACK` is `go_up()`, which is
// a no-op at a drive root, so a user on that surface has no exit but killing the process. That is
// pre-existing (bd `er-effects-rs-mb0y` tracks it), it is the DEFAULT surface, and it is the one
// place "world entry denied forever with no way out" still exists. comdlg32 draws a Cancel button
// whether we want one or not, so the OS arm cannot decline to have the outcome; what it can do is
// make it a designed, bounded, telemetry-visible quit rather than an unhandled dismissal, which is
// what the rest of this file is. Closing the gap on the in-game arm means adding a key to a
// shipping input surface, and that belongs in a change that can be runtime-validated on its own
// rather than riding along with an unproven one.
//
// The exit is the product's OWN quit: `ExitProcess(0)`, the same clean kill
// `system_quit_dialog_handlers.rs` performs on a confirmed Return to Desktop and
// `system_quit_ownership_repro.rs` performs on the quit teardown. The in-world path requests a
// character save first; there is nothing to save here, which is the premise of the whole flow.
//
// ============================================================================================
// WHY THE PICKER THREAD PERFORMS THE EXIT ITSELF (corrected by run pr109-boot-oscancel-20260730-110704)
// ============================================================================================
//
// The first version handed the exit to the GAME TASK, because that task holds `EffectsState` and
// could therefore flush the telemetry carrying the cancel-exit oracle before the process ended --
// "a counter set and then immediately abandoned by `ExitProcess` proves nothing". The reasoning was
// sound and the mechanism was inapplicable, which the first live run showed in one line:
//
//     [+33804ms] the game task did not perform the cancel-exit within 5s -- quitting from the
//                picker thread instead
//
// The hand-off did not merely lose a race; it never worked, and the 5s "backstop" was the only path
// that ever ran. Measured from that run's artifacts:
//
//   * the game task WAS registered and ticking -- `bootstrap.jsonl` records
//     `game_task_recurring_registered`, and the debug log shows it reaching tick 60 at +16806ms,
//     12.5 seconds AFTER the dialog opened. So "the task never ticks at a missing-save boot" is
//     FALSE and was not the cause.
//   * it then stopped. The final telemetry reports `game_task_ticks = 58` and
//     `savelike_opens = 0` while the dialog's own CLOSED line reports `savelike_opens = 898`: the
//     file had gone stale at ~tick 58 / ~+16.8s. Between +16950ms and the cancel at +28790ms the
//     debug log contains nothing but comdlg32's own window churn -- 11.8s with no game thread
//     activity at all.
//   * so by the time the user answered the dialog, the task had been silent for 12 seconds. It was
//     never going to perform the exit, and waiting 5s for it only made the user watch a dead game.
//
// WHY the task stopped is NOT established and is deliberately not guessed at here: the game booted
// normally for the first 12.7s with the dialog already open, so the dialog's mere existence is not
// the cause. `SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN`/`_AT_ANSWER` were added to measure it directly on
// the next run rather than to argue about it on this one.
//
// The correction: this thread -- the one that is demonstrably alive, because it just returned from
// comdlg32 -- owns the whole path. It sets the counters, records the outcome on the lock-free
// bootstrap-event channel, ATTEMPTS the telemetry flush with `try_lock` (a frozen task holding the
// state mutex costs a stale file, never a hung quit), and exits. No hand-off, no 5s wait.

pub(crate) use er_save_picker::{
    BOOT_PICKER_CANCEL_EXIT, BOOT_PICKER_FELL_BACK, BOOT_PICKER_IDLE, BOOT_PICKER_OPEN,
    BOOT_PICKER_PICKED, BootAbortAction, boot_abort_action,
};

pub(crate) use er_telemetry::counters::GAME_TASK_TICKS_TOTAL;
pub(crate) use er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER;
pub(crate) use er_telemetry::counters::SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN;
pub(crate) use er_telemetry::counters::SAVE_PICKER_BOOT_TELEMETRY_FLUSHED;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_DEFER_TICKS;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_EXIT_PERFORMED;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_FALLBACK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_STATE;

/// Flush attempts the picker thread makes before quitting with a stale file.
///
/// Small on purpose. The ordinary case is the game task holding the state mutex for the microseconds
/// of its own tick, which one or two tries cover; the pathological case is a task that has stopped
/// giving the mutex back, and no number of retries fixes that -- it only delays the user's quit.
pub(crate) const BOOT_EXIT_FLUSH_ATTEMPTS: usize = 4;

/// How long the OS arm waits for the core `CreateFileW` detour before handing the pick to the
/// in-game browser.
///
/// A TIME bound, not a tick count, because the two threads that call in tick at wildly different
/// rates -- Present starves to ~4 fps during boot asset streaming while the game task runs at
/// frame rate -- so a tick budget would mean two different waits. The detour is installed from a
/// thread spawned in `DllMain` and is normally live within a second; this is the backstop for the
/// case where it never is, and its expiry is a FALLBACK rather than a failure, so the user still
/// gets a picker.
pub(crate) const BOOT_OS_CORE_HOOK_WAIT: Duration = Duration::from_secs(20);

/// When the OS arm first found the core detour not yet live.
pub(crate) static BOOT_OS_CORE_HOOK_WAIT_STARTED: OnceLock<Instant> = OnceLock::new();
/// One-shot guard for the dialog thread.
pub(crate) static BOOT_OS_THREAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Open the boot missing-save picker on whichever surface the config asks for, ONCE.
///
/// Safe to call from every tick of every thread that reaches the boot picker: the pending gate and
/// the state latch make the second and every later call a no-op. `IDLE` is restored when an arm
/// reports it did not take the pick over, which is what turns "the core detour is not live yet"
/// into a retry instead of a dropped picker.
pub(crate) fn boot_open_missing_save_picker_if_pending() {
    if !missing_save_selection_pending() {
        return;
    }
    // COMPARE-EXCHANGE, not a load-then-store. Present and the game task can both be inside this
    // function at once (the native-Windows build drives the picker from the game task while the
    // Present hook may also be installed), and two dialogs at a boot with one screen is exactly
    // the reopen shape this design exists to prevent.
    if SAVE_PICKER_OS_BOOT_STATE
        .compare_exchange(
            BOOT_PICKER_IDLE,
            BOOT_PICKER_OPEN,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        return;
    }
    // `request_discharged()` IS the "did a surface take this over" question, spelled the way the
    // System>Quit menu pump spells it. The boot arms can only answer `Opened` or `NotOpened` (a
    // boot cancel is handled on the picker's own thread and never returns through the router), so
    // the only thing that releases the latch here is "nobody owns the pick yet" -- which is exactly
    // the case that must be re-asked next tick, and is how the OS arm waits for the core
    // `CreateFileW` detour without either spinning a dialog or stranding the boot.
    if !unsafe { open_picker_for_intent(PickerOpenRequest::MissingSaveBoot) }.request_discharged() {
        SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_IDLE, Ordering::SeqCst);
    }
}

/// OS-mode BOOT source: pick a save container through the OS dialog, on a thread of ours.
///
/// Returns whether the pick has been taken over. `false` means "not yet" -- the core `CreateFileW`
/// detour has not gone live -- and the caller retries on its next tick.
pub(crate) fn boot_os_open_missing_save_picker() -> bool {
    // H4, hoisted OUT of `os_pick_validated` on purpose. Inside, a not-yet-live detour is a
    // `NotOpened` abort -- and the dialog thread has no next tick to spend it on, so it would fall
    // straight through to the in-game browser the very first time the picker armed, which is nearly
    // always: the boot picker arms within milliseconds of `DllMain` while the detour installs from a
    // freshly spawned thread. Checking HERE, on a caller that IS re-entered every tick, makes the
    // normal case a retry and keeps the fallback for a detour that never arrives at all.
    if !crate::experiments::save_file_core_hooks_live() {
        let waited = BOOT_OS_CORE_HOOK_WAIT_STARTED
            .get_or_init(Instant::now)
            .elapsed();
        SAVE_PICKER_OS_BOOT_DEFER_TICKS.fetch_add(1, Ordering::SeqCst);
        if waited < BOOT_OS_CORE_HOOK_WAIT {
            return false;
        }
        return boot_os_fall_back_to_in_game(&format!(
            "the core CreateFileW detour never went live within {}s",
            BOOT_OS_CORE_HOOK_WAIT.as_secs()
        ));
    }
    if BOOT_OS_THREAD_STARTED.swap(true, Ordering::SeqCst) {
        // The thread is already up and owns the state; report the pick as taken so the caller does
        // not release the latch out from under it.
        return true;
    }
    let spawned = std::thread::Builder::new()
        .name("er-effects-boot-os-picker".to_owned())
        .spawn(|| {
            // A panic must not leave the boot latched OPEN with nothing behind it, so the bail-out
            // hands the pick to the in-game browser.
            if std::panic::catch_unwind(boot_os_picker_thread).is_err() {
                boot_os_fall_back_to_in_game("the boot OS picker thread panicked");
            }
        });
    if spawned.is_err() {
        BOOT_OS_THREAD_STARTED.store(false, Ordering::SeqCst);
        return boot_os_fall_back_to_in_game("the boot OS picker thread could not be spawned");
    }
    true
}

/// The dialog, and the one decision each outcome leads to. Runs on `er-effects-boot-os-picker`.
pub(crate) fn boot_os_picker_thread() {
    // Same flavor filter and the same source of it as the in-game boot arm, so the two surfaces
    // cannot offer different containers at the same boot.
    let seamless = save_picker_seamless_mode_after_settle("boot-os-picker");
    let extensions: &[&str] = if seamless { &["co2", "sl2"] } else { &["sl2"] };
    // Same start directory as the in-game boot arm, for the same reason.
    let start_dir = crate::experiments::save_picker_title_start_dir();
    let Some(start_dir) = start_dir.to_str().map(save_picker_windows_path_string) else {
        boot_os_fall_back_to_in_game("the boot start directory is not representable as text");
        return;
    };
    SAVE_PICKER_OS_BOOT_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OPEN_COUNT.fetch_add(1, Ordering::SeqCst);
    // Sample the game task's free-running beat on BOTH sides of the blocking call. Taken here by a
    // thread that is provably alive, so the pair measures the GAME's liveness across the dialog
    // rather than our own -- the question the first live run could not answer.
    SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN.store(
        GAME_TASK_TICKS_TOTAL.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    // The staging is done AFTER the claim drops, not inside the closure: it enumerates a directory
    // and reads every candidate container, and unlike the System>Quit arms there is no save-flow
    // tick reading `SAVE_PICKER_OS_DIALOG_OPEN` as a "browser is live" term at boot, so nothing
    // needs the claim held across it. The closure only copies the path out.
    let picked = os_pick_validated(
        false,
        start_dir,
        "",
        extensions,
        &er_save_picker::PickerIntent::LoadSource,
        no_picker_cover,
        str::to_owned,
    );
    SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER.store(
        GAME_TASK_TICKS_TOTAL.load(Ordering::SeqCst),
        Ordering::SeqCst,
    );
    let ticks_at_open = SAVE_PICKER_BOOT_GAME_TICKS_AT_OPEN.load(Ordering::SeqCst);
    let ticks_at_answer = SAVE_PICKER_BOOT_GAME_TICKS_AT_ANSWER.load(Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-boot-os: game task ticked {} times while the dialog was up (open={ticks_at_open} answer={ticks_at_answer}); 0 means the game was frozen for the dialog's whole life",
        ticks_at_answer.saturating_sub(ticks_at_open)
    ));
    match picked {
        Ok(path) => boot_os_stage_pick(PathBuf::from(path)),
        Err(abort) => match boot_abort_action(abort) {
            BootAbortAction::QuitGame => boot_os_perform_cancel_exit(),
            BootAbortAction::FallBackToInGame => {
                boot_os_fall_back_to_in_game(&format!(
                    "the OS dialog produced no usable answer ({abort:?})"
                ));
            }
        },
    }
}

/// Hand an accepted container to the CHARACTER sub-picker -- the same second stage the in-game arm
/// reaches after its own file pick.
///
/// Not optional, and not symmetry for its own sake. `native_fullread_slot()` reads the slot the
/// sub-picker records; with none recorded it falls through to the configured autoload slot and
/// finally to slot 0, and a picked container whose slot 0 is empty then trips the save watchdog's
/// `process::abort()`. Choosing the file in the OS dialog and the character in the overlay is what
/// keeps "the OS dialog picked my save" from meaning "the game loaded a different character".
///
/// The overlay is armed with a browser rooted at the picked file's own folder, so the sub-picker's
/// existing BACK lands somewhere real. That is a deliberate hand-off to the other surface rather
/// than a re-open of this one: comdlg32 has no character list, and reopening it on BACK is the
/// reopen loop this whole design refuses.
pub(crate) fn boot_os_stage_pick(path: PathBuf) {
    if !crate::experiments::boot_stage_picked_save_for_character_choice(path.clone()) {
        // The predicate accepted it and the slot parse then found nothing -- the two disagree, so
        // there is nothing to choose from. Back to the in-game browser rather than a dead stage.
        boot_os_fall_back_to_in_game(&format!(
            "'{}' cleared the pick predicate but yielded no character slots",
            path.display()
        ));
        return;
    }
    SAVE_PICKER_OS_BOOT_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_PICK_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_PICKED, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-boot-os: accepted '{}'; the character sub-picker owns the rest and the game task completes the pick",
        path.display()
    ));
}

/// Hand the boot pick to the in-game browser. Terminal for the OS arm: the user still gets a
/// picker, so this is a degraded surface rather than a failed boot.
pub(crate) fn boot_os_fall_back_to_in_game(reason: &str) -> bool {
    SAVE_PICKER_OS_BOOT_FALLBACK_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_FELL_BACK, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-boot-os: falling back to the in-game browser -- {reason}"
    ));
    crate::experiments::boot_arm_missing_save_picker_in_game()
}

/// Record the cancel durably, then quit. Runs entirely on the picker thread.
///
/// ORDER IS THE WHOLE DESIGN, because every step after the first has to survive the possibility
/// that the game is already dead:
///
///  1. **counters** -- so any flush that does happen carries the outcome;
///  2. **bootstrap event** -- append-only, lock-free, no game state touched. This is the record that
///     CANNOT fail: it is the same channel `bootstrap.jsonl` already uses from `DllMain` and from
///     spawned threads, and the harness preserves it alongside the telemetry;
///  3. **telemetry flush, `try_lock`** -- the nice-to-have. Success makes the main oracle file
///     describe the cancel; failure records itself as `boot_telemetry_flushed = 0` and costs
///     nothing, because step 2 already told the story;
///  4. **`ExitProcess(0)`**.
///
/// `ExitProcess(0)` is the product's own quit: `system_quit_dialog_handlers.rs` uses it for a
/// confirmed Return to Desktop and `system_quit_ownership_repro.rs` for the quit teardown, both
/// deliberately in preference to the native quit (which rebuilds a title whose `MenuOffscrRendParam`
/// table the teardown has unloaded, and `DLPanic`s). The in-world path requests a character save
/// first and releases the cursor clip; here there is no character to save -- that is the premise of
/// a missing-save boot -- so only the input release carries over.
pub(crate) fn boot_os_perform_cancel_exit() -> ! {
    SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_CANCEL_EXIT, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-boot-os: the user CANCELLED the boot save picker -- at a missing-save boot that means quit (OK -> choose a save, Cancel -> exit)"
    ));
    // THE RECORD THAT CANNOT FAIL. Written before the flush is attempted, because the flush is the
    // part that depends on a thread which may already be gone.
    write_bootstrap_event(BOOTSTRAP_EVENT_BOOT_PICKER_CANCEL_EXIT, "cancelled");
    let flushed = try_write_telemetry_off_game_task(false, BOOT_EXIT_FLUSH_ATTEMPTS);
    SAVE_PICKER_BOOT_TELEMETRY_FLUSHED.store(usize::from(flushed), Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_EXIT_PERFORMED.store(1, Ordering::SeqCst);
    // Flush once more when the first attempt worked, so the file also carries `exit_performed = 1`
    // and `telemetry_flushed = 1` rather than the values from one step earlier. Skipped entirely
    // when the lock was unavailable -- retrying a lock nobody is releasing only delays the quit.
    if flushed {
        let _ = try_write_telemetry_off_game_task(false, 1);
    }
    write_bootstrap_event(
        BOOTSTRAP_EVENT_BOOT_PICKER_CANCEL_EXIT,
        if flushed {
            "exiting_telemetry_fresh"
        } else {
            "exiting_telemetry_stale"
        },
    );
    release_input_block_now();
    append_autoload_debug(format_args!(
        "save-picker-boot-os: quitting -- released the input block and calling ExitProcess(0) (no character was ever loaded, so there is nothing to save and no world to tear down). telemetry_flushed={flushed}{}",
        if flushed {
            ""
        } else {
            " -- the state mutex was not available, so er-effects-telemetry.json predates this cancel; er-effects-bootstrap.jsonl carries the outcome"
        }
    ));
    // ExitProcess never returns, so it IS the `!` tail expression.
    unsafe { windows::Win32::System::Threading::ExitProcess(0) }
}

#[cfg(test)]
mod save_picker_boot_tests {
    use super::*;
    use std::time::Duration;

    /// The detour wait is finite, and the CANCEL PATH HAS NO WAIT AT ALL.
    ///
    /// The second half is the point. The first version of this file made the user's quit depend on
    /// the game task performing it, with a 5s bound; the first live run showed the task had been
    /// silent for 12 seconds by then, so the bound was not a backstop but the only path, and its
    /// only effect was 5 seconds of dead game after the user pressed Cancel. The cancel path now
    /// runs entirely on the thread that just returned from comdlg32, and the only bounded thing left
    /// on it is a `try_lock` flush attempt whose failure is recorded rather than waited out.
    #[test]
    fn the_cancel_path_waits_for_nothing() {
        assert!(BOOT_OS_CORE_HOOK_WAIT > Duration::ZERO);
        const {
            assert!(
                BOOT_EXIT_FLUSH_ATTEMPTS >= 1,
                "at least one flush attempt, or a healthy run would never refresh the file"
            );
            assert!(
                BOOT_EXIT_FLUSH_ATTEMPTS <= 8,
                "the flush is best-effort; retrying a lock nobody releases only delays the user's quit"
            );
        }
    }
}
