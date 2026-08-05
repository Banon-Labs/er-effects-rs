#!/usr/bin/env python3
"""Trace what Seamless Co-op executes during an invasion, live.

WHY
---
Seamless Co-op has its OWN invasion system -- it does not use retail Elden Ring matchmaking --
and `ersc.dll` is Themida-packed, so the file cannot be read statically. The packing only
protects the FILE: by the time the player uses the invasion item, the code is unpacked and
running. Tracing it reads straight through the protection.

The deliverable is a BACKTRACE at each hit. That is what names the ersc.dll functions on the
invasion path, which is the thing static analysis could not produce.

MODES
-----
  --hooks     Intercept ersc RVAs and record args + backtrace on each hit. Cheap; start here.
              Defaults to the one ersc address already established: +0x8f4b0, the callback that
              writes GameMan's lastLoadPosition/lastLoadOrientation, i.e. where a joiner lands.
  --stalker   Follow threads and record every basic block executed inside ersc.dll. Heavier and
              a packer may object, so it is opt-in -- use it when hook backtraces come back
              empty, which is what Themida's missing unwind data would cause.

HOW TO USE IT (the point is a human-driven window)
--------------------------------------------------
  1. Start the game with the Seamless profile and get in-world.
  2. Start this tracer.
  3. Use the invasion item, and keep going until the game says it found someone to invade.
  4. Ctrl-C. Everything is written to the output JSONL.

RUN IT (Frida lives on the WINDOWS python, not the WSL one -- same as frida-nudge.py):
    python.exe "$(wslpath -w scripts/frida-trace-ersc.py)" --hooks
    python.exe "$(wslpath -w scripts/frida-trace-ersc.py)" --hooks --rva 0x8f4b0 --rva 0x...
    python.exe "$(wslpath -w scripts/frida-trace-ersc.py)" --stalker --seconds 60

SELFTEST (no game, no frida):
    python3 scripts/frida-trace-ersc.py --selftest

SAFETY
------
  * Offline `eldenring.exe` ONLY; refuses start_protected_game.exe / EAC.
  * READ-ONLY: the agent never writes target memory and never calls into the target.
  * Bounded: --seconds caps a stalker run so a heavy trace cannot be left running on the
    user's game.
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import threading
from pathlib import Path

TARGET = "eldenring.exe"
FORBIDDEN = ("start_protected_game.exe", "eac", "easyanticheat")
AGENT_JS_PATH = Path(__file__).resolve().parent / "frida-trace-ersc.agent.js"
DEFAULT_MODULE = "ersc.dll"

# The single ersc address established so far: the callback hooked over the vanilla spawn-position
# selector's call site, which writes GameMan+0xaa0 (lastLoadPosition) and +0xab0
# (lastLoadOrientation). If Seamless decides where an invader materializes, it decides it here,
# so this is the highest-value hook to start from. Its backtrace should name the caller chain.
DEFAULT_HOOKS = [
    {"rva": 0x8F4B0, "label": "ersc-spawn-position-callback", "argCount": 4},
]

# A stalker run with no bound would sit on the user's game indefinitely.
DEFAULT_SECONDS = 120


def summarize(records: list[dict]) -> list[str]:
    """Turn raw hit records into the ordered, deduped call path.

    Separated from the frida plumbing because this is the part that produces the ANSWER, and it
    should be checkable without a game. Order is first-seen, because the sequence in which ersc
    functions are reached is the thing being reconstructed -- sorting would destroy it.
    """
    seen: dict[str, None] = {}
    for record in records:
        if record.get("type") == "hit":
            for key in [record.get("at", "")] + [
                frame.get("at", "") for frame in record.get("backtrace", [])
            ]:
                if key and key not in seen:
                    seen[key] = None
        elif record.get("type") == "blocks":
            for block in record.get("blocks", []):
                if block not in seen:
                    seen[block] = None
    return list(seen.keys())


def _selftest() -> int:
    fails = 0

    def check(ok: bool, label: str) -> None:
        nonlocal fails
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            fails += 1

    check(summarize([]) == [], "no records yields no path, not an error")
    check(
        summarize(
            [
                {
                    "type": "hit",
                    "at": "ersc.dll+0x8f4b0",
                    "backtrace": [{"at": "ersc.dll+0x1234"}, {"at": "eldenring.exe+0xaf9d20"}],
                }
            ]
        )
        == ["ersc.dll+0x8f4b0", "ersc.dll+0x1234", "eldenring.exe+0xaf9d20"],
        "a hit contributes itself then its callers, in order",
    )
    # THE POINT OF THE TOOL: the SEQUENCE is the finding. Sorting or set-ordering it would
    # destroy exactly the information the trace exists to recover.
    check(
        summarize(
            [
                {"type": "blocks", "blocks": ["ersc.dll+0x900", "ersc.dll+0x100"]},
                {"type": "blocks", "blocks": ["ersc.dll+0x100", "ersc.dll+0x50"]},
            ]
        )
        == ["ersc.dll+0x900", "ersc.dll+0x100", "ersc.dll+0x50"],
        "first-seen order is preserved and repeats are dropped",
    )
    check(
        summarize([{"type": "hit", "at": "", "backtrace": []}]) == [],
        "an empty hit contributes nothing rather than a blank entry",
    )
    if fails:
        print(f"selftest FAILED ({fails})")
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hooks", action="store_true", help="intercept ersc RVAs (cheap; start here)")
    parser.add_argument("--stalker", action="store_true", help="record every block executed in ersc.dll")
    parser.add_argument("--rva", action="append", default=[], help="extra ersc RVA to hook, e.g. 0x8f4b0")
    parser.add_argument("--module", default=DEFAULT_MODULE, help=f"module to trace (default {DEFAULT_MODULE})")
    parser.add_argument("--seconds", type=int, default=DEFAULT_SECONDS, help="cap on the trace window")
    parser.add_argument("--out", help="output JSONL (default target/runtime-probe/ersc-trace.jsonl)")
    parser.add_argument("--pid", type=int, default=None, help="attach by PID instead of image name")
    parser.add_argument("--selftest", action="store_true", help="prove the path reconstruction")
    args = parser.parse_args()

    if args.selftest:
        return _selftest()
    if not args.hooks and not args.stalker:
        parser.error("one of --hooks or --stalker is required")

    try:
        agent_js = AGENT_JS_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"ERROR: cannot read agent {AGENT_JS_PATH}: {exc}", file=sys.stderr)
        return 6

    try:
        import frida  # type: ignore[import-not-found]  # windows-only python
    except ImportError:
        print(
            "ERROR: frida is not importable. Run under the WINDOWS python, as with "
            'frida-nudge.py:\n  python.exe "$(wslpath -w scripts/frida-trace-ersc.py)" ...',
            file=sys.stderr,
        )
        return 7

    target: object = args.pid if args.pid is not None else TARGET
    if isinstance(target, str) and any(bad in target.lower() for bad in FORBIDDEN):
        print(f"REFUSING to attach to {target!r}: protected/EAC launcher", file=sys.stderr)
        return 2

    out_path = args.out or os.path.join("target", "runtime-probe", "ersc-trace.jsonl")
    os.makedirs(os.path.dirname(os.path.abspath(out_path)), exist_ok=True)

    records: list[dict] = []
    handle = open(out_path, "w", encoding="utf-8")

    def on_message(message, _data):
        if message.get("type") != "send":
            print(f"[agent] {message}", file=sys.stderr)
            return
        payload = message.get("payload") or {}
        records.append(payload)
        handle.write(json.dumps(payload) + "\n")
        handle.flush()
        if payload.get("type") == "hit":
            print(f"HIT  {payload.get('label')}  {payload.get('at')}  args={payload.get('args')}")
            for frame in payload.get("backtrace", [])[:12]:
                print(f"       <- [{frame.get('kind')}] {frame.get('at')}")

    try:
        session = frida.attach(target)
    except Exception as exc:
        print(f"ERROR: attach to {target!r} failed: {exc}", file=sys.stderr)
        handle.close()
        return 3

    try:
        script = session.create_script(agent_js)
        script.on("message", on_message)
        script.load()
        api = script.exports_sync

        info = api.init(args.module)
        if info is None:
            print(
                f"ERROR: {args.module} is not loaded. Is the Seamless profile running?",
                file=sys.stderr,
            )
            return 4
        print(f"{info['name']}  base={info['base']}  size=0x{int(info['size']):x}")

        if args.hooks:
            specs = list(DEFAULT_HOOKS)
            for raw in args.rva:
                rva = int(raw, 16) if raw.lower().startswith("0x") else int(raw, 16)
                specs.append({"rva": rva, "label": f"user-rva-{raw}", "argCount": 4})
            for entry in api.install_hooks(specs):
                print(f"  hook {entry}")

        if args.stalker:
            print("STALKER ON -- this is heavy; the game will slow down.")
            print(api.start_stalker([]))

        print()
        print("=" * 72)
        print("  TRACING. Now use the Seamless invasion item, and keep going until the game")
        print("  says it found someone to invade.")
        print(f"  Ctrl-C when done (auto-stops after {args.seconds}s).")
        print("=" * 72)
        print()

        # Block on an Event rather than polling with sleep. The window ends on exactly two
        # things -- the operator finishing, or the cap expiring -- and an Event expresses both
        # directly: it wakes the instant it is set instead of at the next poll tick, and it
        # burns no CPU while a trace is already slowing the game down.
        finished = threading.Event()

        def _stop(_signum, _frame):
            finished.set()

        previous = signal.signal(signal.SIGINT, _stop)
        try:
            if not finished.wait(timeout=args.seconds):
                print(f"\nreached the {args.seconds}s cap")
            else:
                print("\nstopping on operator signal")
        finally:
            signal.signal(signal.SIGINT, previous)

        if args.stalker:
            try:
                print(api.stop_stalker())
            except Exception as exc:
                print(f"stalker stop failed: {exc}", file=sys.stderr)

        path = summarize(records)
        print()
        print(f"records: {len(records)}   distinct addresses: {len(path)}")
        print(f"written: {out_path}")
        if path:
            print("\nfirst-seen execution path:")
            for entry in path[:60]:
                print(f"  {entry}")
        else:
            print(
                "\nNOTHING WAS RECORDED. That is a real result, not a dud run: either the hooked "
                "address is not on the invasion path, or the invasion never reached it. If "
                "--hooks produced hits but every backtrace was empty, Themida stripped the unwind "
                "data -- rerun with --stalker."
            )
        return 0
    finally:
        handle.close()
        try:
            session.detach()
        except Exception:
            pass


if __name__ == "__main__":
    raise SystemExit(main())
