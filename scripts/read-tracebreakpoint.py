#!/usr/bin/env python3
"""Human-readable view of a linux-x86-debug `tracebreakpoint` evidence JSON.

The raw evidence file buries the useful parts (parsed captures + the live winedbg/gdb
session text, whose newlines are JSON-escaped) inside a deeply nested object. This wraps
it so both the agent and a human can read a capture at a glance: status, each breakpoint
hit with its label=value captures and backtrace, and the raw gdb session rendered with
real newlines.

Usage:
  scripts/read-tracebreakpoint.py <evidence.json> [--raw-only] [--no-raw]
"""
import json
import sys


def find_all(obj, key):
    """Recursively collect every value stored under `key` anywhere in the JSON."""
    out = []
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == key:
                out.append(v)
            out.extend(find_all(v, key))
    elif isinstance(obj, list):
        for v in obj:
            out.extend(find_all(v, key))
    return out


def main():
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    flags = {a for a in sys.argv[1:] if a.startswith("--")}
    if not args:
        print("usage: read-tracebreakpoint.py <evidence.json> [--raw-only] [--no-raw]", file=sys.stderr)
        raise SystemExit(2)
    path = args[0]
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)

    bar = "=" * 78
    if "--raw-only" not in flags:
        print(bar)
        print(f"FILE       : {path}")
        statuses = find_all(data, "status")
        # The top-level status is the most meaningful; show the first string/short one.
        for s in statuses:
            if isinstance(s, str):
                print(f"STATUS     : {s}")
                break
        for label in ("targetPath", "breakpointAddress"):
            vals = find_all(data, label)
            if vals:
                print(f"{label:11}: {vals[0]}")
        # Parsed, structured hits (label = value), if the tool recorded them.
        for hits in find_all(data, "hits"):
            if isinstance(hits, list) and hits:
                for i, h in enumerate(hits):
                    if not isinstance(h, dict):
                        continue
                    print(f"\n--- parsed HIT {i} @ {h.get('address', '?')} ---")
                    for c in h.get("captures", []) or []:
                        if isinstance(c, dict):
                            print(f"  {str(c.get('label')):22} = {c.get('value')}")
                    bt = h.get("backtrace") or []
                    if bt:
                        print("  backtrace:")
                        for frame in bt:
                            print(f"    {frame}")

    # The richest view: the raw winedbg/gdb session text, newlines rendered.
    if "--no-raw" not in flags:
        for out in find_all(data, "stdout"):
            if isinstance(out, str) and "APP-DEBUGGER" in out:
                print("\n" + bar)
                print("RAW winedbg/gdb session (thread-spam filtered, newlines rendered):")
                print(bar)
                for line in out.split("\n"):
                    stripped = line.strip()
                    if stripped.startswith("[New Thread") or stripped.startswith("[Thread"):
                        continue
                    print(line)


if __name__ == "__main__":
    main()
