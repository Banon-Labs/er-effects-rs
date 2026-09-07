//! Reading the two hotkeys, and the edge state that turns "held" into "pressed once".
//!
//! # Why this polls instead of hooking
//!
//! `er-enemynpc-effects` detours `IDirectInputDevice8::GetDeviceState` because it needs to suppress
//! the trigger key -- blanking the byte so the game does not also act on it. This DLL does not
//! suppress anything, so it takes the cheaper path `er-refill-all` already proved: poll
//! `GetAsyncKeyState` and `XInputGetState` from the game's own FrameBegin task.
//!
//! That is not a shortcut, it is the smaller claim. Three DLLs already detour that one
//! `GetDeviceState` slot through the shared hook union, and a fourth claimant would need a
//! `[[shared]]` row in `scripts/me3-dll-conflicts.toml`, a union-shaped handler and a proof that it
//! chains rather than installing a second MinHook instance on the same prologue -- the exact
//! configuration that cost this repo a full day when two DLLs did it accidentally. Layer 1 needs
//! none of that to know a key was pressed. If a later layer needs to steal an input from the game
//! rather than merely observe it, that is when the detour is earned, and it must go through
//! `er_hook::register_shared_hook` with a row in that table.
//!
//! The thread matters: `GetAsyncKeyState`'s low bit is "pressed since the previous call on this
//! thread", and under Wine/Proton it only reports reliably from the thread the game itself polls
//! on. FrameBegin is that thread.

// Windows-only crate in practice; the edge state below is pure logic and stays ungated so its
// tests run on the host.
#![cfg_attr(not(windows), allow(dead_code, unused_imports))]

use er_hotkey_config::{
    keys::Chord,
    pad::{PadChord, PadEdge},
};

use crate::config::LiveBindings;

/// What one poll of the devices found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InputSample {
    /// The possess hotkey went down this poll, on the keyboard.
    pub(crate) keyboard_pressed: bool,
    /// ...or on the pad.
    pub(crate) gamepad_pressed: bool,
    /// The radial chord is held right now. A level, not an edge: it opens a wheel while held.
    pub(crate) radial_held: bool,
    /// The radial hold started or stopped on this poll -- the only radial transition worth a log
    /// line, since the level itself is true for as long as a thumb is on the button.
    pub(crate) radial_changed: bool,
}

impl InputSample {
    /// Which device fired the possess hotkey, for the log line. Keyboard wins a tie because a
    /// simultaneous press on both devices is one intent, not two.
    pub(crate) const fn possess_source(self) -> Option<&'static str> {
        if self.keyboard_pressed {
            Some("keyboard")
        } else if self.gamepad_pressed {
            Some("gamepad")
        } else {
            None
        }
    }
}

/// The latches. One per binding, because they answer different questions and a shared one would
/// make releasing the pad look like releasing the keyboard.
#[derive(Debug)]
pub(crate) struct Edges {
    /// The chord the keyboard latch below is about. Without it a pad rebind would clear that
    /// latch, and a key held at that instant then reads as `down && !was_down` -- a press nobody
    /// made, fired by editing an unrelated line of the config file.
    keyboard: Option<Chord>,
    keyboard_was_down: bool,
    gamepad: PadEdge,
    radial: PadChord,
    radial_held: bool,
}

impl Edges {
    /// Seed from the bindings in force. The pad edges start un-held, which is correct at attach:
    /// nothing has been sampled yet, and the first sample decides.
    pub(crate) fn new(bindings: &LiveBindings) -> Self {
        Self {
            keyboard: bindings.keyboard,
            keyboard_was_down: false,
            gamepad: PadEdge::new(bindings.gamepad),
            radial: bindings.radial,
            radial_held: false,
        }
    }

    /// Move onto new bindings, seeding from the pad sample taken this frame.
    ///
    /// `buttons` is not decoration. If the player is holding the new chord at the instant a reload
    /// binds it, a cleared latch makes the very next poll read `held && !was_held` -- a press
    /// nobody made, fired by the act of saving the file. Seeding from the live sample says "this
    /// is an ongoing hold", and the next genuine press is the one after they let go.
    ///
    /// The keyboard latch is cleared rather than seeded, and only when the keyboard binding is
    /// what moved: `GetAsyncKeyState` cannot be sampled here without consuming its
    /// pressed-since-last-call bit, so there is nothing honest to seed it from. Each of the three
    /// bindings is compared separately for the same reason the pad latch is seeded -- an edit to
    /// one must not disturb the other two.
    ///
    /// Called every tick. In the steady state it is three comparisons and no writes.
    pub(crate) fn rebind(&mut self, bindings: &LiveBindings, buttons: u16) -> bool {
        let mut moved = self.gamepad.rebind(bindings.gamepad, buttons);
        if bindings.radial != self.radial {
            self.radial = bindings.radial;
            self.radial_held = self.radial.held_in(buttons);
            moved = true;
        }
        if bindings.keyboard != self.keyboard {
            self.keyboard = bindings.keyboard;
            self.keyboard_was_down = false;
            moved = true;
        }
        moved
    }

