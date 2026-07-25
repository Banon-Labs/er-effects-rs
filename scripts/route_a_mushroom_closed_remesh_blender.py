#!/usr/bin/env python3
"""Headless Blender helper for generating a closed Mushroom Man source mesh.

Run with Blender, not system Python:
  blender --background --python scripts/route_a_mushroom_closed_remesh_blender.py -- \
    --input-obj ... --input-weights ... --output-obj ... --output-weights ...

The script voxel-remeshes the source OBJ into a closed surface, decimates it under
FLVER capacity, exports a simple triangulated OBJ, and transfers ER bone weights
from the original vertices by inverse-distance nearest-neighbor blending.
"""

from __future__ import annotations

import argparse
import csv
import importlib
import json
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

bpy: Any = None


def load_blender_module() -> None:
    global bpy  # noqa: PLW0603
    if bpy is not None:
        return
    try:
        bpy = importlib.import_module("bpy")
    except ModuleNotFoundError as exc:
        raise RuntimeError(
            "This helper must be run by Blender Python, e.g. "
            "blender --background --python scripts/route_a_mushroom_closed_remesh_blender.py -- ..."
        ) from exc


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input-obj", required=True, type=Path)
    parser.add_argument("--input-weights", required=True, type=Path)
    parser.add_argument("--output-obj", required=True, type=Path)
    parser.add_argument("--output-weights", required=True, type=Path)
    parser.add_argument("--summary", required=True, type=Path)
    parser.add_argument("--voxel-size", type=float, default=0.035)
    parser.add_argument("--target-vertices", type=int, default=1440)
    parser.add_argument("--target-triangles", type=int, default=2296)
    parser.add_argument("--smooth-iterations", type=int, default=4)
    parser.add_argument("--min-component-faces", type=int, default=8)
    parser.add_argument("--nearest", type=int, default=4)
    return parser.parse_args(sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else None)


def import_obj(path: Path) -> Any:
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete()
    if hasattr(bpy.ops.wm, "obj_import"):
        bpy.ops.wm.obj_import(filepath=str(path), forward_axis="Y", up_axis="Z")
    else:
        bpy.ops.import_scene.obj(filepath=str(path), axis_forward="Y", axis_up="Z")
    objects = [obj for obj in bpy.context.selected_objects if obj.type == "MESH"]
    if not objects:
        raise RuntimeError(f"no mesh objects imported from {path}")
    bpy.ops.object.select_all(action="DESELECT")
    for obj in objects:
        obj.select_set(True)
    bpy.context.view_layer.objects.active = objects[0]
    if len(objects) > 1:
        bpy.ops.object.join()
    obj = bpy.context.view_layer.objects.active
    if obj is None or obj.type != "MESH":
        raise RuntimeError("joined object is not a mesh")
    return obj


def apply_modifier(obj: Any, modifier: Any) -> None:
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    bpy.ops.object.modifier_apply(modifier=modifier.name)


def triangulate(obj: Any) -> None:
    mod = obj.modifiers.new("triangulate_for_flver", "TRIANGULATE")
    apply_modifier(obj, mod)


def merge_by_distance(obj: Any, distance: float) -> None:
    bpy.context.view_layer.objects.active = obj
    obj.select_set(True)
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.mesh.remove_doubles(threshold=distance)
    bpy.ops.object.mode_set(mode="OBJECT")


def mesh_counts(obj: Any) -> tuple[int, int]:
    mesh = obj.data
    triangles = sum(max(0, len(poly.vertices) - 2) for poly in mesh.polygons)
    return len(mesh.vertices), triangles


