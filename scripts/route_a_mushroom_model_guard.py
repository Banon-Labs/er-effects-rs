#!/usr/bin/env python3
"""Fail-closed model guards for Mushroom Man production artifacts.

This guard catches model-generation states that are known bad from runtime review,
especially the arm-v1 failure where disconnected hand/forearm islands were treated
as independent shoulder-to-hand arms.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any

ARM_WEIGHT_GROUPS = {
    "L_UpperArm",
    "R_UpperArm",
    "L_Forearm",
    "R_Forearm",
    "L_Hand",
    "R_Hand",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--flver-summary", type=Path, required=True)
    parser.add_argument("--connectivity-audit", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def parse_bool(value: str | None) -> bool:
    return value is not None and value.strip().lower() == "true"


def parse_int(value: str | None) -> int:
    if value is None or not value.strip():
        return 0
    return int(value.strip())


def parse_int_pair(value: str | None) -> tuple[int, int]:
    if value is None or not value.strip():
        return (0, 0)
    left, right = value.split(",", 1)
    return (int(left), int(right))


def parse_summary(path: Path) -> dict[str, str]:
    fields: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        fields[key.strip()] = value.strip()
    return fields


def load_json_object(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise TypeError(f"expected object JSON in {path}")
    return data


def disconnected_arm_weight_groups(audit: dict[str, Any]) -> list[str]:
    groups = audit.get("disconnected_weight_groups", [])
    if not isinstance(groups, list):
        return []
    return sorted(
        group
        for group in groups
        if isinstance(group, str) and group in ARM_WEIGHT_GROUPS
    )


def build_report(summary: dict[str, str], audit: dict[str, Any]) -> dict[str, Any]:
    alerts: list[dict[str, Any]] = []
    arm_enabled = parse_bool(summary.get("arm_compensation_enabled"))
    arm_vertices = parse_int(summary.get("arm_compensated_vertices"))
    left_components, right_components = parse_int_pair(
        summary.get("arm_components_left_right")
    )
    weak_before, weak_after = parse_int_pair(
        summary.get("arm_weak_shoulder_components_before_after")
    )
    distal_before, distal_after = parse_int_pair(
        summary.get("arm_distal_overweighted_components_before_after")
    )
    disconnected_groups = disconnected_arm_weight_groups(audit)
    detached_response = summary.get("arm_detached_island_response", "")
    independent_detached_components = parse_int(
        summary.get("arm_independent_detached_components")
    )
    detached_proxy_vertices = parse_int(summary.get("arm_detached_proxy_vertices"))
    detached_islands_handled = (
        detached_response == "body_proxy_low_hand"
        and independent_detached_components == 0
        and detached_proxy_vertices > 0
    )

    if (
        arm_enabled
        and arm_vertices > 0
        and (left_components > 1 or right_components > 1)
        and not detached_islands_handled
    ):
        alerts.append(
            {
                "code": "ARM_COMPONENT_DISCONNECTED",
                "severity": "error",
                "message": "Arm compensation was applied while arm-weighted geometry is split into multiple components per side.",
                "left_components": left_components,
                "right_components": right_components,
                "recommended_response": "do_not_package; use proxy body tweening or human geometry merge/split guidance before arm mutation",
            }
        )
    if arm_enabled and arm_vertices > 0 and weak_before > 0 and not detached_islands_handled:
        alerts.append(
            {
                "code": "ARM_HAND_ISLAND_WITH_WEAK_UPPER_ANCHOR",
                "severity": "error",
                "message": "Arm compensation attempted to fix weak shoulder/root anchors after generation; this is the failed arm-v1 pattern unless a detached-island response is explicitly implemented.",
                "weak_shoulder_components_before": weak_before,
                "weak_shoulder_components_after": weak_after,
                "recommended_response": "do_not_package; classify detached hand/forearm islands and avoid independent shoulder-to-hand gradients",
            }
        )
    if arm_enabled and arm_vertices > 0 and disconnected_groups and not detached_islands_handled:
        alerts.append(
            {
                "code": "ARM_WEIGHT_GROUPS_DISCONNECTED_IN_SOURCE",
                "severity": "error",
                "message": "Connectivity audit reports disconnected arm weight groups in the source mesh used for this arm-compensated build.",
                "groups": disconnected_groups,
                "recommended_response": "do_not_package; require arm-specific topology/proximity oracle before production",
            }
        )
    if (
        arm_enabled
        and arm_vertices > 0
        and distal_before > 0
        and distal_after == 0
        and weak_before > 0
        and not detached_islands_handled
    ):
        alerts.append(
            {
                "code": "ARM_DISTAL_OVERWEIGHTED_MASKED_BY_GLOBAL_REWRITE",
                "severity": "error",
                "message": "The generator erased distal/weak-arm metrics by rewriting weights, but source topology still contains disconnected components.",
                "distal_overweighted_components_before": distal_before,
                "distal_overweighted_components_after": distal_after,
                "recommended_response": "do_not_package; the oracle must prevent bad geometry from being masked by summary deltas",
            }
        )

    return {
        "status": "fail" if alerts else "pass",
        "summary": {
            "arm_compensation_enabled": arm_enabled,
            "arm_compensated_vertices": arm_vertices,
            "arm_components_left_right": [left_components, right_components],
            "arm_weak_shoulder_components_before_after": [weak_before, weak_after],
            "arm_distal_overweighted_components_before_after": [
                distal_before,
                distal_after,
            ],
            "disconnected_arm_weight_groups": disconnected_groups,
            "arm_detached_island_response": detached_response,
            "arm_independent_detached_components": independent_detached_components,
            "arm_detached_proxy_vertices": detached_proxy_vertices,
            "detached_islands_handled": detached_islands_handled,
        },
        "alerts": alerts,
    }


def main() -> int:
    args = parse_args()
    summary = parse_summary(args.flver_summary)
    audit = load_json_object(args.connectivity_audit)
    report = build_report(summary, audit)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"wrote {args.output}")
    print(f"model_guard_status={report['status']} alerts={len(report['alerts'])}")
    for alert in report["alerts"]:
        print(f"ALERT {alert['code']}: {alert['message']}")
    return 1 if report["alerts"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
