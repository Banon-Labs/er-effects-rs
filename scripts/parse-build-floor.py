#!/usr/bin/env python3
"""Decompose the post-outro-skip build/settle to pin the init-gated menu_open floor.
Reads the newest onscreen-capture log (or argv[1]) and prints, in ms from process start:
  pab_dismiss, outro-skip fire, SetState(2/3/10), BUILD-done (dialog SM first != None),
  SETTLE-done/menu_open (title-accept-byte), plus the build and settle sub-durations."""
import sys, glob, os, re

log = sys.argv[1] if len(sys.argv) > 1 else sorted(
    glob.glob("target/runtime-probe/onscreen-capture-*/er-effects-autoload-debug.log"),
    key=os.path.getmtime)[-1]
lines = open(log, encoding="utf-8", errors="replace").read().splitlines()

def ms(l):
    m = re.match(r"\[\+(\d+)ms\]", l)
    return int(m.group(1)) if m else None

def first(substr, pred=None):
    for l in lines:
        if substr in l and (pred is None or pred(l)):
            return ms(l), l
    return None, None

pab, _ = first("pab-advance: *** SET")
fire, _ = first("pab-outro-skip: *** SET")
ss2, _ = first("SetState(owner", lambda l: "state=2)" in l)
ss3, _ = first("SetState(owner", lambda l: "state=3)" in l)
ss10b, _ = first("SetState(owner", lambda l: "state=10)" in l and "committed_was=3" in l)
mo, _ = first("title-accept-byte: set")

# BUILD-done = first build-floor frame whose SM is Some(...) (dialog became TitleTopDialog)
build_done = None
for l in lines:
    if "build-floor:" in l and "sm(" in l and "=Some(" in l:
        build_done = ms(l); break
# settle markers: first build-floor frame in Loop with latch 0
settle_loop = None
for l in lines:
    if "build-floor:" in l and "=Some(" in l and ", true, " in l:  # loop flag position varies; show raw
        pass

print(f"LOG {os.path.basename(os.path.dirname(log))}")
def show(name, v): print(f"  {name:14} {v}")
show("pab_dismiss", pab)
show("outro_fire", fire)
show("SetState(2)", ss2)
show("SetState(3)", ss3)
show("SetState(10)", ss10b)
show("BUILD_done", build_done)
show("menu_open", mo)
if pab is not None and mo is not None:
    show("WINDOW pab->mo", mo - pab)
if ss10b is not None and build_done is not None:
    show("build ms", build_done - ss10b)
if build_done is not None and mo is not None:
    show("settle ms", mo - build_done)
# Parse build-floor frames: (t, dialog_some, loop, latch)
bf = []
for l in lines:
    if "build-floor:" in l:
        t = ms(l)
        some = "=Some(" in l
        # tuple is (dialog, fadein, loop, tfo, latch)
        loop = None; latch = None
        m = re.search(r"=Some\(\((\d+), (\w+), (\w+), (\w+), (\d+)", l)
        if m:
            loop = m.group(3) == "true"; latch = int(m.group(5))
        bf.append((t, some, loop, latch))
# accept-eligible = first frame dialog Some & loop & latch==0
elig = next((t for (t, s, lp, la) in bf if s and lp and la == 0), None)
show("accept_eligible", elig)
if elig is not None and mo is not None:
    show("STARVATION ms", mo - elig)  # menu ready -> menu_open: pure task-starvation
# largest inter-tick gaps (title task starvation)
gaps = sorted(((bf[i+1][0]-bf[i][0], bf[i][0], bf[i+1][0]) for i in range(len(bf)-1)), reverse=True)[:4]
print("  --- top title-task tick gaps (starvation) ---")
for g, a, b in gaps:
    print(f"    {g}ms  ({a} -> {b})")
