#!/usr/bin/env python3
"""Answer "do shipped NpcParam rows carry nonzero mp/stamina?" straight off the
installed, encrypted `regulation.bin` -- offline, no game launch, no Smithbox.

Stages 1-3 (AES-256-CBC -> DCX/zstd -> BND4) are lifted verbatim from
`scripts/regulation-params.py`, which is validated against this install. This
script adds stage 4b: the PARAM *row data*, laid out by the ER paramdef so the
field offsets are derived rather than guessed, and cross-checked against the
stride measured from consecutive row data offsets.

    python3 scripts/npcparam-fp-stamina-census.py
    python3 scripts/npcparam-fp-stamina-census.py --chr 2030 3570 4500 4730 4080

The paramdef only names fields; every number printed is read out of the
regulation itself. If the paramdef-computed row size disagrees with the measured
stride, that is printed loudly, because it means late-row offsets are guesses.
"""

import argparse
import os
import re
import struct
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET

from compression import zstd

# SoulsFormats `RegulationKey.EldenRing`.
REGULATION_KEY = "99BFFC366A6BC8C6F5827D093602D676C42892A01C207FB024D3AF4E493FEF99"

DEFAULT_REGULATION = os.path.join(
    os.environ.get(
        "ER_GAME_DIR",
        os.path.expanduser("~/.local/share/Steam/steamapps/common/ELDEN RING/Game"),
    ),
    "regulation.bin",
)

DEFAULT_PARAMDEF = os.path.expanduser(
    "~/.local/share/smithbox/app/Assets/PARAM/ER/Defs/NpcParam.xml"
)

# Storage width in bytes, and the struct format used to read one element.
TYPES = {
    "s8": (1, "<b"),
    "u8": (1, "<B"),
    "dummy8": (1, "<B"),
    "s16": (2, "<h"),
    "u16": (2, "<H"),
    "s32": (4, "<i"),
    "u32": (4, "<I"),
    "f32": (4, "<f"),
    "angle32": (4, "<f"),
    "f64": (8, "<d"),
}


