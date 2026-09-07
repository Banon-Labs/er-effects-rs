//! Which keys drive the effect selector, read from `er-net-effects.toml` and re-read while the
//! game runs.
//!
//! # Why these stopped being constants
//!
//! Every key here was a `VK_*` constant with no config line: the four arrows, numpad `+`/`-`,
//! `Alt+'`, and the three show/hide chords. Two of those choices are actively contested --
//! the arrows are the game's own menu and quick-item keys, and `Alt+Insert` is a key another mod
//! in the same me3 profile may well have taken. A player who did not want this DLL eating their
//! arrows had exactly one remedy: unload it.
//!
//! # The two things a rebind must get right
//!
//! **The DirectInput edge mask must be reset.** `input_suppression` remembers which selector keys
//! were down on the previous poll as a bitmask, and the bits are positional -- slot 3 is
//! "cursor right" only while the bindings that produced them are still in force. Carry that mask
//! across a rebind and a key held at that instant either swallows its own press or manufactures
//! one. The [`slot`] indices are therefore fixed, and the caller clears the mask whenever
//! [`refresh_live`] reports [`BindingsUpdate::moved`].
//!
//! **A malformed value must keep the key that was working.** `er_hotkey_config::Binding` owns that
//! rule; this module supplies the parse and the log line.
//!
//! # Alt is a requirement, never a filter
//!
//! A chord with Alt fires only while Alt is held. A chord without Alt fires whether or not it is
//! -- which is what the hard-coded table did, and it matters: holding Alt while arrowing through
//! the list must keep moving the cursor. Getting this backwards would make the arrows deaf
//! whenever the player happened to rest a thumb on Alt.

// Windows-only in practice; kept portable so `cargo test` proves the binding table on the host
// instead of it being reasoned about in a review.
#![cfg_attr(not(windows), allow(dead_code))]

use er_hotkey_config::{Binding, BindingUpdate, Chord, KeyParseError, chord_name, parse_chord};

/// What a bound key drives.
///
/// The four cursor directions are separate variants rather than one `Cursor` because the
/// hold-to-repeat state machine tracks each direction independently, and because a player
/// rebinding "up" has no reason to rebind "left".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectorAction {
    CursorUp,
    CursorDown,
    CursorLeft,
    CursorRight,
    StackAdd,
    StackRemove,
    EffectToggle,
    ShowHide,
    /// Expand the bar from its `[+]` button to the effect list, or minimize it back.
    ///
    /// Separate from [`SelectorAction::ShowHide`], which draws the bar or removes it entirely.
    /// The `[+]` button also takes a left mouse click, but that click is unreachable while the
    /// game holds the pointer -- see [`crate::selector_gate`].
    ExpandCollapse,
}

/// A fixed slot index for every binding, so a bitmask of "which selector keys were down" means the
/// same thing from one poll to the next.
///
/// The first four match `hold_repeat`'s `REPEAT_KEY_COUNT` indices, which is why they lead.
pub(crate) mod slot {
    pub(crate) const CURSOR_UP: usize = 0;
    pub(crate) const CURSOR_DOWN: usize = 1;
    pub(crate) const CURSOR_LEFT: usize = 2;
    pub(crate) const CURSOR_RIGHT: usize = 3;
    pub(crate) const STACK_ADD: usize = 4;
    pub(crate) const STACK_REMOVE: usize = 5;
    pub(crate) const EFFECT_TOGGLE: usize = 6;
    /// Show/hide accepts several chords, so it owns a range.
    pub(crate) const SHOW_HIDE_FIRST: usize = 7;
    pub(crate) const SHOW_HIDE_MAX: usize = 3;
    /// Expand/collapse, placed after the show/hide range so adding it renumbered no existing
    /// slot. The indices feed a positional bitmask, so a renumber is a real hazard even though
    /// nothing persists them.
    pub(crate) const EXPAND_COLLAPSE: usize = SHOW_HIDE_FIRST + SHOW_HIDE_MAX;
    /// One past the last slot. Every mask in this crate is `usize`, so this must stay well under
    /// 64.
    pub(crate) const COUNT: usize = EXPAND_COLLAPSE + 1;
}

