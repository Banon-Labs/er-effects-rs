#!/usr/bin/env python3
"""Read-only live memory inspection: snapshot a window, or trap the code that WRITES it.

WHY THIS EXISTS (user directive 2026-08-12). Answering a read-only question about game memory --
"which field is the caret", "does this offset track the scroll", "what changes when I press End" --
used to mean editing the DLL to add a telemetry dump, rebuilding, tearing the game down and
relaunching it. That throws away a live session and the user's place in the menus to learn something
an attach could have read from the process as it runs. Three such builds were spent on the 02_990
path-editor caret before this tool existed.

So: if the next step only READS memory, use this. Rebuild + relaunch is for changes that alter
BEHAVIOUR, where the standing order forbids validating a new DLL in an already-running process
anyway.

Nothing here writes to the target.

Two modes, both event-driven -- the repo bans sleep-as-synchronization
(`scripts/check-no-timeouts.py`, rule `python-sleep-or-wait-for`), and polling a value on a timer
would be exactly that:

  --snapshot      read the window once and print it. Run it twice around an action to diff by hand;
                  each run is a single deterministic read with no timing in it at all.
  --watch-writes  arm a guard page over the window and report every WRITE: the instruction that did
                  it, the address touched, and the module-relative offset. Strictly better than a
                  diff for "who owns this field", since it names the writer instead of the symptom.

Examples:
    scripts/er-live-fields.py --selftest
    scripts/er-live-fields.py --process eldenring.exe --addr 0xba778a20 --bytes 512 --snapshot
    scripts/er-live-fields.py --process eldenring.exe --addr 0xba778a20 --watch-writes --hits 8
"""

from __future__ import annotations

import argparse
import os
import queue
import struct
import sys

# This script lives in `scripts/`, and `scripts/frida/` is a real directory in this repo. Python puts
# the script's own directory FIRST on sys.path, so a bare `import frida` binds that directory as an
# empty namespace package: the import SUCCEEDS and every attribute access then fails with
# AttributeError, which reads like a frida version problem and is not one. Drop the script directory
# before importing.
_SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
sys.path[:] = [p for p in sys.path if os.path.abspath(p or os.getcwd()) != _SCRIPT_DIR]

MAX_WAIT_SECONDS = 25.0
"""Agent shell calls are hard-capped at 30s, so any blocking wait must return inside that."""


def _reexec_under_uv() -> None:
    """Frida is not installed system-wide on purpose; uv provisions it ephemerally (cached)."""
    if os.environ.get("ER_LIVE_FIELDS_UV") == "1":
        raise SystemExit("frida still unavailable under uv; install it or fix the uv cache")
    os.environ["ER_LIVE_FIELDS_UV"] = "1"
    os.execvp(
        "uv",
        ["uv", "run", "--with", "frida", "python3", os.path.abspath(__file__), *sys.argv[1:]],
    )


try:
    import frida
except ModuleNotFoundError:
    _reexec_under_uv()


AGENT_SOURCE = """
rpc.exports.readWindow = function (addr, size) {
    try {
        return Array.from(new Uint8Array(ptr(addr).readByteArray(size)));
    } catch (e) {
        return null;
    }
};

rpc.exports.watchWrites = function (addr, size) {
    // Guard-page trap: the OS raises once per page on access, frida reports it and re-arms. No
    // timer anywhere -- the target's own writes drive every message this sends.
    MemoryAccessMonitor.enable({ base: ptr(addr), size: size }, {
        onAccess: function (details) {
            var from = details.from;
            var module = from === null ? null : Process.findModuleByAddress(from);
            send({
                kind: 'access',
                operation: details.operation,
                address: details.address.toString(),
                from: from === null ? null : from.toString(),
                module: module === null ? null : module.name,
                moduleOffset: module === null || from === null
                    ? null
                    : from.sub(module.base).toString(),
            });
        }
    });
    return true;
};
"""


def attach(target):
    device = frida.get_local_device()
    session = device.attach(target)
    script = session.create_script(AGENT_SOURCE)
    return session, script


def read_u32s(script, addr: int, size: int) -> list[int] | None:
    raw = script.exports_sync.read_window(hex(addr), size)
    if raw is None:
        return None
    data = bytes(raw)
    return list(struct.unpack_from(f"<{len(data) // 4}I", data, 0))


