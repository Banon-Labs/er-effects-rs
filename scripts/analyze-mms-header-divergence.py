#!/usr/bin/env python3
"""Analyze the MoveMapStep header window across advancer ticks in an er-reload-trace.log.

The trace DLL's finalize-advancer hook now logs, per tick:
  finalize_advancer_afa6d0 call#N mms=0xADDR field25_12a A->B menuData_5d=.. 5e=.. mmshdr[+0=0x.. +8=0x.. ...] <snapshot>

Each distinct mms=0xADDR is one load epoch's MoveMapStep. load2's Update is dropped by the FD4
scheduler after ~6 ticks (field25 stuck at 0) while load1 ticks ~145x and field25 walks 0->9. This
tool groups ticks by mms, prints each load's tick count + field25 progression, and DIFFS the header
windows to surface the offset that (a) stays constant through load1's full walk but (b) differs or
flips in the short-lived load2 group -- the candidate 'active/schedule' field that de-lists load2's
child from the FD4 tick set. Read-only; no game state.

Usage: python3 scripts/analyze-mms-header-divergence.py <path-to-er-reload-trace.log>
"""
from __future__ import annotations

import re
import sys
from collections import OrderedDict

LINE = re.compile(
    r"finalize_advancer_afa6d0 call#(\d+) mms=0x([0-9a-fA-F]+) "
    r"field25_12a (-?\d+)->(-?\d+).*?mmshdr\[(.*?)\]"
)
FIELD = re.compile(r"\+([0-9a-fA-F]+)=0x([0-9a-fA-F]+)")


def parse(path: str) -> "OrderedDict[str, list[dict]]":
    groups: OrderedDict[str, list[dict]] = OrderedDict()
    for line in open(path, encoding="utf-8", errors="replace"):
        m = LINE.search(line)
        if not m:
            continue
        call, mms, f_before, f_after, hdr = m.groups()
        fields = {int(o, 16): int(v, 16) for o, v in FIELD.findall(hdr)}
        groups.setdefault(mms, []).append(
            {
                "call": int(call),
                "f_before": int(f_before),
                "f_after": int(f_after),
                "hdr": fields,
            }
        )
    return groups


def summarize_group(mms: str, ticks: list[dict]) -> dict:
    f25 = [t["f_after"] for t in ticks]
    # which offsets change WITHIN this load's lifetime
    changing = {}
    offs = ticks[0]["hdr"].keys()
    for off in offs:
        vals = [t["hdr"].get(off) for t in ticks]
        uniq = [v for v in dict.fromkeys(vals)]
        if len(uniq) > 1:
            changing[off] = uniq
    return {
        "mms": mms,
        "ticks": len(ticks),
        "field25_progression": f"{f25[0]}..{f25[-1]}" if f25 else "?",
        "field25_max": max(f25) if f25 else None,
        "field25_walk": len(dict.fromkeys(f25)) > 1,
        "first_hdr": ticks[0]["hdr"],
        "last_hdr": ticks[-1]["hdr"],
        "changing_within": changing,
    }


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    groups = parse(sys.argv[1])
    if not groups:
        print("no finalize_advancer_afa6d0 mmshdr lines found (old DLL, or advancer never ticked?)")
        return 1
    sums = [summarize_group(mms, ticks) for mms, ticks in groups.items()]
    # load1 = the group that walks field25 furthest / most ticks; the short stuck ones are reloads.
    sums.sort(key=lambda s: (s["field25_max"] or 0, s["ticks"]), reverse=True)
    print(f"== {len(sums)} MoveMapStep epoch(s) (by distinct mms ptr) ==\n")
    for i, s in enumerate(sums):
        role = "load1/COMPLETES" if i == 0 and s["field25_walk"] else f"reload/STUCK#{i}"
        print(
            f"[{role}] mms=0x{s['mms']} ticks={s['ticks']} "
            f"field25={s['field25_progression']} (max {s['field25_max']})"
        )
        if s["changing_within"]:
            for off, vals in sorted(s["changing_within"].items()):
                print(f"    +0x{off:x} CHANGES within this load: {[hex(v) for v in vals]}")
        print()
    # Cross-epoch divergence: offsets whose FIRST-tick value differs between load1 and each reload.
    if len(sums) >= 2:
        base = sums[0]
        print("== cross-epoch first-tick header divergence (load1 vs each reload) ==")
        for s in sums[1:]:
            print(f"-- load1 mms=0x{base['mms']}  vs  reload mms=0x{s['mms']} --")
            offs = sorted(set(base["first_hdr"]) | set(s["first_hdr"]))
            for off in offs:
                a = base["first_hdr"].get(off)
                b = s["first_hdr"].get(off)
                if a != b:
                    print(f"    +0x{off:x}: load1=0x{a:x} reload=0x{b:x}  <-- DIFFERS")
            # also: what does the reload's LAST tick look like vs its first (the drop signature)
            drop = {
                off: (s["first_hdr"].get(off), s["last_hdr"].get(off))
                for off in s["first_hdr"]
                if s["first_hdr"].get(off) != s["last_hdr"].get(off)
            }
            if drop:
                print("    reload header fields that changed first->last tick (drop signature):")
                for off, (a, b) in sorted(drop.items()):
                    print(f"      +0x{off:x}: 0x{a:x} -> 0x{b:x}")
            print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
