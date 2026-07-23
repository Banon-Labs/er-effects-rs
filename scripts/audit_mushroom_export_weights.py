#!/usr/bin/env python3
"""Audit generated Mushroom Man OBJ/weight exports for mesh and bone connectivity.

This is intentionally independent of Blender so it can run as a cheap build gate
right after export and before packaging/launching a candidate ME3 profile.
"""

from __future__ import annotations

import argparse
import json
from collections import defaultdict, deque
from collections.abc import Iterable
from pathlib import Path
from typing import TypedDict


class ComponentSummary(TypedDict):
    size: int
    side: str
    avg: list[float]
    bbox_min: list[float]
    bbox_max: list[float]
    sample_indices: list[int]


class GroupSummary(TypedDict):
    weighted_vertices: int
    component_count: int
    component_sizes: list[int]
    left_count: int
    right_count: int
    midline_count: int
    components: list[ComponentSummary]


class AuditReport(TypedDict):
    obj: str
    weights: str
    vertex_count: int
    face_count: int
    isolated_vertices: list[int]
    mesh_components: list[ComponentSummary]
    groups: dict[str, GroupSummary]
    disconnected_weight_groups: list[str]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--obj", required=True, type=Path)
    parser.add_argument("--weights", required=True, type=Path)
    parser.add_argument("--json", required=True, type=Path)
    parser.add_argument("--text", required=True, type=Path)
    parser.add_argument(
        "--fail-on-isolated",
        action="store_true",
        help="exit non-zero when OBJ vertices have no edge connectivity",
    )
    return parser.parse_args()


