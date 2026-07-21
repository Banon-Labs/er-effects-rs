# check-no-magic-numbers: allow-file -- Blender/OBJ export helper constants are authoring-pipeline configuration.
"""Export the editable raw mushroom Blender mesh to OBJ + ER-mapped weights.

Run inside Blender with `--python`, passing script arguments after `--`:

  blender --background mushroom_raw_game_compare.blend \
    --python scripts/export_mushroom_blender_edit.py -- --output-dir target/...

The exported OBJ uses FLVER-style axes (X right, Y up, Z depth) converted from
Blender's world-space axes. Weight TSV maps DSR c2280 vertex groups to ER player
bones for the existing `route_a_mushroom_patch_donor.rs` patcher.
"""

from __future__ import annotations

import argparse
import json
import sys
from collections import deque
from pathlib import Path
from typing import TypedDict

bpy = __import__("bpy")
mathutils = __import__("mathutils")
Vector = mathutils.__dict__["Vector"]

DEFAULT_OBJECT_NAME = "EDIT_ME_c2280"


def out(message: str) -> None:
    sys.stdout.write(message + "\n")
    sys.stdout.flush()


def parse_script_args() -> argparse.Namespace:
    argv = sys.argv
    script_args = (
        argv[argv.index("--") + 1 :] if "--" in argv else []
    )  # pi-lens-ignore: python-thread-global-write — local argv slice, no threading
    parser = argparse.ArgumentParser()  # pi-lens-ignore: python-thread-global-write — local parser construction, no threading
    parser.add_argument(
        "--object-name", default=DEFAULT_OBJECT_NAME
    )  # pi-lens-ignore: python-thread-global-write — local parser setup, no threading
    parser.add_argument(
        "--output-dir", required=True
    )  # pi-lens-ignore: python-thread-global-write — local parser setup, no threading
    parser.add_argument(
        "--allow-zero-uv",
        action="store_true",
        help="allow exporting with fallback zero UVs; normally this fails closed to protect texture mapping",
    )  # pi-lens-ignore: python-thread-global-write — local parser setup, no threading
    parser.add_argument(
        "--max-source-vertices",
        type=int,
        help="apply a temporary Blender decimate modifier until the editable mesh has at most this many source vertices",
    )  # pi-lens-ignore: python-thread-global-write — local parser setup, no threading
    parser.add_argument(
        "--weight-mode",
        choices=("procedural", "source"),
        default="procedural",
        help="procedural assigns ER bones from mushroom geometry; source maps Blender vertex groups directly",
    )  # pi-lens-ignore: python-thread-global-write — local parser setup, no threading
    return parser.parse_args(
        script_args
    )  # pi-lens-ignore: python-thread-global-write — local parser result, no threading


def blender_to_flver(vector) -> tuple[float, float, float]:
    return (float(vector.x), float(vector.z), float(vector.y))


def transformed_normal(obj, normal) -> tuple[float, float, float]:
    normal_matrix = obj.matrix_world.to_3x3().inverted().transposed()
    transformed = (
        normal_matrix @ normal
    )  # pi-lens-ignore: python-thread-global-write — local vector math, no threading
    transformed.normalize()  # pi-lens-ignore: python-thread-global-write — local vector normalization, no threading
    return blender_to_flver(
        transformed
    )  # pi-lens-ignore: python-thread-global-write — local tuple conversion, no threading


def normalized_y(
    flver_position: tuple[float, float, float], bbox_min_y: float, bbox_max_y: float
) -> float:  # pi-lens-ignore: python-thread-global-write — pure helper, no threading
    height = bbox_max_y - bbox_min_y
    if abs(height) < 0.000001:
        return 0.5
    return (
        (flver_position[1] - bbox_min_y) / height
    )  # pi-lens-ignore: python-thread-global-write — local arithmetic, no threading


