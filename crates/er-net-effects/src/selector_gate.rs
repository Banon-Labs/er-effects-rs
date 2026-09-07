//! When the effect selector may take the keyboard, and which keys it may take.
//!
//! # The bug this exists to kill
//!
//! Elden Ring reads the keyboard through `IDirectInputDevice8::GetDeviceState` (vtable slot 9,
//! detoured with MinHook in [`crate::input_suppression`]). While the selector claims the keyboard
//! this DLL blanks the four arrow DIK bytes -- `DIK_LEFT 0xcb`, `DIK_RIGHT 0xcd`, `DIK_UP 0xc8`,
//! `DIK_DOWN 0xd0` -- out of that 256-byte table before the game reads it, and separately returns
//! `LRESULT(1)` for arrow key-down/key-up from a `WH_KEYBOARD_LL` hook. Both are hard takings: the
//! game never learns the key was pressed.
//!
//! What armed them was `NetEffectsState::effect_selector_visible`, which is seeded from the
//! `overlay_visible_on_start` config key and defaults to true. That flag does not mean "the
//! selector list is on screen". The bar is drawn by hudhook and starts collapsed
//! (`present_overlay::START_COLLAPSED`), so a default install shows a two-word `ER NET EFFECTS [+]`
//! header and no effect list at all -- while blanking the player's arrow keys out of every
//! DirectInput read. Worse, the DirectInput blanking consulted no runtime gate whatsoever, so it
//! was in force at the title screen and in every menu, where the arrow keys are how you navigate.
//!
//! # The rule
//!
//! Open means the selector is genuinely on screen and able to act: the player has not hidden the
//! bar, the bar is not minimized to its button, and the DLL's own runtime gate says there is a
//! live player.
//!
//! Taking and acting are separate questions, and conflating them is what made the first version of
//! this gate wrong in the other direction.
//!
//! **Taking** is the narrow one: only the four arrows, only while open. A key taken from the game
//! is a key the player pressed and the game never saw, so nothing else is ever taken -- every
//! other key the selector reads is passed straight through.
//!
//! **Acting** splits by what the key does:
//!
//! - Keys that drive the on-screen cursor -- [`SelectorKey::Arrow`] and [`SelectorKey::StackEdit`]
//!   -- require open. A cursor nobody can see is not a thing to drive, and numpad `+` on an
//!   invisible highlight would stack an effect the player never chose and rewrite
//!   `er-net-effects.toml` to match.
//! - Deliberate chords and player-chosen bindings stay live whether the bar is open or not:
//!   [`SelectorKey::ShowHide`] (the only way back to a hidden bar),
//!   [`SelectorKey::ExpandCollapse`] (the only way open -- see below),
//!   [`SelectorKey::EffectToggle`], and [`SelectorKey::Other`] -- which is where the hotkeys from
//!   `.er-net-effects-hotkeys.json` land. Firing an effect while the bar is minimized is this
//!   DLL's primary use; the bar ships minimized precisely so it can be played that way. Gating
//!   those on open would make the DLL useless for the thing it is for.
//!
//! None of them can be taken from the game in the first place: they need Alt, and Elden Ring binds
//! nothing to Alt+0, Alt+9 or Alt+'.
//!
//! # Why expanding needs a key, not just the button
//!
//! The `[+]` header is a mouse target at absolute screen coordinates, hit-tested against
//! `imgui::Io::mouse_pos`, which hudhook fills from `WM_MOUSEMOVE` / `WM_INPUT` on the game's
//! window. From #317 until 2026-08-31 that click was the only thing that could expand the bar,
//! and it never once fired.
//!
//! What was measured, live, run `br-20260831-063324-0a97`, `/proc/<pid>/mem` (no injection):
//!
//! - `overlay_toggle_clicks` finished the ~8-hour session at **0** across 1,296,264 rendered
//!   frames. The button was drawn on every one of them.
//! - The wndproc path itself works -- `MouseClickedPos[0]` held `(2155, 821)`, so imgui had seen
//!   real positions and a real left click at some point. This was not a dead hook.
//! - `MousePos` was nevertheless frozen at `(3190, 794)` on a 3840x2160 display across 12s of
//!   sampling while the game rendered at ~45fps -- far from the button's corner box, which sits
//!   within `SCREEN_MARGIN` of the top right.
//!
//! What is inferred, not proven: that the freeze is Elden Ring holding the mouse for the camera
//! through DirectInput and leaving the Windows cursor where it lies. It fits (the game polls a
//! DirectInput device every frame, and the cursor is hidden in play), but a still pointer is also
//! what a player who is not touching the mouse produces, and that was not separable from a read.
//!
//! Either way the conclusion is the same and does not rest on the mechanism: a hit box in a screen
//! corner is the wrong affordance on a host that may own the pointer, and a feature with exactly
//! one affordance that scores zero in 1.3M frames needs a second one. The click stays -- it works
//! wherever the pointer is free -- and the key is what the player uses.

