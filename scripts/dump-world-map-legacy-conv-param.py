#!/usr/bin/env python3
"""Decrypt regulation.bin and dump WorldMapLegacyConvParam rows.

Answers exactly one question: which source blocks does
`WorldMapLegacyConverter` (built from `WORLD_MAP_LEGACY_CONV_PARAM_ST`,
paramdef index 233, row stride 0x30) know how to project into overworld map
space? A block with no row is rejected by
`CS::WorldMapAreaConverter::ConvertMsbCoordsToMapCoords` @ 0x140876140,
because the legacy converter leaves `blockIdOut` at the original (non-60/61)
areaId and the converter's `areaId ==` accept test then fails.

Needs AES; there is no system pip, so run it under uv:

    uv run --with pycryptodome python3 scripts/dump-world-map-legacy-conv-param.py

Paths are env-overridable:
    ER_REGULATION_BIN   full path to regulation.bin
    ER_GAME_DIR         ELDEN RING/Game directory (used to find regulation.bin)
"""

from __future__ import annotations

import os
import struct
import sys

# Public SoulsFormats regulation key for ELDEN RING (RegulationDecryptor.cs).
ER_REGULATION_KEY = bytes(
    [
        0x99, 0xBF, 0xFC, 0x36, 0x6A, 0x6B, 0xC8, 0xC6,
        0xF5, 0x82, 0x7D, 0x09, 0x36, 0x02, 0xD6, 0x76,
        0xC4, 0x28, 0x92, 0xA0, 0x1C, 0x20, 0x7F, 0xB0,
        0x24, 0xD3, 0xAF, 0x4E, 0x49, 0x3F, 0xEF, 0x99,
    ]
)

DEFAULT_GAME_DIRS = [
    os.path.expanduser("~/.local/share/Steam/steamapps/common/ELDEN RING/Game"),
    "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game",
]

ROW_STRIDE = 0x30


def find_regulation() -> str:
    explicit = os.environ.get("ER_REGULATION_BIN")
    if explicit:
        return explicit
    game_dir = os.environ.get("ER_GAME_DIR")
    candidates = [game_dir] if game_dir else []
    candidates += DEFAULT_GAME_DIRS
    for d in candidates:
        if not d:
            continue
        p = os.path.join(d, "regulation.bin")
        if os.path.isfile(p):
            return p
    raise SystemExit("regulation.bin not found; set ER_REGULATION_BIN")


def decrypt(path: str) -> bytes:
    from Crypto.Cipher import AES  # provided by `uv run --with pycryptodome`

    raw = open(path, "rb").read()
    iv, body = raw[:16], bytearray(raw[16:])
    if len(body) % 16:
        body += b"\x00" * (16 - len(body) % 16)
    return AES.new(ER_REGULATION_KEY, AES.MODE_CBC, iv).decrypt(bytes(body))


def find_all(hay: bytes, needle: bytes) -> list[int]:
    out, s = [], 0
    while True:
        i = hay.find(needle, s)
        if i < 0:
            return out
        out.append(i)
        s = i + 1


def main() -> int:
    path = find_regulation()
    dec = decrypt(path)
    print(f"regulation: {path}")
    print(f"decrypted magic: {dec[:4]!r}  len={len(dec)}")
    if dec[:4] != b"BND4":
        print("DECRYPT FAILED (no BND4 magic) -- wrong key or wrong file")
        return 2

    hits = find_all(dec, b"WORLD_MAP_LEGACY_CONV_PARAM_ST")
    print(f"paramtype string offsets: {[hex(h) for h in hits]}")
    name_hits = find_all(dec, "WorldMapLegacyConvParam".encode("utf-16-le"))
    print(f"utf16 filename offsets: {[hex(h) for h in name_hits]}")
    if not hits:
        print("param file appears DCX-compressed inside the BND4; need Oodle")
        return 3

    # The PARAM header sits before its paramtype string. Walk back to a plausible
    # header by scanning candidate starts and validating the self-describing
    # fields: header+0x0C == offset of the paramtype string (64-bit params store
    # it via the 0x38 pointer when ParamTypeOffset==0).
    for h in hits:
        start = locate_param_start(dec, h)
        if start is None:
            print(f"  could not locate PARAM start for string @ {h:#x}")
            continue
        dump_param(dec, start)
        return 0
    return 4


def locate_param_start(buf: bytes, type_str_off: int) -> int | None:
    """Find the PARAM file start that owns the paramtype string at type_str_off.

    ER PARAMs (FormatVersion >= 202) put the paramtype string at the offset
    stored in header+0x38 with header+0x0C == 0. Scan backwards a bounded window
    for a header whose +0x38 resolves to exactly type_str_off.
    """
    lo = max(0, type_str_off - 0x400000)
    for start in range(type_str_off - 0x40, lo - 1, -1):
        if start + 0x40 > len(buf):
            continue
        try:
            (ptr,) = struct.unpack_from("<q", buf, start + 0x38)
        except struct.error:
            continue
        if ptr == type_str_off - start and struct.unpack_from("<i", buf, start + 0x0C)[0] == 0:
            return start
    return None


def dump_param(buf: bytes, start: int) -> None:
    strings_off = struct.unpack_from("<I", buf, start + 0x00)[0]
    row_count = struct.unpack_from("<H", buf, start + 0x0A)[0]
    print(f"PARAM start {start:#x}  rowCount={row_count}  stringsOffset={strings_off:#x}")

    rows = []
    for i in range(row_count):
        ent = start + 0x40 + i * 0x18
        row_id = struct.unpack_from("<i", buf, ent)[0]
        data_off = struct.unpack_from("<q", buf, ent + 8)[0]
        rows.append((row_id, data_off))

    print(f"{'rowId':>10}  src(area,gx,gz)  srcPos                     dst(area,gx,gz)  dstPos                    isBase")
    src_blocks = set()
    per_area = {}
    for row_id, data_off in rows:
        d = start + data_off
        src_area, src_gx, src_gz = buf[d + 4], buf[d + 5], buf[d + 6]
        sx, sy, sz = struct.unpack_from("<fff", buf, d + 8)
        dst_area, dst_gx, dst_gz = buf[d + 20], buf[d + 21], buf[d + 22]
        dx, dy, dz = struct.unpack_from("<fff", buf, d + 24)
        is_base = buf[d + 36] & 1
        src_blocks.add((src_area, src_gx, src_gz))
        per_area.setdefault(src_area, set()).add((src_gx, src_gz))
        if len(rows) <= 400:
            print(
                f"{row_id:>10}  m{src_area:02d}_{src_gx:02d}_{src_gz:02d}      "
                f"({sx:9.2f},{sy:9.2f},{sz:9.2f})  "
                f"m{dst_area:02d}_{dst_gx:02d}_{dst_gz:02d}      "
                f"({dx:9.2f},{dy:9.2f},{dz:9.2f})  {is_base}"
            )

    print()
    print(f"distinct src (area,gridX,gridZ): {len(src_blocks)}")
    for area in sorted(per_area):
        print(f"  area {area:02d}: {len(per_area[area])} grids -> {sorted(per_area[area])[:12]}")


if __name__ == "__main__":
    sys.exit(main())
