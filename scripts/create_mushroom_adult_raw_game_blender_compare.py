# check-no-magic-numbers: allow-file -- Blender scene layout and RGB constants are UI helper configuration, not reverse-engineered game offsets.
"""Create a raw-game Blender scene for adult DS1 mushroom shaping.

This is the adult/parent-mushroom counterpart to
`create_mushroom_raw_game_blender_compare.py`. It imports the ER player reference
and DSR c2270 exactly through their respective Soulstruct game profiles, then
adds an editable duplicate of the raw adult mushroom so manual shaping starts
from the original DS1/DSR adult asset instead of the current child c2280 route.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))

import create_mushroom_blender_compare as base  # noqa: E402

ADULT_MUSHROOM_FLVER = (
    base.REPO_ROOT
    / "target"
    / "mushroom-route-a-offline"
    / "dsr"
    / "dsr-loose-mushroom"
    / "c2270-chrbnd-dcx"
    / "c2270.flver"
)
ADULT_BLEND_PATH = base.OUT_DIR / "mushroom_adult_raw_game_compare.blend"
ADULT_METRICS_PATH = base.OUT_DIR / "mushroom_adult_raw_game_compare.metrics.json"
ADULT_LOG_PATH = base.OUT_DIR / "mushroom_adult_raw_game_compare.log"
ADULT_OPEN_SCRIPT_PATH = base.OUT_DIR / "open_mushroom_adult_raw_game_compare.ps1"

PLAYER_COLLECTION = "01 ER raw player FC_M_0000 reference"
RAW_REFERENCE_COLLECTION = "02 DSR raw c2270 adult mushroom reference LOCKED"
RAW_EDIT_COLLECTION = "03 EDIT ME raw DSR c2270 adult copy"

COLOR_EDITABLE_RAW = (1.00, 0.55, 0.10, 0.55)


def write_open_script() -> None:
    ADULT_OPEN_SCRIPT_PATH.write_text(  # pi-lens-ignore: python-thread-global-write python-path-traversal — fixed repo-local PowerShell helper path
        "& 'C:\\Program Files\\Blender Foundation\\Blender 4.4\\blender.exe' "
        f"'{ADULT_BLEND_PATH}'\n",
        encoding="utf-8",
    )


def lock_objects(objects) -> None:
    for obj in objects:
        obj.hide_select = True


def duplicate_collection_objects(objects, collection_name: str):
    collection = base.make_collection(collection_name)
    object_map = {}
    duplicates = []
    for obj in objects:
        duplicate = obj.copy()
        if obj.data is not None:
            duplicate.data = obj.data.copy()
        duplicate.name = f"EDIT_ME_ADULT_{obj.name}"
        object_map[obj] = duplicate
        duplicates.append(duplicate)
        collection.objects.link(duplicate)
    for source, duplicate in object_map.items():
        if source.parent in object_map:
            duplicate.parent = object_map[source.parent]
    return duplicates


def main() -> None:
    base.LOG_PATH = ADULT_LOG_PATH
    base.prepare_dirs()
    base.require_path(ADULT_MUSHROOM_FLVER)
    base.require_path(base.PLAYER_FLVER)

    base.enable_soulstruct()
    base.clear_scene()
    base.make_collection(base.BBOX_COLLECTION)

    player_objects = base.import_flver(
        base.PLAYER_FLVER, "ELDEN_RING", PLAYER_COLLECTION, base.COLOR_PLAYER
    )
    raw_reference_objects = base.import_flver(
        ADULT_MUSHROOM_FLVER,
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
        "raw_c2270_adult_reference": base.bbox_metrics(raw_reference_objects),
        "raw_c2270_adult_editable_copy": base.bbox_metrics(raw_edit_objects),
        "sources": {
            "player_flver": str(base.PLAYER_FLVER),
            "raw_adult_mushroom_flver": str(ADULT_MUSHROOM_FLVER),
        },
    }
    player_height = metrics["player_fc_m_0000_raw"]["dims"][2]
    raw_height = metrics["raw_c2270_adult_reference"]["dims"][2]
    metrics["ratios"] = {
        "raw_adult_height_over_player_height": raw_height / player_height
        if player_height
        else None,
    }

    base.add_bbox(
        "raw scene player FC_M_0000 bounds",
        metrics["player_fc_m_0000_raw"],
        base.COLOR_PLAYER_BOUNDS,
    )
    base.add_bbox(
        "raw scene editable c2270 adult bounds",
        metrics["raw_c2270_adult_editable_copy"],
        base.COLOR_MUSHROOM_BOUNDS,
    )
    label = (
        "Raw-game adult mushroom/player scene\n"
        "Blue locked: ER FC_M_0000 as imported from Elden Ring\n"
        "Green locked: DSR c2270 adult/parent mushroom as imported from DSR\n"
        "Orange editable: duplicate of raw DSR c2270 adult mushroom\n"
        f"Adult/player height ratio: {metrics['ratios']['raw_adult_height_over_player_height']:.3f}\n"
        "Edit orange to loosely match the human reference before the adult route export."
    )
    base.add_label(label, (0.0, -1.3, 1.8))

    base.bpy.context.scene.unit_settings.system = "METRIC"
    base.bpy.context.scene.render.engine = "BLENDER_EEVEE_NEXT"
    base.bpy.ops.wm.save_as_mainfile(filepath=str(ADULT_BLEND_PATH))
    ADULT_METRICS_PATH.write_text(json.dumps(metrics, indent=2), encoding="utf-8")
    write_open_script()
    base.log(f"blend={ADULT_BLEND_PATH}")
    base.log(f"metrics={ADULT_METRICS_PATH}")
    base.log(f"open_script={ADULT_OPEN_SCRIPT_PATH}")
    base.bpy.ops.wm.quit_blender()


if __name__ == "__main__":
    main()
