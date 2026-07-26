#!/usr/bin/env python3
"""Static regressions for the input-harness in-world menu drive path."""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
READINESS_WATCH = REPO_ROOT / "scripts/er-readiness-watch.py"


def test_pad_inject_uses_both_cs_ingame_pad_typeids() -> None:
    src = (REPO_ROOT / "crates/er-input-harness-dll/src/pad_inject.rs").read_text()
    assert "const CS_INGAME_PAD_TYPEID_RVAS: [usize; 2] = [0x3d5df27, 0x3d5df28];" in src
    assert "let targets = CS_INGAME_PAD_TYPEID_RVAS.map(|rva| base + rva);" in src
    assert "for target in targets" in src


def test_pad_inject_direct_stamp_writes_are_enabled() -> None:
    src = (REPO_ROOT / "crates/er-input-harness-dll/src/pad_inject.rs").read_text()
    assert "BISect: write disabled" not in src
    assert "BISECT: write disabled" not in src
    assert "crate::win32::write_u8(cached + off, val)" in src
    assert "crate::win32::write_u8(pad + off, val)" in src


def test_input_harness_manifest_names_actual_hook_layer() -> None:
    manifest = (REPO_ROOT / "crates/er-input-harness-dll/Cargo.toml").read_text()
    assert "FD4PadDevice::poll" not in manifest
    assert "DLUID virtual-key builders/writer" in manifest
    assert "0x240e70/0x241130/0x26634a0" in manifest


def test_samechar_runner_arms_product_movement_for_deterministic_reload_driver() -> None:
    runner = (REPO_ROOT / "scripts/run-samechar-3x-threedll.sh").read_text()
    assert 'DRIVE_RELOAD_SLOTS="${DRIVE_RELOAD_SLOTS-0,0}"' in runner
    assert 'WORLD_STABLE_TIMEOUT_S="${WORLD_STABLE_TIMEOUT_S:-90}"' in runner
    assert 'export WORLD_STABLE_TIMEOUT_S' in runner
    assert 'printf \'1\\n\' >"$GAME_DIR/er-effects-prove-movement.txt"' in runner
    assert 'printf \'1\\n\' >"$GAME_DIR/er-effects-stay-active.txt"' in runner
    assert 'printf \'1\\n\' >"$GAME_DIR/er-effects-input-trace.txt"' in runner
    assert 'if [[ "${OBSERVE_ONLY:-0}" != "1" && ( -z "$DRIVE_RELOAD_SLOTS" || "${FORCE_HARNESS_DRIVE:-0}" == "1" ) ]]; then' in runner
    assert 'if [[ "${OBSERVE_ONLY:-0}" != "1" ]]; then\n\tprintf \'%s\\n\' "${HARNESS_DRIVE_MODE:-full}"' not in runner


