#!/usr/bin/env python3
"""Sample the PE-side call stack of a thread in a running Wine/Proton process.

Read-only. Opens /proc/<pid>/mem and /proc/<pid>/task/<tid>/syscall; it does not
ptrace-attach, inject, or suspend anything. Safe to point at a live Elden Ring --
unlike frida.attach(), which kills it (see AGENTS.md).

Why it exists: when eldenring.exe wedges (loading bar stuck, main thread spinning)
the only question that matters is "what code is on the stack". gdb/eu-stack cannot
unwind Wine's PE frames -- there is no eh_frame for a PE image mapped by Wine -- so
the practical method is to take the thread's stack pointer at a syscall boundary,
read a window of stack memory, and report every qword that lands inside a mapped
PE module's executable range. Those are (mostly) return addresses. Frequency across
samples separates the real frames from stale slots.

eldenring.exe maps at 0x140000000, which is also the Ghidra 1.16.2 dump base with
shift 0, so a reported eldenring.exe address can be fed straight to
scripts/ghidra/mcp_query.py getFunctionByAddress.

  python3 scripts/er-pe-stack-sample.py <pid> [--samples N] [--tid TID]
                                        [--window bytes] [--all-modules]
  python3 scripts/er-pe-stack-sample.py --selftest
"""

from __future__ import annotations

import argparse
import collections
import os
import struct
import sys
import time

# Modules worth reporting by default: the game, the mods, and the loaders that
# actually appear in game-thread stacks. Everything else is noise.
DEFAULT_INTEREST = (
    "eldenring.exe",
    "ersc.dll",
    "me3_mod_host.dll",
    "eossdk-win64-shipping.dll",
    "oo2core_6_win64.dll",
    "steamclient64.dll",
    "lsteamclient.dll",
    "steam_api64.dll",
    "d3d12core.dll",
    "dxgi.dll",
    "GameOverlayRenderer64.dll",
)


def read_modules(pid: str) -> "collections.OrderedDict[str, list[int]]":
    """Merge /proc/<pid>/maps into one span per backing file."""
    mods: "collections.OrderedDict[str, list[int]]" = collections.OrderedDict()
    with open(f"/proc/{pid}/maps", encoding="utf-8", errors="replace") as fh:
        for line in fh:
            parts = line.rstrip("\n").split(None, 5)
            if len(parts) < 6:
                continue
            name = parts[5].strip()
            if not name or name.startswith("["):
                continue
            lo_s, hi_s = parts[0].split("-")
            lo, hi = int(lo_s, 16), int(hi_s, 16)
            if name in mods:
                mods[name][0] = min(mods[name][0], lo)
                mods[name][1] = max(mods[name][1], hi)
            else:
                mods[name] = [lo, hi]
    return mods


def make_resolver(mods, interest, all_modules: bool):
    # Sorted spans + bisect would be tidier, but the module count is ~120 and the
    # hot loop is dominated by the /proc/mem read, so a linear scan is fine.
    spans = []
    for name, (lo, hi) in mods.items():
        base = os.path.basename(name)
        if all_modules or base in interest:
            spans.append((lo, hi, base))

    def resolve(addr: int):
        for lo, hi, base in spans:
            if lo <= addr < hi:
                return base, addr - lo
        return None, 0

    return resolve


