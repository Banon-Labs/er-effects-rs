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
//! LAYERS: the TITLE screen PRODUCES the inputmgr+0x90 keystate bitmap (bd TITLE-CONTINUE-is-accept-byte-
//! not-keystate), so the title (PRESS ANY BUTTON -> menu -> Continue) is driven by the global accept byte
//! `base+0x4589bdc` and gated on the title-owner semaphores (title_scan). The IN-WORLD pause menu
//! (System->Quit) is a CONSUMER of +0x90 and IS driven by keystate, gated on the popup top-job pane
//! semaphores (bd QUIT-TO-MENU-semaphores): HasTopMenuJob, menu_id (0xffff IngameTop / 0x25 OptionSetting),
//! OptionSetting tab index (Quit tab = 8), return-title request.
//!
//! Pattern chosen by the flag file `er-harness-drive-mode.txt` (`boot`|`reload`|`full`, default `full`).
//! Fires from a CSTaskImp FrameBegin task (title-active). Telemetry-only NATIVE boot+reload for the
//! vanilla FPS comparison.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use crate::game_mem::{
    OPTIONSETTING_QUIT_TAB_INDEX, flip_fixed_spf, flip_mode_current, menu_data_ptr, menu_flags,
    now_loading, optionsetting_tab_index, pause_menu_open, read_drive_mode_flag,
    return_title_requested, save_state, top_menu_id, top_menu_job_ptr, world_simulating,
};
use crate::input_inject::{
    MenuEvent, advance_press_any_button, input_manager, keep_input_active, native_open_equip_menu,
    native_open_inventory_menu, popup_job_serial, request_open_ingame_menu, tap_menu_event,
};
use crate::log::{harness_log, log_phase};
use crate::pad_inject::set_vk_id;
use crate::title_scan;
use crate::win32::GetTickCount64;

// Keystate tap cadence (in-world menu only): a clean single edge per cycle.
const TAP_SET_FRAMES: u64 = 3;
const TAP_GAP_FRAMES: u64 = 6;
const TAP_CYCLE_FRAMES: u64 = TAP_SET_FRAMES + TAP_GAP_FRAMES;
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
/// A single in-world keystate nav step (open / pane change / tab). ~8s.
const NAV_BUDGET: u64 = 480;
/// Native quit-to-menu confirm + world teardown. ~10s.
const QUIT_BUDGET: u64 = 600;
/// Dwell on the opened Equipment menu (mode `equip`) so its armament tiles populate, the menu
/// renders (fade-in settles), and the oracle can capture + process before teardown. 3s at 60fps
/// (user 2026-07-23: reduced 9s -> 3s teardown delay).
const EQUIP_DWELL_FRAMES: u64 = 180;

// ---- diagnostic probe (mode `probe`): sweep the DLUID virtual-key id space and log the menu response,
// to discover which id (1000..1080) is up/down/confirm/cancel/tab (bd MENU-INPUT-LAYER-virtual-key). ----
const PROBE_OPEN_FRAMES: u64 = 48;
const PROBE_VK_ID_MIN: u32 = 1000;
const PROBE_VK_ID_MAX: u32 = 1080;
const PROBE_ID_SEG_FRAMES: u64 = 24; // per id: edge-toggled presses + observe
const PROBE_LOG_EVERY: u64 = 6;
const PROBE_TOTAL_FRAMES: u64 =
    PROBE_OPEN_FRAMES + (PROBE_VK_ID_MAX - PROBE_VK_ID_MIN + 1) as u64 * PROBE_ID_SEG_FRAMES + 30;

/// Diagnostic: open the pause menu, then inject each virtual-key id 1000..1080 in turn (edge-toggled)
/// into `source+0x88` and LOG the menu response (job ptr, menuMan+0x1c flags word, tab) so the id ->
/// action map is read from evidence -- a confirm id changes the job/flags, a tab id sets a flags bit.
fn probe_menu_tick(im: usize, frame: u64) -> bool {
    // NATIVE-QUIT mode (er-harness-native-quit.txt): drive System->Quit by the DIRECT NATIVE return-to-title
    // request (acceptance §3a: native input can't reach the Scaleform menu; reproduce the action by a native
    // state write). Wait for a brief in-world settle, write menuData+0x5d=1 ONCE, then watch return_title
    // latch + world sim tear down. If it returns to title, the harness title->Continue phases reload.
    if crate::game_mem::native_quit_enabled() {
        let rt = return_title_requested();
        // Fire once at frame 120 (~2s in-world settle). Keep logging to see the quit-to-title transition.
        if frame == 120 {
            let wrote = crate::game_mem::request_return_to_title();
            harness_log!(
                "probe NATIVEQUIT f{frame}: wrote menuData+0x5d=1 ok={wrote} (direct native return-to-title)"
            );
        }
        if frame.is_multiple_of(15) || (118..=140).contains(&frame) {
            harness_log!(
                "probe NATIVEQUIT f{frame} return_title={} world_sim={} now_loading={} pause_menu={} menu_id=0x{:x}",
                rt as u8,
                world_simulating() as u8,
                now_loading(),
                pause_menu_open() as u8,
                top_menu_id(),
            );
        }
        return frame >= PROBE_TOTAL_FRAMES;
    }
    // OS-INPUT mode (er-harness-os-input.txt): send focus-gated OS keyboard taps to the pause menu -- the
    // game's REAL input path that reaches Scaleform (bd SYNTHESIS-pause-menu-is-scaleform; RAM injection
    // proven dead). Open the menu, then tap VK_DOWN (0x28) every ~30 frames; log the menu state so a
    // cursor/tab/menu_id change proves OS input drives the Scaleform menu.
    if crate::game_mem::os_input_enabled() {
        // DISAMBIGUATION (bd OS-keybd-event-ESCAPE...): does keybd_event route to ER under Wine/Proton AT
        // ALL? Test with an OBSERVABLE in-world effect FIRST -- HOLD W (0x57, forward) for frames 60..360
        // (~5s) while in-world, BEFORE opening any menu. The run's OBSERVE loop logs havok position; if the
        // player MOVES during the hold window, keybd_event routes (and the menu no-response is a wrong-key
        // problem). If the player does NOT move, OS keyboard is fundamentally dead in this env.
        // CONTINUOUS hold of W (0x57) from frame 60 onward, re-asserted every 30 frames, NEVER released --
        // so the STABLE-tail havok (well past the load transition) is measured WHILE W is held. If the
        // player position stays frozen during a late continuous W-hold, keybd_event definitively does not
        // route to ER under Wine (the load-transition confound is removed by measuring the frozen tail).
        const VK_W: u8 = 0x57;
        let mut sent = 0u8;
        if frame >= 60 && frame.is_multiple_of(30) {
            sent = crate::win32::send_key_down(VK_W) as u8;
        }
        if frame.is_multiple_of(30) {
            harness_log!(
                "probe OSMOVE f{frame} fg={} holdW={sent} pause_menu={} menu_id=0x{:x}",
                crate::win32::er_window_is_foreground() as u8,
                pause_menu_open() as u8,
                top_menu_id(),
            );
        }
        // Never returns the "done" until the cap; leave W held the whole in-world window.
        return frame >= PROBE_TOTAL_FRAMES;
    }
    // HOLD-ID mode: if er-harness-probe-hold-id.txt sets a vk-id, HOLD only that id (no sweep) to isolate
    // one index's menu action -- e.g. confirm index 34 (id 1034) drives return-to-title (bd NEXT-inworld-
    // menu-idmap-recovery-plan). Stop injecting the instant return_title latches so the quit completes.
    let hold = crate::game_mem::probe_hold_id();
    if hold != 0 {
        if return_title_requested() {
            set_vk_id(0);
            harness_log!(
                "probe HOLD id={hold} f{frame}: RETURN_TITLE LATCHED -> quit triggered, stop inject"
            );
            return true;
        }
        if !pause_menu_open() {
            request_open_ingame_menu(im);
            set_vk_id(0);
            return false;
        }
        // PER-FRAME direct stamp (the builder hook is too sparse in-menu, bd DECISIVE-builder-not-perframe):
        // write source+0x88[hold] every frame so the menu consistently sees the held key.
        set_vk_id(hold);
        if let Some(base) = crate::game_mem::game_base() {
            unsafe { crate::pad_inject::stamp_vk_direct(base, hold, 1) };
        }
        if frame.is_multiple_of(PROBE_LOG_EVERY) {
            let (bf, _wf, _gs, _ms, _o) = crate::pad_inject::pad_snapshot();
            harness_log!(
                "probe HOLD id={hold} f{frame} bf={bf} pause_menu={} menu_id=0x{:x} job=0x{:x} flags=0x{:x} tab={} rt={}",
                pause_menu_open() as u8,
                top_menu_id(),
                top_menu_job_ptr(),
                menu_flags(),
                optionsetting_tab_index(),
                return_title_requested() as u8
            );
        }
        return frame >= PROBE_TOTAL_FRAMES;
    }
    if frame < PROBE_OPEN_FRAMES {
        set_vk_id(0);
        if !pause_menu_open() {
            request_open_ingame_menu(im);
        }
        if frame.is_multiple_of(PROBE_LOG_EVERY) {
            let (bf, wf, gsrc, msrc, obs) = crate::pad_inject::pad_snapshot();
            harness_log!(
                "probe OPEN f{frame} pause_menu={} builder_fires={bf} writer_fires={wf} game_src=0x{gsrc:x} my_src=0x{msrc:x} obs=[{:x},{:x},{:x}] job=0x{:x} flags=0x{:x}",
                pause_menu_open() as u8,
                obs[0],
                obs[1],
                obs[2],
                top_menu_job_ptr(),
                menu_flags()
            );
        }
        return false;
    }
    let _ = im;
    let sweep = frame - PROBE_OPEN_FRAMES;
    let seg = sweep / PROBE_ID_SEG_FRAMES;
    let id = PROBE_VK_ID_MIN + seg as u32;
    if id <= PROBE_VK_ID_MAX {
        let local = sweep % PROBE_ID_SEG_FRAMES;
        // edge-toggle within the id segment: hold TAP_SET frames, release, a few clean edges.
        let held = (local % TAP_CYCLE_FRAMES) < TAP_SET_FRAMES;
        set_vk_id(if held { id } else { 0 });
        // PER-FRAME stamp (now cached: resolves the pad once, then a fault-safe write/frame -- no per-frame
        // RPM tree-walk that stopped the drive, bd BISECT-stamp_vk_direct-stops-drive).
        // EDGE test: write 1 on held frames, 0 on release -> clean 0->1 edges the menu can repeat on
        // (bd DECISIVE-source88... : held-1-only gave no edges). Every frame, cached pad = cheap.
        if let Some(base) = crate::game_mem::game_base() {
            unsafe { crate::pad_inject::stamp_vk_direct(base, id, if held { 1 } else { 0 }) };
        }
        if local.is_multiple_of(PROBE_LOG_EVERY) {
            let (bf, wf, gsrc, msrc, _obs) = crate::pad_inject::pad_snapshot();
            harness_log!(
                "probe id={id} f{frame} bf={bf} wf={wf} gsrc=0x{gsrc:x} msrc=0x{msrc:x} job=0x{:x} flags=0x{:x} tab={} return_title={}",
                top_menu_job_ptr(),
                menu_flags(),
                optionsetting_tab_index(),
                return_title_requested() as u8
            );
        }
    } else {
        set_vk_id(0);
    }
    frame >= PROBE_TOTAL_FRAMES
}

