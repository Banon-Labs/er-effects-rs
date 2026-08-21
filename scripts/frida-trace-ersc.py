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
              empty, which is what Themida's missing unwind data would cause. Expect the game
              to slow down noticeably while it runs.

HOW TO USE IT (the point is a human-driven window)
--------------------------------------------------
  1. Start the game with the Seamless profile and get in-world.
  2. Start this tracer.
  3. Use the invasion item, and keep going until the game says it found someone to invade.
  4. Ctrl-C. Everything is written to the output JSONL.

HOW WE REACH THE PROCESS
------------------------
The game runs under Wine/Proton, so a Linux-side `frida.attach()` cannot see it -- there is no
Linux process to attach to. The working path in this repo is the GADGET: `frida-gadget.dll` is
loaded into the game as an me3 `[[natives]]` entry and listens on 127.0.0.1:27042, and we
connect to that as a REMOTE DEVICE. Same mechanism as scripts/frida/badge-scale.py.

So the game must be launched with a profile that includes the gadget. There is one at
/home/banon/Elden/pr190-invasion-warp-seamless-frida.me3 (written by this repo); it is the
normal invasion-warp Seamless profile plus the gadget DLL.

RUN IT (frida is provisioned ephemerally by uv; nothing is installed system-wide):
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-trace-ersc.py --hooks
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-trace-ersc.py --hooks --rva 0x8f4b0
    uv run --with frida python3 /home/banon/projects/er-effects-rs/scripts/frida-trace-ersc.py --stalker

SELFTEST (no game, no frida):
    python3 /home/banon/projects/er-effects-rs/scripts/frida-trace-ersc.py --selftest

SAFETY
------
  * READ-ONLY: the agent never writes target memory and never calls into the target.
  * Detaches on observable events -- the gadget script being destroyed (game gone) or stdin
    reaching EOF (operator done) -- so nothing is left running on the user's game.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import threading
from pathlib import Path

#: The gadget's listen address, from target/frida-gadget/frida-gadget.config.
DEFAULT_GADGET = "127.0.0.1:27042"
AGENT_JS_PATH = Path(__file__).resolve().parent / "frida-trace-ersc.agent.js"
DEFAULT_MODULE = "ersc.dll"

# The single ersc address established so far: the callback hooked over the vanilla spawn-position
# selector's call site, which writes GameMan+0xaa0 (lastLoadPosition) and +0xab0
# (lastLoadOrientation). If Seamless decides where an invader materializes, it decides it here,
# so this is the highest-value hook to start from. Its backtrace should name the caller chain.
DEFAULT_HOOKS = [
    {"rva": 0x8F4B0, "label": "ersc-spawn-position-callback", "argCount": 4},
]

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
    parser.add_argument("--out", help="output JSONL (default target/runtime-probe/ersc-trace.jsonl)")
    parser.add_argument("--gadget", default=DEFAULT_GADGET, help=f"frida-gadget address (default {DEFAULT_GADGET})")
    parser.add_argument("--selftest", action="store_true", help="prove the path reconstruction")
    args = parser.parse_args()

    if args.selftest:
        return _selftest()
    if not args.hooks and not args.stalker:
        parser.error("one of --hooks or --stalker is required")

    # Detached, python block-buffers stdout, so a run that is working looks identical to one that
    # hung: nothing appears until it exits. Line buffering makes progress visible while it traces.
    try:
        sys.stdout.reconfigure(line_buffering=True)
    except (AttributeError, ValueError):
        pass

    try:
        agent_js = AGENT_JS_PATH.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"ERROR: cannot read agent {AGENT_JS_PATH}: {exc}", file=sys.stderr)
        return 6

    try:
        import frida
    except ImportError:
        print(
            "ERROR: frida is not importable. It is not installed system-wide here on purpose; "
            "uv provisions it per-run:\n"
            "  uv run --with frida python3 "
            "/home/banon/projects/er-effects-rs/scripts/frida-trace-ersc.py --hooks",
            file=sys.stderr,
        )
        return 7

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

    # The game runs under Wine/Proton, so there is no Linux process to attach to. The gadget
    # inside the game listens on a socket and we connect to THAT.
    try:
        device = frida.get_device_manager().add_remote_device(args.gadget)
        session = device.attach("Gadget")
    except Exception as exc:
        print(
            f"ERROR: could not reach frida-gadget at {args.gadget}: {exc}\n"
            "Is the game running with a profile that includes frida-gadget.dll? Try:\n"
            "  /home/banon/Elden/pr190-invasion-warp-seamless-frida.me3",
            file=sys.stderr,
        )
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
        print("  Then press Ctrl-D (or Ctrl-C) here to stop and write the trace.")
        print("=" * 72)
        print()

        # Stay resident on OBSERVABLE events only -- the gadget script being destroyed (the game
        # is gone), or the operator finishing. No timer and no poll: a trace window is exactly as
        # long as it takes a human to use the item and get a match, which is not a number this
        # script can know.
        detached = threading.Event()
        script.on("destroyed", detached.set)
        if sys.stdin.isatty():
            # Interactive: Ctrl-D ends it.
            try:
                sys.stdin.read()
            except KeyboardInterrupt:
                pass
        else:
            # Detached (nohup/&). Reading stdin here is not just useless but harmful -- pointed
            # at /dev/zero it would consume an endless stream of NULs into memory forever, and
            # pointed at /dev/null it would exit instantly and record nothing. Wait for the game
            # instead, and let a signal end it early.
            print("detached: no tty, tracing until the game exits (or this process is killed)")
            detached.wait()

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
