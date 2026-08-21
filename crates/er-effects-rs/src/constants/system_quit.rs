// ============================================================================================
/// XInput poll counter, incremented each XInputGetState call while inject-nav is active and the
/// menu is open. The schedule below is in these poll-frames.
pub(crate) use er_telemetry::counters::INJECT_NAV_FRAME;
pub(crate) use er_title_flow::XINPUT_GAMEPAD_DPAD_DOWN;
/// XINPUT_GAMEPAD.wButtons bits for the System->Quit repro autopilot's controller sequence
/// (D-pad Up, Start, Left-Shoulder/LB, A). D-pad Down is XINPUT_GAMEPAD_DPAD_DOWN above.
pub(crate) const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
/// D-pad Left/Right bits. Used only by the Save Game confirm-chain drive, which does not know
/// (and must not guess) which axis a two-button `CS::MessageBoxDialog` lays its buttons out on:
/// it pulses candidates and latches whichever one actually moves the dialog cursor.
pub(crate) const XINPUT_GAMEPAD_DPAD_LEFT: u16 = 0x0004;
pub(crate) const XINPUT_GAMEPAD_DPAD_RIGHT: u16 = 0x0008;
pub(crate) const XINPUT_GAMEPAD_START: u16 = 0x0010;
pub(crate) const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
pub(crate) const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
pub(crate) const XINPUT_GAMEPAD_A: u16 = 0x1000;
/// XINPUT_GAMEPAD.wButtons B bit (menu Back/Cancel).
pub(crate) const XINPUT_GAMEPAD_B: u16 = 0x2000;
/// Current game-task tick's synthesized gamepad wButtons for the System->Quit repro autopilot,
/// written by `system_quit_repro_tick` and READ by the XInput poll hook (the stage the game reads a
/// gamepad from). 0 = no button. Distinct from INJECT_NAV_CUR_BUTTONS (own_stepper title nav).
pub(crate) use er_telemetry::counters::SQ_REPRO_XINPUT_BUTTONS;
/// ProfileSelect cursor index captured on entry to TO_SLOT (the current/most-recent save the cursor
/// defaults to). The autopilot moves the cursor until it differs, guaranteeing a NON-current save.
/// usize::MAX = not yet captured (reset on entry to TO_SLOT).
pub(crate) use er_telemetry::counters::SQ_REPRO_INITIAL_CURSOR;
/// Throttle the per-tap log.
pub(crate) use er_telemetry::counters::INJECT_NAV_LOG_COUNT;
pub(crate) const INJECT_NAV_LOG_FIRST: usize = 20;
/// The current frame's synthesized gamepad wButtons, computed by the per-frame schedule in
/// own_stepper idx10 and READ by the XInput hook (so the schedule lives in one place that runs
/// every frame, instead of the XInput hook which the game may never poll). 0 = no input.
pub(crate) use er_telemetry::counters::INJECT_NAV_CUR_BUTTONS;
// ---- CAN-MOVE probe (2026-07-18, user-directed readiness gate) ----
// "render-ready" answers "can the user SEE the character"; CAN-MOVE answers "does INPUT MOVE the
// character" -- the second half of the readiness the earlier automated capture lacked. When
// MOVE_PROBE_ACTIVE, the XInput hook stamps MOVE_PROBE_STICK_LY into the left thumbstick (sThumbLY);
// the driver samples oracle_havok_pos before/after and confirms motion beyond a noise threshold.
// play_time advancing is necessary but NOT sufficient (it ticks during the freeze), so movement must
// be proven by a position DELTA under a known injected stick, per AGENTS.md direct-measurement.
/// True while the readiness verifier is injecting a movement stick to test input-causes-movement.
pub(crate) static MOVE_PROBE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Left-thumbstick Y to inject while MOVE_PROBE_ACTIVE (i16 range; +full = forward). Stored as i32.
pub(crate) static MOVE_PROBE_STICK_LY: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
pub(crate) use er_title_flow::CAN_MOVE_CONFIRMED;
/// HARNESS-ATTRIBUTED movement verdict for the CURRENT load epoch -- the contamination-proof result
/// (user 2026-07-20, bd canmove-contaminated-user-moved-harness-never-supplied). The move-probe
/// alternates INJECT-ON / INJECT-OFF windows and requires the char to move WHILE WE inject AND stop
/// when we release, so a USER moving the char cannot read as proof. 0=pending, 1=PROVEN (moved under
/// our stick, still when released), 2=DISPROVEN (our injection did not move it), 3=CONTAMINATED
/// (moved while we were NOT injecting -> external input present). Reset per load epoch. The watcher
/// tears down the instant this leaves 0 (bd collect-decisive-info-teardown-immediately).
pub(crate) static HARNESS_MOVE_VERDICT: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(0);
/// FPS oracle (goal 2026-07-19: stable framerate, comparable across runs, load1 baseline). EMA of the
/// per-frame delta in microseconds (init ~60fps). Written each game-task frame by lifecycle, read by the
/// telemetry oracles as oracle_fps = 1e6 / this. Also the per-epoch worst (max) frame time in us.
pub(crate) static FRAME_TIME_EMA_US: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(16_667);
pub(crate) static FRAME_TIME_WORST_US: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);
/// Load epoch the worst-frame-time window is scoped to (reset the worst tracker when the epoch changes).
pub(crate) use er_telemetry::counters::FRAME_TIME_WORST_EPOCH;
/// Current consecutive-moved-frame count of the in-flight move probe (for the oracle/report).
pub(crate) use er_telemetry::counters::MOVE_PROBE_MOVED_FRAMES;
/// SEMAPHORE SPLIT (user 2026-07-19, bd three-semaphores-can-move-did-move-supplied-input): count of
/// frames the probe actually WROTE the forward stick into a live pad device (`SUPPLIED_MOVEMENT_INPUT`
/// = did WE inject). Distinct from CAN_MOVE (capability) and DID_MOVE (real displacement): if supplied
/// climbs but DID_MOVE stays 0, the injection layer is wrong/ignored (e.g. pad stick vs kb+mouse WASD).
pub(crate) use er_telemetry::counters::SUPPLIED_MOVEMENT_INPUT_FRAMES;
/// CUMULATIVE count of frames with real havok displacement >= threshold WHILE supplying input
/// (`DID_MOVE` = did the character actually move). Unlike MOVE_PROBE_MOVED_FRAMES it does NOT reset on a
/// non-moving frame, so `DID_MOVE > 0` means "moved at least once under our input". Reset per load epoch.
pub(crate) use er_telemetry::counters::DID_MOVE_FRAMES;
/// The load epoch (fresh_deser_count) the current probe is bound to, so it resets per load.
pub(crate) use er_telemetry::counters::MOVE_PROBE_EPOCH;
/// Forward stick deflection the probe injects (near full), the per-FRAME horizontal displacement (world
/// units) that counts as "moving" (a static/frozen char repeats its position exactly, delta ~0; a walk
/// clears this easily), and the sustained consecutive-frame count that PROVES movement (user: 60/load).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MOVE_PROBE_STICK_FORWARD: i32 = 30000;
pub(crate) const MOVE_PROBE_PER_FRAME_THRESHOLD: f32 = 0.01;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MOVE_PROBE_REQUIRED_FRAMES: usize = 60;
/// DInput keyboard scancode DIK_DOWN (down-arrow) -- the menu "move down" keyboard input. The
/// menu is keyboard-navigated under Proton with no controller (XInput is not polled), so the
/// schedule drives this via InputBlocker::set_injected_key (stamped into the blocked keyboard
/// state). 0xD0 = DIK_DOWNARROW.
pub(crate) const DIK_DOWN: u8 = 0xd0;
/// No key injected (clears the stamp on gap/settle frames).
pub(crate) const DIK_NONE: u8 = 0;
/// System->Quit Save Game REPRO AUTOPILOT state machine. Reproduces the controller path to the
/// in-world System menu and always activates the Save Game row by fabricating the XInput gamepad poll
/// (see `system_quit_repro_tick`). Each phase issues its KNOWN edges once and advances ONLY on an
/// observed transition (menu-window semaphore / save-request telemetry / close telemetry) -- never a
/// timer, tap budget, or retry count:
///   WAIT_WORLD -> WAIT_RELOAD (menu-free programmatic switch arm) -> DONE.
/// The intermediate menu-nav states 1..5 (OPEN_MENU / TO_SYSTEM / TO_PROFILE / TO_SLOT / CONFIRM)
/// are GONE: nothing ever transitioned into them, so the arms that implemented them were shipped but
/// unreachable machine code. The values are left unused rather than renumbered because DONE=6 and
/// WAIT_RELOAD=7 are compared against in telemetry consumers.
pub(crate) const SQ_REPRO_STATE_WAIT_WORLD: usize = 0;
pub(crate) const SQ_REPRO_STATE_DONE: usize = 6;
/// Between two back-to-back switches: after a switch's OK is confirmed, wait here for THAT switch's
/// reload to commit (fresh-deser count reached) and the NEW world to be up + settled, then re-arm
/// the state machine (clear the per-switch window/cursor/confirm signals) and drive the next switch.
/// Distinct from DONE so `block_input_enabled`/`xinput_get_state_hook` keep the block engaged and the
/// fabricated pad driving across the reload (they gate on `!= DONE`).
pub(crate) const SQ_REPRO_STATE_WAIT_RELOAD: usize = 7;
/// TAB-RETURN repro (gated by `er-effects-tab-return-repro.txt`): from the open OptionSetting, navigate
/// RIGHT (RB) to the last tab (the Quit/Exit tab, where our injected rows build), then LEFT (LB) back to
/// tab 0 (Game Options), then dwell -- reproducing the blank Game Options pane the user reported (a tab
/// goes blank on RETURN after visiting the custom tab). Uses OPTIONSETTING_CURRENT_TAB feedback.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_STATE_TAB_RETURN: usize = 8;
/// PROFILE-BACK repro: capture per-tab row-table baselines, open the cloned Load Profile row, press
/// B on ProfileSelect, wait for restore, then revisit tabs and compare exact row-table fingerprints.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK_BASELINE: usize = 9;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK_OPEN: usize = 10;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK: usize = 11;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK_TO_GAME_TAB: usize = 12;
/// SAVE-GAME SELF-DRIVE (save-game-flow WP2): once the Save Game row has opened the destination list,
/// walk the confirm boxes the way a user does -- move the dialog cursor onto the affirmative
/// button, then press confirm -- checkpointed on `oracle_save_flow_stage`, never on timers.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_STATE_SAVE_CONFIRM: usize = 13;
/// Nav directions the confirm-chain drive tries, in order, until one moves the dialog cursor.
/// The winning direction is latched in `SQ_REPRO_BOX_NAV_BUTTON` for the rest of the run; a
/// two-button box wraps, so any working axis converges on the target index.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_BOX_NAV_CANDIDATES: [u16; 4] = [
    XINPUT_GAMEPAD_DPAD_LEFT,
    XINPUT_GAMEPAD_DPAD_RIGHT,
    XINPUT_GAMEPAD_DPAD_UP,
    XINPUT_GAMEPAD_DPAD_DOWN,
];
/// Latched working nav button for the confirm boxes (0 = not discovered yet).
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static SQ_REPRO_BOX_NAV_BUTTON: AtomicUsize = AtomicUsize::new(0);
/// Dialog cursor observed when the current nav candidate began its pulse (usize::MAX = none).
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static SQ_REPRO_BOX_NAV_BASELINE: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Candidate index currently being pulsed while the nav direction is still unknown.
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static SQ_REPRO_BOX_NAV_CANDIDATE: AtomicUsize = AtomicUsize::new(0);
/// Frames with no tab change before we treat the strip end as reached (phase 0 -> 1).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_TAB_RETURN_STALL_TICKS: usize = 40;
/// Dwell on Game Options this many ticks so the pane-visibility oracle samples the (blank) tab 0.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_TAB_RETURN_DWELL_TICKS: usize = 180;
pub(crate) static SQ_REPRO_STATE: AtomicUsize = AtomicUsize::new(SQ_REPRO_STATE_WAIT_WORLD);
/// Which back-to-back switch the autopilot is driving (0-based). Switch `i` loads
/// `SQ_REPRO_TARGET_SLOTS[i]`. Proves the feature can load N different characters after one startup.
pub(crate) use er_telemetry::counters::SQ_REPRO_SWITCH_INDEX;
/// How many back-to-back harness-driven switches to drive. Bounded by `SQ_REPRO_TARGET_SLOTS.len()`.
///
/// The Save Game row repro is always-on when the repro harness itself is enabled; it no longer needs
/// an env selector. The legacy switch-count constants below are retained for older ProfileSelect
/// harness code paths, but the active Save Game validation path stops once save-request + menu-close
/// telemetry fires.
// 2 back-to-back switches = load1 -> load2 -> load3 (the goal's "no less than two successive loads"
// after the first automatic load). The harness ships inert (harness_dll_present), so this only affects
// agent-owned runs. Overridable per-run via er-effects-sq-target-switches.txt.
pub(crate) const SQ_REPRO_TARGET_SWITCHES: usize = 2;
/// Exact ProfileSelect Back repro latches. `DONE` means the self-drive opened System->Quit's cloned
/// Load Profile row, observed ProfileSelect, sent B/Back, observed restore, returned to Game Options,
/// and did not arm a profile load.
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_OPENED;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_DONE;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_RESTORE_BASELINE;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_RESTORE_COUNT;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_FINAL_TAB;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_BASELINE_MASK;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_VERIFY_MASK;
pub(crate) use er_telemetry::counters::SQ_REPRO_PROFILE_BACK_MISMATCH_MASK;
pub(crate) static SQ_REPRO_PROFILE_BACK_BASELINE_HASHES: [AtomicUsize; 10] =
    [const { AtomicUsize::new(0) }; 10];
