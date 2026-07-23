# check-no-magic-numbers: allow-file -- Blender scene layout and RGB constants are UI helper configuration, not reverse-engineered game offsets.
"""Create a clean raw-game Blender scene for mushroom/player proportion work.

This scene intentionally omits the generated Route A OBJ. It imports the ER
player reference and DSR c2280 exactly through their respective Soulstruct game
profiles, then adds an editable duplicate of the raw mushroom so manual shaping
starts from the original DSR asset instead of our pre-tweaked pipeline output.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import create_mushroom_blender_compare as base  # noqa: E402

RAW_BLEND_PATH = base.OUT_DIR / "mushroom_raw_game_compare.blend"
RAW_METRICS_PATH = base.OUT_DIR / "mushroom_raw_game_compare.metrics.json"
RAW_LOG_PATH = base.OUT_DIR / "mushroom_raw_game_compare.log"
RAW_OPEN_SCRIPT_PATH = base.OUT_DIR / "open_mushroom_raw_game_compare.ps1"

PLAYER_COLLECTION = "01 ER raw player FC_M_0000 reference"
RAW_REFERENCE_COLLECTION = "02 DSR raw c2280 reference LOCKED"
RAW_EDIT_COLLECTION = "03 EDIT ME raw DSR c2280 copy (no Route A knobs)"

COLOR_EDITABLE_RAW = (1.00, 0.55, 0.10, 0.55)


def write_open_script() -> None:
    RAW_OPEN_SCRIPT_PATH.write_text(  # pi-lens-ignore: python-thread-global-write python-path-traversal — fixed repo-local PowerShell helper path
        "& 'C:\\Program Files\\Blender Foundation\\Blender 4.4\\blender.exe' "  # pi-lens-ignore: python-thread-global-write — fixed script text, no threading
        f"'{RAW_BLEND_PATH}'\n",  # pi-lens-ignore: python-thread-global-write — fixed script text, no threading
        encoding="utf-8",
    )


def lock_objects(objects) -> None:
    for obj in objects:
        obj.hide_select = True


def duplicate_collection_objects(objects, collection_name: str):
    collection = base.make_collection(
        collection_name
    )  # pi-lens-ignore: python-thread-global-write — Blender scene mutation on main thread
    object_map = {}  # pi-lens-ignore: python-thread-global-write — local map, no threading
    duplicates = []  # pi-lens-ignore: python-thread-global-write — local list, no threading
    for obj in objects:
        duplicate = obj.copy()  # pi-lens-ignore: python-thread-global-write — Blender object duplicate on main thread
        if (
            obj.data is not None
        ):  # pi-lens-ignore: python-thread-global-write — Blender data-copy guard on main thread
            duplicate.data = obj.data.copy()
        duplicate.name = f"EDIT_ME_{obj.name}"
        object_map[obj] = duplicate
        duplicates.append(
            duplicate
        )  # pi-lens-ignore: python-thread-global-write — local list append, no threading
        collection.objects.link(
            duplicate
        )  # pi-lens-ignore: python-thread-global-write — Blender scene mutation on main thread
    for (
        source,
        duplicate,
    ) in (
        object_map.items()
    ):  # pi-lens-ignore: python-thread-global-write — local map iteration, no threading
        if (
            source.parent in object_map
        ):  # pi-lens-ignore: python-thread-global-write — local map lookup, no threading
            duplicate.parent = object_map[source.parent]
    return duplicates


def main() -> None:
    base.LOG_PATH = RAW_LOG_PATH
    base.prepare_dirs()  # pi-lens-ignore: python-thread-global-write — sequential setup, no threading
    base.require_path(
        base.RAW_MUSHROOM_FLVER
    )  # pi-lens-ignore: python-thread-global-write — fixed path check, no threading
    base.require_path(
        base.PLAYER_FLVER
    )  # pi-lens-ignore: python-thread-global-write — fixed path check, no threading

    base.enable_soulstruct()  # pi-lens-ignore: python-thread-global-write — Blender add-on setup on main thread
    base.clear_scene()  # pi-lens-ignore: python-thread-global-write — Blender scene mutation on main thread
    base.make_collection(
        base.BBOX_COLLECTION
    )  # pi-lens-ignore: python-thread-global-write — Blender scene mutation on main thread

    player_objects = base.import_flver(
        base.PLAYER_FLVER, "ELDEN_RING", PLAYER_COLLECTION, base.COLOR_PLAYER
    )
    raw_reference_objects = base.import_flver(
        base.RAW_MUSHROOM_FLVER,
        "DARK_SOULS_DSR",
        RAW_REFERENCE_COLLECTION,
        base.COLOR_RAW,
    )
    raw_edit_objects = duplicate_collection_objects(
        raw_reference_objects, RAW_EDIT_COLLECTION
    )

    lock_objects(player_objects)
    lock_objects(raw_reference_objects)
    base.color_mesh_objects(
        raw_edit_objects, COLOR_EDITABLE_RAW, display_type="TEXTURED"
    )

    metrics = {
        "player_fc_m_0000_raw": base.bbox_metrics(player_objects),
        "raw_c2280_reference": base.bbox_metrics(raw_reference_objects),
        "raw_c2280_editable_copy": base.bbox_metrics(raw_edit_objects),
        "sources": {
            "player_flver": str(base.PLAYER_FLVER),
            "raw_mushroom_flver": str(base.RAW_MUSHROOM_FLVER),
        },
    }
    player_height = metrics["player_fc_m_0000_raw"]["dims"][2]
    raw_height = metrics["raw_c2280_reference"]["dims"][2]
    metrics["ratios"] = {
        "raw_height_over_player_height": raw_height / player_height
        if player_height
        else None,
    }

    base.add_bbox(
        "raw scene player FC_M_0000 bounds",
        metrics["player_fc_m_0000_raw"],
        base.COLOR_PLAYER_BOUNDS,
    )
    base.add_bbox(
        "raw scene editable c2280 bounds",
        metrics["raw_c2280_editable_copy"],
        base.COLOR_MUSHROOM_BOUNDS,
    )
    label = (
        "Raw-game mushroom/player scene\n"
        "Blue locked: ER FC_M_0000 as imported from Elden Ring\n"
        "Green locked: DSR c2280 as imported from Dark Souls Remastered\n"
        "Orange editable: duplicate of raw DSR c2280, no Route A knobs\n"  # pi-lens-ignore: python-path-traversal — label text, not a path
        f"Raw/player height ratio: {metrics['ratios']['raw_height_over_player_height']:.3f}\n"
        "Edit orange when you want a clean source-shape target."
    )
    base.add_label(label, (0.0, -1.3, 1.8))

    base.bpy.context.scene.unit_settings.system = "METRIC"
    base.bpy.context.scene.render.engine = "BLENDER_EEVEE_NEXT"
    base.bpy.ops.wm.save_as_mainfile(filepath=str(RAW_BLEND_PATH))
    RAW_METRICS_PATH.write_text(json.dumps(metrics, indent=2), encoding="utf-8")
    write_open_script()  # pi-lens-ignore: python-thread-global-write python-path-traversal — writes fixed repo-local helper path derived from constants above
    base.log(f"blend={RAW_BLEND_PATH}")
    base.log(f"metrics={RAW_METRICS_PATH}")
    base.log(f"open_script={RAW_OPEN_SCRIPT_PATH}")
    base.bpy.ops.wm.quit_blender()


if __name__ == "__main__":
    main()
