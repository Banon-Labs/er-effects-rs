// ============================================================================================
/// XInput poll counter. It was named for the inject-NAV drive that once owned it; that drive is
/// deleted and the sole remaining consumer is the XInput poll hook, which bumps it on every
/// fabricated poll to guarantee a fresh `dwPacketNumber`.
pub(crate) use er_telemetry_core::counters::INJECT_NAV_FRAME;
/// XINPUT_GAMEPAD.wButtons D-pad Up bit.
pub(crate) const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
/// D-pad Left/Right bits.
pub(crate) const XINPUT_GAMEPAD_DPAD_LEFT: u16 = 0x0004;
pub(crate) const XINPUT_GAMEPAD_DPAD_RIGHT: u16 = 0x0008;
/// Synthesized gamepad wButtons read by the XInput poll hook (the stage the game reads a gamepad
/// from). 0 = no button. Its only writer was the deleted System->Quit repro autopilot, so it now
/// reads 0 on every poll.
// The whole inject-NAV drive is gone (2026-08-26): the branch in
// product_core_own_stepper/fallback_drives.rs, its counters (INJECT_NAV_LOG_COUNT /
// INJECT_NAV_LOG_FIRST / INJECT_NAV_CUR_BUTTONS, deleted from er-telemetry-core), its poll-frame
// tap/gap schedule (`inject_nav_buttons` + constants, deleted from er-title-flow), the XInput
// force-connect term that served it, and finally the `inject_nav_enabled()` gate itself -- which
// could only ever return `false`, so none of it ran on any build.
// ---- Can-move probe (2026-07-18, user-directed readiness gate) ----
// "render-ready" answers "can the user SEE the character"; Can-move answers "does input move the
// character" -- the second half of the readiness the earlier automated capture lacked. When
// MOVE_PROBE_ACTIVE, the XInput hook stamps MOVE_PROBE_STICK_LY into the left thumbstick (sThumbLY);
// the driver samples oracle_havok_pos before/after and confirms motion beyond a noise threshold.
// play_time advancing is necessary but not sufficient (it ticks during the freeze), so movement must
// be proven by a position delta under a known injected stick, per AGENTS.md direct-measurement.
/// True while the readiness verifier is injecting a movement stick to test input-causes-movement.
pub(crate) static MOVE_PROBE_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
/// Left-thumbstick Y to inject while MOVE_PROBE_ACTIVE (i16 range; +full = forward). Stored as i32.
pub(crate) static MOVE_PROBE_STICK_LY: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(0);
pub(crate) use er_title_flow::CAN_MOVE_CONFIRMED;
/// Harness-attributed movement verdict for the current load epoch -- the contamination-proof result
/// (user 2026-07-20, bd canmove-contaminated-user-moved-harness-never-supplied). The move-probe
/// alternates inject-on / inject-off windows and requires the char to move while we inject and stop
/// when we release, so a user moving the char cannot read as proof. 0=pending, 1=proven (moved under
/// our stick, still when released), 2=DISPROVEN (our injection did not move it), 3=contaminated
/// (moved while we were not injecting -> external input present). Reset per load epoch. The watcher
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
pub(crate) use er_telemetry_core::counters::FRAME_TIME_WORST_EPOCH;
/// Current consecutive-moved-frame count of the in-flight move probe (for the oracle/report).
pub(crate) use er_telemetry_core::counters::MOVE_PROBE_MOVED_FRAMES;
/// SEMAPHORE split (user 2026-07-19, bd three-semaphores-can-move-did-move-supplied-input): count of
/// frames the probe actually wrote the forward stick into a live pad device (`SUPPLIED_MOVEMENT_INPUT`
/// = did we inject). Distinct from CAN_MOVE (capability) and DID_MOVE (real displacement): if supplied
/// climbs but DID_MOVE stays 0, the injection layer is wrong/ignored (e.g. pad stick vs kb+mouse WASD).
pub(crate) use er_telemetry_core::counters::SUPPLIED_MOVEMENT_INPUT_FRAMES;
/// Cumulative count of frames with real havok displacement >= threshold while supplying input
/// (`DID_MOVE` = did the character actually move). Unlike MOVE_PROBE_MOVED_FRAMES it does not reset on a
/// non-moving frame, so `DID_MOVE > 0` means "moved at least once under our input". Reset per load epoch.
pub(crate) use er_telemetry_core::counters::DID_MOVE_FRAMES;
/// The load epoch (fresh_deser_count) the current probe is bound to, so it resets per load.
pub(crate) use er_telemetry_core::counters::MOVE_PROBE_EPOCH;
/// Forward stick deflection the probe injects (near full), the per-frame horizontal displacement (world
/// units) that counts as "moving" (a static/frozen char repeats its position exactly, delta ~0; a walk
/// clears this easily), and the sustained consecutive-frame count that proves movement (user: 60/load).
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MOVE_PROBE_STICK_FORWARD: i32 = 30000;
pub(crate) const MOVE_PROBE_PER_FRAME_THRESHOLD: f32 = 0.01;
#[allow(dead_code)] // Retained RE constant: no live reader today, kept with the table it was decoded into.
pub(crate) const MOVE_PROBE_REQUIRED_FRAMES: usize = 60;
// DIK_DOWN (0xd0, DIK_DOWNARROW) was stamped into the blocked keyboard state by the inject-NAV
// branch alone, so it went with that branch. DIK_NONE below is still written by the can-move probe.
/// No key injected (clears the stamp on gap/settle frames).
pub(crate) const DIK_NONE: u8 = 0;
/// DirectInput scancode for the 'W' key -- the forward-movement binding the can-move probe stamps
/// into the game's own `GetDeviceState` keyboard buffer. Not the Win32 VK (0x57): this is the DIK the
/// DInput8 keyboard device reports, which is the only keyboard stage `eldenring.exe` reads (1.17
/// imports DINPUT8 + USER32 `GetKeyState`/`GetKeyboardState` and no RawInput API at all).
pub(crate) const DIK_W: u8 = 0x11;
/// Win32 virtual-key code for 'W' -- the same forward-movement key as [`DIK_W`], expressed for the
/// USER32 `GetKeyState`/`GetKeyboardState` stage rather than the DirectInput one.
pub(crate) const VK_W: u8 = 0x57;
// Deleted 2026-09-05 with the System->Quit repro autopilot: SQ_REPRO_STATE and its DONE/WAIT_RELOAD
// values, SQ_REPRO_SWITCH_INDEX, and the `sq_repro_state` / `sq_repro_switch_index` telemetry fields
// they fed. Once the autopilot's tick was gone nothing advanced either, so both would have published
// a constant 0 forever -- a watcher reading "state 0, switch 0" cannot tell a pinned dead counter
// from a run that genuinely never switched, which is worse than the field being absent.
// INJECT_NAV_NO_BUTTONS went with the inject-NAV branch: it existed only to compare against that
// schedule's per-frame wButtons.
pub(crate) use er_title_flow::MSGBOX_CLOSING_LATCH_3B0_OFFSET;
pub(crate) use er_title_flow::MSGBOX_CLOSING_YES;
pub(crate) use er_title_flow::MSGBOX_LATCH_BYTE_MASK;
/// The OK-button handler 0x14078e030(rcx=dialog) -- the std::function the menu router invokes when
/// OK is pressed. Captured from a real OK-press (commit 0x14078ef20 fired with caller 0x78e09c, in
/// the function entered at 0x78e030). It takes only rcx=dialog: reads the dialog cursor (0x140739e20
/// = [dialog+0xd4]), gets the OK callback (0x14078fbd0 from [dialog+0x1298]), builds the result
/// struct (0x1407411e0), and commits (0x14078ef20(dialog, &struct, 1)) -- which closes the dialog
/// and emits its result to the parent so the title flow proceeds. Calling this each frame on every
/// captured MessageBoxDialog skips all of them generically (connection-error, starting-offline, ...)
/// with no input -- it is exactly what a real OK-press runs. Verified entry: `rex push rbx; ... mov
/// rbx,rcx` at 0x78e030; only rcx used.
pub(crate) const MSGBOX_OK_HANDLER_RVA: usize = MsgBoxRva::OkHandler as usize;
/// Confirm latch [dialog+0x1bc0] u8 -- the field a real OK-press sets. The dialog's own per-frame
/// update 0x140927d30 reads it -> commit 0x14078ef20 builds the result functor into [dialog+0x10]
/// -> next update emits stop via EmitResult (sets the +0x3b0 closing latch) -> the dialog tears
/// down. OnDecide alone only highlights/dispatches OK without closing (the modal stays visible and
/// blocks the title flow); setting this latch is what actually closes it like a real press.
pub(crate) const MSGBOX_CONFIRM_LATCH_1BC0_OFFSET: usize =
    core::mem::offset_of!(MsgBoxDialogLayout, confirm_latch);