// Windows-only in practice; kept portable so `cargo test` proves the decision table on the host
// instead of it being reasoned about in a review.
#![cfg_attr(not(windows), allow(dead_code))]

use crate::bindings::SelectorAction;

/// Left arrow.
pub(crate) const VK_LEFT: u32 = 0x25;
/// Up arrow.
pub(crate) const VK_UP: u32 = 0x26;
/// Right arrow.
pub(crate) const VK_RIGHT: u32 = 0x27;
/// Down arrow.
pub(crate) const VK_DOWN: u32 = 0x28;
/// Numpad `0` -- with Alt, one of the three show/hide keys, and the base the effect-trigger
/// hotkey file's `numpad<N>` names count up from.
pub(crate) const VK_NUMPAD0: u32 = 0x60;
/// Numpad `+` -- add the highlighted effect to the always-on stack.
pub(crate) const VK_ADD: u32 = 0x6b;
/// Numpad `-` -- take it back out.
pub(crate) const VK_SUBTRACT: u32 = 0x6d;
/// `'` / `"` -- with Alt, apply or remove the highlighted effect.
pub(crate) const VK_OEM_7: u32 = 0xde;

/// What a key means to the effect selector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectorKey {
    /// Alt+0, Alt+Numpad0 or Alt+Insert -- show or hide the bar. The way back in.
    ShowHide,
    /// Left / Right / Up / Down -- move the selector cursor. Also the game's own menu and
    /// quick-item keys, which is why taking one has a cost.
    Arrow,
    /// Numpad `+` / `-` -- stack or unstack whatever the cursor highlights.
    StackEdit,
    /// Alt+`'` -- apply or remove whatever the cursor highlights.
    EffectToggle,
    /// Alt+9 -- expand the bar from its `[+]` button, or minimize it back.
    ///
    /// The way open, and on this game the only one. See the module note.
    ExpandCollapse,
    /// Everything else, including whatever the player bound in the effect-trigger hotkey file.
    Other,
}

/// Classify a virtual-key code the way the selector reads it, against the bindings in force.
///
/// `alt_down` matters: bare `0` is a game key and bare `'` is a chat key, and neither may be
/// mistaken for a selector command. Only the Alt-modified forms mean anything here.
///
/// The table this used to match on is now `crate::bindings`, because every one of these keys is
/// configurable -- the arrows are the game's own menu keys, and `Alt+Insert` is a chord another
/// mod may have taken. The classification is unchanged; only where the keys come from moved.
pub(crate) fn key_for_vk(vk: u32, alt_down: bool) -> SelectorKey {
    key_for_vk_in(&crate::bindings::live(), vk, alt_down)
}

