#!/usr/bin/env python3
"""Report which save slots `er_save_loader::stats` will decode, and why not.

The ProfileSelect / Load Character row attribute line comes from
`er_save_loader::stats::all_slot_stats`, which locates each slot's serialized
`PlayerGameData` by scanning the slot body for the Elden Ring identity

    RuneLevel == sum(eight attributes at PGD+0x3c) - 79

A character whose stored `level` word does not satisfy that identity (a
respec/level edit that moved one side and not the other, a build importer that
wrote attributes without recomputing the level, ...) is located nowhere in the
body, so its whole row decodes as `None`: no attributes, no vitals, no WL.

This replicates that acceptance test byte-for-byte, plus a face-anchored read of
the true `PlayerGameData` so a rejected slot can be told apart from an empty one
and the size of the disagreement is printed.

    python3 scripts/save-slot-stat-invariant.py <ER0000.sl2|.co2> [--json]

Read-only: the save is opened for reading and nothing else.
"""

from __future__ import annotations

import argparse
import json
import struct
import sys
from pathlib import Path

import numpy as np

BND4_MAGIC = b"BND4"
HDR_HEADER_SIZE_OFF = 0x10
HDR_FILE_HEADER_SIZE_OFF = 0x20
HDR_FILE_COUNT_OFF = 0x0C
ENT_SIZE_OFF = 0x08
ENT_DATA_OFFSET_OFF = 0x10
ENT_NAME_OFFSET_OFF = 0x14
ENTRY_MD5_LEN = 0x10

# er-save-loader/src/stats.rs
PGD_STAT_BASE = 0x3C
PGD_LEVEL = 0x68
PGD_NAME = 0x9C
PGD_NAME_LEN_U16 = 17
LEVEL_FROM_STAT_BASE = PGD_LEVEL - PGD_STAT_BASE  # 0x2c
RUNE_LEVEL_BASE = 79
MIN_ATTR, MAX_ATTR = 1, 99
MAX_RUNE_LEVEL = 713
STAT_COUNT = 8

# The serialized `PlayerGameData` precedes the slot body's `FACE` magic; the
# PGD->face delta is not fixed (0x959c..0xa600 measured across real containers --
# see `bnd4::slot_player_game_data_offset`), so the window is deliberately wide
# and every candidate is checked against the same plausibility test the Rust
# locator uses, rather than assumed.
FACE_MAGIC = b"FACE"
PGD_TO_FACE_MIN = 0x9000
PGD_TO_FACE_MAX = 0xB000


def parse_entries(data: bytes) -> dict[str, tuple[int, int]]:
    if len(data) < 0x40 or data[:4] != BND4_MAGIC:
        raise SystemExit("not a BND4 save container")
    header_size = struct.unpack_from("<q", data, HDR_HEADER_SIZE_OFF)[0]
    stride = struct.unpack_from("<q", data, HDR_FILE_HEADER_SIZE_OFF)[0]
    count = struct.unpack_from("<i", data, HDR_FILE_COUNT_OFF)[0]
    out: dict[str, tuple[int, int]] = {}
    for i in range(count):
        h = header_size + i * stride
        size = struct.unpack_from("<q", data, h + ENT_SIZE_OFF)[0]
        offset = struct.unpack_from("<i", data, h + ENT_DATA_OFFSET_OFF)[0]
        name_off = struct.unpack_from("<i", data, h + ENT_NAME_OFFSET_OFF)[0]
        units = []
        cursor = name_off
        while True:
            unit = struct.unpack_from("<H", data, cursor)[0]
            if unit == 0:
                break
            units.append(unit)
            cursor += 2
        out["".join(map(chr, units))] = (offset, size)
    return out