pub(crate) const MSGBOX_CONFIRM_LATCH_SET: u8 = true as u8;
pub(crate) const PAGE_EXECUTE_READWRITE: u32 = 0x40;
pub(crate) const PAGE_PROTECT_UNSET: u32 = 0;
/// IngameInit drive (recipe B, flagless). The SimpleTitleStep container that
/// bears IngameInit is compiled-in but never instantiated in this build, so we
/// call IngameInit (its state-2 handler) with a synthetic `this`: it only reads
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
/// task (GameMan+0xb80 1->2->3) and synchronously deserializes the real slot
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
/// on the inner TitleStep, so we detour it and, when it fires for the inner
/// TitleStep at GameStepWait, also call the original on the InGameStep with the
/// same live ctx — reusing the engine's real per-frame context (float dt at
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
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_NATIVE_MENU_VISUAL_SUPPRESS: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_NATIVE_MENU_VISUAL_RENDER_SUPPRESS: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_LOGO_START_LOGIN_HIDE: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_LOGO_FORCE_HIDDEN: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_PAB_INFORMATION_COVER: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_GFX_VALUE_SET_VISIBLE: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_SCENE_OBJ_PROXY_NAMED_CHILD_BIND: Once = Once::new();
#[allow(dead_code)] // title-cover hide: call site commented out, pending the delete pass
pub(crate) static START_TITLE_SCALEFORM_BIND_OBSERVER: Once = Once::new();
pub(crate) static START_TITLE_MENU_RESOURCE_ACQUIRE_OBSERVER: Once = Once::new();
pub(crate) static START_TITLE_FLOW_CONTEXT_RECORD_REGULATION: Once = Once::new();
/// One-shot install guard for the stats-panel native-text hooks (named-child capture + SetText).
pub(crate) static START_PROFILE_STATS_TEXT: Once = Once::new();
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static START_NOW_LOADING_HELPER_OBSERVER: Once = Once::new();
/// One-shot install of the loading-tip suppression detour (er-effects-rs-jsm). Installed at DLL attach,
/// before the KnowledgeLoadingScreen ctor sets the first tip (~15s), so no native tip is ever set.
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
pub(crate) use er_telemetry_core::counters::BOOT_FIRST_FRAME_LOGGED;
pub(crate) static BOOTSTRAP_TELEMETRY_SEEN: AtomicUsize =
    AtomicUsize::new(BOOTSTRAP_TELEMETRY_UNSEEN);
