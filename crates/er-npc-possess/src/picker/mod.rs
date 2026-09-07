//! The PICKER -- choosing which creature you are about to become, from inside the game.
//!
//! # What it is for
//!
//! Until this layer the only way to name a creature was the `chr_id` key in
//! `er-npc-possess.toml`. That works, it hot-reloads, and it is genuinely a picker -- but it is
//! one you drive by alt-tabbing to a text editor and typing a four-digit number you have to know
//! in advance. This layer puts the same choice on screen: a list of all 408 creatures the moveset
//! table covers, by name, with what each can do, driven by the keyboard, and it stages exactly
//! what the `chr_id` key would have staged. The TOML path is unchanged and still works.
//!
//! # why it is not on the game'S own `05_010_ProfileSelect` list
//!
//! That was the recommended host, it is the one native widget in the game built for a long
//! scrolling list with per-row text, and this repo already drives it. It is still the wrong host
//! for this feature, on three independent counts, each of which is fatal on its own:
//!
//! 1. **Its prologues are already claimed, with bare hooks.** `er-quickload` detours the
//!    ProfileSelect list builder (`0x875590`), the row-populate (`0x8757e0`), the row-model build
//!    (`0x8752c0`), `MenuItemList::SetCursor` (`0x73bc10`) and the pointer hit test (`0x736c90`)
//!    through `MhHook::new` -- a private MinHook instance, not the `er-hook` union. A second DLL
//!    claiming any of them overwrites the first one's trampoline and the loser goes silent with
//!    no error: measured on 2026-08-23 as `file_open_hits = 0` for an entire session with every
//!    GFx swap the product owns inert and nothing logged by anybody.
//! 2. **The widget the recommendation describes does not exist in the vanilla movie.** The
//!    ten-row compact list, the drive strip and the per-row stat fields are produced by
//!    `er_gfx::title_05_010::stats_panel`, a runtime edit `er-quickload` serves in place of
//!    `05_010_profileselect.gfx` and then validates against a pinned length and FNV. Two DLLs
//!    cannot both substitute one file for one URL.
//! 3. **It cannot be opened from gameplay.** It is reached from a cloned System>Quit row, which
//!    needs a live `PropertyEditDialog` and its action object. The ask here was a HOTKEY, pressed
//!    while looking at the thing you want to become; a picker that requires quitting to the
//!    system menu first is a different feature.
//!
//! So the ten-slot list is not a constraint this layer had to solve -- it is a constraint it does
//! not inherit. The overlay draws as many rows as `[picker] visible_rows` asks for, and the
//! traversal problem that remains (408 rows, four directions) is solved in [`model`] by making
//! left and right jump between initials.
//!
//! # Where it draws instead
//!
//! `er_build_watermark_core::overlay_host` -- the process's single imgui context, the same
//! surface `er-invasion-path` and `er-net-effects` draw on. This module hosts it if nothing else
//! has, and registers a guest callback otherwise; either way there is exactly one `Present` hook
//! in the process, which is the whole reason that arbitration exists. It costs no game function
//! address and claims no game prologue.
//!
//! # What it cannot do, stated rather than discovered
//!
//! **It does not take input away from the game.** The keys that drive it also reach Elden Ring,
//! because suppressing them means claiming the DirectInput `GetDeviceState` prologue that three
//! other shells in this profile already share -- the exact collision
//! `scripts/me3-dll-conflicts.toml` exists to prevent, and a claim this layer deliberately does
//! not make. The shipped defaults are F10 plus the four arrow keys, and every pad binding ships
//! empty -- a judgement about where a collision is least likely rather than a reading of the
//! game's binding table, which is why all of them are rebindable while the game runs. A key that
//! never fires is reported rather than left silent; see [`crate::settings::PickerSettings`].
//!
//! **It does not summon anything.** Choosing writes `[target] mode = "chr_id"`, and that mode
//! searches the characters already loaded in the map -- `possess::game` reports `no loaded enemy
//! matches chr_id` when there is none. So the list is every creature the mod knows, which is a
//! larger set than the creatures you can become where you are standing. That gap closes when the
//! spawn layer lands; until then the honest statement is this one, and it is also in the shipped
//! config file where a player will read it before they blame the picker.
//!
//! **Nothing here is runtime-proven.** The game has not been launched against this code. The
//! catalogue, the navigation and the staging are covered by host tests; the draw is not, and
//! "the panel appears on screen" is not something a `cargo test` can assert.
//!
//! # The four parts
//!
//! * [`catalog`] -- the 408 rows, names joined onto move counts, built once on the first open.
//! * [`model`] -- where the cursor is and what a press moves it to. Pure.
//! * [`render`] -- the imgui panel, and joining the process's overlay. Windows only.
//! * this file -- the edge latches, the open/closed state, and the one line of the seam that
//!   turns a confirm into a staged `[target]`.