    /// Feed one pad sample plus the keyboard's already-computed edge.
    ///
    /// The keyboard is passed in rather than read here so the whole decision is provable on the
    /// host: everything below this line is arithmetic, and the only untestable part is the two
    /// Win32 calls in [`read_pad_buttons`] and [`keyboard_edge`].
    pub(crate) fn feed(&mut self, buttons: u16, keyboard_pressed: bool) -> InputSample {
        let radial_held = self.radial.held_in(buttons);
        let radial_changed = radial_held != self.radial_held;
        self.radial_held = radial_held;
        InputSample {
            keyboard_pressed,
            gamepad_pressed: self.gamepad.feed(buttons),
            radial_held,
            radial_changed,
        }
    }

    /// The keyboard latch, for [`keyboard_edge`] to advance.
    pub(crate) const fn keyboard_latch(&mut self) -> &mut bool {
        &mut self.keyboard_was_down
    }

    /// Is the latch set? Only the tests ask; the game path hands the `&mut` to `keyboard_edge`.
    #[cfg(test)]
    const fn keyboard_was_down(&self) -> bool {
        self.keyboard_was_down
    }
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputGamepadRaw {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    thumb_lx: i16,
    thumb_ly: i16,
    thumb_rx: i16,
    thumb_ry: i16,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputStateRaw {
    packet: u32,
    gamepad: XInputGamepadRaw,
}

/// Sample controller 0 and hand the state to `use_state`, or do nothing when there is no pad.
///
/// One resolution of `XInputGetState`, shared by the two readers, so the DLL search happens once
/// per process rather than once per consumer.
#[cfg(windows)]
fn with_pad_state(use_state: impl FnOnce(&XInputStateRaw)) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows::{
        Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
        core::PCSTR,
    };

    static XINPUT_GET_STATE: AtomicUsize = AtomicUsize::new(0);
    const PROC_ABSENT: usize = usize::MAX;

    type XInputGetStateFn = unsafe extern "system" fn(u32, *mut XInputStateRaw) -> u32;

    let cached = XINPUT_GET_STATE.load(Ordering::SeqCst);
    if cached == PROC_ABSENT {
        return;
    }
    let proc: XInputGetStateFn = if cached == 0 {
        let mut found = 0usize;
        for dll in [c"xinput1_4.dll", c"xinput1_3.dll", c"xinput9_1_0.dll"] {
            let Ok(module) = (unsafe { GetModuleHandleA(PCSTR(dll.as_ptr().cast::<u8>())) }) else {
                continue;
            };
            if let Some(address) =
                unsafe { GetProcAddress(module, PCSTR(c"XInputGetState".as_ptr().cast::<u8>())) }
            {
                found = address as usize;
                break;
            }
        }
        if found == 0 {
            XINPUT_GET_STATE.store(PROC_ABSENT, Ordering::SeqCst);
            return;
        }
        XINPUT_GET_STATE.store(found, Ordering::SeqCst);
        unsafe { std::mem::transmute::<usize, XInputGetStateFn>(found) }
    } else {
        unsafe { std::mem::transmute::<usize, XInputGetStateFn>(cached) }
    };

    let mut state = XInputStateRaw::default();
    // ERROR_SUCCESS(0) == connected. Any other result means no pad in slot 0.
    if unsafe { proc(0, &raw mut state) } == 0 {
        use_state(&state);
    }
}

/// Read controller 0's `wButtons`. Zero when no pad is connected or XInput is absent.
///
/// Resolved through `GetModuleHandle`, never `LoadLibrary`: the game loads XInput for its own
/// gamepad support, so if none of the three names is present the session is keyboard-only and the
/// pad binding is simply unavailable -- not an error worth logging sixty times a second.
#[cfg(windows)]
pub(crate) fn read_pad_buttons() -> u16 {
    let mut buttons = 0;
    with_pad_state(|state| buttons = state.gamepad.buttons);
    buttons
}

#[cfg(not(windows))]
pub(crate) const fn read_pad_buttons() -> u16 {
    0
}

/// Controller 0's left thumbstick, raw. `None` when no pad is connected.
///
/// A second `XInputGetState` call rather than widening [`read_pad_buttons`]'s return, because the
/// two have different lifetimes in the frame: the buttons are sampled before the config reload so
/// a rebind can seed its latch from them, and the stick is read much later and only while
/// something is possessed. Threading a tuple from the first call down to the second consumer would
/// tie an ordering constraint that exists for the latches onto a reader that does not have one.
///
/// Deadzone and normalisation are not applied here -- they belong to
/// [`crate::possess::intent::Stick`], where they are testable on the host. This function is the
/// one untestable line.
#[cfg(windows)]
pub(crate) fn read_left_stick() -> Option<(i16, i16)> {
    let mut out = None;
    with_pad_state(|state| out = Some((state.gamepad.thumb_lx, state.gamepad.thumb_ly)));
    out
}

#[cfg(not(windows))]
pub(crate) const fn read_left_stick() -> Option<(i16, i16)> {
    None
}

/// The keyboard standing in for the left stick: four held keys as one unit-scaled axis pair.
///
/// Returns `None` when nothing is held, which is the same answer a resting stick gives, so the
/// caller cannot tell a keyboard from a pad and does not have to. A diagonal is not normalised
/// here: `Stick::from_axes` clamps a magnitude over 1, so W+D arrives as (1, 1) and leaves as a
/// unit vector pointing where the two keys point.
pub(crate) fn read_move_keys(
    forward: Option<Chord>,
    back: Option<Chord>,
    left: Option<Chord>,
    right: Option<Chord>,
) -> Option<(f32, f32)> {
    let axis = |positive: Option<Chord>, negative: Option<Chord>| {
        let held = |chord: Option<Chord>| chord.is_some_and(chord_held);
        f32::from(i8::from(held(positive)) - i8::from(held(negative)))
    };
    let x = axis(right, left);
    let y = axis(forward, back);
    if x == 0.0 && y == 0.0 {
        return None;
    }
    Some((x, y))
}

/// XInput `wButtons` masks for the two shoulder buttons.
const PAD_LEFT_SHOULDER: u16 = 0x0100;
const PAD_RIGHT_SHOULDER: u16 = 0x0200;

/// How far a trigger must travel to count as pressed.
///
/// `r2` and `l2` are analog on an XInput pad -- they are not in `wButtons` at all, which is why
/// `er_hotkey_config`'s chord vocabulary has no spelling for them and why they need a reader of
/// their own. Microsoft's own documented dead zone is 30/255; this is deliberately higher, because
/// a moveset button that fires a heavy attack on a resting finger is worse than one that needs a
/// firm pull.
const TRIGGER_PRESSED: u8 = 96;

/// Everything the MOVESET layer reads in one frame, and from one PAD read.
///
/// Returns `(face, page)`, both held/not-held bitfields:
///
/// * `face` is the four attack inputs in [`Input::index`] order -- bit 0 `r1`, bit 1 `r2`,
///   bit 2 `l1`, bit 3 `l2`;
/// * `page` is the two attack-set page keys in [`crate::moveset::dispatch::Hand::index`] order --
///   bit 0 the right hand's, bit 1 the left hand's.
///
/// Levels, not edges -- [`FaceEdges`] turns both into presses. Keeping the two apart means the pad
/// read stays the one untestable line in this file and the edge logic is proved on the host.
///
/// One `XInputGetState`, because it is the expensive call here: with no controller attached it
/// walks the driver stack and costs on the order of a millisecond, on the game's own `FrameBegin`
/// thread, every frame. Reading it twice to answer two questions about the same gamepad state would
/// pay that twice for nothing -- and worse, the two reads could disagree, so a d-pad tap could land
/// in one and not the other.
#[cfg(windows)]
pub(crate) fn read_moveset_inputs(buttons: crate::settings::ButtonSettings) -> (u8, u8) {
    let mut face = 0u8;
    let mut pad_buttons = 0u16;
    with_pad_state(|state| {
        let pad = &state.gamepad;
        pad_buttons = pad.buttons;
        face = u8::from(pad.buttons & PAD_RIGHT_SHOULDER != 0)
            | (u8::from(pad.right_trigger >= TRIGGER_PRESSED) << 1)
            | (u8::from(pad.buttons & PAD_LEFT_SHOULDER != 0) << 2)
            | (u8::from(pad.left_trigger >= TRIGGER_PRESSED) << 3);
    });
    let down = |chord: Option<Chord>, pad_chord: PadChord| -> bool {
        chord.is_some_and(chord_held) || pad_chord.held_in(pad_buttons)
    };
    // The held bit only, never `GetAsyncKeyState`'s pressed-since-last-call bit. `crate::picker`
    // polls the same two arrow keys through `chord_held` every frame and consumes that bit, so an
    // edge built on it would lose presses depending on which module happened to run first. A
    // held-level edge through [`FaceEdges`] cannot.
    let page = u8::from(down(buttons.page_right, buttons.pad_page_right))
        | (u8::from(down(buttons.page_left, buttons.pad_page_left)) << 1);
    (face | read_mouse_face_inputs(), page)
}

#[cfg(not(windows))]
pub(crate) const fn read_moveset_inputs(_buttons: crate::settings::ButtonSettings) -> (u8, u8) {
    (0, 0)
}

/// The mouse half of the same four inputs, matching the game's own default keyboard layout:
/// `r1` left click, `r2` shift + left click, `l1` right click, `l2` shift + right click.
///
/// Copied from vanilla rather than invented so a keyboard player's fingers already know it, and so
/// this needs no new config surface. A shifted click reports only the shifted input, or every
/// heavy attack would fire a light one alongside it.
#[cfg(windows)]
fn read_mouse_face_inputs() -> u8 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    const HELD: u16 = 0x8000;
    const VK_LBUTTON: i32 = 0x01;
    const VK_RBUTTON: i32 = 0x02;
    const VK_SHIFT: i32 = 0x10;

