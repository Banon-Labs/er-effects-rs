#!/usr/bin/env python3
"""Reproduce `er-armament-icons`' Ash-of-War icon resolution offline, HUD path vs menu path.

Why this EXISTS: the two surfaces resolve the same ash through *different* code and
disagree. The menu path (`crates/er-armament-icons/src/lib.rs`, `real_icon_id = gem_icon_id`)
draws only the `EquipParamGem` item icon and hides the badge when there is none. The HUD path
(`crates/er-armament-icons/src/hud_badge.rs:441-445`) additionally falls back to
`SwordArtsParam.iconId`, a different icon family. This script says, for every ash, what each
surface would draw -- so the divergence is a table instead of a bug report.

Reads `regulation.bin` directly through `scripts/regulation-params.py`'s decode pipeline
(AES -> DCX/zstd -> BND4 -> PARAM). No Smithbox, no dotnet, no game process.

Field offsets are the ones the DLL itself uses, and the `--validate` mode checks them against
values observed in the live `er-armament-icons.log`:
    EquipParamGem.iconId            u16 @ row+0x04
    EquipParamGem.swordArtsParamId  i32 @ row+0x18
    SwordArtsParam.iconId           u16 @ row+0x1A

    python3 scripts/armament-icon-resolution-census.py
    python3 scripts/armament-icon-resolution-census.py --arts 1174 1170 1189
"""

import argparse
import importlib.util
import os
import struct
import sys
from collections import Counter

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _load_regulation_reader():
    path = os.path.join(REPO, "scripts", "regulation-params.py")
    spec = importlib.util.spec_from_file_location("regulation_params", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def param_rows(files, stem):
    """{row id: (buffer, data offset)} for one PARAM, in file order."""
    key = next(
        (n for n in files if n.rsplit("\\", 1)[-1].removesuffix(".param") == stem), None
    )
    if key is None:
        raise SystemExit(f"{stem}: not present in this regulation")
    buf = files[key]
    count = struct.unpack_from("<H", buf, 0x0A)[0]
    out = {}
    for index in range(count):
        entry = 0x40 + index * 24
        row_id = struct.unpack_from("<i", buf, entry)[0]
        out[row_id] = (buf, struct.unpack_from("<q", buf, entry + 8)[0])
    return out


# Observed in the live log's `badge sample: DRAWN` lines, which print gem_icon and arts_icon
# for the same arts id. Any offset drift (a new patch moving a field) breaks these first.
LIVE_GEM_ICON = {801: 8481, 503: 8402, 301: 8361, 203: 8333, 201: 8331, 4050: 8508, 210: 8339}
LIVE_ARTS_ICON = {503: 21045, 210: 21014, 801: 0, 301: 0, 203: 0, 201: 0, 4050: 0}


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("--regulation")
    parser.add_argument(
        "--arts", type=int, nargs="*", default=[], help="report just these swordArtsParamIds"
    )
    args = parser.parse_args()

    reader = _load_regulation_reader()
    regulation = args.regulation or reader.DEFAULT_REGULATION
    files = reader.bnd4_entries(reader.dcx_unpack(reader.decrypt(regulation)))

    gem = param_rows(files, "EquipParamGem")
    arts = param_rows(files, "SwordArtsParam")

    def gem_icon(row):
        buf, off = gem[row]
        return struct.unpack_from("<H", buf, off + 0x04)[0]

    def gem_arts(row):
        buf, off = gem[row]
        return struct.unpack_from("<i", buf, off + 0x18)[0]

    def arts_icon(row):
        buf, off = arts[row]
        return struct.unpack_from("<H", buf, off + 0x1A)[0]

    print(f"EquipParamGem: {len(gem)} rows   SwordArtsParam: {len(arts)} rows")

    print("\n=== OFFSET VALIDATION against live er-armament-icons.log ===")
    bad = 0
    for a, expected in sorted(LIVE_GEM_ICON.items()):
        got = gem_icon(a * 100) if a * 100 in gem else None
        ok = got == expected
        bad += not ok
        print(f"  gem  {a * 100:7d}.iconId  offline={got}  live={expected}  {'ok' if ok else 'MISMATCH'}")
    for a, expected in sorted(LIVE_ARTS_ICON.items()):
        got = arts_icon(a) if a in arts else None
        ok = got == expected
        bad += not ok
        print(f"  arts {a:7d}.iconId  offline={got}  live={expected}  {'ok' if ok else 'MISMATCH'}")
    if bad:
        raise SystemExit(f"{bad} offset validation failure(s) -- field layout drifted")

    def hud(a):
        """hud_badge.rs: resolve_gem_icon_id, else fall back to SwordArtsParam.iconId."""
        canonical = a * 100
        if canonical in gem and gem_arts(canonical) == a and gem_icon(canonical):
            return "GEM", gem_icon(canonical)
        fallback = arts_icon(a) if a in arts else 0
        return ("FALLBACK", fallback) if fallback else ("HIDE", 0)

    def menu(a):
        """lib.rs: gem icon only; icon_id == 0 hides the badge."""
        canonical = a * 100
        if canonical in gem and gem_arts(canonical) == a and gem_icon(canonical):
            return "GEM", gem_icon(canonical)
        return "HIDE", 0

    targets = args.arts or [a for a in sorted(arts) if a > 0]
    divergent = [(a, hud(a)[1]) for a in targets if hud(a)[0] == "FALLBACK"]

    if args.arts:
        print("\n=== REQUESTED ARTS ===")
        for a in args.arts:
            h, m = hud(a), menu(a)
            note = "  <<< HUD DRAWS PLACEHOLDER, MENU HIDES" if h[0] == "FALLBACK" else ""
            print(f"  arts {a:5d}  HUD={h[0]:8s}{h[1]:6d}   MENU={m[0]:4s}{m[1]:6d}{note}")

    print(f"\n=== DIVERGENCE: {len(divergent)} of {len(targets)} arts rows ===")
    print("HUD falls back to a SwordArtsParam icon where the menu correctly draws nothing.")
    for value, count in Counter(v for _, v in divergent).most_common():
        print(f"  fallback iconId {value}: {count} arts rows")

    reverse = {}
    for row in gem:
        carried = gem_arts(row)
        if carried > 0:
            reverse.setdefault(carried, []).append(row)
    recoverable = [a for a, _ in divergent if any(gem_icon(g) for g in reverse.get(a, []))]
    print(f"\n  of those, {len(recoverable)} DO have an icon-bearing gem row that")
    print("  `arts * 100` misses (a reverse swordArtsParamId index would recover them):")
    for a in recoverable:
        best = [g for g in reverse[a] if gem_icon(g)]
        print(f"    arts {a} -> gem {best} icon {[gem_icon(g) for g in best]}")
    print(f"  the remaining {len(divergent) - len(recoverable)} have no icon-bearing gem at all")
    print("  (weapon-unique ashes -- vanilla shows no badge for these).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