// The catalogue, the model and the edge/repeat logic are pure and stay ungated so `cargo test`
// proves them on the host, where none of the game bindings exist.
#![cfg_attr(not(windows), allow(dead_code))]

pub(crate) mod catalog;
pub(crate) mod model;
#[cfg(windows)]
pub(crate) mod render;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use er_hotkey_config::keys::Chord;
use er_hotkey_config::pad::PadChord;

use crate::log::possess_log;
use crate::picker::catalog::Creature;
use crate::picker::model::{Nav, PickerModel};
use crate::settings::PickerSettings;

/// Frames a direction must be held before it starts repeating. At 60fps this is a third of a
/// second -- long enough that a deliberate single step never repeats, short enough that holding
/// the key feels like holding a key.
const REPEAT_DELAY_TICKS: u32 = 20;
/// Frames between repeats once it starts. Three at 60fps is 20 rows a second, which crosses the
/// longest initial in the shipped catalogue in about three seconds.
const REPEAT_INTERVAL_TICKS: u32 = 3;

/// One held/released input turned into presses, with auto-repeat.
///
/// Repeat is not a nicety here: the catalogue is 408 rows, and a picker where every row costs a
/// separate keypress is one nobody reaches the end of. It is a state machine rather than a
/// counter comparison so that the delay-then-interval behaviour is testable without a clock --
/// [`Self::feed`] is driven by tick counts the caller supplies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct RepeatLatch {
    held: bool,
    ticks_held: u32,
}

impl RepeatLatch {
    /// Advance one frame. Returns true on the frame the input should act -- once on the press,
    /// then every [`REPEAT_INTERVAL_TICKS`] after [`REPEAT_DELAY_TICKS`] of holding.
    pub(crate) const fn feed(&mut self, down: bool) -> bool {
        if !down {
            self.held = false;
            self.ticks_held = 0;
            return false;
        }
        if !self.held {
            self.held = true;
            self.ticks_held = 0;
            return true;
        }
        self.ticks_held += 1;
        if self.ticks_held < REPEAT_DELAY_TICKS {
            return false;
        }
        (self.ticks_held - REPEAT_DELAY_TICKS).is_multiple_of(REPEAT_INTERVAL_TICKS)
    }

    /// Re-seat the latch from the current input state without producing a press.
    ///
    /// Called when a binding moves under a held key. Without it, rebinding `down` onto a key the
    /// player happens to be holding reads as a fresh press and the cursor jumps -- the same
    /// phantom-press bug `er_hotkey_config::pad::PadEdge` documents for the pad chords.
    pub(crate) const fn reseat(&mut self, down: bool) {
        self.held = down;
        self.ticks_held = 0;
    }
}

/// Every latch the picker owns. One per binding, so a rebind of one cannot disturb another.
#[derive(Clone, Copy, Debug, Default)]
struct Latches {
    toggle: RepeatLatch,
    up: RepeatLatch,
    down: RepeatLatch,
    prev_group: RepeatLatch,
    next_group: RepeatLatch,
}

/// The picker's whole state. `None` for the model means closed.
#[derive(Debug, Default)]
struct State {
    open: Option<PickerModel>,
    latches: Latches,
    /// The bindings the latches were seated against, so a rebind can be noticed.
    seated: Option<PickerSettings>,
    /// Rebuilt each frame while open and read by the render thread.
    view: Option<View>,
    /// Frames the list has been open with no navigation key having fired, and whether the
    /// resulting complaint has already been made. See [`DEAD_NAV_TICKS`].
    idle_ticks: u32,
    warned_about_dead_nav: bool,
    /// Where the cursor was when the list last closed, for any reason -- chosen, cancelled,
    /// closed by a possession starting, or disabled mid-list.
    ///
    /// Process-lifetime and deliberately not written to disk: it is a convenience about the last
    /// few seconds, not a preference, and a file would make "where the list opens" survive a game
    /// restart, which is a different promise from the one being made here. Restarting the game
    /// puts it back at whatever `[target]` names.
    last_cursor: Option<usize>,
}

