//! What was observed, and what it adds up to.
//!
//! The whole product of this DLL is one paragraph of text, so the part worth testing is the part
//! that decides what that paragraph says. Everything here is pure: observations go in, a report
//! comes out, and no game, no Windows API and no hook is involved. The Windows half's only job is
//! to feed it honest input.
//!
//! # Two tiers of evidence, kept apart on purpose
//!
//! Some input APIs name the key in their arguments -- `GetAsyncKeyState(VK_F7)` says *F7*, right
//! there. Those give an exact `(module, key)` pair and an exact collision.
//!
//! DirectInput does not. `IDirectInputDevice8::GetDeviceState` hands back all 256 scancodes at
//! once and the caller picks its own out of the buffer afterwards, in its own code, where nothing
//! is observable. A module reading that buffer could be bound to any key on the board. Reporting
//! that as "no collision found" would be a lie of omission, so it is reported as its own,
//! explicitly weaker claim -- and the keys that turn out to be consumed from that buffer (see
//! [`crate::dik`]) are reported as a third thing again.
//!
//! Conflating the tiers is the failure this split exists to prevent: the reproducer that motivated
//! the crate (two mods on `F7`) is tier one, and drowning it in tier-two noise would bury it.

// Windows-only in practice; ungated so the census, the collision table and the report renderer
// stay covered by `cargo test` on the host.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};

use er_invasion_warp_core::keybind::key_name;

/// Which API a caller was seen using. The distinction that matters is [`Surface::names_a_key`]:
/// whether the key is in the call's arguments or hidden in a buffer the caller reads later.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Surface {
    /// `user32!GetAsyncKeyState(vk)` -- the most common mod hotkey path.
    AsyncKeyState,
    /// `user32!GetKeyState(vk)`.
    KeyState,
    /// `user32!RegisterHotKey(hwnd, id, modifiers, vk)` -- a system-wide claim on the key.
    RegisterHotKey,
    /// `user32!GetKeyboardState(buf)` -- the whole board at once.
    KeyboardState,
    /// `user32!SetWindowsHookExW(WH_KEYBOARD_LL, ...)` -- sees every keystroke in the process.
    LowLevelKeyboardHook,
    /// `user32!SetWindowsHookExW(WH_MOUSE_LL, ...)`.
    LowLevelMouseHook,
    /// `IDirectInputDevice8::GetDeviceState` on a keyboard device -- where Elden Ring itself reads.
    DirectInputKeyboard,
    /// The same slot on a mouse device.
    DirectInputMouse,
    /// `xinput*!XInputGetState(index, state)` -- the whole pad at once.
    XInput,
}

impl Surface {
    /// How the surface is written in the report.
    pub const fn label(self) -> &'static str {
        match self {
            Surface::AsyncKeyState => "GetAsyncKeyState",
            Surface::KeyState => "GetKeyState",
            Surface::RegisterHotKey => "RegisterHotKey",
            Surface::KeyboardState => "GetKeyboardState",
            Surface::LowLevelKeyboardHook => "SetWindowsHookEx(WH_KEYBOARD_LL)",
            Surface::LowLevelMouseHook => "SetWindowsHookEx(WH_MOUSE_LL)",
            Surface::DirectInputKeyboard => "DirectInput GetDeviceState(keyboard)",
            Surface::DirectInputMouse => "DirectInput GetDeviceState(mouse)",
            Surface::XInput => "XInputGetState",
        }
    }

    /// Does the call carry the key it is asking about in its own arguments?
    ///
    /// This is the whole tier-one/tier-two split. `false` means the observation can only say
    /// "this module reads input from this device", never which button.
    pub const fn names_a_key(self) -> bool {
        matches!(
            self,
            Surface::AsyncKeyState | Surface::KeyState | Surface::RegisterHotKey
        )
    }
}

/// One input a module was seen taking an interest in.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum InputId {
    /// A Win32 virtual-key code. Mouse buttons live in this space too (`VK_LBUTTON` = 1).
    Key(u16),
    /// The caller read the entire keyboard at once and its actual binding is not observable.
    WholeKeyboard,
    /// Likewise for the mouse.
    WholeMouse,
    /// Likewise for a gamepad.
    WholeGamepad,
}

/// `VK_SHIFT`, `VK_CONTROL`, `VK_MENU` and their sided forms.
///
/// Two mods both reading Shift is not a conflict -- it is two mods using Shift as a modifier,
/// which is the normal and correct thing to do. Lumping those in with a real trigger collision
/// would put noise at the top of the report and teach the reader to skip it.
const MODIFIER_KEYS: &[u16] = &[0x10, 0x11, 0x12, 0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5];

