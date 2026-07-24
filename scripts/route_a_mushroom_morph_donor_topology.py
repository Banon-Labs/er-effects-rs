#!/usr/bin/env python3
# check-no-magic-numbers: allow-file -- offline FLVER binary layout helper; constants mirror ER FLVER table widths.
"""Morph an Elden Ring donor FLVER mesh toward a mushroom silhouette without replacing topology.

This keeps the donor mesh vertex count, vertex order, face-set lists, and index buffers intact.
Only selected mesh vertex positions/normals and the mesh/header bounding boxes are rewritten.
That preserves the game's existing FC LOD face sets so distance switches draw coherent subsets
of the same morphed donor surface instead of truncated subsets of a replacement mesh.
"""

from __future__ import annotations

import argparse
import math
import shutil
import struct
from dataclasses import dataclass
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
MATERIAL_INDEX_OFFSET_IN_MESH = 0x04
HEADER_BBOX_MIN_OFFSET = 0x28
HEADER_BBOX_MAX_OFFSET = 0x34
POSITION_SEMANTIC_ID = 0
NORMAL_SEMANTIC_ID = 3
FLOAT3_FORMAT_ID = 0x02
NORMAL_FORMAT_IDS = {0x10, 0x11, 0x13, 0x2F}
DEFAULT_BINS = 128
SMOOTHING_PASSES = 3
EPSILON = 1.0e-6


@dataclass(frozen=True)
class Vec3:
    x: float
    y: float
    z: float

    def radius_xz(self) -> float:
        return math.hypot(self.x, self.z)


@dataclass(frozen=True)
class Mesh:
    bounding_box_offset: int
    face_set_count: int
    face_set_offset: int
    vertex_buffer_count: int
    vertex_buffer_offset: int


@dataclass(frozen=True)
class FaceSet:
    index_count: int
    index_offset: int
    index_size: int
    triangle_strip: bool


@dataclass(frozen=True)
class VertexBuffer:
    layout_index: int
    vertex_size: int
    vertex_count: int
    buffer_length: int
    buffer_offset: int


@dataclass(frozen=True)
class Layout:
    member_count: int
    member_offset: int


@dataclass(frozen=True)
class LayoutMember:
    struct_offset: int
    format_id: int
    semantic_id: int
    index: int


@dataclass(frozen=True)
class Header:
    version: int
    data_offset: int
    dummy_count: int
    material_count: int
    bone_count: int
    mesh_count: int
    vertex_buffer_count: int
    vertex_index_size: int
    face_set_count: int
    buffer_layout_count: int


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-obj", required=True, type=Path)
    parser.add_argument("--donor-flver", required=True, type=Path)
    parser.add_argument("--output-flver", required=True, type=Path)
    parser.add_argument("--summary", required=True, type=Path)
    parser.add_argument("--donor-mesh-index", required=True, type=int)
    parser.add_argument("--bins", type=int, default=DEFAULT_BINS)
    parser.add_argument(
        "--material-index",
        type=int,
        default=0,
        help="material slot to assign to the morphed donor mesh; default keeps Route A mushroom material slot 0",
    )
    parser.add_argument(
        "--keep-other-meshes",
        action="store_true",
        help="leave non-selected donor meshes visible instead of zeroing their face-set indices",
    )
    parser.add_argument(
        "--backup-existing",
        action="store_true",
        help="write <output>.bak before replacing an existing output file",
    )
    return parser.parse_args()


def read_u8(data: bytes | bytearray, offset: int) -> int:
    return data[offset]


