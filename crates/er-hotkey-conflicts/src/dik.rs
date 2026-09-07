//! Scancodes, virtual keys, and catching the bindings that carry neither.
//!
//! # The blind spot this closes
//!
//! `IDirectInputDevice8::GetDeviceState` -- where Elden Ring itself reads the keyboard, and where
//! `er-charm-enemies` and `er-net-effects` read theirs -- hands back all 256 scancodes at once.
//! The caller picks its own key out of the buffer afterwards, in its own code. Hooking the call
//! therefore says *that* a module reads the keyboard and never *which key* it is bound to.
//!
//! But several of those mods do something observable: they zero their trigger key in the buffer on
//! the way back, so the game does not also act on it. That is visible. A key that
//! `GetAsyncKeyState` says is physically held, while the DirectInput buffer the game receives says
//! it is up, has been taken by somebody in the chain.
//!
//! So this module carries the scancode table needed to line the two views up, and the state
//! machine that decides when the disagreement is real rather than a sampling artefact.
//!
//! # What it cannot see, stated plainly
//!
//! * A mod that merely reads its key without blanking it consumes nothing and leaves no trace
//!   here. It still shows up as a whole-keyboard reader; its key stays unknown.
//! * The buffer is snapshotted at this DLL's position in the handler chain. A handler that
//!   registered before us blanks the byte after we have already looked, so its key is invisible.
//!   Handlers that registered after us are seen.
//! * The consumer is not named. Every module that hooks the DirectInput slot is a candidate, and
//!   the API gives nothing to tell them apart.

// Windows-only in practice; ungated so the table and the streak rule are covered by `cargo test`
// on the host, where a wrong table would otherwise only show up as a wrong warning in a game.
#![cfg_attr(not(windows), allow(dead_code))]

use std::collections::BTreeMap;

/// A DirectInput keyboard state buffer is exactly this many bytes. `DIJOYSTATE2` is 272 and
/// `DIMOUSESTATE` is 16, so an equality test -- never `>=` -- is what keeps joystick axes and
/// mouse deltas from being read as keys.
pub const KEYBOARD_STATE_BYTES: usize = 256;

/// A key is held when the high bit of its scancode byte is set.
pub const DIK_DOWN_BIT: u8 = 0x80;

/// `GetAsyncKeyState`'s high bit: the key is down right now.
pub const VK_DOWN_MASK: i16 = -0x8000;

