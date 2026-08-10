#!/usr/bin/env python3
"""Terse CLI over the Ghidra MCP daemon (see mcp_query.py).

Usage: python3 scripts/ghidra/q.py <camelCaseMethod> key=value [key=value ...]

Prints the first long text field of the result (decompiled code / disassembly)
raw, then any remaining metadata, so decompiles are readable without jq.
"""
import json
import sys
import os

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcp_query import query  # noqa: E402

TEXT_KEYS = (
    "decompiled_code",
    "decompilation",
    "decompiled",
    "code",
    "disassembly",
    "text",
    "listing",
)


def main() -> None:
    if len(sys.argv) < 2:
        print(__doc__)
        raise SystemExit(2)
    method = sys.argv[1]
    params = {}
    for kv in sys.argv[2:]:
        k, _, v = kv.partition("=")
        params[k] = v
    resp = query(method, params, timeout=110)
    result = resp.get("result")
    if isinstance(result, str):
        print(result)
        return
    if isinstance(result, dict):
        for key in TEXT_KEYS:
            val = result.get(key)
            if isinstance(val, str):
                print(val)
                rest = {k: v for k, v in result.items() if k != key}
                if rest:
                    print("--- meta ---")
                    print(json.dumps(rest, indent=2)[:4000])
                return
        print(json.dumps(result, indent=2)[:40000])
        return
    print(json.dumps(resp, indent=2)[:40000])


main()
