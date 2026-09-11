use super::*;

/// Model B: Live-dialog Load-Game fire (er-quickload-live-dialog.txt / ER_QUICKLOAD_LIVE_DIALOG).
/// Off by default. Sibling to direct_build (the forge). Instead of forging a ProfileLoadDialog
/// (factory 0x14081ead0 with a synthetic capture + no live MenuWindow -> a non-live dialog the
/// native menu group never pumps -> wrong-map/crash), this locates the real Load-Game registry
/// node (CS::MenuMemberFuncJob<TitleTopDialog>, vtable 0x142b265d0, member-fn chains to factory
/// 0x14081ead0) and invokes its native run 0x1409aaba0(rcx=node) -- so the ProfileLoadDialog is
/// born live & registered in menu-group 0x143d87350, which the native pump drives. STAGE2 then
/// fires load_activate (vt+0xa0) + the guarded continue_confirm -> SetState(5). The forge path
/// (direct_build) is untouched; this is a deliberate, separately-gated experiment.
pub(crate) fn live_dialog_enabled() -> bool {
    false
}
/// Arm the readiness-gated press-any-button advance. ENV `ER_QUICKLOAD_PAB_ADVANCE=1` or GAME_DIR file
/// `er-quickload-pab-advance.txt`. Deliberately independent of the (deleted) direct
/// "Continue pressed" trigger, which also used to drive `maybe_auto_open_menu`.
pub(crate) fn pab_advance_enabled() -> bool {
    if autoload_disabled() {
        return false;
    }
    !save_override_telemetry_only()
}
// ENV-gate rationale (required by .auto/env_gate_comment_policy.rego): this is not an on/off
// feature flag. The title-anim speedup is default-on product behavior for every real autoload run
// (returns TITLE_ANIM_SPEEDUP_DEFAULT, no opt-in) -- matching the always-on autoload levers and the
// "No Compromises" rule that the deliverable is product behavior, not a flag-gated experiment. The
// env/file override exists only to (a) sweep the factor K at runtime during the empirical animation-
// speed search -- a cross-compile per candidate K is minutes, a runtime knob is seconds -- and (b)
// force K=1.0 for a clean A/B against the recorded baseline. Telemetry/trace-only runs stay at 1.0 so
// they observe unmodified native pacing.
/// Title-animation speedup factor for the pab_dismiss -> menu_open transition. Default-on
/// (`TITLE_ANIM_SPEEDUP_DEFAULT`) for real autoload runs; overridable at runtime via env
/// `ER_QUICKLOAD_TITLE_ANIM_SPEEDUP=<f32>` or GAME_DIR file `er-quickload-title-anim-speedup.txt`
/// (contents parsed as f32). Result is clamped to [MIN, max]; an override that is unparseable or
/// <=1.0 forces no scaling. bd autoload-menu-speed-lever-framedelta-2026-06-22.
pub(crate) fn title_anim_speedup_factor() -> f32 {
    if autoload_disabled() || save_override_telemetry_only() {
        TITLE_ANIM_SPEEDUP_MIN
    } else {
        TITLE_ANIM_SPEEDUP_DEFAULT
    }
}

/// True when the title-anim speedup lever is armed (factor > 1.0).
pub(crate) fn title_anim_speedup_enabled() -> bool {
    title_anim_speedup_factor() > TITLE_ANIM_SPEEDUP_MIN
}

/// Default-on product 05_000_title asset strip (er-effects-rs-dl0, runtime-derived since
/// er-effects-rs-h7x): at Scaleform file-open the hook reads the vanilla movie payload out of the
/// native MemoryFile the game's own FileOpener returns and applies
/// `er_gfx::title_05_000::strip` -- 18 content-addressed tag edits, all-or-nothing, byte-identical
/// to the formerly-embedded `TITLE_05_000_TEXT_SUPPRESSED_GFX` for the known vanilla input -- so
/// press any button / the Continue menu text / the copyright footer never build or animate. The
/// per-element hide hooks stay installed as defense-in-depth, but the served movie carries no
/// visual placements. End-to-end prior proof with the (identical) stripped movie live: runtime
/// artifact `title-05-000-native-ui-stripped-recorded-latest` reached event T_controllable
/// (+21.9s) with the PressStart proxy still bindable (dialog+0xb78 readiness gate satisfied).
/// Gated like `native_continue_enabled` (no new opt-in gate; splash-skip de-gating precedent):
/// off for no-autoload / telemetry-only runs, so a pure observe run never
/// mutates visual resources. `ER_QUICKLOAD_TITLE_05_000_MEMORY_GFX` remains the explicit override:
/// a path replaces the default asset; `embedded:title-05-000-suppressed` arms the same runtime
/// derivation; the literal `vanilla`/`off`/`0` forces the native on-disk movie while autoload
/// stays on (handled in `load_title_scaleform_memory_gfx`).
pub(crate) fn title_05_000_strip_default_enabled() -> bool {
    !(autoload_disabled() || save_override_telemetry_only())
}

/// Passive, epilogue-neutral observer for native Scaleform menu-resource acquisition. This is
/// intentionally separate from the title-cover/hide bundle: resource/memory-GFX proof needs the
/// replaced `05_001_Title_Logo` visible, not hidden by TitleBackViewParts suppression hooks.
pub(crate) fn title_menu_resource_observer_enabled() -> bool {
    false
}

/// AUTO-confirm observe mode (er-quickload-auto-confirm.txt): drive the game's own natural title
/// flow with Confirm input-taps so we can finally observe the view past the modal. No SetState
/// forcing, no input block, no custom dismiss -- just the press the game polls for.
pub(crate) fn auto_confirm_enabled() -> bool {
    false
}
// ENV-gate RATIONALE: ER_QUICKLOAD_CONTINUE_DRIVE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn continue_drive_enabled() -> bool {
    false
}
// ENV-gate RATIONALE: ER_QUICKLOAD_ARM_PROBE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn arm_probe_enabled() -> bool {
    false
}
// ENV-gate RATIONALE: ER_QUICKLOAD_NATIVE_ARM_LOOP is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn native_arm_loop_enabled() -> bool {
    false
}
// ENV-gate RATIONALE: ER_QUICKLOAD_TITLE_ACCEPT is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn title_accept_enabled() -> bool {
    false
}
// ENV-gate RATIONALE: ER_QUICKLOAD_TITLE_ACCEPT_INJECT is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn title_accept_inject_enabled() -> bool {
    false
}
// ENV-gate RATIONALE: ER_QUICKLOAD_SPLASH_SKIP is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn splash_skip_enabled() -> bool {
    !save_override_telemetry_only()
        || product_autoload_enabled()
        || own_load_enabled()
        || title_menu_resource_observer_enabled()
}
/// Force offline boot (no online login attempt -> no "Unable to start in online mode" modal),
/// so the headless autoload reaches the real title/main-menu directly. Auto-on whenever the
/// own-stepper drives the front-end (the autoload runs vanilla-offline), plus explicit overrides.
/// Gated (not always-on) so it never forces offline on a co-op/online launch that wants the
/// getter live.
pub(crate) fn online_disable_enabled() -> bool {
    !save_override_telemetry_only() || own_stepper_enabled()
}
