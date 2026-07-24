#!/usr/bin/env python3
"""Check human-authored Mushroom Man region maps for seam hazards.

This is a red-capable offline alert for the Blender-in-the-loop rigging workflow.
It catches the specific class of issue observed at the feet/leg/groin boundary:
a manual region split that visually follows a quad-like belt but cuts through the
actual triangulated mesh. Such a split creates mixed triangles and long boundary
edges across deformation ownership, which can shear during movement.

Recommended response when `REGION_SPLIT_CUTS_TRIANGLES` fires:
  1. Do not treat the human region as a hard mesh/weight boundary yet.
  2. Convert it to a face-closed region before rigging. Prefer expansion when
     shrink would discard most of the authored region; prefer shrink only when
     the authored region clearly includes too much neighboring body.
  3. Use the face-closed region as the next automatic rigging/weight island, and
     keep a small blend band at the boundary.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
from collections import defaultdict
from pathlib import Path
from typing import Any

SPINE_BONES = {"Pelvis", "Spine", "Spine1", "Spine2"}
LOWER_LIMB_BONES = {"L_Thigh", "R_Thigh", "L_Calf", "R_Calf", "L_Foot", "R_Foot"}
NEAR_SHELL_DISTANCE = 0.08
WEIGHT_SYNC_L1_THRESHOLD = 0.35


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obj", type=Path, required=True)
    parser.add_argument("--weights", type=Path, required=True)
    parser.add_argument(
        "--region-map",
        type=Path,
        required=True,
        help="JSON from route_a_mushroom_extract_blender_region_map.py",
    )
    parser.add_argument("--region", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fail-on-alert", action="store_true")
    return parser.parse_args()


def read_obj(
    path: Path,
) -> tuple[list[tuple[float, float, float]], list[tuple[int, int, int]]]:
    vertices: list[tuple[float, float, float]] = []
    triangles: list[tuple[int, int, int]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("v "):
            _, x, y, z, *_rest = line.split()
            vertices.append((float(x), float(y), float(z)))
        elif line.startswith("f "):
            indices = [int(token.split("/")[0]) - 1 for token in line.split()[1:]]
            if len(indices) == 3:
                triangles.append((indices[0], indices[1], indices[2]))
            elif len(indices) > 3:
                for i in range(1, len(indices) - 1):
                    triangles.append((indices[0], indices[i], indices[i + 1]))
    if not vertices or not triangles:
        raise ValueError(f"OBJ must contain vertices and triangles: {path}")
    return vertices, triangles


def read_weights(path: Path) -> dict[int, dict[str, float]]:
    weights: dict[int, dict[str, float]] = defaultdict(dict)
    with path.open(newline="", encoding="utf-8", errors="replace") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        for row in reader:
            bone = row["er_target_bone"]
            weight = float(row["weight"])
            if weight <= 0.0 or bone.startswith("<"):
                continue
            weights[int(row["vertex"])][bone] = (
                weights[int(row["vertex"])].get(bone, 0.0) + weight
            )
    return weights


def read_region(path: Path, region: str) -> set[int]:
    data = json.loads(path.read_text(encoding="utf-8"))
    try:
        indices = data["regions"][region]["source_vertex_indices"]
    except KeyError as error:
        raise ValueError(f"region {region!r} not found in {path}") from error
    return set(indices)


def unique_edges(
    triangles: list[tuple[int, int, int]],
) -> dict[tuple[int, int], list[int]]:
    edges: dict[tuple[int, int], list[int]] = defaultdict(list)
    for face_index, (a, b, c) in enumerate(triangles):
        for u, v in ((a, b), (b, c), (c, a)):
            if u > v:
                u, v = v, u
            edges[(u, v)].append(face_index)
    return edges


def mixed_faces(
    triangles: list[tuple[int, int, int]], region: set[int]
) -> list[dict[str, Any]]:
    mixed: list[dict[str, Any]] = []
    for face_index, triangle in enumerate(triangles):
        count = sum(vertex in region for vertex in triangle)
        if 0 < count < 3:
            mixed.append(
                {
                    "face_index": face_index,
                    "triangle": list(triangle),
                    "region_vertex_count": count,
                }
            )
    return mixed


def face_closed_variants(
    triangles: list[tuple[int, int, int]], region: set[int]
) -> tuple[set[int], set[int]]:
    expanded = set(region)
    shrunk = set(region)
    for mode, candidate in (("expand", expanded), ("shrink", shrunk)):
        changed = True
        iterations = 0
        while changed and iterations < 32:
            changed = False
            iterations += 1
            for mixed in mixed_faces(triangles, candidate):
                triangle = set(mixed["triangle"])
                before = len(candidate)
                if mode == "expand":
                    candidate.update(triangle)
                else:
                    candidate.difference_update(triangle)
                changed = changed or len(candidate) != before
    return expanded, shrunk


def bone_sums(
    weights: dict[int, dict[str, float]], vertex: int
) -> tuple[float, float, str]:
    vertex_weights = weights.get(vertex, {})
    spine = sum(vertex_weights.get(bone, 0.0) for bone in SPINE_BONES)
    limb = sum(vertex_weights.get(bone, 0.0) for bone in LOWER_LIMB_BONES)
    dominant = (
        max(vertex_weights, key=lambda bone: vertex_weights[bone])
        if vertex_weights
        else "<none>"
    )
    return spine, limb, dominant


def boundary_edges(
    vertices: list[tuple[float, float, float]],
    triangles: list[tuple[int, int, int]],
    region: set[int],
    weights: dict[int, dict[str, float]],
) -> list[dict[str, Any]]:
    y_values = [vertex[1] for vertex in vertices]
    min_y, max_y = min(y_values), max(y_values)
    height_span = max(max_y - min_y, 1.0e-9)
    out: list[dict[str, Any]] = []
    for (u, v), face_indices in unique_edges(triangles).items():
        if (u in region) == (v in region):
            continue
        region_vertex = u if u in region else v
        other_vertex = v if u in region else u
        r_spine, r_limb, r_dom = bone_sums(weights, region_vertex)
        o_spine, o_limb, o_dom = bone_sums(weights, other_vertex)
        midpoint_height = (
            (vertices[region_vertex][1] + vertices[other_vertex][1]) * 0.5 - min_y
        ) / height_span
        out.append(
            {
                "edge": [u, v],
                "region_vertex": region_vertex,
                "other_vertex": other_vertex,
                "midpoint_height_norm": midpoint_height,
                "length": math.dist(vertices[region_vertex], vertices[other_vertex]),
                "region_dominant_bone": r_dom,
                "other_dominant_bone": o_dom,
                "region_spine_weight": r_spine,
                "region_lower_limb_weight": r_limb,
                "other_spine_weight": o_spine,
                "other_lower_limb_weight": o_limb,
                "adjacent_face_count": len(face_indices),
            }
        )
    return sorted(out, key=lambda row: row["length"], reverse=True)


def weight_l1_distance(
    weights: dict[int, dict[str, float]], first: int, second: int
) -> float:
    first_weights = weights.get(first, {})
    second_weights = weights.get(second, {})
    bones = set(first_weights) | set(second_weights)
    return sum(
        abs(first_weights.get(bone, 0.0) - second_weights.get(bone, 0.0))
        for bone in bones
    )


def near_shell_pairs(
    vertices: list[tuple[float, float, float]],
    region: set[int],
    weights: dict[int, dict[str, float]],
) -> list[dict[str, Any]]:
    pairs: list[dict[str, Any]] = []
    for region_vertex in region:
        for outside_vertex, outside_position in enumerate(vertices):
            if outside_vertex in region:
                continue
            distance = math.dist(vertices[region_vertex], outside_position)
            if distance > NEAR_SHELL_DISTANCE:
                continue
            mismatch = weight_l1_distance(weights, region_vertex, outside_vertex)
            pairs.append(
                {
                    "region_vertex": region_vertex,
                    "outside_vertex": outside_vertex,
                    "distance": distance,
                    "weight_l1_mismatch": mismatch,
                }
            )
    return sorted(pairs, key=lambda row: row["distance"])


def choose_response(region_size: int, expanded_size: int, shrunk_size: int) -> str:
    expand_delta = expanded_size - region_size
    shrink_delta = region_size - shrunk_size
    if shrunk_size == 0:
        return "expand_to_face_closed_region"
    if shrink_delta > expand_delta * 1.25:
        return "expand_to_face_closed_region"
    if expand_delta > shrink_delta * 1.25:
        return "shrink_to_face_closed_region"
    return "human_review_boundary_then_expand_or_shrink"


def main() -> int:
    args = parse_args()
    vertices, triangles = read_obj(args.obj)
    weights = read_weights(args.weights)
    region = read_region(args.region_map, args.region)
    mixed = mixed_faces(triangles, region)
    expanded, shrunk = face_closed_variants(triangles, region)
    boundaries = boundary_edges(vertices, triangles, region, weights)
    shell_pairs = near_shell_pairs(vertices, region, weights)
    expanded_shell_pairs = near_shell_pairs(vertices, expanded, weights)
    weight_mismatch_pairs = [
        pair
        for pair in shell_pairs
        if pair["weight_l1_mismatch"] > WEIGHT_SYNC_L1_THRESHOLD
    ]
    expanded_weight_mismatch_pairs = [
        pair
        for pair in expanded_shell_pairs
        if pair["weight_l1_mismatch"] > WEIGHT_SYNC_L1_THRESHOLD
    ]
    alerts = []
    if mixed:
        alerts.append(
            {
                "code": "REGION_SPLIT_CUTS_TRIANGLES",
                "message": "Manual region boundary cuts through triangulated faces instead of following whole-face ownership.",
                "mixed_triangle_count": len(mixed),
                "candidate_face_closure": choose_response(
                    len(region), len(expanded), len(shrunk)
                ),
                "recommended_response": "report_face_closed_candidate_do_not_mutate_weights",
            }
        )
    if boundaries:
        max_edge = max(edge["length"] for edge in boundaries)
        if max_edge > 0.10:
            alerts.append(
                {
                    "code": "REGION_BOUNDARY_HAS_LONG_DEFORMATION_EDGES",
                    "message": "Region boundary has long connected edges across the movement seam.",
                    "max_boundary_edge_length": max_edge,
                    "recommended_response": "inspect_boundary_do_not_rewrite_weights_without_weight_mismatch",
                }
            )
    if shell_pairs:
        max_shell_mismatch = max(pair["weight_l1_mismatch"] for pair in shell_pairs)
        alerts.append(
            {
                "code": "REGION_HAS_COINCIDENT_OR_NEAR_SHELL_SEAM",
                "message": "Region has nearby/coincident non-region vertices that are not represented by connected edge boundaries.",
                "near_shell_pair_count": len(shell_pairs),
                "near_shell_outside_vertex_count": len(
                    {pair["outside_vertex"] for pair in shell_pairs}
                ),
                "max_weight_l1_mismatch": max_shell_mismatch,
                "recommended_response": "only_sync_local_weights_if_REGION_WEIGHT_SEAM_MISMATCH_fires",
            }
        )
    if expanded_shell_pairs:
        max_expanded_shell_mismatch = max(
            pair["weight_l1_mismatch"] for pair in expanded_shell_pairs
        )
        alerts.append(
            {
                "code": "FACE_CLOSURE_CANDIDATE_HAS_NEAR_SHELL_SEAM",
                "message": "The face-closed expansion candidate creates or exposes nearby/coincident non-region shell vertices.",
                "near_shell_pair_count": len(expanded_shell_pairs),
                "near_shell_outside_vertex_count": len(
                    {pair["outside_vertex"] for pair in expanded_shell_pairs}
                ),
                "weight_mismatch_pair_count": len(expanded_weight_mismatch_pairs),
                "max_weight_l1_mismatch": max_expanded_shell_mismatch,
                "recommended_response": "do_not_auto_apply_face_closure_as_weight_region",
            }
        )
    if weight_mismatch_pairs:
        alerts.append(
            {
                "code": "REGION_WEIGHT_SEAM_MISMATCH",
                "message": "Nearby/coincident region and non-region vertices have materially different weights.",
                "mismatch_pair_count": len(weight_mismatch_pairs),
                "threshold": WEIGHT_SYNC_L1_THRESHOLD,
                "max_weight_l1_mismatch": max(
                    pair["weight_l1_mismatch"] for pair in weight_mismatch_pairs
                ),
                "recommended_response": "local_proximity_weight_sync",
            }
        )

    report = {
        "obj": str(args.obj),
        "weights": str(args.weights),
        "region_map": str(args.region_map),
        "region": args.region,
        "source_vertex_count": len(vertices),
        "triangle_count": len(triangles),
        "region_vertex_count": len(region),
        "mixed_triangle_count": len(mixed),
        "boundary_edge_count": len(boundaries),
        "boundary_height_norm_min_max": [
            min((edge["midpoint_height_norm"] for edge in boundaries), default=None),
            max((edge["midpoint_height_norm"] for edge in boundaries), default=None),
        ],
        "expanded_face_closed_vertex_count": len(expanded),
        "shrunk_face_closed_vertex_count": len(shrunk),
        "near_shell_pair_count": len(shell_pairs),
        "near_shell_outside_vertex_count": len(
            {pair["outside_vertex"] for pair in shell_pairs}
        ),
        "weight_mismatch_pair_count": len(weight_mismatch_pairs),
        "expanded_near_shell_pair_count": len(expanded_shell_pairs),
        "expanded_near_shell_outside_vertex_count": len(
            {pair["outside_vertex"] for pair in expanded_shell_pairs}
        ),
        "expanded_weight_mismatch_pair_count": len(expanded_weight_mismatch_pairs),
        "weight_mismatch_threshold": WEIGHT_SYNC_L1_THRESHOLD,
        "alerts": alerts,
        "mixed_triangles_sample": mixed[:60],
        "boundary_edges_sample": boundaries[:60],
        "near_shell_pairs_sample": shell_pairs[:60],
        "expanded_near_shell_pairs_sample": expanded_shell_pairs[:60],
        "weight_mismatch_pairs_sample": weight_mismatch_pairs[:60],
        "expanded_weight_mismatch_pairs_sample": expanded_weight_mismatch_pairs[:60],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(f"wrote {args.output}")
    print(
        f"region={args.region} vertices={len(region)} mixed_triangles={len(mixed)} "
        f"boundary_edges={len(boundaries)} near_shell_pairs={len(shell_pairs)} "
        f"weight_mismatches={len(weight_mismatch_pairs)} expanded_near_shell_pairs={len(expanded_shell_pairs)} "
        f"expanded_weight_mismatches={len(expanded_weight_mismatch_pairs)} expand_face_closed={len(expanded)} "
        f"shrink_face_closed={len(shrunk)} alerts={len(alerts)}"
    )
    for alert in alerts:
        print(
            f"ALERT {alert['code']}: {alert['message']} response={alert['recommended_response']}"
        )
    if args.fail_on_alert and alerts:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