def decrypt(path):
    """Stage 1: AES-256-CBC, IV = the file's first 16 bytes."""
    raw = open(path, "rb").read()
    iv, ciphertext = raw[:16], raw[16:]
    ciphertext = ciphertext[: len(ciphertext) // 16 * 16]
    with tempfile.TemporaryDirectory() as work:
        enc = os.path.join(work, "ct")
        dec = os.path.join(work, "pt")
        open(enc, "wb").write(ciphertext)
        subprocess.run(
            ["openssl", "enc", "-d", "-aes-256-cbc", "-nopad",
             "-K", REGULATION_KEY, "-iv", iv.hex(), "-in", enc, "-out", dec],
            check=True,
            timeout=30,
        )
        plain = open(dec, "rb").read()
    if plain[:4] != b"DCX\0":
        raise SystemExit(f"decrypt produced {plain[:8]!r}, not a DCX -- wrong key or file")
    return plain


def dcx_unpack(dcx):
    """Stage 2: DCX header (big-endian) wrapping a zstd payload."""
    uncompressed = struct.unpack_from(">I", dcx, 0x1C)[0]
    compressed = struct.unpack_from(">I", dcx, 0x20)[0]
    data_offset = struct.unpack_from(">I", dcx, 0x14)[0]
    if dcx[0x24:0x28] != b"DCP\0" or dcx[0x28:0x2C] != b"ZSTD":
        raise SystemExit(f"unexpected DCX compression {dcx[0x24:0x2C]!r}")
    out = zstd.decompress(dcx[data_offset : data_offset + compressed])
    if len(out) != uncompressed or out[:4] != b"BND4":
        raise SystemExit(
            f"DCX payload is {len(out)} bytes of {out[:4]!r}, wanted {uncompressed} of BND4"
        )
    return out


def bnd4_entries(bnd):
    """Stage 3: BND4 directory -> {name: bytes}."""
    count = struct.unpack_from("<i", bnd, 0x0C)[0]
    files = {}
    for index in range(count):
        header = 0x40 + index * 0x24
        size = struct.unpack_from("<q", bnd, header + 0x10)[0]
        data_offset = struct.unpack_from("<I", bnd, header + 0x18)[0]
        name_offset = struct.unpack_from("<I", bnd, header + 0x20)[0]
        end = name_offset
        while bnd[end : end + 2] != b"\x00\x00":
            end += 2
        files[bnd[name_offset:end].decode("utf-16-le")] = bnd[data_offset : data_offset + size]
    return files


def param_rows(param):
    """Stage 4: PARAM row index -> [(id, data_offset)], in file order."""
    row_count = struct.unpack_from("<H", param, 0x0A)[0]
    paramdef_data_version = struct.unpack_from("<H", param, 0x08)[0]
    rows = []
    for index in range(row_count):
        base = 0x40 + index * 24
        row_id = struct.unpack_from("<i", param, base)[0]
        data_offset = struct.unpack_from("<q", param, base + 8)[0]
        rows.append((row_id, data_offset))
    return rows, paramdef_data_version


DEF_RE = re.compile(
    r"^\s*(?P<type>\w+)\s+(?P<name>\w+)\s*"
    r"(?:\[\s*(?P<count>\d+)\s*\])?\s*"
    r"(?::\s*(?P<bits>\d+))?\s*"
    r"(?:=.*)?$"
)


def parse_paramdef(path):
    """ER paramdef -> ({name: (offset, type)}, computed_row_size).

    Bitfield packing follows SoulsFormats: consecutive `type name:bits` fields
    share one storage unit while the type matches and the bits still fit.
    """
    root = ET.parse(path).getroot()
    fields = {}
    order = []
    offset = 0
    bit_offset = -1
    bit_type = None
    for field in root.find("Fields"):
        raw = field.get("Def")
        m = DEF_RE.match(raw)
        if not m:
            raise SystemExit(f"unparsed paramdef field: {raw!r}")
        ftype = m.group("type")
        name = m.group("name")
        count = int(m.group("count") or 1)
        bits = m.group("bits")
        if ftype in ("fixstr", "fixstrW"):
            width = count * (2 if ftype == "fixstrW" else 1)
            bit_offset = -1
            fields.setdefault(name, (offset, ftype))
            order.append((offset, ftype, name, count, None, 1))
            offset += width
            continue
        if ftype not in TYPES:
            raise SystemExit(f"unknown paramdef type {ftype!r} in {raw!r}")
        width, _fmt = TYPES[ftype]
        if bits is None:
            bit_offset = -1
            fields.setdefault(name, (offset, ftype))
            order.append((offset, ftype, name, count, None, width))
            offset += width * count
        else:
            bits = int(bits)
            limit = width * 8
            # SoulsFormats normalises dummy8 to u8 before comparing bit types, so a
            # `u8 x:1` followed by `dummy8 pad:7` shares one byte. Not folding them
            # shifts every later field by a byte.
            norm = "u8" if ftype == "dummy8" else ftype
            if bit_offset == -1 or bit_type != norm or bit_offset + bits > limit:
                bit_offset = 0
                bit_type = norm
                unit_offset = offset
                offset += width
            else:
                unit_offset = offset - width
            fields.setdefault(name, (unit_offset, f"{ftype}:{bits}@{bit_offset}"))
            order.append((unit_offset, ftype, name, count, bits, width))
            bit_offset += bits
    return fields, offset, order


def read_scalar(blob, base, offset, ftype):
    width, fmt = TYPES[ftype]
    return struct.unpack_from(fmt, blob, base + offset)[0]


def main():
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--regulation", default=DEFAULT_REGULATION)
    ap.add_argument("--paramdef", default=DEFAULT_PARAMDEF)
    ap.add_argument(
        "--chr", nargs="*", type=int, default=[2030, 3570, 4500, 4730, 4080],
        help="chr ids to sample (row ids chrId*10000 .. +9999)",
    )
    ap.add_argument("--per-chr", type=int, default=3, help="max rows to print per chr id")
    ap.add_argument("--hexdump", type=int, metavar="ROWID",
                    help="hexdump one row's bytes and exit")
    args = ap.parse_args()

    files = bnd4_entries(dcx_unpack(decrypt(args.regulation)))
    key = next(
        (n for n in files if n.rsplit("\\", 1)[-1].removesuffix(".param") == "NpcParam"), None
    )
    if key is None:
        raise SystemExit("NpcParam not found in regulation")
    param = files[key]
    rows, def_data_version = param_rows(param)

    # Measured stride: the mode of consecutive data-offset deltas.
    offsets = sorted(o for _, o in rows)
    deltas = {}
    for a, b in zip(offsets, offsets[1:]):
        deltas[b - a] = deltas.get(b - a, 0) + 1
    measured_stride = max(deltas, key=deltas.get) if deltas else 0

    fields, computed_size, order = parse_paramdef(args.paramdef)

    print(f"regulation : {args.regulation}")
    print(f"paramdef   : {args.paramdef}")
    print(f"NpcParam   : {len(rows)} rows, ids {min(r for r, _ in rows)}..{max(r for r, _ in rows)}")
    print(f"param header ParamdefDataVersion = {def_data_version}")
    print(f"measured row stride  = {measured_stride} (0x{measured_stride:x})  "
          f"delta histogram: {dict(sorted(deltas.items(), key=lambda kv: -kv[1])[:4])}")
    print(f"paramdef row size    = {computed_size} (0x{computed_size:x})")
    verdict = "AGREE" if computed_size == measured_stride else "DISAGREE"
    print(f"stride vs paramdef   : {verdict}")

    # Where the paramdef drifts away from the real row. A packed PARAM row keeps
    # every scalar naturally aligned, so the first field whose offset is not a
    # multiple of its own width marks the point past which paramdef-derived
    # offsets are guesses. Everything before it is byte-exact.
    drift_at = None
    for off, ftype, name, _count, _bits, width in order:
        if ftype in ("dummy8", "u8", "s8") or width == 1:
            continue
        if off % width:
            drift_at = (off, name)
            break
    if drift_at:
        print(f"paramdef drifts from : 0x{drift_at[0]:x} (first misaligned field {drift_at[1]!r}); "
              f"offsets BELOW that are byte-exact, offsets above are guesses")

    # Independent proof of that boundary, using no paramdef assumption beyond the
    # field TYPES: a drifted byte stream decodes f32 fields as denormals/NaN.
    import math
    def f32_garbage(lo, hi):
        checked = bad = 0
        for name, (off, ftype) in fields.items():
            if ftype != "f32" or not (lo <= off < hi) or off + 4 > measured_stride:
                continue
            for _rid, base in rows:
                value = struct.unpack_from("<f", param, base + off)[0]
                checked += 1
                if not math.isfinite(value) or (value != 0.0 and not 1e-6 <= abs(value) <= 1e7):
                    bad += 1
        return checked, bad
    if drift_at:
        lo_checked, lo_bad = f32_garbage(0, drift_at[0])
        hi_checked, hi_bad = f32_garbage(drift_at[0], measured_stride)
        print(f"f32 sanity below 0x{drift_at[0]:x}: {lo_bad}/{lo_checked} implausible")
        print(f"f32 sanity above 0x{drift_at[0]:x}: {hi_bad}/{hi_checked} implausible")

    for name in ("hp", "mp", "stamina", "getSoul"):
        if name in fields:
            off, ftype = fields[name]
            if drift_at and off < drift_at[0]:
                trust = "CONFIRMED (below paramdef drift point)"
            elif off < measured_stride:
                trust = "UNTRUSTED (above paramdef drift point)"
            else:
                trust = "UNTRUSTED (past end of measured row)"
            print(f"  paramdef {name:10s} @ 0x{off:03x} {ftype:6s}  {trust}")
        else:
            print(f"  paramdef {name:10s} ABSENT")

    if args.hexdump is not None:
        base = dict(rows)[args.hexdump]
        blob = param[base : base + measured_stride]
        print(f"\nrow {args.hexdump} @ file 0x{base:x}, {len(blob)} bytes")
        for i in range(0, len(blob), 16):
            chunk = blob[i : i + 16]
            print(f"  {i:04x}  " + " ".join(f"{b:02x}" for b in chunk))
        return 0

    hp_off, hp_type = fields["hp"]
    mp_off, mp_type = fields["mp"]
    st_off, st_type = fields["stamina"]

    print(f"\nusing offsets: hp @ 0x{hp_off:x} {hp_type}, "
          f"mp @ 0x{mp_off:x} {mp_type}, stamina @ 0x{st_off:x} {st_type}")

    print("\n-- sampled rows --")
    print(f"{'chr':>6}  {'rowId':>10}  {'hp':>9}  {'mp':>9}  {'stamina':>9}")
    for chrid in args.chr:
        lo, hi = chrid * 10000, chrid * 10000 + 9999
        picked = [(rid, off) for rid, off in rows if lo <= rid <= hi]
        picked.sort()
        if not picked:
            print(f"c{chrid:<5}  (no rows in {lo}..{hi})")
            continue
        for rid, off in picked[: args.per_chr]:
            hp = read_scalar(param, off, hp_off, hp_type)
            mp = read_scalar(param, off, mp_off, mp_type)
            st = read_scalar(param, off, st_off, st_type)
            print(f"c{chrid:<5}  {rid:>10}  {hp:>9}  {mp:>9}  {st:>9}")
        if len(picked) > args.per_chr:
            print(f"        ... {len(picked) - args.per_chr} more rows for c{chrid}")

    print("\n-- corpus-wide census --")
    total = len(rows)
    hp_pos = mp_gt1 = mp_nz = st_gt1 = st_nz = 0
    both = 0
    mp_vals = {}
    st_vals = {}
    for rid, off in rows:
        hp = read_scalar(param, off, hp_off, hp_type)
        mp = read_scalar(param, off, mp_off, mp_type)
        st = read_scalar(param, off, st_off, st_type)
        if hp > 0:
            hp_pos += 1
        if mp != 0:
            mp_nz += 1
        if mp > 1:
            mp_gt1 += 1
        if st != 0:
            st_nz += 1
        if st > 1:
            st_gt1 += 1
        if mp > 1 and st > 1:
            both += 1
        mp_vals[mp] = mp_vals.get(mp, 0) + 1
        st_vals[st] = st_vals.get(st, 0) + 1

    def pct(n):
        return f"{n:5d} / {total} = {100.0 * n / total:5.1f}%"

    print(f"  rows total          : {total}")
    print(f"  hp    > 0           : {pct(hp_pos)}")
    print(f"  mp    != 0          : {pct(mp_nz)}")
    print(f"  mp    >  1          : {pct(mp_gt1)}")
    print(f"  stamina != 0        : {pct(st_nz)}")
    print(f"  stamina >  1        : {pct(st_gt1)}")
    print(f"  mp>1 AND stamina>1  : {pct(both)}")
    print(f"  most common mp      : {sorted(mp_vals.items(), key=lambda kv: -kv[1])[:6]}")
    by_chr = {}
    for rid, off in rows:
        if read_scalar(param, off, mp_off, mp_type) > 1:
            by_chr.setdefault(rid // 10000, []).append(rid)
    print(f"  mp>1 lives on {len(by_chr)} chr ids: "
          + ", ".join(f"c{c}({len(v)} rows)" for c, v in sorted(by_chr.items())))
    print(f"  most common stamina : {sorted(st_vals.items(), key=lambda kv: -kv[1])[:6]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
