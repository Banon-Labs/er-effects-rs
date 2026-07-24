#!/usr/bin/env python3
"""Build a Blender comparison scene for Mushroom Man human-in-loop review.

Run this with Blender's Python via `blender --background --python ... -- <args>`.
It imports the compensated Mushroom Man OBJ and the original player body OBJ,
keeps an origin-accurate overlay, and adds side-by-side duplicates for scale.
"""

from __future__ import annotations

import argparse
import importlib
import sys
from pathlib import Path
from typing import Any, cast


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mushroom-obj", type=Path, required=True)
    parser.add_argument("--human-obj", type=Path, required=True)
    parser.add_argument("--output-blend", type=Path, required=True)
    return parser.parse_args(argv)


def import_obj(bpy, path: Path, name: str):
    before = set(bpy.context.scene.objects)
    if hasattr(bpy.ops.wm, "obj_import"):
        bpy.ops.wm.obj_import(filepath=str(path))
    else:
        bpy.ops.import_scene.obj(filepath=str(path))
    imported = [obj for obj in bpy.context.scene.objects if obj not in before]
    if not imported:
        raise RuntimeError(f"Blender imported no objects from {path}")
    if len(imported) == 1:
        obj = imported[0]
        obj.name = name
        obj.data.name = f"{name}Mesh"
        return obj
    parent = bpy.data.objects.new(name, None)
    bpy.context.collection.objects.link(parent)
    for obj in imported:
        obj.parent = parent
    return parent


def make_material(
    bpy, name: str, color: tuple[float, float, float, float], alpha: float = 1.0
):
    mat = bpy.data.materials.new(name)
    mat.diffuse_color = color
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes.get("Principled BSDF")
    if bsdf is not None:
        bsdf.inputs["Base Color"].default_value = color
        bsdf.inputs["Alpha"].default_value = alpha
    mat.blend_method = "BLEND" if alpha < 1.0 else "OPAQUE"
    mat.show_transparent_back = alpha < 1.0
    return mat


def assign_material(obj, mat) -> None:
    targets = [obj] + list(obj.children_recursive)
    for target in targets:
        if getattr(target, "type", None) == "MESH":
            target.data.materials.clear()
            target.data.materials.append(mat)


def set_display(
    obj, display_type: str, show_wire: bool = False, show_in_front: bool = False
) -> None:
    for target in [obj] + list(obj.children_recursive):
        target.display_type = display_type
        target.show_wire = show_wire
        target.show_in_front = show_in_front


def link_to_collection(bpy, obj, collection_name: str) -> None:
    collection = bpy.data.collections.get(collection_name)
    if collection is None:
        collection = bpy.data.collections.new(collection_name)
        bpy.context.scene.collection.children.link(collection)
    for existing in obj.users_collection:
        existing.objects.unlink(obj)
    collection.objects.link(obj)


def duplicate_object_tree(bpy, obj, name: str, location_x: float):
    duplicate = obj.copy()
    duplicate.data = obj.data.copy() if getattr(obj, "data", None) else None
    bpy.context.collection.objects.link(duplicate)
    duplicate.name = name
    duplicate.location.x += location_x
    for child in obj.children:
        child_copy = child.copy()
        child_copy.data = child.data.copy() if getattr(child, "data", None) else None
        bpy.context.collection.objects.link(child_copy)
        child_copy.parent = duplicate
        child_copy.matrix_parent_inverse = child.matrix_parent_inverse.copy()
    return duplicate


def add_label(bpy, text: str, location: tuple[float, float, float]) -> None:
    bpy.ops.object.text_add(location=location, rotation=(1.5708, 0.0, 0.0))
    obj = bpy.context.object
    obj.name = text
    obj.data.body = text
    obj.data.align_x = "CENTER"
    obj.data.size = 0.12


def configure_scene(bpy) -> None:
    bpy.ops.object.light_add(type="AREA", location=(0.0, -3.0, 3.5))
    light = bpy.context.object
    light.name = "Large soft inspection light"
    light.data.energy = 500
    light.data.size = 5
    bpy.ops.object.camera_add(location=(0.0, -4.5, 1.25), rotation=(1.35, 0.0, 0.0))
    bpy.context.scene.camera = bpy.context.object
    bpy.context.scene.render.resolution_x = 1600
    bpy.context.scene.render.resolution_y = 1000
    bpy.context.scene.view_settings.view_transform = "Filmic"


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    bpy = cast(Any, importlib.import_module("bpy"))

    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete()

    mushroom_mat = make_material(
        bpy, "Mushroom compensated body", (1.0, 0.48, 0.16, 1.0)
    )
    human_mat = make_material(
        bpy, "Original player body transparent", (0.15, 0.45, 1.0, 0.27), 0.27
    )

    mushroom = import_obj(
        bpy, args.mushroom_obj, "Mushroom compensated actual FLVER mesh13"
    )
    human = import_obj(
        bpy, args.human_obj, "Original FC_M_0000 player body meshes 0-13"
    )
    assign_material(mushroom, mushroom_mat)
    assign_material(human, human_mat)
    set_display(human, "WIRE", show_wire=True, show_in_front=True)
    link_to_collection(bpy, mushroom, "Origin overlay - true game coordinates")
    link_to_collection(bpy, human, "Origin overlay - true game coordinates")

    side_mushroom = duplicate_object_tree(
        bpy, mushroom, "Side mushroom compensated", 1.35
    )
    side_human = duplicate_object_tree(bpy, human, "Side original player body", -1.35)
    assign_material(side_mushroom, mushroom_mat)
    assign_material(side_human, human_mat)
    set_display(side_human, "TEXTURED", show_wire=True, show_in_front=False)
    link_to_collection(bpy, side_mushroom, "Side by side scale comparison")
    link_to_collection(bpy, side_human, "Side by side scale comparison")

    add_label(bpy, "Original player body", (-1.35, -0.35, 1.9))
    add_label(bpy, "Compensated mushroom", (1.35, -0.35, 1.9))
    add_label(
        bpy,
        "Overlay collection: toggle in Outliner for true-origin alignment",
        (0.0, -0.35, 2.15),
    )
    configure_scene(bpy)

    args.output_blend.parent.mkdir(parents=True, exist_ok=True)
    bpy.ops.wm.save_as_mainfile(filepath=str(args.output_blend))
    print(f"wrote {args.output_blend}")
    return 0


if __name__ == "__main__":
    if "--" in sys.argv:
        script_args = sys.argv[sys.argv.index("--") + 1 :]
    else:
        script_args = sys.argv[1:]
    raise SystemExit(main(script_args))
