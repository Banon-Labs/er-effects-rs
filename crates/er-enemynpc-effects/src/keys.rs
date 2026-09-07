//! Reading the configured hotkey out of the buffer Elden Ring itself reads.
//!
//! Elden Ring reads the keyboard through `IDirectInputDevice8::GetDeviceState`, which fills a
//! 256-byte table indexed by *scancode*, not by virtual key. So a hotkey has to be expressed in DIK
//! codes or it can never be matched against what the game actually reads.
//!
//! The names, the scancode table and the `ctrl+alt+c` chord parser used to live here. They now live
//! in `er-hotkey-config`, which carries both numbering schemes per key so one config vocabulary
//! serves this DLL and the ones that poll `GetAsyncKeyState` instead. Every spelling this file used
//! to accept still parses -- see that crate's alias table. What is left here is the one thing that
//! cannot move: turning the raw `(size, *const u8)` a detour is handed into something safe to read.

// Windows-only in practice; kept portable so the pointer-to-slice boundary below is covered by
// `cargo test` on the host instead of being trusted from memory.
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) use er_hotkey_config::keys::{Chord, parse_scancode_chord};

/// Borrow a DirectInput state buffer as a slice, or nothing when it cannot be one.
///
/// The size check is not defensive padding. Devices of different classes can share one
/// `GetDeviceState` implementation, so the mouse arrives at the keyboard hook carrying a 16-byte
/// `DIMOUSESTATE`; reading scancode offsets out of one finds whatever is next in memory.
///
/// # Safety
/// `data` must be valid for `size` bytes for the duration of the call, which is what the detour's
/// contract with its caller already guarantees.
unsafe fn state_slice<'a>(size: u32, data: *const u8) -> Option<&'a [u8]> {
    if data.is_null() || size == 0 {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(data, size as usize) })
}

/// Are every modifier in `hotkey` and its trigger key all held in this DIK table?
///
/// # Safety
/// As [`state_slice`].
pub(crate) unsafe fn hotkey_is_down(hotkey: Chord, size: u32, data: *const u8) -> bool {
    let Some(state) = (unsafe { state_slice(size, data) }) else {
        return false;
    };
    er_hotkey_config::keys::chord_down(hotkey, state)
}

#[cfg(test)]
mod tests {
    use er_hotkey_config::keys::{
        DIK_DOWN_BIT, KeyParseError, MODIFIER_ALT, MODIFIER_CTRL, MODIFIER_SHIFT,
    };

    use super::*;

    /// `BYTE[256]` -- the DIK table a keyboard `GetDeviceState` fills.
    const KEYBOARD_STATE_BYTES: u32 = 256;

    fn state_with(down: &[usize]) -> [u8; 256] {
        let mut table = [0u8; 256];
        for offset in down {
            table[*offset] = DIK_DOWN_BIT;
        }
        table
    }

    /// The historical default still parses to the same scancode it always did, so an existing
    /// `er-enemynpc-effects.toml` keeps working across the move to the shared table.
    #[test]
    fn the_default_hotkey_still_parses_to_ctrl_alt_c() {
        let parsed = parse_scancode_chord("ctrl+alt+c").expect("ctrl+alt+c parses");
        assert_eq!(parsed.modifiers, MODIFIER_CTRL | MODIFIER_ALT);
        assert_eq!(parsed.dik, Some(0x2e));
    }

    /// Every spelling this crate's own table used to accept still resolves to the same code.
    #[test]
    fn the_old_private_spellings_still_resolve() {
        for (spelling, expected) in [
            ("numpad_add", 0x4eu8),
            ("numpad_plus", 0x4e),
            ("numpad_+", 0x4e),
            ("numpad_subtract", 0x4a),
            ("numpad_decimal", 0x53),
            ("numpad_enter", 0x9c),
            ("numpad7", 0x47),
            ("lbracket", 0x1a),
            ("rbracket", 0x1b),
            ("apostrophe", 0x28),
            ("backtick", 0x29),
            ("insert", 0xd2),
            ("delete", 0xd3),
            ("f12", 0x58),
        ] {
            assert_eq!(
                parse_scancode_chord(spelling).map(|chord| chord.dik),
                Ok(Some(expected)),
                "{spelling}"
            );
        }
    }

    #[test]
    fn combination_matches_only_when_every_key_is_held() {
        let hotkey = parse_scancode_chord("ctrl+alt+c").expect("parse");
        let full = state_with(&[0x1d, 0x38, 0x2e]);
        assert!(unsafe { hotkey_is_down(hotkey, KEYBOARD_STATE_BYTES, full.as_ptr()) });

        let no_alt = state_with(&[0x1d, 0x2e]);
        assert!(!unsafe { hotkey_is_down(hotkey, KEYBOARD_STATE_BYTES, no_alt.as_ptr()) });

        let no_trigger = state_with(&[0x1d, 0x38]);
        assert!(!unsafe { hotkey_is_down(hotkey, KEYBOARD_STATE_BYTES, no_trigger.as_ptr()) });
    }

    #[test]
    fn either_side_of_a_modifier_counts() {
        let hotkey = parse_scancode_chord("ctrl+alt+c").expect("parse");
        let right_hand = state_with(&[0x9d, 0xb8, 0x2e]);
        assert!(unsafe { hotkey_is_down(hotkey, KEYBOARD_STATE_BYTES, right_hand.as_ptr()) });
    }

    /// A mouse-sized buffer arriving at the keyboard hook must never read as a press. Without the
    /// size check, "the hotkey was released" is what a mouse poll looks like, and a held hotkey
    /// then re-arms and toggles once per interleaved poll.
    #[test]
    fn a_short_buffer_is_never_a_press() {
        let hotkey = parse_scancode_chord("insert").expect("parse");
        let mouse_sized = [0xffu8; 16];
        assert!(!unsafe { hotkey_is_down(hotkey, 16, mouse_sized.as_ptr()) });
    }

    /// A null buffer is the other way a detour is handed nothing readable.
    #[test]
    fn a_null_buffer_is_never_a_press() {
        let hotkey = parse_scancode_chord("insert").expect("parse");
        assert!(!unsafe { hotkey_is_down(hotkey, KEYBOARD_STATE_BYTES, std::ptr::null()) });
        // ...and a zero-length one, the other shape `state_slice` has to refuse.
        assert!(!unsafe { hotkey_is_down(hotkey, 0, [0xffu8; 256].as_ptr()) });
    }

    #[test]
    fn unknown_and_incomplete_hotkeys_are_errors() {
        assert!(parse_scancode_chord("ctrl+alt+nonsense").is_err());
        assert!(parse_scancode_chord("ctrl+alt").is_err());
        assert!(parse_scancode_chord("ctrl+a+b").is_err());
        // A key with no scancode can never appear in the buffer this DLL reads, so binding to it
        // would be a hotkey that silently never fires.
        assert!(matches!(
            parse_scancode_chord("F16"),
            Err(KeyParseError::NoScancode(_))
        ));
    }

    #[test]
    fn a_bare_key_needs_no_modifier() {
        let parsed = parse_scancode_chord("f9").expect("f9 parses");
        assert_eq!(parsed.modifiers, 0);
        assert_eq!(parsed.dik, Some(0x43));
        assert_eq!(MODIFIER_SHIFT, 1 << 2, "the mask layout is part of the ABI");
    }
}
