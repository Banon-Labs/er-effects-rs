#!/usr/bin/env python3
"""Blender-side topology audit for Mushroom Man editable meshes.

Run inside Blender with --python and args after --. This audits true source-mesh
connectivity before OBJ export duplicates vertices at UV/normal seams.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import deque
from pathlib import Path
from typing import Any

bpy = __import__("bpy")
mathutils = __import__("mathutils")


def parse_args() -> argparse.Namespace:
    argv = sys.argv
    script_args = argv[argv.index("--") + 1 :] if "--" in argv else []
    parser = argparse.ArgumentParser()
    parser.add_argument("--object-name", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--max-source-vertices", type=int)
    return parser.parse_args(script_args)


def find_object(name: str) -> Any:
    obj = bpy.data.objects.get(name)
    if obj is not None:
        return obj
    candidates = [
        candidate
        for candidate in bpy.context.scene.objects
        if candidate.type == "MESH" and name in candidate.name
    ]
    if len(candidates) == 1:
        return candidates[0]
    raise ValueError(
        f"could not uniquely find mesh {name!r}: {[candidate.name for candidate in candidates]}"
    )


def ensure_object_mode(obj: Any) -> None:
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    if bpy.context.mode != "OBJECT":
        with bpy.context.temp_override(
            object=obj,
            active_object=obj,
            selected_objects=[obj],
            selected_editable_objects=[obj],
        ):
            bpy.ops.object.mode_set(mode="OBJECT")


def decimate_if_needed(obj: Any, max_source_vertices: int | None) -> dict[str, Any]:
    before = len(obj.data.vertices)
    if max_source_vertices is None or before <= max_source_vertices:
        return {
            "requested": max_source_vertices,
            "applied": False,
            "before": before,
            "after": before,
            "ratio": 1.0,
        }
    ensure_object_mode(obj)
    ratio = max_source_vertices / before
    modifier = obj.modifiers.new("MushroomMan_topology_audit_decimate", "DECIMATE")
    modifier.ratio = ratio
    modifier.use_collapse_triangulate = True
    with bpy.context.temp_override(
        object=obj,
        active_object=obj,
        selected_objects=[obj],
        selected_editable_objects=[obj],
    ):
        bpy.ops.object.modifier_apply(modifier=modifier.name)
    return {
        "requested": max_source_vertices,
        "applied": True,
        "before": before,
        "after": len(obj.data.vertices),
        "ratio": ratio,
    }


def blender_to_flver(vector: Any) -> tuple[float, float, float]:
    return (float(vector.x), float(vector.z), float(vector.y))


def connected_components(
    vertex_count: int, adjacency: list[set[int]]
) -> list[list[int]]:
    remaining = set(range(vertex_count))
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
    obj: Any, component: list[int], polygon_indices: list[int]
) -> dict[str, Any]:
    mesh = obj.data
    positions = [
        blender_to_flver(obj.matrix_world @ mesh.vertices[index].co)
        for index in component
    ]
    avg = [
        sum(position[axis] for position in positions) / len(positions)
        for axis in range(3)
    ]
    bbox_min = [min(position[axis] for position in positions) for axis in range(3)]
    bbox_max = [max(position[axis] for position in positions) for axis in range(3)]
    normals = []
    for poly_index in polygon_indices:
        poly = mesh.polygons[poly_index]
        normal_matrix = obj.matrix_world.to_3x3().inverted().transposed()
        normal = normal_matrix @ poly.normal
        normal.normalize()
        normals.append(blender_to_flver(normal))
    avg_normal = [0.0, 0.0, 0.0]
    if normals:
        avg_normal = [
            sum(normal[axis] for normal in normals) / len(normals) for axis in range(3)
        ]
    side = "left" if avg[0] > 0.001 else "right" if avg[0] < -0.001 else "midline"
    return {
        "size": len(component),
        "polygon_count": len(polygon_indices),
        "side": side,
        "avg": avg,
        "bbox_min": bbox_min,
        "bbox_max": bbox_max,
        "avg_normal": avg_normal,
        "sample_vertices": sorted(component)[:20],
        "sample_polygons": sorted(polygon_indices)[:20],
    }


def main() -> int:
    args = parse_args()
    obj = find_object(args.object_name)
    if obj.type != "MESH":
        raise TypeError(f"{obj.name} is {obj.type}, expected MESH")
    decimation = decimate_if_needed(obj, args.max_source_vertices)
    mesh = obj.data
    adjacency = [set() for _vertex in mesh.vertices]
    edge_face_counts: dict[tuple[int, int], int] = {}
    polygon_indices_by_vertex: dict[int, list[int]] = {
        index: [] for index in range(len(mesh.vertices))
    }
    for edge in mesh.edges:
        a, b = edge.vertices
        adjacency[a].add(b)
        adjacency[b].add(a)
        edge_face_counts[tuple(sorted((a, b)))] = 0
    for poly in mesh.polygons:
        verts = list(poly.vertices)
        for vertex_index in verts:
            polygon_indices_by_vertex[vertex_index].append(poly.index)
        for offset, a in enumerate(verts):
            b = verts[(offset + 1) % len(verts)]
            key = tuple(sorted((a, b)))
            edge_face_counts[key] = edge_face_counts.get(key, 0) + 1
    components = connected_components(len(mesh.vertices), adjacency)
    summaries = []
    for component in components:
        polygon_indices = sorted(
            {
                poly_index
                for vertex_index in component
                for poly_index in polygon_indices_by_vertex[vertex_index]
            }
        )
        summaries.append(component_summary(obj, component, polygon_indices))
    isolated_vertices = [
        index for index, neighbors in enumerate(adjacency) if not neighbors
    ]
    boundary_edges = [
        list(edge) for edge, count in edge_face_counts.items() if count == 1
    ]
    nonmanifold_edges = [
        list(edge) for edge, count in edge_face_counts.items() if count > 2
    ]
    report = {
        "object": obj.name,
        "vertices": len(mesh.vertices),
        "edges": len(mesh.edges),
        "polygons": len(mesh.polygons),
        "decimation": decimation,
        "isolated_vertices": isolated_vertices,
        "boundary_edge_count": len(boundary_edges),
        "boundary_edge_samples": boundary_edges[:40],
        "nonmanifold_edge_count": len(nonmanifold_edges),
        "nonmanifold_edge_samples": nonmanifold_edges[:40],
        "component_count": len(summaries),
        "components": summaries,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(args.output)
    print(
        f"object={obj.name} vertices={len(mesh.vertices)} polygons={len(mesh.polygons)} components={len(summaries)} isolated={len(isolated_vertices)} boundary_edges={len(boundary_edges)} nonmanifold_edges={len(nonmanifold_edges)}"
    )
    for index, summary in enumerate(summaries):
        avg = summary["avg"]
        bbox_min = summary["bbox_min"]
        bbox_max = summary["bbox_max"]
        print(
            f"COMP {index:02d} size={summary['size']} polys={summary['polygon_count']} side={summary['side']} "
            f"avg=({avg[0]:.4f},{avg[1]:.4f},{avg[2]:.4f}) "
            f"bbox=({bbox_min[0]:.4f},{bbox_min[1]:.4f},{bbox_min[2]:.4f}).."
            f"({bbox_max[0]:.4f},{bbox_max[1]:.4f},{bbox_max[2]:.4f})"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
