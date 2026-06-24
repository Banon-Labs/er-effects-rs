#!/usr/bin/env python3
"""Find the driver loop of a boot phase from the profiler's stack-scan data.

For main-thread samples in a time window, histogram the eldenring.exe return-addresses captured in
each sample's `stk` (stack scan). A spurious in-module pointer appears rarely; a real persistent
ancestor (the phase driver + its callers) appears in most samples. Ranking by frequency isolates
the real call chain above a hot leaf -> tells us whether the phase is a loop over independent items.

Usage: boot-profile-stacks.py <run_dir> <t0_ms> <t1_ms>
Prints the top frame addresses (deobf VAs) by sample-frequency; symbolize with dump-deobf-shift +
NameAddrs.
"""
from __future__ import annotations

import collections
import json
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) < 4:
        print("usage: boot-profile-stacks.py <run_dir> <t0_ms> <t1_ms>", file=sys.stderr)
        return 2
    run, t0, t1 = Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
    lines = (run / "er-effects-profile.jsonl").read_text(encoding="utf-8", errors="replace").splitlines()
    samples = [json.loads(l) for l in lines[1:] if l.strip()]

    # Busiest thread = main.
    prev, cpu = {}, collections.Counter()
    for o in samples:
        for t in o.get("t", []):
            c = t.get("k", 0) + t.get("u", 0)
            if t["id"] in prev and c >= prev[t["id"]]:
                cpu[t["id"]] += c - prev[t["id"]]
            prev[t["id"]] = c
    main = cpu.most_common(1)[0][0]

    frame_hits = collections.Counter()   # address -> # samples containing it
    pair_hits = collections.Counter()    # (caller-ish, callee-ish) adjacency, for chain ordering
    n = 0
    for o in samples:
        if not (t0 <= o["ms"] <= t1):
            continue
        for t in o.get("t", []):
            if t["id"] != main:
                continue
            stk = t.get("stk")
            if not stk:
                continue
            n += 1
            uniq = list(dict.fromkeys(stk))  # dedupe within a sample, keep order (inner->outer)
            for a in uniq:
                frame_hits[a] += 1
            for a, b in zip(uniq, uniq[1:]):
                pair_hits[(a, b)] += 1
    print(f"main tid={main}, samples-with-stack in [{t0},{t1}]ms = {n}")
    print("top frames by sample-frequency (deobf VA  count  pct):")
    for a, c in frame_hits.most_common(35):
        print(f"  0x{a:x}  {c}  {100*c/max(1,n):.0f}%")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
