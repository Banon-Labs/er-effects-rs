#!/usr/bin/env python3
"""Offline audit of the on-disk ER save slot BODY header vs the on-disk ProfileSummary
record, used to decide which body offset holds the saved BlockId (map id).

Reads .sl2/.co2 containers under the given roots (default: ./save-files), parses BND4,
and for every ACTIVE character slot prints/compares:
  * body+0x00 .. body+0x1c (u32 LE words of the slot body header)
  * USER_DATA010 ProfileSummary record: name, level, map (record+0x30)

Read-only. Never writes to any save file.
"""
import os
import struct
import sys

MD5_LEN = 0x10
SUM_BASE = 0x195c
SUM_STRIDE = 0x24c


def parse_entries(d):
    if d[0:4] != b"BND4":
        return None
    header_size = struct.unpack_from("<q", d, 0x10)[0]
    entry_stride = struct.unpack_from("<q", d, 0x20)[0]
    file_count = struct.unpack_from("<i", d, 0x0C)[0]
    ents = []
    for i in range(file_count):
        h = header_size + i * entry_stride
        size = struct.unpack_from("<q", d, h + 0x08)[0]
        off = struct.unpack_from("<i", d, h + 0x10)[0]
        noff = struct.unpack_from("<i", d, h + 0x14)[0]
        j = noff
        name = b""
        while d[j : j + 2] != b"\x00\x00":
            name += d[j : j + 2]
            j += 2
        ents.append((name.decode("utf-16-le"), off, size))
    return ents


def bodies(d):
    return {n: (off + MD5_LEN, size - MD5_LEN) for n, off, size in parse_entries(d)}


def u32(d, o):
    return struct.unpack_from("<I", d, o)[0]


def active_slots(d, b):
    st, _ = b["USER_DATA010"]
    length = u32(d, st + 0x150)
    ao = st + 0x154 + length
    return [d[ao + i] for i in range(10)]


def read_name(d, o, maxu=17):
    units = []
    for k in range(maxu):
        v = struct.unpack_from("<H", d, o + k * 2)[0]
        if v == 0:
            break
        units.append(v)
    return "".join(chr(x) for x in units), len(units)


def summary_record_base(d, b):
    """On-disk ProfileSummary record 0 = right after the 10 active-slot bytes, whose
    position itself follows the variable-length CSMenuSystemSaveLoad blob. NOT a fixed
    literal: a container with a different blob length shifts the whole table."""
    st, _ = b["USER_DATA010"]
    length = u32(d, st + 0x150)
    return st + 0x154 + length + 10


def scan(path):
    d = open(path, "rb").read()
    b = bodies(d)
    act = active_slots(d, b)
    base = summary_record_base(d, b)
    rows = []
    for slot in range(10):
        key = "USER_DATA%03d" % slot
        if key not in b:
            continue
        bs, _ = b[key]
        rec = base + slot * SUM_STRIDE
        sname, slen = read_name(d, rec)
        row = dict(
            slot=slot,
            active=act[slot],
            sum_name=sname,
            sum_len=slen,
            # on-disk record packs the 0x22-byte name with no padding, so every scalar
            # sits 2 bytes below its RAM-record offset (RAM 0x30 map -> disk 0x2e).
            sum_map=u32(d, rec + 0x2E),
            sum_level=u32(d, rec + 0x22),
            body_start=bs,
        )
        for off in range(0, 0x20, 4):
            row["b%02x" % off] = u32(d, bs + off)
        rows.append(row)
    return rows


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    want = set()
    for a in sys.argv[1:]:
        if a.startswith("--find="):
            want |= {int(x, 16) for x in a.split("=", 1)[1].split(",")}
    roots = args or ["save-files"]
    files = []
    for r in roots:
        for dp, _, fn in os.walk(r):
            for f in fn:
                if f.lower().endswith((".sl2", ".co2")):
                    files.append(os.path.join(dp, f))
    files.sort()
    mism04 = 0
    tot = 0
    hits = []
    for p in files:
        try:
            rows = scan(p)
        except Exception as e:  # noqa: BLE001
            print("ERR", p, e)
            continue
        for r in rows:
            if not r["active"]:
                continue
            tot += 1
            if r["b04"] != r["sum_map"]:
                mism04 += 1
                print(
                    "B04!=SUMMAP",
                    p,
                    r["slot"],
                    hex(r["b04"]),
                    hex(r["sum_map"]),
                    repr(r["sum_name"]),
                )
            for k in ["b%02x" % o for o in range(0, 0x20, 4)]:
                if r[k] in want:
                    hits.append(
                        (
                            p,
                            r["slot"],
                            k,
                            hex(r[k]),
                            r["sum_name"],
                            r["sum_len"],
                            hex(r["sum_map"]),
                            r["sum_level"],
                        )
                    )
    print("active slots scanned:", tot, "| b04 != summary map:", mism04)
    if want:
        print("--- target-word hits ---")
        for h in hits:
            print(h)


if __name__ == "__main__":
    main()