/// Per-frame semaphore snapshot (world_sim computed once by the caller -- it mutates a rising streak).
#[derive(Clone, Copy)]
struct Sem {
    menu: usize,
    world_sim: bool,
    now_loading: bool,
    save_state: i32,
}

impl Sem {
    fn read(world_sim: bool) -> Self {
        Sem {
            menu: menu_data_ptr(),
            world_sim,
            now_loading: now_loading(),
            save_state: save_state(),
        }
    }
    /// A load has actually STARTED (Continue took effect): the load FSM left idle, the now-loading latch
    /// tripped, or the world is already simulating.
    fn load_started(&self) -> bool {
        self.world_sim || self.now_loading || self.save_state > 0
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
    Startup,
    /// Write the accept byte each frame (advances PAB). EFFECT: the Continue/Load menu is built.
    PressAnyButton,
    /// Write the accept byte each frame (Continue is default-focused). EFFECT: a load started.
    Continue,
    /// NO input: wait for genuine in-world simulation (play_time rising). EFFECT: world_sim.
    WaitLoadIn,
    /// IN-WORLD: request the pause menu open. EFFECT: a popup top-job exists (pause_menu_open).
    OpenPauseMenu,
    /// IN-WORLD keystate MoveUp,Confirm. EFFECT: the top pane is OptionSetting (menu_id==0x25).
    NavToOptionSetting,
    /// IN-WORLD keystate TabLeft. EFFECT: the OptionSetting selected tab is the Quit tab (index==8).
    TabToQuit,
    /// IN-WORLD keystate MoveDown,Confirm (activate "Quit to main menu"; popup-accept confirms the
    /// dialog). EFFECT: return-title requested (menuData+0x5d==1) or the world already stopped.
    Quit,
    /// NO INPUT: read and log the CSPcKeyConfig pad binding behind each menu code.
    DumpMenuBindings,
    /// IN-WORLD keystate: MoveDown to the **Load Character from File** row, then Confirm. Injected
    /// events only -- the game decides what the press does. EFFECT: the game opens ProfileSelect,
    /// i.e. the top pane's menu id stops being OptionSetting.
    ActivateLoadFromFile,
    /// NO input: the native teardown to title. EFFECT: world stopped simulating AND load FSM idle.
    QuitTeardown,
    /// DIRECT NATIVE System->Quit (acceptance §3a, bd BREAKTHROUGH-native-return-to-title): native input
    /// can't reach the Scaleform menu, so write menuData+0x5d=1 (the game's own return-title request byte)
    /// each frame. EFFECT: return-title requested (return_title_requested()==1). Replaces the whole input-
    /// based OpenPauseMenu/NavToOptionSetting/TabToQuit/Quit nav.
    NativeQuit,
    /// DIAGNOSTIC (mode `probe`): in-world with the pause menu open, inject a LABELED input sweep (one
    /// eventId at a time, well spaced) and log the observables each frame, to empirically find which
    /// injected keystate actually moves the in-world menu. Never derails; advances at its budget.
    ProbeMenu,
    /// IN-WORLD with the pause menu open: Confirm activates the TOP list entry (Equipment; the
    /// pause-list cursor starts on it, unlike NavToOptionSetting's Up-wrap to System). EFFECT: the
    /// top-job pointer CHANGED (the Equipment submenu replaced the pause list).
    OpenEquipMenu,
    /// NO input: dwell on the opened Equipment menu so its armament tiles populate (and the
    /// er-armament-icons companion's tile hook fires and logs). Advances at its dwell budget.
    DwellEquip,
    /// NATIVE open of the Inventory menu (02_020_Inventory) whose item cells carry the bottom-left
    /// ArtsIcon child. EFFECT: top-job replaced OR the submit serial bumped.
    OpenInventoryMenu,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Startup => "startup",
            Phase::PressAnyButton => "press_any_button",
            Phase::Continue => "continue",
            Phase::WaitLoadIn => "wait_load_in",
            Phase::OpenPauseMenu => "open_pause_menu",
            Phase::NavToOptionSetting => "nav_to_optionsetting",
            Phase::TabToQuit => "tab_to_quit",
            Phase::DumpMenuBindings => "dump_menu_bindings",
            Phase::Quit => "quit",
            Phase::ActivateLoadFromFile => "activate_load_from_file",
            Phase::QuitTeardown => "quit_teardown",
            Phase::NativeQuit => "native_quit",
            Phase::ProbeMenu => "probe_menu",
            Phase::OpenEquipMenu => "open_equip_menu",
            Phase::DwellEquip => "dwell_equip",
            Phase::OpenInventoryMenu => "open_inventory_menu",
        }
    }

    fn budget(self) -> u64 {
        match self {
            Phase::Startup => STARTUP_BUDGET,
            Phase::PressAnyButton => PAB_BUDGET,
            Phase::Continue => CONTINUE_BUDGET,
            Phase::WaitLoadIn => LOAD_BUDGET,
            Phase::OpenPauseMenu
            | Phase::NavToOptionSetting
            | Phase::TabToQuit
            // Wider than the other nav phases on purpose: this one HOLDS (issues nothing) until the
            // pane reports the Load Character from File row, and the cloned rows are built by the
            // product when the Quit pane is first shown, not when the tab index flips.
            | Phase::DumpMenuBindings
            | Phase::ActivateLoadFromFile
            | Phase::OpenEquipMenu
            | Phase::OpenInventoryMenu => NAV_BUDGET,
            Phase::Quit | Phase::QuitTeardown | Phase::NativeQuit => QUIT_BUDGET,
            Phase::ProbeMenu => PROBE_TOTAL_FRAMES,
            Phase::DwellEquip => EQUIP_DWELL_FRAMES,
        }
    }

    /// One frame of the phase. Returns Advanced (effect seen), Running, or Derailed (past budget).
    fn tick(self, base: usize, im: usize, frame: u64, sem: &Sem) -> Status {
        let advanced = match self {
            Phase::Startup => title_scan::title_pab_parked(base),
            Phase::PressAnyButton => {
                advance_press_any_button(base);
                title_scan::title_menu_up(base)
            }
            Phase::Continue => {
                advance_press_any_button(base);
                sem.load_started()
            }
            Phase::WaitLoadIn => sem.world_sim,
            Phase::OpenPauseMenu => {
                // AWAIT FIRST, THEN CALL, THEN AWAIT AGAIN (2026-09-05). The old body called
                // `request_open_ingame_menu` on EVERY frame the menu was not up, which is two
                // different mistakes wearing one line. It cannot tell a menu the USER opened from
                // one it opened itself, so a run where a human pressed Escape looked identical to a
                // run where the harness did -- and the whole point of driving this flow is to prove
                // the path a player takes. And it re-requests while a request is already in flight,
                // so the open and the close can race inside the same phase.
                //
                // The order below is the fix and it is also the contract this phase advertises:
                //   frames 0..PAUSE_MENU_AWAIT_FRAMES   pure AWAIT -- no input at all. An escape
                //                                       menu that is already up, or that someone
                //                                       else opens in this window, is honoured and
                //                                       nothing is injected over it.
                //   after that                          CALL, once per tap cycle rather than every
                //                                       frame, then keep awaiting the same effect.
                // Either way the phase only advances on `pause_menu_open()`, which reads
                // `popupMenu->currentTopMenuJob` -- the game's own pointer, not our intent.
                let open = pause_menu_open();
                if !open {
                    if frame >= PAUSE_MENU_AWAIT_FRAMES && frame.is_multiple_of(TAP_CYCLE_FRAMES) {
                        let requested = request_open_ingame_menu(im);
                        if !PAUSE_MENU_REQUESTED.swap(true, Ordering::Relaxed) {
                            harness_log!(
                                "pause-menu: awaited {PAUSE_MENU_AWAIT_FRAMES}f with no escape menu -> requesting one ourselves (requested={requested})"
                            );
                        }
                    }
                    false
                } else {
                    // Say WHICH of the two happened, once. A phase that advances tells you the menu
                    // is up; only this line tells you whether we opened it, and a run that cannot
                    // answer that cannot claim it drove the menu.
                    if !PHASE_PAUSE_LOGGED.swap(true, Ordering::Relaxed) {
                        harness_log!(
                            "pause-menu: OPEN at f{frame} via {} (job=0x{:x})",
                            if PAUSE_MENU_REQUESTED.load(Ordering::Relaxed) {
                                "our request"
                            } else {
                                "await -- it was already up or someone else opened it"
                            },
                            top_menu_job_ptr()
                        );
                    }
                    // Record the root pane's job pointer while it IS the root, so the next phase can
                    // tell "entered a submenu" from "still on the root" by the pointer the game
                    // itself swaps.
                    SUBMENU_BASELINE_JOB.store(top_menu_job_ptr(), Ordering::Relaxed);
                    true
                }
            }
            Phase::NavToOptionSetting => {
                // TAP, DO NOT HOLD -- and press ONE candidate button per attempt (rewritten
                // 2026-09-05 after br-20260905-234626-ce9a).
                //
                // WHAT THE PREVIOUS BODY ACTUALLY DID, measured rather than reasoned about. It
                // called `stamp_menu_scroll_direct(base, 1)` on EVERY frame, and the delivery
                // channel is not the memory write that call also attempts -- it is
                // `menu_scroll_reader_hook`, which RETURNS the armed value to the game on every
                // read. So the menu was told "one row down" 1,553 consecutive times across 480
                // frames: a runaway scroll that can never come to rest on a row, not a press. On
                // top of that it cycled BOTH candidate confirm buttons (`+0x08` and `+0x10`) at
                // once, so a Confirm landed on whatever row the runaway had reached and there was
                // no way to attribute the result to either button. The phase derailed with
                // `pause_menu=0`: something in that spray CLOSED the escape menu.
                //
                // The two diagnostics that make this readable are worth keeping in mind:
                //   * `menu-gate: +0x19=1 +0x798=0x0 open=true` for all 480 frames -- the menu was
                //     never refusing input, so this was never a gate problem.
                //   * `raw_axis=0` at every sample -- the DIRECT WRITE half never reaches the
                //     device the reader uses (it resolves from PadDevices; the reader's comes from
                //     the padMaps tree). Only the hook's return value was ever the input.
                //
                // So: one discrete tap of UP, then one Confirm, then wait; repeat with the OTHER
                // button if the first attempt produced nothing. UP-then-Confirm is the reversed
                // route (2026-07-17, bd MENU-GAPS-CLOSED): from IngameTop the cursor starts at row
                // 0 and UP wraps to the last row, System/OptionSetting.
                let slot = frame % NAV_ATTEMPT_FRAMES;
                let attempt = frame / NAV_ATTEMPT_FRAMES;
                // Alternate which CSEzMenuViewerPad button this attempt treats as confirm, so the
                // run distinguishes them instead of pressing both and learning nothing.
                let confirm_button = if attempt.is_multiple_of(2) { 1 } else { 2 };
                if slot < NAV_TAP_FRAMES {
                    // UP, for one tap window only.
                    crate::pad_inject::set_menu_scroll(-1);
                    crate::pad_inject::set_menu_buttons(0);
                } else if slot < NAV_TAP_FRAMES + NAV_SETTLE_FRAMES {
                    // RELEASE and let the cursor come to rest. Without this the next read is still
                    // being handed a direction and the row under the cursor keeps moving.
                    crate::pad_inject::set_menu_scroll(0);
                    crate::pad_inject::set_menu_buttons(0);
                } else if slot < NAV_TAP_FRAMES + NAV_SETTLE_FRAMES + NAV_TAP_FRAMES {
                    crate::pad_inject::set_menu_scroll(0);
                    crate::pad_inject::set_menu_buttons(confirm_button);
                } else {
                    crate::pad_inject::set_menu_scroll(0);
                    crate::pad_inject::set_menu_buttons(0);
                }
                // ONE DIAGNOSTIC BLOCK PER ATTEMPT, not one per 120 frames. The old cadence was
                // unaligned with anything the phase did, so a sample could land mid-tap or mid-wait
                // and there was no way to say which press it described. At `slot == 0` every field
                // below describes the state the attempt STARTS from.
                if slot == 0 {
                    crate::pad_inject::sample_menu_pointer();
                    crate::pad_inject::sample_pointer_correction(base);
                    let (px, py) = crate::pad_inject::menu_pointer_observed();
                    let (cx, cy) = crate::pad_inject::menu_pointer_correction();
                    let (device, raw, calls) = crate::pad_inject::menu_scroll_reader_state();
                    harness_log!(
                        "nav-attempt {attempt} (f{frame}): confirm_button={confirm_button} pause_menu={} axis_reader_calls={calls} button_calls={} device=0x{device:x} raw_axis={raw} pointer x={px} y={py} correction x={} y={} (bits 0x{cx:x}/0x{cy:x})",
                        pause_menu_open() as u8,
                        crate::pad_inject::menu_button_reader_calls(),
                        f32::from_bits(cx),
                        f32::from_bits(cy)
                    );
                    // THE GATE FIRST, because it decides whether anything above is even readable as
                    // evidence. `FUN_140758050` shuts every menu pad read when CSMenuMan+0x19 == 0,
                    // or +0x798 != 0, or (disableMouseCursor && the fade plate timer 2 is still
                    // running). A derail with the gate SHUT is not a failed press, it is a menu that
                    // refused input -- and the fix for those two is nothing alike.
                    match crate::game_mem::menu_input_gate() {
                        Some((gate19, disable_cursor, gate798)) => harness_log!(
                            "menu-gate(nav a{attempt}): +0x19={gate19} disableMouseCursor={disable_cursor} +0x798=0x{gate798:x} open={}",
                            gate19 != 0 && gate798 == 0
                        ),
                        None => harness_log!("menu-gate(nav a{attempt}): CSMenuMan not up"),
                    }
                    match crate::game_mem::pause_menu_grid() {
                        Some((offset, selected)) => harness_log!(
                            "menu-grid(nav a{attempt}): GridControl at window+0x{offset:x} selected_cell={selected}"
                        ),
                        None => harness_log!(
                            "menu-grid(nav a{attempt}): no GridControl found in the top menu window"
                        ),
                    }
                }
                // THE MENU CLOSING IS A RESULT, NOT A TIMEOUT. If our input shut the escape menu
                // there is nothing left to navigate, and spending the rest of the 480-frame budget
                // pressing into a closed menu buys no evidence -- so say so immediately, with the
                // attempt and button that did it, and let the phase derail on the next tick.
                if !pause_menu_open() && !NAV_MENU_CLOSED_LOGGED.swap(true, Ordering::Relaxed) {
                    harness_log!(
                        "nav: THE ESCAPE MENU CLOSED at f{frame} during attempt {attempt} with confirm_button={confirm_button} -- our input dismissed it rather than entering a pane"
                    );
                }
                // EFFECT read as a POINTER CHANGE, not as a menu id. `top_menu_id()` reads
                // top_window+0x180, and on the installed 1.17 build that read returns -1 or garbage
                // (measured across br-20260905-041435-e8d0 and -041731-2bc4: 53724, 25445, -1, while
                // `pause_menu_open()` was correctly true) -- the offset is 1.16.2 and has drifted, so
                // waiting for it to equal 0x25 waits forever and the phase DERAILs on a nav that may
                // well have worked. `currentTopMenuJob` (popupMenu+0xB0) is the semaphore this module
                // already documents as the passive "entered a submenu" signal: the game REPLACES that
                // pointer when a submenu opens, pushing the old one to popupMenu+0xD0. A change in it
                // is the game telling us our Confirm entered a pane.
                let entered = top_menu_job_ptr();
                entered != 0 && entered != SUBMENU_BASELINE_JOB.load(Ordering::Relaxed)
            }
            Phase::TabToQuit => {
                // THE TAB-SWITCH (the D_van blocker): one TabLeft = native-binding menu-event 0x30. From
                // the default tab 0 the prev-tab edge WRAPS to the last tab = Quit (index 8). RE-CONFIRMED
                // on the loaded dump: getShownMenuFlags reads inputmgr+0x90+0x30 & 1 -> flag 0x1000
                // (tab-left) and +0x31 -> 0x80000 (tab-right); the OptionSetting GridControl pager consumes
                // it (bd MENU-GAPS-CLOSED-tabswitch-0x30L-0x31R / menu-eventid-set-enumerated). NOT
                // mouse-only -- the 2026-07-17 "mouse-only" verdict was an OS-layer (SendInput) artifact.
                // EFFECT (passive verify): the OptionSetting selected tab index == the Quit tab (8),
                // read at option_window+0x1870+0x10[deref]+0xd4.
                issue_menu_taps_once(im, &[MenuEvent::TabLeft], frame);
                optionsetting_tab_index() == OPTIONSETTING_QUIT_TAB_INDEX
            }
            Phase::DumpMenuBindings => {
                // ONE-SHOT EVIDENCE, no input. Menu navigation reads the FD4 pad device through
                // CS::CSEzMenuViewerPad, and a "menu code" is an index into the CSPcKeyConfig binding
                // table -- not a device id. This prints the pad binding the GAME has for each code we
                // care about, so the next drive can inject the input the game is actually looking for
                // instead of sweeping ids. It replaces nothing and blocks nothing: it advances the
                // moment the config is readable.
                crate::pad_inject::install_menu_scroll_hook(base);
                let (device, raw, calls) = crate::pad_inject::menu_scroll_reader_state();
                harness_log!(
                    "menu-axis: reader calls={calls} menu_device=0x{device:x} raw_axis={raw} (padDevices walk gave a float, so this reports the object the READER dereferenced)"
                );
                let mut any = false;
                for (name, code) in MENU_CODES_OF_INTEREST {
                    if let Some((primary, secondary)) =
                        crate::game_mem::menu_code_pad_binding(*code)
                    {
                        any = true;
                        harness_log!(
                            "menu-binding: {name} code=0x{code:x} pad_primary=0x{primary:x} pad_secondary=0x{secondary:x}"
                        );
                    }
                }
                // THE WHOLE TABLE, ONCE, INCLUDING THE KEYBOARD HALF. The named codes above are the
                // ones a previous pass GUESSED at, and two of them turned out to belong to
                // `CS::SpinCtrl` rather than to list navigation -- an error a dump would have caught
                // for free. Dword [0] is the mode-0 (keyboard) binding, so a row whose [0] reads a
                // recognisable DIK scancode names its own action: 0xd0 down-arrow, 0xc8 up-arrow,
                // 0x1f S, 0x11 W, 0x1c Return, 0x01 Escape. Rows that are entirely zero are unbound
                // and are skipped, which keeps the dump to the codes that exist.
                for code in 0..crate::game_mem::KEY_CONFIG_MAX_MENU_CODE {
                    let Some(row) = crate::game_mem::menu_code_binding_row(code) else {
                        continue;
                    };
                    if row.iter().all(|dword| *dword == 0) {
                        continue;
                    }
                    any = true;
                    harness_log!(
                        "menu-row: code=0x{code:02x} kb=0x{:x} m1=0x{:x} m2=0x{:x} pad1=0x{:x} pad2=0x{:x}",
                        row[0],
                        row[1],
                        row[2],
                        row[3],
                        row[4]
                    );
                }
                // Say whether the press channel is even reachable BEFORE the nav phase depends on it,
                // so "the product export was missing" cannot be mistaken later for "the menu ignored
                // the key". A release is the probe: holding 0 is the resting state anyway.
                harness_log!(
                    "menu-key-channel: er_quickload_hold_dinput_key reachable={}",
                    crate::key_inject::hold(0)
                );
                any
            }
            Phase::ActivateLoadFromFile => {
                // THE ROW ACTIVATION, DRIVEN AS INPUT. Every earlier route committed the second load by
                // WRITING the byte the row's handler writes (`request_return_to_title` ->
                // menuData+0x5d=1, or the product's `switch_slot_arm_programmatic`). That reproduces
                // nothing: the reported black screen appears on a menu-driven second load, and a direct
                // write skips the pane build, the cloned row's action object, ProfileSelect's list
                // build and the native return-chain -- every candidate cause. So this phase presses
                // keys and lets the game decide what they do.
                //
                // THE ROW INDEX IS READ, NOT ASSUMED. One row past ours on this tab is *Return to
                // Desktop*; a guessed row order quits the game instead of loading a character.
                // `optionsetting_load_from_file_row()` walks the pane the tab-select actually
                // displayed and classifies each label, so the count of MoveDown taps below is the
                // measured distance from row 0 (where the cursor sits when a pane opens) to our row.
                // The pane may not have finished building the cloned rows yet; hold rather than
                // press, so a tap can never land on whatever row happens to be under the cursor.
                // THE PANE POINTER IS THE BASELINE, taken on the first frame while OptionSetting is
                // still the top job. `top_menu_id()` used to be this phase's effect check
                // (`top_menu_id() != OPTIONSETTING_MENU_ID && pause_menu_open()`) and that was a
                // FALSE PASS, not merely a weak one: the same module already documents that offset
                // as 1.16.2 and drifted on 1.17, returning -1 or garbage (53724, 25445, -1 measured
                // across br-20260905-041435-e8d0 and -041731-2bc4) -- and garbage is `!= 0x25`, so
                // the phase advanced on its FIRST frame every time, before a single tap was issued,
                // and reported "activated the row" for a run in which nothing was pressed.
                // `Phase::NavToOptionSetting` was moved off that read for exactly this reason; this
                // arm was left behind. Use the same semaphore it uses: the game REPLACES
                // `currentTopMenuJob` when it opens a pane, so a changed pointer is the game saying
                // ProfileSelect came up.
                if ACTIVATE_BASELINE_JOB.load(Ordering::Relaxed) == 0 {
                    ACTIVATE_BASELINE_JOB.store(top_menu_job_ptr(), Ordering::Relaxed);
                }
                let row = crate::game_mem::optionsetting_load_from_file_row();
                if row < 0 {
                    false
                } else {
                    let mut events = [MenuEvent::MoveDown; MAX_QUIT_ROWS + 1];
                    let taps = (row as usize).min(MAX_QUIT_ROWS);
                    events[taps] = MenuEvent::Confirm;
                    issue_menu_taps_once(im, &events[..=taps], frame);
                    // EFFECT: the game left OptionSetting because it opened ProfileSelect for a file.
                    let now = top_menu_job_ptr();
                    now != 0 && now != ACTIVATE_BASELINE_JOB.load(Ordering::Relaxed)
                }
            }
            Phase::Quit => {
                // On the Quit tab (tab-switch proven): COMMIT the return-to-title by satisfying its native
                // side effect directly -- write menuData+0x5d=1, the exact request byte the quit-confirm
                // modal's "Yes" sets -- WITHOUT building/auto-accepting a CS::MessageBoxDialog (AGENTS
                // MessageBox rule: skip the modal, satisfy its semantic side effect without UI/input). The
                // native world-teardown + render-resource release this triggers is identical to the menu's
                // Quit-to-main-menu (menuData+0x5d is the shared quit-functor/idle-timeout request byte),
                // so the reload it feeds is the faithful native reload D_van needs. EFFECT: return-title
                // requested, or the world already began tearing down.
                crate::game_mem::request_return_to_title();
                return_title_requested() || !sem.world_sim
            }
            Phase::QuitTeardown => {
                !sem.world_sim && sem.save_state <= 0 && frame > TAP_CYCLE_FRAMES
            }
            Phase::NativeQuit => {
                // Direct native return-to-title: write menuData+0x5d=1 each frame (bd BREAKTHROUGH-native-
                // return-to-title). No menu input. Complete when the world ACTUALLY tears down (!world_sim),
                // NOT merely when return_title_requested() latches -- that flag can be STALE from a prior
                // reload cycle (reload2: the 2nd native_quit saw return_title=1 left from reload1 and
                // advanced in 0f without quitting). world_sim going false is the real, per-cycle effect.
                crate::game_mem::request_return_to_title();
                !sem.world_sim
            }
            Phase::ProbeMenu => probe_menu_tick(im, frame),
            Phase::OpenEquipMenu => {
                // NATIVE open (run-1 20260723-125948: pad-injected Confirm never reached the
                // Scaleform pause list; user authorized native menu callers). Build the EquipTop
                // job with the game's own pause-row factory and submit it through the native
                // CSPopupMenu top-job path. EFFECT: top-job replaced OR submit serial bumped.
                if frame == 0 {
                    INGAMETOP_JOB.store(top_menu_job_ptr(), Ordering::SeqCst);
                    EQUIP_SERIAL.store(popup_job_serial(im) as usize, Ordering::SeqCst);
                    let dispatched = native_open_equip_menu(base, im);
                    harness_log!("equip: native EquipTop open dispatched={dispatched}");
                }
                let job = top_menu_job_ptr();
                let serial = popup_job_serial(im) as usize;
                (job != 0 && job != INGAMETOP_JOB.load(Ordering::SeqCst))
                    || serial > EQUIP_SERIAL.load(Ordering::SeqCst)
            }
            Phase::DwellEquip => frame >= EQUIP_DWELL_FRAMES,
            Phase::OpenInventoryMenu => {
                // Native open of the Inventory menu (same factory+submit path as EquipTop; the
                // 02_020_Inventory item cells carry the bottom-left ArtsIcon child).
                if frame == 0 {
                    INGAMETOP_JOB.store(top_menu_job_ptr(), Ordering::SeqCst);
                    EQUIP_SERIAL.store(popup_job_serial(im) as usize, Ordering::SeqCst);
                    let dispatched = native_open_inventory_menu(base, im);
                    harness_log!("inv: native Inventory open dispatched={dispatched}");
                }
                let job = top_menu_job_ptr();
                let serial = popup_job_serial(im) as usize;
                (job != 0 && job != INGAMETOP_JOB.load(Ordering::SeqCst))
                    || serial > EQUIP_SERIAL.load(Ordering::SeqCst)
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

/// Issue each MENU EVENT in `events` once (one edge each, in order), via the CONFIRMED native-binding
/// keystate channel `inputmgr+0x90+eventId` (`tap_menu_event`). Same edge cadence the retired pad driver used
/// (OR the edge bit for `TAP_SET_FRAMES`, then gap -- the game's own input producer rewrites +0x90 to 0 on
/// the gap frames, giving one clean 0->1 edge with no auto-repeat). This is the in-world MENU nav lever
/// (open->OptionSetting->tab-switch->quit): getShownMenuFlags (0x1407665e0) reads +0x90[id]&1, so the menu
/// consumes exactly what this writes. Replaces the retired raw-pad `source+0x88` driver, which was
/// BISECT-disabled and never drove the menu (removed 2026-08-21; the negative sweep evidence is recorded
/// on `crate::pad_inject::set_vk_id`). The phase's ADVANCE is its own RAM semaphore, so an event that
/// lands is confirmed by a specific state change and one that does nothing derails on budget.
/// Upper bound on rows the Quit tab can hold -- the same 16 the product's own row walk caps at.
const MAX_QUIT_ROWS: usize = 16;

/// OR this frame's `getShownMenuFlags` word into the phase accumulator. Called from the tap helper so
/// it samples on exactly the frames the harness is injecting.
fn accumulate_shown_flags() {
    let flags = crate::game_mem::menu_flags();
    if flags != 0 {
        PHASE_SHOWN_FLAGS.fetch_or(flags, Ordering::Relaxed);
    }
}

fn issue_menu_taps_once(im: usize, events: &[MenuEvent], frame: u64) {
    accumulate_shown_flags();
    let idx = (frame / TAP_CYCLE_FRAMES) as usize;
    let held = (frame % TAP_CYCLE_FRAMES) < TAP_SET_FRAMES;
    if idx < events.len() {
        if held {
            tap_menu_event(im, events[idx]);
        } else {
            // RELEASE EXPLICITLY. This used to be a comment asserting "the native input producer
            // writes 0 -> clean edge release", which was never verified on 1.17. A bit left set is a
            // button reported HELD, and an edge-driven menu sees no press -- the shape of a Confirm
            // that getShownMenuFlags reports as consumed while nothing transitions.
            crate::input_inject::release_menu_event(im, events[idx]);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DriveMode {
    BootContinueOnly,
    NativeReloadOnly,
    /// Like NativeReloadOnly but drives TWO reload cycles, so epoch3 is a reload FROM a native reload
    /// (epoch2), not from the product autoload. Tests whether repeated reloads self-correct to parity
    /// after the autoload's epoch1 residual is flushed by the first reload (bd
    /// STEP4-RELOAD-REACHES-PARITY / autoload-residual).
    NativeReloadTwice,
    /// Product autoloads; the harness drives ONLY the menu for the second load. No title phases, so
    /// it cannot race the product's autoload the way `full` does.
    MenuReload,
    /// `MenuReload`'s cycle, three times over: loads 2, 3 and 4 each driven through the escape menu
    /// and the Load Character from File row. The mode to reach for when the question is "does the
    /// Nth load work", which is most of them -- the defects this project keeps finding appear on
    /// load 3, not load 2.
    MenuReloadChain,
    FullBootReload,
    Probe,
    /// COMPANION mode for the product run (samechar-3x): the harness does NOT drive boot/menu/continue
    /// (the PRODUCT owns that). It only keeps input active (stay-active/presence) so the product's
    /// harness-gated behavior is enabled without the standalone drive fighting it.
    Passive,
    /// Boot to in-world, open the pause menu, Confirm into the Equipment menu, then dwell so the
    /// armament tiles populate (er-armament-icons badge oracle run, bd er-effects-rs-pe98).
    EquipMenu,
    /// Boot to in-world, open the pause menu, native-open the Inventory menu (02_020_Inventory --
    /// the Melee/Ranged/Shields tabs with bottom-left ArtsIcon cells), then dwell.
    InventoryMenu,
}

impl DriveMode {
    fn from_flag() -> Self {
        let flag = read_drive_mode_flag();
        // Diagnostic for cross-platform launcher drift: the flag file is CWD-relative, and a
        // launcher that spawns the game with an unexpected CWD silently degrades every run to
        // the `full` fallback (native-me3 run 20260727-201106 resolved 'full' despite an
        // 'equip' marker in the game dir). Log the raw read + CWD so the miss is attributable.
        harness_log!(
            "drive: mode flag read -> {:?} (cwd={})",
            flag,
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("<err {e}>"))
        );
        match flag.as_str() {
            "boot" => DriveMode::BootContinueOnly,
            "reload" => DriveMode::NativeReloadOnly,
            "reload2" => DriveMode::NativeReloadTwice,
            "menureload" => DriveMode::MenuReload,
            "menuchain" => DriveMode::MenuReloadChain,
            "probe" => DriveMode::Probe,
            "passive" => DriveMode::Passive,
            "equip" => DriveMode::EquipMenu,
            "inv" => DriveMode::InventoryMenu,
            _ => DriveMode::FullBootReload,
        }
    }
    fn name(self) -> &'static str {
        match self {
            DriveMode::BootContinueOnly => "boot",
            DriveMode::NativeReloadOnly => "reload",
            DriveMode::NativeReloadTwice => "reload2",
            DriveMode::MenuReload => "menureload",
            DriveMode::MenuReloadChain => "menuchain",
            DriveMode::FullBootReload => "full",
            DriveMode::Probe => "probe",
            DriveMode::Passive => "passive",
            DriveMode::EquipMenu => "equip",
            DriveMode::InventoryMenu => "inv",
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
        // The native quit-to-title flow: DIRECT NATIVE return-to-title (menuData+0x5d=1, bd BREAKTHROUGH-
        // native-return-to-title) -- input can't reach the Scaleform menu, so no OpenPauseMenu/Nav/Tab/Quit
        // input nav; write the native request, then wait for the native teardown to title.
        const QUIT_FLOW: [Phase; 2] = [Phase::NativeQuit, Phase::QuitTeardown];
        // reload: WAIT for in-world first (so the PRODUCT's own autoload -- mod-side A/B, MOD_ARMED -- can
        // reach in-world before we act), then native quit-to-title -> reload Continue. The leading
        // WaitLoadIn is a no-input observe, so it is harmless when the harness itself drove the load.
        const RELOAD: &[Phase] = &[
            Phase::WaitLoadIn,
            QUIT_FLOW[0],
            QUIT_FLOW[1],
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
        ];
        // reload2: two full reload cycles -> epoch3 is a reload from the native epoch2 (not the autoload).
        const RELOAD2: &[Phase] = &[
            Phase::WaitLoadIn,
            QUIT_FLOW[0],
            QUIT_FLOW[1],
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            QUIT_FLOW[0],
            QUIT_FLOW[1],
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
        ];
        // MENU-DRIVEN quit-to-title (the D_van default, bd MENU-GAPS-CLOSED / d_van-blocker-...tabswitch):
        // open the pause menu (popup+0x121), nav to OptionSetting (MoveUp+Confirm), TAB-SWITCH to the Quit
        // tab (native-binding menu-event 0x30 -- the reversed blocker), then commit the native return-title.
        // This is the GENUINE menu-driven native reload the acceptance §4 confound-free D_van needs (vanilla
        // must be menu-driven, not the menu-free NativeQuit shortcut that mirrors the mod's own_load path).
        const MENU_QUIT_FLOW: [Phase; 5] = [
            Phase::OpenPauseMenu,
            Phase::NavToOptionSetting,
            Phase::TabToQuit,
            Phase::Quit,
            Phase::QuitTeardown,
        ];
        // full (menu-driven default): boot -> in-world -> menu-driven Quit-to-title -> reload Continue.
        const FULL_MENU: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            MENU_QUIT_FLOW[0],
            MENU_QUIT_FLOW[1],
            MENU_QUIT_FLOW[2],
            MENU_QUIT_FLOW[3],
            MENU_QUIT_FLOW[4],
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
        ];
        // full (NativeQuit fallback, opt-in via er-harness-native-quit.txt): the direct menuData+0x5d=1
        // write with NO menu nav -- the pre-tab-switch path. Kept as an escape hatch if the menu-driven
        // nav derails at runtime; produces the same native teardown but is menu-FREE (§4 confound).
        const FULL_NATIVE: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            QUIT_FLOW[0],
            QUIT_FLOW[1],
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
        ];
        // menureload: the product autoloads, the HARNESS drives the menu. Every other menu-capable mode
        // (`full`) opens with Startup/PressAnyButton/Continue and therefore drives the TITLE itself,
        // which races the product's own autoload for the same Continue -- observed 2026-09-05 on
        // br-20260905-040525-9e27, wedged at the title with 126 threads and 6 CPU ticks per 3s, and
        // reported by the user as "wedged on the last step of the first autoload". This table simply
        // has no title phases: it OBSERVES the product's load in (no input), then runs the real menu
        // nav -- pause menu, OptionSetting, TabToQuit(0x30) -- for the SECOND load, which is the only
        // load the reported black-screen bug is about. `presses=0` cannot happen here.
        const MENU_RELOAD: &[Phase] = &[
            Phase::WaitLoadIn,
            Phase::DumpMenuBindings,
            MENU_QUIT_FLOW[0],
            MENU_QUIT_FLOW[1],
            MENU_QUIT_FLOW[2],
            // NOT MENU_QUIT_FLOW[3] (Phase::Quit): that phase WRITES menuData+0x5d instead of pressing
            // the row, which is the exact shortcut that made every previous run unable to reproduce a
            // menu bug. This drives the row as input and lets the game open ProfileSelect itself.
            Phase::ActivateLoadFromFile,
            Phase::WaitLoadIn,
        ];
        // menuchain: the SAME cycle as `menureload`, run three times, so loads 2, 3 AND 4 all come
        // through the escape menu. It exists because a defect that only appears on the THIRD load
        // is invisible to a table that stops after the second, and because the control-file driver
        // that used to produce loads 3..N was deleted on 2026-09-05 (it armed them without the
        // menu). Repeating the phases is enough: `PHASE_IDX` walks the slice, so every cycle
        // re-enters `OpenPauseMenu` and re-awaits a genuinely closed menu, and the per-entry static
        // reset at the top of `on_frame` stops cycle N reading cycle N-1's answers.
        const MENU_CYCLE: [Phase; 5] = [
            MENU_QUIT_FLOW[0],
            MENU_QUIT_FLOW[1],
            MENU_QUIT_FLOW[2],
            Phase::ActivateLoadFromFile,
            Phase::WaitLoadIn,
        ];
        const MENU_CHAIN: &[Phase] = &[
            Phase::WaitLoadIn,
            Phase::DumpMenuBindings,
            MENU_CYCLE[0],
            MENU_CYCLE[1],
            MENU_CYCLE[2],
            MENU_CYCLE[3],
            MENU_CYCLE[4],
            MENU_CYCLE[0],
            MENU_CYCLE[1],
            MENU_CYCLE[2],
            MENU_CYCLE[3],
            MENU_CYCLE[4],
            MENU_CYCLE[0],
            MENU_CYCLE[1],
            MENU_CYCLE[2],
            MENU_CYCLE[3],
            MENU_CYCLE[4],
        ];
        // probe: reach in-world, then the diagnostic input sweep (mode `probe`).
        const PROBE: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            Phase::ProbeMenu,
        ];
        // equip: reach in-world, open the pause menu, Confirm into Equipment, dwell for the
        // armament-tile badge oracle (bd er-effects-rs-pe98).
        const EQUIP: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            Phase::OpenPauseMenu,
            Phase::OpenEquipMenu,
            Phase::DwellEquip,
        ];
        // inv: reach in-world, open the pause menu, native-open the Inventory menu, dwell.
        const INV: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            Phase::OpenPauseMenu,
            Phase::OpenInventoryMenu,
            Phase::DwellEquip,
        ];
        match self {
            DriveMode::BootContinueOnly => BOOT,
            DriveMode::NativeReloadOnly => RELOAD,
            DriveMode::NativeReloadTwice => RELOAD2,
            DriveMode::MenuReload => MENU_RELOAD,
            DriveMode::MenuReloadChain => MENU_CHAIN,
            // Menu-driven by default (drives the tab-switch); NativeQuit only when the fallback flag is set.
            DriveMode::FullBootReload => {
                if full_quit_native() {
                    FULL_NATIVE
                } else {
                    FULL_MENU
                }
            }
            DriveMode::Probe => PROBE,
            DriveMode::Passive => &[], // companion: no drive, presence only
            DriveMode::EquipMenu => EQUIP,
            DriveMode::InventoryMenu => INV,
        }
    }
}

