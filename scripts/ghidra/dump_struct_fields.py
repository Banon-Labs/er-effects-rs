#!/usr/bin/env python3
"""Print only the named fields of a Ghidra structure via the MCP daemon.

Usage: python3 scripts/ghidra/dump_struct_fields.py [--port 8765] <StructName>...
The full getStructure payload is thousands of lines of `undefined` padding; this
keeps the reviewable part (offset, field name, type) so an agent can read a
layout without burning context.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcp_query import query  # noqa: E402


def main(argv):
    port = 8765
    names = []
    i = 0
    while i < len(argv):
        if argv[i] == "--port":
            port = int(argv[i + 1])
            i += 2
            continue
        names.append(argv[i])
        i += 1
    for s in names:
        r = query("getStructure", {"name": s}, port=port).get("result")
        if not r or "fields" not in r:
            print("=====", s, "MISS", str(r)[:200])
            continue
        print("=====", s, "size=0x%x" % r["size"])
        for f in r["fields"]:
            fn, dt = f["fieldName"], f["dataType"]
            if not fn or dt == "undefined":
                continue
            print("  +0x%03x %-34s %s" % (f["offset"], fn, dt))


if __name__ == "__main__":
    main(sys.argv[1:])
