// ============================================================================================
/// XInput poll counter, incremented each XInputGetState call while inject-nav is active and the
/// menu is open. The schedule below is in these poll-frames.
pub(crate) static INJECT_NAV_FRAME: AtomicUsize = AtomicUsize::new(0);
/// XINPUT_GAMEPAD.wButtons D-pad Down bit (the menu "move down" gamepad input).
pub(crate) const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
/// XINPUT_GAMEPAD.wButtons bits for the System->Quit repro autopilot's controller sequence
/// (D-pad Up, Start, Left-Shoulder/LB, A). D-pad Down is XINPUT_GAMEPAD_DPAD_DOWN above.
pub(crate) const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
pub(crate) const XINPUT_GAMEPAD_START: u16 = 0x0010;
pub(crate) const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
pub(crate) const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
pub(crate) const XINPUT_GAMEPAD_A: u16 = 0x1000;
/// XINPUT_GAMEPAD.wButtons B bit (menu Back/Cancel).
pub(crate) const XINPUT_GAMEPAD_B: u16 = 0x2000;
/// Current game-task tick's synthesized gamepad wButtons for the System->Quit repro autopilot,
/// written by `system_quit_repro_tick` and READ by the XInput poll hook (the stage the game reads a
/// gamepad from). 0 = no button. Distinct from INJECT_NAV_CUR_BUTTONS (own_stepper title nav).
pub(crate) static SQ_REPRO_XINPUT_BUTTONS: AtomicUsize = AtomicUsize::new(0);
/// ProfileSelect cursor index captured on entry to TO_SLOT (the current/most-recent save the cursor
/// defaults to). The autopilot moves the cursor until it differs, guaranteeing a NON-current save.
/// usize::MAX = not yet captured (reset on entry to TO_SLOT).
pub(crate) static SQ_REPRO_INITIAL_CURSOR: AtomicUsize = AtomicUsize::new(usize::MAX);
/// Settle the freshly-opened menu before injecting (poll-frames).
pub(crate) const INJECT_NAV_SETTLE_FRAMES: usize = 90;
/// Down asserted for this many consecutive poll-frames = one clean edge (one cursor step).
pub(crate) const INJECT_NAV_TAP_LEN: usize = 4;
/// Released gap between taps (edge re-arm; menu nav is edge-triggered, not auto-repeat).
pub(crate) const INJECT_NAV_GAP_LEN: usize = 16;
/// One tap+gap cycle length.
pub(crate) const INJECT_NAV_CYCLE: usize = INJECT_NAV_TAP_LEN + INJECT_NAV_GAP_LEN;
/// Number of Down taps to drive. The problem is fully deterministic: the cursor starts on
/// Continue (index 0) and Load Game is index 1, so EXACTLY ONE Down reaches it. There is no state
/// of knowledge that justifies more than one tap, so this is a literal 1 (not a tunable).
pub(crate) const INJECT_NAV_MAX_CYCLES: usize = 1;
/// Throttle the per-tap log.
pub(crate) static INJECT_NAV_LOG_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) const INJECT_NAV_LOG_FIRST: usize = 20;
/// The current frame's synthesized gamepad wButtons, computed by the per-frame schedule in
/// own_stepper idx10 and READ by the XInput hook (so the schedule lives in one place that runs
/// every frame, instead of the XInput hook which the game may never poll). 0 = no input.
pub(crate) static INJECT_NAV_CUR_BUTTONS: AtomicUsize = AtomicUsize::new(0);
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
///   WAIT_WORLD -> OPEN_MENU (START -> 02_000_IngameTop)
///   -> TO_SYSTEM (UP, A -> 02_040_OptionSetting, the quit submenu)
///   -> TO_PROFILE/TO_SAVE_GAME (LB, A -> Save Game row)
///   -> DONE.
/// After a phase's edges are issued it HOLDS (injects nothing) until its transition is observed, so
/// a genuinely missed edge self-reports (stuck waiting) instead of being papered over by a re-tap.
pub(crate) const SQ_REPRO_STATE_WAIT_WORLD: usize = 0;
pub(crate) const SQ_REPRO_STATE_OPEN_MENU: usize = 1;
pub(crate) const SQ_REPRO_STATE_TO_SYSTEM: usize = 2;
pub(crate) const SQ_REPRO_STATE_TO_PROFILE: usize = 3;
pub(crate) const SQ_REPRO_STATE_TO_SLOT: usize = 4;
pub(crate) const SQ_REPRO_STATE_CONFIRM: usize = 5;
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
pub(crate) const SQ_REPRO_STATE_TAB_RETURN: usize = 8;
/// PROFILE-BACK repro: capture per-tab row-table baselines, open the cloned Load Profile row, press
/// B on ProfileSelect, wait for restore, then revisit tabs and compare exact row-table fingerprints.
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK_BASELINE: usize = 9;
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK_OPEN: usize = 10;
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK: usize = 11;
pub(crate) const SQ_REPRO_STATE_PROFILE_BACK_TO_GAME_TAB: usize = 12;
/// TAB_RETURN sub-phase: 0 = drive RIGHT to the last tab, 1 = drive LEFT back to tab 0, 2 = dwell.
pub(crate) static SQ_REPRO_TAB_RETURN_PHASE: AtomicUsize = AtomicUsize::new(0);
/// Highest tab index seen while driving right (end-of-strip detection) and the tick the dwell began.
pub(crate) static SQ_REPRO_TAB_RETURN_MAX_TAB: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_TAB_RETURN_DWELL_START: AtomicUsize = AtomicUsize::new(0);
/// Frames with no tab change before we treat the strip end as reached (phase 0 -> 1).
pub(crate) const SQ_REPRO_TAB_RETURN_STALL_TICKS: usize = 40;
/// Dwell on Game Options this many ticks so the pane-visibility oracle samples the (blank) tab 0.
pub(crate) const SQ_REPRO_TAB_RETURN_DWELL_TICKS: usize = 180;
pub(crate) static SQ_REPRO_STATE: AtomicUsize = AtomicUsize::new(SQ_REPRO_STATE_WAIT_WORLD);
/// The VK code the OPEN_MENU auto-discovery is currently sending (and the winner when IngameTop
/// opens), so the log reports which menu-open key actually worked on native Windows.
pub(crate) static SQ_REPRO_OPEN_KEY_VK: AtomicUsize = AtomicUsize::new(0);
/// Which back-to-back switch the autopilot is driving (0-based). Switch `i` loads
/// `SQ_REPRO_TARGET_SLOTS[i]`. Proves the feature can load N different characters after one startup.
pub(crate) static SQ_REPRO_SWITCH_INDEX: AtomicUsize = AtomicUsize::new(0);
/// How many back-to-back harness-driven switches to drive. Bounded by `SQ_REPRO_TARGET_SLOTS.len()`.
///
/// The Save Game row repro is always-on when the repro harness itself is enabled; it no longer needs
/// an env selector. The legacy switch-count constants below are retained for older ProfileSelect
/// harness code paths, but the active Save Game validation path stops once save-request + menu-close
/// telemetry fires.
pub(crate) const SQ_REPRO_TARGET_SWITCHES: usize = 1;
/// RAM oracle latch (0 -> 1, never reset): the pause-at-menu autopilot observed 05_010_ProfileSelect
/// open and STOPPED there (transitioned to DONE without TO_SLOT/CONFIRM). Exported as telemetry
/// `sq_repro_paused_at_profile_select`; the pause-probe watcher's PASS gate is this latch == 1 while
/// the no-load semaphores (activate count, quickload phase, fresh-deser count) all still read idle.
pub(crate) static SQ_REPRO_PAUSED_AT_PROFILE_SELECT: AtomicUsize = AtomicUsize::new(0);
/// Exact ProfileSelect Back repro latches. `DONE` means the self-drive opened System->Quit's cloned
/// Load Profile row, observed ProfileSelect, sent B/Back, observed restore, returned to Game Options,
/// and did not arm a profile load.
pub(crate) static SQ_REPRO_PROFILE_BACK_OPENED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_PROFILE_BACK_DONE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_PROFILE_BACK_RESTORE_BASELINE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_PROFILE_BACK_RESTORE_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_PROFILE_BACK_FINAL_TAB: AtomicUsize = AtomicUsize::new(usize::MAX);
pub(crate) static SQ_REPRO_PROFILE_BACK_BASELINE_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_PROFILE_BACK_VERIFY_MASK: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SQ_REPRO_PROFILE_BACK_MISMATCH_MASK: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) const SQ_REPRO_TARGET_SLOTS: [i32; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];
/// Baseline of (confirmed_block + confirmed_allow) counts captured at each switch's start, so the
/// CONFIRM state detects THIS switch's OK as an increase over the baseline rather than a cumulative
/// `!= 0` (which switch #2 would trip immediately on switch #1's residual count).
pub(crate) static SQ_REPRO_CONFIRM_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Game-task tick counter within the current repro state (reset to 0 on each state transition). The
/// per-phase edge index is `tick / INJECT_NAV_CYCLE`; the injected edge hold/gap timing REUSES the
/// RE-grounded own_stepper nav constants (edge-triggered menu nav needs a multi-frame hold to
/// register one step; a 1-frame tap is missed -- bd keyboard-dik-down-injection-works-cursor-moves-
/// 2026). No sq-repro-specific timing value is invented.
pub(crate) static SQ_REPRO_STATE_TICK: AtomicUsize = AtomicUsize::new(0);
/// Latches "waiting-for-transition self-reported" for the current state so it logs exactly once
/// (0 = not yet); reset on each state transition. Not a tap budget -- a boolean.
pub(crate) static SQ_REPRO_STATE_TAPS: AtomicUsize = AtomicUsize::new(0);
/// Frames spent in WAIT_RELOAD with a failing gate (reset per switch via `sq_repro_begin_switch`).
/// The observed er-effects-rs-qwj stall sat here with switch #1 stable and fresh-deser == expected,
/// so one of the gates was lying; the periodic gate dump (every `SQ_REPRO_WAIT_RELOAD_LOG_EVERY`
/// frames) names the culprit with data instead of a single opaque waiting line.
pub(crate) static SQ_REPRO_WAIT_RELOAD_FRAMES: AtomicUsize = AtomicUsize::new(0);
/// WAIT_RELOAD gate-dump period in frames (~8.5s at 60fps): frequent enough to bound a stall fast,
/// sparse enough to never spam the debug log across a full reload (~10-15s).
pub(crate) const SQ_REPRO_WAIT_RELOAD_LOG_EVERY: usize = 512;
/// Frames to settle in-world (world stream + HUD) before the autopilot presses START. Pre-existing
/// world-readiness settle; the run that first opened IngameTop used it.
pub(crate) const SQ_REPRO_WORLD_SETTLE_TICKS: usize = 180;
/// No gamepad buttons asserted this frame.
pub(crate) const INJECT_NAV_NO_BUTTONS: u16 = 0;
/// CURSOR-OFFSET PROBE: with exactly ONE deterministic Down (Continue idx0 -> Load Game idx1),
/// snapshot the live TitleTopDialog dwords just BEFORE the Down (cursor should read 0) and again
/// AFTER it settles (cursor should read 1); the dword that goes 0->1 IS the cursor field. This
/// observes the real offset instead of trusting the unverified +0xb0c guess (which the self-fire
/// run read as 0). Frames are relative to the first poll after menu-open.
pub(crate) const CURSOR_PROBE_BASELINE_FRAME: usize = INJECT_NAV_SETTLE_FRAMES - 2;
pub(crate) const CURSOR_PROBE_POSTDOWN_FRAME: usize = INJECT_NAV_SETTLE_FRAMES + 12;
/// Dwords to scan from the dialog base (covers 0..0x2400, the known field range).
pub(crate) const CURSOR_PROBE_SCAN_DWORDS: usize = 0x900;
/// Only dwords in [0, this) are logged as cursor candidates (a row index is small).
pub(crate) const CURSOR_PROBE_SMALL_MAX: u32 = 8;
/// Cap the candidate-dword log per snapshot.
pub(crate) const CURSOR_PROBE_LOG_CAP: usize = 96;
/// "result emitted / closing" latch, set =1 by EmitResult once the dialog begins teardown. We
/// stop calling OnDecide once this is set (avoids re-dispatch / UAF after teardown).
pub(crate) const MSGBOX_CLOSING_LATCH_3B0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, closing_latch);
pub(crate) const MSGBOX_CLOSING_YES: usize = true as usize;
pub(crate) const MSGBOX_LATCH_BYTE_MASK: usize = u8::MAX as usize;
/// THE OK-BUTTON HANDLER 0x14078e030(rcx=dialog) -- the std::function the menu router invokes when
/// OK is pressed. Captured from a real OK-press (commit 0x14078ef20 fired with caller 0x78e09c, in
/// the function entered at 0x78e030). It takes ONLY rcx=dialog: reads the dialog cursor (0x140739e20
/// = [dialog+0xd4]), gets the OK callback (0x14078fbd0 from [dialog+0x1298]), builds the result
/// struct (0x1407411e0), and COMMITS (0x14078ef20(dialog, &struct, 1)) -- which closes the dialog
/// AND emits its result to the parent so the title flow PROCEEDS. Calling this each frame on every
/// captured MessageBoxDialog skips ALL of them generically (connection-error, starting-offline, ...)
/// with no input -- it is exactly what a real OK-press runs. Verified entry: `rex push rbx; ... mov
/// rbx,rcx` at 0x78e030; only rcx used.
pub(crate) const MSGBOX_OK_HANDLER_RVA: usize = 0x78e030;
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
/// Earliest game-task tick to fire the movie dismiss -- a settle floor; the real
/// gate is the movie singleton being present with the expected vtable. Kept modest
/// so the dismiss reliably fires within the runtime window.
pub(crate) const DISMISS_MIN_TICK: u64 = 120;
/// Generous upper bound on the game image span, to sanity-check that a candidate
/// object's vtable points into the module before dereferencing deeper.
/// Sentinel logged when GameMan is null so the field could not be read.
pub(crate) const ARM_PROBE_FIELD_ABSENT: i64 = -1;
/// IngameInit drive (recipe B, flagless). The SimpleTitleStep container that
/// bears IngameInit is compiled-in but NEVER instantiated in this build, so we
/// call IngameInit (its state-2 handler) with a SYNTHETIC `this`: it only reads
/// +0xc0 (the InGameStep) and +0x130 (the map -- != -1 = continue, -1 = new
/// game), primes the world subsystems, and SetupLoad-submits the load. Never
/// touches the force flag 0x143d856a0. The map id is produced by the same parser
/// (0x71fd60) over the default map string the new-game path uses.
pub(crate) const OUTER_STEP_INGAMESTEP_OFFSET: usize = 0xc0;
pub(crate) const OUTER_STEP_MAP_OVERRIDE_130_OFFSET: usize = 0x130;
pub(crate) const INGAMEINIT_HANDLER_RVA: usize = 0xb0a1f0;
pub(crate) const INGAMEINIT_MAP_PARSER_RVA: usize = 0x71fd60;
pub(crate) const DEFAULT_MAP_STRING_RVA: usize = 0x2b62c70;
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
/// PlayGame load-pair target block, bound to upstream `GameMan::move_map_target`
/// (audit-confirmed equal to the hand-decoded 0x14).
pub(crate) const FORCE_PLAY_GAME_GM_LOAD_VALUE_14_OFFSET: usize =
    core::mem::offset_of!(GameMan, move_map_target);