/// How long an open list may go without a single navigation press before it says so.
///
/// A binding that never fires is the worst failure this module can have, because it looks exactly
/// like the feature working and the player being bad at it: the panel is there, the keys do
/// nothing, and nothing is logged. It is also easy to arrive at -- `KP_8` is `VK_NUMPAD8`, which
/// the numpad does not send with NumLock off, and that was the shipped default until it was
/// measured. Ten seconds at 60fps is long enough that reading the list is not mistaken for a
/// fault, and the complaint is made once per opening rather than per frame.
const DEAD_NAV_TICKS: u32 = 600;

/// Lazily built, so a session that never presses the picker key allocates nothing.
static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Is the list up? A lock-free mirror of `State::open`, for the render thread.
///
/// The render callback runs on every `Present` for the life of the process once the overlay is
/// installed, and the list is closed for almost all of them. Reading one atomic instead of taking
/// [`STATE`] keeps that common path off the game thread's lock entirely.
static OPEN: AtomicBool = AtomicBool::new(false);

/// One row as the panel draws it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ViewRow {
    pub(crate) creature: Creature,
    pub(crate) selected: bool,
}

/// Everything the draw needs, published by the game thread once per frame while the picker is up.
///
/// A snapshot rather than a lock the renderer takes on the live model: the render thread must
/// never see half of one frame's cursor and half of another's, and it must never block the game
/// thread to draw.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct View {
    pub(crate) rows: Vec<ViewRow>,
    /// 1-based position of the cursor in the whole catalogue, for the `12 / 408` readout.
    pub(crate) position: usize,
    pub(crate) total: usize,
    /// What a confirm would stage right now.
    pub(crate) selected: Option<Creature>,
}

fn state() -> std::sync::MutexGuard<'static, Option<State>> {
    // Poisoning here means a previous tick panicked. The picker is a list of names; its state is
    // not worth refusing to run over, and `report_panics_to` has already logged the panic that
    // poisoned it.
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Is the list up? Read by the status line and by the confirm path.
pub(crate) fn is_open() -> bool {
    state().as_ref().is_some_and(|state| state.open.is_some())
}

/// The current frame's rows, for the renderer. `None` when the picker is closed.
///
/// Check [`is_drawing`] first -- it answers the same question without a lock, and the answer is
/// "closed" on nearly every frame.
pub(crate) fn view() -> Option<View> {
    state().as_ref().and_then(|state| state.view.clone())
}

/// Lock-free "is there anything to draw". False negatives are impossible; a false positive can
/// last one frame after a close, and costs one lock that finds `None`.
pub(crate) fn is_drawing() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// One frame of the picker: sample its bindings, move the cursor, republish the view.
///
/// `pad_buttons` is the raw XInput `wButtons` word the caller already sampled this frame.
///
/// Call this after the possess hotkey's own edge has been sampled. Both read `GetAsyncKeyState`,
/// whose low bit means "pressed since the previous call ON THIS THREAD" and is cleared by
/// whichever call gets there first; the possess key is the one whose taps must not be lost, so it
/// reads first.
///
/// And call it unconditionally, folding the mod's master switch into `settings.enabled` rather
/// than skipping the call. The panel is drawn from a snapshot this function republishes; a frame
/// that does not run it leaves the last snapshot on screen, and a caller that stops running it
/// leaves the list up forever with no key able to close it.
///
/// Returns whether this frame opened the list, which is the caller's cue that the overlay now has
/// to exist. The install is not done here because it must not happen under this module's lock --
/// see [`crate::overlay::install_once`].
#[must_use]
pub(crate) fn tick(settings: PickerSettings, pad_buttons: u16) -> bool {
    // Both read before the lock, deliberately. Taking the config or engine lock while holding
    // this module's would be the only place in the crate where two locks are held at once, and a
    // lock order that exists in exactly one function is the kind nobody remembers to preserve.
    let staged = crate::config::staged_chr_id();
    let possessing = crate::engine::snapshot().0 == crate::engine::PossessionState::Active;
    let mut guard = state();
    let state = guard.get_or_insert_with(State::default);
    let opened = drive(
        state,
        settings,
        pad_buttons,
        staged,
        possessing,
        crate::input::chord_held,
        possess_log,
    );
    // Published for the render thread, which runs every frame for the life of the process once
    // the overlay is installed. Without it that thread would take this mutex 60-144 times a
    // second forever just to be told the list is closed, contending with the game thread's own
    // per-frame tick. Set here rather than inside `drive` so `drive` stays free of globals and
    // the tests can drive a local `State`.
    OPEN.store(state.open.is_some(), Ordering::Relaxed);
    // The lock is released before the caller INSTALLS anything. `install_once` waits on the
    // game's window, takes a named mutex, walks every loaded module and can end in
    // `Hudhook::apply()`, which creates a D3D12 device and suspends every thread in the process
    // to write its detours. Doing that here would hold this mutex across all of it -- and the
    // render thread takes the same mutex the instant that detour goes live.
    drop(guard);
    opened
}

