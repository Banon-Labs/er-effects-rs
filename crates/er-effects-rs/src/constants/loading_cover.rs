// ============================================================================================
// DETERMINISTIC MENU INPUT PROBE (instrumentation oracle, er-effects-input-probe.txt). After the
// menu opens, inject a single Down tap (Continue->Load Game) at a KNOWN frame, observe a window
// with NO further input, then inject Confirm at a KNOWN frame. Because WE choose the inject
// frames, the decisive question is frame-precise: does the Load-Game leaf d180 tick its leaf
// Update (0x1407ad1c0 -> MENU_D180_LEAF_TICKED grows) on HIGHLIGHT alone (between Down and
// Confirm), or only at Confirm? This is targeted input used as a MEASUREMENT (NOT the zero-input
// deliverable); the Confirm drives the native load so the full chain is captured at a known frame.
// ============================================================================================
/// Probe frame counter (per own_stepper idx10 call, starting when the probe first runs after the
/// menu opens). Schedule below is in these frames.
pub(crate) static INPUT_PROBE_FRAME: AtomicUsize = AtomicUsize::new(0);
/// Set to 1 once the probe is active so the hot menu hooks can cheaply enable the extra
/// leaf-tick accounting (MENU_D180_LEAF_TICKED) without a per-frame file-exists check.
pub(crate) static INPUT_PROBE_ACTIVE: AtomicUsize = AtomicUsize::new(0);
/// One-shot latch: set when the d180 leaf tick is observed during the HIGHLIGHT window (decisive).
pub(crate) static INPUT_PROBE_D180_PRECONFIRM: AtomicUsize = AtomicUsize::new(0);
/// Snapshot of MENU_D180_LEAF_TICKED captured at the Down-inject frame; HIGHLIGHT growth is
/// measured strictly above this baseline.
pub(crate) static INPUT_PROBE_DOWN_LEAF_BASELINE: AtomicUsize = AtomicUsize::new(0);
/// Count of genuine d180 leaf-Update ticks (bumped ONLY by cap_menu_item_update_hook when the
/// ticked item classifies to dialog_factory). Distinct from MENU_LOAD_GAME_ITEM, which the static
/// sequence-iter walk can also set without d180 actually ticking.
pub(crate) static MENU_D180_LEAF_TICKED: AtomicUsize = AtomicUsize::new(MENU_TRACE_UNSEEN_SEQ);
/// Frame to begin the single Down injection (settle the opened menu first).
pub(crate) const INPUT_PROBE_DOWN_START: u64 = 120;
/// Assert the move bit for this many consecutive frames = one clean edge (one cursor step).
pub(crate) const INPUT_PROBE_DOWN_TAP_FRAMES: u64 = 2;
/// Observation window AFTER the Down, with NO input, before the Confirm injection.
pub(crate) const INPUT_PROBE_HIGHLIGHT_FRAMES: u64 = 180;
/// Frame to begin the Confirm injection (= Down end + highlight window).
pub(crate) const INPUT_PROBE_CONFIRM_START: u64 =
    INPUT_PROBE_DOWN_START + INPUT_PROBE_DOWN_TAP_FRAMES + INPUT_PROBE_HIGHLIGHT_FRAMES;
pub(crate) const INPUT_PROBE_CONFIRM_TAP_FRAMES: u64 = 2;
pub(crate) const INPUT_PROBE_LOG_INTERVAL: u64 = 20;

// ============================================================================================
// SELF-DRIVEN GAMEPAD NAV INJECTION (instrument-capture). Distinct from the disproven
// inputmgr+0x90 keystate write (PROVEN non-functional): this injects at the XInput poll source
// (XInputGetState, the stage the game actually reads gamepad from), so a synthesized D-pad Down
// reaches the real input pipeline. The block stays ON (user input suppressed) while the hook
// fabricates the pad state on a schedule, cycling the title-menu cursor so the input/focus-gated
// row populate fires and the row-push/csmenu-ctor hooks capture WHO triggers it -- with the
// user's input blocked so nothing pollutes. Capture-only: D-pad Down nav, NEVER Confirm/A (no
// load, no save write).