/// The mask of the four cursor slots -- the only keys this DLL takes from the game.
pub(crate) const CURSOR_SLOT_MASK: usize = (1 << slot::CURSOR_UP)
    | (1 << slot::CURSOR_DOWN)
    | (1 << slot::CURSOR_LEFT)
    | (1 << slot::CURSOR_RIGHT);

/// The config key for each single-chord binding, and its shipped default.
pub(crate) const SINGLE_BINDINGS: &[(&str, SelectorAction, usize, &str)] = &[
    (
        "selector_up_key",
        SelectorAction::CursorUp,
        slot::CURSOR_UP,
        "up",
    ),
    (
        "selector_down_key",
        SelectorAction::CursorDown,
        slot::CURSOR_DOWN,
        "down",
    ),
    (
        "selector_left_key",
        SelectorAction::CursorLeft,
        slot::CURSOR_LEFT,
        "left",
    ),
    (
        "selector_right_key",
        SelectorAction::CursorRight,
        slot::CURSOR_RIGHT,
        "right",
    ),
    (
        "selector_stack_add_key",
        SelectorAction::StackAdd,
        slot::STACK_ADD,
        "kp_plus",
    ),
    (
        "selector_stack_remove_key",
        SelectorAction::StackRemove,
        slot::STACK_REMOVE,
        "kp_minus",
    ),
    (
        "selector_apply_key",
        SelectorAction::EffectToggle,
        slot::EFFECT_TOGGLE,
        "alt+quote",
    ),
    (
        "selector_expand_key",
        SelectorAction::ExpandCollapse,
        slot::EXPAND_COLLAPSE,
        "alt+9",
    ),
];

/// The config key for the show/hide chords, and their shipped defaults.
///
/// Three of them, because a keyboard may have any one of the three and the bar being hidden with
/// no way back is the worst state this DLL can leave a player in.
pub(crate) const SHOW_HIDE_CONFIG_KEY: &str = "selector_show_hide_key";
pub(crate) const SHOW_HIDE_DEFAULT: &str = "alt+0, alt+kp_0, alt+insert";

/// The key section of the shipped `er-net-effects.toml`, appended to the default file by
/// `crate::config`.
///
/// It lives here, next to the table it documents, so a host test can prove that every line it
/// declares is a binding this module recognises and every value it suggests actually parses. A
/// config file that documents a key the parser rejects is a lie the player reads and then reports
/// as the feature being broken -- and `crate::config` is windows-gated, so a test living there
/// would never run.
pub(crate) const SHIPPED_KEY_SECTION: &str = r#"
# ---------------------------------------------------------------------------------------------
# KEYS. Every one of these is re-read while the game runs -- edit, save, and the new key is live
# within about a second. The log names the old key and the new one each time one moves.
#
# Name a key: A..Z, 0..9, F1..F15, Insert Delete Home End PageUp PageDown Backspace Tab Enter
# Escape Space Left Up Right Down PrintScreen ScrollLock NumLock Pause CapsLock, punctuation by
# symbol or name (- = [ ] \ ; ' , . / `), keypad KP_0..KP_9 KP_Plus KP_Minus KP_Multiply KP_Divide
# KP_Period KP_Enter. Case and spacing do not matter. Prefix with ctrl+, alt+ or shift+ for a
# chord; either side of the keyboard counts.
#
# A key this file does not recognise is reported in the log and THE PREVIOUS KEY STAYS IN FORCE.
# A typo never leaves you with no key at all.

# The four keys that move the selector cursor.
#
# THESE ARE THE ONLY KEYS THIS DLL TAKES FROM THE GAME, and only while the bar is expanded and a
# character is loaded -- the arrows are also Elden Ring's own menu and quick-item keys, so while
# the selector owns one the game never sees it. Rebind them if you would rather keep the arrows.
# They must be keys DirectInput can report, because taking a key from the game means blanking it
# out of the game's own keyboard buffer.
selector_up_key = "up"
selector_down_key = "down"
selector_left_key = "left"
selector_right_key = "right"

# Add / remove the highlighted effect from the always-on stack above.
selector_stack_add_key = "kp_plus"
selector_stack_remove_key = "kp_minus"

# Apply or remove the highlighted effect. Fires whether the bar is expanded or not -- playing with
# the bar minimized is what this DLL is for.
selector_apply_key = "alt+quote"

