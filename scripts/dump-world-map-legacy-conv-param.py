#!/usr/bin/env python3
"""Decrypt regulation.bin and answer: which blocks can the world map project?

`CS::WorldMapAreaConverter::ConvertMsbCoordsToMapCoords` @ 0x140876140 first runs
a block through `WorldMapLegacyConverter::ConvertLegacyDungeonPositionToOverworld
PositionForMap` @ 0x1408775e0. On a tree MISS that callee returns without writing
`blockIdOut`, so `blockIdOut` keeps the ORIGINAL (non-overworld) areaId and the
converter's `areaId ==` accept test fails -- no pin. The tree therefore decides
which non-overworld blocks can carry a map marker at all.

The tree is built by FUN_140876ce0 from `WORLD_MAP_LEGACY_CONV_PARAM_ST`
(paramdef index 233, row stride 0x30). This script replicates that builder
EXACTLY as disassembled:

  * only rows whose `isBasePoint` bit (row byte +0x24, `& 1`) is set are used
  * every base row inserts BOTH directions -- src->dst and dst->src -- because
    FUN_140876ce0 calls FUN_1408776e0 twice with the operands swapped and the
    delta negated
  * FUN_1408776e0 ignores a key that is already present, and when the dst is not
    an overworld block it looks the dst up in the tree; a miss returns 0 and the
    edge is pushed onto a pending list that FUN_140876ce0 retries until a whole
    pass adds nothing (transitive chain resolution)
  * `BlockId::IsOverworldBlockId` @ 0x140660fe0 is `areaId - 0x32 < 0x27`, i.e.
    areas [50, 89) -- which admits area 70 even though only converter slots 0
    (area 60) and 2 (area 61) are configured, so a block resolving to area 70
    still gets no pin.

With `--corpus` it cross-checks the resolved tree against the unpacked MSB corpus
and reports how many `Region/InvasionPoint` maps could actually be pinned.

Needs AES + zstd; there is no system pip, so run under uv:

    uv run --with pycryptodome --with zstandard python3 \
        scripts/dump-world-map-legacy-conv-param.py --corpus

Env overrides (no hard-coded user paths):
    ER_REGULATION_BIN  full path to regulation.bin
    ER_GAME_DIR        ELDEN RING/Game directory
    ER_MSB_CORPUS_ROOT unpacked map/mapstudio root (WitchyBND output)
"""

from __future__ import annotations

import argparse
import collections
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

DEFAULT_CORPUS_ROOTS = [
    os.path.expanduser(
        "~/er-extract/LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/map/mapstudio"
    ),
]

PARAM_FILE_STEM = "WorldMapLegacyConvParam"
ROW_STRIDE = 0x30

# BlockId::IsOverworldBlockId @ 0x140660fe0 -- `areaId - 0x32 < 0x27`.
OVERWORLD_AREA_LO = 50
OVERWORLD_AREA_HI = 89

# Only WorldMapAreaConverter slots 0 and 2 carry a refBlock; their areas are:
CONVERTER_AREAS = (60, 61)


def is_overworld(area: int) -> bool:
    return OVERWORLD_AREA_LO <= area < OVERWORLD_AREA_HI


def find_regulation() -> str:
    explicit = os.environ.get("ER_REGULATION_BIN")
    if explicit:
        return explicit
    candidates = [os.environ.get("ER_GAME_DIR")] + DEFAULT_GAME_DIRS
    for d in candidates:
        if d and os.path.isfile(os.path.join(d, "regulation.bin")):
            return os.path.join(d, "regulation.bin")
    raise SystemExit("regulation.bin not found; set ER_REGULATION_BIN or ER_GAME_DIR")


