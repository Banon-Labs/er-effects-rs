#!/usr/bin/env python3
"""Regression tests for native-bridge boot/loading progress labels."""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
BOOT_PROGRESS = REPO_ROOT / "crates" / "er-effects-rs" / "src" / "experiments" / "gpu_readback" / "boot_progress.rs"
TELEMETRY = REPO_ROOT / "crates" / "er-effects-rs" / "src" / "telemetry" / "runtime_oracles" / "write_game_module_oracles.rs"


def test_phase_labels_are_granular_and_world_gauge_backed() -> None:
    text = BOOT_PROGRESS.read_text(encoding="utf-8", errors="replace")
    assert "const BOOT_VIEW_MILESTONE_COUNT: usize = 12;" in text
    for label in (
        "BUILDING WORLD",
        "STREAMING WORLD",
        "FINALIZING WORLD",
        "ENTERING WORLD",
        "WORLD LOADING",
    ):
        assert label in text
    assert "LOADING_SCREEN_BAR_PROGRESS_PERMILLE" in text
    assert "fn boot_view_world_gauge_submilestone" in text
    assert "{} {}/{} ({} {}/{})" in text


def test_unported_world_ready_claims_are_not_in_boot_label() -> None:
    text = BOOT_PROGRESS.read_text(encoding="utf-8", errors="replace")
    for unproven in (
        "CAN MOVE",
        "PLAYER IN WORLD",
        "SWITCH_ORACLE_PLAYER_PRESENT",
        "SWITCH_ORACLE_MENU_JOB_PRESENT",
    ):
        assert unproven not in text


def test_label_telemetry_is_exposed() -> None:
    telemetry = TELEMETRY.read_text(encoding="utf-8", errors="replace")
    assert "oracle_boot_view_label_hash" in telemetry
    assert "oracle_boot_view_label_ord" in telemetry
    text = BOOT_PROGRESS.read_text(encoding="utf-8", errors="replace")
    assert "BOOT_VIEW_LAST_LABEL_HASH.store" in text
    assert "BOOT_VIEW_MONO_ORD" in text


def main() -> int:
    test_phase_labels_are_granular_and_world_gauge_backed()
    test_unported_world_ready_claims_are_not_in_boot_label()
    test_label_telemetry_is_exposed()
    print("test-boot-progress-labels passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
