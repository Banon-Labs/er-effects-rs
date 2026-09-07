//! The one warning, and everything that must be true for it to be worth reading.
//!
//! # Design rules, all of them learned from reports nobody reads
//!
//! * **The verdict is the first line.** A reader who stops there must still learn whether they
//!   have a problem.
//! * **A finding names the key and every module.** "There is a conflict on F7" is a riddle;
//!   "F7: er_invasion_warp.dll, er_invasion_path.dll" is an instruction.
//! * **What was not checked is printed as loudly as what was.** Parts of this DLL's coverage are
//!   structurally limited: DirectInput hands back the whole keyboard and never names the key, an
//!   API that could not be hooked is a hole, and the game's binding singleton is NULL until the
//!   game initialises it. Leaving any of those silent turns "I did not look" into "there is
//!   nothing there", which is the failure mode that lets a real conflict ship.
//! * **Identical evidence renders identical text.** Everything upstream is a `BTreeMap`, so two
//!   runs can be diffed to see whether a config change helped.

// Windows-only in practice; ungated so the rendering is covered by `cargo test`.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::census::{Census, InputId};
use crate::dik::CONSUMED_STREAK;
use crate::game_bindings::GameBindings;
use er_invasion_warp_core::keybind::key_name;

/// Everything the renderer needs. A struct rather than nine arguments, and it keeps the call site
/// readable at the one place that matters.
pub struct ReportInput<'a> {
    /// What was observed, already attributed.
    pub census: &'a Census,
    /// The game executable's own module file name.
    pub game_module: &'a str,
    /// The game's own bindings, or the reason they are not available.
    pub bindings: &'a GameBindings,
    /// Virtual keys seen held while the game's DirectInput buffer said they were up.
    pub consumed_keys: &'a [u16],
    /// Seconds of observation before the report was rendered.
    pub observed_seconds: u64,
    /// Modules the loader had mapped when the module list went stable.
    pub loaded_modules: usize,
    /// Input-API calls seen. Zero here means the hooks never fired, which is a different
    /// problem from a clean profile and must never render as one.
    pub calls_seen: u64,
    /// APIs this DLL managed to hook, and the ones it did not.
    pub surfaces_hooked: &'a [String],
    pub surfaces_missed: &'a [String],
}

/// The banner both edges of the report carry, so it can be found in a log full of other lines.
const BANNER: &str = "================ er-hotkey-conflicts ================";

/// Render the report as lines.
///
/// Returns at least the banner, the verdict and the caveats: there is no input for which this
/// produces nothing, because "nothing was printed" and "nothing was wrong" must never look alike.
pub fn render(input: &ReportInput<'_>) -> Vec<String> {
    let ReportInput {
        census,
        game_module,
        bindings,
        consumed_keys,
        observed_seconds,
        loaded_modules,
        calls_seen,
        surfaces_hooked,
        surfaces_missed,
    } = input;

    let collisions = census.key_collisions(game_module);
    let modifier_collisions = census.modifier_collisions(game_module);
    let raw_readers = census.whole_device_readers(game_module);
    let game_collisions = bindings.collisions_with(census, game_module);
    let mod_modules: Vec<&str> = census
        .modules()
        .into_iter()
        .filter(|module| module != game_module)
        .collect();

    let mut lines = vec![BANNER.to_string()];

    // The verdict, first. Everything below is the evidence for this one line.
    if *calls_seen == 0 {
        lines.push(
            "VERDICT: UNKNOWN -- not one input call was observed. This is NOT a clean profile; \
             it means no hook fired. See the surfaces line below."
                .to_string(),
        );
    } else if collisions.is_empty() && game_collisions.is_empty() {
        lines.push(format!(
            "VERDICT: no input is claimed by two mods. ({} module(s) polled input)",
            mod_modules.len()
        ));
    } else {
        lines.push(format!(
            "VERDICT: {} CONFLICT(S). Two or more mods want the same input, and only one of them \
             will get it.",
            collisions.len() + game_collisions.len()
        ));
    }

    lines.push(format!(
        "observed {observed_seconds}s across {loaded_modules} loaded module(s); {calls_seen} input \
         call(s), {} of them attributed to a module; {} distinct (module, input, api) row(s)",
        census.attributed_calls(),
        census.row_count()
    ));

    for collision in &collisions {
        let surfaces: Vec<&str> = collision
            .surfaces
            .iter()
            .map(|surface| surface.label())
            .collect();
        lines.push(format!(
            "  CONFLICT  {}  <-  {}   [{}]{}",
            collision.input.describe(),
            collision.modules.join(", "),
            surfaces.join(", "),
            if collision.game_reads_it_too {
                "  (the game reads this key too)"
            } else {
                ""
            }
        ));
    }

    for collision in &game_collisions {
        lines.push(format!(
            "  CONFLICT WITH THE GAME  {}  is bound to \"{}\" and is also taken by {}",
            collision.input.describe(),
            collision.action,
            collision.modules.join(", ")
        ));
    }

    if let GameBindings::Unavailable(reason) = bindings {
        lines.push(format!("  NOT CHECKED: {reason}"));
    }

    for collision in &modifier_collisions {
        lines.push(format!(
            "  shared modifier  {}  <-  {}   (normal: two mods using it as a modifier, not a \
             fight over a trigger)",
            collision.input.describe(),
            collision.modules.join(", ")
        ));
    }

    if !consumed_keys.is_empty() {
        let named: Vec<String> = consumed_keys
            .iter()
            .map(|vk| key_name(i32::from(*vk)))
            .collect();
        lines.push(format!(
            "  TAKEN FROM THE GAME  {}  -- held for {CONSUMED_STREAK}+ samples while the \
             DirectInput buffer the game receives said they were up. A mod is blanking them. \
             DirectInput does not say which mod; the whole-keyboard readers below are the \
             candidates.",
            named.join(", ")
        ));
    }

    for (input, modules) in &raw_readers {
        lines.push(format!(
            "  NOT CHECKED  {} read by {}  -- this API returns the whole device at once and never \
             names a key, so these modules' bindings are invisible here and CANNOT be checked for \
             collisions. Read their configs.",
            input.describe(),
            modules.join(", ")
        ));
    }

    if !surfaces_missed.is_empty() {
        lines.push(format!(
            "  NOT CHECKED  input APIs this DLL could not hook: {}. A mod binding through one of \
             them is invisible to this report.",
            surfaces_missed.join(", ")
        ));
    }
    lines.push(format!("  watching: {}", surfaces_hooked.join(", ")));

    if census.unattributed_calls() > 0 || census.dropped_calls() > 0 {
        lines.push(format!(
            "  {} call(s) could not be traced to a module and {} were dropped by a contended \
             lock; those cannot appear above.",
            census.unattributed_calls(),
            census.dropped_calls()
        ));
    }

    lines.push(BANNER.to_string());
    lines
}