/// The pure form, so the decision table can be exercised against explicit bindings.
pub(crate) fn key_for_vk_in(
    bindings: &crate::bindings::SelectorBindings,
    vk: u32,
    alt_down: bool,
) -> SelectorKey {
    match bindings.action_for(vk, alt_down) {
        Some(SelectorAction::CursorUp | SelectorAction::CursorDown) => SelectorKey::Arrow,
        Some(SelectorAction::CursorLeft | SelectorAction::CursorRight) => SelectorKey::Arrow,
        Some(SelectorAction::StackAdd | SelectorAction::StackRemove) => SelectorKey::StackEdit,
        Some(SelectorAction::EffectToggle) => SelectorKey::EffectToggle,
        Some(SelectorAction::ShowHide) => SelectorKey::ShowHide,
        Some(SelectorAction::ExpandCollapse) => SelectorKey::ExpandCollapse,
        None => SelectorKey::Other,
    }
}

/// Everything that decides whether the selector is on screen and listening.
///
/// All three are read from live state every game tick, so the gate cannot drift from what the
/// player is actually looking at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectorInputState {
    /// The player has not hidden the bar -- `overlay_visible_on_start`, then Alt+0 / Alt+Numpad0 /
    /// Alt+Insert.
    pub(crate) shown: bool,
    /// The bar is minimized to its `[+]` button, so no effect list is drawn. Shown-and-collapsed
    /// is the shipped default, and treating it as open is the bug this module exists for.
    pub(crate) collapsed: bool,
    /// The DLL's own runtime gate: a local player exists and is rendered. False at the title
    /// screen and through every loading screen, where the arrow keys belong to the game's menus.
    pub(crate) runtime_ready: bool,
}

impl SelectorInputState {
    /// Is the selector list genuinely on screen and able to act?
    pub(crate) fn is_open(self) -> bool {
        self.shown && !self.collapsed && self.runtime_ready
    }
}

/// May the selector act on this key?
pub(crate) fn should_handle_key(open: bool, key: SelectorKey) -> bool {
    match key {
        // Without it a hidden bar can never be brought back from the keyboard.
        SelectorKey::ShowHide => true,
        // Gating this on `open` would be circular: open means expanded, so the key that expands
        // the bar would need the bar already expanded. That is not a hypothetical -- the bar ships
        // collapsed, and until this key existed the only thing that could expand it was a mouse
        // click the game does not let the player make.
        SelectorKey::ExpandCollapse => true,
        // Alt+' is a deliberate chord on the effect the player already chose, and the trigger
        // hotkeys are the player's own bindings. Both are meant to be pressed while playing --
        // which is exactly when the bar is minimized -- so neither waits for the bar.
        SelectorKey::EffectToggle | SelectorKey::Other => true,
        // These two drive the visible cursor. Off screen there is nothing to drive, and numpad +
        // would stack an effect the player cannot see and write it back to the config file.
        SelectorKey::Arrow | SelectorKey::StackEdit => open,
    }
}

/// May this key be taken from the game -- blanked out of the DirectInput state, or swallowed by
/// the low-level keyboard hook?
///
/// Only the arrows are ever taken, and only while the selector is open. Everything else is
/// observed and passed through, so a key the selector happens to read is still the game's key.
pub(crate) fn should_consume_key(open: bool, key: SelectorKey) -> bool {
    open && matches!(key, SelectorKey::Arrow)
}

#[cfg(test)]
mod tests {
    /// The shipped defaults for the two chords this module's tests name directly. They are no
    /// longer crate constants -- every key is configurable now -- but the decision table below is
    /// about the shipped bindings, so it needs their codes.
    const VK_INSERT: u32 = 0x2d;
    const VK_0: u32 = 0x30;
    const VK_9: u32 = 0x39;

    use super::*;

    const SHOWN_EXPANDED_IN_WORLD: SelectorInputState = SelectorInputState {
        shown: true,
        collapsed: false,
        runtime_ready: true,
    };

    /// The shipped default before a single click: `overlay_visible_on_start = true` meets
    /// `START_COLLAPSED = true`.
    const SHIPPED_DEFAULT: SelectorInputState = SelectorInputState {
        shown: true,
        collapsed: true,
        runtime_ready: true,
    };

