//! Controller chords -- the pad half of the hotkey vocabulary.
//!
//! This crate's [`Chord`](crate::keys::Chord) is `modifiers + one trigger key`, carried
//! in both Win32 virtual-key and DirectInput scancode numbering. A gamepad has neither numbering
//! and no notion of a modifier: "Select + Start" is two ordinary buttons held together, so the
//! natural representation is a BITMASK over `XINPUT_GAMEPAD.wButtons` and every button in it is
//! equal. Bolting a `pad: Option<u16>` onto `Chord` would have put a field on a type whose whole
//! shape is wrong for it, so this is a separate type instead.
//!
//! # The edge rule that differs from the keyboard
//!
//! `GetAsyncKeyState` has a low bit meaning "pressed since the previous call on this thread", so a
//! keyboard poll catches a press that happened and was released between two polls. **XInput has no
//! such bit** -- `XInputGetState` reports only the state at the instant it is called. A pad chord
//! must therefore edge-detect against the previous poll's `wButtons`, and a press shorter than one
//! poll interval is genuinely invisible. Polling from the game's own menu update (which is where
//! this DLL reads it) means one poll per rendered frame, so that window is a frame.
//!
//! # Why it lives here and not in a DLL
//!
//! It was crate-private to `er-refill-all` until `er-npc-possess` needed the identical thing.
//! Copying it would have made two edge detectors with two chances to reintroduce the
//! phantom-press-on-rebind bug the tests below pin down, in a crate whose entire reason for
//! existing is that hotkey handling kept being reinvented differently each time.

/// `XINPUT_GAMEPAD.wButtons` bits. Named here rather than imported so the parser and its tests
/// build on the host, where the windows-only side of the crate is compiled out.
const PAD_DPAD_UP: u16 = 0x0001;
const PAD_DPAD_DOWN: u16 = 0x0002;
const PAD_DPAD_LEFT: u16 = 0x0004;
const PAD_DPAD_RIGHT: u16 = 0x0008;
const PAD_START: u16 = 0x0010;
const PAD_BACK: u16 = 0x0020;
const PAD_LEFT_THUMB: u16 = 0x0040;
const PAD_RIGHT_THUMB: u16 = 0x0080;
const PAD_LEFT_SHOULDER: u16 = 0x0100;
const PAD_RIGHT_SHOULDER: u16 = 0x0200;
const PAD_A: u16 = 0x1000;
const PAD_B: u16 = 0x2000;
const PAD_X: u16 = 0x4000;
const PAD_Y: u16 = 0x8000;

/// Every accepted spelling of a button.
///
/// Xbox names and PlayStation names both resolve, because the button a player calls "Select" is
/// labelled Back on one pad, Share on another and Touchpad-adjacent on a third, and being told
/// `unknown pad button "select"` for the button whose face literally says Select is the kind of
/// rejection that reads as the feature being broken. Elden Ring's own menus call it Select.
const PAD_BUTTONS: &[(&str, u16)] = &[
    ("start", PAD_START),
    ("options", PAD_START),
    ("menu", PAD_START),
    ("back", PAD_BACK),
    ("select", PAD_BACK),
    ("share", PAD_BACK),
    ("view", PAD_BACK),
    ("a", PAD_A),
    ("cross", PAD_A),
    ("b", PAD_B),
    ("circle", PAD_B),
    ("x", PAD_X),
    ("square", PAD_X),
    ("y", PAD_Y),
    ("triangle", PAD_Y),
    ("lb", PAD_LEFT_SHOULDER),
    ("l1", PAD_LEFT_SHOULDER),
    ("leftshoulder", PAD_LEFT_SHOULDER),
    ("left_shoulder", PAD_LEFT_SHOULDER),
    ("rb", PAD_RIGHT_SHOULDER),
    ("r1", PAD_RIGHT_SHOULDER),
    ("rightshoulder", PAD_RIGHT_SHOULDER),
    ("right_shoulder", PAD_RIGHT_SHOULDER),
    ("ls", PAD_LEFT_THUMB),
    ("l3", PAD_LEFT_THUMB),
    ("leftthumb", PAD_LEFT_THUMB),
    ("left_thumb", PAD_LEFT_THUMB),
    ("rs", PAD_RIGHT_THUMB),
    ("r3", PAD_RIGHT_THUMB),
    ("rightthumb", PAD_RIGHT_THUMB),
    ("right_thumb", PAD_RIGHT_THUMB),
    ("dpad_up", PAD_DPAD_UP),
    ("dpadup", PAD_DPAD_UP),
    ("up", PAD_DPAD_UP),
    ("dpad_down", PAD_DPAD_DOWN),
    ("dpaddown", PAD_DPAD_DOWN),
    ("down", PAD_DPAD_DOWN),
    ("dpad_left", PAD_DPAD_LEFT),
    ("dpadleft", PAD_DPAD_LEFT),
    ("left", PAD_DPAD_LEFT),
    ("dpad_right", PAD_DPAD_RIGHT),
    ("dpadright", PAD_DPAD_RIGHT),
    ("right", PAD_DPAD_RIGHT),
];