/// `currentTopMenuJob` as it stood on the ROOT pause-menu pane. The nav phase advances when the game
/// replaces it, which is how this module already defines "entered a submenu".
/// The menu codes whose pad bindings decide whether a drive can navigate: the list cursor pair the
/// scroller FUN_14074f3c0 reads (0x2c down / 0x2d up) and the tab pair the OptionSetting pager reads
/// (0x30 left / 0x31 right). Confirm is deliberately absent: 0x3d is >= the table's 0x36 bound, so it
/// cannot be a binding index and its identity has to be re-derived rather than assumed.
const MENU_CODES_OF_INTEREST: &[(&str, u32)] = &[
    ("list_down", 0x2c),
    ("list_up", 0x2d),
    ("tab_left", 0x30),
    ("tab_right", 0x31),
];
static SUBMENU_BASELINE_JOB: AtomicUsize = AtomicUsize::new(0);
/// One `Phase::NavToOptionSetting` attempt: tap UP, let the cursor settle, tap Confirm, wait.
/// The waits are the point -- the phase it replaced held a direction for 480 straight frames and the
/// cursor could never come to rest on a row.
const NAV_TAP_FRAMES: u64 = 4;
const NAV_SETTLE_FRAMES: u64 = 20;
const NAV_WAIT_FRAMES: u64 = 36;
const NAV_ATTEMPT_FRAMES: u64 =
    NAV_TAP_FRAMES + NAV_SETTLE_FRAMES + NAV_TAP_FRAMES + NAV_WAIT_FRAMES;
