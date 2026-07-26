#!/usr/bin/env python3
"""Analyze an er-input-harness PROBE-mode log to recover the in-world pause-menu input map.

Probe mode (drive.rs probe_menu_tick) opens the in-world pause menu, then sweeps DLUID virtual-key ids
1000..1080 into `source+0x88` (via the builder-hook producer path, bd
MENU-INPUT-LAYER-virtual-key-array-source-plus-0x88) and logs the menu response per id. This tool reads
that log and answers the three questions that decide whether the producer-hook works:

  1. Does the builder-hook FIRE in-world?            (builder_fires / bf increasing)
  2. Does OUR source match the GAME's writer source? (my_src == game_src -> we write the array the menu reads)
  3. Which ids move the menu?                          (job / flags / tab / return_title change per id = that
                                                        id's action) -> the empirical id->action map, OR
                                                        "no id responded" = hook fires but the menu still does
                                                        not consume our write (deeper producer problem).

Usage: python3 scripts/analyze-menu-probe.py <er-input-harness.log>
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

OPEN_RE = re.compile(
    r"probe OPEN f(?P<f>\d+) pause_menu=(?P<pm>\d+) builder_fires=(?P<bf>\d+) writer_fires=(?P<wf>\d+) "
    r"game_src=0x(?P<gsrc>[0-9a-f]+) my_src=0x(?P<msrc>[0-9a-f]+) obs=\[(?P<obs>[0-9a-fx,]+)\] "
    r"job=0x(?P<job>[0-9a-f]+) flags=0x(?P<flags>[0-9a-f]+)"
)
ID_RE = re.compile(
    r"probe id=(?P<id>\d+) f(?P<f>\d+) bf=(?P<bf>\d+) wf=(?P<wf>\d+) gsrc=0x(?P<gsrc>[0-9a-f]+) "
    r"msrc=0x(?P<msrc>[0-9a-f]+) job=0x(?P<job>[0-9a-f]+) flags=0x(?P<flags>[0-9a-f]+) "
    r"tab=(?P<tab>-?\d+) return_title=(?P<rt>\d+)"
)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    text = Path(sys.argv[1]).read_text(encoding="utf-8", errors="replace")

    opens = [m.groupdict() for m in OPEN_RE.finditer(text)]
    ids = [m.groupdict() for m in ID_RE.finditer(text)]
    print(f"log: {sys.argv[1]}")
    print(f"probe OPEN samples: {len(opens)}   probe id samples: {len(ids)}\n")

    if not opens and not ids:
        print("NO probe lines found -- not a probe-mode log (run with er-harness-drive-mode.txt='probe').")
        return 2

    # Q1/Q2 from the last OPEN sample (menu is up, pre-sweep baseline).
    base = opens[-1] if opens else None
    if base:
        bf, gsrc, msrc = int(base["bf"]), base["gsrc"], base["msrc"]
        print("=== Q1 builder-hook fires + Q2 source match (baseline, menu open) ===")
        print(f"  builder_fires = {bf}   -> {'FIRES' if bf > 0 else 'NOT FIRING (hook not on the frame path!)'}")
        match = gsrc == msrc and gsrc not in ("0", "")
        print(f"  game_src=0x{gsrc}  my_src=0x{msrc}  -> {'MATCH (we write the array the menu reads)' if match else 'MISMATCH -- our source is wrong; the menu reads a different object' if gsrc not in ('0','') else 'game_src unknown (game writer never fired -> no real input seen)'}")
        obs = base["obs"]
        print(f"  game-writer observed ids (obs bitset): {obs}  (nonzero => the game itself writes source+0x88 for real input)\n")

    # Q3: per-id menu response. Baseline job/flags/tab = the mode across id samples (steady menu-open value).
    if ids:
        from collections import Counter
        # Baseline = the steady MENU-OPEN state, not the global mode: closed-menu samples (job==0) would
        # otherwise dominate the mode and make every open-menu sample look like a "response". Restrict the
        # baseline to samples where the menu is open (job != 0); fall back to the global mode if none.
        opened = [r for r in ids if r["job"] not in ("0", "")]
        pool = opened or ids
        base_job = Counter(r["job"] for r in pool).most_common(1)[0][0]
        base_flags = Counter(r["flags"] for r in pool).most_common(1)[0][0]
        base_tab = Counter(r["tab"] for r in pool).most_common(1)[0][0]
        print(f"=== Q3 id->action map (baseline job=0x{base_job} flags=0x{base_flags} tab={base_tab}) ===")
        responders = []
        by_id: dict[str, list[dict]] = {}
        for r in ids:
            by_id.setdefault(r["id"], []).append(r)
        for _id, allrows in sorted(by_id.items(), key=lambda kv: int(kv[0])):
            # Only compare samples where the menu was open for this id (job != 0); a closed sample would
            # spuriously read as a job change vs the open baseline.
            rows = [r for r in allrows if r["job"] not in ("0", "")] or allrows
            changed = []
            if any(r["job"] != base_job for r in rows):
                changed.append(f"job->{next(r['job'] for r in rows if r['job'] != base_job)}")
            if any(r["flags"] != base_flags for r in rows):
                changed.append(f"flags->0x{next(r['flags'] for r in rows if r['flags'] != base_flags)}")
            if any(r["tab"] != base_tab for r in rows):
                changed.append(f"tab->{next(r['tab'] for r in rows if r['tab'] != base_tab)}")
            if any(r["rt"] != '0' for r in rows):
                changed.append("return_title!")
            if changed:
                responders.append((_id, changed))
        if responders:
            print(f"  {len(responders)} id(s) MOVED the menu -> the input map is real:")
            for _id, ch in responders:
                print(f"    id={_id} ({int(_id)-1000} in array): {', '.join(ch)}")
        else:
            print("  NO id moved the menu (job/flags/tab/return_title all constant across the whole sweep).")
            print("  => the producer-hook writes source+0x88 but the menu STILL does not consume it.")
            print("     Next: the menu reads a DIFFERENT array/object than source+0x88 (re-RE the consumer),")
            print("     or the write is still wiped after the hook (wrong builder / ordering).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