# Expand the bar from its ER NET EFFECTS [+] button to the full effect list, and minimize it back.
#
# THIS IS THE ONLY WAY IN WHILE YOU ARE PLAYING. The button also takes a left mouse click, and that
# click works whenever the pointer is free -- but while the game has the mouse the Windows pointer
# the overlay hit-tests against stops following your hand, so a button in the screen corner cannot
# be reached. Measured: it was clicked ZERO times in 1,296,264 drawn frames. Before this key
# existed a bar that ships minimized stayed minimized, and with it the four cursor keys above --
# which require an expanded bar -- did nothing at all.
selector_expand_key = "alt+9"

# Show or hide the bar. Several chords, comma-separated, because this is the ONLY way back to a
# hidden bar and a keyboard may not have all three of these keys.
selector_show_hide_key = "alt+0, alt+kp_0, alt+insert"
"#;

/// Is this config key one of the binding keys?
///
/// Used by the config parser so a binding line is not reported as an unknown key, and so the list
/// of names lives here rather than being repeated in the parser -- a name that drifts between the
/// two would read as a typo in the player's file.
pub(crate) fn is_binding_key(name: &str) -> bool {
    name == SHOW_HIDE_CONFIG_KEY
        || SINGLE_BINDINGS
            .iter()
            .any(|(config_key, _, _, _)| *config_key == name)
}

/// One bound key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BoundKey {
    pub(crate) action: SelectorAction,
    pub(crate) slot: usize,
    pub(crate) chord: Chord,
}

impl BoundKey {
    /// The bit this key occupies in a "which selector keys are down" mask.
    pub(crate) const fn bit(self) -> usize {
        1 << self.slot
    }

    /// Does a virtual key plus the current Alt state press this binding?
    ///
    /// See the module note: Alt in the chord is a requirement; Alt absent from the chord is not a
    /// prohibition.
    pub(crate) const fn matches(self, vk: u32, alt_down: bool) -> bool {
        self.chord.vk == vk && (!self.chord.needs_alt() || alt_down)
    }
}

/// Every key the selector listens for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectorBindings {
    keys: Vec<BoundKey>,
}

impl Default for SelectorBindings {
    /// The shipped defaults, which are exactly the bindings that used to be `VK_*` constants.
    fn default() -> Self {
        let mut keys = Vec::with_capacity(slot::COUNT);
        for (_, action, slot, default) in SINGLE_BINDINGS {
            keys.push(BoundKey {
                action: *action,
                slot: *slot,
                chord: parse_chord(default).expect("a shipped default parses"),
            });
        }
        for (index, chord) in parse_chord_list(SHOW_HIDE_DEFAULT)
            .expect("the shipped show/hide defaults parse")
            .into_iter()
            .enumerate()
            .take(slot::SHOW_HIDE_MAX)
        {
            keys.push(BoundKey {
                action: SelectorAction::ShowHide,
                slot: slot::SHOW_HIDE_FIRST + index,
                chord,
            });
        }
        Self { keys }
    }
}

/// Parse one binding value.
///
/// `cursor` tightens the rule for the four keys this DLL takes from the game: taking a key means
/// blanking its byte out of the DirectInput buffer before the game reads it, and a key with no
/// DirectInput scancode has no byte to blank. Binding one would produce a cursor key that moves
/// the selector and still reaches the game -- an arrow that scrolls the list and swaps your
/// quick-item at the same time. Refusing at parse time turns that into a log line and keeps the
/// key that was working.
///
/// # Errors
/// [`KeyParseError`] from the shared parser, plus [`KeyParseError::NoScancode`] for a cursor key
/// DirectInput cannot report.
fn parse_binding(value: &str, cursor: bool) -> Result<Chord, KeyParseError> {
    let chord = parse_chord(value)?;
    if cursor && chord.dik.is_none() {
        return Err(KeyParseError::NoScancode(value.trim().to_owned()));
    }
    Ok(chord)
}