/// Why a pad chord could not be read, in the words the log line uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PadParseError {
    Empty,
    UnknownButton(String),
}

impl std::fmt::Display for PadParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "no buttons named"),
            Self::UnknownButton(name) => write!(f, "unknown pad button {name:?}"),
        }
    }
}

/// A set of buttons that must be held together. Zero means "no pad binding".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PadChord(pub u16);

impl PadChord {
    pub const fn is_bound(self) -> bool {
        self.0 != 0
    }

    /// Is every button in the chord held in this `wButtons` sample?
    ///
    /// A subset test, not equality: requiring an exact match would mean the chord failed whenever
    /// the player happened to be holding anything else -- and on a pad, resting a thumb on a stick
    /// or nudging the d-pad is not "pressing another button" to the person doing it.
    pub const fn held_in(self, buttons: u16) -> bool {
        self.is_bound() && buttons & self.0 == self.0
    }
}

/// `"select+start"` -> the two bits, in any order, any case, spaces ignored.
pub fn parse_pad_chord(raw: &str) -> Result<PadChord, PadParseError> {
    let mut mask = 0u16;
    let mut named = 0usize;
    for part in raw.split('+') {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        let folded: String = name
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        let Some((_, bit)) = PAD_BUTTONS.iter().find(|(spelling, _)| *spelling == folded) else {
            return Err(PadParseError::UnknownButton(name.to_owned()));
        };
        mask |= bit;
        named += 1;
    }
    if named == 0 {
        return Err(PadParseError::Empty);
    }
    Ok(PadChord(mask))
}

/// Name a chord back for the log, so a player can see which buttons the DLL actually took.
///
/// Canonical spelling, not the player's: echoing their text back would show that the file was read
/// but not that it was understood, and those are the two cases a log line here has to separate.
pub fn pad_chord_name(chord: PadChord) -> String {
    if !chord.is_bound() {
        return "(none)".to_owned();
    }
    // Canonical spelling per bit, in a fixed order, so the same chord always prints the same way.
    const CANONICAL: &[(u16, &str)] = &[
        (PAD_BACK, "Select"),
        (PAD_START, "Start"),
        (PAD_LEFT_SHOULDER, "LB"),
        (PAD_RIGHT_SHOULDER, "RB"),
        (PAD_LEFT_THUMB, "LS"),
        (PAD_RIGHT_THUMB, "RS"),
        (PAD_DPAD_UP, "DPad_Up"),
        (PAD_DPAD_DOWN, "DPad_Down"),
        (PAD_DPAD_LEFT, "DPad_Left"),
        (PAD_DPAD_RIGHT, "DPad_Right"),
        (PAD_A, "A"),
        (PAD_B, "B"),
        (PAD_X, "X"),
        (PAD_Y, "Y"),
    ];
    let names: Vec<&str> = CANONICAL
        .iter()
        .filter(|(bit, _)| chord.0 & bit != 0)
        .map(|(_, name)| *name)
        .collect();
    names.join("+")
}

/// Edge detector for a pad chord.
///
/// Holds the previous poll's `wButtons` because XInput has no "pressed since last call" bit.
#[derive(Clone, Copy, Debug, Default)]
pub struct PadEdge {
    chord: PadChord,
    was_held: bool,
}

impl PadEdge {
    pub const fn new(chord: PadChord) -> Self {
        Self {
            chord,
            was_held: false,
        }
    }

    /// Move onto a different chord, seeding the held state from the current pad sample.
    ///
    /// `buttons` is not decoration and clearing `was_held` instead is a bug. If the player is
    /// holding Select+Start at the instant a config reload binds Select+Start, then a cleared
    /// latch makes the very next poll read `held && !was_held` -- a press nobody made, fired by
    /// the act of saving the file. Seeding from the live sample says "this chord is already down,
    /// it is an ongoing hold", and the next genuine press is the one after they let go.
    ///
    /// This is the pad's version of the keyboard rebind rule. The keyboard needs a discarded
    /// `GetAsyncKeyState` read as well, because its low bit has been accumulating since process
    /// start; XInput keeps no such per-thread state, so one honest sample is the whole fix.
    pub fn rebind(&mut self, chord: PadChord, buttons: u16) -> bool {
        if chord == self.chord {
            return false;
        }
        self.chord = chord;
        self.was_held = chord.held_in(buttons);
        true
    }