def load_obj(
    path: Path,
) -> tuple[list[tuple[float, float, float]], list[tuple[int, int, int]]]:
    vertices: list[tuple[float, float, float]] = []
    faces: list[tuple[int, int, int]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("v "):
            _tag, x, y, z = line.split()[:4]
            vertices.append((float(x), float(y), float(z)))
        elif line.startswith("f "):
            indices = [int(token.split("/")[0]) - 1 for token in line.split()[1:]]
            if len(indices) == 3:
                faces.append((indices[0], indices[1], indices[2]))
    return vertices, faces


def build_adjacency(
    vertex_count: int, faces: Iterable[tuple[int, int, int]]
) -> list[set[int]]:
    adjacency = [set() for _index in range(vertex_count)]
    for a, b, c in faces:
        adjacency[a].update((b, c))
        adjacency[b].update((a, c))
        adjacency[c].update((a, b))
    return adjacency


def connected_components(
    indices: Iterable[int], adjacency: list[set[int]]
) -> list[list[int]]:
    remaining = set(indices)
    components: list[list[int]] = []
    while remaining:
        start = remaining.pop()
        queue = deque([start])
        component = [start]
        while queue:
            current = queue.popleft()
            for nxt in adjacency[current]:
                if nxt in remaining:
                    remaining.remove(nxt)
                    queue.append(nxt)
                    component.append(nxt)
        components.append(component)
    components.sort(key=len, reverse=True)
    return components


def component_summary(
    component: list[int], vertices: list[tuple[float, float, float]]
) -> ComponentSummary:
    positions = [vertices[index] for index in component]
    avg = [
        sum(position[axis] for position in positions) / len(positions)
        for axis in range(3)
    ]
    bbox_min = [min(position[axis] for position in positions) for axis in range(3)]
    bbox_max = [max(position[axis] for position in positions) for axis in range(3)]
    side = "left" if avg[0] > 0.001 else "right" if avg[0] < -0.001 else "midline"
    return {
        "size": len(component),
        "side": side,
        "avg": avg,
        "bbox_min": bbox_min,
        "bbox_max": bbox_max,
        "sample_indices": sorted(component)[:20],
    }


def load_weight_groups(path: Path) -> dict[str, set[int]]:
    groups: dict[str, set[int]] = defaultdict(set)
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines()[1:]:
        parts = line.split("\t")
        if len(parts) >= 7 and float(parts[6]) > 0.0001:
            groups[parts[5]].add(int(parts[0]))
    return dict(groups)


def group_summary(
    indices: set[int],
    adjacency: list[set[int]],
    vertices: list[tuple[float, float, float]],
) -> GroupSummary:
    components = connected_components(indices, adjacency)
    left_count = sum(1 for index in indices if vertices[index][0] > 0.001)
    right_count = sum(1 for index in indices if vertices[index][0] < -0.001)
    midline_count = len(indices) - left_count - right_count
    return {
        "weighted_vertices": len(indices),
        "component_count": len(components),
        "component_sizes": [len(component) for component in components],
        "left_count": left_count,
        "right_count": right_count,
        "midline_count": midline_count,
        "components": [
            component_summary(component, vertices) for component in components
        ],
    }


def write_text_report(report: AuditReport, path: Path) -> None:
    lines: list[str] = []
    lines.append("Mushroom export connectivity audit")
    lines.append(
        f"mesh vertices={report['vertex_count']} faces={report['face_count']} "
        f"isolated_no_edge_vertices={len(report['isolated_vertices'])}"
    )
    lines.append(
        "full_mesh_connected_islands="
        + str(len(report["mesh_components"]))
        + " sizes="
        + str([component["size"] for component in report["mesh_components"]])
    )
    disconnected_groups = report["disconnected_weight_groups"]
    lines.append(
        f"disconnected_weight_groups={len(disconnected_groups)} {disconnected_groups}"
    )
    group_summaries = report["groups"]
    for group_name in sorted(group_summaries):
        summary = group_summaries[group_name]
        if summary["component_count"] <= 1 and group_name not in {
            "Head",
            "L_Foot",
            "R_Foot",
        }:
            continue
        lines.append(
            f"GROUP {group_name}: weighted={summary['weighted_vertices']} "
            f"components={summary['component_sizes']} "
            f"L/R/M={summary['left_count']}/{summary['right_count']}/{summary['midline_count']}"
        )
        for component in summary["components"][:6]:
            avg = component["avg"]
            bbox_min = component["bbox_min"]
            bbox_max = component["bbox_max"]
            lines.append(
                f"  comp size={component['size']} side={component['side']} "
                f"avg=({avg[0]:.4f},{avg[1]:.4f},{avg[2]:.4f}) "
                f"bbox=({bbox_min[0]:.4f},{bbox_min[1]:.4f},{bbox_min[2]:.4f}).."
                f"({bbox_max[0]:.4f},{bbox_max[1]:.4f},{bbox_max[2]:.4f}) "
                f"sample={component['sample_indices'][:12]}"
            )
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    args = parse_args()
    vertices, faces = load_obj(args.obj)
    adjacency = build_adjacency(len(vertices), faces)
    isolated = [index for index, neighbors in enumerate(adjacency) if not neighbors]
    mesh_components = [
        component_summary(component, vertices)
        for component in connected_components(range(len(vertices)), adjacency)
    ]
    groups = load_weight_groups(args.weights)
    group_summaries = {
        group_name: group_summary(indices, adjacency, vertices)
        for group_name, indices in sorted(groups.items())
    }
    disconnected_weight_groups = [
        group_name
        for group_name, summary in sorted(group_summaries.items())
        if summary["component_count"] > 1
    ]
    report: AuditReport = {
        "obj": str(args.obj),
        "weights": str(args.weights),
        "vertex_count": len(vertices),
        "face_count": len(faces),
        "isolated_vertices": isolated,
        "mesh_components": mesh_components,
        "groups": group_summaries,
        "disconnected_weight_groups": disconnected_weight_groups,
    }
    args.json.parent.mkdir(parents=True, exist_ok=True)
    args.text.parent.mkdir(parents=True, exist_ok=True)
    args.json.write_text(json.dumps(report, indent=2), encoding="utf-8")
    write_text_report(report, args.text)
    print(args.text)
    if args.fail_on_isolated and isolated:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