    #[test]
    fn the_shipped_default_is_not_open() {
        assert!(
            !SHIPPED_DEFAULT.is_open(),
            "a bar minimized to its [+] button shows no effect list, so it is not open -- \
             this is the exact state that was eating the player's arrow keys"
        );
    }

    #[test]
    fn only_a_shown_expanded_live_selector_is_open() {
        assert!(SHOWN_EXPANDED_IN_WORLD.is_open());
        assert!(
            !SelectorInputState {
                shown: false,
                ..SHOWN_EXPANDED_IN_WORLD
            }
            .is_open()
        );
        assert!(
            !SelectorInputState {
                collapsed: true,
                ..SHOWN_EXPANDED_IN_WORLD
            }
            .is_open()
        );
        assert!(
            !SelectorInputState {
                runtime_ready: false,
                ..SHOWN_EXPANDED_IN_WORLD
            }
            .is_open(),
            "the title screen and every loading screen have no player, and their menus are \
             driven with the arrow keys"
        );
    }

    #[test]
    fn a_closed_selector_ignores_only_the_keys_that_drive_its_cursor() {
        for key in [SelectorKey::Arrow, SelectorKey::StackEdit] {
            assert!(
                !should_handle_key(false, key),
                "a closed selector has no visible cursor for {key:?} to move"
            );
        }
        assert!(should_handle_key(false, SelectorKey::ShowHide));
    }

    /// The regression this names. The first cut of this gate made every key wait for an open bar.
    /// The bar ships minimized, so that silently killed the effect-trigger hotkeys from
    /// `.er-net-effects-hotkeys.json` -- bindings whose entire purpose is to fire an effect while
    /// you are playing, which is precisely when the bar is minimized. It made the DLL useless for
    /// the one thing it is for, in the name of fixing a complaint that was only ever about arrow
    /// keys. Pinned here beside
    /// [`the_shipped_default_neither_acts_on_nor_takes_an_arrow_key`] so the two halves of the
    /// rule cannot drift apart: arrows strict, player bindings live.
    #[test]
    fn a_closed_selector_still_fires_effect_trigger_hotkeys() {
        let open = SHIPPED_DEFAULT.is_open();
        assert!(
            !open,
            "the shipped default is closed -- that is the premise"
        );

        // `numpad_multiply`, the key in the default hotkey file, plus a plain function key.
        for vk in [0x6a, 0x74] {
            let key = key_for_vk(vk, false);
            assert_eq!(key, SelectorKey::Other);
            assert!(
                should_handle_key(open, key),
                "a hotkey bound in .er-net-effects-hotkeys.json must fire while playing"
            );
            assert!(
                !should_consume_key(open, key),
                "...and must still reach the game, because it was never the selector's to take"
            );
        }

        // Alt+' toggles the effect already chosen; it worked with the bar down before this gate
        // existed and must keep working.
        let toggle = key_for_vk(VK_OEM_7, true);
        assert_eq!(toggle, SelectorKey::EffectToggle);
        assert!(should_handle_key(open, toggle));
        assert!(!should_consume_key(open, toggle));
    }

    #[test]
    fn an_open_selector_acts_on_every_key_it_knows() {
        for key in [
            SelectorKey::ShowHide,
            SelectorKey::Arrow,
            SelectorKey::StackEdit,
            SelectorKey::EffectToggle,
            SelectorKey::Other,
        ] {
            assert!(
                should_handle_key(true, key),
                "an open selector wants {key:?}"
            );
        }
    }

    #[test]
    fn a_closed_selector_takes_nothing_from_the_game() {
        for key in [
            SelectorKey::ShowHide,
            SelectorKey::Arrow,
            SelectorKey::StackEdit,
            SelectorKey::EffectToggle,
            SelectorKey::Other,
        ] {
            assert!(
                !should_consume_key(false, key),
                "a closed selector must forward {key:?} to the game untouched"
            );
        }
    }

