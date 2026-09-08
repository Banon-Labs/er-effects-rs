#!/usr/bin/env python3
"""Name the code a WEDGED ELDEN RING is parked in, without attaching a debugger.

Why this exists
---------------
On 2026-08-29 the game stopped rendering ~12 s into boot with every thread asleep and zero CPU,
and four consecutive hypotheses about the cause were tested by rebuilding, relaunching, and
watching the same number come back. Each cost minutes and falsified exactly one guess. The
process was sitting right there the whole time with the answer in its stacks.

This reads them. No `frida.attach` -- that kills this target (bd
`frida-attach-kills-wine-eldenring-use-proc-mem-2026-08-12`) -- and no ptrace stop: it opens
`/proc/<pid>/mem` and scans each thread's stack for values that land inside the game image, the
same poor-man's unwind `er-crash-logging` uses for its `callers=` line. A scanned stack contains
dead frames as well as live ones, so a hit is a candidate, not a call stack; what makes it useful
is that the candidates cluster, and an address on many threads at once is where they are waiting.

What it waits for
-----------------
`--until-wedged` polls total process CPU and only dumps once it flatlines, because a dump taken
while the game is still working describes a game that is working. The flatline threshold is
deliberately strict: a boot that is merely slow still burns hundreds of ticks a second.

Usage
    python3 scripts/er-wedge-stacks.py --pid 12345
    python3 scripts/er-wedge-stacks.py --until-wedged --max-seconds 25
    python3 scripts/er-wedge-stacks.py --selftest
"""

from __future__ import annotations

import argparse
import glob
import os
import struct
import sys
import time

# The game image, as mapped by Wine. Every ELDEN RING code address falls in here.
IMAGE_LO = 0x140000000
IMAGE_HI = 0x148000000
# Bytes of stack read above each thread's stack pointer. A frame chain deep enough to be
# interesting fits comfortably; reading more mostly adds dead frames from earlier calls.
STACK_WINDOW = 0x10000
# Total process CPU ticks over one sample period at or below which the game counts as wedged.
# A game that is merely loading slowly still burns hundreds; this is "doing nothing at all".
WEDGED_TICKS = 3
# Sample period for the CPU flatline test.
SAMPLE_SECONDS = 3.0


def game_pid() -> int | None:
    """The running `eldenring.exe`, by comm.

    Deliberately name-only. The prefix and appid rules in `scripts/er-teardown.py` both go false
    for a game launched through the Steam Linux Runtime -- its exe and cwd are inside the bwrap
    container -- and that false negative has already cost two diagnoses.
    """
    for entry in glob.glob("/proc/[0-9]*"):
        try:
            with open(f"{entry}/comm", encoding="utf-8") as handle:
                if handle.read().strip() == "eldenring.exe":
                    return int(entry.rsplit("/", 1)[-1])
        except OSError:
            continue
    return None


def cpu_ticks(pid: int) -> int | None:
    """utime + stime for the whole process."""
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as handle:
            fields = handle.read().split()
    except OSError:
        return None
    try:
        return int(fields[13]) + int(fields[14])
    except (IndexError, ValueError):
        return None


def thread_stack_pointer(pid: int, tid: str) -> int | None:
    """The thread's SP, from `/proc/<pid>/task/<tid>/syscall`.

    That file is `nr arg0..arg5 sp pc` for a thread blocked in a syscall, and the literal string
    `running` for one that is not -- which is itself the answer for a wedge, since a wedged
    process has none.
    """
    try:
        with open(f"/proc/{pid}/task/{tid}/syscall", encoding="utf-8") as handle:
            fields = handle.read().split()
    except OSError:
        return None
    if len(fields) < 2:
        return None
    try:
        return int(fields[-2], 16)
    except ValueError:
        return None


def live_threads(pid: int) -> list[str]:
    """Task paths whose thread is not a zombie."""
    out = []
    for task in sorted(glob.glob(f"/proc/{pid}/task/*")):
        try:
            with open(f"{task}/stat", encoding="utf-8") as handle:
                if handle.read().split()[2] != "Z":
                    out.append(task)
        except (OSError, IndexError):
            continue
    return out


def open_process_memory(pid: int):
    """A readable `mem` handle for the process, opened through a live thread.

    `/proc/<pid>/mem` is `/proc/<pid>/task/<leader>/mem`, and when the thread-group leader is a
    zombie that open fails with ESRCH -- while the process is very much alive and its other
    threads are readable. That is not a corner case here: it is the exact state this tool exists
    to inspect. Measured 2026-08-29: the game's main thread exits ~12 s into boot with quickload
    loaded, leaving 81 live threads behind a `Z` leader, and four scan attempts in a row reported
    "No such process" against a process burning 195 CPU ticks per three seconds.
    """
    for task in live_threads(pid):
        try:
            return open(f"{task}/mem", "rb", 0)  # noqa: SIM115 -- caller closes
        except OSError:
            continue
    return None


