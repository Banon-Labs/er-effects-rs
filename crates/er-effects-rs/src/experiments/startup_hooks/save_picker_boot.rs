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
// the boot bar keeps animating, and `PickerDim::None` follows from that: there is no frozen game
// for a cover to account for.
//
// UNPROVEN, AND SAID PLAINLY: no runtime evidence exists that comdlg32 renders and takes input
// from a non-UI thread under this Wine/Proton build. The reasoning is that a common dialog is a
// real window with its own message queue on its creating thread, which is a different mechanism
// from the `GetAsyncKeyState` polling the bd memory above measured failing off the render thread.
// If it turns out comdlg32 needs a specific thread here, the failure is visible without a
// screenshot: `oracle_save_picker_os_open_count` advances with `oracle_save_picker_os_boot_state`
// stuck at OPEN and no CLOSED line in the debug log.
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
// It runs on the GAME TASK rather than on this thread, for one reason: the task holds
// `EffectsState`, so it can flush the telemetry file that carries the cancel-exit oracle before
// the process ends. A counter set and then immediately abandoned by `ExitProcess` proves nothing.

/// Nothing has opened a boot picker (or this is not a missing-save boot).
pub(crate) const BOOT_PICKER_IDLE: usize = 0;
/// A surface owns the boot pick and is waiting on the user.
pub(crate) const BOOT_PICKER_OPEN: usize = 1;
/// A file cleared the shared validity predicate; the character sub-picker owns the rest.
pub(crate) const BOOT_PICKER_PICKED: usize = 2;
/// The user cancelled the boot OS dialog; the game is quitting.
pub(crate) const BOOT_PICKER_CANCEL_EXIT: usize = 3;
/// comdlg32 was unusable; the in-game browser took the pick over.
pub(crate) const BOOT_PICKER_FELL_BACK: usize = 4;

pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_DEFER_TICKS;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_EXIT_PENDING;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_EXIT_PERFORMED;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_FALLBACK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_OPEN_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_PICK_COUNT;
pub(crate) use er_telemetry::counters::SAVE_PICKER_OS_BOOT_STATE;

/// How long the OS arm waits for the core `CreateFileW` detour before handing the pick to the
/// in-game browser.
///
/// A TIME bound, not a tick count, because the two threads that call in tick at wildly different
/// rates -- Present starves to ~4 fps during boot asset streaming while the game task runs at
/// frame rate -- so a tick budget would mean two different waits. The detour is installed from a
/// thread spawned in `DllMain` and is normally live within a second; this is the backstop for the
/// case where it never is, and its expiry is a FALLBACK rather than a failure, so the user still
/// gets a picker.
const BOOT_OS_CORE_HOOK_WAIT: Duration = Duration::from_secs(20);

/// How long the picker thread waits for the game task to perform a requested cancel-exit before
/// performing it itself.
///
/// The hand-off exists so the telemetry flush happens; it must not become a new way to strand the
/// boot. If the game task is not ticking, quitting late beats not quitting -- the user pressed
/// Cancel on a screen whose only other outcome is a title they cannot leave.
const BOOT_OS_EXIT_HANDOFF_WAIT: Duration = Duration::from_secs(5);

/// When the OS arm first found the core detour not yet live.
static BOOT_OS_CORE_HOOK_WAIT_STARTED: OnceLock<Instant> = OnceLock::new();
/// One-shot guard for the dialog thread.
static BOOT_OS_THREAD_STARTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// What an abandoned boot open means. PURE and separated from the code that acts on it, so the one
/// decision that can terminate the process is checkable by a test rather than by reading a thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BootAbortAction {
    /// The user decided. Quit the game.
    QuitGame,
    /// We could not ask. Hand the pick to the in-game browser instead of acting on a choice
    /// nobody made.
    FallBackToInGame,
}

/// Map an abandoned OS open onto the boot intent's response.
///
/// ONLY a genuine user cancel quits. A comdlg32 failure, a refused re-entrant open and an
/// exhausted reopen bound all mean the dialog could not be used, and terminating a user's game
/// over a defect in a file dialog is the opposite of what the cancel contract promises.
pub(crate) fn boot_abort_action(abort: OsPickAbort) -> BootAbortAction {
    match abort {
        OsPickAbort::Cancelled => BootAbortAction::QuitGame,
        OsPickAbort::Unavailable => BootAbortAction::FallBackToInGame,
    }
}

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
    let taken = unsafe { open_picker_for_intent(PickerOpenRequest::MissingSaveBoot) };
    if !taken {
        SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_IDLE, Ordering::SeqCst);
    }
}

