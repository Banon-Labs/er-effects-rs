//! Self-drive of the FULL native product flow, split into NAMED PHASES, each with per-phase telemetry
//! (bd HARNESS-per-phase-telemetry-full-native-flow). Every phase drives its input (or waits), then
//! gates on a SPECIFIC RAM semaphore that its input took effect within a bounded frame budget. If the
//! semaphore is not seen in budget the harness is DERAILED (teardown-on-miss, bd HARNESS-drive-
//! semaphore-gated-teardown-on-miss): it stops driving and logs DERAILED so the run monitor tears the
//! game down. No blind A->A->A, no "advance anyway".
//!
//! On every phase completion (advanced OR derailed) one JSON line is appended to
//! `er-input-harness-phases.jsonl` with the phase name, duration (ms + frames), and the semaphore state
//! at exit, so vanilla and product runs can be diffed phase-by-phase.
//!
//! LAYERS (bd TITLE-CONTINUE-is-accept-byte-not-keystate): the TITLE screen PRODUCES the inputmgr+0x90
//! keystate bitmap, so keystate Confirm 0x3d is IGNORED there -- the title (PRESS ANY BUTTON -> menu ->
//! Continue) is driven by writing the global accept byte `base+0x4589bdc` and gating on the title-owner
//! RAM semaphores (owner+0x48 state, dialog+0xa40) and the LOAD-STARTED semaphores. The IN-WORLD pause
//! menu (System->Quit) is a CONSUMER of +0x90 and IS driven by keystate.
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
use crate::log::{harness_log, log_phase};
use crate::title_scan;
use crate::win32::GetTickCount64;

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
/// Boot -> PRESS ANY BUTTON ready: image map + boot-flow settle is long (~150s at 60fps).
const STARTUP_BUDGET: u64 = 9000;
/// PRESS ANY BUTTON -> Continue/Load menu built. ~5s.
const PAB_BUDGET: u64 = 300;
/// Continue -> a load started. ~5s.
const CONTINUE_BUDGET: u64 = 300;
/// A load completing to genuine in-world simulation (asset load is long + slow). ~150s.
const LOAD_BUDGET: u64 = 9000;
/// In-world: open pause menu + navigate to the return-title row. ~10s.
const MENU_FLOW_BUDGET: u64 = 600;
/// Native quit-to-menu: confirm the return-title dialog + teardown of the world. ~10s.
const QUIT_BUDGET: u64 = 600;

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
    /// NO input: wait until the title is parked at PRESS ANY BUTTON. EFFECT: title_pab_parked.
    /// (measures boot -> PAB time)
    Startup,
    /// Write the accept byte each frame (advances PAB). EFFECT: the Continue/Load menu is built.
    /// (measures PAB -> menu)
    PressAnyButton,
    /// Write the accept byte each frame (Continue is default-focused). EFFECT: a load started.
    /// (measures Continue -> load-start)
    Continue,
    /// NO input: wait for genuine in-world simulation (play_time rising). EFFECT: world_sim.
    /// (measures load duration)
    WaitLoadIn,
    /// IN-WORLD: open the pause menu, then keystate-nav to the return-title row. EFFECT: coarse settle.
    /// (the "menu flow" section)
    MenuFlow,
    /// IN-WORLD keystate Confirm: accept the return-title dialog. EFFECT: the world was torn down.
    /// (the native quit-to-menu; its telemetry captures the teardown)
    QuitToMenu,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Startup => "startup",
            Phase::PressAnyButton => "press_any_button",
            Phase::Continue => "continue",
            Phase::WaitLoadIn => "wait_load_in",
            Phase::MenuFlow => "menu_flow",
            Phase::QuitToMenu => "quit_to_menu",
        }
    }

    fn budget(self) -> u64 {
        match self {
            Phase::Startup => STARTUP_BUDGET,
            Phase::PressAnyButton => PAB_BUDGET,
            Phase::Continue => CONTINUE_BUDGET,
            Phase::WaitLoadIn => LOAD_BUDGET,
            Phase::MenuFlow => MENU_FLOW_BUDGET,
            Phase::QuitToMenu => QUIT_BUDGET,
        }
    }

    /// One frame of the phase. Returns Advanced (effect seen), Running, or Derailed (past budget).
    fn tick(self, base: usize, im: usize, frame: u64, sem: &Sem) -> Status {
        let advanced = match self {
            Phase::Startup => title_scan::title_pab_parked(base),
            Phase::PressAnyButton => {
                // Drive the title via its OWN confirm signal (accept byte), not keystate.
                advance_press_any_button(base);
                title_scan::title_menu_up(base)
            }
            Phase::Continue => {
                // Continue is default-focused once the menu is up; the accept byte confirms it.
                advance_press_any_button(base);
                sem.load_started()
            }
            Phase::WaitLoadIn => sem.world_sim,
            Phase::MenuFlow => menu_flow_tick(im, frame),
            Phase::QuitToMenu => {
                tap_pattern(im, &[MenuEvent::Confirm], frame);
                // Returned to title = world stopped simulating (torn down) after a real hold.
                !sem.world_sim && sem.load_fsm <= 0 && frame > SETTLE_FRAMES
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

/// Frame (within the MenuFlow phase) at which the pause menu first came up, or `u64::MAX` before then.
/// The keystate nav sequence is anchored to this so it starts only after the menu is actually open.
static MENU_OPENED_FRAME: AtomicU64 = AtomicU64::new(u64::MAX);

/// MenuFlow: request the in-world pause menu until a menu-data owner exists, THEN run the ordered
/// keystate nav (MoveUp,Confirm -> OptionSetting; TabLeft -> Quit page; MoveDown,Confirm -> return-title
/// row) with the standard tap cadence. Returns true (coarse settle) once the taps are issued + settled.
fn menu_flow_tick(im: usize, frame: u64) -> bool {
    // Step 1: open the pause menu (request while no menu-data owner exists).
    if menu_data_ptr() == 0 {
        request_open_ingame_menu(im);
        MENU_OPENED_FRAME.store(u64::MAX, Ordering::SeqCst);
        return false;
    }
    // Step 2: anchor the nav to the frame the menu opened, then run the ordered tap sequence.
    let opened = match MENU_OPENED_FRAME.load(Ordering::SeqCst) {
        u64::MAX => {
            MENU_OPENED_FRAME.store(frame, Ordering::SeqCst);
            frame
        }
        f => f,
    };
    let local = frame - opened;
    const NAV: &[MenuEvent] = &[
        MenuEvent::MoveUp,
        MenuEvent::Confirm,
        MenuEvent::TabLeft,
        MenuEvent::MoveDown,
        MenuEvent::Confirm,
    ];
    settle_after_taps(im, NAV, local)
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
        // boot: process start -> in-world (the four boot phases only).
        const BOOT: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
        ];
        // reload: assumes already in-world; menu flow -> native quit-to-menu -> reload Continue.
        const RELOAD: &[Phase] = &[
            Phase::MenuFlow,
            Phase::QuitToMenu,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
        ];
        // full: the whole native flow, then a reload Continue for the FPS comparison.
        const FULL: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            Phase::MenuFlow,
            Phase::QuitToMenu,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
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
static PHASE_START_TICK: AtomicU64 = AtomicU64::new(0);
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

/// Emit one per-phase telemetry line (the exact shape the run oracle consumes).
fn emit_phase_telemetry(
    base: usize,
    name: &str,
    idx: usize,
    outcome: &str,
    start_tick: u64,
    frame: u64,
    sem: &Sem,
) {
    let end_tick = unsafe { GetTickCount64() };
    let duration_ms = end_tick.saturating_sub(start_tick);
    let title_state = title_scan::title_state(base);
    let a40 = title_scan::title_dialog_a40(base);
    let line = format!(
        "{{\"phase\":\"{name}\",\"idx\":{idx},\"outcome\":\"{outcome}\",\"start_tick_ms\":{start_tick},\"end_tick_ms\":{end_tick},\"duration_ms\":{duration_ms},\"start_frame\":0,\"end_frame\":{frame},\"duration_frames\":{frame},\"title_state\":{title_state},\"a40\":{a40},\"menu\":\"0x{:x}\",\"world_sim\":{},\"now_loading\":{},\"load_fsm\":{}}}",
        sem.menu, sem.world_sim as u8, sem.now_loading as u8, sem.load_fsm
    );
    log_phase(&line);
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
    if frame == 0 {
        // Phase entry: stamp the wall-clock start tick and reset per-phase internal state.
        let tick = unsafe { GetTickCount64() };
        PHASE_START_TICK.store(tick, Ordering::SeqCst);
        MENU_OPENED_FRAME.store(u64::MAX, Ordering::SeqCst);
        harness_log!("phase[{idx}] {} ENTER at +{tick}ms", phase.name());
    }
    let start_tick = PHASE_START_TICK.load(Ordering::SeqCst);
    // world_simulating mutates a rising streak -> compute exactly once per frame.
    let sem = Sem::read(world_simulating());

    match phase.tick(base, im, frame, &sem) {
        Status::Running => {}
        Status::Advanced => {
            harness_log!(
                "phase[{idx}] {} ADVANCED after {frame}f (menu=0x{:x} world_sim={} now_loading={} load_fsm={} title_state={})",
                phase.name(),
                sem.menu,
                sem.world_sim as u8,
                sem.now_loading as u8,
                sem.load_fsm,
                title_scan::title_state(base)
            );
            emit_phase_telemetry(base, phase.name(), idx, "advanced", start_tick, frame, &sem);
            PHASE_IDX.store(idx + 1, Ordering::SeqCst);
            PHASE_FRAME.store(0, Ordering::SeqCst);
            if idx + 1 >= phases.len() {
                harness_log!("drive: DONE -- all phases complete");
            }
        }
        Status::Derailed => {
            harness_log!(
                "phase[{idx}] {} DERAILED: effect semaphore not seen within {}f budget (menu=0x{:x} world_sim={} now_loading={} load_fsm={} title_state={}) -- STOPPING drive; tear down and analyze",
                phase.name(),
                phase.budget(),
                sem.menu,
                sem.world_sim as u8,
                sem.now_loading as u8,
                sem.load_fsm,
                title_scan::title_state(base)
            );
            emit_phase_telemetry(base, phase.name(), idx, "derailed", start_tick, frame, &sem);
            DERAILED.store(true, Ordering::SeqCst);
        }
    }
}