def read_u32(data: bytes | bytearray, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def read_f32(data: bytes | bytearray, offset: int) -> float:
    return struct.unpack_from("<f", data, offset)[0]


def write_u16(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<H", data, offset, value)


def write_u32(data: bytearray, offset: int, value: int) -> None:
    struct.pack_into("<I", data, offset, value)


def write_f32(data: bytearray, offset: int, value: float) -> None:
    struct.pack_into("<f", data, offset, value)


def read_vec3(data: bytes | bytearray, offset: int) -> Vec3:
    return Vec3(read_f32(data, offset), read_f32(data, offset + 4), read_f32(data, offset + 8))


def write_vec3(data: bytearray, offset: int, value: Vec3) -> None:
    write_f32(data, offset, value.x)
    write_f32(data, offset + 4, value.y)
    write_f32(data, offset + 8, value.z)


def write_snorm8x4(data: bytearray, offset: int, values: tuple[float, float, float, float]) -> None:
    for i, value in enumerate(values):
        scaled = max(-127, min(127, round(value * 127.0)))
        data[offset + i] = scaled & 0xFF


def parse_header(data: bytes | bytearray) -> Header:
    if len(data) < HEADER_SIZE or data[:6] != b"FLVER\0" or data[6:8] != b"L\0":
        raise ValueError("expected little-endian FLVER header")
    return Header(
        version=read_u32(data, 0x08),
        data_offset=read_u32(data, 0x0C),
        dummy_count=read_u32(data, 0x14),
        material_count=read_u32(data, 0x18),
        bone_count=read_u32(data, 0x1C),
        mesh_count=read_u32(data, 0x20),
        vertex_buffer_count=read_u32(data, 0x24),
        vertex_index_size=read_u8(data, 0x48),
        face_set_count=read_u32(data, 0x50),
        buffer_layout_count=read_u32(data, 0x54),
    )


def table_offsets(header: Header) -> dict[str, int]:
    bone_table = HEADER_SIZE + DUMMY_SIZE * header.dummy_count + MATERIAL_SIZE * header.material_count
    mesh_table = bone_table + BONE_SIZE * header.bone_count
    face_set_table = mesh_table + MESH_SIZE * header.mesh_count
    vertex_buffer_table = face_set_table + FACE_SET_SIZE * header.face_set_count
    layout_table = vertex_buffer_table + VERTEX_BUFFER_SIZE * header.vertex_buffer_count
    return {
        "mesh": mesh_table,
        "face_set": face_set_table,
        "vertex_buffer": vertex_buffer_table,
        "layout": layout_table,
    }


def parse_meshes(data: bytes | bytearray, offset: int, count: int) -> list[Mesh]:
    meshes: list[Mesh] = []
    for i in range(count):
        base = offset + i * MESH_SIZE
        meshes.append(
            Mesh(
                bounding_box_offset=read_u32(data, base + 0x18),
                face_set_count=read_u32(data, base + 0x20),
                face_set_offset=read_u32(data, base + 0x24),
                vertex_buffer_count=read_u32(data, base + 0x28),
                vertex_buffer_offset=read_u32(data, base + 0x2C),
            )
        )
    return meshes


def parse_face_sets(data: bytes | bytearray, offset: int, count: int, vertex_index_size: int) -> list[FaceSet]:
    face_sets: list[FaceSet] = []
    for i in range(count):
        base = offset + i * FACE_SET_SIZE
        face_sets.append(
            FaceSet(
                triangle_strip=read_u8(data, base + 0x04) != 0,
                index_count=read_u32(data, base + 0x08),
                index_offset=read_u32(data, base + 0x0C),
                index_size=read_u32(data, base + 0x18) or vertex_index_size,
            )
        )
    return face_sets


def parse_vertex_buffers(data: bytes | bytearray, offset: int, count: int) -> list[VertexBuffer]:
    buffers: list[VertexBuffer] = []
    for i in range(count):
        base = offset + i * VERTEX_BUFFER_SIZE
        buffers.append(
            VertexBuffer(
                layout_index=read_u32(data, base + 0x04),
                vertex_size=read_u32(data, base + 0x08),
                vertex_count=read_u32(data, base + 0x0C),
                buffer_length=read_u32(data, base + 0x18),
                buffer_offset=read_u32(data, base + 0x1C),
            )
        )
    return buffers


def parse_layouts(data: bytes | bytearray, offset: int, count: int) -> list[Layout]:
    layouts: list[Layout] = []
    for i in range(count):
        base = offset + i * BUFFER_LAYOUT_SIZE
        layouts.append(Layout(member_count=read_u32(data, base), member_offset=read_u32(data, base + 0x0C)))
    return layouts


def parse_layout_members(data: bytes | bytearray, offset: int, count: int) -> list[LayoutMember]:
    members: list[LayoutMember] = []
    for i in range(count):
        base = offset + i * LAYOUT_MEMBER_SIZE
        members.append(
            LayoutMember(
                struct_offset=read_u32(data, base + 0x04),
                format_id=read_u32(data, base + 0x08),
                semantic_id=read_u32(data, base + 0x0C),
                index=read_u32(data, base + 0x10),
            )
        )
    return members


def parse_u32_list(data: bytes | bytearray, offset: int, count: int) -> list[int]:
    return [read_u32(data, offset + i * 4) for i in range(count)]


def read_obj_positions(path: Path) -> list[Vec3]:
    positions: list[Vec3] = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        parts = line.split()
        if parts[:1] == ["v"] and len(parts) >= 4:
            positions.append(Vec3(float(parts[1]), float(parts[2]), float(parts[3])))
    if not positions:
        raise ValueError(f"no OBJ vertices found: {path}")
    return positions


def bbox(positions: list[Vec3]) -> tuple[Vec3, Vec3]:
    return (
        Vec3(min(p.x for p in positions), min(p.y for p in positions), min(p.z for p in positions)),
        Vec3(max(p.x for p in positions), max(p.y for p in positions), max(p.z for p in positions)),
    )


def normalize_height(y: float, bounds: tuple[Vec3, Vec3]) -> float:
    low, high = bounds
    height = high.y - low.y
    if abs(height) < EPSILON:
        return 0.0
    return max(0.0, min(1.0, (y - low.y) / height))


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def envelope(positions: list[Vec3], bounds: tuple[Vec3, Vec3], bins: int) -> list[float]:
    values = [0.0 for _ in range(bins)]
    for position in positions:
        index = min(bins - 1, int(normalize_height(position.y, bounds) * (bins - 1)))
        values[index] = max(values[index], position.radius_xz())

    known = [i for i, value in enumerate(values) if value > EPSILON]
    if not known:
        return [1.0 for _ in values]
    for i, value in enumerate(values):
        if value > EPSILON:
            continue
        nearest = min(known, key=lambda candidate: abs(candidate - i))
        values[i] = values[nearest]

    for _ in range(SMOOTHING_PASSES):
        smoothed = values.copy()
        for i in range(1, bins - 1):
            smoothed[i] = (values[i - 1] + values[i] * 2.0 + values[i + 1]) / 4.0
        values = smoothed
    return values


def sample(values: list[float], t: float) -> float:
    if len(values) == 1:
        return values[0]
    scaled = max(0.0, min(1.0, t)) * (len(values) - 1)
    low = int(math.floor(scaled))
    high = min(len(values) - 1, low + 1)
    return lerp(values[low], values[high], scaled - low)


def fallback_angle(index: int) -> float:
    return (index * 2.399963229728653) % (math.pi * 2.0)


def normal_for(position: Vec3) -> Vec3:
    length = math.sqrt(position.x * position.x + position.z * position.z)
    if length < EPSILON:
        return Vec3(0.0, 1.0, 0.0)
    return Vec3(position.x / length, 0.0, position.z / length)


def read_donor_positions(
    data: bytes | bytearray,
    header: Header,
    vertex_buffer: VertexBuffer,
    members: list[LayoutMember],
) -> list[Vec3]:
    position_member = next(
        (
            member
            for member in members
            if member.semantic_id == POSITION_SEMANTIC_ID and member.format_id == FLOAT3_FORMAT_ID
        ),
        None,
    )
    if position_member is None:
        raise ValueError("selected donor vertex buffer has no float3 position member")
    start = header.data_offset + vertex_buffer.buffer_offset
    return [
        read_vec3(data, start + i * vertex_buffer.vertex_size + position_member.struct_offset)
        for i in range(vertex_buffer.vertex_count)
    ]


def morph_positions(source_positions: list[Vec3], donor_positions: list[Vec3], bins: int) -> list[Vec3]:
    source_bounds = bbox(source_positions)
    donor_bounds = bbox(donor_positions)
    source_envelope = envelope(source_positions, source_bounds, bins)
    donor_envelope = envelope(donor_positions, donor_bounds, bins)
    source_min, source_max = source_bounds

    morphed: list[Vec3] = []
    for index, donor in enumerate(donor_positions):
        t = normalize_height(donor.y, donor_bounds)
        donor_radius = donor.radius_xz()
        donor_shell_radius = max(sample(donor_envelope, t), EPSILON)
        radius_fraction = max(0.0, min(1.0, donor_radius / donor_shell_radius))
        target_shell_radius = sample(source_envelope, t)
        target_radius = target_shell_radius * math.sqrt(radius_fraction)
        angle = math.atan2(donor.z, donor.x) if donor_radius >= EPSILON else fallback_angle(index)
        morphed.append(
            Vec3(
                math.cos(angle) * target_radius,
                lerp(source_min.y, source_max.y, t),
                math.sin(angle) * target_radius,
            )
        )
    return morphed


def patch_morphed_vertices(
    data: bytearray,
    header: Header,
    vertex_buffer: VertexBuffer,
    members: list[LayoutMember],
    positions: list[Vec3],
) -> None:
    start = header.data_offset + vertex_buffer.buffer_offset
    if len(positions) != vertex_buffer.vertex_count:
        raise ValueError(f"morphed vertex count {len(positions)} != donor count {vertex_buffer.vertex_count}")
    for vertex_index, position in enumerate(positions):
        vertex_start = start + vertex_index * vertex_buffer.vertex_size
        normal = normal_for(position)
        for member in members:
            offset = vertex_start + member.struct_offset
            if member.semantic_id == POSITION_SEMANTIC_ID and member.format_id == FLOAT3_FORMAT_ID:
                write_vec3(data, offset, position)
            elif member.semantic_id == NORMAL_SEMANTIC_ID and member.format_id in NORMAL_FORMAT_IDS:
                write_snorm8x4(data, offset, (normal.x, normal.y, normal.z, 1.0))


def zero_face_set_indices(data: bytearray, header: Header, face_set: FaceSet) -> None:
    start = header.data_offset + face_set.index_offset
    if face_set.index_size == 16:
        for index in range(face_set.index_count):
            write_u16(data, start + index * 2, 0)
    elif face_set.index_size == 32:
        for index in range(face_set.index_count):
            write_u32(data, start + index * 4, 0)
    else:
        raise ValueError(f"unsupported face set index size {face_set.index_size}")


def write_bbox(data: bytearray, offset: int, bounds: tuple[Vec3, Vec3]) -> None:
    low, high = bounds
    write_vec3(data, offset, low)
    write_vec3(data, offset + 0x0C, high)


def main() -> int:
    args = parse_args()
    source_positions = read_obj_positions(args.source_obj)
    data = bytearray(args.donor_flver.read_bytes())
    header = parse_header(data)
    if header.version != 0x2001A:
        raise ValueError(f"expected ER FLVER 0x2001A, got 0x{header.version:X}")

    offsets = table_offsets(header)
    meshes = parse_meshes(data, offsets["mesh"], header.mesh_count)
    face_sets = parse_face_sets(data, offsets["face_set"], header.face_set_count, header.vertex_index_size)
    vertex_buffers = parse_vertex_buffers(data, offsets["vertex_buffer"], header.vertex_buffer_count)
    layouts = parse_layouts(data, offsets["layout"], header.buffer_layout_count)

    donor_mesh = meshes[args.donor_mesh_index]
    donor_mesh_table_offset = offsets["mesh"] + args.donor_mesh_index * MESH_SIZE
    write_u32(data, donor_mesh_table_offset + MATERIAL_INDEX_OFFSET_IN_MESH, args.material_index)

    vertex_buffer_indices = parse_u32_list(data, donor_mesh.vertex_buffer_offset, donor_mesh.vertex_buffer_count)
    if not vertex_buffer_indices:
        raise ValueError("selected donor mesh has no vertex buffers")
    vertex_buffer = vertex_buffers[vertex_buffer_indices[0]]
    layout = layouts[vertex_buffer.layout_index]
    members = parse_layout_members(data, layout.member_offset, layout.member_count)

    donor_positions = read_donor_positions(data, header, vertex_buffer, members)
    morphed_positions = morph_positions(source_positions, donor_positions, args.bins)
    patch_morphed_vertices(data, header, vertex_buffer, members, morphed_positions)

    morphed_bounds = bbox(morphed_positions)
    write_bbox(data, HEADER_BBOX_MIN_OFFSET, morphed_bounds)
    if donor_mesh.bounding_box_offset:
        write_bbox(data, donor_mesh.bounding_box_offset, morphed_bounds)

    selected_face_set_indices = set(
        parse_u32_list(data, donor_mesh.face_set_offset, donor_mesh.face_set_count)
    )
    retained_face_sets = 0
    hidden_face_sets = 0
    retained_face_set_capacities: list[int] = []
    for mesh_index, mesh in enumerate(meshes):
        indices = parse_u32_list(data, mesh.face_set_offset, mesh.face_set_count)
        for face_set_index in indices:
            face_set = face_sets[face_set_index]
            if mesh_index == args.donor_mesh_index:
                retained_face_sets += 1
                retained_face_set_capacities.append(face_set.index_count // 3)
            elif not args.keep_other_meshes and face_set_index not in selected_face_set_indices:
                zero_face_set_indices(data, header, face_set)
                hidden_face_sets += 1

    if args.output_flver.exists() and args.backup_existing:
        shutil.copy2(args.output_flver, args.output_flver.with_suffix(args.output_flver.suffix + ".bak"))
    args.output_flver.parent.mkdir(parents=True, exist_ok=True)
    args.output_flver.write_bytes(data)

    source_bounds = bbox(source_positions)
    args.summary.parent.mkdir(parents=True, exist_ok=True)
    args.summary.write_text(
        "\n".join(
            [
                "Route A donor-topology mushroom morph summary",
                "topology_mode=preserve-donor-vertices-indices-face-sets",
                f"source_obj={args.source_obj}",
                f"donor_flver={args.donor_flver}",
                f"output_flver={args.output_flver}",
                f"donor_mesh_index={args.donor_mesh_index}",
                f"donor_vertex_count={len(donor_positions)}",
                f"source_vertex_count={len(source_positions)}",
                f"retained_face_sets={retained_face_sets}",
                f"retained_face_set_triangle_capacities={','.join(str(v) for v in retained_face_set_capacities)}",
                f"hidden_face_sets={hidden_face_sets}",
                f"source_bbox_min={source_bounds[0].x:.9f},{source_bounds[0].y:.9f},{source_bounds[0].z:.9f}",
                f"source_bbox_max={source_bounds[1].x:.9f},{source_bounds[1].y:.9f},{source_bounds[1].z:.9f}",
                f"morphed_bbox_min={morphed_bounds[0].x:.9f},{morphed_bounds[0].y:.9f},{morphed_bounds[0].z:.9f}",
                f"morphed_bbox_max={morphed_bounds[1].x:.9f},{morphed_bounds[1].y:.9f},{morphed_bounds[1].z:.9f}",
                "runtime_status=not launched; offline FLVER topology-preserving morph only",
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    print(args.output_flver)
    print(args.summary)
    print(f"retained_face_sets={retained_face_sets} hidden_face_sets={hidden_face_sets}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