    let held = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & HELD != 0;
    let shift = held(VK_SHIFT);
    let mut out = 0u8;
    if held(VK_LBUTTON) {
        out |= if shift { 1 << 1 } else { 1 };
    }
    if held(VK_RBUTTON) {
        out |= if shift { 1 << 3 } else { 1 << 2 };
    }
    out
}

/// Rising edges over a held/not-held bitfield -- the four moveset inputs, and the two attack-set
/// page keys, which want exactly the same treatment.
///
/// Edges rather than levels because an attack is a press, not a hold: a held trigger would refire
/// the combo sixty times a second and walk the whole moveset in a fifth of a second. A held page
/// key would spin through every page in the same fifth of a second and land somewhere arbitrary.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FaceEdges {
    held: u8,
}

impl FaceEdges {
    /// Feed one frame's levels, get back the bits that went down this frame.
    pub(crate) const fn feed(&mut self, held: u8) -> u8 {
        let pressed = held & !self.held;
        self.held = held;
        pressed
    }
}

/// One keyboard edge for the optional chord.
///
/// Both bits of `GetAsyncKeyState` are used and the low one is not optional: it means "pressed
/// since the previous call on this thread", so it catches a press that happened and was released
/// between two frames -- a tap shorter than 16ms, which is an ordinary keypress.
#[cfg(windows)]
pub(crate) fn keyboard_edge(chord: Chord, was_down: &mut bool) -> bool {
    use er_hotkey_config::keys::{MODIFIER_CTRL, MODIFIER_SHIFT};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    const HELD: u16 = 0x8000;
    const PRESSED_SINCE_LAST_CALL: u16 = 0x0001;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_SHIFT: i32 = 0x10;

    let held = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & HELD != 0;
    if (chord.modifiers & MODIFIER_CTRL != 0 && !held(VK_CONTROL))
        || (chord.needs_alt() && !held(VK_MENU))
        || (chord.modifiers & MODIFIER_SHIFT != 0 && !held(VK_SHIFT))
    {
        // A modifier is up. Drop the latch so releasing the trigger later is not read as a press.
        *was_down = false;
        return false;
    }
    let state = unsafe { GetAsyncKeyState(chord.vk as i32) } as u16;
    let down = state & HELD != 0;
    let edge = (down && !*was_down) || state & PRESSED_SINCE_LAST_CALL != 0;
    *was_down = down;
    edge
}