def er_target_for_source_group(
    name: str,
    flver_position: tuple[float, float, float],
    bbox_min_y: float,
    bbox_max_y: float,
) -> (
    str
):  # pi-lens-ignore: python-thread-global-write — pure mapping helper, no threading
    y_norm = normalized_y(flver_position, bbox_min_y, bbox_max_y)
    match name:
        case "Pelvis":
            return "Pelvis"
        case "Spine1":
            return "Spine"
        case (
            "Spine2"
        ):  # pi-lens-ignore: python-thread-global-write — pattern match, no threading
            return "Spine1"
        case (
            "Spine3"
        ):  # pi-lens-ignore: python-thread-global-write — pattern match, no threading
            return "Spine2"  # pi-lens-ignore: python-thread-global-write — mapping return, no threading
        case (
            "Neck"
        ):  # pi-lens-ignore: python-thread-global-write — pattern match, no threading
            return "Neck"
        case (
            "Head"
        ):  # pi-lens-ignore: python-thread-global-write — pattern match, no threading
            return "Head"
        case "LArm1":
            return "L_UpperArm"
        case "LArm2":
            return "L_Forearm"
        case (
            "LArmPalm"
            | "LArmDigit11"
            | "LArmDigit12"
            | "LArmDigit21"
            | "LArmDigit22"
            | "LArmDigit31"
            | "LArmDigit32"
        ):
            return "L_Hand"
        case "RArm1":
            return "R_UpperArm"
        case "RArm2":
            return "R_Forearm"
        case (
            "RArmPalm"
            | "RArmDigit11"
            | "RArmDigit12"
            | "RArmDigit21"
            | "RArmDigit22"
            | "RArmDigit31"
            | "RArmDigit32"
        ):
            return "R_Hand"
        case "LLeg1":
            if y_norm < 0.10:
                return "L_Foot"
            if y_norm < 0.24:
                return "L_Calf"
            return "L_Thigh"
        case "RLeg1":
            if y_norm < 0.10:
                return "R_Foot"
            if y_norm < 0.24:
                return "R_Calf"
            return "R_Thigh"
        case "c2280" | "Model_Dmy" | "sfx_dummy" | "固定dmy" | "master":
            return "<unused>"
        case _:
            return "Spine2"


def find_object(name: str):
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
    editable = [
        candidate
        for candidate in bpy.context.scene.objects
        if candidate.type == "MESH"
        and "EDIT ME"
        in " ".join(collection.name for collection in candidate.users_collection)
    ]
    if len(editable) == 1:
        return editable[0]
    raise ValueError(
        f"could not uniquely find editable mesh {name!r}; candidates={[candidate.name for candidate in editable]}"
    )


def ensure_triangles(mesh) -> None:
    non_triangles = [poly.index for poly in mesh.polygons if len(poly.vertices) != 3]
    if non_triangles:
        raise ValueError(
            f"editable mesh must be triangulated; non-triangle polygons={non_triangles[:10]}"
        )  # pi-lens-ignore: python-thread-global-write — exception construction, no threading


def export_vertex_count(obj, allow_zero_uv: bool) -> int:
    export_vertices, _export_triangles, _uv_source = build_export_geometry(
        obj, allow_zero_uv
    )
    return len(export_vertices)


