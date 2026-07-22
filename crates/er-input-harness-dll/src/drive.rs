//! Self-drive: SEMAPHORE-GATED, TEARDOWN-ON-MISS (user 2026-07-22, bd HARNESS-drive-semaphore-gated-
//! teardown-on-miss). Every phase presses an input, then waits a BOUNDED number of frames for a SPECIFIC
//! RAM semaphore confirming the press took effect. If the semaphore does not appear in budget, the
//! harness is DERAILED: it stops driving and logs DERAILED so the run monitor tears the game down. No
//! blind A->A->A, no "advance anyway".
//!
//! KEY RE (bd TITLE-CONTINUE-is-accept-byte-not-keystate): the TITLE screen PRODUCES the inputmgr+0x90
//! keystate bitmap, so keystate Confirm 0x3d is IGNORED there -- the title's confirm signal is the
//! global accept byte base+0x4589bdc. So the title (PRESS ANY BUTTON -> menu -> Continue) is driven by
//! writing the accept byte and gating on the LOAD-STARTED semaphore (now_loading / GameMan+0xb80). The
//! IN-WORLD pause menu (System->Quit) is a CONSUMER of +0x90 and IS driven by keystate.
//!
//! Pattern chosen by the flag file `er-harness-drive-mode.txt` (`boot`|`reload`|`full`, default `full`).
//! Fires from a CSTaskImp FrameBegin task (title-active). Telemetry-only NATIVE boot+reload for the
//! vanilla FPS comparison.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::game_mem::{
    load_fsm, menu_data_ptr, now_loading, read_drive_mode_flag, world_simulating,
};
use crate::input_inject::{
    MenuEvent, advance_press_any_button, input_manager, keep_input_active,
    request_open_ingame_menu, tap_menu_event,
};
use crate::log::harness_log;

// Keystate tap cadence (in-world menu only): a clean single edge per cycle.
const TAP_SET_FRAMES: u64 = 3;
const TAP_GAP_FRAMES: u64 = 6;
const TAP_CYCLE_FRAMES: u64 = TAP_SET_FRAMES + TAP_GAP_FRAMES;
/// Frames after a keystate nav step's taps to let the menu react before advancing.
const SETTLE_FRAMES: u64 = 30;
/// Popup-accept cadence (dialog-OK id 0x01, harmless when no dialog is up).
const POPUP_SET_FRAMES: u64 = 2;
const POPUP_CYCLE_FRAMES: u64 = 8;

// ---- per-phase frame budgets (derail if the effect semaphore is not seen within) ----
/// Accept-byte title Continue: menu-open + confirm + load-kick. ~5s at 60fps.
const TITLE_CONTINUE_BUDGET: u64 = 300;
/// In-world menu open: request byte honored + window up. ~4s.
const MENU_OPEN_BUDGET: u64 = 240;
/// A keystate nav step (taps + settle + a little slack).
const NAV_BUDGET: u64 = 120;
/// Return-to-title: teardown of the world. ~10s.
const RETURN_TITLE_BUDGET: u64 = 600;
/// A load completing to genuine in-world simulation (asset load is long + slow). ~150s.
const LOAD_BUDGET: u64 = 9000;

/// Per-frame semaphore snapshot (world_sim computed once by the caller -- it mutates a rising streak).
#[derive(Clone, Copy)]
struct Sem {
    menu: usize,
    world_sim: bool,
    now_loading: bool,
    load_fsm: i32,
}

impl Sem {
    fn read(world_sim: bool) -> Self {
        Sem {
            menu: menu_data_ptr(),
            world_sim,
            now_loading: now_loading(),
            load_fsm: load_fsm(),
        }
    }
    /// A load has actually STARTED (Continue took effect): the load FSM left idle, the now-loading latch
    /// tripped, or the world is already simulating.
    fn load_started(&self) -> bool {
        self.world_sim || self.now_loading || self.load_fsm > 0
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Status {
    Running,
    Advanced,
    /// Effect semaphore not seen within budget -> the drive is derailed.
    Derailed,
}

#[derive(Clone, Copy)]
enum Phase {
    /// TITLE: write the accept byte each frame (drives PAB -> menu -> Continue). EFFECT: a load started.
    TitleContinue,
    /// Wait for genuine in-world simulation (play_time rising). EFFECT: world_sim.
    WaitInWorld,
    /// IN-WORLD: request the pause menu. EFFECT: a menu window came up.
    OpenIngameMenu,
    /// IN-WORLD keystate: MoveUp then Confirm (IngameTop -> OptionSetting). EFFECT: settle (coarse).
    NavToSystemEnter,
    /// IN-WORLD keystate: TabLeft (OptionSetting -> Quit page). EFFECT: settle (coarse).
    TabToQuit,
    /// IN-WORLD keystate: MoveDown then Confirm (return-title row). EFFECT: settle (coarse).
    ActivateReturnTitle,
    /// IN-WORLD keystate: Confirm the return-title dialog. EFFECT: world stopped simulating (at title).
    ConfirmReturnTitle,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::TitleContinue => "title_continue(accept-byte)",
            Phase::WaitInWorld => "wait_in_world",
            Phase::OpenIngameMenu => "open_ingame_menu",
            Phase::NavToSystemEnter => "nav_to_system_enter",
            Phase::TabToQuit => "tab_to_quit",
            Phase::ActivateReturnTitle => "activate_return_title",
            Phase::ConfirmReturnTitle => "confirm_return_title",
        }
    }