#[cfg(not(windows))]
pub(crate) const fn keyboard_edge(_chord: Chord, _was_down: &mut bool) -> bool {
    false
}

/// Is this chord held right now? A level read, not an edge.
///
/// The picker needs levels rather than edges because it auto-repeats: an edge detector cannot
/// tell "held for forty frames" from "pressed once", and a 408-row list that steps once per
/// keypress is one nobody reaches the end of. The repeat state machine lives in
/// [`crate::picker::RepeatLatch`], where it is testable; this is the untestable line that feeds
/// it.
///
/// Only the held bit is read, never `GetAsyncKeyState`'s low "pressed since the previous call"
/// bit -- but calling this still clears that bit for the key, because Windows clears it on read
/// regardless of which bit the caller looked at. So a chord shared with the possess hotkey would
/// have its taps stolen if this ran first, which is why `crate::picker::tick` is documented to
/// run after the possess edge has been sampled.
#[cfg(windows)]
pub(crate) fn chord_held(chord: Chord) -> bool {
    use er_hotkey_config::keys::{MODIFIER_CTRL, MODIFIER_SHIFT};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;

    const HELD: u16 = 0x8000;
    const VK_CONTROL: i32 = 0x11;
    const VK_MENU: i32 = 0x12;
    const VK_SHIFT: i32 = 0x10;

    // SAFETY: `GetAsyncKeyState` reads this thread's keyboard state and takes no pointer.
    let held = |vk: i32| unsafe { GetAsyncKeyState(vk) } as u16 & HELD != 0;
    if (chord.modifiers & MODIFIER_CTRL != 0 && !held(VK_CONTROL))
        || (chord.needs_alt() && !held(VK_MENU))
        || (chord.modifiers & MODIFIER_SHIFT != 0 && !held(VK_SHIFT))
    {
        return false;
    }
    held(chord.vk as i32)
}

