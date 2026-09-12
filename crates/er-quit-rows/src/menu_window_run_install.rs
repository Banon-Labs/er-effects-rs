//! Who needs the detour at `PAB_NODE_UPDATE_RVA`, as a pure decision.
//!
//! Its own module, and outside the `#[cfg(windows)]` half of this crate, so the decision runs under
//! `cargo test -p er-quit-rows` on the host. Every other gate in `experiments::gating` sits behind
//! that cfg and is only ever type-checked by the cross-compile; this one earned a place a test can
//! reach, because getting it wrong removed a user-facing surface without failing to build and
//! without logging anything.
//!
//! # One address, two consumers
//!
//! `MENU_WINDOW_JOB_RUN_RVA == PAB_NODE_UPDATE_RVA == 0x7ad1c0`, and `pab_node_update_detour` does
//! two unrelated jobs there. It calls `pab_advance_try`, the boot autoload's readiness-gated
//! press-any-button advance, which gates itself on `pab_advance_enabled` and so costs one atomic
//! read in a build that does not want it. Then it calls `system_quit_menu_window_run_post`, which is
//! the game's own menu pump frame and is where the System>Quit rows do the work they cannot do
//! anywhere else.
//!
//! That second half is not a detail. It latches the `02_000_IngameTop`, `02_040_OptionSetting` and
//! `05_010_ProfileSelect` windows, hides the first two behind the third, submits the return-title
//! chain that finishes a character switch, opens the Save Game destination browser and pumps the
//! save-flow confirm boxes.
//!
//! # The regression
//!
//! Until 2026-09-11 the install asked only `pab_advance_enabled()`. That was sound while MinHook
//! bound one detour per address and the title-advance install deterministically won the slot, which
//! is the 2026-07-15 arrangement the passenger call came from. `er-hook` unions handlers now, so the
//! slot is no longer scarce and the coupling was all that was left of the reason.
//!
//! It cost a surface when `autoload` left this shell's default feature set. `autoload_disabled()` is
//! then a compile-time true, `pab_advance_enabled()` a compile-time false, and the detour was never
//! registered -- so `system_quit_menu_window_run_post` never ran once in a session.
//!
//! Measured in `er-quit-rows-debug.log`, 2026-09-11 17:00:14, on a Load Character press. The job
//! was built and submitted exactly as designed. Three lines in sequence, each quoted whole:
//! `profile-load route FIRE 05_010_ProfileSelect wrapper 0x140820570`, then
//! `profile-load route SUBMIT job=0x973ee540`, then
//! `ProfileSelect append observed ... count=2 ... appended_window=0x8ee0`, with the row itself
//! reporting `opened=true`. The same 638-line session carries zero `pab-run-post` lines, zero
//! `MenuWindowJob::Run resource=` lines and zero `real-system-window hide` lines. The picker
//! existed and was in the System dialog's window list; the pause menu in front of it was never
//! hidden and never stopped taking input.
//!
//! The same absent pump shows from two further sides in that log: the Save Game row's close-all
//! reports `option=0x0 top=0x0` because those latches are written in the pump too, and a Save Game
//! press stages stage 3 and then gives up with `destination browser never opened after 180 ticks`.

/// Whether the detour at `PAB_NODE_UPDATE_RVA` has to be installed this run.
///
/// Either consumer is reason enough, and neither can speak for the other: `boot_autoload` is the
/// press-any-button advance, `quit_rows` is the menu pump body above. Installing for the rows does
/// not resurrect the boot advance -- `pab_advance_try` returns at its own `pab_advance_enabled()`
/// check before it reads anything.
#[must_use]
pub fn menu_window_job_run_hook_required(boot_autoload: bool, quit_rows: bool) -> bool {
    boot_autoload || quit_rows
}

/// Whether this shell arms the System>Quit rows.
///
/// Unconditional, and stated here rather than as a literal `true` at the install site so the claim
/// has one home. The arm call is spawned with no gate at all (`install_system_quit_duplicate_button_hook`,
/// from `experiments::lifecycle::hook_installers`), and the `quit-rows` feature names the rows
/// without gating them -- so reading `cfg!(feature = "quit-rows")` here would tie the menu pump to a
/// flag that does not decide whether the rows exist, which is the same class of mistake as tying it
/// to `autoload`. If the rows ever do become conditional, this is the line that changes.
#[must_use]
pub const fn quit_rows_armed() -> bool {
    true
}

#[cfg(test)]
mod menu_window_run_install_tests {
    use super::menu_window_job_run_hook_required;

    /// The regression this predicate exists for: a shell that compiles the boot autoload out still
    /// carries the System>Quit rows, and the menu pump those rows need shares the autoload's detour
    /// address. Asking only the autoload left a submitted `05_010_ProfileSelect` drawn behind a
    /// pause menu that nothing hid.
    #[test]
    fn the_quit_rows_keep_the_menu_pump_when_the_boot_autoload_is_compiled_out() {
        assert!(menu_window_job_run_hook_required(false, true));
    }

    /// The other consumer on its own, so the two terms cannot be collapsed back into one.
    #[test]
    fn the_boot_autoload_installs_it_without_the_quit_rows() {
        assert!(menu_window_job_run_hook_required(true, false));
    }

    /// Both consumers is still one install: the detour is registered once, and the union chains
    /// whatever else claims the address.
    #[test]
    fn both_consumers_want_the_same_single_install() {
        assert!(menu_window_job_run_hook_required(true, true));
    }

    /// A build with neither consumer detours nothing: the address is the game's, and an install
    /// with no caller behind it is a trampoline under live menu threads for no reason.
    #[test]
    fn a_build_with_neither_consumer_installs_nothing() {
        assert!(!menu_window_job_run_hook_required(false, false));
    }

    /// This shell always arms the rows, so it always needs the pump -- whatever the boot autoload
    /// is doing. Pins the pairing the install site actually evaluates, not just the predicate.
    #[test]
    fn this_shell_needs_the_pump_whatever_the_boot_autoload_does() {
        for boot_autoload in [false, true] {
            assert!(
                menu_window_job_run_hook_required(boot_autoload, super::quit_rows_armed()),
                "boot_autoload={boot_autoload}"
            );
        }
    }
}
