#!/usr/bin/env python3
"""Corpus save explorer: find every ER0000.sl2/.co2 under a root that contains a
character with a given name, and report that character's slot, level, runes, and
highest weapon upgrade level.

WHY THIS EXISTS (bd build-finder-tool-dont-skip-solved-problems-2026-07-20):
the save-manager corpus dirs are labeled by the manager's OWN names, NOT the
in-game character name, so a directory-name grep for "angrE" finds nothing even
though the character exists. This reads the in-game name out of each save's
plaintext BND4 body (ER PC saves are plaintext, md5-per-slot) and maps
name -> absolute file path + slot + level + top weapon upgrade.

Decode reuses the evidence-bound scripts/save-slot-oracle.py (name @ player+0x94,
level @ player+0x60), the same decoder enumerate-valid-saves.py trusts. Highest
weapon upgrade is derived from the slot's GaItem table (see max_weapon_upgrade).

Usage:
    scripts/find-save-char.sh <root-dir> '<name>' [--exact] [--json]
    # e.g. scripts/find-save-char.sh ./ 'angrE'
"""
from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent


def _load_oracle():
    spec = importlib.util.spec_from_file_location("save_slot_oracle", HERE / "save-slot-oracle.py")
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# --- highest weapon upgrade -------------------------------------------------
# ER weapon reinforcement is encoded in the weapon's param id (fullId = baseId +
# reinforceLevel, reinforceLevel 0..25). The value LIVES in the slot's GaItem
# table, but that table has NO local structural spec here (docs/bnd4-save-format.md
# stops at the container; the SL2.bt internal ChrAsm/GaItem struct is not vendored).
# A SLOT-WIDE byte scan CANNOT isolate it: empirically a fresh level-7 character
# yields 16k+ "category-0" and 2.7k "0x8000_0000" (ash-of-war, NOT weapon) pair
# matches -- pure noise -- so any %100 over them fabricates a bogus "+20". Rather
# than emit a fabricated number, this returns None until it is backed by the real
# GaItem-table offset+stride (or ChrAsm equipped-weapon param ids). See bd
# find-save-char-weapon-upgrade-needs-gaitem-table-offset-2026-07-20.
def max_weapon_upgrade(slot_data: bytes) -> int | None:
    """Highest weapon reinforcement (+N) -- UNIMPLEMENTED reliably; returns None.

    A trustworthy value requires the GaItem table structure (offset+stride) or the
    ChrAsm equipped-weapon param ids, neither of which is vendored locally. A
    slot-wide heuristic is provably noise (fresh characters read a fake +20), so we
    report None ('?') instead of a fabricated level.
    """
    _ = slot_data
    return None


def scan_file(oracle, path: Path, query: str, exact: bool) -> list[dict[str, Any]]:
    try:
        data = path.read_bytes()
    except OSError:
        return []
    q = query.casefold()
    matches: list[dict[str, Any]] = []
    for slot in range(10):
        try:
            result = oracle.decode_save_slot(data, path, slot)
        except Exception:
            continue
        df = result.get("decoded_fields") or {}
        name = (df.get("name") or "").strip()
        level = df.get("level")
        if df.get("name_empty_like") or not name:
            continue
        hit = (name.casefold() == q) if exact else (q in name.casefold())
        if not hit:
            continue
        slot_data, _ = oracle.extract_slot(data, slot)
        matches.append(
            {
                "abspath": str(path.resolve()),
                "slot": slot,
                "name": name,
                "level": level,
                "runes": df.get("runes"),
                "max_weapon_upgrade": max_weapon_upgrade(slot_data),
                "ext": path.suffix.lower().lstrip("."),
            }
        )
    return matches


def main() -> int:
    ap = argparse.ArgumentParser(description="Find ER saves containing a named character.")
    ap.add_argument("root", help="directory to search recursively for ER0000.sl2/.co2")
    ap.add_argument("name", help="in-game character name to find (substring by default)")
    ap.add_argument("--exact", action="store_true", help="require an exact (case-insensitive) name match")
    ap.add_argument("--json", action="store_true", help="emit JSON instead of human lines")
    args = ap.parse_args()

    root = Path(args.root)
    if not root.is_dir():
        print(f"error: not a directory: {root}", file=sys.stderr)
        return 2

    def fmt(m: dict[str, Any]) -> str:
        wl = m["max_weapon_upgrade"]
        wl_s = f"+{wl}" if wl is not None else "?"
        return (
            f"{m['abspath']}\tslot={m['slot']}\tname={m['name']!r}\t"
            f"level={m['level']}\trunes={m['runes']}\ttop_weapon={wl_s}\t({m['ext']})"
        )

    oracle = _load_oracle()
    all_matches: list[dict[str, Any]] = []
    files = [
        p
        for p in sorted(root.rglob("ER0000.*"))
        if p.suffix.lower().lstrip(".") in ("sl2", "co2")
        and "er-effects-save-redirect-stage" not in p.as_posix()
    ]
    # Stream each match the instant its file decodes (flush=True) so a background
    # run can be monitored live for the expected name instead of blocking to the end.
    for i, p in enumerate(files):
        ms = scan_file(oracle, p, args.name, args.exact)
        all_matches.extend(ms)
        if not args.json:
            for m in ms:
                print(fmt(m), flush=True)
        elif ms:
            print(f"# [{i + 1}/{len(files)}] {len(ms)} match(es) in {p}", file=sys.stderr, flush=True)

    if args.json:
        print(json.dumps({"query": args.name, "exact": args.exact, "matches": all_matches}, indent=2))
    elif not all_matches:
        print(f"# no save under {root} contains a character matching '{args.name}'", flush=True)
    return 0 if all_matches else 1


if __name__ == "__main__":
    raise SystemExit(main())