/// The whole of [`tick`] over a caller-owned state, with the keyboard reader and the log sink
/// injected. Returns whether this call opened the list.
///
/// Split out for the tests: they drive a local [`State`] rather than the process-wide one, so
/// they need no game and cannot interfere with each other when `cargo test` runs them in
/// parallel. The log sink is injected for the same reason and it is not cosmetic -- `possess_log`
/// opens a CWD-relative `er-npc-possess.log`, so a test that let it through wrote that file into
/// the crate root and then had several test threads interleave appends into it. It did, until
/// this parameter existed.
fn drive(
    state: &mut State,
    settings: PickerSettings,
    pad_buttons: u16,
    staged_chr_id: Option<u32>,
    possessing: bool,
    keyboard_down: impl Fn(Chord) -> bool,
    log: impl Fn(std::fmt::Arguments<'_>),
) -> bool {
    let mut opened = false;
    let down = |chord: Option<Chord>, pad: PadChord| -> bool {
        chord.is_some_and(&keyboard_down) || pad_held(pad, pad_buttons)
    };
    let toggle = down(settings.toggle, settings.pad_toggle);
    let up = down(settings.up, settings.pad_up);
    let step_down = down(settings.down, settings.pad_down);
    let prev = down(settings.prev_group, settings.pad_prev_group);
    let next = down(settings.next_group, settings.pad_next_group);

    // A REBIND RE-seats every latch from this frame'S input and produces no press. Seating from
    // the live state rather than clearing to "not held" is the whole point: clearing would make
    // the next frame read a still-held key as a fresh press. The very first tick of the process
    // takes this branch too, which is why a key already down at load does not open the picker.
    if state.seated != Some(settings) {
        state.seated = Some(settings);
        state.latches.toggle.reseat(toggle);
        state.latches.up.reseat(up);
        state.latches.down.reseat(step_down);
        state.latches.prev_group.reseat(prev);
        state.latches.next_group.reseat(next);
        return false;
    }

    // The toggle deliberately does not repeat: holding the picker key must not flap the list open
    // and shut sixty times a second.
    let toggle_fired = state.latches.toggle.feed(toggle);
    let toggle_pressed = toggle_fired && !state.latches.toggle.repeating();
    let presses = [
        (state.latches.up.feed(up), Nav::Up),
        (state.latches.down.feed(step_down), Nav::Down),
        (state.latches.prev_group.feed(prev), Nav::PrevGroup),
        (state.latches.next_group.feed(next), Nav::NextGroup),
    ];

    if !settings.enabled {
        // The latches were still advanced above, so switching the picker back on mid-hold does
        // not read a held key as a press. Nothing else happens.
        if close_remembering(state) {
            log(format_args!(
                "picker: disabled while the list was up; list closed"
            ));
        }
        state.view = None;
        return false;
    }

    // The list is closed while you are wearing something, and this is the invariant that keeps
    // the possess hotkey unambiguous. That key confirms while the list is up and releases while
    // a possession is running, and if both could be true at once it would have to mean two
    // things on the same press -- with the wrong one, silently, being "you are still a dragon".
    // Refusing to open here makes them mutually exclusive by construction rather than by a rule
    // in a comment. You pick who to become while you are yourself; to switch, release first.
    if possessing {
        if close_remembering(state) {
            log(format_args!(
                "picker: a possession started; list closed so the possess hotkey means RELEASE \
                 again"
            ));
        }
        state.view = None;
        return false;
    }

    if toggle_pressed {
        if close_remembering(state) {
            state.view = None;
            log(format_args!("picker: closed without choosing"));
            return false;
        }
        let creatures = catalog::creatures();
        // Where it was beats what is staged. Re-opening lands exactly where you left the cursor,
        // including after choosing something -- browsing away from the staged creature and
        // closing is a position worth keeping, and the staged id is only a starting guess for the
        // first open of the session. Clamped, because the catalogue could in principle be shorter
        // than it was (it is generated, and a regenerated table is a different length).
        let start = state
            .last_cursor
            .filter(|cursor| *cursor < creatures.len())
            .or_else(|| staged_chr_id.and_then(catalog::index_of))
            .unwrap_or(0);
        state.open = Some(PickerModel::at(start, creatures.len()));
        state.idle_ticks = 0;
        state.warned_about_dead_nav = false;
        opened = true;
        log(format_args!(
            "picker: opened at {} of {} creatures -- move with the [picker] keys, then press the \
             possess hotkey to choose",
            start + 1,
            creatures.len()
        ));
    }

    let Some(model) = state.open.as_mut() else {
        return opened;
    };
    let groups = catalog::groups();
    let mut moved = false;
    for (fired, nav) in presses {
        if fired {
            model.nav(groups, nav);
            moved = true;
        }
    }
    let snapshot = *model;
    state.view = Some(build_view(snapshot, settings.visible_rows as usize));

    // The dead-binding complaint. See `DEAD_NAV_TICKS`.
    if moved {
        state.idle_ticks = 0;
    } else {
        state.idle_ticks = state.idle_ticks.saturating_add(1);
        if state.idle_ticks >= DEAD_NAV_TICKS && !state.warned_about_dead_nav {
            state.warned_about_dead_nav = true;
            log(format_args!(
                "picker: the list has been open {DEAD_NAV_TICKS} frames and not one navigation \
                 key has fired. The bindings in force are up={} down={} prev_group={} \
                 next_group={} -- if those are not the keys you are pressing, edit [picker] in \
                 the config; if they are, note that a KP_* binding needs NumLock ON, because \
                 without it the numpad sends the arrow-key codes instead",
                chord_text(settings.up),
                chord_text(settings.down),
                chord_text(settings.prev_group),
                chord_text(settings.next_group),
            ));
        }
    }
    opened
}

/// A binding's name for a log line, or `(none)` when it is unbound.
fn chord_text(chord: Option<Chord>) -> String {
    chord.map_or_else(|| "(none)".to_owned(), er_hotkey_config::keys::chord_name)
}

/// Take the picker's answer, if it has one.
///
/// Called from the possess hotkey's edge: while the list is up, that key confirms rather than
/// possesses. Reusing it is deliberate -- it means the whole feature costs the player one new key
/// to learn instead of two, and "the key that starts a possession also chooses what to possess"
/// is the sentence the config file can print.
///
/// That the key means two things is safe only because the two states are mutually exclusive:
/// [`drive`] refuses to open the list while a possession is running and closes it if one starts,
/// so this can never fire on the press that was meant to release.
///
/// Returns the chosen creature and closes the list. `None` when the picker is not open, in which
/// case the caller possesses as it always did.
pub(crate) fn take_confirm() -> Option<Creature> {
    let mut guard = state();
    let chosen = take_confirm_from(guard.as_mut()?);
    OPEN.store(false, Ordering::Relaxed);
    chosen
}

/// Close the list, remembering where the cursor was. Every close goes through here.
///
/// Returns whether anything was open, which is what the callers used to get from `Option::take`.
/// A close path that reached for `state.open.take()` directly would compile and would silently
/// forget the position, and that is a defect nobody would report as one -- it just feels like the
/// list opens in the wrong place sometimes.
fn close_remembering(state: &mut State) -> bool {
    match state.open.take() {
        Some(model) => {
            state.last_cursor = Some(model.cursor());
            true
        }
        None => false,
    }
}

fn take_confirm_from(state: &mut State) -> Option<Creature> {
    let model = state.open.take()?;
    state.last_cursor = Some(model.cursor());
    state.view = None;
    let chosen = catalog::creatures().get(model.cursor()).copied();
    if chosen.is_none() {
        possess_log(format_args!(
            "picker: confirmed with an empty catalogue; nothing staged"
        ));
    }
    chosen
}

/// Build one frame's rows out of the cursor.
fn build_view(model: PickerModel, visible_rows: usize) -> View {
    let creatures = catalog::creatures();
    let window = model.window(creatures.len(), visible_rows);
    let cursor = model.cursor();
    View {
        rows: creatures[window.top..window.top + window.count]
            .iter()
            .enumerate()
            .map(|(offset, creature)| ViewRow {
                creature: *creature,
                selected: window.top + offset == cursor,
            })
            .collect(),
        position: if creatures.is_empty() { 0 } else { cursor + 1 },
        total: creatures.len(),
        selected: creatures.get(cursor).copied(),
    }
}

/// Is every button of `chord` down in this frame's `wButtons`? An empty chord is unbound and is
/// never held, which is what makes `PadChord::default()` a usable "no binding".
const fn pad_held(chord: PadChord, buttons: u16) -> bool {
    chord.0 != 0 && buttons & chord.0 == chord.0
}

impl RepeatLatch {
    /// True once the latch has begun auto-repeating, so a binding that must not repeat can say so.
    const fn repeating(self) -> bool {
        self.held && self.ticks_held >= REPEAT_DELAY_TICKS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_latch_fires_once_on_the_press_and_not_again_while_merely_held() {
        let mut latch = RepeatLatch::default();
        assert!(latch.feed(true), "the press itself");
        for tick in 0..REPEAT_DELAY_TICKS - 1 {
            assert!(!latch.feed(true), "still inside the delay at tick {tick}");
        }
    }

    #[test]
    fn a_held_latch_repeats_on_the_interval_after_the_delay() {
        let mut latch = RepeatLatch::default();
        assert!(latch.feed(true));
        let mut fired = 0;
        // Enough frames to be well past the delay, so the count is the interval rather than the
        // boundary.
        for _ in 0..REPEAT_DELAY_TICKS + REPEAT_INTERVAL_TICKS * 10 {
            if latch.feed(true) {
                fired += 1;
            }
        }
        assert!(fired >= 10, "{fired} repeats");
        assert!(
            fired <= 12,
            "{fired} repeats is faster than the declared interval"
        );
    }

    #[test]
    fn releasing_a_latch_re_arms_it() {
        let mut latch = RepeatLatch::default();
        assert!(latch.feed(true));
        assert!(!latch.feed(false));
        assert!(latch.feed(true), "a second press fires again");
    }

    /// The phantom press a rebind under a held key would otherwise produce.
    #[test]
    fn re_seating_under_a_held_key_produces_no_press() {
        let mut latch = RepeatLatch::default();
        latch.reseat(true);
        assert!(!latch.feed(true), "already held; not a new press");
        assert!(!latch.feed(false));
        assert!(
            latch.feed(true),
            "and a real press after a release still fires"
        );
    }

    #[test]
    fn a_pad_chord_needs_every_button_and_an_empty_one_is_never_held() {
        assert!(pad_held(PadChord(0x0021), 0x0021));
        assert!(pad_held(PadChord(0x0021), 0x00ff), "extra buttons are fine");
        assert!(
            !pad_held(PadChord(0x0021), 0x0020),
            "one of two is not held"
        );
        assert!(!pad_held(PadChord(0), 0xffff), "unbound is never held");
        assert!(!pad_held(PadChord(0), 0));
    }

    /// A `Creature` in the view is the same value the catalogue holds, so the panel cannot show a
    /// name that belongs to a different id.
    #[test]
    fn the_view_window_carries_the_catalogue_rows_it_names() {
        let creatures = catalog::creatures();
        let model = PickerModel::at(200, creatures.len());
        let view = build_view(model, 15);
        assert_eq!(view.rows.len(), 15);
        assert_eq!(view.total, creatures.len());
        assert_eq!(view.position, 201);
        let selected = view.rows.iter().filter(|row| row.selected).count();
        assert_eq!(selected, 1, "exactly one row is highlighted");
        let highlighted = view.rows.iter().find(|row| row.selected).unwrap();
        assert_eq!(highlighted.creature, creatures[200]);
        assert_eq!(view.selected, Some(creatures[200]));
    }

    #[test]
    fn the_view_shows_the_whole_catalogue_when_it_is_shorter_than_the_viewport() {
        let view = build_view(PickerModel::at(0, 3), 5000);
        assert_eq!(view.rows.len(), catalog::creatures().len());
    }

    /// A local rig, so no test touches the process-wide `STATE` and `cargo test`'s parallelism
    /// cannot make one test's cursor another's.
    struct Rig {
        state: State,
        settings: PickerSettings,
        /// What `[target]` has staged, injected rather than read from the process-wide config.
        /// Reading the real one would build it, and building it writes `er-npc-possess.toml`
        /// into whatever directory `cargo test` happens to run in -- which it did, into the
        /// crate root, before this field existed.
        staged: Option<u32>,
        /// Whether a possession is running, injected for the same reason.
        possessing: bool,
        /// Whether the last tick opened the list -- the signal the real caller uses to install
        /// the overlay.
        opened: bool,
        /// Every log line the picker emitted, captured instead of written. See `drive`'s docs for
        /// why the sink is a parameter at all.
        log: std::cell::RefCell<Vec<String>>,
    }

    impl Rig {
        fn new() -> Self {
            let mut rig = Self {
                state: State::default(),
                settings: PickerSettings::default(),
                staged: None,
                possessing: false,
                opened: false,
                log: std::cell::RefCell::new(Vec::new()),
            };
            // Seat the latches. The first tick after a (re)bind is deliberately inert.
            rig.tick(None);
            rig
        }

        /// One frame with `held` down and nothing else.
        fn tick(&mut self, held: Option<Chord>) {
            self.opened = drive(
                &mut self.state,
                self.settings,
                0,
                self.staged,
                self.possessing,
                |chord| held == Some(chord),
                |args| self.log.borrow_mut().push(args.to_string()),
            );
        }

        /// A press and its release, which is what a real tap is.
        fn tap(&mut self, chord: Option<Chord>) {
            self.tick(chord);
            self.tick(None);
        }

        fn toggle(&self) -> Option<Chord> {
            self.settings.toggle
        }

        fn is_open(&self) -> bool {
            self.state.open.is_some()
        }

        fn view(&self) -> Option<&View> {
            self.state.view.as_ref()
        }
    }

    /// Driving the whole tick on the host, with the keyboard reader injected: a press of the
    /// toggle opens the list, a press of `down` moves it, and a second toggle closes it.
    #[test]
    fn the_tick_opens_navigates_and_closes_without_a_game() {
        let mut rig = Rig::new();
        assert!(!rig.is_open(), "a bare tick opens nothing");
        let down = rig.settings.down.expect("the shipped down default parses");

        rig.tick(rig.toggle());
        assert!(rig.is_open(), "the toggle press opened it");
        let first = rig.view().expect("open means a view").position;

        rig.tick(None);
        rig.tick(Some(down));
        let second = rig.view().expect("still open").position;
        assert_eq!(second, first + 1, "one press moved one row");

        rig.tick(None);
        rig.tick(rig.toggle());
        assert!(!rig.is_open(), "the second toggle press closed it");
        assert!(rig.view().is_none());
    }

    /// The whole point of the group axis, driven through the tick rather than the model: four
    /// presses of `next_group` land on four different initials.
    #[test]
    fn the_group_key_walks_initials_through_the_tick() {
        let mut rig = Rig::new();
        rig.tap(rig.toggle());
        let next = rig
            .settings
            .next_group
            .expect("the shipped next_group default parses");
        let mut initials = Vec::new();
        for _ in 0..4 {
            let creature = rig.view().and_then(|view| view.selected).expect("selected");
            initials.push(creature.group());
            rig.tap(Some(next));
        }
        initials.dedup();
        assert_eq!(initials.len(), 4, "each press moved to a new initial");
    }

    #[test]
    fn confirming_hands_back_the_highlighted_creature_and_closes_the_list() {
        let mut rig = Rig::new();
        assert_eq!(
            take_confirm_from(&mut rig.state),
            None,
            "closed: the possess key still possesses"
        );

        rig.tap(rig.toggle());
        assert!(rig.is_open());
        let showing = rig
            .view()
            .and_then(|view| view.selected)
            .expect("a selection");
        let confirmed = take_confirm_from(&mut rig.state).expect("open: the possess key confirms");
        assert_eq!(confirmed, showing);
        assert!(!rig.is_open(), "confirming closes the list");
        assert!(rig.view().is_none());
    }

    /// The master switch has to close a list that is already up, or turning the picker off leaves
    /// a panel on screen that no key can dismiss.
    #[test]
    fn disabling_the_picker_closes_an_open_list() {
        let mut rig = Rig::new();
        rig.tap(rig.toggle());
        assert!(rig.is_open());

        rig.settings.enabled = false;
        rig.tick(None); // re-seats under the new settings
        rig.tick(None);
        assert!(!rig.is_open());
        assert!(rig.view().is_none());
    }

    /// Holding the toggle must not flap the list. Every other binding repeats; this one does not.
    #[test]
    fn holding_the_toggle_opens_the_list_once_and_leaves_it_open() {
        let mut rig = Rig::new();
        let toggle = rig.toggle();
        for _ in 0..REPEAT_DELAY_TICKS + REPEAT_INTERVAL_TICKS * 20 {
            rig.tick(toggle);
        }
        assert!(rig.is_open(), "still open after a long hold");
    }

    /// Re-opening lands where you left off. Without this the picker is a list you have to walk
    /// back through every single time you want to adjust a choice you already made.
    #[test]
    fn the_list_opens_on_whatever_is_already_staged() {
        let mut rig = Rig::new();
        rig.staged = Some(4630);
        rig.tap(rig.toggle());
        let selected = rig.view().and_then(|view| view.selected).expect("selected");
        assert_eq!(selected.chr_id, 4630);
        assert_eq!(selected.name, "Runebear");
    }

    /// ...and a staged id the catalogue does not have opens at the top rather than refusing to
    /// open. `[target] chr_id` accepts any integer, so this is reachable from a typo.
    #[test]
    fn a_staged_id_the_catalogue_does_not_have_opens_at_the_top() {
        let mut rig = Rig::new();
        rig.staged = Some(999_999);
        rig.tap(rig.toggle());
        assert!(rig.is_open());
        assert_eq!(rig.view().expect("a view").position, 1);
    }

    /// The invariant that keeps the possess hotkey meaning one thing: no list while wearing
    /// something, so that key is confirm or release and never both.
    #[test]
    fn a_possession_closes_the_list_and_refuses_to_open_a_new_one() {
        let mut rig = Rig::new();
        rig.tap(rig.toggle());
        assert!(rig.is_open());

        rig.possessing = true;
        rig.tick(None);
        assert!(
            !rig.is_open(),
            "the list closed when the possession started"
        );
        assert!(rig.view().is_none());

        rig.tap(rig.toggle());
        assert!(
            !rig.is_open(),
            "and it will not open again while possessing"
        );

        rig.possessing = false;
        rig.tap(rig.toggle());
        assert!(rig.is_open(), "releasing gives the list back");
    }

    /// A binding that never fires must not be silent. This is the failure the keypad defaults
    /// would have shipped -- `KP_8` is `VK_NUMPAD8`, which the numpad does not send with NumLock
    /// off -- and it looks exactly like the feature working and the player being bad at it.
    #[test]
    fn an_open_list_that_never_moves_says_so_once() {
        let mut rig = Rig::new();
        rig.tap(rig.toggle());
        assert!(rig.is_open());
        rig.log.borrow_mut().clear();

        for _ in 0..DEAD_NAV_TICKS + 500 {
            rig.tick(None);
        }
        let complaints = rig
            .log
            .borrow()
            .iter()
            .filter(|line| line.contains("not one navigation key has fired"))
            .count();
        assert_eq!(complaints, 1, "once per opening, not once per frame");
        assert!(
            rig.log.borrow().iter().any(|line| line.contains("NumLock")),
            "the complaint has to name the cause a player can act on"
        );
    }

    /// ...and a list that is being driven never complains.
    #[test]
    fn a_list_being_navigated_never_complains() {
        let mut rig = Rig::new();
        rig.tap(rig.toggle());
        let down = rig.settings.down.expect("default down parses");
        rig.log.borrow_mut().clear();
        for _ in 0..DEAD_NAV_TICKS + 200 {
            rig.tap(Some(down));
        }
        assert!(
            !rig.log
                .borrow()
                .iter()
                .any(|line| line.contains("not one navigation key has fired"))
        );
    }

    /// The caller installs the overlay off this signal, so it has to be true exactly once.
    #[test]
    fn only_the_frame_that_opens_the_list_reports_opening_it() {
        let mut rig = Rig::new();
        assert!(!rig.opened, "a bare tick opens nothing");
        rig.tick(rig.toggle());
        assert!(rig.opened, "this frame opened it");
        rig.tick(None);
        assert!(!rig.opened, "and no later frame claims to have");
        assert!(rig.is_open());
    }

    /// The unbound case: with no toggle bound at all, nothing the player does opens the picker.
    /// Every close remembers the cursor, and a close with nothing open changes nothing. This is
    /// the property the four call sites depend on and the one a fifth could silently break.
    #[test]
    fn closing_the_list_remembers_where_the_cursor_was() {
        let mut state = State::default();
        assert!(!close_remembering(&mut state), "nothing was open");
        assert_eq!(state.last_cursor, None, "a no-op close invents no position");

        state.open = Some(PickerModel::at(37, 408));
        assert!(close_remembering(&mut state));
        assert_eq!(state.last_cursor, Some(37));
        assert!(state.open.is_none(), "and it really did close");

        // A second close with nothing open must not erase what the first one learned.
        assert!(!close_remembering(&mut state));
        assert_eq!(state.last_cursor, Some(37));
    }

    /// Confirming is a close too, so choosing a creature and re-opening lands on it rather than
    /// back at the top -- the case a `take()` on the confirm path alone would have missed.
    #[test]
    fn confirming_also_remembers_the_position() {
        let mut state = State {
            open: Some(PickerModel::at(12, 408)),
            ..Default::default()
        };
        let _ = take_confirm_from(&mut state);
        assert_eq!(state.last_cursor, Some(12));
    }

    #[test]
    fn an_unbound_toggle_cannot_open_the_list() {
        let mut rig = Rig::new();
        rig.settings.toggle = None;
        rig.settings.pad_toggle = PadChord::default();
        rig.tick(None);
        for _ in 0..10 {
            rig.tick(rig.settings.down);
        }
        assert!(!rig.is_open());
    }
}
