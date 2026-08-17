//! Which `IDirectInputDevice8::GetDeviceState` calls are KEYBOARD reads.
//!
//! THE BUG THIS EXISTS TO KILL. Every DirectInput device of a class shares one vtable, so hooking
//! the keyboard's `GetDeviceState` slot also intercepts the mouse when both resolve to the same
//! implementation -- the install path already detects that case. A mouse read hands the hook a
//! 16-byte `DIMOUSESTATE`, and reading DIK offsets out of it finds nothing, so the arrow keys all
//! look RELEASED. That wipes the held-key mask, and the next genuine keyboard read -- same frame,
//! key still physically down -- looks like a brand new press.
//!
//! The result is one queued step per interleaved poll for as long as the key is held: the
//! selector runs away and a single tap cannot be delivered reliably. It also silently disables
//! hold-repeat, because a key that re-presses every poll never survives long enough to latch.
//! Measured on run br-20260816-225144-95cb: 139 queued selector keys, 0 of them repeats.
//!
//! Sizes are the discriminator, and they are unambiguous: a keyboard state is the 256-byte DIK
//! table and nothing else is 256 bytes.

// Windows-only in practice; kept portable so the sizes below are asserted by `cargo test` on the
// host instead of being trusted from memory.
#![cfg_attr(not(windows), allow(dead_code))]

/// `BYTE[256]` -- the DIK table filled by a keyboard `GetDeviceState`.
pub(crate) const KEYBOARD_STATE_BYTES: usize = 256;

/// Is this state buffer the keyboard's DIK table?
///
/// EXACT, not `>=`: `DIJOYSTATE2` is 272 bytes and would pass a lower bound while containing
/// axis and POV data at the offsets we would read as arrow keys.
pub(crate) fn is_keyboard_state(size: u32) -> bool {
    size as usize == KEYBOARD_STATE_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    // The DirectInput state structures that share a device vtable with the keyboard, by size.
    const DIMOUSESTATE: u32 = 16;
    const DIMOUSESTATE2: u32 = 20;
    const DIJOYSTATE: u32 = 80;
    const DIJOYSTATE2: u32 = 272;

    #[test]
    fn the_dik_table_is_keyboard_state() {
        assert!(is_keyboard_state(KEYBOARD_STATE_BYTES as u32));
    }

    #[test]
    fn mouse_reads_are_not_keyboard_state() {
        // The case measured in-game: mouse polls sharing the hooked vtable entry, each one
        // previously reporting "every arrow released" and re-arming the press.
        assert!(!is_keyboard_state(DIMOUSESTATE));
        assert!(!is_keyboard_state(DIMOUSESTATE2));
    }

    #[test]
    fn joystick_reads_are_not_keyboard_state() {
        assert!(!is_keyboard_state(DIJOYSTATE));
        assert!(
            !is_keyboard_state(DIJOYSTATE2),
            "DIJOYSTATE2 is 272 bytes: a >= bound would read POV/axis data as arrow keys"
        );
    }

    #[test]
    fn a_zero_or_truncated_read_is_not_keyboard_state() {
        assert!(!is_keyboard_state(0));
        assert!(!is_keyboard_state(255));
    }
}
