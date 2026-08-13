#!/usr/bin/env python3
"""Read-only live memory inspection of a running (Wine/Proton) game process.

WHY THIS EXISTS (user directive 2026-08-12). Answering a read-only question about game memory --
"which field is the caret", "does this offset track the scroll", "what is this pointer now" -- used
to mean editing the DLL to add a telemetry dump, rebuilding, tearing the game down and relaunching
it. That throws away a live session and the user's place in the menus to learn something a read
could have taken straight out of the running process.

HOW IT READS, AND WHY NOT FRIDA (learned the hard way 2026-08-12). This opens `/proc/<pid>/mem` and
seeks. Nothing is injected into the target, no thread is suspended, and no code runs inside the
game -- it is the same mechanism a debugger uses to peek memory, minus the debugger.

Do NOT reach for frida here. `frida.attach()` on the Wine/Proton `eldenring.exe` injects a
bootstrapper that segfaults **inside the target**: it printed "bootstrapper crashed with signal 11"
and killed a live session mid-session, destroying the very thing the read was meant to preserve.
A read must never be able to do that, so the injection path is gone rather than merely discouraged.

If you need to know WHICH CODE writes a field (not just its value), that genuinely needs in-process
instrumentation, and the sanctioned path for a Wine target is the `linux-x86-debug` sibling toolkit's
`tracebreakpoint` (winedbg --gdb attach), NOT frida. See AGENTS.md.

Examples:
    scripts/er-live-fields.py --selftest
    scripts/er-live-fields.py --process eldenring.exe --addr 0x2d7aaa40 --bytes 384
    scripts/er-live-fields.py --process eldenring.exe --addr 0x2d7aaa40 --expect-max 56
    scripts/er-live-fields.py --process eldenring.exe --addr 0x2d7aaa40 --raw
"""

from __future__ import annotations

import argparse
import os
import struct
import sys

PROC = "/proc"


def find_pid(name: str) -> int | None:
    """Resolve a process name by scanning /proc.

    Deliberately not pgrep: the repo's cupcake guard blocks it outright (it false-negatives on this
    box and self-matches its own command line). `comm` is truncated to 15 characters by the kernel,
    so match on a prefix rather than equality -- "eldenring.exe" fits, but a longer name would not.
    """
    want = name.lower()
    for entry in os.listdir(PROC):
        if not entry.isdigit():
            continue
        try:
            with open(f"{PROC}/{entry}/comm", encoding="utf-8", errors="replace") as fh:
                comm = fh.read().strip().lower()
        except OSError:
            continue
        if comm and (comm == want or want.startswith(comm) or comm.startswith(want[:15])):
            return int(entry)
    return None


def read_window(pid: int, addr: int, size: int) -> bytes | None:
    """Read `size` bytes at `addr`. Returns None if the address is not mapped."""
    try:
        # Buffering off: a buffered reader would happily read ahead past the requested window into
        # an unmapped neighbouring page and turn a good read into an error.
        with open(f"{PROC}/{pid}/mem", "rb", 0) as fh:
            fh.seek(addr)
            data = fh.read(size)
    except (OSError, ValueError):
        return None
    return data if data and len(data) == size else None


def dump(pid: int, addr: int, size: int, expect_max: int, raw: bool) -> int:
    data = read_window(pid, addr, size)
    if data is None:
        print(f"FAIL: 0x{addr:x} is not readable in pid {pid}", file=sys.stderr)
        return 2

    print(f"snapshot pid={pid} 0x{addr:x} bytes={len(data)}")
    if raw:
        for off in range(0, len(data), 16):
            chunk = data[off : off + 16]
            text = "".join(chr(b) if 32 <= b < 127 else "." for b in chunk)
            print(f"  +0x{off:<4x} {chunk.hex(' '):<47}  {text}")
        return 0

    words = struct.unpack_from(f"<{len(data) // 8}Q", data, 0)
    for slot, qword in enumerate(words):
        off = slot * 8
        lo = qword & 0xFFFFFFFF
        hi = qword >> 32
        if qword == 0:
            continue
        notes = []
        # A pointer-shaped value in the game's heap/module range, so object graphs are walkable
        # without hand-decoding every qword.
        if 0x10000 < qword < 0x7FFFFFFFFFFF:
            notes.append(f"ptr=0x{qword:x}")
        # An index-shaped value inside 0..expect_max separates a candidate caret/cursor/length from
        # pointer halves and timers, without hiding anything else.
        if expect_max:
            for half, value in (("lo", lo), ("hi", hi)):
                if 0 <= value <= expect_max:
                    notes.append(f"{half}={value}*")
        print(f"  +0x{off:<4x} {lo:>10} {hi:>10}  {' '.join(notes)}")
    return 0


def selftest() -> int:
    """Prove the read path end to end without touching the game.

    A CHILD process is the target rather than this one, so the test exercises the real
    cross-process read rather than a same-process shortcut that would pass for the wrong reason.
    """
    import subprocess

    child_source = (
        "import ctypes, struct, sys\n"
        "buf = ctypes.create_string_buffer(4096)\n"
        "struct.pack_into('<Q', buf, 8, 0xfeedface)\n"
        "print(ctypes.addressof(buf), flush=True)\n"
        "sys.stdin.readline()\n"
    )
    child = subprocess.Popen(
        [sys.executable, "-c", child_source],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        line = child.stdout.readline().strip() if child.stdout else ""
        if not line.isdigit():
            print(f"SELFTEST FAIL: child did not report an address ({line!r})", file=sys.stderr)
            return 1
        data = read_window(child.pid, int(line), 64)
    finally:
        child.kill()
        child.wait(timeout=5)

    if data is None:
        print("SELFTEST FAIL: could not read the child's memory", file=sys.stderr)
        return 1
    value = struct.unpack_from("<Q", data, 8)[0]
    if value != 0xFEEDFACE:
        print(f"SELFTEST FAIL: read 0x{value:x}, expected 0xfeedface", file=sys.stderr)
        return 1
    print("SELFTEST OK: cross-process read works (slot +0x8 == 0xfeedface)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, help="target pid")
    parser.add_argument("--process", help="target process name, e.g. eldenring.exe")
    parser.add_argument("--addr", help="address to inspect, hex or decimal")
    parser.add_argument("--bytes", type=int, default=512, help="window size (default 512)")
    parser.add_argument("--raw", action="store_true", help="hex+ascii dump instead of qwords")
    parser.add_argument(
        "--expect-max",
        type=int,
        default=0,
        help="mark 32-bit halves whose value is within 0..N (e.g. a text length)",
    )
    parser.add_argument("--selftest", action="store_true", help="verify the read path on a child")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if args.addr is None:
        parser.error("--addr is required (or use --selftest)")

    pid = args.pid
    if pid is None:
        if not args.process:
            parser.error("one of --pid or --process is required")
        pid = find_pid(args.process)
        if pid is None:
            print(f"FAIL: no process matching {args.process!r} is running", file=sys.stderr)
            return 3

    return dump(pid, int(args.addr, 0), args.bytes, args.expect_max, args.raw)


if __name__ == "__main__":
    raise SystemExit(main())