def decimate_to_vertex_budget(
    obj, max_source_vertices: int | None, allow_zero_uv: bool
) -> dict[str, object]:
    before_source = len(obj.data.vertices)
    before_export = export_vertex_count(obj, allow_zero_uv)
    if max_source_vertices is None:
        return {
            "requested": None,
            "applied": False,
            "source_vertices_before": before_source,
            "source_vertices_after": before_source,
            "export_vertices_before": before_export,
            "export_vertices_after": before_export,
            "ratio": 1.0,
        }
    if max_source_vertices <= 0:
        raise ValueError("--max-source-vertices must be greater than zero")

    if before_export <= max_source_vertices:
        return {
            "requested": max_source_vertices,
            "applied": False,
            "source_vertices_before": before_source,
            "source_vertices_after": before_source,
            "export_vertices_before": before_export,
            "export_vertices_after": before_export,
            "ratio": 1.0,
        }

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
    ratio = min(1.0, max_source_vertices / before_export)
    for attempt in range(8):
        modifier = obj.modifiers.new(f"MushroomMan_vertex_budget_{attempt}", "DECIMATE")
        modifier.ratio = ratio
        modifier.use_collapse_triangulate = True
        with bpy.context.temp_override(
            object=obj,
            active_object=obj,
            selected_objects=[obj],
            selected_editable_objects=[obj],
        ):
            bpy.ops.object.modifier_apply(modifier=modifier.name)
        ensure_triangles(obj.data)
        after_export = export_vertex_count(obj, allow_zero_uv)
        if after_export <= max_source_vertices:
            return {
                "requested": max_source_vertices,
                "applied": True,
                "source_vertices_before": before_source,
                "source_vertices_after": len(obj.data.vertices),
                "export_vertices_before": before_export,
                "export_vertices_after": after_export,
                "ratio": ratio,
            }
        ratio *= max_source_vertices / after_export * 0.995

    raise ValueError(
        f"decimation could not fit export vertex budget {max_source_vertices}; before={before_export} after={export_vertex_count(obj, allow_zero_uv)}"
    )


def uv_layer_data(mesh):
    if not mesh.uv_layers or mesh.uv_layers.active is None:
        return None
    layer_data = mesh.uv_layers.active.data
    return layer_data if len(layer_data) > 0 else None


def is_raw_mushroom_reference(candidate) -> bool:
    collection_text = " ".join(
        collection.name for collection in candidate.users_collection
    ).lower()
    candidate_name = candidate.name.lower()
    return (
        candidate_name in {"c2270", "c2280"}
        or "raw c2270" in collection_text
        or "raw c2280" in collection_text
        or (
            "raw" in collection_text
            and "reference" in collection_text
            and ("mushroom" in collection_text or "dsr" in collection_text)
        )
    )


def find_uv_reference_mesh(obj):
    source_mesh = obj.data
    source_loop_count = len(source_mesh.loops)
    for candidate in bpy.context.scene.objects:
        if candidate == obj or candidate.type != "MESH":
            continue
        if not is_raw_mushroom_reference(candidate):
            continue
        if len(candidate.data.loops) != source_loop_count:
            continue
        if uv_layer_data(candidate.data) is not None:
            return candidate.data
    return None


def uv_for_loop(mesh, loop_index: int, reference_mesh=None) -> tuple[float, float]:
    layer_data = uv_layer_data(mesh)
    if layer_data is None and reference_mesh is not None:
        layer_data = uv_layer_data(reference_mesh)
    if layer_data is None or loop_index >= len(layer_data):
        return (0.0, 0.0)
    uv = layer_data[loop_index].uv
    return (float(uv.x), 1.0 - float(uv.y))


def export_key(
    source_index: int, uv: tuple[float, float], normal: tuple[float, float, float]
) -> tuple[int, int, int, int, int, int]:
    return (
        source_index,
        round(uv[0] * 1_000_000),
        round(uv[1] * 1_000_000),
        round(normal[0] * 1_000_000),
        round(normal[1] * 1_000_000),
        round(normal[2] * 1_000_000),
    )