pub(crate) use er_telemetry_core::counters::SAFE_INPUT_CONFIRM_FRAMES_REMAINING;

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
// Menu-UI capture (Path B / zero-input state-stepper): log-only trampolines on the title
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
/// router_this -- the object that owns the Continue/Load-Game/NewGame rows -- which is not
/// field-linked from the TitleTopDialog (a dialog-struct scan misses it). Latched into
/// MENU_ROUTER_THIS so the own-stepper can read its rows + drive the Load-Game select zero-input.
pub(crate) static CAP_CSMENU_CTOR_ORIG: AtomicUsize = AtomicUsize::new(HOOK_ORIGINAL_UNSET);
pub(crate) static CAP_CSMENU_CTOR_COUNT: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
pub(crate) const CAP_CSMENU_CTOR_LOG_FIRST: usize = TraceSampleLimit::Value8 as usize;
/// The captured title CSMenu controller (router_this). 0 until its ctor 0x1409060d8 latches it.
pub(crate) static MENU_ROUTER_THIS: AtomicUsize = AtomicUsize::new(TITLE_OWNER_SCAN_START_ADDRESS);
/// The title-menu "Load Game" row entry (stride-0x210 row whose action functor [entry+0xf8]
/// chains to dialog_factory 0x14081ead0). Captured by the row-push hook's post-build scan. Its
/// layout is the CSMenu-row layout (action at +0xf8), distinct from the FD4 MenuWindowJob d180
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
/// Real function entry is 0x1409060d0 (`rex push rbp` prologue, objdump-verified); the doc's
/// 0x9060d8 lands after 5 pushes (push rbp/rsi/rdi/r12/r13) -- hooking there installs a
/// trampoline mid-prologue and corrupts the stack, so the prior capture was unreliable.
pub(crate) const CSMENU_CTOR_RVA: u32 = ProfileLoadMenuRva::CsMenuCtor as u32;
pub(crate) const ROUTER_THIS_VTABLE_RVA: usize = 0x02afa070;
/// Row-push functions (reliable .text RVAs, no .rdata skew): rebuild_rows 0x14078d2c0 (bulk
/// emplace) and append_one 0x14078eea0 (single). If either fires headless the Continue/Load rows
/// are materialized zero-input (and rcx reaches router_this); if neither fires the interactive
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
/// Unconditional row-push capture: log the caller stack of every rebuild_rows/append_one fire
/// (first N), regardless of whether the container is the title menu. Under Model A the row
/// populate fires for the ProfileLoadDialog slot list (not the title Continue/Load list), so the
/// content-gated `inspect_row_container` log would miss it; this captures who triggers populate.
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
pub(crate) use er_telemetry_core::counters::GET_ASYNC_KEY_STATE_ORIG;
pub(crate) use er_telemetry_core::counters::GET_KEY_STATE_ORIG;
pub(crate) use er_telemetry_core::counters::DIRECT_INPUT8_CREATE_ORIG;
pub(crate) use er_telemetry_core::counters::DIRECT_INPUT_CREATE_DEVICE_ORIG;
pub(crate) use er_telemetry_core::counters::DIRECT_INPUT_GET_DEVICE_STATE_ORIG;
pub(crate) use er_telemetry_core::counters::TITLE_HANDOFF_COMPLETE;
pub(crate) use er_title_flow::TITLE_OWNER_PTR;
#[allow(dead_code)] // Retained diagnostic state: no live reader today, kept with its sibling telemetry.
pub(crate) static FORCE_PLAY_GAME_CALLED: AtomicUsize =
    AtomicUsize::new(TITLE_NATIVE_JOB_NOT_CALLED);
