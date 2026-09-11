#!/usr/bin/env python3
"""Name a list of addresses through a Ghidra MCP daemon, one line per address.

`getFunctionByAddress` answers one address per call, so resolving a decompilation's
callee list means a dozen round trips. Batching them here keeps that to one command
and prints the containing function's entry alongside the name, which is what tells an
interior address apart from an entry point.

Usage: `python3 scripts/ghidra/name-addresses.py <port> <address>...`
"""

import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
#: Hard cap on one daemon round trip. A lookup that has not answered in this long is a daemon
#: problem, not a slow query -- scripts/check-no-timeouts.py holds every agent-run subprocess to
#: the same 30 seconds so a mistake fails fast rather than after minutes.
QUERY_TIMEOUT_SECONDS = 30
QUERY = os.path.join(HERE, "mcp_query.py")


def name_one(port: str, address: str) -> str:
    proc = subprocess.run(
        [
            sys.executable,
            QUERY,
            "--port",
            port,
            "getFunctionByAddress",
            "--params",
            json.dumps({"address": address}),
        ],
        capture_output=True,
        text=True,
        timeout=QUERY_TIMEOUT_SECONDS,
        check=False,
    )
    try:
        result = json.loads(proc.stdout).get("result")
    except Exception:
        return f"unparseable: {proc.stdout[:200]!r}"
    if not isinstance(result, dict):
        return str(result)[:300]
    parts = [str(result.get("name"))]
    for key in ("address", "entry_point", "signature", "size"):
        if result.get(key) is not None:
            parts.append(f"{key}={result[key]}")
    return " | ".join(parts)


def main() -> int:
    if len(sys.argv) < 3:
        print(__doc__)
        return 2
    port = sys.argv[1]
    for address in sys.argv[2:]:
        print(f"{address} -> {name_one(port, address)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
