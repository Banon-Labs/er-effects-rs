# check-no-magic-numbers: allow-file -- Blender scene layout and RGB constants are UI helper configuration, not reverse-engineered game offsets.
"""Create a Blender scene for direct mushroom-vs-player proportion feedback.

Run with Blender's Python, not system Python. This script uses the installed
Soulstruct Blender add-on to import FLVERs, then writes a comparison .blend and
metrics JSON under target/mushroom-route-a-offline/blender-compare/.
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

bpy = __import__("bpy")
gpu = __import__("gpu")
gpu_extras_batch = __import__("gpu_extras.batch", fromlist=["batch_for_shader"])
mathutils = __import__("mathutils")
Vector = mathutils.__dict__["Vector"]

# Soulstruct's add-on imports GPU draw helpers at module load time. In Blender
# background mode those helpers are unavailable, but FLVER import itself does
# not need them. Patch only the background-only draw setup path.
gpu_shader = gpu.__dict__["shader"]
gpu_shader.from_builtin = lambda _name: None
gpu_extras_batch.__dict__["batch_for_shader"] = lambda *_args, **_kwargs: None

REPO_ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = REPO_ROOT / "target" / "mushroom-route-a-offline" / "blender-compare"
PROJECT_ROOT = OUT_DIR / "soulstruct-project-root"
BLEND_PATH = OUT_DIR / "mushroom_player_scale_compare.blend"
METRICS_PATH = OUT_DIR / "mushroom_player_scale_compare.metrics.json"
LOG_PATH = OUT_DIR / "mushroom_player_scale_compare.log"

RAW_MUSHROOM_FLVER = (
    REPO_ROOT
    / "target"
    / "mushroom-route-a-offline"
    / "dsr"
    / "dsr-loose-mushroom"
    / "c2280-chrbnd-dcx"
    / "c2280.flver"
)
PLAYER_FLVER = (
    REPO_ROOT
    / "target"
    / "mushroom-route-a-offline"
    / "er-naked-parts"
    / "fc_m_0000-partsbnd-dcx"
    / "FC_M_0000.flver"
)
LIVE_TWEAK_OBJ = (
    REPO_ROOT
    / "target"
    / "mushroom-route-a-offline"
    / "arm-sweep"
    / "live-tweak"
    / "c2280-rust-export"
    / "c2280_route_a_scaled.obj"
)
PROTOTYPE_OBJ = (
    REPO_ROOT
    / "target"
    / "mushroom-route-a-offline"
    / "prototype"
    / "c2280-rust-export"
    / "c2280_route_a_scaled.obj"
)

PLAYER_COLLECTION = "01 ER player reference FC_M_0000"
RAW_COLLECTION = "02 DSR raw c2280 mushroom"
ROUTE_A_COLLECTION = "03 Current Route A live-tweak OBJ"
BBOX_COLLECTION = "99 comparison bounds and labels"

COLOR_PLAYER = (0.20, 0.55, 1.00, 0.45)
COLOR_RAW = (0.20, 1.00, 0.35, 0.50)
COLOR_ROUTE_A = (1.00, 0.55, 0.10, 0.55)
COLOR_PLAYER_BOUNDS = (0.05, 0.30, 1.00, 1.00)
COLOR_MUSHROOM_BOUNDS = (1.00, 0.35, 0.05, 1.00)


def log(message: str) -> None:
    sys.stdout.write(message + "\n")
    sys.stdout.flush()
    with LOG_PATH.open("a", encoding="utf-8") as log_file:
        log_file.write(message + "\n")  # pi-lens-ignore: python-thread-global-write — false positive; sequential log write, no threading


def require_path(path: Path) -> None:
    if not path.exists():
        raise FileNotFoundError(path)  # pi-lens-ignore: python-thread-global-write — false positive; exception construction, no threading


def prepare_dirs() -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    LOG_PATH.write_text("", encoding="utf-8")  # pi-lens-ignore: python-thread-global-write — false positive; fixed output path, no threading
    PROJECT_ROOT.mkdir(parents=True, exist_ok=True)  # pi-lens-ignore: python-thread-global-write — false positive; fixed output path, no threading
    for directory_name in ("parts", "chr", "map", "obj", "asset", "material"):  # pi-lens-ignore: python-thread-global-write — false positive; sequential setup loop, no threading
        (PROJECT_ROOT / directory_name).mkdir(exist_ok=True)


def enable_soulstruct() -> None:
    result = bpy.ops.preferences.addon_enable(module="io_soulstruct")
    log(f"addon_enable={result}")
    try:
        save_result = bpy.ops.wm.save_userpref()
        log(f"save_userpref={save_result}")
    except Exception as ex:  # noqa: BLE001 - Blender may refuse in factory/background mode.  # pi-lens-ignore: bare-except — Blender operators can throw broad runtime exceptions.
        log(f"save_userpref_warning={ex}")  # pi-lens-ignore: python-thread-global-write — false positive; sequential log write, no threading


def configure_soulstruct_game(game_enum: str) -> None:
    settings = bpy.context.scene.soulstruct_settings
    settings.game_enum = game_enum
    if game_enum == "ELDEN_RING":
        settings.eldenring_project_root_str = str(PROJECT_ROOT)
        settings.eldenring_game_root_str = str(PROJECT_ROOT)
    elif game_enum == "DARK_SOULS_DSR":
        settings.darksouls1r_project_root_str = str(PROJECT_ROOT)
        settings.darksouls1r_game_root_str = str(PROJECT_ROOT)
    settings.prefer_import_from_project = True
    settings.also_export_to_game = False
    import_settings = bpy.context.scene.flver_import_settings
    for property_name in ("import_textures", "import_textures_only"):
        if property_name in import_settings.__annotations__:
            try:
                import_settings[property_name] = False
            except Exception as ex:  # noqa: BLE001 - Blender RNA properties can reject assignment.  # pi-lens-ignore: bare-except — Blender RNA assignment can throw broad runtime exceptions.
                log(f"import_setting_warning {property_name}={ex}")  # pi-lens-ignore: python-thread-global-write — false positive; sequential log write, no threading


def clear_scene() -> None:
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete()


def make_collection(name: str):
    collection = bpy.data.collections.new(name)
    bpy.context.scene.collection.children.link(collection)
    return collection


def move_to_collection(objects, collection) -> None:
    for obj in objects:
        if obj.name not in collection.objects:
            collection.objects.link(obj)
        for old_collection in list(obj.users_collection):
            if old_collection != collection:
                old_collection.objects.unlink(obj)


def color_mesh_objects(objects, color, display_type: str = "TEXTURED") -> None:
    for obj in objects:
        if obj.type != "MESH":
            continue
        obj.color = color
        obj.show_wire = True
        obj.show_in_front = True
        obj.display_type = display_type
        for slot in obj.material_slots:
            if slot.material:
                slot.material.diffuse_color = color


def import_flver(path: Path, game_enum: str, collection_name: str, color):
    require_path(path)
    configure_soulstruct_game(game_enum)
    collection = make_collection(collection_name)
    before = {obj.name for obj in bpy.context.scene.objects}
    try:
        result = bpy.ops.import_scene.flver(directory=str(path.parent) + "\\", files=[{"name": path.name}])
        log(f"import_flver {path.name} result={result}")
    except RuntimeError as ex:
        if "view3d.view_selected" not in str(ex):  # pi-lens-ignore: bare-except — RuntimeError is intentionally filtered to one Blender background-mode warning.
            raise
        log(f"import_flver {path.name} background_view_warning={ex}")
    objects = [obj for obj in bpy.context.scene.objects if obj.name not in before]
    move_to_collection(objects, collection)
    color_mesh_objects(objects, color)
    log(f"import_flver {path.name} objects={len(objects)} meshes={sum(obj.type == 'MESH' for obj in objects)}")
    return objects


def import_obj(path: Path, collection_name: str, color):
    require_path(path)
    collection = make_collection(collection_name)
    before = {obj.name for obj in bpy.context.scene.objects}
    result = bpy.ops.wm.obj_import(filepath=str(path))
    log(f"import_obj {path.name} result={result}")
    objects = [obj for obj in bpy.context.scene.objects if obj.name not in before]
    move_to_collection(objects, collection)
    color_mesh_objects(objects, color, display_type="WIRE")
    return objects


def mesh_objects(objects):
    return [obj for obj in objects if obj.type == "MESH"]


def combined_bbox(objects):
    points = []
    for obj in mesh_objects(objects):
        for corner in obj.bound_box:
            points.append(obj.matrix_world @ Vector(corner))
    if not points:  # pi-lens-ignore: python-thread-global-write — false positive; local list check, no threading
        raise ValueError("no mesh points for bbox")
    mins = [min(point[index] for point in points) for index in range(3)]
    maxs = [max(point[index] for point in points) for index in range(3)]
    return mins, maxs


def bbox_metrics(objects):
    mins, maxs = combined_bbox(objects)
    dims = [maxs[index] - mins[index] for index in range(3)]
    center = [(mins[index] + maxs[index]) / 2 for index in range(3)]
    return {"min": mins, "max": maxs, "dims": dims, "center": center}


def make_material(name: str, color):
    material = bpy.data.materials.new(name)
    material.diffuse_color = color
    return material


def add_bbox(name: str, metrics: dict, color) -> None:
    mins = metrics["min"]
    maxs = metrics["max"]
    verts = [
        (mins[0], mins[1], mins[2]),
        (maxs[0], mins[1], mins[2]),
        (maxs[0], maxs[1], mins[2]),
        (mins[0], maxs[1], mins[2]),
        (mins[0], mins[1], maxs[2]),
        (maxs[0], mins[1], maxs[2]),
        (maxs[0], maxs[1], maxs[2]),
        (mins[0], maxs[1], maxs[2]),
    ]
    edges = [
        (0, 1), (1, 2), (2, 3), (3, 0),
        (4, 5), (5, 6), (6, 7), (7, 4),
        (0, 4), (1, 5), (2, 6), (3, 7),
    ]
    mesh = bpy.data.meshes.new(name + " mesh")
    mesh.from_pydata(verts, edges, [])
    mesh.update()  # pi-lens-ignore: python-thread-global-write — false positive; Blender mesh update on single thread
    obj = bpy.data.objects.new(name, mesh)  # pi-lens-ignore: python-thread-global-write — false positive; Blender object creation on single thread
    collection = bpy.data.collections[BBOX_COLLECTION]
    collection.objects.link(obj)
    obj.display_type = "WIRE"
    obj.show_in_front = True
    obj.color = color
    obj.data.materials.append(make_material(name + " material", color))


def add_label(body: str, location) -> None:
    bpy.ops.object.text_add(location=location, rotation=(1.2, 0.0, 0.0))
    obj = bpy.context.object
    obj.name = "comparison notes"
    obj.data.name = "comparison notes text"
    obj.data.body = body
    obj.data.align_x = "LEFT"
    obj.data.size = 0.08
    bpy.data.collections[BBOX_COLLECTION].objects.link(obj)
    for old_collection in list(obj.users_collection):
        if old_collection.name != BBOX_COLLECTION:
            old_collection.objects.unlink(obj)


def write_open_script() -> None:
    script_path = OUT_DIR / "open_mushroom_compare.ps1"
    script_path.write_text(
        "& 'C:\\Program Files\\Blender Foundation\\Blender 4.4\\blender.exe' "  # pi-lens-ignore: python-thread-global-write — false positive; fixed PowerShell command string
        f"'{BLEND_PATH}'\n",
        encoding="utf-8",
    )


def main() -> None:
    prepare_dirs()
    require_path(RAW_MUSHROOM_FLVER)
    require_path(PLAYER_FLVER)
    route_a_obj = LIVE_TWEAK_OBJ if LIVE_TWEAK_OBJ.exists() else PROTOTYPE_OBJ
    require_path(route_a_obj)

    enable_soulstruct()
    clear_scene()
    make_collection(BBOX_COLLECTION)

    player_objects = import_flver(PLAYER_FLVER, "ELDEN_RING", PLAYER_COLLECTION, COLOR_PLAYER)
    raw_objects = import_flver(RAW_MUSHROOM_FLVER, "DARK_SOULS_DSR", RAW_COLLECTION, COLOR_RAW)
    route_a_objects = import_obj(route_a_obj, ROUTE_A_COLLECTION, COLOR_ROUTE_A)

    metrics = {
        "player_fc_m_0000": bbox_metrics(player_objects),
        "raw_c2280": bbox_metrics(raw_objects),
        "route_a_current": bbox_metrics(route_a_objects),
        "sources": {
            "player_flver": str(PLAYER_FLVER),
            "raw_mushroom_flver": str(RAW_MUSHROOM_FLVER),
            "route_a_obj": str(route_a_obj),
        },
    }
    player_height = metrics["player_fc_m_0000"]["dims"][2]
    route_a_height = metrics["route_a_current"]["dims"][2]
    raw_height = metrics["raw_c2280"]["dims"][2]
    metrics["ratios"] = {
        "raw_height_over_player_height": raw_height / player_height if player_height else None,
        "route_a_height_over_player_height": route_a_height / player_height if player_height else None,
    }

    add_bbox("player FC_M_0000 bounds", metrics["player_fc_m_0000"], COLOR_PLAYER_BOUNDS)
    add_bbox("current Route A mushroom bounds", metrics["route_a_current"], COLOR_MUSHROOM_BOUNDS)
    label = (
        "Mushroom/player comparison scene\n"
        "Blue: ER FC_M_0000 player reference\n"
        "Green: raw DSR c2280 import\n"
        "Orange wire: current Route A live-tweak OBJ\n"
        f"RouteA/player height ratio: {metrics['ratios']['route_a_height_over_player_height']:.3f}\n"
        "Use this scene for direct scale/weight/proportion feedback before more runtime slider tuning."
    )
    add_label(label, (0.0, -1.3, 1.8))

    bpy.context.scene.unit_settings.system = "METRIC"
    bpy.context.scene.render.engine = "BLENDER_EEVEE_NEXT"
    bpy.ops.wm.save_as_mainfile(filepath=str(BLEND_PATH))
    METRICS_PATH.write_text(json.dumps(metrics, indent=2), encoding="utf-8")
    write_open_script()  # pi-lens-ignore: python-path-traversal — writes fixed repo-local helper path derived from constant OUT_DIR
    log(f"blend={BLEND_PATH}")
    log(f"metrics={METRICS_PATH}")
    log(f"open_script={OUT_DIR / 'open_mushroom_compare.ps1'}")
    bpy.ops.wm.quit_blender()


if __name__ == "__main__":
    main()