def sample(pid: str, tid: str, nsamples: int, window: int, resolve, budget: float):
    """Collect nsamples stack windows, return (count, Counter of frames, first raw dump).

    The retry path yields the timeslice (`os.sched_yield`) rather than sleeping. There is no
    readiness primitive for "the target thread entered a syscall", and a sleep would be the wrong
    shape anyway: what this loop needs is for the other thread to be scheduled, which is exactly
    what a yield asks for. `budget` is a wall-clock ceiling so a thread that never leaves userspace
    ends the sampler instead of spinning forever.
    """
    frames: collections.Counter = collections.Counter()
    ordered_example = []
    got = 0
    deadline = time.time() + budget
    with open(f"/proc/{pid}/mem", "rb", 0) as mem:
        while got < nsamples and time.time() < deadline:
            try:
                with open(f"/proc/{pid}/task/{tid}/syscall", encoding="utf-8") as fh:
                    tok = fh.read().split()
            except FileNotFoundError:
                break
            except OSError:
                os.sched_yield()
                continue
            # "running" (single token) means the task is on-CPU in userspace and the
            # kernel will not hand out its registers. Only a syscall stop exposes
            # sp/pc, so poll until we catch one.
            if len(tok) < 3:
                os.sched_yield()
                continue
            try:
                sp = int(tok[-2], 16)
            except ValueError:
                os.sched_yield()
                continue
            if sp == 0:
                os.sched_yield()
                continue
            try:
                mem.seek(sp)
                buf = mem.read(window)
            except OSError:
                os.sched_yield()
                continue
            if not buf:
                os.sched_yield()
                continue
            got += 1
            seq = []
            for off in range(0, len(buf) - 8, 8):
                (val,) = struct.unpack_from("<Q", buf, off)
                mod, delta = resolve(val)
                if mod is not None:
                    seq.append((off, f"{mod}+0x{delta:x}", val))
            for _off, label, _val in seq:
                frames[label] += 1
            if not ordered_example:
                ordered_example = seq
    return got, frames, ordered_example


def selftest() -> int:
    """Sample this interpreter itself; proves the /proc plumbing works end to end."""
    pid = str(os.getpid())
    mods = read_modules(pid)
    if not mods:
        print("selftest FAIL: no modules parsed from own maps")
        return 1
    resolve = make_resolver(mods, (), all_modules=True)
    # Our own executable must resolve.
    exe = os.path.realpath(f"/proc/{pid}/exe")
    hit = any(os.path.basename(exe) == os.path.basename(n) for n in mods)
    if not hit:
        print(f"selftest FAIL: own exe {exe} not in parsed maps")
        return 1
    try:
        with open(f"/proc/{pid}/mem", "rb", 0) as mem:
            mem.seek(next(iter(mods.values()))[0])
            mem.read(16)
    except OSError as exc:
        print(f"selftest FAIL: cannot read own /proc/mem: {exc}")
        return 1
    lo = next(iter(mods.values()))[0]
    mod, delta = resolve(lo)
    if mod is None or delta != 0:
        print("selftest FAIL: resolver did not map a known base to +0x0")
        return 1
    print(f"selftest OK: parsed {len(mods)} modules, /proc/mem readable, resolver sane")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("pid", nargs="?", help="target process id")
    ap.add_argument("--tid", default=None, help="thread id (default: the main thread, tid == pid)")
    ap.add_argument("--samples", type=int, default=14)
    ap.add_argument("--window", type=lambda s: int(s, 0), default=0x6000, help="stack bytes to read above sp")
    ap.add_argument("--budget", type=float, default=20.0, help="seconds to spend catching syscall stops")
    ap.add_argument("--all-modules", action="store_true", help="report every mapped module, not just the interesting ones")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.pid:
        ap.error("pid is required (or pass --selftest)")

    pid = args.pid
    tid = args.tid or pid
    if not os.path.isdir(f"/proc/{pid}/task/{tid}"):
        print(f"no such thread: /proc/{pid}/task/{tid}", file=sys.stderr)
        return 2

    mods = read_modules(pid)
    resolve = make_resolver(mods, DEFAULT_INTEREST, args.all_modules)
    got, frames, example = sample(pid, tid, args.samples, args.window, resolve, args.budget)

    if got == 0:
        print("caught no syscall stop -- the thread never left userspace during the budget.")
        return 1

    print(f"pid={pid} tid={tid} samples={got} window=0x{args.window:x}")
    print("=== PE addresses present on the stack, by sample frequency ===")
    for label, count in frames.most_common(40):
        print(f"  {count:4d}/{got}  {label}")
    print()
    print("=== one raw window, in stack order (sp -> higher) ===")
    for off, label, val in example[:40]:
        print(f"  sp+0x{off:<5x} 0x{val:012x}  {label}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