pub(crate) static SQ_REPRO_PROFILE_BACK_BASELINE_COUNTS: [AtomicUsize; 10] =
    [const { AtomicUsize::new(usize::MAX) }; 10];
pub(crate) static SQ_REPRO_PROFILE_BACK_VERIFY_HASHES: [AtomicUsize; 10] =
    [const { AtomicUsize::new(0) }; 10];
pub(crate) static SQ_REPRO_PROFILE_BACK_VERIFY_COUNTS: [AtomicUsize; 10] =
    [const { AtomicUsize::new(usize::MAX) }; 10];
/// The explicit ProfileSelect slot each switch loads. Slots 4/5 are the two REAL, distinct
/// characters in the pinned gold save (25-Invades-patches): slot 4 = 'Speed Bean', slot 5 =
/// 'Patches' (bd system-quit-switch-loads-original-not-picked-rootcause-2026-07-02). The autopilot
/// drives the ProfileSelect cursor to the exact target (not "one off current"), so each switch lands
/// on a real character regardless of which slot the reload made current. The third entry returns to
/// slot 4, matching the 3rd in-session ProfileSelect open that crashed the native thumbnail builder
/// on the empty renderer table (er-effects-rs-j3r), the deterministic repro/validation for the
/// table-repair hook.
// SAME-CHARACTER repeat load (the goal: two+ successive loads of angrE, slot 0). Every switch loads
// slot 0, not a different slot per switch -- the old [0,1,2,..] loaded a DIFFERENT character on switch #2
// (user 2026-07-21: "the stats on screen don't match the player loaded"). Override per-run via
// er-effects-sq-target-slots.txt if a multi-character sweep is ever wanted.
pub(crate) const SQ_REPRO_TARGET_SLOTS: [i32; 10] = [0; 10];
/// Baseline of (confirmed_block + confirmed_allow) counts captured at each switch's start, so the
/// CONFIRM state detects THIS switch's OK as an increase over the baseline rather than a cumulative
/// `!= 0` (which switch #2 would trip immediately on switch #1's residual count).
pub(crate) use er_telemetry::counters::SQ_REPRO_CONFIRM_BASELINE;
/// Game-task tick counter within the current repro state (reset to 0 on each state transition). The
/// per-phase edge index is `tick / INJECT_NAV_CYCLE`; the injected edge hold/gap timing REUSES the
/// RE-grounded own_stepper nav constants (edge-triggered menu nav needs a multi-frame hold to
/// register one step; a 1-frame tap is missed -- bd keyboard-dik-down-injection-works-cursor-moves-
/// 2026). No sq-repro-specific timing value is invented.
pub(crate) use er_telemetry::counters::SQ_REPRO_STATE_TICK;
/// Latches "waiting-for-transition self-reported" for the current state so it logs exactly once
/// (0 = not yet); reset on each state transition. Not a tap budget -- a boolean.
pub(crate) use er_telemetry::counters::SQ_REPRO_STATE_TAPS;
/// Frames spent in WAIT_RELOAD with a failing gate (reset per switch via `sq_repro_begin_switch`).
/// The observed er-effects-rs-qwj stall sat here with switch #1 stable and fresh-deser == expected,
/// so one of the gates was lying; the periodic gate dump (every `SQ_REPRO_WAIT_RELOAD_LOG_EVERY`
/// frames) names the culprit with data instead of a single opaque waiting line.
pub(crate) use er_telemetry::counters::SQ_REPRO_WAIT_RELOAD_FRAMES;
/// WAIT_RELOAD gate-dump period in frames (~8.5s at 60fps): frequent enough to bound a stall fast,
/// sparse enough to never spam the debug log across a full reload (~10-15s).
pub(crate) const SQ_REPRO_WAIT_RELOAD_LOG_EVERY: usize = 512;
/// Frames to settle in-world (world stream + HUD) before the autopilot presses START. Pre-existing
/// world-readiness settle; the run that first opened IngameTop used it.
pub(crate) const SQ_REPRO_WORLD_SETTLE_TICKS: usize = 180;
/// Frames the switch-arm gate will wait, AFTER the settle, for the current load to PROVE genuine
/// movement (HARNESS_MOVE_VERDICT==1: the can-move probe confirmed >=60 frames of injected-stick
/// movement with a clean OFF-tail). Once the probe is gated on the rendered state (2026-07-21) the
/// verdict fires reliably (load3 latched it), so waiting for it makes EACH load prove movement before
/// the next switch, not just the last. Reaching this timeout emits a failed-epoch verdict and leaves the
/// harness parked; it never advances past an unproven load.
pub(crate) const SQ_REPRO_MOVE_PROOF_TIMEOUT_TICKS: usize = 900;
/// FREEZE VERDICT DEADLINE. Frames the WAIT_RELOAD gate allows a reload to prove genuine movement
/// before emitting a one-shot frozen-epoch verdict. The harness does NOT advance to another switch on
/// this deadline: doing so overwrote the still-open portrait target and made the failed epoch disappear
/// under a recovery load. The bounded run's global cap owns teardown if movement never proves.
pub(crate) const SQ_REPRO_FREEZE_RECOVERY_DEADLINE: usize = 900;
/// WAIT_WORLD movement-proof deadline (2026-07-18): before driving switch #1, wait for load1 to PROVE
/// movement (CAN_MOVE_CONFIRMED) so the reload is triggered from a genuinely playable state, not a
/// half-streamed one -- prior runs drove OPEN_MENU while load1 was still at mms 13-16. Fallback ticks
/// so a load that never proves movement still advances (per the strict parity, driving one more load
/// recovers a frozen one) instead of hard-stalling. ~1500f (~47s at 32fps) is generous for a real load.
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const SQ_REPRO_WAIT_WORLD_MOVE_DEADLINE: usize = 1500;
/// No gamepad buttons asserted this frame.
pub(crate) const INJECT_NAV_NO_BUTTONS: u16 = 0;
pub(crate) use er_title_flow::MSGBOX_CLOSING_LATCH_3B0_OFFSET;
pub(crate) use er_title_flow::MSGBOX_CLOSING_YES;
pub(crate) use er_title_flow::MSGBOX_LATCH_BYTE_MASK;
/// THE OK-BUTTON HANDLER 0x14078e030(rcx=dialog) -- the std::function the menu router invokes when
/// OK is pressed. Captured from a real OK-press (commit 0x14078ef20 fired with caller 0x78e09c, in
/// the function entered at 0x78e030). It takes ONLY rcx=dialog: reads the dialog cursor (0x140739e20
/// = [dialog+0xd4]), gets the OK callback (0x14078fbd0 from [dialog+0x1298]), builds the result
/// struct (0x1407411e0), and COMMITS (0x14078ef20(dialog, &struct, 1)) -- which closes the dialog
/// AND emits its result to the parent so the title flow PROCEEDS. Calling this each frame on every
/// captured MessageBoxDialog skips ALL of them generically (connection-error, starting-offline, ...)
/// with no input -- it is exactly what a real OK-press runs. Verified entry: `rex push rbx; ... mov
/// rbx,rcx` at 0x78e030; only rcx used.
pub(crate) const MSGBOX_OK_HANDLER_RVA: usize = MsgBoxRva::OkHandler as usize;
/// CONFIRM latch [dialog+0x1bc0] u8 -- the field a real OK-press sets. The dialog's own per-frame
/// UPDATE 0x140927d30 reads it -> commit 0x14078ef20 builds the result functor into [dialog+0x10]
/// -> next UPDATE emits stop via EmitResult (sets the +0x3b0 closing latch) -> the dialog TEARS
/// DOWN. OnDecide alone only highlights/dispatches OK WITHOUT closing (the modal stays visible and
/// blocks the title flow); setting this latch is what actually closes it like a real press.
pub(crate) const MSGBOX_CONFIRM_LATCH_1BC0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, confirm_latch);
pub(crate) const MSGBOX_CONFIRM_LATCH_SET: u8 = true as u8;
pub(crate) const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub(crate) const PAGE_PROTECT_UNSET: u32 = 0;
/// IngameInit drive (recipe B, flagless). The SimpleTitleStep container that
/// bears IngameInit is compiled-in but NEVER instantiated in this build, so we
/// call IngameInit (its state-2 handler) with a SYNTHETIC `this`: it only reads
/// +0xc0 (the InGameStep) and +0x130 (the map -- != -1 = continue, -1 = new
/// game), primes the world subsystems, and SetupLoad-submits the load. Never
/// touches the force flag 0x143d856a0. The map id is produced by the same parser
/// (0x71fd60) over the default map string the new-game path uses.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const OUTER_STEP_INGAMESTEP_OFFSET: usize = 0xc0;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const OUTER_STEP_MAP_OVERRIDE_130_OFFSET: usize = 0x130;
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const INGAMEINIT_HANDLER_RVA: usize = 0xb0a1f0;
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const INGAMEINIT_MAP_PARSER_RVA: usize = 0x71fd60;
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const DEFAULT_MAP_STRING_RVA: usize = 0x2b62c70;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const INGAMEINIT_SYNTHETIC_QWORDS: usize = 0x40;
/// Genuine offline continue drive (recipe Option 1). The MoveMapList save-load
/// dispatcher 0x140afb880 (clean entry; its Arxan-scrambled body cross-jumps to
/// the offline-continue deserialize 0x14067b290 at 0x140afbc3e). With GameMan
/// b73 set it selects current_slot_load 0x67b570 (begin), then drives the async
/// task (GameMan+0xb80 1->2->3) and synchronously deserializes the REAL slot
/// character, also building the world singletons. owner is rbx; owner+0x12c =
/// slot. Done when GameMan+0x10 == 1. Never writes 0x143d856a0.
pub(crate) const MOVEMAP_DISPATCHER_RVA: usize = 0xafb880;
pub(crate) const GAME_MAN_B73_FLAG_OFFSET: usize = GAME_MAN_FLAG_B73_PROBE_OFFSET;
pub(crate) const GAME_MAN_B73_FLAG_SET: u8 = true as u8;
pub(crate) const GAME_MAN_REAL_LOAD_DONE_OFFSET: usize =
    core::mem::offset_of!(GameMan, warp_requested);
