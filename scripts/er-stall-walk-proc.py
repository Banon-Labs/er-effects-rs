#!/usr/bin/env python3
"""Walk a wedged Wine process's threads entirely from `/proc` -- no code runs in the target.

# Why this exists beside the Frida walker

`scripts/er-frida-stall-walk.py` is the better tool when it works, but it needs the target to
execute Frida's agent, and a fully deadlocked game cannot: `session.create_script` returns
`frida.TransportError: timeout was reached`. Measured 2026-09-08 against a game whose 130 threads
were all parked. So the walker that has to work on the worst case cannot depend on the target
running anything at all.

Everything here is a read:

* `/proc/<pid>/task/*/syscall` gives each thread's userspace stack pointer and program counter at
  the syscall boundary -- readable for one's own processes with no privilege.
* `/proc/<pid>/mem` gives the stack itself, which is scanned for qwords that land inside a known
  module. That is a fuzzy backtrace: it over-reports, because a stale return address left on the
  stack looks exactly like a live one. It is also the only kind available without unwind data, and
  it is what the game's own crash logger produces.

Module bases are read from `/proc/<pid>/maps`, which under Wine carries no filename for a PE (they
map as anonymous memory), so the well-known bases are named explicitly and everything else is
reported as a bare address inside its mapping.

    python3 scripts/er-stall-walk-proc.py            # finds eldenring.exe itself
    python3 scripts/er-stall-walk-proc.py --pid 1234
    python3 scripts/er-stall-walk-proc.py --selftest
"""

from __future__ import annotations

import argparse
import pathlib
import struct
import sys

# Bases that are fixed for this target, so a frame in one of them is named rather than left bare.
# Mod DLLs land in the Wine DLL arena and are discovered from `maps` instead.
KNOWN_BASES = [(0x140000000, 0x146000000, "eldenring.exe"), (0x180000000, 0x180400000, "ersc.dll")]
# How much stack to scan per thread. Deep enough to cross the game's task frames into a mod DLL,
# small enough that 130 threads stay inside the shell budget.
STACK_BYTES = 0x2000
FRAMES_PER_THREAD = 24


def game_pid() -> int | None:
    for entry in pathlib.Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if (entry / "comm").read_text(encoding="utf-8").strip() == "eldenring.exe":
                return int(entry.name)
        except OSError:
            continue
    return None


def mappings(pid: int) -> list:
    out = []
    try:
        for line in (pathlib.Path("/proc") / str(pid) / "maps").read_text(errors="replace").splitlines():
            span, perms = line.split()[0], line.split()[1]
            low, high = (int(part, 16) for part in span.split("-"))
            name = line.split(maxsplit=5)[5] if len(line.split(maxsplit=5)) > 5 else ""
            out.append((low, high, perms, name))
    except OSError:
        pass
    return out


def attribute(address: int, maps: list) -> str | None:
    """Name an address, or return None when it is not inside anything executable."""
    for low, high, name in KNOWN_BASES:
        if low <= address < high:
            return f"{name}+0x{address - low:x}"
    for low, high, perms, name in maps:
        if low <= address < high and "x" in perms:
            return f"{name or 'anon'}@0x{low:x}+0x{address - low:x}"
    return None


def thread_pc_sp(pid: int, tid: int) -> tuple:
    """The `/proc/<tid>/syscall` sp and pc, or (None, None) when the thread is not in a syscall."""
    try:
        parts = (pathlib.Path("/proc") / str(pid) / "task" / str(tid) / "syscall").read_text().split()
    except OSError:
        return (None, None)
    if len(parts) < 2 or parts[0] == "running":
        return (None, None)
    try:
        return (int(parts[-2], 16), int(parts[-1], 16))
    except ValueError:
        return (None, None)