def build_export_geometry(
    obj, allow_zero_uv: bool
) -> tuple[list[dict], list[list[int]], str]:
    mesh = obj.data
    reference_mesh = find_uv_reference_mesh(obj)
    uv_source = (
        "editable"
        if uv_layer_data(mesh) is not None
        else "raw-reference"
        if reference_mesh
        else "fallback-zero"
    )
    if uv_source == "fallback-zero" and not allow_zero_uv:
        raise ValueError(
            "editable mesh has no UV layer and no matching raw mushroom reference UVs; "
            "refusing to export zero UVs because textures would not map correctly"
        )
    export_vertices: list[dict] = []
    export_lookup: dict[tuple[int, int, int, int, int, int], int] = {}
    export_triangles: list[list[int]] = []
    for polygon in mesh.polygons:
        triangle: list[int] = []
        for source_index, loop_index in zip(
            polygon.vertices, polygon.loop_indices, strict=True
        ):
            vertex = mesh.vertices[source_index]
            position = blender_to_flver(obj.matrix_world @ vertex.co)
            normal = transformed_normal(obj, mesh.loops[loop_index].normal)
            uv = uv_for_loop(mesh, loop_index, reference_mesh)
            key = export_key(source_index, uv, normal)
            export_index = export_lookup.get(key)
            if (
                export_index is None
            ):  # pi-lens-ignore: python-thread-global-write — local cache check, no threading
                export_index = len(export_vertices)
                export_lookup[key] = export_index
                export_vertices.append(  # pi-lens-ignore: python-thread-global-write — local export list append, no threading
                    {
                        "source_index": source_index,
                        "position": position,
                        "normal": normal,
                        "uv": uv,
                    }
                )
            triangle.append(export_index)
        export_triangles.append(
            triangle
        )  # pi-lens-ignore: python-thread-global-write — local triangle list append, no threading
    return (
        export_vertices,
        export_triangles,
        uv_source,
    )  # pi-lens-ignore: python-thread-global-write — local tuple return, no threading


def write_obj(
    path: Path, export_vertices: list[dict], export_triangles: list[list[int]]
) -> None:
    with path.open("w", encoding="utf-8") as file:
        file.write(
            "# Exported from Blender EDIT_ME_c2280 for er-effects-rs donor patching\n"
        )  # pi-lens-ignore: python-thread-global-write — sequential file write, no threading
        file.write(
            "o blender_edit_c2280\n"
        )  # pi-lens-ignore: python-thread-global-write — sequential file write, no threading
        for export_vertex in export_vertices:  # pi-lens-ignore: python-thread-global-write — sequential export loop, no threading
            position = export_vertex["position"]
            file.write(f"v {position[0]:.9f} {position[1]:.9f} {position[2]:.9f}\n")
        for export_vertex in export_vertices:
            uv = export_vertex["uv"]
            file.write(f"vt {uv[0]:.9f} {uv[1]:.9f}\n")
        for export_vertex in export_vertices:
            normal = export_vertex["normal"]
            file.write(f"vn {normal[0]:.9f} {normal[1]:.9f} {normal[2]:.9f}\n")
        for triangle in export_triangles:
            indices = [export_index + 1 for export_index in triangle]
            file.write(
                "f " + " ".join(f"{index}/{index}/{index}" for index in indices) + "\n"
            )


def clamp01(value: float) -> float:
    return max(0.0, min(1.0, value))


def side_prefix_for_x(x: float) -> str:
    # Existing raw DSR c2270/c2280 arm groups export with positive X as L_Hand
    # and negative X as R_Hand after Blender->FLVER conversion.
    return "L" if x >= 0.0 else "R"


def smoothstep(edge0: float, edge1: float, value: float) -> float:
    if abs(edge1 - edge0) < 0.000001:
        return 1.0 if value >= edge1 else 0.0
    t = clamp01((value - edge0) / (edge1 - edge0))
    return t * t * (3.0 - 2.0 * t)


def normalize_weights(weights: dict[str, float]) -> dict[str, float]:
    total = sum(value for value in weights.values() if value > 0.0)
    if total <= 0.0:
        return {"Spine2": 1.0}
    return {bone: value / total for bone, value in weights.items() if value > 0.000001}


def source_adjacency(obj) -> list[list[int]]:
    adjacency = [set() for _vertex in obj.data.vertices]
    for edge in obj.data.edges:
        a, b = edge.vertices
        adjacency[a].add(b)
        adjacency[b].add(a)
    return [sorted(neighbors) for neighbors in adjacency]