/// DirectInput scancode -> Win32 virtual key, from `dinput.h` and `winuser.h`.
///
/// Only the keys a hotkey is plausibly bound to. The sided modifiers are listed and the
/// side-agnostic `VK_SHIFT`/`VK_CONTROL`/`VK_MENU` are deliberately NOT: those three have no
/// single scancode, so admitting them would make every modifier look half-consumed.
const DIK_TO_VK: &[(u8, u16)] = &[
    (0x01, 0x1b), // Escape
    (0x02, 0x31), // 1
    (0x03, 0x32),
    (0x04, 0x33),
    (0x05, 0x34),
    (0x06, 0x35),
    (0x07, 0x36),
    (0x08, 0x37),
    (0x09, 0x38),
    (0x0a, 0x39), // 9
    (0x0b, 0x30), // 0
    (0x0c, 0xbd), // -
    (0x0d, 0xbb), // =
    (0x0e, 0x08), // Backspace
    (0x0f, 0x09), // Tab
    (0x10, 0x51), // Q
    (0x11, 0x57),
    (0x12, 0x45),
    (0x13, 0x52),
    (0x14, 0x54),
    (0x15, 0x59),
    (0x16, 0x55),
    (0x17, 0x49),
    (0x18, 0x4f),
    (0x19, 0x50), // P
    (0x1a, 0xdb), // [
    (0x1b, 0xdd), // ]
    (0x1c, 0x0d), // Enter
    (0x1d, 0xa2), // Left Ctrl
    (0x1e, 0x41), // A
    (0x1f, 0x53),
    (0x20, 0x44),
    (0x21, 0x46),
    (0x22, 0x47),
    (0x23, 0x48),
    (0x24, 0x4a),
    (0x25, 0x4b),
    (0x26, 0x4c), // L
    (0x27, 0xba), // ;
    (0x28, 0xde), // '
    (0x29, 0xc0), // `
    (0x2a, 0xa0), // Left Shift
    (0x2b, 0xdc), // \
    (0x2c, 0x5a), // Z
    (0x2d, 0x58),
    (0x2e, 0x43),
    (0x2f, 0x56),
    (0x30, 0x42),
    (0x31, 0x4e),
    (0x32, 0x4d), // M
    (0x33, 0xbc), // ,
    (0x34, 0xbe), // .
    (0x35, 0xbf), // /
    (0x36, 0xa1), // Right Shift
    (0x37, 0x6a), // Numpad *
    (0x38, 0xa4), // Left Alt
    (0x39, 0x20), // Space
    (0x3a, 0x14), // Caps Lock
    (0x3b, 0x70), // F1
    (0x3c, 0x71),
    (0x3d, 0x72),
    (0x3e, 0x73),
    (0x3f, 0x74),
    (0x40, 0x75),
    (0x41, 0x76), // F7
    (0x42, 0x77),
    (0x43, 0x78),
    (0x44, 0x79), // F10
    (0x45, 0x90), // Num Lock
    (0x46, 0x91), // Scroll Lock
    (0x47, 0x67), // Numpad 7
    (0x48, 0x68),
    (0x49, 0x69),
    (0x4a, 0x6d), // Numpad -
    (0x4b, 0x64),
    (0x4c, 0x65),
    (0x4d, 0x66),
    (0x4e, 0x6b), // Numpad +
    (0x4f, 0x61),
    (0x50, 0x62),
    (0x51, 0x63),
    (0x52, 0x60), // Numpad 0
    (0x53, 0x6e), // Numpad .
    (0x57, 0x7a), // F11
    (0x58, 0x7b), // F12
    (0x9c, 0x0d), // Numpad Enter -- shares VK_RETURN with 0x1c
    (0x9d, 0xa3), // Right Ctrl
    (0xb5, 0x6f), // Numpad /
    (0xb8, 0xa5), // Right Alt
    (0xc5, 0x13), // Pause
    (0xc7, 0x24), // Home
    (0xc8, 0x26), // Up
    (0xc9, 0x21), // Page Up
    (0xcb, 0x25), // Left
    (0xcd, 0x27), // Right
    (0xcf, 0x23), // End
    (0xd0, 0x28), // Down
    (0xd1, 0x22), // Page Down
    (0xd2, 0x2d), // Insert
    (0xd3, 0x2e), // Delete
    (0xdb, 0x5b), // Left Win
    (0xdc, 0x5c), // Right Win
    (0xdd, 0x5d), // Menu
];

/// Every virtual key that has at least one scancode, sorted and de-duplicated.
///
/// This is the exact set worth asking `GetAsyncKeyState` about for the consumption check: a key
/// with no scancode can never appear in a DirectInput buffer, so scanning it would produce a
/// permanent false positive.
pub fn comparable_virtual_keys() -> Vec<u16> {
    let mut keys: Vec<u16> = DIK_TO_VK.iter().map(|(_, vk)| *vk).collect();
    keys.sort_unstable();
    keys.dedup();
    keys
}

/// The virtual key a scancode produces, if this table knows it.
///
/// The direction the game's own binding table needs: Elden Ring stores a binding as an internal
/// key id, resolves it to a DIK scancode through a lookup table in its own image, and this turns
/// that scancode into the virtual-key space the census counts in.
pub fn vk_for_scancode(dik: u8) -> Option<u16> {
    DIK_TO_VK
        .iter()
        .find(|(scancode, _)| *scancode == dik)
        .map(|(_, vk)| *vk)
}

/// Every scancode that produces `vk`. More than one for the keys with a numpad twin and for the
/// sided modifiers.
pub fn scancodes_for(vk: u16) -> Vec<u8> {
    DIK_TO_VK
        .iter()
        .filter(|(_, mapped)| *mapped == vk)
        .map(|(dik, _)| *dik)
        .collect()
}

/// Is `vk` shown as held anywhere in a DirectInput keyboard buffer?
pub fn down_in_buffer(vk: u16, buffer: &[u8]) -> bool {
    scancodes_for(vk)
        .into_iter()
        .any(|dik| matches!(buffer.get(usize::from(dik)), Some(byte) if byte & DIK_DOWN_BIT != 0))
}

/// How many consecutive samples must disagree before a key is called consumed.
///
/// One is not enough. The physical read and the buffer snapshot are taken at different instants,
/// so a key pressed in the gap between them looks exactly like a key somebody blanked. Two
/// samples at the scan rate mean the disagreement has persisted across a real interval, which a
/// press-and-release cannot fake.
pub const CONSUMED_STREAK: u8 = 2;

