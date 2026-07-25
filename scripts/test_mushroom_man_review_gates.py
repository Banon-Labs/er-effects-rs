#!/usr/bin/env python3
"""Review-gate tests for Mushroom Man replacement candidates.

These are intentionally artifact-level tests because the user-visible failures
showed up only after package/runtime review. They do not launch Elden Ring.
"""

from __future__ import annotations

import json
import zipfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CANDIDATE_ROOT = REPO_ROOT / "target/mushroom-route-a-offline/blender-edit/adult-closed-remesh-v12"
CANDIDATE_OBJ = CANDIDATE_ROOT / "closed_remesh.obj"
CANDIDATE_FLVER_GUARD = CANDIDATE_ROOT / "flver/model-guard-report.json"
CANDIDATE_PROFILE = (
    REPO_ROOT
    / "target/mushroom-replacement-lod-redirect-spine-arm-closed-remesh-v12-install/mushroom-man.me3"
)
CANDIDATE_ZIP = REPO_ROOT / "target/mushroom-man-replacement-lod-redirect-spine-arm-closed-remesh-v12.zip"
RUNTIME_SOURCE = REPO_ROOT / "crates/mushroom-man-runtime/src/lib.rs"
REQUIRED_HIDE_CATEGORIES = {"face", "hair", "eyelashes", "beards", "eyeballs"}
REQUIRED_ORACLE_MARKERS = {
    "world_character_saveload_readiness",
    "patch_missing_at_readiness_teardown",
    "PATCH_APPLIED",
}
MIN_OUTWARD_NORMAL_DOT_RATIO = 0.98


def load_obj(path: Path) -> tuple[list[tuple[float, float, float]], list[tuple[float, float, float]], list[list[tuple[int, int]]]]:
    vertices: list[tuple[float, float, float]] = []
    normals: list[tuple[float, float, float]] = []
    faces: list[list[tuple[int, int]]] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("v "):
            _, x, y, z = line.split()[:4]
            vertices.append((float(x), float(y), float(z)))
        elif line.startswith("vn "):
            _, x, y, z = line.split()[:4]
            normals.append((float(x), float(y), float(z)))
        elif line.startswith("f "):
            face: list[tuple[int, int]] = []
            for part in line.split()[1:4]:
                pieces = part.split("/")
                vertex_index = int(pieces[0]) - 1
                normal_index = int(pieces[2]) - 1 if len(pieces) > 2 and pieces[2] else vertex_index
                face.append((vertex_index, normal_index))
            faces.append(face)
    return vertices, normals, faces


def dot(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def sub(a: tuple[float, float, float], b: tuple[float, float, float]) -> tuple[float, float, float]:
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def centroid(vertices: list[tuple[float, float, float]]) -> tuple[float, float, float]:
    return (
        sum(vertex[0] for vertex in vertices) / len(vertices),
        sum(vertex[1] for vertex in vertices) / len(vertices),
        sum(vertex[2] for vertex in vertices) / len(vertices),
    )


def test_closed_topology_has_zero_tear_proxies() -> None:
    report = json.loads(CANDIDATE_FLVER_GUARD.read_text(encoding="utf-8"))
    summary = report["summary"]
    expected = {
        "topology_triangle_components": 1,
        "topology_boundary_edges": 0,
        "topology_boundary_components": 0,
        "topology_nonmanifold_edges": 0,
        "topology_degenerate_faces": 0,
    }
    for key, value in expected.items():
        assert summary[key] == value, f"{key}: expected {value}, got {summary[key]}"
    assert report["status"] == "pass"
    assert report["alerts"] == []


def test_normals_face_outward_for_review_candidate() -> None:
    vertices, normals, faces = load_obj(CANDIDATE_OBJ)
    assert vertices, "candidate OBJ has no vertices"
    assert len(normals) == len(vertices), "candidate OBJ must include one normal per vertex"
    center = centroid(vertices)
    outward = 0
    total = 0
    for face in faces:
        for vertex_index, normal_index in face:
            total += 1
            if dot(sub(vertices[vertex_index], center), normals[normal_index]) > 0:
                outward += 1
    ratio = outward / max(total, 1)
    assert ratio >= MIN_OUTWARD_NORMAL_DOT_RATIO, (
        f"outward normal ratio {ratio:.3f} is below {MIN_OUTWARD_NORMAL_DOT_RATIO:.3f}; "
        "review packages must not show inside-out texture surfaces"
    )


def test_package_and_profile_include_mushroom_runtime_dll() -> None:
    profile_text = CANDIDATE_PROFILE.read_text(encoding="utf-8")
    assert "[[natives]]" in profile_text
    assert "mushroom_man.dll" in profile_text
    with zipfile.ZipFile(CANDIDATE_ZIP) as package:
        names = package.namelist()
        assert any(name.endswith("/mushroom_man.dll") for name in names)
        manifest = json.loads(package.read("package-manifest.json"))
    manifest_files = manifest["files"]
    assert manifest["file_count"] == len(manifest_files)
    assert any(file["path"].endswith("/mushroom_man.dll") for file in manifest_files)


def test_package_manifest_declares_required_hide_coverage_categories() -> None:
    with zipfile.ZipFile(CANDIDATE_ZIP) as package:
        manifest = json.loads(package.read("package-manifest.json"))
    coverage = manifest.get("mushroom_man_hide_coverage", {})
    categories = set(coverage.get("required_categories", []))
    assert categories >= REQUIRED_HIDE_CATEGORIES, (
        "package manifest must explicitly prove hide coverage for "
        f"{sorted(REQUIRED_HIDE_CATEGORIES)}; got {sorted(categories)}"
    )
    assert set(coverage.get("verified_categories", [])) >= REQUIRED_HIDE_CATEGORIES


def test_runtime_has_fail_closed_readiness_oracle_for_missing_patch() -> None:
    source = RUNTIME_SOURCE.read_text(encoding="utf-8")
    missing = sorted(marker for marker in REQUIRED_ORACLE_MARKERS if marker not in source)
    assert not missing, (
        "runtime must expose an oracle that tears down when world+character+saveload "
        f"readiness is reached before the hide patch applies; missing markers: {missing}"
    )


def main() -> int:
    tests = [
        test_closed_topology_has_zero_tear_proxies,
        test_normals_face_outward_for_review_candidate,
        test_package_and_profile_include_mushroom_runtime_dll,
        test_package_manifest_declares_required_hide_coverage_categories,
        test_runtime_has_fail_closed_readiness_oracle_for_missing_patch,
    ]
    failures: list[str] = []
    for test in tests:
        try:
            test()
        except Exception as exc:  # noqa: BLE001 - standalone script prints all failures.
            failures.append(test.__name__)
            print(f"[FAIL] {test.__name__}: {exc}")
        else:
            print(f"[ok] {test.__name__}")
    if failures:
        print(f"\nFAILED {len(failures)}/{len(tests)}: {failures}")
        return 1
    print(f"\nAll {len(tests)} Mushroom Man review gates passed.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