def remesh_closed(obj: Any, args: argparse.Namespace) -> None:
    remesh = obj.modifiers.new("closed_voxel_surface", "REMESH")
    remesh.mode = "VOXEL"
    remesh.voxel_size = args.voxel_size
    if hasattr(remesh, "adaptivity"):
        remesh.adaptivity = 0.0
    if hasattr(remesh, "use_smooth_shade"):
        remesh.use_smooth_shade = True
    # Keep disconnected pieces during voxelization so close pieces can fuse by voxel size
    # rather than being silently discarded.
    if hasattr(remesh, "use_remove_disconnected"):
        remesh.use_remove_disconnected = False
    apply_modifier(obj, remesh)
    merge_by_distance(obj, args.voxel_size * 0.05)
    if args.smooth_iterations > 0:
        smooth = obj.modifiers.new("fair_closed_surface", "SMOOTH")
        smooth.factor = 0.45
        smooth.iterations = args.smooth_iterations
        apply_modifier(obj, smooth)
    triangulate(obj)

    for _ in range(10):
        vertices, triangles = mesh_counts(obj)
        if vertices <= args.target_vertices and triangles <= args.target_triangles:
            break
        ratio = min(args.target_vertices / max(vertices, 1), args.target_triangles / max(triangles, 1)) * 0.94
        if ratio >= 0.995:
            break
        decimate = obj.modifiers.new("flver_capacity_decimate", "DECIMATE")
        decimate.ratio = max(0.05, min(0.98, ratio))
        decimate.use_collapse_triangulate = True
        apply_modifier(obj, decimate)
        merge_by_distance(obj, args.voxel_size * 0.05)
        triangulate(obj)


def read_weight_rows(path: Path) -> tuple[list[tuple[float, float, float]], list[dict[str, float]]]:
    positions_by_vertex: dict[int, tuple[float, float, float]] = {}
    weights_by_vertex: dict[int, dict[str, float]] = defaultdict(dict)
    with path.open(newline="", encoding="utf-8") as handle:
        reader = csv.DictReader(handle, delimiter="\t")
        for row in reader:
            vertex = int(row["vertex"])
            positions_by_vertex[vertex] = (
                float(row["source_x"]),
                float(row["source_y"]),
                float(row["source_z"]),
            )
            weights_by_vertex[vertex][row["er_target_bone"]] = weights_by_vertex[vertex].get(
                row["er_target_bone"], 0.0
            ) + float(row["weight"])
    max_index = max(positions_by_vertex)
    positions = [positions_by_vertex[i] for i in range(max_index + 1)]
    weights = [weights_by_vertex[i] for i in range(max_index + 1)]
    return positions, weights


def sqdist(a: tuple[float, float, float], b: tuple[float, float, float]) -> float:
    return (a[0] - b[0]) ** 2 + (a[1] - b[1]) ** 2 + (a[2] - b[2]) ** 2


def transfer_weights(
    output_vertices: list[tuple[float, float, float]],
    source_positions: list[tuple[float, float, float]],
    source_weights: list[dict[str, float]],
    nearest: int,
) -> list[dict[str, float]]:
    transferred: list[dict[str, float]] = []
    for vertex in output_vertices:
        ranked = sorted(
            ((sqdist(vertex, source), index) for index, source in enumerate(source_positions)),
            key=lambda pair: pair[0],
        )[:nearest]
        accum: dict[str, float] = defaultdict(float)
        divisor = 0.0
        for distance_squared, index in ranked:
            weight = 1.0 / max(distance_squared, 1.0e-8)
            divisor += weight
            for bone, bone_weight in source_weights[index].items():
                accum[bone] += bone_weight * weight
        normalized = {bone: value / divisor for bone, value in accum.items() if value > 1.0e-8}
        total = sum(normalized.values()) or 1.0
        transferred.append({bone: value / total for bone, value in normalized.items()})
    return transferred


