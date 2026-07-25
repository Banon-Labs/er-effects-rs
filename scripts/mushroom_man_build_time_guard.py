#!/usr/bin/env python3
"""Build-time visual-symptom guard for Mushroom Man review packages.

This script converts user-visible failures into static checks before packaging:

- visible face/hair/eyes => dictionary-wide FG_A face aliases must be staged,
  not just a manifest declaration;
- black arm holes => the closed-remesh candidate must preserve lower-arm/hand
  weighted surface coverage relative to the authored source.

It does not launch Elden Ring and does not inspect screenshots.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
from pathlib import Path
from typing import Any

MIN_ARM_COVERAGE_RATIO = 0.50
ARM_WEIGHT_THRESHOLD = 0.12
REQUIRED_HIDE_CATEGORIES = {"face", "hair", "eyelashes", "beards", "eyeballs"}
LOWER_ARM_BONES: tuple[str, ...] = ("L_Forearm", "R_Forearm", "L_Hand", "R_Hand")
PART_EXT = ".partsbnd.dcx"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-mod", required=True, type=Path)
    parser.add_argument("--staging-summary", required=True, type=Path)
    parser.add_argument("--candidate-weights", required=True, type=Path)
    parser.add_argument("--source-weights", required=True, type=Path)
    parser.add_argument("--dictionary", type=Path)
    parser.add_argument("--flver-guard", type=Path)
    parser.add_argument("--output", type=Path)
    return parser.parse_args()


def load_key_value_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            values[key.strip()] = value.strip()
    return values


def find_dictionary(explicit: Path | None, repo_root: Path) -> Path:
    candidates: list[Path] = []
    if explicit is not None:
        candidates.append(explicit)
    env_path = os.environ.get("ER_FILE_DICTIONARY_JSON")
    if env_path:
        candidates.append(Path(env_path))
    candidates.extend(
        [
            repo_root
            / ".deps/Smithbox/Assets/File Dictionaries/ER-File-Dictionary.json",
            repo_root / "../Smithbox/Assets/File Dictionaries/ER-File-Dictionary.json",
            repo_root / "../smithbox/Assets/File Dictionaries/ER-File-Dictionary.json",
            Path("/mnt/d/Smithbox/Assets/File Dictionaries/ER-File-Dictionary.json"),
        ]
    )
    for candidate in candidates:
        if candidate.is_file():
            return candidate
    raise FileNotFoundError(
        "could not find ER-File-Dictionary.json for FG_A coverage guard"
    )


def dictionary_fg_names(dictionary_path: Path) -> set[str]:
    data = json.loads(dictionary_path.read_text(encoding="utf-8"))
    names: set[str] = set()
    for entry in data.get("Entries", []):
        if not isinstance(entry, dict):
            continue
        path = str(entry.get("Path", "")).lower()
        filename = str(entry.get("Filename", "")).lower()
        if (
            path.startswith("/parts/")
            and path.endswith(PART_EXT)
            and filename.startswith("fg_a_")
        ):
            names.add(Path(path).name)
    if not names:
        raise ValueError(f"dictionary has no FG_A part entries: {dictionary_path}")
    return names


def load_weight_counts(path: Path) -> dict[str, int]:
    counts: dict[str, int] = {}
    seen_by_bone: dict[str, set[int]] = {}
    for bone_name in LOWER_ARM_BONES:
        bone = str(bone_name)
        counts[bone] = 0
        seen_by_bone[bone] = set()
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        for row in reader:
            bone = row.get("er_target_bone", "")
            if bone not in seen_by_bone:
                continue
            if float(row.get("weight", "0")) >= ARM_WEIGHT_THRESHOLD:
                seen_by_bone[bone].add(int(row["vertex"]))
    for bone, vertices in seen_by_bone.items():
        counts[bone] = len(vertices)
    return counts


def build_report(args: argparse.Namespace) -> dict[str, Any]:
    repo_root = Path.cwd()
    source_mod = args.source_mod
    parts_dir = source_mod / "parts"
    staging_summary = load_key_value_file(args.staging_summary)
    dictionary = find_dictionary(args.dictionary, repo_root)
    expected_fg = dictionary_fg_names(dictionary)
    actual_fg = {path.name.lower() for path in parts_dir.glob("fg_a_*.partsbnd.dcx")}
    candidate_counts = load_weight_counts(args.candidate_weights)
    source_counts = load_weight_counts(args.source_weights)
    flver_guard_status = "unknown"
    if args.flver_guard and args.flver_guard.exists():
        flver_guard_status = json.loads(
            args.flver_guard.read_text(encoding="utf-8")
        ).get("status", "unknown")

    alerts: list[dict[str, Any]] = []
    fg_mode = staging_summary.get("fg_alias_mode", "")
    staged_fg_count = int(staging_summary.get("fg_hidden_face_files", "0"))
    if (
        fg_mode != "dictionary"
        or staged_fg_count < len(expected_fg)
        or not expected_fg <= actual_fg
    ):
        alerts.append(
            {
                "code": "MUSHROOM_HIDE_FG_ALIAS_COVERAGE_INCOMPLETE",
                "severity": "error",
                "message": "Visible face/hair/eye guard requires dictionary-wide FG_A hidden aliases staged before packaging.",
                "required_categories": sorted(REQUIRED_HIDE_CATEGORIES),
                "fg_alias_mode": fg_mode,
                "staged_fg_count": staged_fg_count,
                "expected_fg_count": len(expected_fg),
                "actual_fg_count": len(actual_fg),
                "missing_fg_count": len(expected_fg - actual_fg),
            }
        )

    coverage: dict[str, dict[str, float]] = {}
    for bone in LOWER_ARM_BONES:
        source_count = source_counts[bone]
        candidate_count = candidate_counts[bone]
        ratio = candidate_count / max(source_count, 1)
        coverage[bone] = {
            "source_count": float(source_count),
            "candidate_count": float(candidate_count),
            "ratio": ratio,
        }
        if ratio < MIN_ARM_COVERAGE_RATIO:
            alerts.append(
                {
                    "code": "MUSHROOM_LOWER_ARM_SURFACE_COVERAGE_LOW",
                    "severity": "error",
                    "message": "Visible black arm-hole guard requires lower-arm/hand surface coverage to be preserved before packaging.",
                    "bone": bone,
                    "candidate_count": candidate_count,
                    "source_count": source_count,
                    "ratio": ratio,
                    "minimum_ratio": MIN_ARM_COVERAGE_RATIO,
                    "weight_threshold": ARM_WEIGHT_THRESHOLD,
                }
            )

    return {
        "status": "pass" if not alerts else "fail",
        "alerts": alerts,
        "summary": {
            "source_mod": str(source_mod),
            "staging_summary": str(args.staging_summary),
            "candidate_weights": str(args.candidate_weights),
            "source_weights": str(args.source_weights),
            "dictionary": str(dictionary),
            "flver_guard_status": flver_guard_status,
            "fg_alias_mode": fg_mode,
            "staged_fg_count": staged_fg_count,
            "expected_fg_count": len(expected_fg),
            "actual_fg_count": len(actual_fg),
            "lower_arm_surface_coverage": coverage,
        },
    }


def main() -> int:
    args = parse_args()
    report = build_report(args)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
        print(f"wrote {args.output}")
    print(
        f"mushroom_build_time_guard_status={report['status']} alerts={len(report['alerts'])}"
    )
    for alert in report["alerts"]:
        print(f"ALERT {alert['code']}: {alert['message']}")
    return 0 if report["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