def side_filtered_weights(
    weights: dict[str, float], position: tuple[float, float, float]
) -> dict[str, float]:
    if position[0] > 0.001:
        filtered = {
            bone: value for bone, value in weights.items() if not bone.startswith("R_")
        }
    elif position[0] < -0.001:
        filtered = {
            bone: value for bone, value in weights.items() if not bone.startswith("L_")
        }
    else:
        filtered = {
            bone: value
            for bone, value in weights.items()
            if not bone.startswith("L_") and not bone.startswith("R_")
        }
    return normalize_weights(filtered)


def smooth_source_weight_maps(
    weights_by_source: list[dict[str, float]],
    source_positions: list[tuple[float, float, float]],
    adjacency: list[list[int]],
    protected_source_indices: set[int],
    iterations: int = 4,
) -> list[dict[str, float]]:
    smoothed = [dict(weights) for weights in weights_by_source]
    for _iteration in range(iterations):
        next_weights: list[dict[str, float]] = []
        for index, weights in enumerate(smoothed):
            if index in protected_source_indices or not adjacency[index]:
                next_weights.append(weights)
                continue
            mixed: dict[str, float] = {
                bone: value * 0.72 for bone, value in weights.items()
            }
            neighbor_scale = 0.28 / len(adjacency[index])
            for neighbor in adjacency[index]:
                for bone, value in smoothed[neighbor].items():
                    mixed[bone] = mixed.get(bone, 0.0) + value * neighbor_scale
            next_weights.append(side_filtered_weights(mixed, source_positions[index]))
        smoothed = next_weights
    return smoothed


def role_priority(role: str) -> int:
    return {
        "cap": 5,
        "upper_detached": 4,
        "upper_side": 4,
        "foot": 3,
        "arm": 2,
        "trunk": 1,
    }.get(role, 0)


class ComponentProfile(TypedDict):
    indices: list[int]
    size: int
    avg_x: float
    avg_y_norm: float
    min_y_norm: float
    max_y_norm: float
    avg_x_norm_abs: float
    side: str
    role: str


def connected_component_profiles(
    export_vertices: list[dict],
    export_triangles: list[list[int]],
    bbox: dict[str, list[float]],
) -> list[ComponentProfile]:
    adjacency = [set() for _vertex in export_vertices]
    for triangle in export_triangles:
        if len(triangle) != 3:
            continue
        a, b, c = triangle
        adjacency[a].update((b, c))
        adjacency[b].update((a, c))
        adjacency[c].update((a, b))

    min_y = bbox["min"][1]
    height = bbox["dims"][1]
    max_abs_x = max(abs(bbox["min"][0]), abs(bbox["max"][0]))
    profiles: list[ComponentProfile | None] = [None for _vertex in export_vertices]
    remaining = set(range(len(export_vertices)))
    component_infos: list[ComponentProfile] = []
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
        positions = [export_vertices[index]["position"] for index in component]
        avg_x = sum(position[0] for position in positions) / len(positions)
        avg_y = sum(position[1] for position in positions) / len(positions)
        min_component_y = min(position[1] for position in positions)
        max_component_y = max(position[1] for position in positions)
        info: ComponentProfile = {
            "indices": component,
            "size": len(component),
            "avg_x": avg_x,
            "avg_y_norm": clamp01((avg_y - min_y) / height) if height > 0.0 else 0.5,
            "min_y_norm": clamp01((min_component_y - min_y) / height)
            if height > 0.0
            else 0.0,
            "max_y_norm": clamp01((max_component_y - min_y) / height)
            if height > 0.0
            else 1.0,
            "avg_x_norm_abs": abs(avg_x) / max_abs_x if max_abs_x > 0.0 else 0.0,
            "side": side_prefix_for_x(avg_x),
            "role": "trunk",
        }
        component_infos.append(info)

    cap_candidates = [
        info
        for info in component_infos
        if info["avg_y_norm"] > 0.78
        and info["avg_x_norm_abs"] < 0.25
        and info["size"] >= 32
    ]
    cap_component = max(
        cap_candidates or component_infos,
        key=lambda info: (info["avg_y_norm"], info["size"]),
    )
    for info in component_infos:
        role = "trunk"
        if info is cap_component:
            role = "cap"
        elif (info["avg_y_norm"] < 0.13 and info["avg_x_norm_abs"] > 0.10) or (
            info["max_y_norm"] < 0.20 and info["avg_x_norm_abs"] > 0.10
        ):
            role = "foot"
        elif info["size"] <= 12 and info["min_y_norm"] > 0.65:
            role = "upper_detached"
        elif info["min_y_norm"] > 0.62:
            role = "upper_side"
        elif 0.35 <= info["avg_y_norm"] <= 0.62 and info["avg_x_norm_abs"] > 0.45:
            role = "arm"
        info["role"] = role
        for index in info["indices"]:
            profiles[index] = info
    return [profile if profile is not None else cap_component for profile in profiles]


