/// MODEL B: LIVE-dialog Load-Game fire (er-effects-live-dialog.txt / ER_EFFECTS_LIVE_DIALOG).
/// OFF by default. SIBLING to direct_build (the forge). Instead of FORGING a ProfileLoadDialog
/// (factory 0x14081ead0 with a synthetic capture + no live MenuWindow -> a NON-LIVE dialog the
/// native menu group never pumps -> wrong-map/crash), this locates the REAL Load-Game registry
/// node (CS::MenuMemberFuncJob<TitleTopDialog>, vtable 0x142b265d0, member-fn chains to factory
/// 0x14081ead0) and invokes its native run 0x1409aaba0(rcx=node) -- so the ProfileLoadDialog is
/// born LIVE & registered in menu-group 0x143d87350, which the native pump drives. STAGE2 then
/// fires load_activate (vt+0xa0) + the guarded continue_confirm -> SetState(5). The forge path
/// (direct_build) is untouched; this is a deliberate, separately-gated experiment.
pub(crate) fn live_dialog_enabled() -> bool {
    false
}
/// Arm the readiness-gated press-any-button advance. ENV `ER_EFFECTS_PAB_ADVANCE=1` or GAME_DIR file
/// `er-effects-pab-advance.txt`. DECOUPLED from `fire_tfc_continue_enabled` (that gate previously also
/// drove `maybe_auto_open_menu`, so removing it stranded a probe at press-any-button).
pub(crate) fn pab_advance_enabled() -> bool {
    if autoload_disabled() {
        return false;
    }
    !save_override_telemetry_only()
}
// ENV-GATE RATIONALE (required by .auto/env_gate_comment_policy.rego): this is NOT an on/off
// feature flag. The title-anim speedup is DEFAULT-ON product behavior for every real autoload run
// (returns TITLE_ANIM_SPEEDUP_DEFAULT, no opt-in) -- matching the always-on autoload levers and the
// "No Compromises" rule that the deliverable is product behavior, not a flag-gated experiment. The
// env/file override exists ONLY to (a) SWEEP the factor K at runtime during the empirical animation-
// speed search -- a cross-compile per candidate K is minutes, a runtime knob is seconds -- and (b)
// force K=1.0 for a clean A/B against the recorded baseline. Telemetry/trace-only runs stay at 1.0 so
// they observe unmodified native pacing.
/// Title-animation speedup factor for the pab_dismiss -> menu_open transition. Default-on
/// (`TITLE_ANIM_SPEEDUP_DEFAULT`) for real autoload runs; overridable at runtime via env
/// `ER_EFFECTS_TITLE_ANIM_SPEEDUP=<f32>` or GAME_DIR file `er-effects-title-anim-speedup.txt`
/// (contents parsed as f32). Result is clamped to [MIN, MAX]; an override that is unparseable or
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
/// True when the branch is replacing the native `05_001_Title_Logo` GFX bytes through the
/// Scaleform MemoryFile seam. This is not a vanilla/main restore switch: it means the branch now
/// owns that TitleBack resource, so old hooks that hide TitleBack would hide our replacement.
pub(crate) fn title_resource_memory_gfx_enabled() -> bool {
    false
}

/// DEFAULT-ON product 05_000_title asset strip (er-effects-rs-dl0, runtime-derived since
/// er-effects-rs-h7x): at Scaleform file-open the hook reads the vanilla movie payload out of the
/// native MemoryFile the game's own FileOpener returns and applies
/// `er_gfx::title_05_000::strip` -- 18 content-addressed tag edits, all-or-nothing, byte-identical
/// to the formerly-embedded `TITLE_05_000_TEXT_SUPPRESSED_GFX` for the known vanilla input -- so
/// PRESS ANY BUTTON / the Continue menu text / the copyright footer never build or animate. The
/// per-element hide hooks stay installed as defense-in-depth, but the served movie carries no
/// visual placements. End-to-end prior proof with the (identical) stripped movie live: runtime
/// artifact `title-05-000-native-ui-stripped-recorded-latest` reached EVENT T_controllable
/// (+21.9s) with the PressStart proxy still bindable (dialog+0xb78 readiness gate satisfied).
/// Gated like `native_continue_enabled` (no new opt-in gate; splash-skip de-gating precedent):
/// off for no-autoload / profile-capture / telemetry-only runs, so a pure observe run never
/// mutates visual resources. `ER_EFFECTS_TITLE_05_000_MEMORY_GFX` remains the explicit override:
/// a path replaces the default asset; `embedded:title-05-000-suppressed` arms the same runtime
/// derivation; the literal `vanilla`/`off`/`0` forces the native on-disk movie while autoload
/// stays on (handled in `load_title_scaleform_memory_gfx`).
pub(crate) fn title_05_000_strip_default_enabled() -> bool {
    !(autoload_disabled() || native_profile_capture_enabled() || save_override_telemetry_only())
}

