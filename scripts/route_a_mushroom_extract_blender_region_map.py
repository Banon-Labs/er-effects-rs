#!/usr/bin/env python3
"""Extract user-split Blender objects into a Mushroom Man vertex region map.

Run with Blender's Python after loading a comparison .blend:

    blender --background scene.blend --python scripts/route_a_mushroom_extract_blender_region_map.py -- \
      --source-obj target/.../blender_edit_c2280.obj \
      --region feet="Mushroom compensated actual FLVER mesh13.001" \
      --output target/.../manual-region-map.json

The region map lets a human split/nudge obvious deformation regions in Blender,
while offline scripts turn those edits into repeatable source-vertex ownership
hints for automatic rigging/weight self-healing.
"""

from __future__ import annotations

import argparse
import importlib
import json
import math
import sys
from collections import defaultdict, deque
from pathlib import Path
from typing import Any, cast


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-obj", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--output-tsv",
        type=Path,
        help="optional simple region/source_vertex_index TSV for Rust-side consumers",
    )
    parser.add_argument(
        "--region",
        action="append",
        default=[],
        help="region_name=Blender object name. Can be repeated.",
    )
    parser.add_argument("--quantum", type=float, default=1.0e-5)
    parser.add_argument("--max-match-distance", type=float, default=1.0e-3)
    return parser.parse_args(argv)


def parse_region_specs(specs: list[str]) -> dict[str, str]:
    regions: dict[str, str] = {}
    for spec in specs:
        if "=" not in spec:
            raise ValueError(f"region spec must be name=object, got {spec!r}")
        name, object_name = spec.split("=", 1)
        name = name.strip()
        object_name = object_name.strip()
        if not name or not object_name:
            raise ValueError(f"region spec must be name=object, got {spec!r}")
        regions[name] = object_name
    if not regions:
        raise ValueError("at least one --region name=object spec is required")
    return regions


def read_obj_positions(path: Path) -> list[tuple[float, float, float]]:
    positions: list[tuple[float, float, float]] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if not line.startswith("v "):
            continue
        _, x, y, z, *_rest = line.split()
        positions.append((float(x), float(y), float(z)))
    if not positions:
        raise ValueError(f"source OBJ has no vertices: {path}")
    return positions


def quantize(
    position: tuple[float, float, float], quantum: float
) -> tuple[int, int, int]:
    return (
        round(position[0] / quantum),
        round(position[1] / quantum),
        round(position[2] / quantum),
    )


def distance(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return math.sqrt(sum((a[i] - b[i]) ** 2 for i in range(3)))


def source_obj_to_blender_position(
    position: tuple[float, float, float],
) -> tuple[float, float, float]:
    return (position[0], -position[2], position[1])


def build_position_buckets(
    positions: list[tuple[float, float, float]], quantum: float
) -> dict[tuple[int, int, int], deque[int]]:
    buckets: dict[tuple[int, int, int], deque[int]] = defaultdict(deque)
    for index, position in enumerate(positions):
        buckets[quantize(position, quantum)].append(index)
    return buckets


def object_world_positions(
    bpy: Any, object_name: str
) -> list[tuple[float, float, float]]:
    obj = bpy.data.objects.get(object_name)
    if obj is None:
        available = ", ".join(sorted(bpy.data.objects.keys()))
        raise ValueError(
            f"Blender object {object_name!r} not found. Available: {available}"
        )
    if obj.type != "MESH":
        raise ValueError(f"Blender object {object_name!r} is {obj.type}, expected MESH")
    depsgraph = bpy.context.evaluated_depsgraph_get()
    evaluated = obj.evaluated_get(depsgraph)
    mesh = evaluated.to_mesh()
    try:
        matrix = evaluated.matrix_world
        return [tuple(matrix @ vertex.co) for vertex in mesh.vertices]
    finally:
        evaluated.to_mesh_clear()


def match_region_vertices(
    source_positions: list[tuple[float, float, float]],
    region_positions: list[tuple[float, float, float]],
    quantum: float,
    max_match_distance: float,
) -> tuple[list[int], float, list[tuple[float, float, float]]]:
    match_positions = [
        source_obj_to_blender_position(position) for position in source_positions
    ]
    buckets = build_position_buckets(match_positions, quantum)
    matched: list[int] = []
    max_distance = 0.0
    unmatched: list[tuple[float, float, float]] = []

    for position in region_positions:
        key = quantize(position, quantum)
        index = buckets.get(key, deque()).popleft() if buckets.get(key) else None
        if index is None:
            best_index = None
            best_distance = float("inf")
            for candidate_index, source_position in enumerate(match_positions):
                d = distance(position, source_position)
                if d < best_distance:
                    best_distance = d
                    best_index = candidate_index
            if best_index is not None and best_distance <= max_match_distance:
                index = best_index
                max_distance = max(max_distance, best_distance)
        if index is None:
            unmatched.append(position)
        else:
            matched.append(index)
            max_distance = max(max_distance, distance(position, match_positions[index]))

    return sorted(set(matched)), max_distance, unmatched


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    bpy = cast(Any, importlib.import_module("bpy"))
    source_positions = read_obj_positions(args.source_obj)
    region_specs = parse_region_specs(args.region)

    regions: dict[str, dict[str, Any]] = {}
    for region_name, object_name in region_specs.items():
        positions = object_world_positions(bpy, object_name)
        indices, max_distance, unmatched = match_region_vertices(
            source_positions,
            positions,
            args.quantum,
            args.max_match_distance,
        )
        regions[region_name] = {
            "object": object_name,
            "source_vertex_indices": indices,
            "object_vertex_count": len(positions),
            "matched_source_vertex_count": len(indices),
            "max_match_distance": max_distance,
            "unmatched_object_vertices": unmatched[:20],
            "unmatched_object_vertex_count": len(unmatched),
        }

    result = {
        "blend_file": bpy.data.filepath,
        "source_obj": str(args.source_obj),
        "source_coordinate_transform": "blender_obj_import=(x,-z,y)",
        "source_vertex_count": len(source_positions),
        "regions": regions,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2), encoding="utf-8")
    if args.output_tsv is not None:
        args.output_tsv.parent.mkdir(parents=True, exist_ok=True)
        with args.output_tsv.open("w", encoding="utf-8") as tsv:
            tsv.write("region\tsource_vertex_index\n")
            for region_name, region in regions.items():
                for index in region["source_vertex_indices"]:
                    tsv.write(f"{region_name}\t{index}\n")
    print(f"wrote {args.output}")
    for name, region in regions.items():
        print(
            f"region={name} object={region['object']} "
            f"object_vertices={region['object_vertex_count']} "
            f"matched={region['matched_source_vertex_count']} "
            f"unmatched={region['unmatched_object_vertex_count']} "
            f"max_distance={region['max_match_distance']:.9f}"
        )
    return 0


if __name__ == "__main__":
    if "--" in sys.argv:
        script_args = sys.argv[sys.argv.index("--") + 1 :]
    else:
        script_args = sys.argv[1:]
    raise SystemExit(main(script_args))