def first_accepted_stat_block(body: bytes):
    """The first byte offset `located_stat_block` would accept, or None."""
    raw = np.frombuffer(body, dtype=np.uint8)
    best = None
    for align in range(4):
        arr = raw[align:]
        arr = arr[: len(arr) // 4 * 4].view("<i4")
        if arr.size < STAT_COUNT + LEVEL_FROM_STAT_BASE // 4:
            continue
        in_range = (arr >= MIN_ATTR) & (arr <= MAX_ATTR)
        runs = np.convolve(in_range.astype(np.int32), np.ones(STAT_COUNT, np.int32), "valid")
        cand = np.nonzero(runs == STAT_COUNT)[0]
        if cand.size == 0:
            continue
        level_idx = cand + LEVEL_FROM_STAT_BASE // 4
        keep = level_idx < arr.size
        cand, level_idx = cand[keep], level_idx[keep]
        if cand.size == 0:
            continue
        sums = arr[np.add.outer(cand, np.arange(STAT_COUNT))].sum(axis=1)
        level = arr[level_idx]
        good = (level == sums - RUNE_LEVEL_BASE) & (level >= 1) & (level <= MAX_RUNE_LEVEL)
        hit = np.nonzero(good)[0]
        if hit.size == 0:
            continue
        i = int(hit[0])
        base = int(cand[i]) * 4 + align
        entry = (base, [int(v) for v in arr[cand[i] : cand[i] + STAT_COUNT]], int(level[i]))
        if best is None or entry[0] < best[0]:
            best = entry
    return best


def true_pgd(body: bytes):
    """Face-anchored `PlayerGameData`: (pgd_offset, name, level, attrs) or None.

    Independent of the rune-level identity on purpose -- this is what tells a
    rejected slot (a real character the identity refuses) apart from an empty one.
    """
    raw = np.frombuffer(body, dtype=np.uint8)
    magic = np.frombuffer(FACE_MAGIC, dtype=np.uint8)
    hits = np.nonzero(
        (raw[:-3] == magic[0])
        & (raw[1:-2] == magic[1])
        & (raw[2:-1] == magic[2])
        & (raw[3:] == magic[3])
    )[0]
    for face in hits.tolist()[:8]:
        lo = max(0, face - PGD_TO_FACE_MAX)
        hi = max(0, face - PGD_TO_FACE_MIN)
        for pgd in range(lo, hi + 1):
            if pgd + 0x120 > len(body):
                continue
            level = struct.unpack_from("<i", body, pgd + PGD_LEVEL)[0]
            if not 1 <= level <= MAX_RUNE_LEVEL:
                continue
            attrs = list(struct.unpack_from("<8i", body, pgd + PGD_STAT_BASE))
            if not all(MIN_ATTR <= a <= MAX_ATTR for a in attrs):
                continue
            units = []
            for i in range(PGD_NAME_LEN_U16):
                unit = struct.unpack_from("<H", body, pgd + PGD_NAME + i * 2)[0]
                if unit == 0:
                    break
                units.append(unit)
            name = "".join(map(chr, units)).strip()
            if not name:
                continue
            return pgd, name, level, attrs
    return None


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("save", type=Path, help="ER0000.sl2 / ER0000.co2")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    data = args.save.read_bytes()
    entries = parse_entries(data)
    rows = []
    for slot in range(10):
        name = f"USER_DATA{slot:03d}"
        if name not in entries:
            continue
        offset, size = entries[name]
        body = data[offset + ENTRY_MD5_LEN : offset + size]
        accepted = first_accepted_stat_block(body)
        anchored = true_pgd(body)
        row = {
            "slot": slot,
            "accepted": accepted is not None,
            "name": anchored[1] if anchored else None,
            "level": anchored[2] if anchored else None,
            "attributes": anchored[3] if anchored else None,
        }
        if anchored:
            row["attribute_sum"] = sum(anchored[3])
            row["implied_level"] = sum(anchored[3]) - RUNE_LEVEL_BASE
            row["delta"] = anchored[2] - row["implied_level"]
        rows.append(row)
        if args.json:
            continue
        if anchored is None:
            print(f"slot {slot}: EMPTY (no FACE-anchored PlayerGameData)")
        elif accepted:
            print(
                f"slot {slot}: OK        name={anchored[1]!r} level={anchored[2]}"
                f" attrs={anchored[3]} sum={row['attribute_sum']} implies RL {row['implied_level']}"
            )
        else:
            print(
                f"slot {slot}: REJECTED  name={anchored[1]!r} level={anchored[2]}"
                f" attrs={anchored[3]} sum={row['attribute_sum']} implies RL {row['implied_level']}"
                f" (delta {row['delta']:+d}) -- er_save_loader::stats decodes this slot as None:"
                f" no attributes, no vitals, no WL"
            )
    if args.json:
        json.dump({"save": str(args.save), "slots": rows}, sys.stdout, indent=1)
        print()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