/// Parse a comma-separated chord list. All-or-nothing: a list with one bad entry is a value the
/// player got wrong, and half-applying it would bind some of what they wrote and silently drop
/// the rest.
///
/// # Errors
/// The first entry that does not parse.
pub(crate) fn parse_chord_list(value: &str) -> Result<Vec<Chord>, KeyParseError> {
    let mut chords = Vec::new();
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        chords.push(parse_chord(entry)?);
    }
    if chords.is_empty() {
        return Err(KeyParseError::Empty);
    }
    Ok(chords)
}

impl SelectorBindings {
    /// Every bound key.
    ///
    /// The order is not the slot order and never was -- `apply_show_hide` rebinds by
    /// retain-and-append, so a rebound show/hide chord moves to the end. Read `BoundKey::slot`;
    /// nothing here may depend on position in this slice.
    pub(crate) fn keys(&self) -> &[BoundKey] {
        &self.keys
    }

    /// Which action this virtual key drives right now, if any.
    pub(crate) fn action_for(&self, vk: u32, alt_down: bool) -> Option<SelectorAction> {
        self.keys
            .iter()
            .find(|key| key.matches(vk, alt_down))
            .map(|key| key.action)
    }

    /// The scancodes of the four cursor keys -- the only bytes this DLL blanks out of the game's
    /// keyboard buffer.
    ///
    /// A cursor key bound to something DirectInput cannot report has no scancode and is simply
    /// absent here: there is no byte to blank, and the key still works through the low-level
    /// keyboard hook.
    pub(crate) fn cursor_scancodes(&self) -> Vec<u8> {
        self.keys
            .iter()
            .filter(|key| key.bit() & CURSOR_SLOT_MASK != 0)
            .filter_map(|key| key.chord.dik)
            .collect()
    }

    /// Apply one config file's values, keeping whatever was working for anything that does not
    /// parse.
    pub(crate) fn apply(&mut self, setting: impl Fn(&str) -> Option<String>) -> BindingsUpdate {
        let mut update = BindingsUpdate::default();

        for (config_key, _, slot_index, _) in SINGLE_BINDINGS {
            let Some(raw) = setting(config_key) else {
                continue;
            };
            let Some(entry) = self.keys.iter_mut().find(|key| key.slot == *slot_index) else {
                continue;
            };
            let mut binding = Binding::new(entry.chord);
            let cursor = entry.bit() & CURSOR_SLOT_MASK != 0;
            match binding.apply(&raw, |value| parse_binding(value, cursor)) {
                BindingUpdate::Unchanged => {}
                BindingUpdate::Changed { from, to } => {
                    entry.chord = to;
                    update.moved = true;
                    update.messages.push(format!(
                        "{config_key} {} -> {}",
                        chord_name(from),
                        chord_name(to)
                    ));
                }
                BindingUpdate::Rejected { value, error, kept } => {
                    update.messages.push(format!(
                        "{config_key} {value:?}: {error}; staying on {}",
                        chord_name(kept)
                    ));
                }
            }
        }

        if let Some(raw) = setting(SHOW_HIDE_CONFIG_KEY) {
            self.apply_show_hide(&raw, &mut update);
        }

        update
    }

    /// The show/hide list, which is the one binding that accepts several chords.
    fn apply_show_hide(&mut self, raw: &str, update: &mut BindingsUpdate) {
        let before: Vec<Chord> = self
            .keys
            .iter()
            .filter(|key| key.action == SelectorAction::ShowHide)
            .map(|key| key.chord)
            .collect();
        match parse_chord_list(raw) {
            Ok(chords) => {
                let chords: Vec<Chord> = chords.into_iter().take(slot::SHOW_HIDE_MAX).collect();
                if chords == before {
                    return;
                }
                self.keys
                    .retain(|key| key.action != SelectorAction::ShowHide);
                for (index, chord) in chords.iter().enumerate() {
                    self.keys.push(BoundKey {
                        action: SelectorAction::ShowHide,
                        slot: slot::SHOW_HIDE_FIRST + index,
                        chord: *chord,
                    });
                }
                update.moved = true;
                update.messages.push(format!(
                    "{SHOW_HIDE_CONFIG_KEY} {} -> {}",
                    describe(&before),
                    describe(&chords)
                ));
            }
            // The bar can be hidden, and these chords are the only way back to it. Refusing a bad
            // value here is not a nicety -- accepting an empty list would leave a player with a
            // bar they cannot recover and a config file that looks correct.
            Err(error) => update.messages.push(format!(
                "{SHOW_HIDE_CONFIG_KEY} {raw:?}: {error}; staying on {}",
                describe(&before)
            )),
        }
    }
}

