#!/usr/bin/env python3
"""Compact per-slot identity dump for an ER0000.sl2/.co2, for gold-save+slot validation.

Imports the evidence-bound decoder from save-slot-oracle.py and prints one line per slot
(name, level, class archetype, map id c30, runes), flagging real vs empty-like slots. Optional
--expect-name / --expect-level highlight (and, with --require, fail-closed on) the matching slot.

Usage:
  dump-save-slots.py <save.sl2> [--expect-name NAME] [--expect-level N] [--require] [--deep]

--deep additionally walks each slot's serialized CSGaitemImp map (the first block the
game deserializes on load) and flags slots whose map would crash the loader. Evidence
(slot0-corrupt-save-invalidity-signature-2026-07-22): CS::CSGaitemImp::Deserialize
(dump 0x140671220, live rva 0x671130) walks EXACTLY 0x1400 {u32 handle, u32 itemId}
pairs; every pair with handle != 0 allocates a live gaitem via
GetGaItemHandle{Weapon,Protector,Accessory,Goods,Gem} (category = (handle>>28)&7),
which pops CSGaitemImp's free-index queue (pristine capacity 0x13ff, runtime-measured;
ring of 0x1400 loses one slot to the empty/full ambiguity). The deserialize NEVER
frees pre-existing entries first, and the title screen's default character already
holds allocations (CharaInitParam default equip, e.g. FUN_140259900 arrow/bolt slots),
so a saved map with 0x13ff non-empty entries underflows the queue mid-walk ->
GetUnindexedGaItemHandle returns handle 0 -> CSGaitemHandle::GetIndex -> -1 ->
gaitemInsTable[-1]->Deserialize() dispatches through CSGaitemImp's own vtable ptr
(rcx=0x142a7e430) -> the AV at game rva 0x67141a. A category >= 5 handle skips the
Get* entirely with the same -1 dispatch, immediately.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("save_slot_oracle", HERE / "save-slot-oracle.py")
oracle = importlib.util.module_from_spec(spec)
assert spec and spec.loader
spec.loader.exec_module(oracle)

ARCHETYPES = {
    0: "Vagabond", 1: "Warrior", 2: "Hero", 3: "Bandit", 4: "Astrologer", 5: "Prophet",
    6: "Confessor", 7: "Samurai", 8: "Prisoner", 9: "Wretch",
}

# ---- --deep gaitem-map walk (see module docstring for the RE evidence chain) ----
# Slot payload layout (slot_data includes the 0x10 per-slot MD5 prefix; the game's
# DLMemoryInputStream starts at slot_data+0x10): 0x10 version header (u32 version,
# u32 blockId, 8 bytes), then for current versions (FUN_1402624d0(version) true, all
# observed 0xfc saves) a 0x10-byte skip, then the CSGaitemImp map.
GAITEM_MAP_ANCHOR_WITH_SKIP = 0x30   # slot_data offset (stream +0x20)
GAITEM_MAP_ANCHOR_NO_SKIP = 0x20     # fallback for old versions without the skip
GAITEM_MAP_PAIR_COUNT = 0x1400
# Per-category ins Deserialize extra bytes after the {handle,itemId} pair, verified
# byte-exactly against this save (walk end == PlayerGameData block start on 10/10
# slots) and matching ClayAmore's SL2.bt GaItem template:
#   weapon: u32 unk, u32 unk, u32 aow_gaitem_handle, u8 unk  (13 bytes)
#   protector: u32 unk, u32 unk                              (8 bytes)
#   accessory/goods/gem: none observed (0 bytes; goods/accessory gaitems are
#   allocated later by inventory deserialize, not stored in this map)
GAITEM_CATEGORY_EXTRA_BYTES = {0: 13, 1: 8, 2: 0, 3: 0, 4: 0}
GAITEM_CATEGORY_NAMES = {0: "wep", 1: "pro", 2: "acc", 3: "goods", 4: "gem"}
# Free-index-queue pristine capacity (runtime-measured; see crates/er-effects-rs/src/
# constants/gaitem_restore.rs slack telemetry: full == 0x13ff free indices).
GAITEM_FREE_QUEUE_CAPACITY = 0x13FF
# A map with exactly 0x13ff allocating entries crashes whenever >= 1 gaitem is already
# resident (always true at the title: default-character equip). Leave headroom for the
# resident set so near-full maps are flagged too.
GAITEM_ALLOC_SAFETY_HEADROOM = 0x7F
GAITEM_ALLOC_INVALID_THRESHOLD = GAITEM_FREE_QUEUE_CAPACITY - GAITEM_ALLOC_SAFETY_HEADROOM  # 0x1380


def walk_gaitem_map(slot_data: bytes, anchor: int) -> dict:
    """Walk the serialized CSGaitemImp map exactly like CS::CSGaitemImp::Deserialize.

    Returns a dict with: ok (walk completed), allocs (non-empty entry count == free-queue
    pops the loader will perform), cats (per-category counts), end (offset after the map,
    == the PlayerGameData block start when the anchor is right), bad_category (first
    handle with (handle>>28)&7 >= 5, the instant-crash form), truncated (ran off the
    payload end).
    """
    off = anchor
    allocs = 0
    cats = {c: 0 for c in GAITEM_CATEGORY_EXTRA_BYTES}
    bad_category = None
    truncated = False
    for _ in range(GAITEM_MAP_PAIR_COUNT):
        if off + 8 > len(slot_data):
            truncated = True
            break
        handle, _item_id = struct.unpack_from("<II", slot_data, off)
        off += 8
        if handle == 0:
            continue
        cat = (handle >> 28) & 7
        if cat not in GAITEM_CATEGORY_EXTRA_BYTES:
            bad_category = (handle, cat)
            break
        allocs += 1
        cats[cat] += 1
        off += GAITEM_CATEGORY_EXTRA_BYTES[cat]
    return {
        "ok": bad_category is None and not truncated,
        "allocs": allocs,
        "cats": cats,
        "end": off,
        "bad_category": bad_category,
        "truncated": truncated,
    }


def deep_slot_analysis(slot_data: bytes) -> dict:
    """Gaitem-map validity analysis for one extracted USER_DATA00N payload.

    Verdicts: INVALID (would crash the loader), VALID (walk grounded + under threshold),
    INDETERMINATE (walk could not be grounded on a PlayerGameData block -- unknown layout,
    NOT proof of corruption).
    """
    stored_md5 = slot_data[:0x10]
    calc_md5 = hashlib.md5(slot_data[0x10:]).digest()
    md5_ok = stored_md5 == calc_md5
    result = None
    anchor_used = None
    grounded = False
    for anchor in (GAITEM_MAP_ANCHOR_WITH_SKIP, GAITEM_MAP_ANCHOR_NO_SKIP):
        walk = walk_gaitem_map(slot_data, anchor)
        # Ground-truth the walk: it must land exactly on a plausible PlayerGameData
        # block (the next thing PlayerGameData::Deserialize reads after the map).
        if walk["ok"] and oracle.plausible_player_game_data_core(slot_data, walk["end"]):
            result, anchor_used, grounded = walk, anchor, True
            break
        if result is None:
            result, anchor_used = walk, anchor
    reasons = []
    if not md5_ok:
        reasons.append("md5-mismatch")
    if result["bad_category"] is not None:
        handle, cat = result["bad_category"]
        reasons.append(f"bad-category handle=0x{handle:08x} cat={cat} (instant gaitemInsTable[-1] AV)")
    if result["truncated"]:
        reasons.append("map-truncated")
    if grounded and result["allocs"] >= GAITEM_ALLOC_INVALID_THRESHOLD:
        full = " FULL-TABLE(0x13ff)" if result["allocs"] >= GAITEM_FREE_QUEUE_CAPACITY else ""
        reasons.append(
            f"gaitem-map-allocs=0x{result['allocs']:x} >= 0x{GAITEM_ALLOC_INVALID_THRESHOLD:x}{full}"
            " (free-queue underflow -> AV @ rva 0x67141a)"
        )
    if reasons:
        verdict = "INVALID"
    elif grounded:
        verdict = "VALID"
    else:
        verdict = "INDETERMINATE"
    return {
        "verdict": verdict,
        "reasons": reasons,
        "md5_ok": md5_ok,
        "grounded": grounded,
        "anchor": anchor_used,
        **result,
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("save", type=Path)
    ap.add_argument("--expect-name", default=None)
    ap.add_argument("--expect-level", type=int, default=None)
    ap.add_argument("--require", action="store_true", help="exit 1 if no slot matches the expectation")
    ap.add_argument(
        "--deep",
        action="store_true",
        help="also walk each slot's CSGaitemImp map and flag loader-crashing slots (see module docstring)",
    )
    args = ap.parse_args()

    data = args.save.read_bytes()
    print(f"# {args.save}")
    matched: list[int] = []
    real = 0
    deep_invalid: list[int] = []
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
        arch = ARCHETYPES.get(f.get("archetype"), f.get("archetype"))
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
                slot_data, _src = oracle.extract_slot(data, slot)
                deep = deep_slot_analysis(slot_data)
            except Exception as exc:  # noqa: BLE001 - report structurally, never crash the dump
                print(f"    deep: <analysis error: {exc}>")
                continue
            cats = " ".join(
                f"{GAITEM_CATEGORY_NAMES[c]}={n}" for c, n in deep["cats"].items() if n
            ) or "none"
            why = f" reasons: {'; '.join(deep['reasons'])}" if deep["reasons"] else ""
            print(
                f"    deep: [{deep['verdict']}] gaitem_allocs=0x{deep['allocs']:x}/0x{GAITEM_FREE_QUEUE_CAPACITY:x}"
                f" cats[{cats}] md5={'OK' if deep['md5_ok'] else 'BAD'}"
                f" map@0x{deep['anchor']:x}..0x{deep['end']:x} grounded={deep['grounded']}{why}"
            )
            if deep["verdict"] == "INVALID":
                deep_invalid.append(slot)

    print(f"# real slots: {real}/{oracle.SLOT_COUNT}")
    if args.deep:
        if deep_invalid:
            print(f"# DEEP: loader-crashing (INVALID) slot(s): {deep_invalid}")
        else:
            print("# DEEP: no loader-crashing slots detected")
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
