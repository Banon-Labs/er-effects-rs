#!/usr/bin/env python3
"""Build the Windows-proof renderer smoke verdict from runtime telemetry."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


def as_int(value: Any, default: int = 0) -> int:
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value, 0)
        except ValueError:
            return default
    return default


def build_verdict(
    *,
    artifact_dir: Path,
    telemetry: dict[str, Any] | None,
    telemetry_written: bool,
    watcher_status: int,
    require_world_ready: bool,
) -> dict[str, Any]:
    mode = isinstance(telemetry, dict) and as_int(telemetry.get("oracle_windows_proof_mode"), 0) == 1
    hits = as_int(
        telemetry.get("oracle_forbidden_render_backend_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    winreconfig_change_display_suppressed = as_int(
        telemetry.get("oracle_winreconfig_change_display_suppressed") if isinstance(telemetry, dict) else None,
        0,
    )
    native_overlay_frames = as_int(
        telemetry.get("oracle_native_overlay_frames") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_stage = as_int(
        telemetry.get("oracle_native_overlay_stage") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_failure = as_int(
        telemetry.get("oracle_native_overlay_failure") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_handoff_ready_hits = as_int(
        telemetry.get("oracle_native_overlay_handoff_ready_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_covering_loading_hits = as_int(
        telemetry.get("oracle_native_overlay_covering_loading_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_show = as_int(
        telemetry.get("oracle_native_overlay_show") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_content_frames = as_int(
        telemetry.get("oracle_native_overlay_content_frames") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_bar_pixel_frames = as_int(
        telemetry.get("oracle_native_overlay_bar_pixel_frames") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_bar_pixel_missing_frames = as_int(
        telemetry.get("oracle_native_overlay_bar_pixel_missing_frames") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_bar_pixel_last_count = as_int(
        telemetry.get("oracle_native_overlay_bar_pixel_last_count") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_zorder_lift_hits = as_int(
        telemetry.get("oracle_native_overlay_zorder_lift_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_present_ok_hits = as_int(
        telemetry.get("oracle_native_overlay_present_ok_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_present_fail_hits = as_int(
        telemetry.get("oracle_native_overlay_present_fail_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_is_window = as_int(
        telemetry.get("oracle_native_overlay_child_is_window") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_is_visible = as_int(
        telemetry.get("oracle_native_overlay_child_is_visible") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_window = as_int(
        telemetry.get("oracle_native_overlay_child_window") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_parent_match = as_int(
        telemetry.get("oracle_native_overlay_child_parent_match") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_client_match = as_int(
        telemetry.get("oracle_native_overlay_child_client_match") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_cover_match = as_int(
        telemetry.get("oracle_native_overlay_child_cover_match") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_geometry_mismatch_hits = as_int(
        telemetry.get("oracle_native_overlay_child_geometry_mismatch_hits")
        if isinstance(telemetry, dict)
        else None,
        -1,
    )
    native_overlay_parent_hwnd = as_int(
        telemetry.get("oracle_native_overlay_parent_hwnd") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_pixel_probe_matches = as_int(
        telemetry.get("oracle_native_overlay_pixel_probe_matches") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_pixel_probe_rgba = as_int(
        telemetry.get("oracle_native_overlay_pixel_probe_rgba") if isinstance(telemetry, dict) else None,
        -1,
    )
    scaleform_memoryfile_custom_asset_hits = as_int(
        telemetry.get("oracle_scaleform_memoryfile_custom_asset_hits") if isinstance(telemetry, dict) else None,
        -1,
    )
    native_overlay_child_attached = (
        native_overlay_child_window == 1
        and native_overlay_parent_hwnd > 0
        and native_overlay_child_parent_match == 1
        and native_overlay_child_cover_match == 1
    )
    native_overlay_proven = native_overlay_frames > 0 and native_overlay_pixel_probe_matches > 0
    native_overlay_handoff_observed = native_overlay_handoff_ready_hits > 0
    native_overlay_visible_during_loading = native_overlay_covering_loading_hits > 0
    native_overlay_full_content_proven = native_overlay_content_frames > 0
    native_overlay_bar_pixels_proven = native_overlay_bar_pixel_frames > 0 and native_overlay_bar_pixel_missing_frames == 0
    native_overlay_zorder_proven = native_overlay_zorder_lift_hits > 0
    native_overlay_present_proven = native_overlay_present_ok_hits > 0 and native_overlay_present_fail_hits == 0
    native_overlay_window_live = native_overlay_child_is_window == 1 and native_overlay_child_is_visible == 1
    native_overlay_covered_loading = (
        native_overlay_visible_during_loading and native_overlay_full_content_proven and native_overlay_bar_pixels_proven
    )
    native_overlay_hidden_at_world_ready = native_overlay_show == 0
    scaleform_memoryfile_custom_asset_observed = scaleform_memoryfile_custom_asset_hits > 0
    windows_proof_render_runtime = (
        mode
        and hits == 0
        and native_overlay_child_attached
        and native_overlay_proven
        and native_overlay_zorder_proven
        and native_overlay_present_proven
        and native_overlay_window_live
        and (
            not require_world_ready
            or (
                native_overlay_visible_during_loading
                and native_overlay_covered_loading
                and native_overlay_hidden_at_world_ready
            )
        )
    )
    return {
        "artifact_dir": str(artifact_dir),
        "watcher_status": watcher_status,
        "watcher_pass": watcher_status == 0,
        "telemetry_written": telemetry_written,
        "oracle_windows_proof_mode": 1 if mode else 0,
        "oracle_forbidden_render_backend_hits": hits,
        "oracle_winreconfig_change_display_suppressed": winreconfig_change_display_suppressed,
        "oracle_native_overlay_frames": native_overlay_frames,
        "oracle_native_overlay_stage": native_overlay_stage,
        "oracle_native_overlay_failure": native_overlay_failure,
        "oracle_native_overlay_handoff_ready_hits": native_overlay_handoff_ready_hits,
        "oracle_native_overlay_covering_loading_hits": native_overlay_covering_loading_hits,
        "oracle_native_overlay_show": native_overlay_show,
        "oracle_native_overlay_content_frames": native_overlay_content_frames,
        "oracle_native_overlay_bar_pixel_frames": native_overlay_bar_pixel_frames,
        "oracle_native_overlay_bar_pixel_missing_frames": native_overlay_bar_pixel_missing_frames,
        "oracle_native_overlay_bar_pixel_last_count": native_overlay_bar_pixel_last_count,
        "oracle_native_overlay_zorder_lift_hits": native_overlay_zorder_lift_hits,
        "oracle_native_overlay_present_ok_hits": native_overlay_present_ok_hits,
        "oracle_native_overlay_present_fail_hits": native_overlay_present_fail_hits,
        "oracle_native_overlay_child_is_window": native_overlay_child_is_window,
        "oracle_native_overlay_child_is_visible": native_overlay_child_is_visible,
        "oracle_native_overlay_child_window": native_overlay_child_window,
        "oracle_native_overlay_child_parent_match": native_overlay_child_parent_match,
        "oracle_native_overlay_child_client_match": native_overlay_child_client_match,
        "oracle_native_overlay_child_cover_match": native_overlay_child_cover_match,
        "oracle_native_overlay_child_geometry_mismatch_hits": native_overlay_child_geometry_mismatch_hits,
        "oracle_native_overlay_parent_hwnd": native_overlay_parent_hwnd,
        "oracle_native_overlay_pixel_probe_matches": native_overlay_pixel_probe_matches,
        "oracle_native_overlay_pixel_probe_rgba": native_overlay_pixel_probe_rgba,
        "oracle_scaleform_memoryfile_custom_asset_hits": scaleform_memoryfile_custom_asset_hits,
        "native_overlay_child_attached": native_overlay_child_attached,
        "native_overlay_proven": native_overlay_proven,
        "native_overlay_handoff_observed": native_overlay_handoff_observed,
        "native_overlay_visible_during_loading": native_overlay_visible_during_loading,
        "native_overlay_full_content_proven": native_overlay_full_content_proven,
        "native_overlay_bar_pixels_proven": native_overlay_bar_pixels_proven,
        "native_overlay_zorder_proven": native_overlay_zorder_proven,
        "native_overlay_present_proven": native_overlay_present_proven,
        "native_overlay_window_live": native_overlay_window_live,
        "native_overlay_covered_loading": native_overlay_covered_loading,
        "native_overlay_hidden_at_world_ready": native_overlay_hidden_at_world_ready,
        "scaleform_memoryfile_custom_asset_observed": scaleform_memoryfile_custom_asset_observed,
        "require_world_ready": require_world_ready,
        "windows_proof_render_runtime": windows_proof_render_runtime,
    }


def load_telemetry(path: Path) -> dict[str, Any] | None:
    try:
        telemetry = json.loads(path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        return None
    return telemetry if isinstance(telemetry, dict) else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-dir", type=Path, required=True)
    parser.add_argument("--telemetry", type=Path, required=True)
    parser.add_argument("--verdict", type=Path, required=True)
    parser.add_argument("--watcher-status", type=int, required=True)
    parser.add_argument("--require-world-ready", action="store_true")
    parser.add_argument(
        "--require-handoff",
        action="store_true",
        help="Compatibility alias for --require-world-ready; GFx handoff is not product cover.",
    )
    args = parser.parse_args()

    telemetry = load_telemetry(args.telemetry)
    verdict = build_verdict(
        artifact_dir=args.artifact_dir,
        telemetry=telemetry,
        telemetry_written=args.telemetry.is_file(),
        watcher_status=args.watcher_status,
        require_world_ready=args.require_world_ready or args.require_handoff,
    )
    args.verdict.write_text(json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print("windows-proof-render-smoke:", json.dumps(verdict, sort_keys=True))
    return 0 if verdict["watcher_pass"] and verdict["windows_proof_render_runtime"] else 3


if __name__ == "__main__":
    raise SystemExit(main())
