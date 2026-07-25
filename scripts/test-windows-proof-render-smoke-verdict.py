#!/usr/bin/env python3
"""Regression tests for the Windows-proof renderer smoke verdict contract."""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
VERDICT_PATH = REPO_ROOT / "scripts" / "windows-proof-render-smoke-verdict.py"


def load_verdict_module():
    spec = importlib.util.spec_from_file_location("windows_proof_render_smoke_verdict", VERDICT_PATH)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def good_telemetry() -> dict[str, Any]:
    return {
        "oracle_windows_proof_mode": 1,
        "oracle_forbidden_render_backend_hits": 0,
        "oracle_native_overlay_frames": 12,
        "oracle_native_overlay_stage": 10,
        "oracle_native_overlay_failure": 0,
        "oracle_native_overlay_handoff_ready_hits": 2,
        "oracle_native_overlay_covering_loading_hits": 2,
        "oracle_native_overlay_show": 0,
        "oracle_native_overlay_content_frames": 2,
        "oracle_native_overlay_pixel_probe_matches": 1,
        "oracle_native_overlay_pixel_probe_rgba": 0x2EB8EDFF,
        "oracle_scaleform_memoryfile_custom_asset_hits": 0,
    }


def verdict(
    telemetry: dict[str, Any],
    *,
    require_world_ready: bool = True,
    watcher_status: int = 0,
) -> dict[str, Any]:
    module = load_verdict_module()
    return module.build_verdict(
        artifact_dir=Path("/tmp/artifact"),
        telemetry=telemetry,
        telemetry_written=True,
        watcher_status=watcher_status,
        require_world_ready=require_world_ready,
    )


def assert_rejects_missing(key: str) -> None:
    telemetry = good_telemetry()
    telemetry[key] = 0
    got = verdict(telemetry)
    assert not got["windows_proof_render_runtime"], key


def test_require_world_ready_accepts_full_positive_contract() -> None:
    got = verdict(good_telemetry())
    assert got["watcher_pass"] is True
    assert got["native_overlay_proven"] is True
    assert got["native_overlay_handoff_observed"] is True
    assert got["native_overlay_visible_during_loading"] is True
    assert got["native_overlay_full_content_proven"] is True
    assert got["native_overlay_covered_loading"] is True
    assert got["native_overlay_hidden_at_world_ready"] is True
    assert got["scaleform_memoryfile_custom_asset_observed"] is False
    assert got["windows_proof_render_runtime"] is True


def test_rejects_forbidden_backend_hit() -> None:
    telemetry = good_telemetry()
    telemetry["oracle_forbidden_render_backend_hits"] = 1
    assert not verdict(telemetry)["windows_proof_render_runtime"]


def test_rejects_missing_bridge_pixel_match() -> None:
    assert_rejects_missing("oracle_native_overlay_pixel_probe_matches")


def test_rejects_missing_loading_visibility() -> None:
    assert_rejects_missing("oracle_native_overlay_covering_loading_hits")


def test_rejects_missing_full_content_proof() -> None:
    assert_rejects_missing("oracle_native_overlay_content_frames")


def test_rejects_bridge_still_visible_at_world_ready() -> None:
    telemetry = good_telemetry()
    telemetry["oracle_native_overlay_show"] = 1
    got = verdict(telemetry)
    assert got["native_overlay_hidden_at_world_ready"] is False
    assert not got["windows_proof_render_runtime"]


def test_does_not_require_scaleform_memoryfile_asset_commit() -> None:
    telemetry = good_telemetry()
    telemetry["oracle_scaleform_memoryfile_custom_asset_hits"] = 0
    got = verdict(telemetry)
    assert got["scaleform_memoryfile_custom_asset_observed"] is False
    assert got["windows_proof_render_runtime"] is True


def test_rejects_old_gfx_handoff_semantics() -> None:
    telemetry = good_telemetry()
    telemetry["oracle_native_overlay_handoff_ready_hits"] = 2
    telemetry["oracle_native_overlay_covering_loading_hits"] = 0
    telemetry["oracle_native_overlay_show"] = 0
    got = verdict(telemetry)
    assert got["native_overlay_handoff_observed"] is True
    assert got["native_overlay_visible_during_loading"] is False
    assert got["native_overlay_covered_loading"] is False
    assert not got["windows_proof_render_runtime"]


def test_short_mode_does_not_require_world_ready_specific_oracles() -> None:
    telemetry = good_telemetry()
    telemetry["oracle_native_overlay_covering_loading_hits"] = 0
    telemetry["oracle_native_overlay_show"] = 1
    telemetry["oracle_scaleform_memoryfile_custom_asset_hits"] = 0
    got = verdict(telemetry, require_world_ready=False)
    assert got["windows_proof_render_runtime"] is True


def test_require_handoff_alias_uses_world_stable_target() -> None:
    script = REPO_ROOT / "scripts" / "run-windows-proof-render-smoke.sh"
    for flag in ["--require-world-ready", "--require-handoff"]:
        with tempfile.TemporaryDirectory() as tmp:
            artifact = Path(tmp) / "artifact"
            subprocess.run(
                ["bash", str(script), "--dry-run", flag],
                check=True,
                cwd=REPO_ROOT,
                env={**os.environ, "ARTIFACT_DIR": str(artifact)},
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            summary = json.loads((artifact / "dry-run-summary.json").read_text(encoding="utf-8"))
            assert summary["watch_target"] == "world-stable", flag
            assert summary["require_world_ready"] == 1, flag


def test_native_overlay_handoff_target_is_not_registered() -> None:
    watcher = (REPO_ROOT / "scripts" / "er-readiness-watch.py").read_text(encoding="utf-8")
    assert "native-overlay-handoff" not in watcher


def main() -> int:
    tests = [
        test_require_world_ready_accepts_full_positive_contract,
        test_rejects_forbidden_backend_hit,
        test_rejects_missing_bridge_pixel_match,
        test_rejects_missing_loading_visibility,
        test_rejects_missing_full_content_proof,
        test_rejects_bridge_still_visible_at_world_ready,
        test_does_not_require_scaleform_memoryfile_asset_commit,
        test_rejects_old_gfx_handoff_semantics,
        test_short_mode_does_not_require_world_ready_specific_oracles,
        test_require_handoff_alias_uses_world_stable_target,
        test_native_overlay_handoff_target_is_not_registered,
    ]
    for test in tests:
        test()
    print("test-windows-proof-render-smoke-verdict passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
