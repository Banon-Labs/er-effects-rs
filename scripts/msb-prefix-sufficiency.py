#!/usr/bin/env python3
"""Measure how small a DECOMPRESSED prefix of an Elden Ring `.msb` still yields the
complete set of `POINT_PARAM_ST` InvasionPoint regions.

Elden Ring `.msb.dcx` payloads are Oodle Kraken streams cut into independent
0x40000-byte raw blocks (proven by `OodleLZ_GetCompressedStepForRawStep`'s
`pIndependent` output; see `scripts/oodle-dcx-probe`). So a caller can decode only
the first N blocks. This script answers what N has to be, by truncating an
already-decompressed `.msb` at N*0x40000 and re-running the InvasionPoint reader.

Usage: msb-prefix-sufficiency.py <dir-of-decompressed-msb> [dir-of-msb.dcx]
"""

import glob
import json
import os
import struct
import sys

BLK = 0x40000

MSB_DIR = sys.argv[1] if len(sys.argv) > 1 else ""
DCX_DIR = (
    sys.argv[2]
    if len(sys.argv) > 2
    else os.path.expanduser("~/er-extract/LOOK_HERE_ALL_ASSETS_20260713/map/mapstudio")
)
ORACLE = os.path.expanduser("~/er-extract/invasion_points.20260804.jsonl")


def extract(b):
    """Return the InvasionPoint tuples, or raise if `b` is too short."""

    def u32(o):
        return struct.unpack_from("<I", b, o)[0]

    def i32(o):
        return struct.unpack_from("<i", b, o)[0]

    def u64(o):
        return struct.unpack_from("<Q", b, o)[0]

    def f32(o):
        return struct.unpack_from("<f", b, o)[0]

    def wstr(o):
        e = o
        while e + 2 <= len(b) and b[e : e + 2] != b"\x00\x00":
            e += 2
        if e + 2 > len(b):
            raise EOFError("wstr past end")
        return b[o:e].decode("utf-16-le")

    sect = u32(8)
    out = None
    for _ in range(8):
        cnt = u32(sect + 4)
        name = wstr(u64(sect + 8))
        offs = [u64(sect + 0x10 + 8 * i) for i in range(cnt - 1)]
        nxt = u64(sect + 0x10 + 8 * (cnt - 1))
        if name == "POINT_PARAM_ST":
            res = []
            for e in offs:
                if i32(e + 8) != 1:
                    continue
                td = e + u64(e + 0x58)
                res.append(
                    (
                        i32(e + 0x2C),
                        round(f32(e + 0x14), 3),
                        round(f32(e + 0x18), 3),
                        round(f32(e + 0x1C), 3),
                        i32(td),
                        wstr(e + u64(e)),
                    )
                )
            out = res
        if nxt == 0:
            break
        sect = nxt
    if out is None:
        raise EOFError("no POINT_PARAM_ST")
    return out


def comp_bytes_for_blocks(dcx_path, nblk):
    """Compressed bytes that must be read to decode the first `nblk` raw blocks."""
    b = open(dcx_path, "rb").read()
    unc = struct.unpack_from(">I", b, 0x1C)[0]
    p, raw, k = 0x4C, 0, 0
    while raw < unc and k < nblk:
        p += 2  # Oodle block header
        v = (b[p] << 16) | (b[p + 1] << 8) | b[p + 2]
        p += 3  # quantum header (no checksums in the shipped corpus)
        p += (v & 0x3FFFF) + 1
        raw += min(BLK, unc - raw)
        k += 1
    return p - 0x4C


def main():
    if not MSB_DIR or not os.path.isdir(MSB_DIR):
        sys.exit("usage: msb-prefix-sufficiency.py <dir-of-decompressed-msb> [dir-of-msb.dcx]")
    oracle = {}
    if os.path.exists(ORACLE):
        for line in open(ORACLE):
            o = json.loads(line)
            oracle[o["map"]] = o["invasion_points"]

    for path in sorted(glob.glob(os.path.join(MSB_DIR, "*.msb"))):
        name = os.path.basename(path)[:-4]
        full = open(path, "rb").read()
        ref = extract(full)
        dcx = os.path.join(DCX_DIR, name + ".msb.dcx")
        print(
            f"== {name} unc={len(full)} points={len(ref)} "
            f"oracle={len(oracle.get(name, []))} oracle_match={len(ref) == len(oracle.get(name, []))}"
        )
        for nblk in (1, 2, 3, 4):
            raw = min(nblk * BLK, len(full))
            try:
                got = extract(full[:raw])
                ok = got == ref
                n = len(got)
            except Exception:
                ok, n = False, "SHORT"
            cb = comp_bytes_for_blocks(dcx, nblk) if os.path.exists(dcx) else -1
            print(
                f"   blocks={nblk} raw={raw:8d} comp_read={cb:7d} "
                f"points={n} identical_to_full={ok}"
            )


if __name__ == "__main__":
    main()