def test_continue_and_boot_view_timing_oracles_exist() -> None:
    counters = (REPO_ROOT / "crates/er-telemetry/src/counters.rs").read_text()
    assert "pub static BOOT_VIEW_PUMP_STOP_MS" in counters
    assert "pub static BOOT_VIEW_DARK_GAP_FAILURES" in counters
    assert "pub static BOOT_VIEW_PRESENT_FULL_CLEAR_HITS" in counters
    assert "pub static BOOT_VIEW_PRESENT_COVER_FAILURES" in counters
    assert "pub static BOOT_VIEW_PRE_WORLD_STOP_FAILURES" in counters
    assert "pub static BOOT_VIEW_TELEMETRY_HANDOFF_STAMPS" in counters
    assert "pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_HITS" in counters
    assert "pub static BOOT_VIEW_NATIVE_GFX_FADE_HOLD_COMPLETE_MS" in counters
    assert "pub static LOADING_SCREEN_UPDATE_LAST_MS" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_HOOK_INSTALLED" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_HITS" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_FIRST_MS" in counters
    assert "pub static LOADING_SCREEN_GFX_FADEOUT_LAST_MS" in counters
    assert "pub static LOADING_SCREEN_CLOSE_SENT_FIRST_MS" in counters
    assert "pub static OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS" in counters
    assert "pub static TFC_FORCED_CONTINUE_HANDOFF_MS" in counters

    product_core = (REPO_ROOT / "crates/er-effects-rs/src/experiments/mod/product_core_own_stepper.rs").read_text()
    assert "pub(crate) fn mark_own_load_forced_continue_handoff()" in product_core
    assert "OWN_LOAD_FORCED_CONTINUE_HANDOFF_MS.compare_exchange" in product_core
    assert "pub(crate) fn mark_tfc_forced_continue_handoff()" in product_core
    assert "TFC_FORCED_CONTINUE_HANDOFF_MS.compare_exchange" in product_core

    present = (REPO_ROOT / "crates/er-effects-rs/src/experiments/present_overlay.rs").read_text()
    assert "fn set_boot_view_pump_stop_reason" in present
    assert "BOOT_VIEW_PUMP_STOP_MS.store" in present

    game_oracles = (REPO_ROOT / "crates/er-effects-rs/src/telemetry/runtime_oracles/write_game_module_oracles.rs").read_text()
    assert '"oracle_boot_view_pump_stop_ms"' in game_oracles
    assert '"oracle_boot_view_dark_gap_failures"' in game_oracles
    assert '"oracle_boot_view_missed_handoff_failures"' in game_oracles
    assert '"oracle_boot_view_telemetry_handoff_stamps"' in game_oracles
    assert '"oracle_boot_view_present_full_clear_hits"' in game_oracles
    assert '"oracle_boot_view_present_cover_failures"' in game_oracles
    assert '"oracle_boot_view_pre_world_stop_failures"' in game_oracles
    assert '"oracle_boot_view_native_gfx_fade_hold_hits"' in game_oracles
    assert '"oracle_boot_view_native_gfx_fade_hold_complete_ms"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_hook_installed"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_hits"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_first_ms"' in game_oracles
    assert '"oracle_loading_screen_gfx_fadeout_last_ms"' in game_oracles
    assert '"oracle_loading_screen_update_last_ms"' in game_oracles
    assert '"oracle_loading_screen_close_sent_first_ms"' in game_oracles
    assert "BOOT_VIEW_HANDOFF_SEEN_MS" in game_oracles
    assert ".compare_exchange(0, now_ms" in game_oracles
    readiness_watch = READINESS_WATCH.read_text()
    assert "BOOT_VIEW_DARK_GAP_FAILURE" in readiness_watch
    assert "telemetry_boot_view_dark_gap_failure_detected" in readiness_watch
    assert "oracle_boot_view_missed_handoff_failures" in readiness_watch
    assert "oracle_boot_view_present_cover_failures" in readiness_watch
    assert "boot_view_present_cover_failure" in readiness_watch
    assert "boot_view_pre_world_stop_failure" in readiness_watch
    boot_progress = (REPO_ROOT / "crates/er-effects-rs/src/experiments/gpu_readback/boot_progress.rs").read_text()
    present_overlay = (REPO_ROOT / "crates/er-effects-rs/src/experiments/present_overlay.rs").read_text()
    assert "BOOT_VIEW_EPOCH_WORLD_LIVE.load(Ordering::SeqCst) == cur" in present_overlay
    assert "cur != 0" in present_overlay
    assert "crate::experiments::is_native_windows() && idx >= 8" not in boot_progress
    assert "let can_move_handoff" in boot_progress
    assert "boot_view_cover_release_ready(can_move_handoff)" in boot_progress
    assert "boot_view_player_render_ready" in boot_progress
    assert "BOOT_VIEW_RELEASE_FADE_MS" in boot_progress
    assert "BOOT_VIEW_NATIVE_GFX_FADEOUT_HOLD_MS" in boot_progress
    assert "BOOT_VIEW_NATIVE_LOADING_QUIET_HOLD_MS" in boot_progress
    assert "LOADING_SCREEN_GFX_FADEOUT_LAST_MS.load" in boot_progress
    assert "LOADING_SCREEN_UPDATE_LAST_MS.load" in boot_progress
    assert "native_gfx_hold_pending" in boot_progress
    assert "holding opaque cover through native loading fade/quiet window" in boot_progress
    assert "composite_boot_release_fade_frame" in boot_progress
    assert "IN_WORLD_REACHED.load(Ordering::SeqCst) == IN_WORLD_REACHED_YES)" not in boot_progress
    assert "CAN_MOVE_CONFIRMED" in game_oracles
    assert "MOVE_PROBE_EPOCH" in game_oracles
    assert "is_render_group_enabled" in game_oracles
    telemetry = (REPO_ROOT / "crates/er-effects-rs/src/telemetry/runtime_oracles/write_telemetry.rs").read_text()
    assert 'oracle_own_load_forced_continue_handoff_ms' in telemetry
    assert 'oracle_tfc_forced_continue_handoff_ms' in telemetry


def main() -> int:
    tests = [
        test_pad_inject_uses_both_cs_ingame_pad_typeids,
        test_pad_inject_direct_stamp_writes_are_enabled,
        test_input_harness_manifest_names_actual_hook_layer,
        test_samechar_runner_arms_product_movement_for_deterministic_reload_driver,
        test_continue_and_boot_view_timing_oracles_exist,
    ]
    for test in tests:
        test()
    print("input harness static checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
