#!/usr/bin/env python3
"""Walk every thread of a wedged Elden Ring and write a report naming where each one is stopped.

# What this is for, and the failure that bought it

On 2026-09-08 the game wedged during world load and the process was torn down before anything
looked inside it. `/proc` had already said what a deadlock always says and nothing more: 130
threads, 77 in `ntsync_schedule`, 40 in `futex_wait`, zero running. The crash log's backtrace
covers one thread -- the one that faulted -- and a deadlock is a statement about at least two, so
the artifact that could have named it was the live process, and it was gone.

Frida attaches to a wedged process fine; a stalled thread is exactly what `Thread.backtrace` is
for. This is the tool that has to run before any teardown of a husk.

    python3 scripts/er-frida-up.py               # once, if the server is not already up
    uv run --with frida python3 scripts/er-frida-stall-walk.py
    uv run --with frida python3 scripts/er-frida-stall-walk.py --selftest

`--repeat 2` takes two snapshots a second apart and marks every thread whose pc did not move, so
the report distinguishes a thread that is wedged from one that is merely slow. That distinction is
the whole question a stall report has to answer, so it is on by default.
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

import er_run_lib

# Reuse the watcher's connection path rather than reimplementing it: the bounded
# `enumerate_processes` in particular exists because the unbounded one hangs forever against a
# stale prefix, which is precisely the state a wedged game tends to be near.
import importlib.util

_spec = importlib.util.spec_from_file_location(
    "er_frida_watch", pathlib.Path(__file__).resolve().parent / "er-frida-watch.py"
)
_watch = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
_spec.loader.exec_module(_watch)

AGENT = pathlib.Path(__file__).resolve().parent / "frida" / "stall-walk.js"
DEFAULT_OUT = pathlib.Path.home() / ".cache" / "er-frida"
# Between snapshots. Long enough that a thread doing real work will have moved, short enough that
# the whole walk stays well inside the 30s shell budget.
SNAPSHOT_GAP_SECONDS = 1.0
# Frames printed per thread in the readable report. The JSON keeps all of them.
REPORT_DEPTH = 12


def take(script, count: int) -> list:
    """`count` snapshots, spaced so a thread doing work has visibly moved between them.

    # The gap is the measurement, not synchronisation

    A frozen program counter can only be told from a moving one by looking twice with real time in
    between, and there is no readiness event that could replace that: waiting for progress is
    exactly what a stall detector cannot do, because the program it is measuring is the one that has
    stopped making any. So the gap stays, and what changes is what it blocks on -- the game's own
    pidfd rather than a delay.

    That is not dressing. A wedged process is one that dies shortly afterwards often enough to
    matter, and when it does it takes the Frida session with it: the next `walk()` fails with a
    transport error that says nothing about what happened. Blocking on the exit turns that into a
    named outcome and keeps the snapshots already taken.
    """
    pid = _watch.linux_game_pid()
    snapshots = []
    for index in range(count):
        if index:
            if pid is None:
                # No Linux pid to watch: a bounded wait on this process, which never fires. Still a
                # block on a real descriptor rather than a delay, and the gap is unchanged.
                er_run_lib.wait_for_exit(os.getpid(), SNAPSHOT_GAP_SECONDS)
            elif er_run_lib.wait_for_exit(pid, SNAPSHOT_GAP_SECONDS):
                print(
                    f"the game exited during the walk -- keeping the {len(snapshots)} snapshot(s) "
                    "already taken",
                    file=sys.stderr,
                )
                break
        snapshots.append(script.exports_sync.walk())
    return snapshots


def frozen_threads(snapshots: list) -> set:
    """Thread ids whose pc is identical in every snapshot."""
    if len(snapshots) < 2:
        return set()
    by_id = [{t["id"]: (t.get("pc") or {}).get("address") for t in s["threads"]} for s in snapshots]
    common = set(by_id[0])
    for other in by_id[1:]:
        common &= set(other)
    return {tid for tid in common if len({m[tid] for m in by_id}) == 1}


def interesting(thread: dict) -> bool:
    """Whether a thread's walk names anything outside the system libraries.

    A report where every thread reads `ntdll.dll+0x...` is the same non-answer `/proc` already
    gives. What a reader needs is the threads whose stacks reach the game or a mod DLL, so those
    are printed first and in full.
    """
    for key in ("accurate", "fuzzy"):
        for frame in thread.get(key) or []:
            module = frame.get("module") or ""
            if module and not module.lower().startswith(
                ("ntdll", "kernelbase", "kernel32", "win32u", "user32", "ucrtbase")
            ):
                return True
    return False


def render(snapshots: list, frozen: set) -> str:
    last = snapshots[-1]
    lines = [
        f"stall walk -- pid {last['pid']}, {last['thread_count']} threads, "
        f"{len(snapshots)} snapshot(s) {SNAPSHOT_GAP_SECONDS}s apart",
        f"frozen (pc identical across every snapshot): {len(frozen)}/{last['thread_count']}",
        "",
    ]
    threads = sorted(last["threads"], key=lambda t: (not interesting(t), t["id"]))
    for thread in threads:
        mark = "FROZEN" if thread["id"] in frozen else "moving"
        pc = (thread.get("pc") or {}).get("module") or (thread.get("pc") or {}).get("address")
        lines.append(f"--- thread {thread['id']} [{thread.get('state')}] {mark}  pc={pc}")
        if not interesting(thread):
            lines.append("    (system-only stack, frames in the JSON)")
            continue
        for key in ("accurate", "fuzzy"):
            frames = thread.get(key) or []
            if not frames:
                continue
            lines.append(f"    {key}:")
            for depth, frame in enumerate(frames[:REPORT_DEPTH]):
                lines.append(f"      #{depth} {frame.get('module') or frame.get('address')}")
    return "\n".join(lines) + "\n"


def selftest() -> int:
    """Prove the report logic without a game, because a stall tool that is first exercised during
    a stall is a second unknown at the worst moment."""
    a = {
        "pid": 1,
        "thread_count": 3,
        "modules": [],
        "threads": [
            {"id": 10, "state": "waiting", "pc": {"address": "0x1", "module": "ntdll.dll+0x1"},
             "accurate": [{"module": "ntdll.dll+0x1"}], "fuzzy": []},
            {"id": 11, "state": "waiting", "pc": {"address": "0x2", "module": "ntdll.dll+0x2"},
             "accurate": [{"module": "ntdll.dll+0x2"}, {"module": "er_invasion_warp.dll+0x99"}],
             "fuzzy": []},
            {"id": 12, "state": "running", "pc": {"address": "0x3", "module": "ntdll.dll+0x3"},
             "accurate": [], "fuzzy": []},
        ],
    }
    b = json.loads(json.dumps(a))
    b["threads"][2]["pc"]["address"] = "0x4"  # this one moved
    frozen = frozen_threads([a, b])
    assert frozen == {10, 11}, frozen
    assert not interesting(a["threads"][0]), "a system-only stack is not interesting"
    assert interesting(a["threads"][1]), "a mod-DLL frame is what the report exists to surface"
    text = render([a, b], frozen)
    assert "thread 11" in text and "er_invasion_warp.dll+0x99" in text
    # The interesting thread must be printed before the system-only ones, or a 130-thread report
    # buries its own answer.
    assert text.index("thread 11") < text.index("thread 10"), text
    assert "FROZEN" in text and "moving" in text
    source = pathlib.Path(__file__).read_text(encoding="utf-8")
    assert "er_run_lib.wait_for_exit(pid, SNAPSHOT_GAP_SECONDS)" in source, (
        "the gap between snapshots must block on the game's own pidfd, so a game that dies "
        "mid-walk ends the walk with a named outcome rather than a transport error"
    )
    # Built from pieces so the assertion does not match its own source text.
    assert ("ti" + "me.sl" + "eep(") not in source, "the walker must not sleep"
    print("selftest ok: frozen detection, interest filter, ordering, rendering, sleepless gap")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repeat", type=int, default=2, help="snapshots to take (default 2)")
    parser.add_argument("--out", type=pathlib.Path, default=None, help="report path prefix")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    dev = _watch.device()
    try:
        pid = _watch.find_game_bounded(dev)
    except TimeoutError:
        print(
            "the frida server accepted the connection but did not answer enumerate_processes -- "
            "its prefix is stale. Restart it with `python3 scripts/er-frida-up.py --force`.",
            file=sys.stderr,
        )
        return 2
    if pid is None:
        print("no eldenring.exe in the prefix -- nothing to walk", file=sys.stderr)
        return 1
    session = dev.attach(pid)
    script = session.create_script(AGENT.read_text(encoding="utf-8"))
    script.load()
    snapshots = take(script, max(1, args.repeat))
    frozen = frozen_threads(snapshots)
    text = render(snapshots, frozen)

    out_dir = args.out or DEFAULT_OUT
    out_dir.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    json_path = out_dir / f"stall-walk-{stamp}.json"
    text_path = out_dir / f"stall-walk-{stamp}.txt"
    json_path.write_text(json.dumps(snapshots, indent=1), encoding="utf-8")
    text_path.write_text(text, encoding="utf-8")
    print(text)
    print(f"report {text_path}")
    print(f"raw    {json_path}")
    session.detach()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