impl InputId {
    /// How the input is written in the report.
    pub fn describe(self) -> String {
        match self {
            InputId::Key(vk) => key_name(i32::from(vk)),
            InputId::WholeKeyboard => "<entire keyboard>".to_string(),
            InputId::WholeMouse => "<entire mouse>".to_string(),
            InputId::WholeGamepad => "<entire gamepad>".to_string(),
        }
    }

    /// Is this a specific key rather than a whole-device read?
    pub const fn is_specific(self) -> bool {
        matches!(self, InputId::Key(_))
    }

    /// Is this one of the three modifier keys? See [`MODIFIER_KEYS`].
    pub fn is_modifier(self) -> bool {
        match self {
            InputId::Key(vk) => MODIFIER_KEYS.contains(&vk),
            _ => false,
        }
    }
}

/// An input that more than one party is interested in.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Collision {
    /// The key or device being contended.
    pub input: InputId,
    /// Every module observed taking it, sorted, excluding the game executable.
    pub modules: Vec<String>,
    /// The APIs it was taken through, sorted.
    pub surfaces: Vec<Surface>,
    /// Whether the game executable itself was also seen reading this key.
    pub game_reads_it_too: bool,
}

/// Everything observed this run, folded down to counts.
///
/// A `BTreeMap` rather than a hash map so the report is byte-identical for identical evidence:
/// a warning that reorders itself between runs cannot be diffed, and diffing two runs is exactly
/// what somebody does when they change a config and want to know whether it helped.
#[derive(Clone, Debug, Default)]
pub struct Census {
    observations: BTreeMap<(String, InputId, Surface), u64>,
    unattributed: u64,
    dropped: u64,
}

impl Census {
    /// Record `times` calls of `surface` for `input` by `module`.
    pub fn record(&mut self, module: &str, input: InputId, surface: Surface, times: u64) {
        let entry = self
            .observations
            .entry((module.to_string(), input, surface))
            .or_insert(0);
        *entry = entry.saturating_add(times);
    }

    /// Record a call whose caller could not be resolved to any loaded module.
    ///
    /// Counted rather than dropped silently: "nothing collided" and "attribution failed for every
    /// call" produce the same empty report, and they are opposite situations.
    pub fn record_unattributed(&mut self, times: u64) {
        self.unattributed = self.unattributed.saturating_add(times);
    }

    /// Record calls the hot path could not enqueue because the census was locked.
    pub fn record_dropped(&mut self, times: u64) {
        self.dropped = self.dropped.saturating_add(times);
    }

    /// Calls attributed to a named module.
    pub fn attributed_calls(&self) -> u64 {
        self.observations
            .values()
            .copied()
            .fold(0, u64::saturating_add)
    }

    /// Calls whose caller could not be named.
    pub const fn unattributed_calls(&self) -> u64 {
        self.unattributed
    }

    /// Calls the hot path had to drop.
    pub const fn dropped_calls(&self) -> u64 {
        self.dropped
    }

    /// Every module that appeared at all, sorted.
    pub fn modules(&self) -> BTreeSet<&str> {
        self.observations
            .keys()
            .map(|(module, _, _)| module.as_str())
            .collect()
    }

    /// Distinct `(module, input, surface)` rows, for the diagnostic status line.
    pub fn row_count(&self) -> usize {
        self.observations.len()
    }

    /// Every row, for the evidence dump. Sorted by construction.
    pub fn rows(&self) -> impl Iterator<Item = (&str, InputId, Surface, u64)> {
        self.observations
            .iter()
            .map(|((module, input, surface), count)| (module.as_str(), *input, *surface, *count))
    }

    /// Specific keys claimed by two or more distinct modules.
    ///
    /// `game_module` is the game executable's own file name; it is excluded from the module count
    /// because "the game reads this key" is a different finding, reported on the collision itself
    /// as [`Collision::game_reads_it_too`] rather than as a second mod.
    pub fn key_collisions(&self, game_module: &str) -> Vec<Collision> {
        self.collisions_over(game_module, |input| {
            input.is_specific() && !input.is_modifier()
        })
    }

    /// The same, for the three modifier keys. Kept separate; see [`MODIFIER_KEYS`].
    pub fn modifier_collisions(&self, game_module: &str) -> Vec<Collision> {
        self.collisions_over(game_module, InputId::is_modifier)
    }