pub(crate) const GAME_MAN_REAL_LOAD_DONE_VALUE: i32 = true as i32;
#[repr(C)]
pub(crate) struct ContinueOwnerLayout {
    pub(crate) storage: [usize; 0x40],
}

#[repr(C)]
pub(crate) struct ContinueOwnerFields {
    pub(crate) unknown_000: [u8; 0x12a],
    pub(crate) flag_12a: u8,
    pub(crate) unknown_12b: u8,
    pub(crate) slot: i32,
}

pub(crate) const CONTINUE_OWNER_SLOT_OFFSET: usize =
    core::mem::offset_of!(ContinueOwnerFields, slot);
pub(crate) const CONTINUE_OWNER_FLAG_12A_OFFSET: usize =
    core::mem::offset_of!(ContinueOwnerFields, flag_12a);
pub(crate) const CONTINUE_OWNER_FLAG_12A_VALUE: u8 = false as u8;
pub(crate) const CONTINUE_OWNER_QWORDS: usize =
    core::mem::size_of::<ContinueOwnerLayout>() / core::mem::size_of::<usize>();
pub(crate) const CONTINUE_DRIVE_MIN_TICK: u64 = 120;
pub(crate) const CONTINUE_DRIVE_AFTER_GAME_MAN_TICKS: u64 = u64::MIN;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET: usize = 0xb28;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET: usize = 0x12d;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET: usize = 0x12e;
/// InGameStep manual-tick experiment (lever / "direct drive the load"). The
/// load job at `owner+0x2e8` is a `CS::InGameStep` whose step machine only
/// advances while its FD4StepTemplate::Execute pump (`0x140b0bd60`) is ticked
/// each frame. `force_play_game` submits the load (`job+0xd8=1`) but never ticks
/// the step, so it orphans. The engine already calls `0x140b0bd60` every frame
/// on the inner TitleStep, so we DETOUR it and, when it fires for the inner
/// TitleStep at GameStepWait, also call the original on the InGameStep with the
/// SAME live ctx — reusing the engine's real per-frame context (float dt at
/// ctx+0x8) instead of fabricating one. The InGameStep's own state lives at
/// `+0x48` (`-1` == finished); we tick only while `+0xd8 != 0` and `+0x48 != -1`.
#[allow(dead_code)] // Retained RE address: decoded from the game binary, no live caller today.
pub(crate) const STEP_PUMP_DRIVER_RVA: u32 = 0x00b0bd60;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const INGAMESTEP_FINISHED_SENTINEL: i32 = -1;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const INGAMESTEP_LOAD_DONE: i32 = 0;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const INGAMESTEP_PUMP_D8_UNOBSERVED: i32 = -2;
/// FD4StepTemplate force-state override fields (pump `0x140b0bd60` @ 0xb0be01:
/// `if byte[+0x69]!=0 && byte[+0xa8]==0 { +0x48 = +0x4c = [+0xac]; +0xa8=0 }`).
/// If `+0x69` is set and `+0xac` pins the step index, the machine never advances.
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const INGAMESTEP_OVERRIDE_TRIGGER_OFFSET: usize = 0x69;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const INGAMESTEP_OVERRIDE_GUARD_OFFSET: usize = 0xa8;
#[allow(dead_code)] // Retained RE offset: decoded struct layout, no live reader today.
pub(crate) const INGAMESTEP_OVERRIDE_TARGET_OFFSET: usize = 0xac;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const INGAMESTEP_OVERRIDE_TRIGGER_CLEAR: u8 = false as u8;
pub(crate) const MENU_TASK_NULL_STATE_QWORD: usize = NULL_MODULE_BASE;
pub(crate) const MENU_TASK_NULL_PAYLOAD_PTR: usize = NULL_MODULE_BASE;
pub(crate) const MENU_TASK_STATE_PAYLOAD_CODE_OFFSET: usize =
    core::mem::offset_of!(MenuTaskStateLayout, payload_code);