pub(crate) const FORCE_PLAY_GAME_GM_PAIR_GATE_B28_OFFSET: usize = 0xb28;
pub(crate) const FORCE_PLAY_GAME_GM_VALIDATE_12D_OFFSET: usize = 0x12d;
pub(crate) const FORCE_PLAY_GAME_GM_VALIDATE_12E_OFFSET: usize = 0x12e;
/// SelectBot selection-injection lane (runs 300/301 static decode). The
/// SimpleTitleStep MenuLoop pump 0xb0a5e0 parses a serialized SelectBot stream
/// keyed by "CSEzSelectBot.MoveMapListStep" into owner+0x130 (parsed selection)
/// and submits a task onto owner+0x128 (title queue). The stream data lives in
/// the registry object pointed to by global [0x143d87360]. The pump's direct
/// PlayGame trigger 0xb0a78b is gated by byte [0x143d856a0] (load-active, which
/// the sole writer 0x140c8fe90 sets downstream of the load). This read-only
/// probe samples those fields to confirm the registry is live and the pump idles
/// with an empty stream before any write is attempted.
pub(crate) const SELECTBOT_OWNER_TITLE_QUEUE_128_OFFSET: usize = 0x128;
pub(crate) const SELECTBOT_OWNER_PARSED_SELECTION_130_OFFSET: usize = 0x130;
pub(crate) const SELECTBOT_REGISTRY_GLOBAL_RVA: usize = 0x3d87360;
pub(crate) const SELECTBOT_LOAD_GATE_RVA: usize = 0x3d856a0;
/// The MenuLoop pump 0xb0a5e0 sets `[input_manager+0x6b0]=1` near its entry
/// (`mov rax,[0x143d6b7b0]; mov byte [rax+0x6b0],1` at 0xb0a64d) every frame it
/// executes. Sampling this byte tells us whether the outer SimpleTitleStep is
/// actually running MenuLoop at the title idle (so SelectBot injection would be
/// parsed) or is still parked before it (so injection alone would be a no-op
/// until the title-accept advances the outer state).
pub(crate) const SELECTBOT_INPUT_MANAGER_GLOBAL_RVA: usize = 0x3d6b7b0;
pub(crate) const SELECTBOT_PUMP_RAN_FLAG_OFFSET: usize = 0x6b0;
/// Lever-1 title-accept experiment (runs 304+). Static RE (bd
/// `title-accept-lever-143d856a0`) shows inner MenuJobWait (state 10, 0xb0d400)
/// advances to state 11 (Finish) iff the global byte `[0x143d856a0]` (==
/// `SELECTBOT_LOAD_GATE_RVA`) is non-zero — it is the title-accept/"proceed"
/// latch, not a load-downstream flag. We set it ONCE, only while the inner owner
/// is confirmed at MenuJobWait, to drive the native title-accept with zero input,
/// then keep sampling to observe the cascade.
pub(crate) const TITLE_STEP_MENU_JOB_WAIT_STATE: i32 = TITLE_STEP_MENU_JOB_WAIT;
pub(crate) const TITLE_PROCEED_GATE_SET_VALUE: u8 = true as u8;
/// Global menu-accept byte 0x144589bdc (RVA 0x4589bdc): the decoded "a button was accepted"
/// flag the input pipeline sets on press, read via getter 0x140e85f50 from TitleTopDialog::update
/// (and 22 other menu accept-gates). When non-zero at the parked title, update runs the open-menu
/// registrar 0x1409b24e0 NATURALLY (build Continue/Load + transfer focus -> select-layer build) --
/// unlike a direct registrar self-fire which opened a competing dialog and reverted. Setting this
/// flag zero-input is the ToS-style "satisfy the accept side-effect" advance (NOT a synthesized
/// DInput/keystate/XInput event). bd title-global-accept-byte-144589bdc-zeroinput-advance-2026.
pub(crate) const TITLE_GLOBAL_ACCEPT_BYTE_RVA: usize = 0x4589bdc;
/// Menu-system manager singleton pointer global 0x143d5dea8 (89 refs). The title press-accept
/// handler 0x1409b1260 does `mov rax,[0x143d5dea8]; if rax: movb [rax],1; jmp registrar 0x1409b24e0`
/// -- it sets the singleton's +0 byte (a "menu-open in progress" flag) then opens the main menu
/// IN PLACE. Replicating this (set the flag, then registrar on the validated TitleTopDialog) is the
/// NARROW title-specific advance that should reach the main menu WITHOUT the language/ToS build that
/// the broad global accept byte over-triggers, and without the competing-dialog revert a bare
/// registrar self-fire caused. bd title-accept-to-registrar-narrow-path-143d5dea8-2026.
pub(crate) const TITLE_MENU_TRANSITION_SINGLETON_RVA: usize = 0x3d5dea8;
pub(crate) const TITLE_MENU_TRANSITION_FLAG_SET_VALUE: u8 = true as u8;
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
pub(crate) const STEP_PUMP_DRIVER_RVA: u32 = 0x00b0bd60;
pub(crate) const INGAMESTEP_STEP_STATE_OFFSET: usize = 0x48;
pub(crate) const INGAMESTEP_NEXT_STATE_OFFSET: usize = 0x4c;
pub(crate) const INGAMESTEP_FINISHED_SENTINEL: i32 = -1;
pub(crate) const INGAMESTEP_LOAD_DONE: i32 = 0;
pub(crate) const INGAMESTEP_PUMP_D8_UNOBSERVED: i32 = -2;
/// FD4StepTemplate force-state override fields (pump `0x140b0bd60` @ 0xb0be01:
/// `if byte[+0x69]!=0 && byte[+0xa8]==0 { +0x48 = +0x4c = [+0xac]; +0xa8=0 }`).
/// If `+0x69` is set and `+0xac` pins the step index, the machine never advances.
pub(crate) const INGAMESTEP_OVERRIDE_TRIGGER_OFFSET: usize = 0x69;
pub(crate) const INGAMESTEP_OVERRIDE_GUARD_OFFSET: usize = 0xa8;
pub(crate) const INGAMESTEP_OVERRIDE_TARGET_OFFSET: usize = 0xac;
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
pub(crate) static START_NOW_LOADING_HELPER_OBSERVER: Once = Once::new();
pub(crate) static START_LOADING_BG_REPLACE_BIND: Once = Once::new();
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
pub(crate) static START_TITLE_CUSTOM_COVER_RUN: Once = Once::new();
pub(crate) static START_BOOT_PROFILER: Once = Once::new();
/// One-shot latch for the "first game-task frame ran" boot-phase marker (0 = not yet logged).
pub(crate) static BOOT_FIRST_FRAME_LOGGED: AtomicUsize = AtomicUsize::new(0);
pub(crate) static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize =
    AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);