    /// Modules that read a whole device at once, per device.
    ///
    /// Not a collision list. Two modules reading the raw keyboard might be bound to the same key
    /// or to different ones, and this API cannot tell -- which is the finding.
    pub fn whole_device_readers(&self, game_module: &str) -> Vec<(InputId, Vec<String>)> {
        let mut grouped: BTreeMap<InputId, BTreeSet<&str>> = BTreeMap::new();
        for (module, input, _) in self.observations.keys() {
            if input.is_specific() || module == game_module {
                continue;
            }
            grouped.entry(*input).or_default().insert(module.as_str());
        }
        grouped
            .into_iter()
            .map(|(input, modules)| {
                (
                    input,
                    modules.into_iter().map(str::to_string).collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    fn collisions_over(
        &self,
        game_module: &str,
        accept: impl Fn(InputId) -> bool,
    ) -> Vec<Collision> {
        let mut by_input: BTreeMap<InputId, (BTreeSet<&str>, BTreeSet<Surface>, bool)> =
            BTreeMap::new();
        for (module, input, surface) in self.observations.keys() {
            // Both halves of the tier split, checked here rather than assumed. A specific key can
            // only have come from an API that names one; anything else reaching this point would
            // mean a surface had been given a key it could not possibly have known, and reporting
            // that as a collision would accuse a module of a binding nobody observed.
            if !surface.names_a_key() || !accept(*input) {
                continue;
            }
            let slot = by_input.entry(*input).or_default();
            if module == game_module {
                slot.2 = true;
            } else {
                slot.0.insert(module.as_str());
                slot.1.insert(*surface);
            }
        }
        by_input
            .into_iter()
            .filter(|(_, (modules, _, _))| modules.len() >= 2)
            .map(|(input, (modules, surfaces, game))| Collision {
                input,
                modules: modules.into_iter().map(str::to_string).collect(),
                surfaces: surfaces.into_iter().collect(),
                game_reads_it_too: game,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "eldenring.exe";
    const VK_F7: u16 = 0x76;
    const VK_F8: u16 = 0x77;
    const VK_SHIFT: u16 = 0x10;

    /// The reproducer this crate was written for, in table form: `er-invasion-warp` polls F7 every
    /// frame and a second shell defaulted its own toggle to F7. Pressing F7 warped the player
    /// instead of toggling the other feature and nothing anywhere warned about it.
    #[test]
    fn two_mods_on_one_key_is_a_collision() {
        let mut census = Census::default();
        census.record(
            "er_invasion_warp.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            600,
        );
        census.record(
            "er_invasion_path.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            600,
        );
        let collisions = census.key_collisions(GAME);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].input, InputId::Key(VK_F7));
        assert_eq!(
            collisions[0].modules,
            vec!["er_invasion_path.dll", "er_invasion_warp.dll"]
        );
        assert!(!collisions[0].game_reads_it_too);
    }

    #[test]
    fn one_mod_on_a_key_is_not_a_collision() {
        let mut census = Census::default();
        census.record(
            "er_invasion_warp.dll",
            InputId::Key(VK_F8),
            Surface::AsyncKeyState,
            600,
        );
        assert!(census.key_collisions(GAME).is_empty());
    }

    /// The game reading a key is not a second mod. It is reported on the collision instead, so a
    /// key only the game and one mod use still shows up -- see [`game_only_pairs_are_still_named`].
    #[test]
    fn the_game_is_not_counted_as_a_colliding_mod() {
        let mut census = Census::default();
        census.record(GAME, InputId::Key(VK_F7), Surface::AsyncKeyState, 60);
        census.record(
            "er_invasion_warp.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            60,
        );
        assert!(
            census.key_collisions(GAME).is_empty(),
            "one mod plus the game is not a mod-vs-mod collision"
        );
    }

    #[test]
    fn game_only_pairs_are_still_named() {
        let mut census = Census::default();
        census.record(GAME, InputId::Key(VK_F7), Surface::AsyncKeyState, 60);
        census.record(
            "er_invasion_warp.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            60,
        );
        census.record(
            "er_invasion_path.dll",
            InputId::Key(VK_F7),
            Surface::AsyncKeyState,
            60,
        );
        let collisions = census.key_collisions(GAME);
        assert_eq!(collisions.len(), 1);
        assert!(collisions[0].game_reads_it_too);
    }

    /// Two mods reading Shift is two mods using a modifier, not a fight over a trigger. It is
    /// still reported, just not in the same list.
    #[test]
    fn modifiers_are_split_out_of_the_main_collision_list() {
        let mut census = Census::default();
        for module in ["er_invasion_warp.dll", "er_charm_enemies.dll"] {
            census.record(module, InputId::Key(VK_SHIFT), Surface::AsyncKeyState, 60);
        }
        assert!(census.key_collisions(GAME).is_empty());
        let modifiers = census.modifier_collisions(GAME);
        assert_eq!(modifiers.len(), 1);
        assert_eq!(modifiers[0].input, InputId::Key(VK_SHIFT));
    }

    /// A whole-device read can never be a key collision, because the key is not in the call.
    #[test]
    fn whole_device_reads_never_become_key_collisions() {
        let mut census = Census::default();
        for module in ["er_charm_enemies.dll", "er_net_effects.dll"] {
            census.record(
                module,
                InputId::WholeKeyboard,
                Surface::DirectInputKeyboard,
                600,
            );
        }
        assert!(census.key_collisions(GAME).is_empty());
        let readers = census.whole_device_readers(GAME);
        assert_eq!(readers.len(), 1);
        assert_eq!(readers[0].0, InputId::WholeKeyboard);
        assert_eq!(
            readers[0].1,
            vec!["er_charm_enemies.dll", "er_net_effects.dll"]
        );
    }

    /// The game is the biggest raw-keyboard reader in the process and listing it would say
    /// nothing: every profile has exactly one game.
    #[test]
    fn the_game_is_not_listed_as_a_raw_reader() {
        let mut census = Census::default();
        census.record(
            GAME,
            InputId::WholeKeyboard,
            Surface::DirectInputKeyboard,
            6000,
        );
        assert!(census.whole_device_readers(GAME).is_empty());
    }

    /// Ordering is part of the contract: a warning that shuffles between runs cannot be diffed.
    #[test]
    fn output_order_does_not_depend_on_insertion_order() {
        let mut forwards = Census::default();
        forwards.record("b.dll", InputId::Key(VK_F7), Surface::AsyncKeyState, 1);
        forwards.record("a.dll", InputId::Key(VK_F7), Surface::KeyState, 1);
        let mut backwards = Census::default();
        backwards.record("a.dll", InputId::Key(VK_F7), Surface::KeyState, 1);
        backwards.record("b.dll", InputId::Key(VK_F7), Surface::AsyncKeyState, 1);
        assert_eq!(
            forwards.key_collisions(GAME),
            backwards.key_collisions(GAME)
        );
        assert_eq!(
            forwards.key_collisions(GAME)[0].surfaces,
            vec![Surface::AsyncKeyState, Surface::KeyState]
        );
    }

    #[test]
    fn counts_accumulate_per_row() {
        let mut census = Census::default();
        census.record("a.dll", InputId::Key(VK_F7), Surface::AsyncKeyState, 3);
        census.record("a.dll", InputId::Key(VK_F7), Surface::AsyncKeyState, 4);
        assert_eq!(census.attributed_calls(), 7);
        assert_eq!(census.row_count(), 1);
    }

    /// "Nothing collided" and "attribution failed for every call" render the same empty list, so
    /// the second has to be countable or the report cannot tell them apart.
    #[test]
    fn unattributed_and_dropped_calls_are_counted_separately() {
        let mut census = Census::default();
        census.record_unattributed(5);
        census.record_dropped(2);
        assert_eq!(census.attributed_calls(), 0);
        assert_eq!(census.unattributed_calls(), 5);
        assert_eq!(census.dropped_calls(), 2);
    }

    #[test]
    fn keys_are_named_the_way_a_player_would_recognise_them() {
        assert_eq!(InputId::Key(VK_F7).describe(), "F7");
        assert_eq!(InputId::Key(0x2d).describe(), "Insert");
        assert_eq!(InputId::WholeKeyboard.describe(), "<entire keyboard>");
    }

    /// Which surfaces name a key is the tier-one/tier-two split, so it is asserted rather than
    /// left to whoever next adds a variant.
    #[test]
    fn only_argument_carrying_surfaces_name_a_key() {
        assert!(Surface::AsyncKeyState.names_a_key());
        assert!(Surface::KeyState.names_a_key());
        assert!(Surface::RegisterHotKey.names_a_key());
        assert!(!Surface::DirectInputKeyboard.names_a_key());
        assert!(!Surface::KeyboardState.names_a_key());
        assert!(!Surface::XInput.names_a_key());
        assert!(!Surface::LowLevelKeyboardHook.names_a_key());
    }
}
