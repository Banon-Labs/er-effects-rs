#!/usr/bin/env python3
"""Compact per-slot identity dump for an ER0000.sl2/.co2, for gold-save+slot validation.

Imports the evidence-bound decoder from save-slot-oracle.py and prints one line per slot
(name, level, class archetype, map id c30, runes), flagging real vs empty-like slots. Optional
--expect-name / --expect-level highlight (and, with --require, fail-closed on) the matching slot.

--deep additionally walks each slot's CS::CSGaitemImp gaitem-handle map (the array the loader
deserializes at CSGaitemImp::Deserialize) and reports the non-empty count, whether it trips the
loader-crash INVALID predicate, and the top item-id tallies (to spot editor-generated bulk like
"2513x item X"). See bd memory slot0-corrupt-save-invalidity-signature-2026-07-22 for the RE chain.

Usage:
  dump-save-slots.py <save.sl2> [--deep] [--expect-name NAME] [--expect-level N] [--require]
"""
from __future__ import annotations

import argparse
import importlib.util
import struct
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("save_slot_oracle", HERE / "save-slot-oracle.py")
assert spec and spec.loader
oracle = importlib.util.module_from_spec(spec)
spec.loader.exec_module(oracle)

ARCHETYPES = {
    0: "Vagabond", 1: "Warrior", 2: "Hero", 3: "Bandit", 4: "Astrologer", 5: "Prophet",
    6: "Confessor", 7: "Samurai", 8: "Prisoner", 9: "Wretch",
}

# --- Gaitem-map walk (--deep) -------------------------------------------------
# The decrypted slot payload begins with a 0x10-byte MD5 header, then a 0x20-byte
# skip, then CS::CSGaitemImp's gaitem-handle map: a packed run of {u32 handle,
# u32 itemId} pairs.  slot_data offset = 0x10 (md5) + 0x20 (skip) = 0x30.  The
# game's in-memory table has 0x1400 (5120) capacity, so the loader walks up to
# 0x1400 pairs; a genuinely full editor map fills all 0x1400 entries (the crash
# signature).  Verified against the corpus 2026-07-23: at 0x30 the first pairs
# decode as clean sequential gem handles (0xc081xxxx / 0xc0b8xxxx, itemId top
# bit 0x80000000 = Ash-of-War), confirming the offset from the bd RE memory.
GAITEM_MAP_OFFSET = 0x30
GAITEM_MAP_CAPACITY = 0x1400
GAITEM_PAIR_SIZE = 8
# Near-capacity fill underflows the loader's free-index queue -> AV at CSGaitemImp
# rva 0x67141a.  0x13ff (== capacity-1) is guaranteed fatal; 0x1380 leaves 0x7f
# headroom (bd slot0-corrupt-save-invalidity-signature-2026-07-22).
GAITEM_INVALID_NONEMPTY = 0x1380
# A live gaitem handle is {category-nibble | instance-byte | table-index} with
# the top bit set, so its top nibble is 0x8..0xc == categories 0..4
# (Weapon/Protector/Accessory/Goods/Gem via (handle>>28)&7).  A non-empty pair
# whose top nibble is outside 0x8..0xc is NOT a gaitem handle: it is the first
# byte of the variable-length gaitem *stream* / following save data that trails
# the packed map (the naive "walk all 0x1400" from the RE note reads into it and
# reports hundreds of bogus class>=5 "handles" even on healthy saves -- see the
# `flat` figures).  We therefore bound the map at that first non-handle pair.
GAITEM_HANDLE_TOP_NIBBLE_MIN = 0x8
GAITEM_HANDLE_TOP_NIBBLE_MAX = 0xc


def _read_u32(data: bytes, off: int) -> int | None:
    if off < 0 or off + 4 > len(data):
        return None
    return struct.unpack_from("<I", data, off)[0]


