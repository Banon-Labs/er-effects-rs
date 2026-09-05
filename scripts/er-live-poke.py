#!/usr/bin/env python3
"""Read, scan and write a running ELDEN RING's memory from OUTSIDE the process.

WHY THIS EXISTS, and why it is not frida. Frida cannot bootstrap into this target: three attempts on
2026-09-05 produced three different failures, ending at `ptrace pokedata: Input/output error`. That
was never a permissions problem -- measured on the live game (pid 2266653, ER 1.17 under Proton):

    /proc/<pid>/mem  open O_RDONLY -> OK
    /proc/<pid>/mem  open O_RDWR   -> OK
    PTRACE_ATTACH                  -> 0

so this process can already read AND write the game's address space directly. Frida's ptrace-based
agent injection is a means to that end, and the end is available without it.

WHAT THIS REPLACES. Every live question used to cost a Rust verb in `er-input-harness`'s REPL, a
cross-compile and a relaunch -- "teach me a new input" and "rebuild the DLL" were the same act. Scans
and reads do not need to be either: they are a seek and a read on a file descriptor.

WHAT IT DELIBERATELY DOES NOT DO. It writes memory only when asked with `poke`, and driving GAME
INPUT is not what it is for: the input path is the DirectInput keyboard buffer, which the game refills
every frame, so an outside write races it and loses. Input still belongs in the in-process stamp.
Reading state -- which grid is the cursor, what is its cell, did that press land -- is what this owns.

    scripts/er-live-poke.py --pid N scan 0x142a94438        every address holding that qword
    scripts/er-live-poke.py --pid N read 0x89f78ab8 32      dump qwords
    scripts/er-live-poke.py --pid N grid                    every CS::GridControl + selected cell
    scripts/er-live-poke.py --pid N poke 0xADDR 0xVALUE     write one qword (explicit, never implied)
"""

import argparse
import os
import re
import struct
import sys

# CS::GridControl on 1.17, from scripts/er-rtti-map.py walking MSVC RTTI in eldenring-deobf-1.17.bin.
GRID_CONTROL_VTABLE = 0x142A94438
# Selected cell, off the pager's comparisons against the extents at +0xd0/+0xd8/+0xdc.
GRID_SELECTED_OFFSET = 0xD4
# Skip the game's own PE image and anything below the heap: a vtable VALUE appears inside .rdata as
# the vtable itself, which is not an instance and would be reported as one.
HEAP_LO = 0x10000000
CHUNK = 1 << 20


def readable_regions(pid):
    """Committed, readable regions, skipping file-backed mappings of the game image.

    Anonymous rw regions are where instances live; including the mapped PE would report the vtable's
    own address as an object every time.
    """
    out = []
    with open(f"/proc/{pid}/maps") as handle:
        for line in handle:
            m = re.match(r"([0-9a-f]+)-([0-9a-f]+) (\S{4})\s+\S+\s+\S+\s+\S+\s*(.*)", line)
            if not m:
                continue
            start, end, perms, path = int(m.group(1), 16), int(m.group(2), 16), m.group(3), m.group(4)
            if "r" not in perms:
                continue
            out.append((start, end, path))
    return out


def scan(pid, needle, max_hits, lo=HEAP_LO):
    packed = struct.pack("<Q", needle)
    hits = []
    with open(f"/proc/{pid}/mem", "rb", 0) as mem:
        for start, end, _path in readable_regions(pid):
            if end <= lo:
                continue
            address = max(start, lo)
            while address < end:
                size = min(CHUNK, end - address)
                try:
                    mem.seek(address)
                    buf = mem.read(size)
                except OSError:
                    address += size
                    continue
                if not buf:
                    break
                offset = buf.find(packed)
                while offset != -1:
                    if (address + offset) % 8 == 0:
                        hits.append(address + offset)
                        if len(hits) >= max_hits:
                            return hits, True
                    offset = buf.find(packed, offset + 1)
                address += size
    return hits, False


def read_qwords(pid, address, count):
    with open(f"/proc/{pid}/mem", "rb", 0) as mem:
        try:
            mem.seek(address)
            buf = mem.read(count * 8)
        except OSError as exc:
            return None, str(exc)
    return [struct.unpack_from("<Q", buf, i * 8)[0] for i in range(len(buf) // 8)], None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--pid", type=int, required=False)
    ap.add_argument("--max-hits", type=int, default=64)
    ap.add_argument("command", nargs="?", default="grid")
    ap.add_argument("args", nargs="*")
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()

    if a.selftest:
        # Prove the read path against THIS process rather than against the game: a selftest that
        # needs the game running is a selftest nobody runs.
        me = os.getpid()
        vals, err = read_qwords(me, id(0) & ~7, 1)
        print("SELFTEST OK" if err is None else f"SELFTEST FAILED: {err}")
        return 0 if err is None else 1

    if not a.pid:
        print("--pid is required", file=sys.stderr)
        return 2

    if a.command == "grid":
        hits, capped = scan(a.pid, GRID_CONTROL_VTABLE, a.max_hits)
        print(f"GridControl vtable 0x{GRID_CONTROL_VTABLE:x}: {len(hits)} instance(s)"
              f"{' (CAPPED)' if capped else ''}")
        for hit in hits:
            cells, _ = read_qwords(a.pid, hit + GRID_SELECTED_OFFSET, 1)
            cell = (cells[0] & 0xFFFFFFFF) if cells else -1
            if cell >= 0x80000000:
                cell -= 1 << 32
            print(f"  0x{hit:x}  selected_cell={cell}")
    elif a.command == "scan":
        needle = int(a.args[0], 0)
        hits, capped = scan(a.pid, needle, a.max_hits)
        print(f"0x{needle:x}: {len(hits)} hit(s){' (CAPPED)' if capped else ''}")
        for hit in hits:
            print(f"  0x{hit:x}")
    elif a.command == "read":
        address = int(a.args[0], 0)
        count = int(a.args[1], 0) if len(a.args) > 1 else 8
        vals, err = read_qwords(a.pid, address, count)
        if err:
            print(f"unreadable: {err}", file=sys.stderr)
            return 1
        for i, v in enumerate(vals):
            print(f"  [0x{address + i * 8:x}] +0x{i * 8:<4x} = 0x{v:x}")
    elif a.command == "poke":
        address, value = int(a.args[0], 0), int(a.args[1], 0)
        with open(f"/proc/{a.pid}/mem", "r+b", 0) as mem:
            mem.seek(address)
            mem.write(struct.pack("<Q", value))
        print(f"wrote 0x{value:x} to 0x{address:x}")
    else:
        print(f"unknown command {a.command!r}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