pub(crate) const MENU_TRACE_EVENT_INCREMENT: usize = true as usize;
pub(crate) const TASK_ENQUEUE_TRACE_INCREMENT: usize = true as usize;
pub(crate) static START_GAME_TASK: Once = Once::new();
pub(crate) static START_CONTINUE_TRACE: Once = Once::new();
pub(crate) static START_SAFE_INPUT_HOOKS: Once = Once::new();
pub(crate) static START_SPLASH_SKIP: Once = Once::new();
pub(crate) static START_ONLINE_DISABLE: Once = Once::new();
// START_FOREGROUND_FORCE removed 2026-07-16 (foreground-force dropped from the product).
pub(crate) static START_SOUND_POST_EVENT_OBSERVER: Once = Once::new();
pub(crate) static START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS: Once = Once::new();
pub(crate) static START_TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS: Once = Once::new();
pub(crate) static START_TITLE_LOGO_START_LOGIN_HIDE: Once = Once::new();
pub(crate) static START_TITLE_LOGO_FORCE_HIDDEN: Once = Once::new();
pub(crate) static START_TITLE_PAB_INFORMATION_COVER: Once = Once::new();
pub(crate) static START_TITLE_GFX_VALUE_SET_VISIBLE: Once = Once::new();
pub(crate) static START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND: Once = Once::new();
pub(crate) static START_TITLE_SCALEFORM_BIND_OBSERVER: Once = Once::new();
pub(crate) static START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER: Once = Once::new();
pub(crate) static START_TITLE_FLOW_CONTEXT_RECORD_REGULATION: Once = Once::new();
/// One-shot install guard for the stats-panel native-text hooks (named-child capture + SetText).
pub(crate) static START_PROFILE_STATS_TEXT: Once = Once::new();
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static START_NOW_LOADING_HELPER_OBSERVER: Once = Once::new();
/// One-shot install of the loading-tip suppression detour (er-effects-rs-jsm). Installed at DLL attach,
/// BEFORE the KnowledgeLoadingScreen ctor sets the first tip (~15s), so no native tip is ever set.
pub(crate) static START_TIP_SUPPRESSION: Once = Once::new();
/// One-shot install of the always-on Scaleform descriptor-heap null guard (er-effects-rs-y22i).
/// Installed unconditionally at DLL attach -- it is a crash guard, not a feature.
pub(crate) static START_SCALEFORM_GUARD: Once = Once::new();
/// One-shot install latch for the D3D12 Present overlay (the deterministic loading-portrait display path).
pub(crate) static START_PRESENT_OVERLAY: Once = Once::new();
pub(crate) static START_PROFILE_RENDERER_TEARDOWN_SPARE: Once = Once::new();
pub(crate) static START_PROFILE_SELECT_TABLE_DIAG: Once = Once::new();
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static START_TITLE_CUSTOM_COVER_RUN: Once = Once::new();
pub(crate) static START_BOOT_PROFILER: Once = Once::new();
/// One-shot latch for the "first game-task frame ran" boot-phase marker (0 = not yet logged).
pub(crate) use er_telemetry::counters::BOOT_FIRST_FRAME_LOGGED;
pub(crate) static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize =
    AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);