/// Keys seen physically held while the game's own DirectInput buffer says they are up.
///
/// Fed one sample at a time; a key is reported once it has disagreed for [`CONSUMED_STREAK`]
/// samples in a row, and each key is reported only once per run.
#[derive(Clone, Debug, Default)]
pub struct ConsumptionWatch {
    streaks: BTreeMap<u16, u8>,
    reported: Vec<u16>,
}

impl ConsumptionWatch {
    /// Fold one sample in, returning any keys that crossed the streak threshold this time.
    ///
    /// `physically_down` is the virtual keys `GetAsyncKeyState` reports held; `buffer` is the
    /// DirectInput keyboard state as the game will receive it.
    pub fn sample(&mut self, physically_down: &[u16], buffer: &[u8]) -> Vec<u16> {
        let mut newly_reported = Vec::new();
        if buffer.len() != KEYBOARD_STATE_BYTES {
            // Not a keyboard buffer -- a mouse or joystick state reached the snapshot. Reading
            // scancode offsets out of one finds noise, so the whole sample is discarded rather
            // than half-trusted.
            self.streaks.clear();
            return newly_reported;
        }
        for vk in physically_down {
            if down_in_buffer(*vk, buffer) {
                self.streaks.remove(vk);
                continue;
            }
            let streak = self.streaks.entry(*vk).or_insert(0);
            *streak = streak.saturating_add(1);
            if *streak >= CONSUMED_STREAK && !self.reported.contains(vk) {
                self.reported.push(*vk);
                newly_reported.push(*vk);
            }
        }
        // A key that is no longer held cannot still be mid-streak.
        self.streaks.retain(|vk, _| physically_down.contains(vk));
        newly_reported
    }