/// One-shot for the "the escape menu closed under us" line, so it names the attempt that did it
/// instead of repeating every frame afterwards.
static NAV_MENU_CLOSED_LOGGED: AtomicBool = AtomicBool::new(false);
/// Frames `Phase::OpenPauseMenu` spends purely AWAITING an escape menu before it asks for one.
/// 90 frames is ~1.5s at 60fps: long enough that a menu already up, or one opened by someone else
/// in that window, is used as-is, and short enough that a run nobody is watching still proceeds.
const PAUSE_MENU_AWAIT_FRAMES: u64 = 90;
/// Did THIS visit to `Phase::OpenPauseMenu` ask the game to open the menu? Distinguishes the two
/// halves of await-or-call, which is the difference between driving the menu and watching one.
static PAUSE_MENU_REQUESTED: AtomicBool = AtomicBool::new(false);
/// One-shot for this visit's "OPEN via ..." line, so an open that persists for many frames logs once.
static PHASE_PAUSE_LOGGED: AtomicBool = AtomicBool::new(false);
/// `currentTopMenuJob` as it stood on the OptionSetting pane, recorded when
/// `Phase::ActivateLoadFromFile` starts. Its REPLACEMENT is that phase's effect.
static ACTIVATE_BASELINE_JOB: AtomicUsize = AtomicUsize::new(0);
/// Every bit `getShownMenuFlags` raised at any point during the current phase (CSMenuManImp+0x1c,
/// OR-accumulated). THE POINT: a phase that derails tells you the effect was not seen, but not
/// whether the INPUT was consumed -- and those are different defects with different fixes. This word
/// is the game's own answer to "which menu input fired", so a Confirm we injected shows up as 0x100
/// here whether or not the pane then changed. Accumulated rather than sampled because the flags are
/// per-frame and the telemetry line is written once, at the end.
static PHASE_SHOWN_FLAGS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static PHASE_IDX: AtomicUsize = AtomicUsize::new(0);
static PHASE_FRAME: AtomicU64 = AtomicU64::new(0);
static PHASE_START_TICK: AtomicU64 = AtomicU64::new(0);
static POPUP_FRAME: AtomicU64 = AtomicU64::new(0);
static MODE_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Set once the local player has existed this session. Gates the Passive-mode title advance so it
/// can never touch the BOOT title -- only a title reached after a world, i.e. the post-switch one.
static WORLD_HAS_EXISTED: AtomicBool = AtomicBool::new(false);
static DERAILED: AtomicBool = AtomicBool::new(false);
static ONFRAME_IM_NULL_DIAG: AtomicBool = AtomicBool::new(false);
/// currentTopMenuJob (+0xB0) recorded at IngameTop, to detect the submenu-entry replacement.
static INGAMETOP_JOB: AtomicUsize = AtomicUsize::new(0);
/// CSPopupMenu job-submit serial (popup+0x168) recorded before native menu opens.
static EQUIP_SERIAL: AtomicUsize = AtomicUsize::new(0);
/// FULL quit-flow selector cache: u8::MAX = unresolved, 0 = menu-driven (default), 1 = NativeQuit fallback.
static FULL_QUIT_MODE: AtomicU8 = AtomicU8::new(u8::MAX);

