#!/usr/bin/env python3
"""Find every live address that still HOLDS a given pointer value, read-only.

WHY THIS EXISTS (2026-08-12). `scripts/er-live-fields.py` answers "what is at this
address". It cannot answer the question that actually pins a lifetime bug: "this object
was destructed, so WHO is still pointing at it?" Enumerating candidate owners by hand is
guesswork, and this session burned four wrong mechanisms doing exactly that while a live
bugged process sat there with the answer in it.

Concretely it was written to chase a Load-Game path-editor input softlock where the
`02_990` MenuWindow at 0x1b3354880 had a NULL vtable (destructed) while its SceneObjProxy
at 0x1b3354a08 was still alive with in-image vtables. Whoever still references that proxy
is the thing keeping a dead menu in the graph.

HOW IT READS, AND WHY NOT FRIDA. Same contract as `er-live-fields.py`: it opens
`/proc/<pid>/mem` and reads. Nothing is injected, no thread is suspended, no code runs in
the target. Do NOT reach for frida on this Wine/Proton target -- `frida.attach()` injects a
bootstrapper that segfaults INSIDE eldenring.exe and kills the session (measured
2026-08-12, bd `frida-attach-kills-wine-eldenring-use-proc-mem-2026-08-12`). A read must
never be able to destroy the state it is reading.

The scan is bounded on purpose: only private, readable, non-file-backed regions by
default, so it walks the heaps rather than every mapped DLL image. `--include-images`
widens it when a static/global holder is suspected.

Examples:
  scripts/er-live-refs.py --process eldenring.exe --value 0x1b3354a08
  scripts/er-live-refs.py --process eldenring.exe --value 0x1b3354a08 --include-images
  scripts/er-live-refs.py --selftest
"""

from __future__ import annotations

import argparse
import os
import re
import struct
import sys

CHUNK = 1 << 20


def find_pid(name: str) -> int | None:
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/comm", errors="replace") as fh:
                if fh.read().strip() == name:
                    return int(entry)
        except OSError:
            continue
    return None


def regions(pid: int, include_images: bool) -> list[tuple[int, int, str]]:
    """Readable regions worth scanning, as (start, end, label)."""
    out: list[tuple[int, int, str]] = []
    with open(f"/proc/{pid}/maps", errors="replace") as fh:
        for line in fh:
            m = re.match(r"([0-9a-f]+)-([0-9a-f]+) (\S{4}) \S+ \S+ \S+\s*(.*)", line)
            if not m:
                continue
            start, end, perms, path = int(m.group(1), 16), int(m.group(2), 16), m.group(3), m.group(4)
            if "r" not in perms:
                continue
            # File-backed mappings are module images; skip unless explicitly requested. A
            # heap-only scan is what finds an owner that is itself a heap object.
            if path and not include_images:
                continue
            out.append((start, end, path or "[anon]"))
    return out


def scan(pid: int, value: int, include_images: bool, limit: int) -> list[tuple[int, str]]:
    needle = struct.pack("<Q", value)
    hits: list[tuple[int, str]] = []
    with open(f"/proc/{pid}/mem", "rb", 0) as mem:
        for start, end, label in regions(pid, include_images):
            addr = start
            while addr < end:
                size = min(CHUNK, end - addr)
                try:
                    mem.seek(addr)
                    buf = mem.read(size)
                except (OSError, ValueError, OverflowError):
                    addr += size
                    continue
                if not buf:
                    addr += size
                    continue
                off = buf.find(needle)
                while off != -1:
                    if off % 8 == 0:  # aligned slots only; unaligned matches are noise
                        hits.append((addr + off, label))
                        if len(hits) >= limit:
                            return hits
                    off = buf.find(needle, off + 1)
                addr += size
    return hits


def selftest() -> int:
    """Prove the scan finds a pointer this process really holds, in this process."""
    import ctypes

    payload = ctypes.create_string_buffer(64)
    target = ctypes.addressof(payload)
    holder = ctypes.c_uint64(target)
    holder_addr = ctypes.addressof(holder)
    hits = scan(os.getpid(), target, include_images=True, limit=4096)
    found = any(a == holder_addr for a, _ in hits)
    print(f"selftest: target=0x{target:x} holder=0x{holder_addr:x} hits={len(hits)} found_holder={found}")
    if not found:
        print("selftest FAILED: the known holder slot was not reported", file=sys.stderr)
        return 1
    print("selftest ok")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--pid", type=int)
    ap.add_argument("--process")
    ap.add_argument("--value", help="pointer value to hunt for, hex or decimal")
    ap.add_argument("--include-images", action="store_true", help="also scan file-backed module images")
    ap.add_argument("--limit", type=int, default=256)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if not args.value:
        ap.error("--value is required")
    pid = args.pid
    if pid is None and args.process:
        pid = find_pid(args.process)
    if pid is None:
        print("no target pid (pass --pid or --process)", file=sys.stderr)
        return 2
    value = int(args.value, 0)
    hits = scan(pid, value, args.include_images, args.limit)
    print(f"pid={pid} holders of 0x{value:x}: {len(hits)}"
          f"{' (limit reached)' if len(hits) >= args.limit else ''}")
    for addr, label in hits:
        print(f"  0x{addr:x}  {label}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
