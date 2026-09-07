//! A binding a hook callback can read without taking a lock.
//!
//! The place a hotkey is actually matched is inside a detour on the game's own keyboard read --
//! `IDirectInputDevice8::GetDeviceState`, or a `WH_KEYBOARD_LL` procedure. That code runs on
//! whatever thread the game happened to poll from, arbitrarily often, and while it runs the game is
//! waiting. Reaching for a `Mutex` there buys two problems for no benefit: a lock the reload path
//! also wants (so the game thread can be made to wait on a file read), and a poisoned lock if
//! anything above ever panics, which would silently stop the hotkey working for the rest of the
//! session.
//!
//! A [`Chord`] is three small numbers, so it fits in one `u64` and the whole question goes away.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::keys::{Chord, Scancode, VirtualKey};

/// Bit 16 -- set when the chord carries a scancode. Without it, "no scancode" and "scancode 0"
/// would be the same bits, and scancode 0 is a real (if unused) buffer index.
const HAS_SCANCODE: u64 = 1 << 16;

/// Pack a chord into one word.
#[must_use]
pub const fn pack_chord(chord: Chord) -> u64 {
    let modifiers = chord.modifiers as u64;
    let (scancode, present) = match chord.dik {
        Some(dik) => (dik as u64, HAS_SCANCODE),
        None => (0, 0),
    };
    modifiers | (scancode << 8) | present | ((chord.vk as u64) << 32)
}

/// Unpack a chord. `None` for the all-zero word, which is the "nothing stored yet" state -- a real
/// chord always carries either a scancode or a nonzero virtual key.
#[must_use]
pub const fn unpack_chord(bits: u64) -> Option<Chord> {
    if bits == 0 {
        return None;
    }
    let dik = if bits & HAS_SCANCODE != 0 {
        Some(((bits >> 8) & 0xff) as Scancode)
    } else {
        None
    };
    Some(Chord {
        modifiers: (bits & 0xff) as u8,
        vk: ((bits >> 32) & 0xffff_ffff) as VirtualKey,
        dik,
    })
}

/// A [`Chord`] a detour can read with one atomic load.
#[derive(Debug)]
pub struct AtomicChord(AtomicU64);

impl AtomicChord {
    /// An empty slot. `const` so it can be a `static` in the DLL that owns the hook.
    #[must_use]
    pub const fn unset() -> Self {
        Self(AtomicU64::new(0))
    }

    /// A slot that already holds the built-in default, so the hook works before the first reload.
    #[must_use]
    pub const fn new(chord: Chord) -> Self {
        Self(AtomicU64::new(pack_chord(chord)))
    }

    /// Publish a new binding. Ordered so a detour that sees the new chord also sees any edge-state
    /// reset the caller did first.
    pub fn store(&self, chord: Chord) {
        self.0.store(pack_chord(chord), Ordering::SeqCst);
    }

    /// The binding in force, or `None` before anything was stored.
    #[must_use]
    pub fn load(&self) -> Option<Chord> {
        unpack_chord(self.0.load(Ordering::SeqCst))
    }
}

impl Default for AtomicChord {
    fn default() -> Self {
        Self::unset()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{
        MODIFIER_ALT, MODIFIER_CTRL, MODIFIER_SHIFT, parse_chord, parse_scancode_chord,
    };

    /// Every chord the key table can produce must survive the round trip. A packing bug here is a
    /// hotkey that fires on the wrong key, which reads as the config being ignored.
    #[test]
    fn every_named_chord_survives_the_round_trip() {
        for entry in crate::keys::NAMED_KEYS {
            for modifiers in ["", "ctrl+", "alt+", "shift+", "ctrl+alt+shift+"] {
                let raw = format!("{modifiers}{}", entry.aliases[0]);
                let chord = parse_chord(&raw).expect(&raw);
                assert_eq!(unpack_chord(pack_chord(chord)), Some(chord), "{raw}");
            }
        }
    }

    #[test]
    fn the_widest_values_survive_the_round_trip() {
        let widest = Chord {
            modifiers: MODIFIER_CTRL | MODIFIER_ALT | MODIFIER_SHIFT,
            vk: 0xffff_ffff,
            dik: Some(0xff),
        };
        assert_eq!(unpack_chord(pack_chord(widest)), Some(widest));
    }

    /// "No scancode" and "scancode 0" must not pack to the same word.
    #[test]
    fn a_missing_scancode_is_distinct_from_scancode_zero() {
        let missing = Chord {
            modifiers: 0,
            vk: 0x7f,
            dik: None,
        };
        let zero = Chord {
            modifiers: 0,
            vk: 0x7f,
            dik: Some(0),
        };
        assert_ne!(pack_chord(missing), pack_chord(zero));
        assert_eq!(unpack_chord(pack_chord(missing)), Some(missing));
        assert_eq!(unpack_chord(pack_chord(zero)), Some(zero));
    }

    #[test]
    fn an_empty_slot_reads_as_nothing_stored() {
        let slot = AtomicChord::unset();
        assert_eq!(slot.load(), None);
        let chord = parse_scancode_chord("ctrl+alt+c").expect("parse");
        slot.store(chord);
        assert_eq!(slot.load(), Some(chord));
    }

    #[test]
    fn a_preloaded_slot_works_before_the_first_reload() {
        let chord = parse_scancode_chord("insert").expect("parse");
        let slot = AtomicChord::new(chord);
        assert_eq!(slot.load(), Some(chord));
    }
}
