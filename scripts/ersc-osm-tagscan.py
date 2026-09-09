#!/usr/bin/env python3
"""Find Seamless Co-op's option-menu object in a running game, by content.

# The question this exists to answer

`local_invasion_filter::scan_for_session` looks for the session by walking `ersc.dll`'s own
writable sections and testing each qword as a pointer. On 2026-09-08 that scan was widened from
2.00 MB to the module's full 10.98 MB, and the result was decisive in the unwelcome direction:
crossing all of it still finds no global whose `+0x58` is a session -- so the OSM, the object every
ERSC option action takes as its first argument, is not parked in an `ersc.dll` global at all. The
static picture agrees: `ersc+0x258d0` has zero direct callers, no vtable holds it, and the
registrar at `ersc+0x2a1e0` jumps straight into the mutated `ERSC` section. Open issue
er-effects-rs-9i0g.

What is left is to look for the OSM by what it contains rather than by who points at it. It carries
the ASCII tag `seamless` at `+0x68` (measured live 2026-08-04) and the session at `+0x58`. That
pair is a content signature, and unlike a pointer scan it does not care where the object lives.

This tool answers whether that signature is findable, and how uniquely, before any of it is built
into the DLL -- because a heap-wide scan on the game thread is expensive enough that it should be
proven to work first.

# How it reads

`/proc/<pid>/mem`, seeking over the writable regions named in `/proc/<pid>/maps`. Nothing is
injected, no thread is suspended, and no code runs in the target -- the same mechanism, and the
same reason for it, as `scripts/er-live-fields.py`: `frida.attach()` on this Wine/Proton target
segfaults inside the game and kills the session. See AGENTS.md.

    python3 scripts/ersc-osm-tagscan.py
    python3 scripts/ersc-osm-tagscan.py --process eldenring.exe --json
    python3 scripts/ersc-osm-tagscan.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# Both measured against Seamless v2.0.1 and recorded in
# `crates/er-invasion-warp/src/local_invasion_filter/ersc.rs`, which is the authority; they are
# repeated here rather than parsed out of the Rust because this tool has to run against a game the
# repo may not currently build for.
OSM_TAG = b"seamless"
OSM_TAG_OFFSET = 0x68
NEXT_OBJECT_OFFSET = 0x58
SESSION_STATE_OFFSET = 0x150
SESSION_MUTEX_OFFSET = 0x100
MTX_COUNT_OFFSET = 0x4C
# `_Mtx_try`, the bit MSVC's `std::mutex` constructor adds. Requiring it is what separates a real
# session from a table of ones: `_Mtx_plain` is `1`, and an object whose stride happens to be 0x50
# reads `1` at both `+0x100` and `+0x150` and satisfies a weaker check twice over.
MTX_TRY = 0x02
MTX_RECURSIVE = 0x100
# The four state codes this build's actions actually write.
SESSION_STATES = (0x01, 0x0E, 0x23, 0x13)
# A region larger than this is a reservation, not a heap the OSM lives in; skipping them keeps one
# pass to a few seconds rather than minutes.
MAX_REGION_BYTES = 512 << 20


def find_pid(process: str) -> int | None:
    """The pid whose `comm` is `process`, or `None`.

    Wine reports the Windows executable name in `comm`, which is why this matches on it rather
    than on the `exe` symlink -- that points at `wine64-preloader` for every Windows process in
    the prefix.
    """
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            if (entry / "comm").read_text(encoding="utf-8").strip() == process:
                return int(entry.name)
        except OSError:
            continue
    return None


def writable_regions(pid: int) -> list[tuple[int, int, str]]:
    """The process's writable mappings, as `(low, high, path)`."""
    pattern = re.compile(r"([0-9a-f]+)-([0-9a-f]+) (\S+)\s+\S+\s+\S+\s+\S+\s*(.*)")
    out: list[tuple[int, int, str]] = []
    for line in Path(f"/proc/{pid}/maps").read_text(encoding="utf-8").splitlines():
        matched = pattern.match(line)
        if not matched:
            continue
        low = int(matched.group(1), 16)
        high = int(matched.group(2), 16)
        perms = matched.group(3)
        if "r" in perms and "w" in perms and (high - low) <= MAX_REGION_BYTES:
            out.append((low, high, matched.group(4) or "[anon]"))
    return out