pub(crate) use er_telemetry::counters::SAFE_INPUT_CONFIRM_FRAMES_REMAINING;

pub(crate) static MENU_CONTINUE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_NEW_OR_LOAD_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_OTHER_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static NATIVE_SUBMIT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static RESULT_EVENT_HANDLER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static RESULT_ACTION_BUILDER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static RESULT_EVENT_WRAPPER_BUILDER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static TASK_ENQUEUE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SET_SAVE_SLOT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SAVE_REQUEST_PROFILE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static REQUEST_SAVE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CURRENT_SLOT_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CONTINUE_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static COMBINED_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MAP_LOAD_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static SAVE_LOAD_STATE_INIT_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
// MENU-UI capture (Path B / zero-input state-stepper): log-only trampolines on the title
// menu-navigation functions so one real user navigation (press-any-key -> Continue/Load ->
// slot -> confirm) yields the exact this-pointers + construction order + call sequence for
// the 4 interactions. SetState (state sequence), Continue confirm, ProfileLoadDialog activate
// (slot-20 + variant), the enter-Load-Game builder, the selector-step tick, the menu mount.
pub(crate) static CAP_SETSTATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_LOAD_ACTIVATE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_LOAD_ACTIVATE2_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_BUILDER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_SELECTOR_TICK_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_MENU_DESER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// ProfileLoadDialog lambda factory 0x14081ead0 (op-new 0x1cd0 + ctor 0x1409a3d90). Hooking
/// it with a caller backtrace captures the full construction chain: press-any-key -> main
/// menu -> "Load Game" activated -> dialog built, plus the rcx/rdx context the factory needs
/// (so the dialog can be built zero-input in the replay).
pub(crate) static CAP_DIALOG_FACTORY_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// Title CSMenu-controller ("router_this") ctor 0x1409060d8: installs the controller vtable
/// (runtime 0x142afa070) and the +0x1290 selectable-row vector. Hooking it captures the live
/// router_this -- the object that owns the Continue/Load-Game/NewGame rows -- which is NOT
/// field-linked from the TitleTopDialog (a dialog-struct scan misses it). Latched into
/// MENU_ROUTER_THIS so the own-stepper can read its rows + drive the Load-Game select zero-input.
pub(crate) static CAP_CSMENU_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_CSMENU_CTOR_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_CSMENU_CTOR_LOG_FIRST: usize = TraceSampleLimit::Value8 as usize;
/// The captured title CSMenu controller (router_this). 0 until its ctor 0x1409060d8 latches it.
pub(crate) static MENU_ROUTER_THIS: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The title-menu "Load Game" ROW entry (stride-0x210 row whose action functor [entry+0xf8]
/// chains to dialog_factory 0x14081ead0). Captured by the row-push hook's post-build scan. Its
/// layout is the CSMenu-row layout (action at +0xf8), DISTINCT from the FD4 MenuWindowJob d180
/// (+0xa8). Invoking its action builds the ProfileLoadDialog zero-input.
pub(crate) static MENU_LOADGAME_ROW_ENTRY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The matching "Continue" row entry (action -> continue_confirm 0x140b0e180), for reference.
pub(crate) static MENU_CONTINUE_ROW_ENTRY: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native title-menu task node whose update wrapper is ContinueWrapper 0x14082bac0. Captured by
/// the FD4 registry enqueue hook after TitleTopDialog::open_menu materializes the native menu.
pub(crate) static MENU_CONTINUE_TASK_NODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Native TitleTopDialog Continue MenuMemberFuncJob node whose member function reaches
/// ContinueWrapper 0x14082bac0. This is a passive semantic latch only; product proof must still
/// advance through native accept/submit semantics, not direct-load shortcuts.
pub(crate) static MENU_CONTINUE_MEMBER_NODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// Passive native submit/result-chain telemetry. These hooks only call through and record whether
/// product execution entered native submit, result.vtable+0x60, and the action builder; they must
/// never drive load directly.
pub(crate) static NATIVE_SUBMIT_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static NATIVE_SUBMIT_LAST_RESULT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_HANDLER_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_ACTION_BUILDER_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_EVENT_LAST_RESULT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_EVENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_RAW_QWORD0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_FD4_CODE: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_EVENT_LAST_FD4_ARG: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_RESULT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_EVENT: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WORD0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WORD1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_INSERT_HITS: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_ACTION_LAST_INSERT_ARG0: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_ARG1: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_INSERT_RET_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_WRAPPER_BUILDER_HITS: AtomicUsize =
    AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RCX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RDX: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_R8: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RET: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// router_this ctor RVA and its installed (runtime) primary vtable RVA (= base+this at runtime;
/// on-disk objdump shows 0x2af9270, +0xe00 dump/PE skew).
/// REAL function entry is 0x1409060d0 (`rex push rbp` prologue, objdump-verified); the doc's
/// 0x9060d8 lands AFTER 5 pushes (push rbp/rsi/rdi/r12/r13) -- hooking there installs a
/// trampoline mid-prologue and corrupts the stack, so the prior capture was unreliable.
pub(crate) const CSMENU_CTOR_RVA: u32 = ProfileLoadMenuRva::CsMenuCtor as u32;
pub(crate) const ROUTER_THIS_VTABLE_RVA: usize = 0x02afa070;
/// Row-push functions (RELIABLE .text RVAs, no .rdata skew): rebuild_rows 0x14078d2c0 (bulk
/// emplace) and append_one 0x14078eea0 (single). If EITHER fires headless the Continue/Load rows
/// ARE materialized zero-input (and rcx reaches router_this); if NEITHER fires the interactive
/// menu controller is input-instantiated (the architectural floor). rcx = list-model container;
/// [container+8] = router_this back-ptr.
pub(crate) static CAP_REBUILD_ROWS_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_APPEND_ONE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
/// FD4/menu registry insertion helper 0x1407a7b60, called directly by TitleTopDialog::open_menu
/// after each menu entry descriptor is built. The existing task_enqueue_7a7b60 hook logs
/// rcx/rdx/ret fingerprints to map where the opened Continue/Load-Game entries are stored.
pub(crate) static CAP_MENU_INSERT_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_MENU_INSERT_LOG_FIRST: usize = TraceSampleLimit::Value24 as usize;

#[repr(C)]
pub(crate) struct CapMenuInsertTraceLayout {
    pub(crate) vtable: usize,
    pub(crate) qword_8: usize,
    pub(crate) qword_10: usize,
    pub(crate) qword_18: usize,
    pub(crate) unknown_20: [u8; 0x18],
    pub(crate) qword_38: usize,
    pub(crate) unknown_40: [u8; 0x10],
    pub(crate) qword_50: usize,
}

pub(crate) const CAP_MENU_INSERT_VTABLE_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, vtable);
pub(crate) const CAP_MENU_INSERT_QWORD_8_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_8);
pub(crate) const CAP_MENU_INSERT_QWORD_10_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_10);
pub(crate) const CAP_MENU_INSERT_QWORD_18_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_18);
pub(crate) const CAP_MENU_INSERT_QWORD_38_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_38);
pub(crate) const CAP_MENU_INSERT_QWORD_50_OFFSET: usize =
    core::mem::offset_of!(CapMenuInsertTraceLayout, qword_50);