def walk(pid: int, maps: list) -> list:
    rows = []
    try:
        mem = open(f"/proc/{pid}/mem", "rb", 0)
    except OSError as error:
        print(f"cannot read /proc/{pid}/mem: {error}", file=sys.stderr)
        return rows
    task = pathlib.Path("/proc") / str(pid) / "task"
    for entry in sorted(task.iterdir(), key=lambda e: int(e.name)):
        tid = int(entry.name)
        try:
            comm = (entry / "comm").read_text().strip()
            wchan = (entry / "wchan").read_text().strip()
        except OSError:
            continue
        sp, pc = thread_pc_sp(pid, tid)
        frames = []
        if sp:
            try:
                mem.seek(sp)
                blob = mem.read(STACK_BYTES)
            except OSError:
                blob = b""
            seen = set()
            for offset in range(0, len(blob) - 8, 8):
                value = struct.unpack_from("<Q", blob, offset)[0]
                if value < 0x10000:
                    continue
                named = attribute(value, maps)
                if named is None or named in seen:
                    continue
                seen.add(named)
                frames.append(named)
                if len(frames) >= FRAMES_PER_THREAD:
                    break
        rows.append({"tid": tid, "comm": comm, "wchan": wchan, "pc": pc, "sp": sp, "frames": frames})
    mem.close()
    return rows


def interesting(row: dict) -> bool:
    """Whether a thread's stack reaches the game image or a mod DLL rather than only system code."""
    return any(
        frame.startswith(("eldenring.exe+", "ersc.dll+")) or "er_" in frame for frame in row["frames"]
    )


def render(pid: int, rows: list) -> str:
    hot = [row for row in rows if interesting(row)]
    lines = [
        f"proc stall walk -- pid {pid}, {len(rows)} threads, "
        f"{len(hot)} whose stack reaches the game or a mod DLL",
        "",
    ]
    for row in sorted(rows, key=lambda r: (not interesting(r), r["tid"])):
        head = f"--- {row['tid']} [{row['comm']}] wchan={row['wchan']}"
        if not interesting(row):
            lines.append(head + "  (system-only)")
            continue
        lines.append(head)
        for depth, frame in enumerate(row["frames"]):
            lines.append(f"      #{depth} {frame}")
    return "\n".join(lines) + "\n"


def selftest() -> int:
    maps = [(0x7F0000000000, 0x7F0000010000, "r-xp", "/usr/lib/libc.so")]
    assert attribute(0x140010043, maps) == "eldenring.exe+0x10043"
    assert attribute(0x180025850, maps) == "ersc.dll+0x25850"
    assert attribute(0x7F0000000100, maps) == "/usr/lib/libc.so@0x7f0000000000+0x100"
    assert attribute(0x1234, maps) is None, "a small integer is not a frame"
    rows = [
        {"tid": 1, "comm": "eldenring.exe", "wchan": "futex_wait", "pc": 1, "sp": 1,
         "frames": ["/usr/lib/libc.so@0x0+0x1"]},
        {"tid": 2, "comm": "eldenring.exe", "wchan": "ntsync_schedule", "pc": 1, "sp": 1,
         "frames": ["ersc.dll+0x25850", "eldenring.exe+0x26d7de5"]},
    ]
    assert not interesting(rows[0]) and interesting(rows[1])
    text = render(7, rows)
    assert text.index("--- 2 ") < text.index("--- 1 "), "the interesting thread must be printed first"
    assert "ersc.dll+0x25850" in text
    # This walker must never write to the target. The only handle it opens is read-only.
    assert '"rb"' in pathlib.Path(__file__).read_text(), "the mem handle must stay read-only"
    print("selftest ok: attribution, interest filter, ordering, read-only handle")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pid", type=int, default=None)
    parser.add_argument("--out", type=pathlib.Path, default=pathlib.Path.home() / ".cache" / "er-frida")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    pid = args.pid or game_pid()
    if pid is None:
        print("no eldenring.exe -- nothing to walk", file=sys.stderr)
        return 1
    maps = mappings(pid)
    rows = walk(pid, maps)
    text = render(pid, rows)
    args.out.mkdir(parents=True, exist_ok=True)
    path = args.out / f"proc-stall-walk-{pid}.txt"
    path.write_text(text, encoding="utf-8")
    print(text)
    print(f"report {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