def snapshot(target, addr: int, size: int, expect_max: int) -> int:
    session, script = attach(target)
    script.load()
    try:
        values = read_u32s(script, addr, size)
        if values is None:
            print(f"FAIL: 0x{addr:x} is not readable in the target", file=sys.stderr)
            return 2
        print(f"snapshot 0x{addr:x} slots={len(values)}")
        for slot, value in enumerate(values):
            if value == 0:
                continue
            # An index-shaped value sits inside 0..expect_max; flagging it separates a candidate
            # caret/cursor from pointer halves and timers without hiding anything else.
            mark = "*" if expect_max and 0 <= value <= expect_max else ""
            print(f"  +0x{slot * 4:<4x} {value}{mark}")
        return 0
    finally:
        session.detach()


def watch_writes(target, addr: int, size: int, hits: int) -> int:
    session, script = attach(target)
    events: queue.Queue = queue.Queue()

    def on_message(message, _data) -> None:
        if message.get("type") == "send":
            events.put(message["payload"])
        else:
            events.put({"kind": "error", "detail": message})

    script.on("message", on_message)
    script.load()
    try:
        script.exports_sync.watch_writes(hex(addr), size)
        print(f"armed guard page over 0x{addr:x}..0x{addr + size:x}; waiting for writes")
        seen = 0
        while seen < hits:
            try:
                # Blocks on a real event, with a bound so the call cannot outlive the shell cap.
                event = events.get(timeout=MAX_WAIT_SECONDS)
            except queue.Empty:
                print("NO WRITE observed in the window before the wait bound")
                return 0
            seen += 1
            if event.get("kind") != "access":
                print(f"[{seen}] {event}")
                continue
            print(
                f"[{seen}] {event['operation']} at {event['address']}"
                f" by {event.get('from')}"
                f" ({event.get('module')}+{event.get('moduleOffset')})"
            )
        return 0
    finally:
        session.detach()


CHILD_SOURCE = """
import ctypes, struct, sys, time
buf = ctypes.create_string_buffer(4096)
print(ctypes.addressof(buf), flush=True)
sys.stdin.readline()
value = 0
while True:
    value += 1
    struct.pack_into('<I', buf, 8, value)
"""


def selftest() -> int:
    """Prove attach + read end to end without touching the game.

    A CHILD process is the target rather than this one: frida attaching to its own process deadlocks
    -- the injected agent needs the main thread that is sitting in the attach call -- which shows up
    as a silent hang, not an error. Never hand someone a script that has not been run.
    """
    import subprocess

    child = subprocess.Popen(
        [sys.executable, "-c", CHILD_SOURCE],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        line = child.stdout.readline().strip() if child.stdout else ""
        if not line.isdigit():
            print(f"SELFTEST FAIL: child did not report an address ({line!r})", file=sys.stderr)
            return 1
        addr = int(line)
        session, script = attach(child.pid)
        script.load()
        try:
            before = read_u32s(script, addr, 64)
            if before is None:
                print("SELFTEST FAIL: could not read the child's memory", file=sys.stderr)
                return 1
            # Release the child into its write loop, then read again. The handshake is the child's
            # own stdin read, so readiness is an event rather than a delay.
            child.stdin.write("go\n")
            child.stdin.flush()
            after = read_u32s(script, addr, 64)
            while after is not None and after[2] == before[2]:
                after = read_u32s(script, addr, 64)
        finally:
            session.detach()
    finally:
        child.kill()
        child.wait(timeout=5)

    if after is None:
        print("SELFTEST FAIL: window became unreadable", file=sys.stderr)
        return 1
    print(f"SELFTEST OK: attach+read works (slot +0x8 {before[2]} -> {after[2]})")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, help="target pid")
    parser.add_argument("--process", help="target process name, e.g. eldenring.exe")
    parser.add_argument("--addr", help="address to inspect, hex or decimal")
    parser.add_argument("--bytes", type=int, default=512, help="window size (default 512)")
    parser.add_argument("--snapshot", action="store_true", help="read the window once and print it")
    parser.add_argument(
        "--watch-writes", action="store_true", help="report the code that writes the window"
    )
    parser.add_argument("--hits", type=int, default=5, help="writes to report (default 5)")
    parser.add_argument(
        "--expect-max",
        type=int,
        default=0,
        help="mark snapshot slots whose value is within 0..N (e.g. a text length)",
    )
    parser.add_argument("--selftest", action="store_true", help="verify attach+read on a child")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if args.addr is None:
        parser.error("--addr is required (or use --selftest)")
    if args.pid is None and not args.process:
        parser.error("one of --pid or --process is required")
    if args.snapshot == args.watch_writes:
        parser.error("pick exactly one of --snapshot or --watch-writes")

    addr = int(args.addr, 0)
    target = args.pid if args.pid is not None else args.process
    if args.snapshot:
        return snapshot(target, addr, args.bytes, args.expect_max)
    return watch_writes(target, addr, args.bytes, args.hits)


if __name__ == "__main__":
    raise SystemExit(main())