pub(crate) static CAP_ROW_PUSH_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_ROW_PUSH_LOG_FIRST: usize = 12;
/// UNCONDITIONAL row-push capture: log the caller stack of EVERY rebuild_rows/append_one fire
/// (first N), regardless of whether the container is the title menu. Under Model A the row
/// populate fires for the ProfileLoadDialog slot list (not the title Continue/Load list), so the
/// content-gated `inspect_row_container` log would miss it; this captures WHO triggers populate.
pub(crate) static CAP_ROW_PUSH_ALLFIRE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_ROW_PUSH_ALLFIRE_LOG_FIRST: usize = 24;
pub(crate) const REBUILD_ROWS_RVA: u32 = 0x0078d2c0;
pub(crate) const APPEND_ONE_RVA: u32 = 0x0078eea0;
pub(crate) const ROW_CONTAINER_BACKPTR_8: usize = 0x8;
pub(crate) static CAP_SELECTOR_TICK_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_SELECTOR_TICK_LOG_FIRST: usize = TraceSampleLimit::Value4 as usize;
pub(crate) const CAP_SELECTOR_TICK_LOG_INTERVAL: usize = CAP_SELECTOR_TICK_LOG_INTERVAL_TICKS;
/// Selector-owner step (0x140826d50) install-flag field: 0 on the first tick (fires the
/// delegate-installer 0x140828270), 1 afterwards.
#[repr(C)]
pub(crate) struct SelectorStepLayout {
    pub(crate) unknown_000: [u8; 0x68],
    pub(crate) install_flag: u8,
}