def load_bnd4(path: str) -> bytes:
    """regulation.bin = AES-CBC(no padding) -> DCX(ZSTD) -> BND4."""
    from Crypto.Cipher import AES  # uv --with pycryptodome

    raw = open(path, "rb").read()
    iv, body = raw[:16], bytearray(raw[16:])
    if len(body) % 16:
        body += b"\x00" * (16 - len(body) % 16)
    dec = AES.new(ER_REGULATION_KEY, AES.MODE_CBC, iv).decrypt(bytes(body))
    if dec[:4] == b"BND4":
        return dec
    if dec[:4] != b"DCX\x00":
        raise SystemExit(f"decrypt failed: magic {dec[:4]!r} (wrong key or file)")
    fmt = dec[0x28:0x2C]
    if fmt != b"ZSTD":
        raise SystemExit(f"unsupported DCX compression {fmt!r}")
    # DCX sizes are BIG endian.
    uncompressed = struct.unpack_from(">I", dec, 0x1C)[0]
    compressed = struct.unpack_from(">I", dec, 0x20)[0]
    dca = dec.find(b"DCA\x00")
    data = dca + struct.unpack_from(">I", dec, dca + 4)[0]

    import zstandard  # uv --with zstandard

    out = zstandard.ZstdDecompressor().decompress(
        dec[data : data + compressed], max_output_size=uncompressed + 16
    )
    if out[:4] != b"BND4":
        raise SystemExit(f"decompress produced {out[:4]!r}, expected BND4")
    return out


def find_param(bnd: bytes, stem: str) -> tuple[int, str]:
    """Return (file offset, name) of the last BND4 entry whose name contains stem.

    ER regulation stores 194 entries with a 0x24-byte file header:
    flags(4) | -1(4) | compressedSize(8) | uncompressedSize(8) | dataOffset(4) |
    id(4) | nameOffset(4). Names are NUL-terminated UTF-16LE full paths; the
    DLC02 `merged\\DLC02\\...` entry is the live one.
    """
    count = struct.unpack_from("<i", bnd, 0x0C)[0]
    header_size = struct.unpack_from("<q", bnd, 0x20)[0]
    hit = None
    for i in range(count):
        o = 0x40 + i * header_size
        data_off, _fid, name_off = struct.unpack_from("<III", bnd, o + 24)
        end = name_off
        while bnd[end : end + 2] != b"\x00\x00":
            end += 2
        name = bnd[name_off:end].decode("utf-16-le")
        if stem in name:
            hit = (data_off, name)
    if hit is None:
        raise SystemExit(f"{stem} not found in regulation BND4")
    return hit


def read_rows(bnd: bytes, base: int) -> list[dict]:
    strings_off = struct.unpack_from("<I", bnd, base + 0x00)[0]
    row_count = struct.unpack_from("<H", bnd, base + 0x0A)[0]
    data_start = struct.unpack_from("<I", bnd, base + 0x30)[0]
    # Self-check: row table then row data then the paramtype string.
    expect_data = 0x40 + row_count * 0x18
    expect_strings = data_start + row_count * ROW_STRIDE
    if data_start != expect_data or strings_off != expect_strings:
        print(
            f"WARNING: layout check failed "
            f"(dataStart {data_start:#x} vs {expect_data:#x}, "
            f"strings {strings_off:#x} vs {expect_strings:#x})"
        )
    rows = []
    for i in range(row_count):
        e = base + 0x40 + i * 0x18
        row_id = struct.unpack_from("<i", bnd, e)[0]
        p = base + struct.unpack_from("<q", bnd, e + 8)[0]
        rows.append(
            dict(
                id=row_id,
                src=(bnd[p + 4], bnd[p + 5], bnd[p + 6]),
                src_pos=struct.unpack_from("<fff", bnd, p + 8),
                dst=(bnd[p + 20], bnd[p + 21], bnd[p + 22]),
                dst_pos=struct.unpack_from("<fff", bnd, p + 24),
                is_base=bnd[p + 36] & 1,
            )
        )
    return rows


def build_tree(rows: list[dict]) -> tuple[dict, list]:
    """Replicate FUN_140876ce0 + FUN_1408776e0."""
    tree: dict[tuple, tuple] = {}

    def insert(src, dst) -> bool:
        if src in tree:  # duplicate key ignored
            return True
        if is_overworld(dst[0]):
            tree[src] = dst
            return True
        if dst in tree:  # chain through an already-resolved entry
            tree[src] = tree[dst]
            return True
        return False  # deferred, retried by the caller's fixpoint loop

    pending = []
    for r in rows:
        if not r["is_base"]:
            continue
        for a, b in ((r["src"], r["dst"]), (r["dst"], r["src"])):
            if not insert(a, b):
                pending.append((a, b))
    changed = True
    while changed:
        changed = False
        rest = []
        for a, b in pending:
            if insert(a, b):
                changed = True
            else:
                rest.append((a, b))
        pending = rest
    return tree, pending