def scan_stacks(pid: int) -> dict[int, list[str]]:
    """`{game-image address: [tid, ...]}` over every readable thread stack."""
    hits: dict[int, list[str]] = {}
    mem = open_process_memory(pid)
    if mem is None:
        print(f"no readable thread memory for pid {pid}", file=sys.stderr)
        return hits
    with mem:
        for task in live_threads(pid):
            tid = os.path.basename(task)
            sp = thread_stack_pointer(pid, tid)
            if not sp:
                continue
            try:
                mem.seek(sp)
                buf = mem.read(STACK_WINDOW)
            except OSError:
                continue
            for offset in range(0, len(buf) - 8, 8):
                value = struct.unpack_from("<Q", buf, offset)[0]
                if IMAGE_LO <= value < IMAGE_HI:
                    hits.setdefault(value, []).append(tid)
    return hits


def wait_for_wedge(pid: int, max_seconds: float) -> tuple[bool, dict[int, list[str]]]:
    """Poll until the process stops burning CPU. Returns (wedged, the most recent stack scan).

    Snapshot every sample, and keep the last one that worked. Detect-then-scan is always too late
    here: measured 2026-08-29, the game flatlines and then exits within seconds, and three
    consecutive attempts to scan after the flatline found `/proc/<pid>/mem` already gone. A scan
    from the sample before it died is worth infinitely more than a perfect scan of nothing.
    """
    deadline = time.monotonic() + max_seconds
    last: dict[int, list[str]] = {}
    while time.monotonic() < deadline:
        before = cpu_ticks(pid)
        time.sleep(SAMPLE_SECONDS)  # the sample period of a measurement, not a synchronisation
        after = cpu_ticks(pid)
        if before is None or after is None:
            print("process exited -- reporting the last snapshot taken before it did")
            return True, last
        burned = after - before
        snapshot = scan_stacks(pid)
        if snapshot:
            last = snapshot
        print(f"  cpu_ticks_in_{SAMPLE_SECONDS:g}s={burned} stack_hits={len(snapshot)}")
        if burned <= WEDGED_TICKS:
            return True, last
    return False, last


def leader_state(pid: int) -> str:
    """The thread-group leader's process state letter, or `?`."""
    try:
        with open(f"/proc/{pid}/stat", encoding="utf-8") as handle:
            return handle.read().split()[2]
    except (OSError, IndexError):
        return "?"


def report(pid: int, hits: dict[int, list[str]], top: int) -> None:
    state = leader_state(pid)
    note = "  <-- MAIN THREAD HAS EXITED; the workers below are idle, not stuck" if state == "Z" else ""
    print(f"pid {pid}: leader state {state}{note}")
    print(f"pid {pid}: {len(hits)} distinct game-image address(es) across the thread stacks")
    ranked = sorted(hits.items(), key=lambda item: (-len(item[1]), item[0]))
    for address, tids in ranked[:top]:
        rva = address - IMAGE_LO
        print(f"  0x{address:x}  rva 0x{rva:x}  on {len(tids):3d} thread(s)")
    if len(ranked) > top:
        print(f"  ... {len(ranked) - top} more, not shown")


def selftest() -> int:
    """Exercise the parsing on this process, where the answers are knowable."""
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    me = os.getpid()
    check("cpu_ticks reads a live pid", cpu_ticks(me) is not None, True)
    check("cpu_ticks refuses a dead pid", cpu_ticks(2**30), None)
    # This process has at least one thread, and its SP parses or it is `running` (which parses to
    # None). Either is a pass; a crash is not.
    for task in glob.glob(f"/proc/{me}/task/*")[:4]:
        thread_stack_pointer(me, os.path.basename(task))
    check("game_pid returns an int or None", isinstance(game_pid(), (int, type(None))), True)
    # The image window must be the game's and not, say, a heap range.
    check("image window is 128MB", IMAGE_HI - IMAGE_LO, 0x8000000)
    check("wedged threshold is strict", WEDGED_TICKS <= 5, True)
    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, help="target pid (default: the running eldenring.exe)")
    parser.add_argument(
        "--until-wedged",
        action="store_true",
        help="poll CPU first and only dump once the process flatlines",
    )
    parser.add_argument("--max-seconds", type=float, default=25.0)
    parser.add_argument("--top", type=int, default=25)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    pid = args.pid or game_pid()
    if pid is None:
        print("no eldenring.exe running")
        return 1
    if args.until_wedged:
        wedged, snapshot = wait_for_wedge(pid, args.max_seconds)
        if not wedged and not snapshot:
            print("still burning CPU -- not wedged, nothing to dump")
            return 1
        report(pid, snapshot, args.top)
        return 0
    report(pid, scan_stacks(pid), args.top)
    return 0


if __name__ == "__main__":
    sys.exit(main())