/// What applying a config file's values did.
///
/// `moved` and `messages` are different questions, and conflating them is a bug with a symptom:
/// a rejected value produces a message but must not count as a change, because a change clears
/// the DirectInput edge mask and a clear while a key is held reads as a press the player never
/// made. So a config file with a permanent typo in it would otherwise manufacture one phantom
/// press per reload, forever.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BindingsUpdate {
    /// True when at least one binding actually moved. The caller's edge-reset signal.
    pub(crate) moved: bool,
    /// Lines to log: one per binding that moved, one per value refused.
    pub(crate) messages: Vec<String>,
}

/// The bindings in force, shared with the hook callbacks that read the keyboard.
///
/// An `Arc` behind an `RwLock` rather than the bindings themselves: the DirectInput detour and the
/// low-level keyboard procedure both consult this, on the game's own threads, while a game frame
/// waits. They clone the `Arc` under a read lock held for one pointer copy and then work from a
/// snapshot, so a reload can never make the game wait on a file read.
static LIVE: std::sync::OnceLock<std::sync::RwLock<std::sync::Arc<SelectorBindings>>> =
    std::sync::OnceLock::new();

fn live_cell() -> &'static std::sync::RwLock<std::sync::Arc<SelectorBindings>> {
    LIVE.get_or_init(|| std::sync::RwLock::new(std::sync::Arc::new(SelectorBindings::default())))
}