#[cfg(not(windows))]
pub(crate) const fn chord_held(_chord: Chord) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use er_hotkey_config::pad::parse_pad_chord;

    use super::*;

    const PAD_BACK: u16 = 0x0020;
    const PAD_START: u16 = 0x0010;
    const PAD_DPAD_DOWN: u16 = 0x0002;
    const PAD_A: u16 = 0x1000;

    fn bindings(gamepad: &str, radial: &str) -> LiveBindings {
        LiveBindings {
            enabled: true,
            keyboard: None,
            gamepad: parse_pad_chord(gamepad).expect("gamepad chord"),
            radial: parse_pad_chord(radial).expect("radial chord"),
        }
    }

    /// Nothing held reads as a resting stick, so the caller cannot tell a keyboard from a pad.
    /// On the host `chord_held` is a const `false`, which is exactly the "nothing held" case.
    #[test]
    fn no_movement_key_held_is_the_same_answer_a_resting_stick_gives() {
        let chord = |s: &str| er_hotkey_config::keys::parse_chord(s).ok();
        assert_eq!(
            read_move_keys(chord("W"), chord("S"), chord("A"), chord("D")),
            None
        );
        // ...and an entirely unbound set cannot fire either, which is what an emptied value means.
        assert_eq!(read_move_keys(None, None, None, None), None);
    }

    #[test]
    fn the_possess_chord_fires_once_per_press_however_long_it_is_held() {
        let live = bindings("select+start", "dpad_down");
        let mut edges = Edges::new(&live);
        let both = PAD_BACK | PAD_START;
        assert!(edges.feed(both, false).gamepad_pressed, "the press");
        assert!(!edges.feed(both, false).gamepad_pressed, "still held");
        assert!(!edges.feed(0, false).gamepad_pressed, "released");
        assert!(edges.feed(both, false).gamepad_pressed, "pressed again");
    }

    /// The radial is a level, not an edge: it opens a wheel for as long as it is held, so it must
    /// stay true on every poll rather than firing once.
    #[test]
    fn the_radial_reports_a_level_and_flags_only_the_transitions() {
        let live = bindings("select+start", "dpad_down");
        let mut edges = Edges::new(&live);

        let sample = edges.feed(PAD_DPAD_DOWN, false);
        assert!(sample.radial_held);
        assert!(sample.radial_changed, "the hold started");
        let sample = edges.feed(PAD_DPAD_DOWN, false);
        assert!(sample.radial_held, "STILL held -- not a one-shot");
        assert!(!sample.radial_changed);
        let sample = edges.feed(0, false);
        assert!(!sample.radial_held);
        assert!(sample.radial_changed, "the hold ended");
    }

    /// The phantom press. Saving the config while resting on the chord that the save binds must
    /// not fire the feature.
    #[test]
    fn rebinding_onto_an_already_held_chord_does_not_fire() {
        let mut edges = Edges::new(&bindings("a", "dpad_down"));
        assert!(edges.feed(PAD_A, false).gamepad_pressed);

        let both = PAD_BACK | PAD_START;
        assert!(edges.rebind(&bindings("select+start", "dpad_down"), both));
        assert!(!edges.feed(both, false).gamepad_pressed, "an ongoing hold");
        assert!(!edges.feed(0, false).gamepad_pressed, "released");
        assert!(
            edges.feed(both, false).gamepad_pressed,
            "the first real press"
        );
    }

    /// Same rule for the radial: a rebind while the new chord is down is an ongoing hold, so the
    /// wheel does not flash open on the frame the file was saved.
    #[test]
    fn rebinding_the_radial_onto_a_held_chord_is_not_a_transition() {
        let mut edges = Edges::new(&bindings("select+start", "a"));
        edges.feed(PAD_DPAD_DOWN, false);
        assert!(edges.rebind(&bindings("select+start", "dpad_down"), PAD_DPAD_DOWN));
        let sample = edges.feed(PAD_DPAD_DOWN, false);
        assert!(sample.radial_held);
        assert!(!sample.radial_changed, "already held when it was bound");
    }

    /// The latch that must not be collateral damage. Editing the PAD chord clears the pad latch;
    /// clearing the keyboard latch at the same time would make a key held at that instant read as
    /// `down && !was_down` on the next poll -- a press fired by saving an unrelated line.
    #[test]
    fn a_pad_rebind_leaves_the_keyboard_latch_alone() {
        let mut edges = Edges::new(&bindings("a", "dpad_down"));
        *edges.keyboard_latch() = true;
        assert!(edges.rebind(&bindings("select+start", "dpad_down"), 0));
        assert!(
            edges.keyboard_was_down(),
            "the keyboard binding did not move, so its latch must not have been touched"
        );

        // ...and moving the keyboard binding does clear it, because the new key has never been
        // sampled and `GetAsyncKeyState` cannot be probed here without eating its edge bit.
        let mut moved = bindings("select+start", "dpad_down");
        moved.keyboard = Some(er_hotkey_config::keys::parse_chord("F9").expect("F9"));
        assert!(edges.rebind(&moved, 0));
        assert!(!edges.keyboard_was_down());
    }

    /// Rebinding onto the same bindings is not a move, and must not disturb either latch --
    /// otherwise every reformat of the config file re-primes the edges mid-hold.
    #[test]
    fn rebinding_onto_the_same_bindings_is_not_a_move() {
        let live = bindings("select+start", "dpad_down");
        let mut edges = Edges::new(&live);
        let both = PAD_BACK | PAD_START;
        assert!(edges.feed(both, false).gamepad_pressed);
        assert!(!edges.rebind(&live, both));
        assert!(!edges.feed(both, false).gamepad_pressed, "the same hold");
    }

    /// An unbound device never fires, and does not stop the other one working.
    #[test]
    fn an_unbound_pad_never_fires_but_the_keyboard_still_does() {
        let live = LiveBindings {
            enabled: true,
            keyboard: None,
            gamepad: PadChord::default(),
            radial: PadChord::default(),
        };
        let mut edges = Edges::new(&live);
        let sample = edges.feed(0xffff, false);
        assert!(!sample.gamepad_pressed);
        assert!(!sample.radial_held);
        assert!(edges.feed(0, true).keyboard_pressed);
    }

    /// A press on both devices in one poll is one intent, and the log line names one source.
    #[test]
    fn a_simultaneous_press_reports_a_single_source() {
        let mut edges = Edges::new(&bindings("select+start", "dpad_down"));
        let sample = edges.feed(PAD_BACK | PAD_START, true);
        assert!(sample.keyboard_pressed && sample.gamepad_pressed);
        assert_eq!(sample.possess_source(), Some("keyboard"));
        assert_eq!(InputSample::default().possess_source(), None);
        assert_eq!(
            InputSample {
                gamepad_pressed: true,
                ..InputSample::default()
            }
            .possess_source(),
            Some("gamepad")
        );
    }

    /// Off the game, both device reads are inert rather than absent -- so the tick above compiles
    /// and runs on the host without a `#[cfg]` at every call site.
    #[cfg(not(windows))]
    #[test]
    fn the_device_reads_are_inert_on_the_host() {
        assert_eq!(read_pad_buttons(), 0);
        let mut latch = false;
        assert!(!keyboard_edge(
            er_hotkey_config::keys::parse_chord("F9").expect("F9"),
            &mut latch
        ));
    }
}