def corpus_root() -> str | None:
    explicit = os.environ.get("ER_MSB_CORPUS_ROOT")
    for d in ([explicit] if explicit else []) + DEFAULT_CORPUS_ROOTS:
        if d and os.path.isdir(d):
            return d
    return None


def scan_corpus(root: str) -> dict[str, int]:
    maps: dict[str, int] = {}
    for dirpath, dirnames, _ in os.walk(root):
        for d in dirnames:
            if not d.endswith("-msb-dcx"):
                continue
            ip = os.path.join(dirpath, d, "Region", "InvasionPoint")
            if not os.path.isdir(ip):
                continue
            n = len([f for f in os.listdir(ip) if f.endswith(".xml")])
            if n:
                maps[d[: -len("-msb-dcx")]] = n
    return maps


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--rows", action="store_true", help="print every param row")
    ap.add_argument(
        "--corpus",
        action="store_true",
        help="cross-check against the unpacked MSB InvasionPoint corpus",
    )
    args = ap.parse_args()

    reg = find_regulation()
    bnd = load_bnd4(reg)
    base, name = find_param(bnd, PARAM_FILE_STEM)
    rows = read_rows(bnd, base)
    print(f"regulation : {reg}")
    print(f"param      : {name}  @ {base:#x}")
    print(f"rows       : {len(rows)}  (isBasePoint: {sum(r['is_base'] for r in rows)})")

    if args.rows:
        for r in rows:
            s, d = r["src"], r["dst"]
            print(
                f"  {r['id']:>6}  m{s[0]:02d}_{s[1]:02d}_{s[2]:02d} -> "
                f"m{d[0]:02d}_{d[1]:02d}_{d[2]:02d}  base={r['is_base']}"
            )

    tree, unresolved = build_tree(rows)
    areas = collections.Counter(v[0] for v in tree.values())
    print(f"tree       : {len(tree)} entries, {len(unresolved)} unresolved edges")
    print(f"override areas: {dict(sorted(areas.items()))}")
    stranded = {k: v for k, v in tree.items() if not is_overworld(v[0])}
    no_conv = {k: v for k, v in tree.items() if v[0] not in CONVERTER_AREAS}
    print(
        f"  entries resolving outside a configured converter area "
        f"{CONVERTER_AREAS}: {len(no_conv)}  (non-overworld: {len(stranded)})"
    )

    if args.corpus:
        root = corpus_root()
        if root is None:
            print("corpus root not found; set ER_MSB_CORPUS_ROOT -- skipping")
            return 0
        maps = scan_corpus(root)
        cat: collections.Counter = collections.Counter()
        pts: collections.Counter = collections.Counter()
        blocked = []
        for m, n in maps.items():
            a = [int(v) for v in m[1:].split("_")]
            key = (a[0], a[1], a[2])
            if is_overworld(a[0]):
                c = "already overworld"
            elif key not in tree:
                c = "NO TREE ENTRY -> no pin"
                blocked.append((m, n, None))
            elif tree[key][0] in CONVERTER_AREAS:
                c = f"projectable via area {tree[key][0]}"
            else:
                c = f"override area {tree[key][0]} -> NO CONVERTER"
                blocked.append((m, n, tree[key]))
            cat[c] += 1
            pts[c] += n
        print()
        print(f"corpus     : {root}")
        print(f"maps with Region/InvasionPoint: {len(maps)}  points: {sum(maps.values())}")
        for c in sorted(cat):
            print(f"  {c:<34} maps={cat[c]:>3}  points={pts[c]:>5}")
        if blocked:
            print("  unprojectable:")
            for m, n, t in sorted(blocked):
                print(f"    {m}  {n} points  -> {t}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