def procedural_mushroom_weights(
    position: tuple[float, float, float],
    bbox: dict[str, list[float]],
    component_info: ComponentProfile | None,
) -> tuple[str, dict[str, float]]:
    min_x, min_y, _min_z = bbox["min"]
    max_x, max_y, _max_z = bbox["max"]
    height = max_y - min_y
    if height <= 0.0:
        return "fallback", {"Spine2": 1.0}
    y_norm = clamp01((position[1] - min_y) / height)
    max_abs_x = max(abs(min_x), abs(max_x))
    x_norm_abs = abs(position[0]) / max_abs_x if max_abs_x > 0.0 else 0.0
    side = side_prefix_for_x(position[0])
    if component_info is not None:
        component_role = component_info["role"]
        component_side = component_info["side"]
        if component_role == "cap":
            return "cap_component", normalize_weights({"Spine2": 1.0})
        if component_role in {"upper_detached", "upper_side"}:
            return component_role, normalize_weights({"Spine2": 1.0})
        if component_role == "foot":
            side = component_side
            x_norm_abs = max(x_norm_abs, 0.16)
        if component_role == "arm":
            side = component_side

    # Feet/leg blobs are the low *outer* side lobes. Keep the center/midline
    # base on pelvis/spine instead of assigning it to either foot; otherwise
    # ankle/foot movement visibly bleeds across the mushroom midline.
    if y_norm < 0.20 and x_norm_abs > 0.05:
        foot = smoothstep(0.20, 0.02, y_norm)
        return (
            "foot",
            normalize_weights(
                {
                    f"{side}_Foot": 0.55 + 0.35 * foot,
                    f"{side}_Calf": 0.45 - 0.35 * foot,
                }
            ),
        )
    if y_norm < 0.36 and x_norm_abs > 0.08:
        calf = smoothstep(0.36, 0.14, y_norm)
        return (
            "leg",
            normalize_weights(
                {
                    f"{side}_Calf": 0.45 + 0.35 * calf,
                    f"{side}_Thigh": 0.55 - 0.35 * calf,
                }
            ),
        )

    # Side protrusions in the middle of the mesh are the mushroom arms. Weight
    # along lateral reach, not by Blender source groups, so saved/manual vertex
    # groups are optional instead of required.
    arm_band = smoothstep(0.22, 0.34, y_norm) * (1.0 - smoothstep(0.70, 0.82, y_norm))
    arm_side = smoothstep(0.42, 0.58, x_norm_abs)
    if arm_band * arm_side > 0.20:
        reach = clamp01((x_norm_abs - 0.42) / 0.58)
        upper = max(0.0, 1.0 - reach * 2.2)
        hand = smoothstep(0.58, 0.94, reach)
        forearm = max(0.0, 1.0 - upper - hand)
        return (
            "arm",
            normalize_weights(
                {
                    f"{side}_UpperArm": 0.10 + 0.80 * upper,
                    f"{side}_Forearm": 0.15 + 0.80 * forearm,
                    f"{side}_Hand": 0.05 + 0.85 * hand,
                }
            ),
        )

    if y_norm < 0.24:
        return "center_base", normalize_weights({"Pelvis": 0.82, "Spine": 0.18})
    if y_norm < 0.44:
        t = smoothstep(0.24, 0.44, y_norm)
        return (
            "lower_trunk",
            normalize_weights(
                {"Pelvis": 0.35 * (1.0 - t), "Spine": 0.55, "Spine1": 0.45 * t}
            ),
        )
    if y_norm < 0.64:
        t = smoothstep(0.44, 0.64, y_norm)
        return (
            "mid_trunk",
            normalize_weights(
                {"Spine": 0.30 * (1.0 - t), "Spine1": 0.55, "Spine2": 0.45 * t}
            ),
        )
    if y_norm < 0.78:
        t = smoothstep(0.64, 0.78, y_norm)
        head = 0.35 * t if component_info is None else 0.0
        return (
            "upper_trunk",
            normalize_weights(
                {"Spine1": 0.25 * (1.0 - t), "Spine2": 0.60, "Head": head}
            ),
        )
    if component_info is not None:
        return "upper_non_head", normalize_weights({"Spine2": 1.0})
    return "cap", normalize_weights({"Spine2": 1.0})