/// Last owner (TitleStep) pointer seen by the SetState trace detour. The detour fires from the
/// first title transition (~+12s), long before the TITLE_OWNER_PTR scan caches it (~+31s), so the
/// gm-snap session-liveness sampler falls back to this to cover the boot load window.
pub(crate) use er_telemetry_core::counters::TITLE_SETSTATE_TRACE_LAST_OWNER;
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
pub(crate) use er_telemetry_core::counters::ASSERT_LOG_LINES_WRITTEN;
pub(crate) static PROCESS_EXIT_LOGGED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
pub(crate) use er_telemetry_core::counters::AV_LOG_LINES_WRITTEN;
pub(crate) use er_telemetry_core::counters::FATAL_EXCEPTION_LOG_LINES_WRITTEN;
pub(crate) use er_telemetry_core::counters::OTHER_EXCEPTION_LOG_LINES_WRITTEN;
pub(crate) use er_telemetry_core::counters::VEH_REENTRANT_REFUSALS;
/// Base address (HINSTANCE) of this injected DLL, captured from `DllMain`'s hmodule at
/// `DLL_PROCESS_ATTACH`. Under Wine/Proton the DLL is relocated far from the game module
/// (observed ~0x6ffe_xxxx_xxxx), so a crash whose faulting RIP / return addresses land in
/// our own code print as raw values the game-base resolver cannot decode. Recording our own
/// base lets the AV handler annotate those frames as `self+0xRVA`, mappable via the DLL's
/// symbols. `NULL_MODULE_BASE` until DllMain runs.
pub(crate) static SELF_DLL_BASE: AtomicUsize = AtomicUsize::new(NULL_MODULE_BASE);
/// `SizeOfImage` of this DLL (PE optional-header field read from `SELF_DLL_BASE`), so the AV
/// handler can bound-check an address to `[base, base+size)` before treating it as `self+RVA`.
pub(crate) use er_telemetry_core::counters::SELF_DLL_SIZE;
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