/// The bindings in force right now.
///
/// Poisoning is recovered from rather than propagated: a lock poisoned by a panic elsewhere must
/// not silently stop the player's keys working for the rest of the session.
pub(crate) fn live() -> std::sync::Arc<SelectorBindings> {
    let cell = live_cell();
    match cell.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Apply a config file's values to the live bindings.
///
/// The returned [`BindingsUpdate::moved`] is the caller's edge-reset signal; a rejected value
/// produces a message without setting it, which is the whole reason the two are separate.
pub(crate) fn refresh_live(setting: impl Fn(&str) -> Option<String>) -> BindingsUpdate {
    let cell = live_cell();
    let mut guard = match cell.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut next = (**guard).clone();
    let update = next.apply(setting);
    if update.moved {
        *guard = std::sync::Arc::new(next);
    }
    update
}

/// Render a chord list the way the config file spells it.
fn describe(chords: &[Chord]) -> String {
    if chords.is_empty() {
        return "nothing".to_owned();
    }
    chords
        .iter()
        .map(|chord| chord_name(*chord))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use er_hotkey_config::keys::MODIFIER_ALT;

    use super::*;

    fn from_pairs<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |wanted: &str| {
            pairs
                .iter()
                .find(|(key, _)| *key == wanted)
                .map(|(_, value)| (*value).to_owned())
        }
    }

    /// The shipped defaults must be exactly the bindings that used to be hard-coded, or a player
    /// who never opens the config file gets a different DLL than they had.
    #[test]
    fn the_defaults_are_the_keys_that_used_to_be_constants() {
        let bindings = SelectorBindings::default();
        for (vk, action) in [
            (0x26u32, SelectorAction::CursorUp),
            (0x28, SelectorAction::CursorDown),
            (0x25, SelectorAction::CursorLeft),
            (0x27, SelectorAction::CursorRight),
            (0x6b, SelectorAction::StackAdd),
            (0x6d, SelectorAction::StackRemove),
        ] {
            assert_eq!(bindings.action_for(vk, false), Some(action), "{vk:#04x}");
        }
        // The three Alt chords, which need Alt and mean nothing without it.
        assert_eq!(
            bindings.action_for(0xde, true),
            Some(SelectorAction::EffectToggle)
        );
        assert_eq!(bindings.action_for(0xde, false), None, "bare ' is chat");
        for vk in [0x30u32, 0x60, 0x2d] {
            assert_eq!(
                bindings.action_for(vk, true),
                Some(SelectorAction::ShowHide),
                "{vk:#04x}"
            );
            assert_eq!(bindings.action_for(vk, false), None, "{vk:#04x}");
        }
    }

    /// Alt in a chord is a requirement; Alt absent is not a prohibition. Holding Alt while arrowing
    /// through the list must keep moving the cursor.
    #[test]
    fn a_chord_without_alt_fires_whether_or_not_alt_is_held() {
        let bindings = SelectorBindings::default();
        assert_eq!(
            bindings.action_for(0x25, true),
            Some(SelectorAction::CursorLeft)
        );
        assert_eq!(
            bindings.action_for(0x6b, true),
            Some(SelectorAction::StackAdd)
        );
    }

    /// A key nothing is bound to is nothing to this DLL -- which is what leaves every other key
    /// the player's.
    #[test]
    fn an_unbound_key_drives_nothing() {
        let bindings = SelectorBindings::default();
        for vk in [0x57u32, 0x20, 0x1b, 0x74] {
            assert_eq!(bindings.action_for(vk, false), None, "{vk:#04x}");
            assert_eq!(bindings.action_for(vk, true), None, "{vk:#04x}");
        }
    }

    /// RELOAD: a changed key is picked up and the log line names both ends.
    #[test]
    fn a_changed_key_is_picked_up_and_names_both_ends() {
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(from_pairs(&[("selector_up_key", "k")]));
        assert!(update.moved, "the caller must reset its edge mask");
        assert_eq!(update.messages, vec!["selector_up_key Up -> K".to_owned()]);
        assert_eq!(
            bindings.action_for(0x4b, false),
            Some(SelectorAction::CursorUp)
        );
        assert_eq!(
            bindings.action_for(0x26, false),
            None,
            "the old key is free"
        );
    }

    /// RELOAD: an unchanged file must not churn. Every reported change resets the DirectInput edge
    /// mask, and a reset while a key is held reads as a press the player never made.
    #[test]
    fn an_unchanged_file_does_not_churn() {
        let mut bindings = SelectorBindings::default();
        let shipped: Vec<(&str, &str)> = SINGLE_BINDINGS
            .iter()
            .map(|(key, _, _, default)| (*key, *default))
            .chain(std::iter::once((SHOW_HIDE_CONFIG_KEY, SHOW_HIDE_DEFAULT)))
            .collect();
        assert_eq!(
            bindings.apply(from_pairs(&shipped)),
            BindingsUpdate::default()
        );
        // ...and the same keys spelled differently are still the same keys.
        assert_eq!(
            bindings.apply(from_pairs(&[
                ("selector_up_key", "UP"),
                ("selector_stack_add_key", "numpad_+"),
                ("selector_apply_key", "ALT + apostrophe"),
                (SHOW_HIDE_CONFIG_KEY, "alt+0,alt+numpad0,alt+ins"),
            ])),
            BindingsUpdate::default()
        );
    }

    /// RELOAD: a malformed value keeps the key that was working -- not the shipped default, and
    /// not nothing.
    #[test]
    fn a_malformed_value_falls_back_to_the_previous_value_not_to_nothing() {
        let mut bindings = SelectorBindings::default();
        bindings.apply(from_pairs(&[("selector_up_key", "k")]));
        let update = bindings.apply(from_pairs(&[("selector_up_key", "Winkey")]));
        assert_eq!(update.messages.len(), 1, "{update:?}");
        assert!(update.messages[0].contains("Winkey"), "{update:?}");
        assert!(update.messages[0].contains("staying on K"), "{update:?}");
        assert!(
            !update.moved,
            "a rejection is not a change -- reporting one would clear the edge mask and \
             manufacture a phantom press once per reload, forever"
        );
        assert_eq!(
            bindings.action_for(0x4b, false),
            Some(SelectorAction::CursorUp),
            "the working key stays in force"
        );
    }

    /// The show/hide chords are the only way back to a hidden bar, so a bad value must leave the
    /// working ones alone rather than unbind them.
    #[test]
    fn a_malformed_show_hide_list_keeps_the_chords_that_worked() {
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(from_pairs(&[(SHOW_HIDE_CONFIG_KEY, "alt+0, alt+Winkey")]));
        assert_eq!(update.messages.len(), 1, "{update:?}");
        assert!(update.messages[0].contains("Winkey"), "{update:?}");
        assert!(!update.moved, "a half-parsed list must not half-apply");
        for vk in [0x30u32, 0x60, 0x2d] {
            assert_eq!(
                bindings.action_for(vk, true),
                Some(SelectorAction::ShowHide)
            );
        }
    }

    /// An empty show/hide value would leave a hidden bar unrecoverable, so it is refused like any
    /// other bad value.
    #[test]
    fn an_empty_show_hide_list_is_refused() {
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(from_pairs(&[(SHOW_HIDE_CONFIG_KEY, "  ")]));
        assert_eq!(update.messages.len(), 1, "{update:?}");
        assert!(!update.moved);
        assert_eq!(
            bindings.action_for(0x30, true),
            Some(SelectorAction::ShowHide)
        );
    }

    #[test]
    fn the_show_hide_list_can_be_rebound_to_one_chord() {
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(from_pairs(&[(SHOW_HIDE_CONFIG_KEY, "ctrl+]")]));
        assert_eq!(update.messages.len(), 1, "{update:?}");
        assert!(update.moved);
        assert_eq!(
            bindings.action_for(0xdd, false),
            Some(SelectorAction::ShowHide)
        );
        assert_eq!(bindings.action_for(0x30, true), None, "the old chords go");
    }

    /// A key the file does not mention keeps whatever is in force. Deleting a line is the same as
    /// never having written it.
    #[test]
    fn an_absent_key_is_neither_a_change_nor_a_rejection() {
        let mut bindings = SelectorBindings::default();
        assert_eq!(bindings.apply(|_| None), BindingsUpdate::default());
        assert_eq!(bindings, SelectorBindings::default());
    }

    /// Read the `key = "value"` lines out of the shipped key section, the way the config parser
    /// does.
    fn shipped_lines() -> Vec<(String, String)> {
        SHIPPED_KEY_SECTION
            .lines()
            .map(|line| line.split('#').next().unwrap_or_default().trim())
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let (key, value) = line.split_once('=')?;
                Some((
                    key.trim().to_owned(),
                    value.trim().trim_matches('"').to_owned(),
                ))
            })
            .collect()
    }

    /// Structural. Every binding must appear in the shipped file, and every line the shipped file
    /// declares must be a binding. A key missing from the file is one a player cannot discover; a
    /// line the parser does not know is reported to them as an unknown key in a file we wrote.
    #[test]
    fn the_shipped_file_declares_exactly_the_bindings_that_exist() {
        let shipped = shipped_lines();
        assert!(!shipped.is_empty(), "the key section parsed to nothing");
        for (key, _) in &shipped {
            assert!(
                is_binding_key(key),
                "the shipped file declares {key:?}, which the parser does not know"
            );
        }
        for (config_key, _, _, _) in SINGLE_BINDINGS {
            assert!(
                shipped.iter().any(|(key, _)| key == config_key),
                "{config_key} is a binding no shipped config file mentions"
            );
        }
        assert!(
            shipped.iter().any(|(key, _)| key == SHOW_HIDE_CONFIG_KEY),
            "{SHOW_HIDE_CONFIG_KEY} is a binding no shipped config file mentions"
        );
    }

    /// The values the shipped file suggests must parse to exactly the built-in defaults. A file
    /// whose own values do not read is a typo the player is blamed for; one whose values read as
    /// different keys silently changes the DLL for anyone who accepts the generated file.
    #[test]
    fn the_shipped_values_parse_to_the_built_in_defaults() {
        let shipped = shipped_lines();
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(|wanted: &str| {
            shipped
                .iter()
                .find(|(key, _)| key == wanted)
                .map(|(_, value)| value.clone())
        });
        assert_eq!(
            update,
            BindingsUpdate::default(),
            "the shipped file disagrees with the built-in defaults"
        );
    }

    /// The slots are positional and feed a bitmask, so nothing may collide and nothing may exceed
    /// the mask's width.
    #[test]
    fn every_slot_is_distinct_and_fits_the_mask() {
        let bindings = SelectorBindings::default();
        let mut seen = 0usize;
        for key in bindings.keys() {
            assert!(key.slot < slot::COUNT, "{:?} past the slot count", key.slot);
            assert!(key.slot < usize::BITS as usize, "slot outside the mask");
            assert_eq!(seen & key.bit(), 0, "slot {} used twice", key.slot);
            seen |= key.bit();
        }
        assert_eq!(seen & CURSOR_SLOT_MASK, CURSOR_SLOT_MASK);
    }

    /// Every shipped default must be a key DirectInput can report.
    ///
    /// The DirectInput read is the strong path: `input_suppression::queue_dinput_selector_edges`
    /// inspects the very 256-byte buffer the game itself polls, so a key with a scancode is seen
    /// exactly when the game sees it, whatever the window is doing with messages. A key with none
    /// can only reach the selector through the `WH_KEYBOARD_LL` hook -- a separate mechanism with
    /// separate failure modes. Shipping a default that arrives only on the weaker path is how a
    /// key ends up "not working" for reasons no log explains, which is the shape of bug this
    /// whole module already carries scars from.
    #[test]
    fn every_shipped_default_is_a_key_directinput_can_report() {
        let bindings = SelectorBindings::default();
        for key in bindings.keys() {
            assert!(
                key.chord.dik.is_some(),
                "{:?} ships bound to a key DirectInput cannot report",
                key.action
            );
        }
    }

    /// Only the four cursor keys are ever taken from the game, so only their scancodes are
    /// blanked out of its buffer. A stack or show/hide key appearing here would be a key the
    /// player pressed and the game never saw, for no reason.
    #[test]
    fn only_the_cursor_keys_are_blanked_from_the_games_buffer() {
        let bindings = SelectorBindings::default();
        let mut scancodes = bindings.cursor_scancodes();
        scancodes.sort_unstable();
        assert_eq!(scancodes, vec![0xc8, 0xcb, 0xcd, 0xd0], "the four arrows");
    }

    /// A cursor key must be one DirectInput can report, because taking a key from the game means
    /// blanking its byte out of the buffer and a key with no scancode has no byte. Binding one
    /// would give the player an arrow that both scrolls the list and still reaches the game.
    #[test]
    fn a_cursor_key_the_game_buffer_cannot_carry_is_refused() {
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(from_pairs(&[("selector_up_key", "F16")]));
        assert!(!update.moved, "{update:?}");
        assert_eq!(update.messages.len(), 1, "{update:?}");
        assert!(update.messages[0].contains("F16"), "{update:?}");
        assert_eq!(
            bindings.cursor_scancodes().len(),
            4,
            "the four arrows still take the game's keys"
        );

        // A key that does parse in both schemes moves the blanked byte with it.
        assert!(
            bindings
                .apply(from_pairs(&[("selector_up_key", "k")]))
                .moved
        );
        let mut scancodes = bindings.cursor_scancodes();
        scancodes.sort_unstable();
        assert_eq!(
            scancodes,
            vec![0x25, 0xcb, 0xcd, 0xd0],
            "DIK_K replaced DIK_UP"
        );
    }

    /// A non-cursor key is never taken from the game, so it may be one DirectInput cannot report:
    /// it still arrives through the low-level keyboard hook.
    #[test]
    fn a_non_cursor_key_may_be_one_directinput_cannot_report() {
        let mut bindings = SelectorBindings::default();
        let update = bindings.apply(from_pairs(&[("selector_stack_add_key", "F16")]));
        assert!(update.moved, "{update:?}");
        assert_eq!(
            bindings.action_for(0x7f, false),
            Some(SelectorAction::StackAdd)
        );
        assert_eq!(
            bindings.cursor_scancodes().len(),
            4,
            "and it is still not blanked from the game's buffer"
        );
    }

    /// Alt must not be droppable from a chord by a partial parse: `"alt+0"` and `"0"` are
    /// different bindings, and the second would take the player's top-row 0.
    #[test]
    fn alt_survives_a_reparse() {
        let mut bindings = SelectorBindings::default();
        bindings.apply(from_pairs(&[(SHOW_HIDE_CONFIG_KEY, "alt+0")]));
        let chord = bindings
            .keys()
            .iter()
            .find(|key| key.action == SelectorAction::ShowHide)
            .expect("show/hide is bound")
            .chord;
        assert_eq!(chord.modifiers & MODIFIER_ALT, MODIFIER_ALT);
        assert_eq!(bindings.action_for(0x30, false), None);
    }
}
