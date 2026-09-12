//! Who needs the `05_010_ProfileSelect` chrome, as a pure decision.
//!
//! Its own module beside [`crate::menu_window_run_install`], and outside the `#[cfg(windows)]` half
//! of this crate, for the same reason: the gate it replaces removed a user-facing surface without
//! failing to build and without logging anything, so the decision earned a place a host test can
//! reach.
//!
//! # What the chrome is
//!
//! Two things arm together in `experiments::lifecycle::title_visual_startup`, under one
//! `START_PROFILE_STATS_TEXT` block:
//!
//! * `PROFILE_05_010_RUNTIME_EDIT_ARMED`, which is the only thing
//!   `profile_table_gfx_files::profile_05_010_swap_to_edited` checks before it serves the edited
//!   movie in place of the vanilla one at Scaleform file-open. The edit removes the face box and
//!   adds the `ErStats` field the character's attribute line is drawn into.
//! * `install_profile_row_populate_hook`, which pushes each slot's attributes into that field.
//!
//! Neither is optional for the three System>Quit rows: **Load Character**, **Load Character from
//! File** and **Save Game** all stand on `05_010_ProfileSelect`, and all three reach it through the
//! same movie the title's own `LOAD GAME` opens.
//!
//! # The regression
//!
//! Until 2026-09-11 the gate asked only `autoload_disabled()`. That was sound while this source was
//! `er-quickload`, where the boot autoload was the only consumer that existed. When `autoload` left
//! this shell's default feature set in commit `16db29bf`, `autoload_disabled()` became a
//! compile-time true and the block never ran, so the movie was passed through vanilla.
//!
//! Measured in `er-quit-rows-debug.log`, 2026-09-11 17:25, build `b9fe6da3`. The Quit tab's own grid
//! edit served normally -- `system-quit-gfx: 02_040 quit6 runtime edit derived in=44016 out=44107`
//! -- and the file-open observer saw the other movie go past on the same boot,
//! `file-open title-memory label=05_010_profileselect total=70`, with no edit line beside it. Three
//! Load Character presses later in the same session opened `05_010_ProfileSelect` three times and
//! the log carries no `05_010` edit line at all. Same commit, same trim, same shape as the menu
//! pump it sits next to: the code was there, the switch was not.
//!
//! # Telemetry-only is not a consumer
//!
//! `save_override_telemetry_only()` means no character is loaded at all, so there are no slots to
//! draw attributes for. It refuses the chrome regardless of who wants it, which is why it is a
//! separate term rather than another consumer.

/// Whether this run serves the edited `05_010_ProfileSelect` movie and pushes row attributes into
/// it.
///
/// Either consumer is reason enough and neither can speak for the other: `boot_autoload` reaches
/// the movie through the title's own `LOAD GAME`, `quit_rows` reaches it from the System>Quit tab.
/// `telemetry_only` is not a consumer but a refusal -- a run with no character loaded has nothing
/// to render -- so it wins over both.
#[must_use]
pub fn profile_select_chrome_required(
    boot_autoload: bool,
    quit_rows: bool,
    telemetry_only: bool,
) -> bool {
    !telemetry_only && (boot_autoload || quit_rows)
}

#[cfg(test)]
mod profile_select_chrome_gate_tests {
    use super::profile_select_chrome_required;

    /// The regression this predicate exists for: a shell that compiles the boot autoload out still
    /// carries the three System>Quit rows, and all three stand on `05_010_ProfileSelect`. Asking
    /// only the autoload served the vanilla movie to a menu built for the edited one.
    #[test]
    fn the_quit_rows_keep_the_chrome_when_the_boot_autoload_is_compiled_out() {
        assert!(profile_select_chrome_required(false, true, false));
    }

    /// The other consumer on its own, so the two terms cannot be collapsed back into one.
    #[test]
    fn the_boot_autoload_arms_it_without_the_quit_rows() {
        assert!(profile_select_chrome_required(true, false, false));
    }

    /// Both consumers is still one arm: `START_PROFILE_STATS_TEXT` is a `Once`.
    #[test]
    fn both_consumers_want_the_same_single_arm() {
        assert!(profile_select_chrome_required(true, true, false));
    }

    /// A build with neither consumer serves the vanilla movie, which is the correct answer when
    /// nothing in the process will ever open it.
    #[test]
    fn a_build_with_neither_consumer_arms_nothing() {
        assert!(!profile_select_chrome_required(false, false, false));
    }

    /// Telemetry-only loads no character, so it refuses over either consumer and over both.
    #[test]
    fn telemetry_only_refuses_whoever_is_asking() {
        for boot_autoload in [false, true] {
            for quit_rows in [false, true] {
                assert!(
                    !profile_select_chrome_required(boot_autoload, quit_rows, true),
                    "boot_autoload={boot_autoload} quit_rows={quit_rows}"
                );
            }
        }
    }

    /// This shell always arms the rows, so it always wants the chrome -- whatever the boot autoload
    /// is doing. Pins the pairing the gate actually evaluates, not just the predicate.
    #[test]
    fn this_shell_wants_the_chrome_whatever_the_boot_autoload_does() {
        for boot_autoload in [false, true] {
            assert!(
                profile_select_chrome_required(
                    boot_autoload,
                    crate::menu_window_run_install::quit_rows_armed(),
                    false
                ),
                "boot_autoload={boot_autoload}"
            );
        }
    }
}