    fn budget(self) -> u64 {
        match self {
            Phase::TitleContinue => TITLE_CONTINUE_BUDGET,
            Phase::WaitInWorld => LOAD_BUDGET,
            Phase::OpenIngameMenu => MENU_OPEN_BUDGET,
            Phase::NavToSystemEnter | Phase::TabToQuit | Phase::ActivateReturnTitle => NAV_BUDGET,
            Phase::ConfirmReturnTitle => RETURN_TITLE_BUDGET,
        }
    }

    /// One frame of the phase. Returns Advanced (effect seen), Running, or Derailed (past budget).
    fn tick(self, base: usize, im: usize, frame: u64, sem: &Sem) -> Status {
        let advanced = match self {
            Phase::TitleContinue => {
                // Drive the title via its OWN confirm signal (accept byte), not keystate. Writing it each
                // frame walks PAB -> menu(Continue focused) -> confirm. Effect: a load began.
                advance_press_any_button(base);
                sem.load_started()
            }
            Phase::WaitInWorld => sem.world_sim,
            Phase::OpenIngameMenu => {
                if sem.menu == 0 {
                    request_open_ingame_menu(im);
                }
                sem.menu != 0
            }
            Phase::NavToSystemEnter => {
                settle_after_taps(im, &[MenuEvent::MoveUp, MenuEvent::Confirm], frame)
            }
            Phase::TabToQuit => settle_after_taps(im, &[MenuEvent::TabLeft], frame),
            Phase::ActivateReturnTitle => {
                settle_after_taps(im, &[MenuEvent::MoveDown, MenuEvent::Confirm], frame)
            }
            Phase::ConfirmReturnTitle => {
                tap_pattern(im, &[MenuEvent::Confirm], frame);
                // Returned to title = world stopped simulating (torn down) after a real hold.
                !sem.world_sim && frame > SETTLE_FRAMES && sem.load_fsm <= 0
            }
        };
        if advanced {
            Status::Advanced
        } else if frame >= self.budget() {
            Status::Derailed
        } else {
            Status::Running
        }
    }
}

/// Tap each event once per cycle, cycling through the list (used by phases that keep asserting input).
fn tap_pattern(im: usize, events: &[MenuEvent], frame: u64) {
    if events.is_empty() {
        return;
    }
    if (frame % TAP_CYCLE_FRAMES) < TAP_SET_FRAMES {
        let idx = ((frame / TAP_CYCLE_FRAMES) as usize) % events.len();
        tap_menu_event(im, events[idx]);
    }
}

/// Tap each event once in order (one cycle each), then settle. Returns true once taps are issued and
/// SETTLE_FRAMES elapsed (the coarse "the nav step ran" signal for in-world menus whose per-pane window
/// id is not resolved standalone -- the keystate menu IS a +0x90 consumer, so the taps land).
fn settle_after_taps(im: usize, events: &[MenuEvent], frame: u64) -> bool {
    let taps = events.len() as u64;
    let taps_done = frame / TAP_CYCLE_FRAMES;
    if taps_done < taps {
        if (frame % TAP_CYCLE_FRAMES) < TAP_SET_FRAMES {
            tap_menu_event(im, events[taps_done as usize]);
        }
        false
    } else {
        frame >= taps * TAP_CYCLE_FRAMES + SETTLE_FRAMES
    }
}

#[derive(Clone, Copy)]
enum DriveMode {
    BootContinueOnly,
    NativeReloadOnly,
    FullBootReload,
}

