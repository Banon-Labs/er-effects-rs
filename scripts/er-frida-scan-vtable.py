#!/usr/bin/env python3
"""Count live instances of one C++ class in the running game, by scanning for its vtable.

# Why this exists

On 2026-09-09 a boot stalled waiting for a `CS::MenuMemberFuncJob<CS::TitleTopDialog>` node.
A first scan reported two dead candidates and stopped at a self-imposed 812 MB cap, printing
`truncated: true`. That is a statement about coverage, not about the process -- but read
quickly it looks like proof the object does not exist, which is the wrong conclusion to draw
about the one object the whole diagnosis turns on.

So this tool has no byte cap and reports its own coverage explicitly: how many ranges exist,
how many it read, how many it could not. A caller can then tell an absence from a shortfall.

Usage:
    uv run --with frida python3 scripts/er-frida-scan-vtable.py 0x142b29650
    uv run --with frida python3 scripts/er-frida-scan-vtable.py 0x142b29650 --json out.json
"""

from __future__ import annotations

import argparse
import json
import sys
import threading

DEVICE = "127.0.0.1:27042"
GAME = "eldenring.exe"
# Everything below the game module is Wine/CRT heap; the module itself holds the vtable and
# would report itself as a hit.
HEAP_CEILING = "0x140000000"

# The agent scans at top level and `send()`s before it returns, and `Script.load()` blocks until
# that top-level code finishes -- so the scan's own duration is not a timeout this script has to
# choose. All that is left is handing the queued message to the callback thread, which is why the
# cap below is small: it bounds delivery, not work.
DELIVERY_TIMEOUT_SECONDS = 30.0

AGENT = """
const PATTERN = '%(pattern)s';
const out = { total_ranges: 0, total_bytes: 0, scanned_ranges: 0, scanned_bytes: 0,
              unreadable_ranges: 0, hits: [] };
const ranges = Process.enumerateRanges('rw-').filter(r => r.base.compare(ptr('%(ceiling)s')) < 0);
out.total_ranges = ranges.length;
for (const r of ranges) out.total_bytes += r.size;
for (const r of ranges) {
    let hits;
    try { hits = Memory.scanSync(r.base, r.size, PATTERN); }
    catch (e) { out.unreadable_ranges++; continue; }
    out.scanned_ranges++; out.scanned_bytes += r.size;
    for (const h of hits) out.hits.push('0x' + h.address.toString(16));
}
send(out);
"""


def little_endian_pattern(value: int) -> str:
    return " ".join(f"{b:02x}" for b in value.to_bytes(8, "little"))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("vtable", help="vtable address, hex (0x...)")
    ap.add_argument("--json", help="also write the raw result here")
    args = ap.parse_args()

    import frida

    value = int(args.vtable, 16)
    device = frida.get_device_manager().add_remote_device(DEVICE)
    matches = [p for p in device.enumerate_processes() if p.name.lower() == GAME]
    if not matches:
        print(f"{GAME} is not running", file=sys.stderr)
        return 2
    session = device.attach(matches[0].pid)
    received: list = []
    delivered = threading.Event()

    def on_message(message, _data):
        received.append(message)
        delivered.set()

    script = session.create_script(
        AGENT % {"pattern": little_endian_pattern(value), "ceiling": HEAP_CEILING}
    )
    script.on("message", on_message)
    # Blocks for the whole scan: the agent has no callbacks and returns only once it has sent.
    script.load()
    delivered.wait(DELIVERY_TIMEOUT_SECONDS)
    session.detach()
    if not received:
        print("the agent finished but sent nothing", file=sys.stderr)
        return 3

    result = received[0].get("payload", received[0])
    if args.json:
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(result, handle, indent=2)
    covered = result["scanned_bytes"] / result["total_bytes"] * 100 if result["total_bytes"] else 0
    print(
        f"coverage: {result['scanned_ranges']}/{result['total_ranges']} ranges, "
        f"{result['scanned_bytes'] / 1e6:.0f}/{result['total_bytes'] / 1e6:.0f} MB "
        f"({covered:.1f}%), {result['unreadable_ranges']} unreadable"
    )
    print(f"instances of vtable {args.vtable}: {len(result['hits'])}")
    for hit in result["hits"][:64]:
        print("   ", hit)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