/// OS-mode BOOT source: pick a save container through the OS dialog, on a thread of ours.
///
/// Returns whether the pick has been taken over. `false` means "not yet" -- the core `CreateFileW`
/// detour has not gone live -- and the caller retries on its next tick.
pub(crate) fn boot_os_open_missing_save_picker() -> bool {
    // H4, hoisted OUT of `os_pick_validated` on purpose. Inside, a not-yet-live detour is an
    // `Unavailable` abort, and at boot that would fall straight through to the in-game browser the
    // very first time the picker armed -- which is nearly always, because the boot picker arms
    // within milliseconds of `DllMain` while the detour installs from a freshly spawned thread.
    // Checking here makes the normal case a retry and keeps the fallback for a detour that never
    // arrives at all.
    if !crate::experiments::save_file_core_hooks_live() {
        let waited = BOOT_OS_CORE_HOOK_WAIT_STARTED.get_or_init(Instant::now).elapsed();
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
fn boot_os_picker_thread() {
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
    // The staging is done AFTER the claim drops, not inside the closure: it enumerates a directory
    // and reads every candidate container, and unlike the System>Quit arms there is no save-flow
    // tick reading `SAVE_PICKER_OS_DIALOG_OPEN` as a "browser is live" term at boot, so nothing
    // needs the claim held across it. The closure only copies the path out.
    let picked = os_pick_validated(
        false,
        start_dir,
        "",
        extensions,
        &crate::experiments::save_picker::PickerIntent::LoadSource,
        PickerDim::None,
        str::to_owned,
    );
    match picked {
        Ok(path) => boot_os_stage_pick(PathBuf::from(path)),
        Err(abort) => match boot_abort_action(abort) {
            BootAbortAction::QuitGame => boot_os_request_cancel_exit(),
            BootAbortAction::FallBackToInGame => {
                boot_os_fall_back_to_in_game(&format!("comdlg32 was unusable ({abort:?})"));
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
fn boot_os_stage_pick(path: PathBuf) {
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
fn boot_os_fall_back_to_in_game(reason: &str) -> bool {
    SAVE_PICKER_OS_BOOT_FALLBACK_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_FELL_BACK, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-boot-os: falling back to the in-game browser -- {reason}"
    ));
    crate::experiments::boot_arm_missing_save_picker_in_game()
}

/// Ask for the cancel-exit, then make sure it happens.
///
/// The request goes to the game task so the telemetry that PROVES this outcome is written before
/// the process ends. The bounded wait afterwards is a backstop, not synchronization: if the task
/// is not ticking, this thread performs the exit itself rather than leaving the user on a title
/// they cannot leave.
fn boot_os_request_cancel_exit() {
    SAVE_PICKER_OS_BOOT_CANCEL_EXIT_COUNT.fetch_add(1, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_STATE.store(BOOT_PICKER_CANCEL_EXIT, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_EXIT_PENDING.store(1, Ordering::SeqCst);
    append_autoload_debug(format_args!(
        "save-picker-boot-os: the user CANCELLED the boot save picker -- at a missing-save boot that means quit (OK -> choose a save, Cancel -> exit). Handing the exit to the game task so the telemetry flush lands first"
    ));
    // Held-but-never-sent channel: `recv_timeout` is this repo's sanctioned bounded wait (the dim
    // overlay and the window observer pace themselves the same way) and is not a sleep used as
    // synchronization -- the loop exits the moment the game task publishes the exit.
    let (_pace_tx, pace_rx) = std::sync::mpsc::channel::<()>();
    let deadline = Instant::now() + BOOT_OS_EXIT_HANDOFF_WAIT;
    while Instant::now() < deadline {
        if SAVE_PICKER_OS_BOOT_EXIT_PERFORMED.load(Ordering::SeqCst) != 0 {
            return;
        }
        let _ = pace_rx.recv_timeout(Duration::from_millis(25));
    }
    append_autoload_debug(format_args!(
        "save-picker-boot-os: the game task did not perform the cancel-exit within {}s -- quitting from the picker thread instead (the telemetry file will not carry the final flush)",
        BOOT_OS_EXIT_HANDOFF_WAIT.as_secs()
    ));
    boot_os_perform_cancel_exit();
}

/// True while a boot cancel-exit is owed. Read by the game task, which owns the flush.
pub(crate) fn boot_os_cancel_exit_requested() -> bool {
    SAVE_PICKER_OS_BOOT_EXIT_PENDING.load(Ordering::SeqCst) != 0
}

/// Quit. Call ONLY after the telemetry flush (the game task's path) or from the backstop above.
///
/// `ExitProcess(0)` is the product's own quit: `system_quit_dialog_handlers.rs` uses it for a
/// confirmed Return to Desktop and `system_quit_ownership_repro.rs` for the quit teardown, both
/// deliberately in preference to the native quit (which rebuilds a title whose `MenuOffscrRendParam`
/// table the teardown has unloaded, and `DLPanic`s). The in-world path requests a character save
/// first and releases the cursor clip; here there is no character to save -- that is the premise of
/// a missing-save boot -- so only the input release carries over.
pub(crate) fn boot_os_perform_cancel_exit() -> ! {
    SAVE_PICKER_OS_BOOT_EXIT_PERFORMED.store(1, Ordering::SeqCst);
    SAVE_PICKER_OS_BOOT_EXIT_PENDING.store(0, Ordering::SeqCst);
    release_input_block_now();
    append_autoload_debug(format_args!(
        "save-picker-boot-os: quitting -- released the input block and calling ExitProcess(0) (no character was ever loaded, so there is nothing to save and no world to tear down)"
    ));
    unsafe { windows::Win32::System::Threading::ExitProcess(0) };
    // ExitProcess never returns; this is unreachable and only satisfies the `!` return type.
    unreachable!("ExitProcess(0) does not return")
}

#[cfg(test)]
mod save_picker_boot_tests {
    use super::*;

    /// THE decision that can terminate a user's game, pinned. Only a cancel -- the one outcome
    /// that IS a user decision -- quits; every "we could not ask" outcome falls back to the
    /// in-game browser instead of acting on a choice nobody made.
    #[test]
    fn only_a_user_cancel_quits_the_game() {
        assert_eq!(
            boot_abort_action(OsPickAbort::Cancelled),
            BootAbortAction::QuitGame
        );
        assert_eq!(
            boot_abort_action(OsPickAbort::Unavailable),
            BootAbortAction::FallBackToInGame,
            "a comdlg32 defect must never terminate the process"
        );
    }

    /// Every boot state is distinct. They are exported as one telemetry field, so a collision
    /// would make two different outcomes indistinguishable in the only record that survives the
    /// process -- and one of those outcomes is a quit.
    #[test]
    fn the_boot_states_are_distinguishable_in_telemetry() {
        let states = [
            BOOT_PICKER_IDLE,
            BOOT_PICKER_OPEN,
            BOOT_PICKER_PICKED,
            BOOT_PICKER_CANCEL_EXIT,
            BOOT_PICKER_FELL_BACK,
        ];
        for (index, state) in states.iter().enumerate() {
            for other in &states[index + 1..] {
                assert_ne!(state, other, "two boot states share a telemetry value");
            }
        }
        assert_eq!(
            BOOT_PICKER_IDLE, 0,
            "a session that never reaches a missing-save boot must read as IDLE"
        );
    }

    /// The bounds are finite and ordered: the backstop that quits from the picker thread must be
    /// shorter than the wait for a detour that may never arrive, or a user who cancelled during
    /// the detour wait would sit through both.
    #[test]
    fn both_boot_waits_are_finite() {
        assert!(BOOT_OS_CORE_HOOK_WAIT > Duration::ZERO);
        assert!(BOOT_OS_EXIT_HANDOFF_WAIT > Duration::ZERO);
        assert!(BOOT_OS_EXIT_HANDOFF_WAIT < BOOT_OS_CORE_HOOK_WAIT);
    }
}
