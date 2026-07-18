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
from pathlib import Path

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


def vertex_uvs(mesh) -> list[tuple[float, float]]:
    uvs = [(0.0, 0.0) for _ in mesh.vertices]
    if not mesh.uv_layers:
        return uvs
    layer = mesh.uv_layers.active.data
    seen = set()
    for polygon in mesh.polygons:
        for vertex_index, loop_index in zip(
            polygon.vertices, polygon.loop_indices, strict=True
        ):  # pi-lens-ignore: python-thread-global-write — local UV scan, no threading
            if vertex_index in seen:
                continue
            uv = layer[loop_index].uv
            uvs[vertex_index] = (float(uv.x), float(uv.y))
            seen.add(vertex_index)
    return uvs


def write_obj(
    path: Path,
    obj,
    positions: list[tuple[float, float, float]],
    normals: list[tuple[float, float, float]],
    uvs: list[tuple[float, float]],
) -> None:
    with path.open("w", encoding="utf-8") as file:
        file.write(
            "# Exported from Blender EDIT_ME_c2280 for er-effects-rs donor patching\n"
        )  # pi-lens-ignore: python-thread-global-write — sequential file write, no threading
        file.write(
            "o blender_edit_c2280\n"
        )  # pi-lens-ignore: python-thread-global-write — sequential file write, no threading
        for position in positions:  # pi-lens-ignore: python-thread-global-write — sequential export loop, no threading
            file.write(f"v {position[0]:.9f} {position[1]:.9f} {position[2]:.9f}\n")
        for uv in uvs:
            file.write(f"vt {uv[0]:.9f} {uv[1]:.9f}\n")
        for normal in normals:
            file.write(f"vn {normal[0]:.9f} {normal[1]:.9f} {normal[2]:.9f}\n")
        for polygon in obj.data.polygons:
            indices = [vertex_index + 1 for vertex_index in polygon.vertices]
            file.write(
                "f " + " ".join(f"{index}/{index}/{index}" for index in indices) + "\n"
            )


def write_weights(
    path: Path, obj, positions: list[tuple[float, float, float]]
) -> dict[str, int]:
    bbox_min_y = min(position[1] for position in positions)
    bbox_max_y = max(position[1] for position in positions)
    counts: dict[str, int] = {}
    with path.open("w", encoding="utf-8") as file:
        file.write(
            "vertex\tsource_x\tsource_y\tsource_z\tsource_bone\ter_target_bone\tweight\n"
        )
        for vertex in obj.data.vertices:
            position = positions[vertex.index]
            accum: dict[str, float] = {}
            for group_ref in vertex.groups:
                group_name = obj.vertex_groups[group_ref.group].name
                target = er_target_for_source_group(
                    group_name, position, bbox_min_y, bbox_max_y
                )
                if target.startswith("<") or group_ref.weight <= 0.0:
                    continue
                accum[target] = accum.get(target, 0.0) + float(group_ref.weight)
            if not accum:
                accum["Spine2"] = 1.0
            for target, weight in sorted(accum.items(), key=lambda item: item[0]):
                counts[target] = counts.get(target, 0) + 1
                file.write(
                    f"{vertex.index}\t{position[0]:.9f}\t{position[1]:.9f}\t{position[2]:.9f}\t<blender>\t{target}\t{weight:.9f}\n"
                )
    return counts


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
    mesh = obj.data
    ensure_triangles(mesh)
    positions = [
        blender_to_flver(obj.matrix_world @ vertex.co) for vertex in mesh.vertices
    ]
    normals = [transformed_normal(obj, vertex.normal) for vertex in mesh.vertices]
    uvs = vertex_uvs(mesh)
    obj_path = output_dir / "blender_edit_c2280.obj"
    weights_path = output_dir / "blender_edit_c2280_weights.tsv"
    summary_path = output_dir / "blender_edit_c2280_summary.json"
    write_obj(obj_path, obj, positions, normals, uvs)
    weight_counts = write_weights(weights_path, obj, positions)
    summary = {
        "blend_file": bpy.data.filepath,
        "object": obj.name,
        "vertex_count": len(mesh.vertices),
        "polygon_count": len(mesh.polygons),
        "bbox": bbox_for(positions),
        "weight_target_counts": weight_counts,
        "obj": str(obj_path),
        "weights": str(weights_path),
    }
    summary_path.write_text(
        json.dumps(summary, indent=2), encoding="utf-8"
    )  # pi-lens-ignore: python-path-traversal — output dir supplied by build script under target/
    out(f"wrote {obj_path}")
    out(f"wrote {weights_path}")
    out(f"wrote {summary_path}")
    out(
        f"vertices={summary['vertex_count']} polygons={summary['polygon_count']} bbox_dims={summary['bbox']['dims']}"
    )
    bpy.ops.wm.quit_blender()


if __name__ == "__main__":
    main()