    #[test]
    fn even_an_open_selector_only_takes_the_arrows() {
        assert!(should_consume_key(true, SelectorKey::Arrow));
        for key in [
            SelectorKey::ShowHide,
            SelectorKey::ExpandCollapse,
            SelectorKey::StackEdit,
            SelectorKey::EffectToggle,
            SelectorKey::Other,
        ] {
            assert!(
                !should_consume_key(true, key),
                "{key:?} is read, never taken: the game must still see it"
            );
        }
    }

    #[test]
    fn the_shipped_default_neither_acts_on_nor_takes_an_arrow_key() {
        let open = SHIPPED_DEFAULT.is_open();
        let arrow = key_for_vk(VK_LEFT, false);
        assert_eq!(arrow, SelectorKey::Arrow);
        assert!(!should_handle_key(open, arrow));
        assert!(!should_consume_key(open, arrow));
    }

    #[test]
    fn every_arrow_classifies_as_an_arrow_with_or_without_alt() {
        for vk in [VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN] {
            assert_eq!(key_for_vk(vk, false), SelectorKey::Arrow);
            assert_eq!(key_for_vk(vk, true), SelectorKey::Arrow);
        }
    }

    #[test]
    fn the_show_hide_keys_need_alt() {
        for vk in [VK_0, VK_NUMPAD0, VK_INSERT] {
            assert_eq!(key_for_vk(vk, true), SelectorKey::ShowHide);
            assert_eq!(
                key_for_vk(vk, false),
                SelectorKey::Other,
                "bare 0 / Insert belong to the game"
            );
        }
    }

    #[test]
    fn the_effect_toggle_needs_alt() {
        assert_eq!(key_for_vk(VK_OEM_7, true), SelectorKey::EffectToggle);
        assert_eq!(key_for_vk(VK_OEM_7, false), SelectorKey::Other);
    }

    #[test]
    fn the_stack_keys_are_bare_numpad_plus_and_minus() {
        assert_eq!(key_for_vk(VK_ADD, false), SelectorKey::StackEdit);
        assert_eq!(key_for_vk(VK_SUBTRACT, false), SelectorKey::StackEdit);
    }

    #[test]
    fn an_unrelated_key_is_never_a_selector_command() {
        // W, Space, Escape, F5 -- the keys a player is actually holding while this runs.
        for vk in [0x57, 0x20, 0x1b, 0x74] {
            assert_eq!(key_for_vk(vk, false), SelectorKey::Other);
            assert_eq!(key_for_vk(vk, true), SelectorKey::Other);
        }
    }

    #[test]
    fn the_expand_key_needs_alt() {
        assert_eq!(key_for_vk(VK_9, true), SelectorKey::ExpandCollapse);
        assert_eq!(
            key_for_vk(VK_9, false),
            SelectorKey::Other,
            "bare 9 is a game key"
        );
    }

    /// The regression. The bar ships minimized, and from #317 until 2026-08-31 the only thing that
    /// could expand it was a left click on a hit box in a screen corner -- which scored
    /// `overlay_toggle_clicks == 0` across a measured 1,296,264 rendered frames. So the shipped
    /// default must have a key that acts while the bar is closed, or the effect list and the four
    /// cursor keys behind it are dead.
    #[test]
    fn the_shipped_default_can_be_expanded_from_the_keyboard() {
        let open = SHIPPED_DEFAULT.is_open();
        assert!(!open, "the shipped default is collapsed, hence not open");
        let expand = key_for_vk(VK_9, true);
        assert_eq!(expand, SelectorKey::ExpandCollapse);
        assert!(
            should_handle_key(open, expand),
            "the key that OPENS the bar may not require the bar to be open"
        );
        assert!(
            !should_consume_key(open, expand),
            "expanding is read, never taken: the game must still see the key"
        );
    }
}