def write_obj(
    path: Path, obj: Any, min_component_faces: int
) -> tuple[list[tuple[float, float, float]], list[tuple[int, int, int]], int]:
    mesh = obj.data
    vertices = [tuple(obj.matrix_world @ vertex.co) for vertex in mesh.vertices]
    triangles: list[tuple[int, int, int]] = []
    for poly in mesh.polygons:
        if len(poly.vertices) == 3:
            triangles.append(tuple(poly.vertices))
        else:
            verts = list(poly.vertices)
            for i in range(1, len(verts) - 1):
                triangles.append((verts[0], verts[i], verts[i + 1]))
    vertices, triangles, removed_components = filter_small_triangle_components(
        vertices, triangles, min_component_faces
    )
    normals = vertex_normals(vertices, triangles)
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write("o closed_mushroom_man\n")
        for x, y, z in vertices:
            handle.write(f"v {x:.9f} {y:.9f} {z:.9f}\n")
        for _ in vertices:
            handle.write("vt 0.000000000 0.000000000\n")
        for x, y, z in normals:
            handle.write(f"vn {x:.9f} {y:.9f} {z:.9f}\n")
        for a, b, c in triangles:
            handle.write(f"f {a + 1}/{a + 1}/{a + 1} {b + 1}/{b + 1}/{b + 1} {c + 1}/{c + 1}/{c + 1}\n")
    return vertices, triangles, removed_components


def vertex_normals(
    vertices: list[tuple[float, float, float]], triangles: list[tuple[int, int, int]]
) -> list[tuple[float, float, float]]:
    normals = [(0.0, 0.0, 0.0) for _ in vertices]
    for a, b, c in triangles:
        ax, ay, az = vertices[a]
        bx, by, bz = vertices[b]
        cx, cy, cz = vertices[c]
        ux, uy, uz = bx - ax, by - ay, bz - az
        vx, vy, vz = cx - ax, cy - ay, cz - az
        nx = uy * vz - uz * vy
        ny = uz * vx - ux * vz
        nz = ux * vy - uy * vx
        for index in (a, b, c):
            ox, oy, oz = normals[index]
            normals[index] = (ox + nx, oy + ny, oz + nz)
    normalized = []
    for x, y, z in normals:
        length = max((x * x + y * y + z * z) ** 0.5, 1.0e-8)
        normalized.append((x / length, y / length, z / length))
    return normalized


def filter_small_triangle_components(
    vertices: list[tuple[float, float, float]],
    triangles: list[tuple[int, int, int]],
    min_component_faces: int,
) -> tuple[list[tuple[float, float, float]], list[tuple[int, int, int]], int]:
    if min_component_faces <= 1 or not triangles:
        return vertices, triangles, 0
    edge_faces: dict[tuple[int, int], list[int]] = defaultdict(list)
    for face_index, (a, b, c) in enumerate(triangles):
        for u, v in ((a, b), (b, c), (c, a)):
            edge_faces[(u, v) if u <= v else (v, u)].append(face_index)
    face_adjacency = [set() for _ in triangles]
    for faces in edge_faces.values():
        for i in range(len(faces)):
            for j in range(i + 1, len(faces)):
                face_adjacency[faces[i]].add(faces[j])
                face_adjacency[faces[j]].add(faces[i])
    seen = [False] * len(triangles)
    kept_faces: set[int] = set()
    removed_components = 0
    for start in range(len(triangles)):
        if seen[start]:
            continue
        queue = [start]
        seen[start] = True
        component: list[int] = []
        while queue:
            face = queue.pop()
            component.append(face)
            for neighbor in face_adjacency[face]:
                if not seen[neighbor]:
                    seen[neighbor] = True
                    queue.append(neighbor)
        if len(component) >= min_component_faces:
            kept_faces.update(component)
        else:
            removed_components += 1
    kept_triangles_old = [triangles[index] for index in sorted(kept_faces)]
    used_vertices = sorted({vertex for face in kept_triangles_old for vertex in face})
    remap = {old: new for new, old in enumerate(used_vertices)}
    filtered_vertices = [vertices[index] for index in used_vertices]
    filtered_triangles = [
        (remap[face[0]], remap[face[1]], remap[face[2]]) for face in kept_triangles_old
    ]
    return filtered_vertices, filtered_triangles, removed_components


