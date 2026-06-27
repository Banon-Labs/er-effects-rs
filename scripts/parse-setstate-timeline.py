#!/usr/bin/env python3
"""Parse the menu-build-overlap trace: print the native title state-transition timeline
(title-setstate-trace lines) alongside the pab_dismiss / dialog-appears / menu_open markers
from the newest onscreen-capture run (or a path given as argv[1])."""
import sys, glob, os, re

if len(sys.argv) > 1:
    log = sys.argv[1]
else:
    cands = sorted(
        glob.glob("target/runtime-probe/onscreen-capture-*/er-effects-autoload-debug.log"),
        key=os.path.getmtime,
    )
    if not cands:
        print("no onscreen-capture log found")
        sys.exit(1)
    log = cands[-1]

print(f"LOG: {log}\n")
pat = re.compile(
    r"(title-setstate-trace|pab-advance: \*\*\* SET|pab-advance: press-any-button job READY"
    r"|title-accept-byte: set|title-anim-skip|title-anim-diag.*dialog|own_stepper: STAGE1c"
    r"|splash-skip: patched)"
)
for line in open(log, encoding="utf-8", errors="replace"):
    if pat.search(line):
        print(line.rstrip()[:200])
