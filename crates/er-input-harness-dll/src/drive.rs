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

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::game_mem::{
    current_open_menu_id, flip_fixed_spf, flip_mode_current, load_fsm, menu_data_ptr, menu_flags,
    now_loading, optionsetting_tab_index, pause_menu_open, read_drive_mode_flag,
    return_title_requested, top_menu_id, top_menu_job_ptr, top_window_dialog_accept_ready,
    world_simulating,
};
use crate::input_inject::{
    MenuEvent, advance_press_any_button, input_manager, keep_input_active, native_open_equip_menu,
    native_open_inventory_menu, native_open_weapon_upgrade_menu, popup_job_serial,
    request_open_ingame_menu, tap_menu_event,
};
use crate::log::{harness_log, log_phase};
use crate::pad_inject::{PadButton, set_pad_button, set_vk_id};
use crate::title_scan;
use crate::win32::GetTickCount64;

// Keystate tap cadence (in-world menu only): a clean single edge per cycle.
const TAP_SET_FRAMES: u64 = 3;
const TAP_GAP_FRAMES: u64 = 6;
const TAP_CYCLE_FRAMES: u64 = TAP_SET_FRAMES + TAP_GAP_FRAMES;
/// Popup-accept cadence (dialog-OK id 0x01, harmless when no dialog is up).
const POPUP_SET_FRAMES: u64 = 2;
const POPUP_CYCLE_FRAMES: u64 = 8;
/// Settle after the tab-switch tap (no passive tab-index read; verified downstream by the quit phase).
const TAB_SETTLE_FRAMES: u64 = 30;

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
        if frame % 15 == 0 || (118..=140).contains(&frame) {
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
        if frame >= 60 && frame % 30 == 0 {
            sent = crate::win32::send_key_down(VK_W) as u8;
        }
        if frame % 30 == 0 {
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
        if frame % PROBE_LOG_EVERY == 0 {
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
        if frame % PROBE_LOG_EVERY == 0 {
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
        if local % PROBE_LOG_EVERY == 0 {
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
    open_menu: i64,
    dialog_accept_ready: bool,
    world_sim: bool,
    now_loading: bool,
    load_fsm: i32,
}

impl Sem {
    fn read(world_sim: bool) -> Self {
        Sem {
            menu: menu_data_ptr(),
            open_menu: current_open_menu_id().map_or(-1, i64::from),
            dialog_accept_ready: top_window_dialog_accept_ready(),
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
    /// NATIVE open of the weapon-upgrade/reinforcement menu. EFFECT: researched open-menu id 0x17 or
    /// the native submit path visibly swaps/serializes the top job.
    OpenWeaponUpgradeMenu,
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
            Phase::Quit => "quit",
            Phase::QuitTeardown => "quit_teardown",
            Phase::NativeQuit => "native_quit",
            Phase::ProbeMenu => "probe_menu",
            Phase::OpenEquipMenu => "open_equip_menu",
            Phase::DwellEquip => "dwell_equip",
            Phase::OpenInventoryMenu => "open_inventory_menu",
            Phase::OpenWeaponUpgradeMenu => "open_weapon_upgrade_menu",
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
            | Phase::OpenEquipMenu
            | Phase::OpenInventoryMenu
            | Phase::OpenWeaponUpgradeMenu => NAV_BUDGET,
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
                if !pause_menu_open() {
                    request_open_ingame_menu(im);
                }
                pause_menu_open()
            }
            Phase::NavToOptionSetting => {
                // In-world menu nav is driven by the RAW PAD DEVICE (bd ROOTCAUSE-plus90-is-OUTPUT), not
                // +0x90. currentTopMenuJob (+0xB0) is REPLACED when a submenu opens (bd PANE-ID-FIX):
                // record the IngameTop job on entry; entering System/OptionSetting swaps it. EFFECT: the
                // top-job pointer CHANGED (a submenu is up).
                if frame == 0 {
                    INGAMETOP_JOB.store(top_menu_job_ptr(), Ordering::SeqCst);
                }
                issue_pad_taps_once(&[PadButton::Up, PadButton::Confirm], frame);
                let job = top_menu_job_ptr();
                job != 0 && job != INGAMETOP_JOB.load(Ordering::SeqCst)
            }
            Phase::TabToQuit => {
                // No passive tab-index read (option_window is buried in the +0xB0 sequence). Issue one
                // TabLeft and settle; the QUIT phase's return_title_requested verifies the whole nav
                // landed (if TabLeft missed the Quit tab, quit derails downstream -- honest).
                issue_pad_taps_once(&[PadButton::TabLeft], frame);
                frame >= TAP_CYCLE_FRAMES + TAB_SETTLE_FRAMES
            }
            Phase::Quit => {
                issue_pad_taps_once(&[PadButton::Down, PadButton::Confirm], frame);
                // Down+Confirm activates the Quit-to-main-menu row -> confirm dialog; the every-frame
                // popup-accept confirms the dialog. Quit started once the request byte is set (or the
                // world already began tearing down).
                return_title_requested() || !sem.world_sim
            }
            Phase::QuitTeardown => !sem.world_sim && sem.load_fsm <= 0 && frame > TAP_CYCLE_FRAMES,
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
            Phase::OpenWeaponUpgradeMenu => {
                // Native open of the weapon-upgrade/reinforcement menu. Advance only when the
                // current-open-menu semaphore reaches the researched weapon-upgrade id or the native
                // submit path visibly swaps/serializes the top job.
                if frame == 0 {
                    INGAMETOP_JOB.store(top_menu_job_ptr(), Ordering::SeqCst);
                    EQUIP_SERIAL.store(popup_job_serial(im) as usize, Ordering::SeqCst);
                    let dispatched = native_open_weapon_upgrade_menu(base, im);
                    harness_log!("upgrade: native WeaponUpgrade open dispatched={dispatched}");
                }
                let job = top_menu_job_ptr();
                let serial = popup_job_serial(im) as usize;
                sem.open_menu == i64::from(crate::input_scheduler::WEAPON_UPGRADE_OPEN_MENU_ID)
                    || (job != 0 && job != INGAMETOP_JOB.load(Ordering::SeqCst))
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

/// Issue each PAD button in `buttons` once (one tap cycle each, in order), EDGE-TOGGLED (held for
/// TAP_SET frames then released, so the edge-triggered menu registers one clean press per button), then
/// release. The phase's ADVANCE is its own effect semaphore, not the taps -- a press that lands is
/// confirmed by a specific RAM change, and one that does nothing derails on budget. Drives the RAW PAD
/// (bd ROOTCAUSE-plus90-is-OUTPUT), not the +0x90 keystate.
fn issue_pad_taps_once(buttons: &[PadButton], frame: u64) {
    let idx = (frame / TAP_CYCLE_FRAMES) as usize;
    let held = (frame % TAP_CYCLE_FRAMES) < TAP_SET_FRAMES;
    if idx < buttons.len() && held {
        set_pad_button(buttons[idx]);
    } else {
        set_pad_button(PadButton::None);
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
    /// Boot to in-world, open the pause menu, native-open the weapon-upgrade/reinforcement menu,
    /// then dwell for semaphore logging. No confirm inputs.
    WeaponUpgradeMenu,
}

impl DriveMode {
    fn from_flag() -> Self {
        match read_drive_mode_flag().as_str() {
            "boot" => DriveMode::BootContinueOnly,
            "reload" => DriveMode::NativeReloadOnly,
            "reload2" => DriveMode::NativeReloadTwice,
            "probe" => DriveMode::Probe,
            "passive" => DriveMode::Passive,
            "equip" => DriveMode::EquipMenu,
            "inv" => DriveMode::InventoryMenu,
            "upgrade" => DriveMode::WeaponUpgradeMenu,
            _ => DriveMode::FullBootReload,
        }
    }
    fn name(self) -> &'static str {
        match self {
            DriveMode::BootContinueOnly => "boot",
            DriveMode::NativeReloadOnly => "reload",
            DriveMode::NativeReloadTwice => "reload2",
            DriveMode::FullBootReload => "full",
            DriveMode::Probe => "probe",
            DriveMode::Passive => "passive",
            DriveMode::EquipMenu => "equip",
            DriveMode::InventoryMenu => "inv",
            DriveMode::WeaponUpgradeMenu => "upgrade",
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
        // full: the whole native flow, then a reload Continue for the FPS comparison.
        const FULL: &[Phase] = &[
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
        // upgrade: reach in-world, native-open the weapon-upgrade menu, dwell/log semaphores only.
        const UPGRADE: &[Phase] = &[
            Phase::Startup,
            Phase::PressAnyButton,
            Phase::Continue,
            Phase::WaitLoadIn,
            Phase::OpenPauseMenu,
            Phase::OpenWeaponUpgradeMenu,
            Phase::DwellEquip,
        ];
        match self {
            DriveMode::BootContinueOnly => BOOT,
            DriveMode::NativeReloadOnly => RELOAD,
            DriveMode::NativeReloadTwice => RELOAD2,
            DriveMode::FullBootReload => FULL,
            DriveMode::Probe => PROBE,
            DriveMode::Passive => &[], // companion: no drive, presence only
            DriveMode::EquipMenu => EQUIP,
            DriveMode::InventoryMenu => INV,
            DriveMode::WeaponUpgradeMenu => UPGRADE,
        }
    }
}

static PHASE_IDX: AtomicUsize = AtomicUsize::new(0);
static PHASE_FRAME: AtomicU64 = AtomicU64::new(0);
static PHASE_START_TICK: AtomicU64 = AtomicU64::new(0);
static POPUP_FRAME: AtomicU64 = AtomicU64::new(0);
static MODE_IDX: AtomicUsize = AtomicUsize::new(usize::MAX);
static DERAILED: AtomicBool = AtomicBool::new(false);
static ONFRAME_IM_NULL_DIAG: AtomicBool = AtomicBool::new(false);
/// currentTopMenuJob (+0xB0) recorded at IngameTop, to detect the submenu-entry replacement.
static INGAMETOP_JOB: AtomicUsize = AtomicUsize::new(0);
/// CSPopupMenu job-submit serial (popup+0x168) recorded before the native EquipTop open.
static EQUIP_SERIAL: AtomicUsize = AtomicUsize::new(0);

fn resolve_mode() -> DriveMode {
    // MUST stay index-aligned with the `idx` match below (bd reload2-crash-MODES-oob): every DriveMode
    // needs a slot here or MODES[cached] panics. NativeReloadTwice=5 was added to the match but not here,
    // so the 2nd per-frame resolve_mode() indexed MODES[5] out-of-bounds -> crash ~after boot (run64/65/67).
    const MODES: [DriveMode; 8] = [
        DriveMode::BootContinueOnly,  // 0
        DriveMode::NativeReloadOnly,  // 1
        DriveMode::FullBootReload,    // 2
        DriveMode::Probe,             // 3
        DriveMode::Passive,           // 4
        DriveMode::NativeReloadTwice, // 5
        DriveMode::EquipMenu,         // 6
        DriveMode::InventoryMenu,     // 7
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
                // product's own autoload must be disarmed (er-effects-diag-no-autoload) so they don't
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
        DriveMode::WeaponUpgradeMenu => 8,
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
    // The DECISIVE fps signal (bd MECHANISM-20fps-cap-fixedspf-0.05): 0.05 = the loading 20fps cap,
    // 0.0167 = 60fps. The differential loop diffs THIS per phase, not raw fps.
    let fixed_spf = flip_fixed_spf();
    let flip_mode = flip_mode_current();
    let line = format!(
        "{{\"phase\":\"{name}\",\"idx\":{idx},\"outcome\":\"{outcome}\",\"start_tick_ms\":{start_tick},\"end_tick_ms\":{end_tick},\"duration_ms\":{duration_ms},\"start_frame\":0,\"end_frame\":{frame},\"duration_frames\":{frame},\"title_state\":{title_state},\"a40\":{a40},\"pause_menu_open\":{},\"menu_id\":{menu_id},\"open_menu\":{},\"tab_index\":{tab},\"return_title\":{},\"dialog_accept_ready\":{},\"fixed_spf\":{fixed_spf:.4},\"flip_mode\":{flip_mode},\"menu\":\"0x{:x}\",\"world_sim\":{},\"now_loading\":{},\"load_fsm\":{}}}",
        pause_menu_open() as u8,
        sem.open_menu,
        return_title_requested() as u8,
        sem.dialog_accept_ready as u8,
        sem.menu,
        sem.world_sim as u8,
        sem.now_loading as u8,
        sem.load_fsm
    );
    log_phase(&line);
}

/// Run one frame of the drive. Called on the game thread from the CSTaskImp FrameBegin task.
pub fn on_frame(base: usize) {
    keep_input_active(base);

    // COMPANION (product run): presence + stay-active only; the PRODUCT owns the drive. No phases, no
    // popup-accept, no pad injection -- so the standalone drive never fights the product's own flow.
    if resolve_mode() == DriveMode::Passive {
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
        harness_log!("phase[{idx}] {} ENTER at +{tick}ms", phase.name());
    }
    let start_tick = PHASE_START_TICK.load(Ordering::SeqCst);
    // world_simulating mutates a rising streak -> compute exactly once per frame.
    let sem = Sem::read(world_simulating());

    match phase.tick(base, im, frame, &sem) {
        Status::Running => {}
        Status::Advanced => {
            harness_log!(
                "phase[{idx}] {} ADVANCED after {frame}f (pause_menu={} menu_id={} open_menu={} tab={} return_title={} dialog_accept_ready={} world_sim={} load_fsm={} title_state={})",
                phase.name(),
                pause_menu_open() as u8,
                top_menu_id(),
                sem.open_menu,
                optionsetting_tab_index(),
                return_title_requested() as u8,
                sem.dialog_accept_ready as u8,
                sem.world_sim as u8,
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
                "phase[{idx}] {} DERAILED: effect not seen within {}f (pause_menu={} menu_id={} open_menu={} tab={} return_title={} dialog_accept_ready={} world_sim={} load_fsm={} title_state={}) -- STOPPING drive; tear down and analyze",
                phase.name(),
                phase.budget(),
                pause_menu_open() as u8,
                top_menu_id(),
                sem.open_menu,
                optionsetting_tab_index(),
                return_title_requested() as u8,
                sem.dialog_accept_ready as u8,
                sem.world_sim as u8,
                sem.load_fsm,
                title_scan::title_state(base)
            );
            emit_phase_telemetry(base, phase.name(), idx, "derailed", start_tick, frame, &sem);
            DERAILED.store(true, Ordering::SeqCst);
        }
    }
}
