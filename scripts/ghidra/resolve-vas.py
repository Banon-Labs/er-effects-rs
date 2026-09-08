#!/usr/bin/env python3
"""Resolve a list of VAs to the functions that contain them, via the Ghidra MCP daemon.

The single most common follow-up to a byte scan: `find-deobf-bytes.py` prints a column of
addresses, and the next question is always "which functions are those?". One call per address
against `getFunctionByAddress` answers it, and a loop in the shell keeps tripping the worktree
guard, so it lives here instead.

Usage:
    python3 scripts/ghidra/resolve-vas.py 0x1402bf9b5 0x1402bfb23 ...
    python3 scripts/find-deobf-bytes.py '89??24c40000' | ... | python3 scripts/ghidra/resolve-vas.py -

`-` reads whitespace-separated addresses from stdin. `GPORT` picks the daemon: 8765 is the
1.16.2 named dump (the default, and the only one with symbols), 8767 the 1.17 structure dump.
A VA inside no known function prints `-`, which is a real answer -- it usually means the scan
landed in data rather than in code.
"""

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from mcp_query import query  # noqa: E402


def addresses(argv: list[str]) -> list[str]:
    """The addresses to resolve, from the command line or from stdin for `-`."""
    raw = " ".join(sys.stdin.read().split()) if argv == ["-"] else " ".join(argv)
    return re.findall(r"(?:0x)?[0-9a-fA-F]{6,16}", raw)


def main() -> int:
    argv = sys.argv[1:]
    if not argv:
        print(__doc__)
        return 2
    port = int(os.environ.get("GPORT", "8765"))
    for va in addresses(argv):
        at = va if va.startswith("0x") else f"0x{va}"
        try:
            result = query("getFunctionByAddress", {"address": at}, port=port).get(
                "result", {}
            )
        except Exception as exc:  # noqa: BLE001 -- the daemon's failure is the answer here
            print(f"{at}\tERROR\t{exc}")
            continue
        if not isinstance(result, dict) or result.get("error"):
            print(f"{at}\t-\t{result.get('error') if isinstance(result, dict) else result}")
            continue
        name = result.get("name", "-")
        entry = result.get("address", "-")
        print(f"{at}\t{name}\t@{entry}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