/// The one-line form for the shared imgui overlay, where there is room for a sentence and no more.
pub fn overlay_line(census: &Census, game_module: &str, bindings: &GameBindings) -> String {
    let collisions = census.key_collisions(game_module).len()
        + bindings.collisions_with(census, game_module).len();
    if collisions == 0 {
        "hotkey-conflicts: none".to_string()
    } else {
        let keys: Vec<String> = census
            .key_collisions(game_module)
            .iter()
            .map(|collision| collision.input.describe())
            .collect();
        format!("hotkey-conflicts: {collisions} on {}", keys.join(", "))
    }
}

/// A collision found after the report was already printed.
///
/// The brief says one warning, and one warning is what [`render`] produces. But a mod whose hotkey
/// is only polled inside a menu can appear an hour later, and dropping that on the floor to
/// protect a formatting rule would be the wrong trade. It goes to the log as a plainly-labelled
/// amendment, never as a second banner.
pub fn amendment(input: InputId, modules: &[String]) -> String {
    format!(
        "LATE CONFLICT (after the report): {}  <-  {}",
        input.describe(),
        modules.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::census::Surface;
    use crate::game_bindings::{GameBinding, NOT_INITIALISED, action_label};

    const GAME: &str = "eldenring.exe";
    const VK_F7: u16 = 0x76;
    const VK_E: u16 = 0x45;

    fn watching() -> Vec<String> {
        vec!["GetAsyncKeyState".to_string()]
    }

    fn input<'a>(census: &'a Census, bindings: &'a GameBindings) -> ReportInput<'a> {
        ReportInput {
            census,
            game_module: GAME,
            bindings,
            consumed_keys: &[],
            observed_seconds: 42,
            loaded_modules: 15,
            calls_seen: 1200,
            surfaces_hooked: Box::leak(Box::new(watching())),
            surfaces_missed: &[],
        }
    }

    fn f7_pair() -> Census {
        let mut census = Census::default();
        for module in ["er_invasion_warp.dll", "er_invasion_path.dll"] {
            census.record(module, InputId::Key(VK_F7), Surface::AsyncKeyState, 600);
        }
        census
    }

    /// The reproducer, rendered. Both module names and the key must be on one line, because that
    /// line is the whole product.
    #[test]
    fn the_reproducer_renders_both_modules_and_the_key() {
        let census = f7_pair();
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let text = render(&input(&census, &bindings)).join("\n");
        assert!(text.contains("VERDICT: 1 CONFLICT(S)"), "{text}");
        assert!(text.contains("CONFLICT  F7"), "{text}");
        assert!(text.contains("er_invasion_warp.dll"), "{text}");
        assert!(text.contains("er_invasion_path.dll"), "{text}");
    }

    /// A clean profile still gets a verdict, and it must not read as an error.
    #[test]
    fn a_clean_profile_says_so_in_the_first_line() {
        let mut census = Census::default();
        census.record(
            "er_invasion_warp.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            600,
        );
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let lines = render(&input(&census, &bindings));
        assert!(lines[1].starts_with("VERDICT: no input is claimed by two mods"));
    }

    /// The failure that must never look like success: hooks that never fired.
    #[test]
    fn zero_calls_is_unknown_not_clean() {
        let census = Census::default();
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let mut report = input(&census, &bindings);
        report.calls_seen = 0;
        let lines = render(&report);
        assert!(lines[1].starts_with("VERDICT: UNKNOWN"), "{}", lines[1]);
        assert!(lines[1].contains("NOT a clean profile"));
    }

    /// The half that did not run has to say it did not run, in every report, including a clean
    /// one -- otherwise a clean verdict overclaims.
    #[test]
    fn the_unavailable_game_bindings_caveat_is_always_printed() {
        let census = Census::default();
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let text = render(&input(&census, &bindings)).join("\n");
        assert!(text.contains("NOT CHECKED"), "{text}");
        assert!(text.contains(NOT_INITIALISED), "{text}");
    }

    #[test]
    fn a_game_binding_collision_names_the_action() {
        let mut census = Census::default();
        census.record(
            "er_charm_enemies.dll",
            InputId::Key(VK_E),
            Surface::AsyncKeyState,
            10,
        );
        let bindings = GameBindings::Table(vec![GameBinding {
            input: InputId::Key(VK_E),
            action: action_label(0x12, 0),
        }]);
        let text = render(&input(&census, &bindings)).join("\n");
        assert!(text.contains("CONFLICT WITH THE GAME  E"), "{text}");
        assert!(text.contains("game action #0x12"), "{text}");
        assert!(
            !text.contains(NOT_INITIALISED),
            "a table was available; the caveat must not print"
        );
    }

    /// A whole-keyboard reader is a limitation of the API, and the report has to say so rather
    /// than let a reader infer the module has no bindings.
    #[test]
    fn raw_keyboard_readers_are_reported_as_unchecked_not_as_clean() {
        let mut census = Census::default();
        census.record(
            "er_charm_enemies.dll",
            InputId::WholeKeyboard,
            Surface::DirectInputKeyboard,
            600,
        );
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let text = render(&input(&census, &bindings)).join("\n");
        assert!(text.contains("NOT CHECKED  <entire keyboard>"), "{text}");
        assert!(text.contains("er_charm_enemies.dll"), "{text}");
        assert!(text.contains("Read their configs"), "{text}");
    }

    #[test]
    fn consumed_keys_are_named_and_the_limitation_stated() {
        let census = Census::default();
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let mut report = input(&census, &bindings);
        report.consumed_keys = &[0x43, 0x25];
        let text = render(&report).join("\n");
        assert!(text.contains("TAKEN FROM THE GAME  C, Left"), "{text}");
        assert!(text.contains("does not say which mod"), "{text}");
    }

    #[test]
    fn a_surface_that_could_not_be_hooked_is_disclosed() {
        let census = Census::default();
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let missed = vec!["XInputGetState".to_string()];
        let mut report = input(&census, &bindings);
        report.surfaces_missed = &missed;
        let text = render(&report).join("\n");
        assert!(text.contains("could not hook: XInputGetState"), "{text}");
    }

    /// Two runs over identical evidence must be byte-identical, or the report cannot be diffed.
    #[test]
    fn identical_evidence_renders_identically() {
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let first = render(&input(&f7_pair(), &bindings));
        let second = render(&input(&f7_pair(), &bindings));
        assert_eq!(first, second);
    }

    #[test]
    fn the_report_is_delimited_at_both_ends() {
        let census = Census::default();
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        let lines = render(&input(&census, &bindings));
        assert_eq!(lines.first().map(String::as_str), Some(BANNER));
        assert_eq!(lines.last().map(String::as_str), Some(BANNER));
    }

    #[test]
    fn the_overlay_line_fits_on_one_line_and_names_the_keys() {
        let bindings = GameBindings::Unavailable(NOT_INITIALISED);
        assert_eq!(
            overlay_line(&Census::default(), GAME, &bindings),
            "hotkey-conflicts: none"
        );
        assert_eq!(
            overlay_line(&f7_pair(), GAME, &bindings),
            "hotkey-conflicts: 1 on F7"
        );
    }

    #[test]
    fn an_amendment_is_labelled_as_late_rather_than_repeating_the_banner() {
        let line = amendment(
            InputId::Key(VK_F7),
            &["a.dll".to_string(), "b.dll".to_string()],
        );
        assert!(line.starts_with("LATE CONFLICT"));
        assert!(!line.contains(BANNER));
        assert!(line.contains("F7"));
    }
}