pub(crate) static SAFE_INPUT_CONFIRM_FRAMES_REMAINING: AtomicUsize = AtomicUsize::new(0);

pub(crate) static MENU_CONTINUE_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_NEW_OR_LOAD_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_OTHER_LOAD_WRAPPER_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static MENU_TASK_UPDATE_WRAPPER_ORIG: AtomicUsize =
    AtomicUsize::new(HOOK_ORIGINAL_UNSET);
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
pub(crate) static GET_ASYNC_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static GET_KEY_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DIRECT_INPUT8_CREATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DIRECT_INPUT_CREATE_DEVICE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static DIRECT_INPUT_GET_DEVICE_STATE_ORIG: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_HANDOFF_COMPLETE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static TITLE_OWNER_PTR: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
pub(crate) static TITLE_OWNER_TRACE_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) static TITLE_NATIVE_JOB_CALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
pub(crate) static FORCE_PLAY_GAME_CALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
/// Trampoline to the original STEP_MenuJobWait (0x140b0d400) for the title-anim speedup hook. 0 = not hooked.
pub(crate) static TITLE_ANIM_SPEED_ORIG: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for installing the title-anim speedup hook.
pub(crate) static TITLE_ANIM_SPEED_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Trampoline to the original title step-setter `SetState(owner,int)` (0x140b0d960) for the
/// read-only state-transition trace hook. 0 = not hooked. bd menu-build-overlap-lever-2026-06-24.
pub(crate) static TITLE_SETSTATE_TRACE_ORIG: AtomicUsize = AtomicUsize::new(0);
/// One-shot guard for installing the title step-setter trace hook.
pub(crate) static TITLE_SETSTATE_TRACE_HOOK_INSTALLED: AtomicUsize = AtomicUsize::new(0);
/// Last owner (TitleStep) pointer seen by the SetState trace detour. The detour fires from the
/// FIRST title transition (~+12s), long before the TITLE_OWNER_PTR scan caches it (~+31s), so the
/// gm-snap session-liveness sampler falls back to this to cover the BOOT load window.
pub(crate) static TITLE_SETSTATE_TRACE_LAST_OWNER: AtomicUsize = AtomicUsize::new(0);
pub(crate) static SUBMIT_PLAY_GAME_PHASE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(SUBMIT_PHASE_INIT);
pub(crate) static FORCE_PLAY_GAME_LAST_STATE: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(FORCE_PLAY_GAME_STATE_UNOBSERVED);
pub(crate) static TITLE_PROCEED_GATE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// One-shot latch for the global-accept-byte (0x144589bdc) zero-input title-advance lever.
pub(crate) static TITLE_ACCEPT_BYTE_GATE_FIRED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static INGAMESTEP_PUMP_LAST_D8: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(INGAMESTEP_PUMP_D8_UNOBSERVED);
pub(crate) static INGAMESTEP_PUMP_LAST_NEXT: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(INGAMESTEP_PUMP_D8_UNOBSERVED);
pub(crate) static INGAMESTEP_UNPIN_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static NATIVE_AUTOLOAD_ARMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static SYNTHETIC_OUTER_PTR: AtomicUsize = AtomicUsize::new(0);
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
pub(crate) static ASSERT_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
pub(crate) static RENDER_FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
pub(crate) static PROCESS_EXIT_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static AV_LOG_LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
/// Base address (HINSTANCE) of THIS injected DLL, captured from `DllMain`'s hmodule at
/// `DLL_PROCESS_ATTACH`. Under Wine/Proton the DLL is relocated far from the game module
/// (observed ~0x6ffe_xxxx_xxxx), so a crash whose faulting RIP / return addresses land in
/// our own code print as raw values the game-base resolver cannot decode. Recording our own
/// base lets the AV handler annotate those frames as `self+0xRVA`, mappable via the DLL's
/// symbols. `NULL_MODULE_BASE` until DllMain runs.
pub(crate) static SELF_DLL_BASE: AtomicUsize = AtomicUsize::new(NULL_MODULE_BASE);
/// `SizeOfImage` of this DLL (PE optional-header field read from `SELF_DLL_BASE`), so the AV
/// handler can bound-check an address to `[base, base+size)` before treating it as `self+RVA`.
pub(crate) static SELF_DLL_SIZE: AtomicUsize = AtomicUsize::new(0);
pub(crate) static CRASH_LOGGER_INSTALLED: std::sync::Once = std::sync::Once::new();
pub(crate) static INGAMEINIT_DRIVE_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) static TITLE_OWNER_SCAN_COUNTDOWN: AtomicUsize =
    AtomicUsize::new(TITLE_OWNER_SCAN_COUNTDOWN_READY);
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