/// Whether the FULL drive mode should use the NativeQuit (menuData+0x5d=1 direct) fallback instead of the
/// menu-driven Quit flow (open->OptionSetting->TabToQuit(0x30)->commit). Opt-in via er-harness-native-quit.txt.
/// Resolved and logged ONCE, then cached, so `phases()` does not read a file every frame.
fn full_quit_native() -> bool {
    match FULL_QUIT_MODE.load(Ordering::SeqCst) {
        0 => false,
        1 => true,
        _ => {
            let native = crate::game_mem::native_quit_enabled();
            FULL_QUIT_MODE.store(native as u8, Ordering::SeqCst);
            harness_log!(
                "drive: FULL quit flow = {}",
                if native {
                    "NativeQuit fallback (menuData+0x5d=1 direct, menu-FREE)"
                } else {
                    "menu-driven (open -> OptionSetting -> TabToQuit(0x30) -> native commit)"
                }
            );
            native
        }
    }
}

fn resolve_mode() -> DriveMode {
    // MUST stay index-aligned with the `idx` match below (bd reload2-crash-MODES-oob): every DriveMode
    // needs a slot here or MODES[cached] panics. NativeReloadTwice=5 was added to the match but not here,
    // so the 2nd per-frame resolve_mode() indexed MODES[5] out-of-bounds -> crash ~after boot (run64/65/67).
    const MODES: [DriveMode; 10] = [
        DriveMode::BootContinueOnly,  // 0
        DriveMode::NativeReloadOnly,  // 1
        DriveMode::FullBootReload,    // 2
        DriveMode::Probe,             // 3
        DriveMode::Passive,           // 4
        DriveMode::NativeReloadTwice, // 5
        DriveMode::EquipMenu,         // 6
        DriveMode::InventoryMenu,     // 7
        DriveMode::MenuReload,        // 8
        DriveMode::MenuReloadChain,   // 9
    ];
    let cached = MODE_IDX.load(Ordering::SeqCst);
    if cached != usize::MAX {
        return MODES[cached];
    }
    // Product loaded -> COMPANION: stand down (real runtime condition, not a marker file). Only when
    // running standalone does the mode flag select a standalone drive pattern. EXCEPTION: the force-drive
    // override (er-harness-force-drive.txt / ER_HARNESS_FORCE_DRIVE) makes the harness drive even with the
    // product loaded -- the VANILLA agent-driven baseline needs the product's telemetry AND harness drive
    // (bd VANILLA-BASELINE-blocked-harness-forces-passive-when-product-loaded).
    let mode =
        if crate::game_mem::product_dll_present() && !crate::game_mem::force_drive_requested() {
            if crate::game_mem::companion_autoload_requested() {
                // Drive the boot menu-Continue as the AUTOLOAD (menu path = run49 PARITY) instead of
                // standing down for the product's menu-free own_load_continue, which leaves the ~4-6fps
                // epoch1 render residual preserved through reloads (bd STEP4-FIX-DIRECTION-PROVEN). The
                // product's own autoload must be disarmed (er-quickload-diag-no-autoload) so they don't
                // compete for the boot load; after the boot Continue the harness is done and the product's
                // switch machinery owns subsequent loads.
                DriveMode::BootContinueOnly
            } else {
                DriveMode::Passive
            }
        } else {
            DriveMode::from_flag()
        };
    let idx = match mode {
        DriveMode::BootContinueOnly => 0,
        DriveMode::NativeReloadOnly => 1,
        DriveMode::FullBootReload => 2,
        DriveMode::Probe => 3,
        DriveMode::Passive => 4,
        DriveMode::NativeReloadTwice => 5,
        DriveMode::EquipMenu => 6,
        DriveMode::InventoryMenu => 7,
        DriveMode::MenuReload => 8,
        DriveMode::MenuReloadChain => 9,
    };
    MODE_IDX.store(idx, Ordering::SeqCst);
    harness_log!(
        "drive: mode='{}' phases={}",
        mode.name(),
        mode.phases().len()
    );
    mode
}

