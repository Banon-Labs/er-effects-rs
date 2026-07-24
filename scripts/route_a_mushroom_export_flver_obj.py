#!/usr/bin/env python3
"""Export selected FLVER meshes to OBJ for Mushroom Man offline comparison.

This is intentionally narrow: it exports positions and triangle indices from an
already-unpacked Elden Ring FLVER so a human can compare the generated Mushroom
Man body against the original player body in Blender. It does not modify game
files or launch the game.
"""

from __future__ import annotations

import argparse
import struct
from pathlib import Path

HEADER_SIZE = 0x80
DUMMY_SIZE = 0x40
MATERIAL_SIZE = 0x20
BONE_SIZE = 0x80
MESH_SIZE = 0x30
FACE_SET_SIZE = 0x20
VERTEX_BUFFER_SIZE = 0x20
BUFFER_LAYOUT_SIZE = 0x10
LAYOUT_MEMBER_SIZE = 0x14


def read_u8(data: bytes, offset: int) -> int:
    return data[offset]


def read_u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def read_u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def read_f32(data: bytes, offset: int) -> float:
    return struct.unpack_from("<f", data, offset)[0]


def parse_mesh_selection(text: str) -> list[int]:
    meshes: list[int] = []
    for part in text.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            lo_s, hi_s = part.split("-", 1)
            lo, hi = int(lo_s), int(hi_s)
            meshes.extend(range(lo, hi + 1))
        else:
            meshes.append(int(part))
    return sorted(dict.fromkeys(meshes))


def parse_layout_members(
    data: bytes, layout_table: int, layout_index: int
) -> list[tuple[int, int, int, int]]:
    layout_offset = layout_table + layout_index * BUFFER_LAYOUT_SIZE
    member_count = read_u32(data, layout_offset)
    member_offset = read_u32(data, layout_offset + 0x0C)
    members = []
    for i in range(member_count):
        offset = member_offset + i * LAYOUT_MEMBER_SIZE
        members.append(
            (
                read_u32(data, offset + 0x04),
                read_u32(data, offset + 0x08),
                read_u32(data, offset + 0x0C),
                read_u32(data, offset + 0x10),
            )
        )
    return members


def triangle_strip_to_triangles(indices: list[int]) -> list[tuple[int, int, int]]:
    triangles: list[tuple[int, int, int]] = []
    for i in range(len(indices) - 2):
        a, b, c = indices[i], indices[i + 1], indices[i + 2]
        if a in (b, c) or b == c:
            continue
        if i % 2:
            triangles.append((b, a, c))
        else:
            triangles.append((a, b, c))
    return triangles