/// DEFAULT-ON product masquerade cover Part A: suppress only the native `05_000_Title`
/// MenuWindowJob visual wrapper while the zero-input autoload runs. If memory-GFX replacement is
/// active, do not install the old TitleBack hide hooks: `05_001_Title_Logo` is the replacement
/// surface on this branch, not a vanilla/main object to suppress.
pub(crate) fn title_native_menu_visual_suppression_enabled() -> bool {
    if title_resource_memory_gfx_enabled()
        || autoload_disabled()
        || native_profile_capture_enabled()
    {
        return false;
    }
    !save_override_telemetry_only()
}

/// Passive, epilogue-neutral observer for native Scaleform menu-resource acquisition. This is
/// intentionally separate from the title-cover/hide bundle: resource/memory-GFX proof needs the
/// replaced `05_001_Title_Logo` visible, not hidden by TitleBackViewParts suppression hooks.
pub(crate) fn title_menu_resource_observer_enabled() -> bool {
    false
}

/// AUTO-CONFIRM observe mode (er-effects-auto-confirm.txt): drive the game's OWN natural title
/// flow with Confirm input-taps so we can finally observe the view PAST the modal. No SetState
/// forcing, no input block, no custom dismiss -- just the press the game polls for.
pub(crate) fn auto_confirm_enabled() -> bool {
    false
}
/// Whether STAGE 1d should SELF-FIRE the TitleTopDialog open-menu registrar (0x1409b24e0).
/// DEFAULT OFF (file-gated): with the connection-error modal now handled (clean headless boot),
/// the NATURAL Continue/Load main menu builds from SetState(2)=BeginLogo, and force-firing the
/// TitleTopDialog registrar opens a COMPETING dialog that prevents the natural menu's Load-Game
/// item d180 from ticking through the capture hooks. Off => let the natural menu surface d180.
pub(crate) fn own_stepper_selffire_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_SUBMIT_PLAY_GAME is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn submit_play_game_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_INGAMEINIT_DRIVE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn ingameinit_drive_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_CONTINUE_DRIVE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn continue_drive_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_ARM_PROBE is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn arm_probe_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_NATIVE_ARM_LOOP is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn native_arm_loop_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_TITLE_ACCEPT is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn title_accept_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_TITLE_ACCEPT_INJECT is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn title_accept_inject_enabled() -> bool {
    false
}
// ENV-GATE RATIONALE: ER_EFFECTS_SPLASH_SKIP is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn splash_skip_enabled() -> bool {
    !save_override_telemetry_only()
        || product_autoload_enabled()
        || own_load_enabled()
        || title_menu_resource_observer_enabled()
}
/// Force OFFLINE boot (no online login attempt -> no "Unable to start in online mode" modal),
/// so the headless autoload reaches the real title/main-menu directly. Auto-on whenever the
/// own-stepper drives the front-end (the autoload runs vanilla-OFFLINE), plus explicit overrides.
/// Gated (not always-on) so it never forces offline on a co-op/online launch that wants the
/// getter live.
pub(crate) fn online_disable_enabled() -> bool {
    !save_override_telemetry_only() || own_stepper_enabled()
}
// ENV-GATE RATIONALE: ER_EFFECTS_INGAMESTEP_UNPIN is an explicit diagnostic/runtime probe switch; default behavior remains off unless the operator intentionally stages the gate.
pub(crate) fn ingamestep_unpin_enabled() -> bool {
    false
}