/// Emit one per-phase telemetry line (the exact shape the run oracle consumes). Includes the in-world
/// pane semaphores so a phase's boundary is fully reconstructable offline.
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
    let menu_id = top_menu_id();
    let tab = optionsetting_tab_index();
    let shown_flags = PHASE_SHOWN_FLAGS.swap(0, Ordering::Relaxed);
    // The DECISIVE fps signal (bd MECHANISM-20fps-cap-fixedspf-0.05): 0.05 = the loading 20fps cap,
    // 0.0167 = 60fps. The differential loop diffs THIS per phase, not raw fps.
    let fixed_spf = flip_fixed_spf();
    let flip_mode = flip_mode_current();
    let line = format!(
        "{{\"phase\":\"{name}\",\"idx\":{idx},\"outcome\":\"{outcome}\",\"start_tick_ms\":{start_tick},\"end_tick_ms\":{end_tick},\"duration_ms\":{duration_ms},\"start_frame\":0,\"end_frame\":{frame},\"duration_frames\":{frame},\"title_state\":{title_state},\"a40\":{a40},\"pause_menu_open\":{},\"menu_id\":{menu_id},\"tab_index\":{tab},\"return_title\":{},\"fixed_spf\":{fixed_spf:.4},\"flip_mode\":{flip_mode},\"menu\":\"0x{:x}\",\"world_sim\":{},\"now_loading\":{},\"save_state\":{},\"shown_menu_flags\":\"0x{shown_flags:x}\"}}",
        pause_menu_open() as u8,
        return_title_requested() as u8,
        sem.menu,
        sem.world_sim as u8,
        sem.now_loading as u8,
        sem.save_state
    );
    log_phase(&line);
}

