#!/usr/bin/env python3
"""Compact per-slot identity dump for an ER0000.sl2/.co2, for gold-save+slot validation.

Imports the evidence-bound decoder from save-slot-oracle.py and prints one line per slot
(name, level, class archetype, map id c30, runes), flagging real vs empty-like slots. Optional
--expect-name / --expect-level highlight (and, with --require, fail-closed on) the matching slot.

Usage:
  dump-save-slots.py <save.sl2> [--expect-name NAME] [--expect-level N] [--require]
"""
from __future__ import annotations

import argparse
import importlib.util
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


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("save", type=Path)
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