    /// Feed one `wButtons` sample. True exactly once per press of the whole chord.
    pub fn feed(&mut self, buttons: u16) -> bool {
        let held = self.chord.held_in(buttons);
        let edge = held && !self.was_held;
        self.was_held = held;
        edge
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_default_parses_to_select_plus_start() {
        let chord = parse_pad_chord("select+start").expect("default parses");
        assert_eq!(chord.0, PAD_BACK | PAD_START);
        assert_eq!(pad_chord_name(chord), "Select+Start");
    }

    #[test]
    fn order_case_and_spacing_do_not_matter() {
        let a = parse_pad_chord("select+start").unwrap();
        for spelling in [
            "START + SELECT",
            "Start+Back",
            " back  +  start ",
            "view+menu",
        ] {
            assert_eq!(parse_pad_chord(spelling).unwrap(), a, "{spelling}");
        }
    }

    #[test]
    fn playstation_names_resolve_to_the_same_bits_as_xbox_names() {
        assert_eq!(
            parse_pad_chord("l1+r1").unwrap(),
            parse_pad_chord("lb+rb").unwrap()
        );
        assert_eq!(
            parse_pad_chord("cross").unwrap(),
            parse_pad_chord("a").unwrap()
        );
    }

    #[test]
    fn an_unknown_button_names_itself_rather_than_failing_silently() {
        assert_eq!(
            parse_pad_chord("select+turbo"),
            Err(PadParseError::UnknownButton("turbo".to_owned()))
        );
        assert_eq!(parse_pad_chord("   "), Err(PadParseError::Empty));
    }

    /// The subset rule: holding extra buttons must not break the chord.
    #[test]
    fn extra_buttons_held_alongside_the_chord_still_fire_it() {
        let chord = parse_pad_chord("select+start").unwrap();
        assert!(chord.held_in(PAD_BACK | PAD_START));
        assert!(chord.held_in(PAD_BACK | PAD_START | PAD_A | PAD_DPAD_LEFT));
        assert!(!chord.held_in(PAD_BACK));
        assert!(!chord.held_in(PAD_START));
    }

    /// One press is one edge, however long it is held.
    #[test]
    fn holding_the_chord_fires_once_not_every_frame() {
        let mut edge = PadEdge::new(parse_pad_chord("select+start").unwrap());
        let both = PAD_BACK | PAD_START;
        assert!(edge.feed(both), "the press itself");
        assert!(!edge.feed(both), "still held");
        assert!(!edge.feed(both), "still held");
        assert!(!edge.feed(0), "released");
        assert!(edge.feed(both), "pressed again");
    }

    /// Releasing one of the two buttons re-arms, because the chord is no longer satisfied.
    #[test]
    fn releasing_half_the_chord_re_arms_it() {
        let mut edge = PadEdge::new(parse_pad_chord("select+start").unwrap());
        assert!(edge.feed(PAD_BACK | PAD_START));
        assert!(!edge.feed(PAD_BACK), "start released, chord no longer held");
        assert!(edge.feed(PAD_BACK | PAD_START), "re-pressed");
    }

    /// A rebind must not manufacture a press out of buttons that were already down.
    ///
    /// This is the phantom-press bug in its pad form: save the config while resting on the new
    /// chord and the feature fires itself.
    #[test]
    fn rebinding_onto_an_already_held_chord_does_not_fire() {
        let both = PAD_BACK | PAD_START;
        let mut edge = PadEdge::new(parse_pad_chord("a").unwrap());
        assert!(edge.feed(PAD_A), "old chord pressed");

        // Select+Start is already held at the instant the reload binds it.
        assert!(edge.rebind(parse_pad_chord("select+start").unwrap(), both));
        assert!(!edge.feed(both), "an ongoing hold is not a press");
        assert!(!edge.feed(both), "still not");
        assert!(!edge.feed(0), "released");
        assert!(edge.feed(both), "THIS is the first real press");
    }

    /// The same rebind while the new chord is not held stays armed for the next real press.
    #[test]
    fn rebinding_onto_a_released_chord_arms_normally() {
        let both = PAD_BACK | PAD_START;
        let mut edge = PadEdge::new(parse_pad_chord("a").unwrap());
        assert!(edge.rebind(parse_pad_chord("select+start").unwrap(), 0));
        assert!(edge.feed(both), "first press after the rebind counts");
    }

    /// Rebinding onto the same chord is not a change, and must not disturb the latch -- otherwise
    /// every reformat of the config file re-primes the edge mid-hold.
    #[test]
    fn rebinding_onto_the_same_chord_is_not_a_change() {
        let both = PAD_BACK | PAD_START;
        let chord = parse_pad_chord("select+start").unwrap();
        let mut edge = PadEdge::new(chord);
        assert!(edge.feed(both), "pressed");
        assert!(!edge.rebind(chord, both), "same chord, no move");
        assert!(!edge.feed(both), "still the same hold, not a new press");
    }

    #[test]
    fn an_unbound_chord_never_fires() {
        let mut edge = PadEdge::new(PadChord::default());
        assert!(!edge.feed(0xffff));
        assert!(!PadChord::default().held_in(0xffff));
    }
}
