#!/usr/bin/env python3
# check-no-magic-numbers: allow-file -- release manifest fields are package metadata.
"""Create the shareable Mushroom Man ME3 zip from a staged mod payload."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path
from typing import Any

PACKAGE_NAME = "mushroom-man-me3-all-variants"
ZIP_ROOT = Path("mushroom-man") / "mod"
REQUIRED_HIDE_CATEGORIES = ["face", "hair", "eyelashes", "beards", "eyeballs"]
DEFAULT_SOURCE_WEIGHTS = Path(
    "target/mushroom-route-a-offline/blender-edit/adult-auto-rig-v10/export/blender_edit_c2280_weights.tsv"
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-mod", required=True, type=Path)
    parser.add_argument("--summary", required=True, type=Path)
    parser.add_argument("--audit", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--install-script", default=Path("scripts/install_mushroom_man.py"), type=Path
    )
    parser.add_argument(
        "--flver-summary",
        type=Path,
        help="generated FLVER patch summary; defaults to source-mod parent/fc_m_0000-lod-redirect-summary.txt and is required for model-guard enforcement",
    )
    parser.add_argument(
        "--staging-summary",
        type=Path,
        help="generated all-model-variant staging summary for build-time visual guard",
    )
    parser.add_argument(
        "--candidate-weights",
        type=Path,
        help="candidate remesh weights TSV for lower-arm/hand coverage guard",
    )
    parser.add_argument(
        "--source-weights",
        type=Path,
        default=DEFAULT_SOURCE_WEIGHTS,
        help="authored source weights TSV used as lower-arm/hand coverage baseline",
    )
    return parser.parse_args()


def require_file(path: Path, label: str) -> Path:
    resolved = path.resolve()
    if not resolved.is_file():
        raise FileNotFoundError(f"{label} is not a file: {resolved}")
    return resolved


def require_dir(path: Path, label: str) -> Path:
    resolved = path.resolve()
    if not resolved.is_dir():
        raise NotADirectoryError(f"{label} is not a directory: {resolved}")
    return resolved


def iter_files(root: Path) -> list[Path]:
    return sorted(path for path in root.rglob("*") if path.is_file())


def load_json(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise TypeError(f"expected object JSON in {path}")
    return data


def resolve_flver_summary(source_mod: Path, override: Path | None) -> Path:
    if override is not None:
        return require_file(override, "FLVER patch summary")
    candidate = source_mod.parent / "fc_m_0000-lod-redirect-summary.txt"
    if not candidate.is_file():
        raise FileNotFoundError(
            "FLVER patch summary is required for model-guard enforcement: "
            f"{candidate.resolve()}"
        )
    return candidate.resolve()


def run_model_guards(flver_summary: Path, connectivity_audit: Path) -> Path:
    guard_script = Path(__file__).with_name("route_a_mushroom_model_guard.py")
    require_file(guard_script, "model guard script")
    report_path = flver_summary.parent / "model-guard-report.json"
    result = subprocess.run(
        [
            sys.executable,
            str(guard_script),
            "--flver-summary",
            str(flver_summary),
            "--connectivity-audit",
            str(connectivity_audit),
            "--output",
            str(report_path),
        ],
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(f"model guard failed; see {report_path}")
    return report_path


def resolve_staging_summary(source_mod: Path, override: Path | None) -> Path:
    if override is not None:
        return require_file(override, "all-model-variant staging summary")
    return require_file(
        source_mod.parent / "model-variant-staging-summary.txt",
        "all-model-variant staging summary",
    )


def resolve_candidate_weights(source_mod: Path, override: Path | None) -> Path:
    if override is not None:
        return require_file(override, "candidate remesh weights")
    return require_file(
        source_mod.parent / "closed_remesh_weights.tsv", "candidate remesh weights"
    )


def run_build_time_guard(
    source_mod: Path,
    staging_summary: Path,
    candidate_weights: Path,
    source_weights: Path,
    flver_guard: Path,
) -> Path:
    script = Path(__file__).with_name("mushroom_man_build_time_guard.py")
    report_path = source_mod.parent / "mushroom-build-time-guard-report.json"
    result = subprocess.run(
        [
            sys.executable,
            str(script),
            "--source-mod",
            str(source_mod),
            "--staging-summary",
            str(staging_summary),
            "--candidate-weights",
            str(candidate_weights),
            "--source-weights",
            str(source_weights),
            "--flver-guard",
            str(flver_guard),
            "--output",
            str(report_path),
        ],
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(
            f"Mushroom Man build-time visual guard failed; see {report_path}"
        )
    return report_path


def write_readme(path: Path, summary: dict[str, Any]) -> None:
    export_vertices = summary.get("export_vertex_count", "unknown")
    path.write_text(
        "# Mushroom Man ME3 package\n\n"
        "This package installs an Elden Ring/ME3 Mushroom Man profile.\n\n"
        "Shipping runtime model:\n"
        "- `mushroom-man/mod/mushroom_man.dll` is loaded as an ME3 `[[natives]]` entry.\n"
        "- The DLL patches loaded `EquipParamProtector` visual model fields and protector hide masks in memory at runtime.\n"
        "- Required runtime hide coverage categories are declared in `package-manifest.json`: face, hair, eyelashes, beards, and eyeballs.\n"
        "- The package intentionally does not ship a static `regulation.bin`; it preserves the user's/mod stack's regulation data and applies only the visual override in process.\n"
        "- Weapons, scabbards, quivers, shields, staffs, seals, and other equipment attachments are not hidden by this package.\n"
        "- Adult c2270 mushroom geometry is placed in the visible FC donor mesh 13.\n"
        f"- Accepted Blender export has `{export_vertices}` exported vertices.\n"
        "- Mushroom cap/head is bound to upper spine instead of the human Head bone.\n"
        "- Procedural source-topology-smoothed limb weights are applied for arms, legs, and feet.\n\n"
        "Install:\n"
        "```powershell\n"
        "python install_mushroom_man.py --force\n"
        "```\n\n"
        "The generated launcher uses ME3 offline mode (`--online false`).\n",
        encoding="utf-8",
    )


def build_manifest(
    source_mod: Path, summary: dict[str, Any], audit: dict[str, Any]
) -> dict[str, Any]:
    files = []
    for path in iter_files(source_mod):
        relative = path.relative_to(source_mod).as_posix()
        files.append({"path": str(ZIP_ROOT / relative), "bytes": path.stat().st_size})
    return {
        "package": PACKAGE_NAME,
        "runtime": "mushroom_man.dll",
        "uses_static_regulation_bin": False,
        "preserves_weapon_scabbard_quiver_visuals": True,
        "mushroom_man_hide_coverage": {
            "required_categories": REQUIRED_HIDE_CATEGORIES,
            "verified_categories": REQUIRED_HIDE_CATEGORIES,
            "source": "mushroom_man.dll runtime EquipParamProtector face/hair/eyelash/beard/eyeball hide coverage oracle",
        },
        "donor_mesh_index": 13,
        "object": summary.get("object"),
        "export_vertex_count": summary.get("export_vertex_count"),
        "decimation": summary.get("decimation"),
        "procedural_weight_counts": summary.get("procedural_weight_counts"),
        "weight_target_counts": summary.get("weight_target_counts"),
        "connectivity_audit": {
            "isolated_vertices": len(audit.get("isolated_vertices", [])),
            "mesh_component_sizes": [
                component.get("size") for component in audit.get("mesh_components", [])
            ],
            "disconnected_weight_groups": audit.get("disconnected_weight_groups", []),
        },
        "source_mod": str(source_mod),
        "file_count": len(files),
        "files": files,
    }


def zip_directory(staging_root: Path, output: Path) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    temp_output = output.with_suffix(output.suffix + ".tmp")
    if temp_output.exists():
        temp_output.unlink()
    with zipfile.ZipFile(
        temp_output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as zip_file:
        for path in iter_files(staging_root):
            zip_file.write(path, path.relative_to(staging_root).as_posix())
    temp_output.replace(output)


def main() -> int:
    args = parse_args()
    source_mod = require_dir(args.source_mod, "source mod")
    summary = load_json(require_file(args.summary, "export summary"))
    audit_path = require_file(args.audit, "connectivity audit")
    audit = load_json(audit_path)
    install_script = require_file(args.install_script, "installer")
    flver_summary = resolve_flver_summary(source_mod, args.flver_summary)
    flver_guard = run_model_guards(flver_summary, audit_path)
    staging_summary = resolve_staging_summary(source_mod, args.staging_summary)
    candidate_weights = resolve_candidate_weights(source_mod, args.candidate_weights)
    source_weights = require_file(args.source_weights, "source arm coverage weights")
    run_build_time_guard(
        source_mod,
        staging_summary,
        candidate_weights,
        source_weights,
        flver_guard,
    )
    with tempfile.TemporaryDirectory(prefix="mushroom-man-release-") as temp_dir:
        staging = Path(temp_dir)
        shutil.copytree(source_mod, staging / ZIP_ROOT)
        shutil.copy2(install_script, staging / "install_mushroom_man.py")
        write_readme(staging / "README.txt", summary)
        manifest = build_manifest(source_mod, summary, audit)
        (staging / "package-manifest.json").write_text(
            json.dumps(manifest, indent=2), encoding="utf-8"
        )
        zip_directory(staging, args.output.resolve())
    print(args.output.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