def write_weights(
    path: Path,
    obj,
    export_vertices: list[dict],
    export_triangles: list[list[int]],
    weight_mode: str,
) -> tuple[dict[str, int], dict[str, int]]:
    positions = [export_vertex["position"] for export_vertex in export_vertices]
    bbox = bbox_for(positions)
    source_positions = [
        blender_to_flver(obj.matrix_world @ source_vertex.co)
        for source_vertex in obj.data.vertices
    ]
    source_bbox = bbox_for(source_positions)
    bbox_min_y = source_bbox["min"][1]
    bbox_max_y = source_bbox["max"][1]
    component_by_source: list[ComponentProfile | None] = [
        None for _source_vertex in obj.data.vertices
    ]
    if weight_mode == "procedural":
        component_profiles = connected_component_profiles(
            export_vertices, export_triangles, bbox
        )
        for export_index, export_vertex in enumerate(export_vertices):
            source_index = export_vertex["source_index"]
            profile = component_profiles[export_index]
            current = component_by_source[source_index]
            if current is None or role_priority(profile["role"]) > role_priority(
                current["role"]
            ):
                component_by_source[source_index] = profile

    source_weight_maps: list[dict[str, float]] = []
    procedural_counts: dict[str, int] = {}
    for source_index, source_vertex in enumerate(obj.data.vertices):
        position = source_positions[source_index]
        accum: dict[str, float] = {}
        for group_ref in source_vertex.groups:
            group_name = obj.vertex_groups[group_ref.group].name
            target = er_target_for_source_group(
                group_name, position, bbox_min_y, bbox_max_y
            )
            if target.startswith("<") or group_ref.weight <= 0.0:
                continue
            accum[target] = accum.get(target, 0.0) + float(group_ref.weight)
        if weight_mode == "procedural":
            procedural_kind, procedural_weights = procedural_mushroom_weights(
                position, source_bbox, component_by_source[source_index]
            )
            procedural_counts[procedural_kind] = (
                procedural_counts.get(procedural_kind, 0) + 1
            )
            accum = procedural_weights
        elif not accum:
            accum["Spine2"] = 1.0
        else:
            accum = normalize_weights(accum)
        source_weight_maps.append(accum)

    if weight_mode == "procedural":
        protected_source_indices = {
            source_index
            for source_index, profile in enumerate(component_by_source)
            if profile is not None
            and profile["role"] in {"cap", "upper_detached", "upper_side"}
        }
        source_weight_maps = smooth_source_weight_maps(
            source_weight_maps,
            source_positions,
            source_adjacency(obj),
            protected_source_indices,
        )
        for source_index, profile in enumerate(component_by_source):
            if profile is not None and profile["role"] == "cap":
                continue
            head_weight = source_weight_maps[source_index].pop("Head", 0.0)
            if head_weight > 0.0:
                source_weight_maps[source_index]["Spine2"] = (
                    source_weight_maps[source_index].get("Spine2", 0.0) + head_weight
                )
                source_weight_maps[source_index] = normalize_weights(
                    source_weight_maps[source_index]
                )

    counts: dict[str, int] = {}
    with path.open("w", encoding="utf-8") as file:
        file.write(
            "vertex\tsource_x\tsource_y\tsource_z\tsource_bone\ter_target_bone\tweight\n"
        )
        for export_index, export_vertex in enumerate(export_vertices):
            position = export_vertex["position"]
            accum = source_weight_maps[export_vertex["source_index"]]
            for target, weight in sorted(accum.items(), key=lambda item: item[0]):
                counts[target] = counts.get(target, 0) + 1
                file.write(
                    f"{export_index}\t{position[0]:.9f}\t{position[1]:.9f}\t{position[2]:.9f}\t<blender-source>\t{target}\t{weight:.9f}\n"
                )
    return counts, procedural_counts