def export_flver_obj(
    input_flver: Path, output_obj: Path, selected_meshes: list[int], all_face_sets: bool
) -> dict[str, int]:
    data = input_flver.read_bytes()
    if len(data) < HEADER_SIZE or data[:6] != b"FLVER\0" or data[6:8] != b"L\0":
        raise ValueError(f"{input_flver} is not a little-endian FLVER")

    data_offset = read_u32(data, 0x0C)
    dummy_count = read_u32(data, 0x14)
    material_count = read_u32(data, 0x18)
    bone_count = read_u32(data, 0x1C)
    mesh_count = read_u32(data, 0x20)
    vertex_buffer_count = read_u32(data, 0x24)
    default_index_size = read_u8(data, 0x48)
    face_set_count = read_u32(data, 0x50)
    layout_count = read_u32(data, 0x54)

    bone_table = HEADER_SIZE + DUMMY_SIZE * dummy_count + MATERIAL_SIZE * material_count
    mesh_table = bone_table + BONE_SIZE * bone_count
    face_set_table = mesh_table + MESH_SIZE * mesh_count
    vertex_buffer_table = face_set_table + FACE_SET_SIZE * face_set_count
    layout_table = vertex_buffer_table + VERTEX_BUFFER_SIZE * vertex_buffer_count
    if layout_count == 0:
        raise ValueError("FLVER has no buffer layouts")

    obj_lines: list[str] = [f"# Exported from {input_flver}"]
    obj_vertex_base = 1
    exported_vertices = 0
    exported_triangles = 0
    exported_meshes = 0

    for mesh_index in selected_meshes:
        if mesh_index >= mesh_count:
            raise ValueError(
                f"mesh index {mesh_index} out of range; mesh_count={mesh_count}"
            )
        mesh_offset = mesh_table + mesh_index * MESH_SIZE
        face_set_ref_count = read_u32(data, mesh_offset + 0x20)
        face_set_ref_offset = read_u32(data, mesh_offset + 0x24)
        vertex_buffer_ref_count = read_u32(data, mesh_offset + 0x28)
        vertex_buffer_ref_offset = read_u32(data, mesh_offset + 0x2C)
        if vertex_buffer_ref_count == 0 or face_set_ref_count == 0:
            continue

        vertex_buffer_index = read_u32(data, vertex_buffer_ref_offset)
        vertex_buffer_offset = (
            vertex_buffer_table + vertex_buffer_index * VERTEX_BUFFER_SIZE
        )
        layout_index = read_u32(data, vertex_buffer_offset + 0x04)
        vertex_size = read_u32(data, vertex_buffer_offset + 0x08)
        vertex_count = read_u32(data, vertex_buffer_offset + 0x0C)
        buffer_offset = read_u32(data, vertex_buffer_offset + 0x1C)
        members = parse_layout_members(data, layout_table, layout_index)
        position_members = [m for m in members if m[2] == 0 and m[1] == 0x02]
        if not position_members:
            raise ValueError(
                f"mesh {mesh_index} vertex buffer has no Float3 position member"
            )
        position_offset = position_members[0][0]

        obj_lines.append(f"o mesh_{mesh_index}")
        buffer_start = data_offset + buffer_offset
        for i in range(vertex_count):
            offset = buffer_start + i * vertex_size + position_offset
            obj_lines.append(
                f"v {read_f32(data, offset):.9f} {read_f32(data, offset + 4):.9f} {read_f32(data, offset + 8):.9f}"
            )

        face_set_refs = [
            read_u32(data, face_set_ref_offset + i * 4)
            for i in range(face_set_ref_count)
        ]
        if not all_face_sets:
            face_set_refs = face_set_refs[:1]
        for face_set_index in face_set_refs:
            face_set_offset = face_set_table + face_set_index * FACE_SET_SIZE
            triangle_strip = read_u8(data, face_set_offset + 0x04) != 0
            index_count = read_u32(data, face_set_offset + 0x08)
            index_offset = read_u32(data, face_set_offset + 0x0C)
            index_size = read_u32(data, face_set_offset + 0x18) or default_index_size
            width = 2 if index_size == 16 else 4
            indices = []
            for i in range(index_count):
                offset = data_offset + index_offset + i * width
                indices.append(
                    read_u16(data, offset) if width == 2 else read_u32(data, offset)
                )
            if triangle_strip:
                triangles = triangle_strip_to_triangles(indices)
            else:
                triangles = [
                    (indices[i], indices[i + 1], indices[i + 2])
                    for i in range(0, len(indices) - 2, 3)
                ]
            for a, b, c in triangles:
                obj_lines.append(
                    f"f {obj_vertex_base + a} {obj_vertex_base + b} {obj_vertex_base + c}"
                )
            exported_triangles += len(triangles)

        obj_vertex_base += vertex_count
        exported_vertices += vertex_count
        exported_meshes += 1

    output_obj.parent.mkdir(parents=True, exist_ok=True)
    output_obj.write_text("\n".join(obj_lines) + "\n", encoding="utf-8")
    return {
        "meshes": exported_meshes,
        "vertices": exported_vertices,
        "triangles": exported_triangles,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input-flver", type=Path, required=True)
    parser.add_argument("--output-obj", type=Path, required=True)
    parser.add_argument(
        "--meshes", default="0-13", help="comma/range mesh selection, e.g. 0-13 or 13"
    )
    parser.add_argument(
        "--all-face-sets",
        action="store_true",
        help="export every face set instead of only first/L0 per mesh",
    )
    args = parser.parse_args()
    stats = export_flver_obj(
        args.input_flver,
        args.output_obj,
        parse_mesh_selection(args.meshes),
        args.all_face_sets,
    )
    print(f"wrote {args.output_obj}")
    print(
        f"meshes={stats['meshes']} vertices={stats['vertices']} triangles={stats['triangles']}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