def write_weights(
    path: Path,
    vertices: list[tuple[float, float, float]],
    weights: list[dict[str, float]],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write("vertex\tsource_x\tsource_y\tsource_z\tsource_bone\ter_target_bone\tweight\n")
        for index, ((x, y, z), bone_weights) in enumerate(zip(vertices, weights, strict=True)):
            for bone, weight in sorted(bone_weights.items(), key=lambda pair: pair[1], reverse=True):
                if weight <= 1.0e-7:
                    continue
                handle.write(
                    f"{index}\t{x:.9f}\t{y:.9f}\t{z:.9f}\t<closed-remesh>\t{bone}\t{weight:.9f}\n"
                )


def topology_metrics(
    vertices: list[tuple[float, float, float]], triangles: list[tuple[int, int, int]]
) -> dict[str, int]:
    edge_faces: dict[tuple[int, int], list[int]] = defaultdict(list)
    degenerate = 0
    for face_index, (a, b, c) in enumerate(triangles):
        if len({a, b, c}) < 3:
            degenerate += 1
            continue
        for u, v in ((a, b), (b, c), (c, a)):
            edge_faces[(u, v) if u <= v else (v, u)].append(face_index)
    boundary_edges = [edge for edge, faces in edge_faces.items() if len(faces) == 1]
    nonmanifold_edges = [edge for edge, faces in edge_faces.items() if len(faces) > 2]
    face_adjacency = [set() for _ in triangles]
    for faces in edge_faces.values():
        for i in range(len(faces)):
            for j in range(i + 1, len(faces)):
                face_adjacency[faces[i]].add(faces[j])
                face_adjacency[faces[j]].add(faces[i])
    seen = [False] * len(triangles)
    components = 0
    for start in range(len(triangles)):
        if seen[start]:
            continue
        components += 1
        queue = [start]
        seen[start] = True
        while queue:
            face = queue.pop()
            for neighbor in face_adjacency[face]:
                if not seen[neighbor]:
                    seen[neighbor] = True
                    queue.append(neighbor)
    boundary_adjacency: dict[int, list[int]] = defaultdict(list)
    for a, b in boundary_edges:
        boundary_adjacency[a].append(b)
        boundary_adjacency[b].append(a)
    boundary_seen: set[int] = set()
    boundary_components = 0
    for start in list(boundary_adjacency):
        if start in boundary_seen:
            continue
        boundary_components += 1
        queue = [start]
        boundary_seen.add(start)
        while queue:
            vertex = queue.pop()
            for neighbor in boundary_adjacency[vertex]:
                if neighbor not in boundary_seen:
                    boundary_seen.add(neighbor)
                    queue.append(neighbor)
    return {
        "vertices": len(vertices),
        "triangles": len(triangles),
        "triangle_components": components,
        "boundary_edges": len(boundary_edges),
        "boundary_components": boundary_components,
        "nonmanifold_edges": len(nonmanifold_edges),
        "degenerate_faces": degenerate,
    }


def main() -> int:
    load_blender_module()
    args = parse_args()
    source_positions, source_weights = read_weight_rows(args.input_weights)
    obj = import_obj(args.input_obj)
    remesh_closed(obj, args)
    vertices, triangles, removed_components = write_obj(
        args.output_obj, obj, args.min_component_faces
    )
    transferred = transfer_weights(vertices, source_positions, source_weights, args.nearest)
    write_weights(args.output_weights, vertices, transferred)
    topology = topology_metrics(vertices, triangles)
    summary: dict[str, object] = {
        **topology,
        "input_obj": str(args.input_obj),
        "input_weights": str(args.input_weights),
        "output_obj": str(args.output_obj),
        "output_weights": str(args.output_weights),
        "voxel_size": args.voxel_size,
        "target_vertices": args.target_vertices,
        "target_triangles": args.target_triangles,
        "smooth_iterations": args.smooth_iterations,
        "min_component_faces": args.min_component_faces,
        "removed_small_components": removed_components,
        "nearest": args.nearest,
    }
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.summary.write_text(json.dumps(summary, indent=2), encoding="utf-8")
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
