#!/usr/bin/env python3
"""Adversarial verification of the minimal MSBE POINT_PARAM_ST InvasionPoint reader.

Written independently from the layout documented by
  * Ghidra 1.16.2 `/msb/MsbHeader`, `/msb/MsbSetHeader<MODEL_PARAM_ST>`,
    `/msb/MODEL_PARAM_ST`, `/CS/CSMsbPointShapeData`, and
  * `fstools_formats::msb::{SetHeader, point::Header}` (git rev 4b0db28),
NOT from the reference implementation under test.

Compares every field it can against the WitchyBND-derived oracle jsonl.

Inputs (env-overridable):
  MSB_DIR        directory tree holding DECOMPRESSED `*.msb` files
  MSB_ORACLE     WitchyBND-derived jsonl, one `{"map":..., "invasion_points":[...]}` per line
  MSB_DCX_DIR    shipped `map/mapstudio` directory holding `*.msb.dcx`
"""

import glob
import json
import os
import struct
import sys

MSB_DIR = os.environ.get("MSB_DIR", "")
ORACLE = os.environ.get(
    "MSB_ORACLE", os.path.expanduser("~/er-extract/invasion_points.20260804.jsonl")
)
DCX_DIR = os.environ.get(
    "MSB_DCX_DIR",
    os.path.expanduser("~/er-extract/LOOK_HERE_ALL_ASSETS_20260713/map/mapstudio"),
)


def u32(b, o):
    return struct.unpack_from("<I", b, o)[0]


def i32(b, o):
    return struct.unpack_from("<i", b, o)[0]


def u64(b, o):
    return struct.unpack_from("<Q", b, o)[0]


def f32(b, o):
    return struct.unpack_from("<f", b, o)[0]


def wstr(b, o):
    end = o
    while b[end : end + 2] != b"\x00\x00":
        end += 2
    return b[o:end].decode("utf-16-le")


def param_sets(b):
    """Yield (name, version, set_offset, entry_offsets, next_offset) for every param set."""
    assert b[:4] == b"MSB ", b[:4]
    sect = u32(b, 8)
    for _ in range(8):
        ver = u32(b, sect)
        cnt = u32(b, sect + 4)
        name = wstr(b, u64(b, sect + 8))
        offs = [u64(b, sect + 0x10 + 8 * i) for i in range(cnt - 1)]
        nxt = u64(b, sect + 0x10 + 8 * (cnt - 1))
        yield name, ver, sect, offs, nxt
        if nxt == 0:
            return
        sect = nxt


def main():
    if not MSB_DIR or not os.path.isdir(MSB_DIR):
        sys.exit("set MSB_DIR to a directory tree of decompressed .msb files")

    oracle = {}
    if os.path.exists(ORACLE):
        with open(ORACLE) as fh:
            for line in fh:
                d = json.loads(line)
                oracle[d["map"]] = d["invasion_points"]

    matched = 0
    examined = 0
    total_points = 0
    for path in sorted(glob.glob(os.path.join(MSB_DIR, "**", "*.msb"), recursive=True)):
        name = os.path.basename(path)[:-4]
        b = open(path, "rb").read()
        examined += 1

        dcx = os.path.join(DCX_DIR, name + ".msb.dcx")
        dcx_unc = None
        if os.path.exists(dcx):
            dcx_unc = struct.unpack(">I", open(dcx, "rb").read(0x20)[0x1C:0x20])[0]

        order, point_sect = [], None
        for nm, ver, sect, offs, _nxt in param_sets(b):
            order.append((nm, ver, len(offs)))
            if nm == "POINT_PARAM_ST":
                point_sect = (sect, offs)
        assert point_sect, name
        _sect, offs = point_sect

        hist, running = {}, {}
        ordinal_ok, region_ids, invasion = True, [], []
        contiguous, saw_other_after = True, False
        last_end = 0
        for gi, e in enumerate(offs):
            t = i32(b, e + 8)
            hist[t] = hist.get(t, 0) + 1
            k = running.get(t, 0)
            if u32(b, e + 0x0C) != k:
                ordinal_ok = False
            running[t] = k + 1
            region_ids.append(i32(b, e + 0x2C))
            if t != 1:
                saw_other_after = True
                continue
            if saw_other_after:
                contiguous = False
            td = e + u64(b, e + 0x58)
            invasion.append(
                dict(
                    gi=gi,
                    index=u32(b, e + 0x0C),
                    region_id=i32(b, e + 0x2C),
                    shape_type=u32(b, e + 0x10),
                    shape_data_off=u64(b, e + 0x48),
                    struct_s_off=u64(b, e + 0x60),
                    pos=[f32(b, e + 0x14), f32(b, e + 0x18), f32(b, e + 0x1C)],
                    rot=[f32(b, e + 0x20), f32(b, e + 0x24), f32(b, e + 0x28)],
                    prio=i32(b, td),
                    map_studio_layer=u32(b, e + 0x44),
                )
            )
            last_end = max(last_end, td + 4)

        want = oracle.get(name, [])
        bad = []
        if len(want) != len(invasion):
            bad.append(("count", len(want), len(invasion)))
        for got, exp in zip(invasion, want):
            if int(exp["RegionID"]) != got["region_id"]:
                bad.append(("RegionID", exp["RegionID"], got["region_id"]))
            if int(exp["Priority"]) != got["prio"]:
                bad.append(("Priority", exp["Priority"], got["prio"]))
            if int(exp["MapStudioLayer"]) != got["map_studio_layer"]:
                bad.append(("MapStudioLayer", exp["MapStudioLayer"], got["map_studio_layer"]))
            if exp["Shape"] is not None:
                bad.append(("Shape", exp["Shape"], None))
            for axis, key in enumerate("XYZ"):
                if abs(float(exp["Position"][key]) - got["pos"][axis]) > 5e-3:
                    bad.append(("Position" + key, exp["Position"][key], got["pos"][axis]))
                if abs(float(exp["Rotation"][key]) - got["rot"][axis]) > 5e-3:
                    bad.append(("Rotation" + key, exp["Rotation"][key], got["rot"][axis]))
        ok = not bad
        matched += ok
        total_points += len(invasion)
        pct = 100.0 * last_end / len(b) if invasion else 0.0
        print(f"{name}: size={len(b)} dcx_uncompressed={dcx_unc} size_match={len(b) == dcx_unc}")
        print(f"  sets={[o[0] for o in order]}")
        print(f"  versions={sorted({o[1] for o in order})} entry_counts={[o[2] for o in order]}")
        print(
            f"  region_entries={len(offs)} invasion={len(invasion)} oracle={len(want)} "
            f"ORACLE_MATCH={ok}"
        )
        print(
            f"  per_subtype_ordinal_at_0x0C={ordinal_ok} "
            f"region_id_unique={len(set(region_ids)) == len(region_ids)} "
            f"first_invasion_at_group_index={invasion[0]['gi'] if invasion else None} "
            f"invasion_block_contiguous={contiguous}"
        )
        print(
            f"  shape_types={sorted({p['shape_type'] for p in invasion})} "
            f"shape_data_offsets={sorted({p['shape_data_off'] for p in invasion})} "
            f"struct_s_offsets_nonzero={sorted({p['struct_s_off'] != 0 for p in invasion})} "
            f"last_invasion_byte=0x{last_end:x} ({pct:.2f}% of file)"
        )
        print(f"  point_subtype_histogram={dict(sorted(hist.items()))}")
        if bad:
            print(f"  MISMATCHES({len(bad)}): {bad[:8]}")

    print(f"\nmaps examined={examined} fully matching oracle={matched} invasion_points={total_points}")


main()