def walk_gaitem_map(slot_data: bytes) -> dict:
    """Walk the CSGaitemImp gaitem-handle map and classify it.

    Returns the bounded-map view (the real deserialized entries) plus the raw
    "flat" full-0x1400 view for transparency, the invalid verdict, and item-id
    tallies over the bounded map.
    """
    map_nonempty = 0
    map_badclass = 0  # class>=5 handle inside the bounded map (should be 0)
    unique_handles: set[int] = set()
    item_ids: Counter[int] = Counter()
    boundary = GAITEM_MAP_CAPACITY
    boundary_reason = "capacity"
    for i in range(GAITEM_MAP_CAPACITY):
        off = GAITEM_MAP_OFFSET + i * GAITEM_PAIR_SIZE
        handle = _read_u32(slot_data, off)
        item_id = _read_u32(slot_data, off + 4)
        if handle is None or item_id is None:
            boundary = i
            boundary_reason = "eof"
            break
        if handle == 0:
            continue  # empty slot in the packed map
        top_nibble = handle >> 28
        if top_nibble < GAITEM_HANDLE_TOP_NIBBLE_MIN or top_nibble > GAITEM_HANDLE_TOP_NIBBLE_MAX:
            boundary = i
            boundary_reason = "stream" if (handle & 0x80000000) == 0 else "badclass"
            break
        map_nonempty += 1
        if ((handle >> 28) & 7) >= 5:  # unreachable given the nibble gate, kept explicit
            map_badclass += 1
        unique_handles.add(handle)
        item_ids[item_id] += 1

    # Raw flat walk over the full 0x1400 capacity (the literal RE recipe); on a
    # non-full save this reads into the trailing stream, so its counts are
    # inflated and its class>=5 tally is noise -- reported only for transparency.
    flat_nonempty = 0
    flat_badclass = 0
    for i in range(GAITEM_MAP_CAPACITY):
        handle = _read_u32(slot_data, GAITEM_MAP_OFFSET + i * GAITEM_PAIR_SIZE)
        if handle is None:
            break
        if handle != 0:
            flat_nonempty += 1
            if ((handle >> 28) & 7) >= 5:
                flat_badclass += 1

    invalid_reasons = []
    if map_nonempty >= GAITEM_INVALID_NONEMPTY:
        invalid_reasons.append(f"nonempty 0x{map_nonempty:x}>=0x{GAITEM_INVALID_NONEMPTY:x}")
    if map_badclass:
        invalid_reasons.append(f"{map_badclass} class>=5 handle(s)")
    return {
        "map_nonempty": map_nonempty,
        "map_badclass": map_badclass,
        "unique_handles": len(unique_handles),
        "unique_item_ids": len(item_ids),
        "boundary_idx": boundary,
        "boundary_reason": boundary_reason,
        "flat_nonempty": flat_nonempty,
        "flat_badclass": flat_badclass,
        "invalid": bool(invalid_reasons),
        "invalid_reasons": invalid_reasons,
        "top_item_ids": item_ids.most_common(6),
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("save", type=Path)
    ap.add_argument("--deep", action="store_true",
                    help="also walk each slot's CSGaitemImp gaitem map (count, INVALID verdict, top item-ids)")
    ap.add_argument("--expect-name", default=None)
    ap.add_argument("--expect-level", type=int, default=None)
    ap.add_argument("--require", action="store_true", help="exit 1 if no slot matches the expectation")
    args = ap.parse_args()

    data = args.save.read_bytes()
    print(f"# {args.save}")
    matched: list[int] = []
    real = 0
    for slot in range(oracle.SLOT_COUNT):
        try:
            res = oracle.decode_save_slot(data, args.save, slot)
        except Exception as exc:  # noqa: BLE001 - report structurally, never crash the dump
            print(f"slot {slot}: <decode error: {exc}>")
            continue
        f = res.get("decoded_fields") or {}
        name = f.get("name", "")
        empty = bool(f.get("name_empty_like", True))
        level = f.get("level")
        _arch = f.get("archetype")
        arch = ARCHETYPES.get(_arch, _arch) if isinstance(_arch, int) else _arch
        c30 = f.get("saved_map_c30")
        c30s = f"0x{c30:x}" if isinstance(c30, int) else c30
        runes = f.get("runes")
        if not empty:
            real += 1
        flag = ""
        name_ok = args.expect_name is None or name.strip().lower() == args.expect_name.strip().lower()
        level_ok = args.expect_level is None or level == args.expect_level
        if (args.expect_name is not None or args.expect_level is not None) and name_ok and level_ok and not empty:
            flag = "  <== MATCH"
            matched.append(slot)
        tag = "empty" if empty else "REAL "
        print(f"slot {slot}: [{tag}] name={name!r:24} level={level} class={arch} c30={c30s} runes={runes}{flag}")
        if args.deep:
            try:
                slot_data, _ = oracle.extract_slot(data, slot)
                g = walk_gaitem_map(slot_data)
            except Exception as exc:  # noqa: BLE001 - never crash the dump on a deep-walk error
                print(f"         gaitem: <walk error: {exc}>")
            else:
                verdict = ("INVALID(" + "; ".join(g["invalid_reasons"]) + ")") if g["invalid"] else "valid"
                fill = 100.0 * g["map_nonempty"] / GAITEM_MAP_CAPACITY
                tops = ", ".join(f"0x{iid:08x}x{cnt}" for iid, cnt in g["top_item_ids"])
                print(
                    f"         gaitem: nonempty=0x{g['map_nonempty']:x}({g['map_nonempty']}) "
                    f"uniq_ids={g['unique_item_ids']} fill={fill:.1f}% of 0x{GAITEM_MAP_CAPACITY:x} "
                    f"=> {verdict}  [flat0x1400={g['flat_nonempty']} bad={g['flat_badclass']} incl-stream, "
                    f"mapend=0x{g['boundary_idx']:x}/{g['boundary_reason']}]"
                )
                if tops:
                    print(f"         gaitem top item-ids: {tops}")

    print(f"# real slots: {real}/{oracle.SLOT_COUNT}")
    if args.expect_name is not None or args.expect_level is not None:
        want = f"name={args.expect_name!r} level={args.expect_level}"
        if matched:
            print(f"# MATCH for {want}: slot(s) {matched}")
        else:
            print(f"# NO MATCH for {want}")
            if args.require:
                return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