impl DriveMode {
    fn from_flag() -> Self {
        match read_drive_mode_flag().as_str() {
            "boot" => DriveMode::BootContinueOnly,
            "reload" => DriveMode::NativeReloadOnly,
            _ => DriveMode::FullBootReload,
        }
    }
    fn name(self) -> &'static str {
        match self {
            DriveMode::BootContinueOnly => "boot",
            DriveMode::NativeReloadOnly => "reload",
            DriveMode::FullBootReload => "full",
        }
    }
    fn phases(self) -> &'static [Phase] {
        const BOOT: &[Phase] = &[Phase::TitleContinue, Phase::WaitInWorld];
        const RELOAD: &[Phase] = &[
            Phase::OpenIngameMenu,
            Phase::NavToSystemEnter,
            Phase::TabToQuit,
            Phase::ActivateReturnTitle,
            Phase::ConfirmReturnTitle,
            Phase::TitleContinue,
            Phase::WaitInWorld,
        ];
        const FULL: &[Phase] = &[
            Phase::TitleContinue,
            Phase::WaitInWorld,
            Phase::OpenIngameMenu,
            Phase::NavToSystemEnter,
            Phase::TabToQuit,
            Phase::ActivateReturnTitle,
            Phase::ConfirmReturnTitle,
            Phase::TitleContinue,
            Phase::WaitInWorld,
        ];
        match self {
            DriveMode::BootContinueOnly => BOOT,
            DriveMode::NativeReloadOnly => RELOAD,
            DriveMode::FullBootReload => FULL,
        }
    }
}

static PHASE_IDX: AtomicUsize = AtomicUsize::new(0);
static PHASE_FRAME: AtomicU64 = AtomicU64::new(0);
static POPUP_FRAME: AtomicU64 = AtomicU64::new(0);
static MODE_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
static DERAILED: AtomicBool = AtomicBool::new(false);

fn resolve_mode() -> DriveMode {
    const MODES: [DriveMode; 3] = [
        DriveMode::BootContinueOnly,
        DriveMode::NativeReloadOnly,
        DriveMode::FullBootReload,
    ];
    let cached = MODE_IDX.load(Ordering::SeqCst);
    if cached != usize::MAX {
        return MODES[cached];
    }
    let mode = DriveMode::from_flag();
    let idx = match mode {
        DriveMode::BootContinueOnly => 0,
        DriveMode::NativeReloadOnly => 1,
        DriveMode::FullBootReload => 2,
    };
    MODE_IDX.store(idx, Ordering::SeqCst);
    harness_log!(
        "drive: mode='{}' phases={}",
        mode.name(),
        mode.phases().len()
    );
    mode
}

/// Run one frame of the drive. Called on the game thread from the CSTaskImp FrameBegin task.
pub fn on_frame(base: usize) {
    keep_input_active(base);

    if DERAILED.load(Ordering::SeqCst) {
        return; // stopped driving; the run monitor tears the game down on the DERAILED marker
    }

    let Some(im) = input_manager(base) else {
        return;
    };

    // GENERALLY ACCEPT POPUPS every frame (dialog-OK id 0x01; consumed only while a modal dialog is up).
    let pf = POPUP_FRAME.fetch_add(1, Ordering::SeqCst);
    if pf % POPUP_CYCLE_FRAMES < POPUP_SET_FRAMES {
        tap_menu_event(im, MenuEvent::PopupAccept);
    }

    let phases = resolve_mode().phases();
    let idx = PHASE_IDX.load(Ordering::SeqCst);
    if idx >= phases.len() {
        return; // all phases complete
    }
    let phase = phases[idx];
    let frame = PHASE_FRAME.fetch_add(1, Ordering::SeqCst);
    // world_simulating mutates a rising streak -> compute exactly once per frame.
    let sem = Sem::read(world_simulating());

    match phase.tick(base, im, frame, &sem) {
        Status::Running => {}
        Status::Advanced => {
            harness_log!(
                "phase[{idx}] {} ADVANCED after {frame}f (menu=0x{:x} world_sim={} now_loading={} load_fsm={})",
                phase.name(),
                sem.menu,
                sem.world_sim as u8,
                sem.now_loading as u8,
                sem.load_fsm
            );
            PHASE_IDX.store(idx + 1, Ordering::SeqCst);
            PHASE_FRAME.store(0, Ordering::SeqCst);
            if idx + 1 >= phases.len() {
                harness_log!("drive: DONE -- all phases complete");
            }
        }
        Status::Derailed => {
            harness_log!(
                "phase[{idx}] {} DERAILED: effect semaphore not seen within {}f budget (menu=0x{:x} world_sim={} now_loading={} load_fsm={}) -- STOPPING drive; tear down and analyze",
                phase.name(),
                phase.budget(),
                sem.menu,
                sem.world_sim as u8,
                sem.now_loading as u8,
                sem.load_fsm
            );
            DERAILED.store(true, Ordering::SeqCst);
        }
    }
}