pub(crate) const SELECTOR_STEP_INSTALL_FLAG_68_OFFSET: usize =
    core::mem::offset_of!(SelectorStepLayout, install_flag);
// b80 save-mount orchestration capture (own-stepper-dispatcher-mount-failed-and-wrote-
// save-2026 next-approach): entry/exit logging trampolines on the 5 b80 functions so a
// real user-driven .co2 load yields the exact call order + args + which fn populates
// io18/io20 + which transitions b80 + which applies the character.
pub(crate) static B80_PREVIEW_INITIATOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_LOAD_SAVE_DATA_INITIATOR_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_FULL_LOAD_INITIATOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_POLL_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static B80_DESERIALIZE_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static C30_WRITER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::GET_ASYNC_KEY_STATE_ORIG;
pub(crate) use er_telemetry::counters::GET_KEY_STATE_ORIG;
pub(crate) use er_telemetry::counters::DIRECT_INPUT8_CREATE_ORIG;
pub(crate) use er_telemetry::counters::DIRECT_INPUT_CREATE_DEVICE_ORIG;
pub(crate) use er_telemetry::counters::DIRECT_INPUT_GET_DEVICE_STATE_ORIG;
pub(crate) use er_telemetry::counters::TITLE_HANDOFF_COMPLETE;
pub(crate) use er_title_flow::TITLE_OWNER_PTR;
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static FORCE_PLAY_GAME_CALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
/// Last owner (TitleStep) pointer seen by the SetState trace detour. The detour fires from the
/// FIRST title transition (~+12s), long before the TITLE_OWNER_PTR scan caches it (~+31s), so the
/// gm-snap session-liveness sampler falls back to this to cover the BOOT load window.
pub(crate) use er_telemetry::counters::TITLE_SETSTATE_TRACE_LAST_OWNER;
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static SUBMIT_PLAY_GAME_PHASE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(SUBMIT_PHASE_INIT);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static FORCE_PLAY_GAME_LAST_STATE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(FORCE_PLAY_GAME_STATE_UNOBSERVED);
pub(crate) use er_title_flow::TITLE_ACCEPT_BYTE_GATE_FIRED;
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static INGAMESTEP_PUMP_LAST_D8: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(INGAMESTEP_PUMP_D8_UNOBSERVED);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static INGAMESTEP_PUMP_LAST_NEXT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(INGAMESTEP_PUMP_D8_UNOBSERVED);
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static INGAMESTEP_UNPIN_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static CONTINUE_OWNER_PTR: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) const CONTINUE_DRIVE_GM_FIRST_SEEN_UNSET: u64 = 0;
pub(crate) static CONTINUE_DRIVE_GM_FIRST_SEEN_TICK: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(CONTINUE_DRIVE_GM_FIRST_SEEN_UNSET);
pub(crate) static CONTINUE_DRIVE_FIRST_ATTEMPT_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static CONTINUE_DRIVE_BEGUN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static ORIGINAL_EXIT_PROCESS: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_TERMINATE_PROCESS: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_RTL_EXIT_USER_PROCESS: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_NT_TERMINATE_PROCESS: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static ORIGINAL_ASSERT_WRAPPER: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) use er_telemetry::counters::ASSERT_LOG_LINES_WRITTEN;
pub(crate) static PROCESS_EXIT_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) use er_telemetry::counters::AV_LOG_LINES_WRITTEN;
pub(crate) use er_telemetry::counters::FATAL_EXCEPTION_LOG_LINES_WRITTEN;
pub(crate) use er_telemetry::counters::OTHER_EXCEPTION_LOG_LINES_WRITTEN;
/// Base address (HINSTANCE) of THIS injected DLL, captured from `DllMain`'s hmodule at
/// `DLL_PROCESS_ATTACH`. Under Wine/Proton the DLL is relocated far from the game module
/// (observed ~0x6ffe_xxxx_xxxx), so a crash whose faulting RIP / return addresses land in
/// our own code print as raw values the game-base resolver cannot decode. Recording our own
/// base lets the AV handler annotate those frames as `self+0xRVA`, mappable via the DLL's
/// symbols. `NULL_MODULE_BASE` until DllMain runs.
pub(crate) static SELF_DLL_BASE: AtomicUsize = AtomicUsize::new(NULL_MODULE_BASE);
/// `SizeOfImage` of this DLL (PE optional-header field read from `SELF_DLL_BASE`), so the AV
/// handler can bound-check an address to `[base, base+size)` before treating it as `self+RVA`.
pub(crate) use er_telemetry::counters::SELF_DLL_SIZE;
pub(crate) static CRASH_LOGGER_INSTALLED: std::sync::Once = std::sync::Once::new();
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static INGAMEINIT_DRIVE_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) use er_title_flow::TITLE_OWNER_SCAN_COUNTDOWN;
pub(crate) static SAFE_INPUT_CONFIRM_PULSE_SEQ: AtomicUsize =
    AtomicUsize::new(SAFE_INPUT_FIRST_PULSE_INDEX as usize);
pub(crate) static MENU_TRACE_EVENT_SEQ: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MENU_TRACE_LAST_SEQ: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static MENU_TRACE_LAST_HOOK_RVA: AtomicUsize =
    AtomicUsize::new(TRACE_UNKNOWN_TABLE_RVA as usize);
pub(crate) static MENU_TRACE_LAST_TABLE_RVA: AtomicUsize =
    AtomicUsize::new(TRACE_UNKNOWN_TABLE_RVA as usize);
pub(crate) static MENU_TRACE_LAST_THIS: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static MENU_TRACE_LAST_STATE_QWORD: AtomicUsize =
    AtomicUsize::new(MENU_TASK_NULL_STATE_QWORD);
pub(crate) static MENU_TRACE_LAST_PAYLOAD_PTR: AtomicUsize =
    AtomicUsize::new(MENU_TASK_NULL_PAYLOAD_PTR);
pub(crate) static TASK_ENQUEUE_TRACE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
