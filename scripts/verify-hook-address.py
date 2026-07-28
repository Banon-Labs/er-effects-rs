#!/usr/bin/env python3
"""Verify a VA holds the SAME function in the 1.16.2 Ghidra dump and in `eldenring-deobf.bin`.

Why this is mandatory before hooking
------------------------------------
The Ghidra dump is authoritative for MEANING; `eldenring-deobf.bin` is authoritative for
ADDRESSES (it is what the DLL actually patches). They are not the same file, and the offset
between them is piecewise-constant per code region -- so a VA read off the dump can land
mid-instruction in the live image and turn a detour into a crash.

Comparing the two disassemblies as TEXT does not work: Ghidra prints `RAX,RSP` where capstone
prints `rax, rsp`, `[RAX + -0x5f]` vs `[rax - 0x5f]`, `JZ` vs `je`. An earlier pass reported all
six addresses as MISMATCH purely from that formatting. So this compares the two things that are
format-independent and still catch a misaligned address: the MNEMONIC and the instruction
LENGTH, in order.

Usage:
  python3 scripts/verify-hook-address.py 0x1408d1d00 [0x1408d2110 ...] [--count 40]

Exit 0 only when every address matches. Needs the Ghidra MCP daemon on :8765
(`bash scripts/ghidra/mcp-up-1162.sh`).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

IMAGE_BASE = 0x140000000
DEOBF = "eldenring-deobf.bin"
#: Repo-wide non-game shell cap (scripts/check-no-timeouts.py MAX_TIMEOUT_SECONDS).
MCP_QUERY_TIMEOUT_SECONDS = 30

# Ghidra and capstone spell the same instruction differently; neither difference can mask a
# misaligned address, since a wrong VA changes the LENGTH sequence almost immediately.
ALIASES = {
    "jz": "je",
    "jnz": "jne",
    "jc": "jb",
    "jnc": "jae",
    "setz": "sete",
    "setnz": "setne",
    "cmovz": "cmove",
    "cmovnz": "cmovne",
    "cmovnb": "cmovae",
    "movsxd": "movsxd",
    "retn": "ret",
}


def _bootstrap():
    try:
        import capstone  # noqa: F401

        return
    except ImportError:
        pass
    if os.environ.get("_ER_VERIFY_BOOTSTRAPPED"):
        sys.exit("capstone unavailable and uv bootstrap already attempted")
    os.execvpe(
        "uv",
        ["uv", "run", "--with", "capstone", "python3", *sys.argv],
        dict(os.environ, _ER_VERIFY_BOOTSTRAPPED="1"),
    )


def repo_root() -> str:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, timeout=10
        ).stdout.strip()
        if out:
            return out
    except Exception:
        pass
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def norm(m: str) -> str:
    m = m.lower().strip()
    return ALIASES.get(m, m)


def dump_insns(root: str, va: int, count: int) -> list[tuple[int, str]]:
    """(address, mnemonic) from the Ghidra 1.16.2 dump. Lengths come from address deltas."""
    out = subprocess.run(
        [
            sys.executable,
            os.path.join(root, "scripts/ghidra/mcp_query.py"),
            "disassembleFunction",
            "--params",
            json.dumps({"address": f"{va:x}", "length": count * 8}),
        ],
        capture_output=True,
        text=True,
        # Hard safety cap only. The daemon answers a bounded disassembly request in well
        # under a second, so reaching this means it is wedged, not slow.
        timeout=MCP_QUERY_TIMEOUT_SECONDS,
    )
    data = json.loads(out.stdout)
    items = data.get("result", {}).get("instructions", [])
    return [(int(i["address"], 16), norm(i["mnemonic"])) for i in items][:count]


def main() -> int:
    _bootstrap()
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    ap = argparse.ArgumentParser()
    ap.add_argument("addresses", nargs="+")
    ap.add_argument("--count", type=int, default=32, help="instructions to compare")
    args = ap.parse_args()

    root = repo_root()
    with open(os.path.join(root, DEOBF), "rb") as f:
        image = f.read()
    md = Cs(CS_ARCH_X86, CS_MODE_64)

    all_ok = True
    for spec in args.addresses:
        va = int(spec, 0)
        ghidra = dump_insns(root, va, args.count)
        if not ghidra:
            print(f"0x{va:x}  FAIL: dump returned no instructions")
            all_ok = False
            continue
        # Ghidra gives addresses, not sizes; derive size from the next address.
        g_pairs = [
            (m, ghidra[i + 1][0] - a) for i, (a, m) in enumerate(ghidra[:-1])
        ]
        off = va - IMAGE_BASE
        d_pairs = [
            (norm(i.mnemonic), i.size)
            for i in md.disasm(image[off : off + args.count * 8], va)
        ][: len(g_pairs)]

        n = min(len(g_pairs), len(d_pairs))
        bad = [
            (k, g_pairs[k], d_pairs[k]) for k in range(n) if g_pairs[k] != d_pairs[k]
        ]
        if bad:
            all_ok = False
            print(f"0x{va:x}  MISMATCH ({len(bad)}/{n} differ) -- do NOT hook this address")
            for k, g, d in bad[:6]:
                print(f"    #{k}: dump={g}  deobf={d}")
        else:
            print(f"0x{va:x}  OK  ({n} instructions identical: mnemonic + length)")
    return 0 if all_ok else 1


if __name__ == "__main__":
    sys.exit(main())