    /// Every key reported consumed so far, in the order it was first seen.
    pub fn reported(&self) -> &[u16] {
        &self.reported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_C: u16 = 0x43;
    const DIK_C: usize = 0x2e;
    const VK_LEFT: u16 = 0x25;
    const DIK_LEFT: usize = 0xcb;
    const VK_F7: u16 = 0x76;
    const DIK_F7: usize = 0x41;

    fn buffer_with(down: &[usize]) -> Vec<u8> {
        let mut buffer = vec![0u8; KEYBOARD_STATE_BYTES];
        for offset in down {
            buffer[*offset] = DIK_DOWN_BIT;
        }
        buffer
    }

    /// The table is the whole basis of the comparison, so the three keys this crate's own
    /// reproducers use are pinned rather than trusted.
    #[test]
    fn the_scancode_table_maps_the_keys_this_workspace_actually_binds() {
        assert_eq!(scancodes_for(VK_F7), vec![DIK_F7 as u8], "er-invasion-warp");
        assert_eq!(scancodes_for(VK_C), vec![DIK_C as u8], "er-charm-enemies");
        assert_eq!(
            scancodes_for(VK_LEFT),
            vec![DIK_LEFT as u8],
            "er-net-effects"
        );
        // er-net-effects' other binding: numpad multiply.
        assert_eq!(scancodes_for(0x6a), vec![0x37]);
    }

    /// Enter has a numpad twin, and a mod blanking only one of them has not taken the key.
    #[test]
    fn a_key_with_two_scancodes_counts_as_present_if_either_is_down() {
        let vk_return = 0x0d;
        assert_eq!(scancodes_for(vk_return), vec![0x1c, 0x9c]);
        assert!(down_in_buffer(vk_return, &buffer_with(&[0x1c])));
        assert!(down_in_buffer(vk_return, &buffer_with(&[0x9c])));
        assert!(!down_in_buffer(vk_return, &buffer_with(&[])));
    }

    /// The side-agnostic modifiers have no scancode of their own, so they are excluded from the
    /// comparison entirely -- otherwise every held Shift would read as consumed forever.
    #[test]
    fn the_sideless_modifiers_are_not_comparable() {
        for sideless in [0x10u16, 0x11, 0x12] {
            assert!(scancodes_for(sideless).is_empty());
        }
        let comparable = comparable_virtual_keys();
        assert!(comparable.contains(&0xa0), "left shift is comparable");
        assert!(!comparable.contains(&0x10), "bare shift is not");
    }

    /// The reproducer: a key held with its scancode blanked out of the buffer, twice in a row.
    #[test]
    fn a_blanked_key_is_reported_after_the_streak() {
        let mut watch = ConsumptionWatch::default();
        let blanked = buffer_with(&[]);
        assert!(
            watch.sample(&[VK_C], &blanked).is_empty(),
            "one sample is not enough"
        );
        assert_eq!(watch.sample(&[VK_C], &blanked), vec![VK_C]);
        assert_eq!(watch.reported(), [VK_C]);
    }

    /// The false positive the streak exists to kill: a key pressed in the gap between the two
    /// reads shows one disagreement and then agrees.
    #[test]
    fn a_single_sampling_gap_is_not_a_consumption() {
        let mut watch = ConsumptionWatch::default();
        assert!(watch.sample(&[VK_C], &buffer_with(&[])).is_empty());
        assert!(watch.sample(&[VK_C], &buffer_with(&[DIK_C])).is_empty());
        assert!(watch.sample(&[VK_C], &buffer_with(&[])).is_empty());
        assert!(watch.reported().is_empty());
    }

    /// A key the game does see is never reported however long it is held.
    #[test]
    fn a_key_the_game_receives_is_never_reported() {
        let mut watch = ConsumptionWatch::default();
        let visible = buffer_with(&[DIK_F7]);
        for _ in 0..10 {
            assert!(watch.sample(&[VK_F7], &visible).is_empty());
        }
        assert!(watch.reported().is_empty());
    }

    #[test]
    fn a_key_is_reported_once_not_once_per_sample() {
        let mut watch = ConsumptionWatch::default();
        let blanked = buffer_with(&[]);
        watch.sample(&[VK_C], &blanked);
        assert_eq!(watch.sample(&[VK_C], &blanked), vec![VK_C]);
        for _ in 0..5 {
            assert!(watch.sample(&[VK_C], &blanked).is_empty());
        }
        assert_eq!(watch.reported(), [VK_C]);
    }

    /// A mouse-sized buffer reaching the snapshot must not be read as a keyboard: 16 bytes of
    /// mouse deltas would say every key on the board had been taken.
    #[test]
    fn a_non_keyboard_buffer_is_discarded_rather_than_misread() {
        let mut watch = ConsumptionWatch::default();
        let mouse_sized = vec![0u8; 16];
        for _ in 0..5 {
            assert!(
                watch
                    .sample(&[VK_C, VK_F7, VK_LEFT], &mouse_sized)
                    .is_empty()
            );
        }
        assert!(watch.reported().is_empty());
    }

    /// Releasing the key clears its streak, so two separate taps do not add up to one report.
    #[test]
    fn releasing_a_key_resets_its_streak() {
        let mut watch = ConsumptionWatch::default();
        let blanked = buffer_with(&[]);
        assert!(watch.sample(&[VK_C], &blanked).is_empty());
        assert!(watch.sample(&[], &blanked).is_empty());
        assert!(watch.sample(&[VK_C], &blanked).is_empty());
        assert!(watch.reported().is_empty());
    }

    /// er-net-effects blanks all four arrows while its selector is open; all four are reported.
    #[test]
    fn several_keys_can_be_consumed_at_once() {
        let arrows = [0x25u16, 0x26, 0x27, 0x28];
        let mut watch = ConsumptionWatch::default();
        let blanked = buffer_with(&[]);
        watch.sample(&arrows, &blanked);
        let mut reported = watch.sample(&arrows, &blanked);
        reported.sort_unstable();
        assert_eq!(reported, arrows);
    }

    /// The reverse direction, which the game's own binding table depends on: a scancode out of
    /// Elden Ring's key-id lookup table has to land on the same virtual key the census counts in,
    /// or a game-vs-mod collision is reported against the wrong key.
    #[test]
    fn scancodes_resolve_back_to_the_virtual_key_they_came_from() {
        assert_eq!(vk_for_scancode(DIK_F7 as u8), Some(VK_F7));
        assert_eq!(vk_for_scancode(DIK_C as u8), Some(VK_C));
        assert_eq!(vk_for_scancode(DIK_LEFT as u8), Some(VK_LEFT));
        assert_eq!(vk_for_scancode(0x11), Some(0x57), "DIK_W -> VK_W");
        assert_eq!(vk_for_scancode(0x00), None, "scancode zero is not a key");
        assert_eq!(vk_for_scancode(0xff), None);
    }

    #[test]
    fn every_comparable_key_round_trips_through_the_table() {
        for vk in comparable_virtual_keys() {
            let codes = scancodes_for(vk);
            assert!(!codes.is_empty(), "{vk:#x} claims to be comparable");
            assert!(down_in_buffer(vk, &buffer_with(&[usize::from(codes[0])])));
        }
    }
}