def bbox_for(positions: list[tuple[float, float, float]]) -> dict[str, list[float]]:
    mins = [min(position[index] for position in positions) for index in range(3)]
    maxs = [max(position[index] for position in positions) for index in range(3)]
    return {
        "min": mins,
        "max": maxs,
        "dims": [maxs[index] - mins[index] for index in range(3)],
        "center": [(mins[index] + maxs[index]) / 2.0 for index in range(3)],
    }


def main() -> None:
    args = parse_script_args()
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    obj = find_object(args.object_name)
    if obj.type != "MESH":
        raise TypeError(f"{obj.name} is {obj.type}, expected MESH")
    decimation = decimate_to_vertex_budget(
        obj, args.max_source_vertices, args.allow_zero_uv
    )
    mesh = obj.data
    ensure_triangles(mesh)
    export_vertices, export_triangles, uv_source = build_export_geometry(
        obj, args.allow_zero_uv
    )
    if (
        args.max_source_vertices is not None
        and len(export_vertices) > args.max_source_vertices
    ):
        raise ValueError(
            f"export vertex count {len(export_vertices)} exceeds requested budget {args.max_source_vertices}"
        )
    positions = [export_vertex["position"] for export_vertex in export_vertices]
    obj_path = output_dir / "blender_edit_c2280.obj"
    weights_path = output_dir / "blender_edit_c2280_weights.tsv"
    summary_path = output_dir / "blender_edit_c2280_summary.json"
    write_obj(obj_path, export_vertices, export_triangles)
    weight_counts, procedural_weight_counts = write_weights(
        weights_path, obj, export_vertices, export_triangles, args.weight_mode
    )
    summary = {
        "blend_file": bpy.data.filepath,
        "object": obj.name,
        "source_vertex_count": len(mesh.vertices),
        "export_vertex_count": len(export_vertices),
        "polygon_count": len(mesh.polygons),
        "decimation": decimation,
        "bbox": bbox_for(positions),
        "weight_mode": args.weight_mode,
        "weight_target_counts": weight_counts,
        "procedural_weight_counts": procedural_weight_counts,
        "uv_source": uv_source,
        "uv_v_flipped_for_flver": True,
        "obj": str(obj_path),
        "weights": str(weights_path),
    }
    summary_path.write_text(  # pi-lens-ignore: python-path-traversal — output dir supplied by build script under target/
        json.dumps(
            summary, indent=2
        ),  # pi-lens-ignore: python-path-traversal — data string for fixed summary_path write
        encoding="utf-8",  # pi-lens-ignore: python-path-traversal — data string for fixed summary_path write
    )
    out(f"wrote {obj_path}")
    out(f"wrote {weights_path}")
    out(f"wrote {summary_path}")
    out(
        f"source_vertices={summary['source_vertex_count']} export_vertices={summary['export_vertex_count']} polygons={summary['polygon_count']} bbox_dims={summary['bbox']['dims']}"
    )
    bpy.ops.wm.quit_blender()


if __name__ == "__main__":
    main()