def session_verdict(blob: bytes, base: int, session: int) -> str:
    """What `session` looks like, judged the way the DLL's own scan judges it.

    Returns a short phrase rather than a bool so a near-miss is readable: knowing that a candidate
    failed on `_Count` and not on `_Type` is the difference between a wrong offset and a wrong
    object.
    """
    def dword(addr: int) -> int | None:
        offset = addr - base
        if offset < 0 or offset + 4 > len(blob):
            return None
        return int.from_bytes(blob[offset : offset + 4], "little")

    state = dword(session + SESSION_STATE_OFFSET)
    kind = dword(session + SESSION_MUTEX_OFFSET)
    count = dword(session + SESSION_MUTEX_OFFSET + MTX_COUNT_OFFSET)
    if state is None or kind is None or count is None:
        return "unreadable in this region"
    parts = [f"state={state:#x}", f"_Type={kind:#x}", f"_Count={count:#x}"]
    if state not in SESSION_STATES:
        parts.append("REJECT: state is not one this build writes")
    elif not (kind & MTX_TRY):
        parts.append("REJECT: _Type lacks _Mtx_try, so it is not a std::mutex")
    elif not (kind & MTX_RECURSIVE) and count > 1:
        parts.append("REJECT: _Count above 1 on a non-recursive mutex")
    else:
        parts.append("SESSION-SHAPED")
    return " ".join(parts)


def scan(pid: int) -> list[dict[str, object]]:
    regions = writable_regions(pid)
    hits: list[dict[str, object]] = []
    with open(f"/proc/{pid}/mem", "rb", buffering=0) as mem:
        for low, high, path in regions:
            try:
                mem.seek(low)
                blob = mem.read(high - low)
            except (OSError, ValueError, OverflowError):
                continue
            start = 0
            while True:
                found = blob.find(OSM_TAG, start)
                if found < 0:
                    break
                start = found + 1
                osm = low + found - OSM_TAG_OFFSET
                # The tag occurs inside longer strings too (`seamless buddy system`, source
                # paths), so alignment is the first cheap filter on a text match.
                if osm % 8:
                    continue
                next_offset = found - OSM_TAG_OFFSET + NEXT_OBJECT_OFFSET
                if next_offset < 0 or next_offset + 8 > len(blob):
                    continue
                session = int.from_bytes(blob[next_offset : next_offset + 8], "little")
                hits.append(
                    {
                        "osm": osm,
                        "session": session,
                        "region": path,
                        "verdict": session_verdict(blob, low, session) if session else "null",
                    }
                )
    return hits


def find_pointers_to(pid: int, target: int) -> list[dict[str, object]]:
    """Every 8-aligned qword in the process equal to `target`, with the offset it would sit at.

    The inverse of the tag search, and the one that does not depend on a tag. `ersc_owner_or_refuse`
    needs the OSM -- an object whose `+0x58` is the session -- and a full sweep of `ersc.dll`'s
    writable data finds none, so the question is whether such an object exists anywhere. If a hit
    lands at `NEXT_OBJECT_OFFSET` from a plausible allocation, that is the owner and the DLL's
    search is simply looking in the wrong place. If every hit is at some other offset, the OSM does
    not hold the session at `+0x58` in this build and the offset itself is wrong.
    """
    needle = target.to_bytes(8, "little")
    out: list[dict[str, object]] = []
    with open(f"/proc/{pid}/mem", "rb", buffering=0) as mem:
        for low, high, path in writable_regions(pid):
            try:
                mem.seek(low)
                blob = mem.read(high - low)
            except (OSError, ValueError, OverflowError):
                continue
            start = 0
            while True:
                found = blob.find(needle, start)
                if found < 0:
                    break
                start = found + 1
                if (low + found) % 8:
                    continue
                out.append(
                    {
                        "at": low + found,
                        "as_osm": low + found - NEXT_OBJECT_OFFSET,
                        "region": path,
                    }
                )
    return out