/// Run one frame of the drive. Called on the game thread from the CSTaskImp FrameBegin task.
pub fn on_frame(base: usize) {
    keep_input_active(base);

    // THE LIVE COMMAND LOOP, ABOVE EVERY EARLY RETURN BELOW -- deliberately. Its whole value is
    // asking questions of a session that has ALREADY stopped driving: a derailed phase leaves the
    // game up and in exactly the state worth interrogating, and a `return` before this point would
    // make that the one moment the harness cannot answer.
    crate::repl::on_frame(base);

    // COMPANION (product run): presence + stay-active only; the PRODUCT owns the drive. No phases, no
    // popup-accept, no pad injection -- so the standalone drive never fights the product's own flow.
    if resolve_mode() == DriveMode::Passive {
        // ...WITH ONE EXCEPTION: a title parked at PRESS ANY BUTTON (2026-09-04).
        //
        // This is the single input a user supplies that nothing else in an agent-driven run does.
        // After a System->Quit switch the world tears down to a clean title and the product's
        // autoload waits there for the title to advance; a human presses a button and the load
        // proceeds. With no human, the title sits parked forever, the picked-slot load never
        // starts, and every agent-driven validation of the switch feature stalls at exactly that
        // point (bd er-effects-rs-tkfb -- measured stalls of 154s and 185s with the product
        // retrying its accept byte the whole time).
        //
        // It does not fight the product: `advance_press_any_button` writes the SAME title global
        // accept byte the product itself writes, so this is idempotent with the product's own
        // attempt rather than a competing input. It is gated on the title actually being parked, so
        // it is inert everywhere else, and it lives in the DIAGNOSTIC harness DLL, which is never
        // part of a product profile -- a user's run is unaffected by its existence.
        //
        // ORDER MATTERS AND IS WHY THIS WAS NOT ENOUGH ON ITS OWN EARLIER: while the product was
        // force-hiding PressStart unconditionally, no accept could land no matter who wrote it.
        // That gate is fixed separately; this supplies the press once the component is visible.
        // ONLY A TITLE THAT COMES AFTER A WORLD -- NEVER THE BOOT TITLE (corrected 2026-09-04).
        //
        // The first version of this pressed any parked title, and it broke the BOOT load: run
        // br-20260905-000614-2576 never got past milestone_idx=5 with player_present=False, where
        // the immediately preceding build (same product DLL, no press) reached the world normally
        // (br-20260905-000200-2791: player_present=True, milestone_idx=11). Pressing the boot title
        // races the product's own autoload, which owns that transition.
        //
        // The post-switch title is the only one that needs a press, and it is distinguishable
        // without reading product state: a world has already existed this session. Latch that the
        // first time the player is present, and only press once the latch is set.
        if crate::game_mem::player_present() {
            WORLD_HAS_EXISTED.store(true, Ordering::SeqCst);
        } else if WORLD_HAS_EXISTED.load(Ordering::SeqCst) && title_scan::title_pab_parked(base) {
            advance_press_any_button(base);
        }
        return;
    }

    if DERAILED.load(Ordering::SeqCst) {
        return; // stopped driving; the run monitor tears the game down on the DERAILED marker
    }

    let Some(im) = input_manager(base) else {
        // DIAG (bd BREAKTHROUGH2 task-stop): log ONCE if input_manager stops resolving mid-drive (the
        // suspected cause of the drive silently stopping after the first pad injection changed the menu).
        if !ONFRAME_IM_NULL_DIAG.swap(true, Ordering::SeqCst) {
            harness_log!(
                "on_frame: input_manager returned None -> drive silently stops this frame"
            );
        }
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
        let tick = unsafe { GetTickCount64() };
        PHASE_START_TICK.store(tick, Ordering::SeqCst);
        // PER-ENTRY, NOT PER-RUN. These describe ONE visit to a phase, and the chain modes visit
        // the same phase once per load -- so a static left set by cycle 1 would make cycle 2 report
        // cycle 1's answer ("we opened it" when this time we awaited it), which is worse than no
        // answer at all.
        PAUSE_MENU_REQUESTED.store(false, Ordering::Relaxed);
        NAV_MENU_CLOSED_LOGGED.store(false, Ordering::Relaxed);
        PHASE_PAUSE_LOGGED.store(false, Ordering::Relaxed);
        ACTIVATE_BASELINE_JOB.store(0, Ordering::Relaxed);
        harness_log!("phase[{idx}] {} ENTER at +{tick}ms", phase.name());
    }
    let start_tick = PHASE_START_TICK.load(Ordering::SeqCst);
    // world_simulating mutates a rising streak -> compute exactly once per frame.
    let sem = Sem::read(world_simulating());

    match phase.tick(base, im, frame, &sem) {
        Status::Running => {}
        Status::Advanced => {
            harness_log!(
                "phase[{idx}] {} ADVANCED after {frame}f (pause_menu={} menu_id={} tab={} return_title={} world_sim={} save_state={} title_state={})",
                phase.name(),
                pause_menu_open() as u8,
                top_menu_id(),
                optionsetting_tab_index(),
                return_title_requested() as u8,
                sem.world_sim as u8,
                sem.save_state,
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
                "phase[{idx}] {} DERAILED: effect not seen within {}f (pause_menu={} menu_id={} tab={} return_title={} world_sim={} save_state={} title_state={}) -- STOPPING drive; tear down and analyze",
                phase.name(),
                phase.budget(),
                pause_menu_open() as u8,
                top_menu_id(),
                optionsetting_tab_index(),
                return_title_requested() as u8,
                sem.world_sim as u8,
                sem.save_state,
                title_scan::title_state(base)
            );
            emit_phase_telemetry(base, phase.name(), idx, "derailed", start_tick, frame, &sem);
            DERAILED.store(true, Ordering::SeqCst);
        }
    }
}