def selftest() -> int:
    """Prove the signature logic on synthetic bytes, so a live run of zero hits means zero hits.

    The three cases are the three this tool exists to tell apart: a real session, the stride-0x50
    table of ones that defeated the DLL's previous check on 2026-09-08, and a plausible object
    whose state field holds a value no action writes.
    """
    base = 0x10000
    blob = bytearray(0x400)

    def put(offset: int, value: int) -> None:
        blob[offset : offset + 4] = value.to_bytes(4, "little")

    put(SESSION_MUTEX_OFFSET, 0x03)  # _Mtx_plain | _Mtx_try, what std::mutex writes
    put(SESSION_MUTEX_OFFSET + MTX_COUNT_OFFSET, 0)
    put(SESSION_STATE_OFFSET, 0x0E)
    good = session_verdict(bytes(blob), base, base)
    assert "SESSION-SHAPED" in good, good

    # The measured layout of `0x451200` in run br-20260908-200726-15d4: ones every 0x50 bytes,
    # the run starting at +0x60, so they land on +0x100 and +0x150 -- the mutex `_Type` and the
    # state field, one stride apart. Two checks reading one repeating field is one check.
    ones = bytearray(0x400)
    for offset in range(0x60, 0x400, 0x50):
        ones[offset : offset + 4] = (1).to_bytes(4, "little")
    ones[SESSION_MUTEX_OFFSET + MTX_COUNT_OFFSET : SESSION_MUTEX_OFFSET + MTX_COUNT_OFFSET + 4] = (
        0x6FFF
    ).to_bytes(4, "little")
    table = session_verdict(bytes(ones), base, base)
    assert "_Mtx_try" in table, table

    put(SESSION_STATE_OFFSET, 0x77)
    stranger = session_verdict(bytes(blob), base, base)
    assert "state is not one this build writes" in stranger, stranger

    print("selftest ok:")
    print(f"  real session      -> {good}")
    print(f"  stride-0x50 ones  -> {table}")
    print(f"  unknown state     -> {stranger}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--process", default="eldenring.exe")
    parser.add_argument("--pid", type=int)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument(
        "--pointers-to",
        help="hex address; report every 8-aligned qword in the process holding it",
    )
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    pid = args.pid or find_pid(args.process)
    if pid is None:
        print(f"no {args.process} running", file=sys.stderr)
        return 1

    if args.pointers_to:
        target = int(args.pointers_to, 16)
        found = find_pointers_to(pid, target)
        print(f"pid {pid}: {len(found)} pointer(s) to {target:#x}")
        for hit in found:
            print(
                f"  at {hit['at']:#x}  (would be OSM {hit['as_osm']:#x} "
                f"if this is +{NEXT_OBJECT_OFFSET:#x})  {hit['region']}"
            )
        return 0

    regions = writable_regions(pid)
    total = sum(high - low for low, high, _ in regions)
    hits = scan(pid)
    if args.json:
        print(json.dumps({"pid": pid, "regions": len(regions), "hits": hits}, indent=2))
        return 0

    print(f"pid {pid}: {len(regions)} writable regions, {total / 1048576:.0f} MB")
    print(f"{len(hits)} object(s) carrying {OSM_TAG!r} at +{OSM_TAG_OFFSET:#x}, 8-aligned")
    for hit in hits:
        print(f"  osm={hit['osm']:#x} session={hit['session']:#x}  {hit['verdict']}")
        print(f"      region {hit['region']}")
    if not hits:
        print("  -- none. The OSM either does not exist yet this run (the menu has never been")
        print("     opened) or does not carry the tag where this expects it.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
