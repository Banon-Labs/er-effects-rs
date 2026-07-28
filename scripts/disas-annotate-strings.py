#!/usr/bin/env python3
"""Disassemble a function in a flat ER image and annotate RIP-relative operands with the
ASCII/UTF-16 string (or pointer) they point at.

Why this exists
---------------
Scaleform/UI reversing in this game is almost entirely "which child NAME does this binder
resolve?", and that name is always a RIP-relative `LEA reg,[rip+disp]` into `.rdata`. Reading
the disassembly alone shows `LEA R8,[0x142ad25c8]` and nothing else; the Ghidra MCP daemon
exposes no memory-read method (`readMemory`/`getBytes`/... are all "Unknown method"), so the
string had to be chased by hand every time. This resolves them inline.

Both ER images in the repo root are FLAT memory images based at 0x140000000 -- `offset = va -
base` with no section walk needed (same convention as `scripts/dantelion-static-scraper.py`).

  eldenring-deobf.bin  1.16.2, AUTHORITATIVE FOR ADDRESSES (what the DLL hooks/patches)
  dump-exec.bin        1.16.1, matches the older Ghidra dump -- semantics only

Usage
-----
  python3 scripts/disas-annotate-strings.py 0x1408d1e30 [--bytes 0x400] [--image deobf|dump]
  python3 scripts/disas-annotate-strings.py 0x1408d1e30 --calls      # only CALL/LEA lines
  python3 scripts/disas-annotate-strings.py --at 0x142ad25c8         # just read one string

Capstone is not installed system-wide; the script re-execs itself under `uv run --with
capstone` exactly like `scripts/dump-deobf-shift.py` does.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys

IMAGE_BASE = 0x140000000
IMAGES = {"deobf": "eldenring-deobf.bin", "dump": "dump-exec.bin"}


def _bootstrap():
    """Re-exec under uv with capstone when it is not importable (no system pip here)."""
    try:
        import capstone  # noqa: F401

        return
    except ImportError:
        pass
    if os.environ.get("_ER_DISAS_BOOTSTRAPPED"):
        sys.exit("capstone unavailable and uv bootstrap already attempted")
    env = dict(os.environ, _ER_DISAS_BOOTSTRAPPED="1")
    os.execvpe(
        "uv",
        ["uv", "run", "--with", "capstone", "python3", *sys.argv],
        env,
    )


def repo_root() -> str:
    try:
        out = subprocess.run(
            ["git", "rev-parse", "--show-toplevel"],
            capture_output=True,
            text=True,
            timeout=10,
        ).stdout.strip()
        if out:
            return out
    except Exception:
        pass
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load_image(which: str) -> bytes:
    name = IMAGES[which]
    path = os.path.join(repo_root(), name)
    if not os.path.exists(path):
        sys.exit(f"{path} not found")
    with open(path, "rb") as f:
        return f.read()


def read_str(data: bytes, va: int, limit: int = 120) -> str | None:
    """ASCII (or UTF-16LE) string at `va`, or None when it does not look like text."""
    off = va - IMAGE_BASE
    if off < 0 or off >= len(data):
        return None
    raw = data[off : off + limit * 2]
    # ASCII
    end = raw.find(b"\0")
    if end > 0:
        cand = raw[:end]
        if all(0x20 <= b < 0x7F for b in cand):
            return cand.decode("ascii")
    # UTF-16LE
    if len(raw) >= 4 and raw[1] == 0 and 0x20 <= raw[0] < 0x7F:
        chars = []
        for i in range(0, len(raw) - 1, 2):
            lo, hi = raw[i], raw[i + 1]
            if lo == 0 and hi == 0:
                break
            if hi != 0 or not (0x20 <= lo < 0x7F):
                return None
            chars.append(chr(lo))
        if chars:
            return "u16:" + "".join(chars)
    return None


def annotate(data: bytes, insn) -> str:
    """Trailing comment for one instruction: resolved RIP target + string/pointer."""
    from capstone import x86

    parts = []
    for op in insn.operands:
        if op.type == x86.X86_OP_MEM and op.mem.base == x86.X86_REG_RIP:
            target = insn.address + insn.size + op.mem.disp
            s = read_str(data, target)
            if s is not None:
                parts.append(f'-> 0x{target:x} "{s}"')
            else:
                off = target - IMAGE_BASE
                if 0 <= off + 8 <= len(data):
                    q = int.from_bytes(data[off : off + 8], "little")
                    ptr_str = read_str(data, q) if IMAGE_BASE <= q < IMAGE_BASE + len(data) else None
                    if ptr_str is not None:
                        parts.append(f'-> 0x{target:x} = ptr 0x{q:x} "{ptr_str}"')
                    else:
                        parts.append(f"-> 0x{target:x} (= 0x{q:x})")
                else:
                    parts.append(f"-> 0x{target:x}")
    return "   ; " + "  ".join(parts) if parts else ""


def main() -> None:
    _bootstrap()
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("va", nargs="?", help="function VA, e.g. 0x1408d1e30")
    ap.add_argument("--at", help="just print the string at this VA and exit")
    ap.add_argument("--bytes", default="0x400", help="max bytes to decode (default 0x400)")
    ap.add_argument("--image", choices=sorted(IMAGES), default="deobf")
    ap.add_argument("--calls", action="store_true", help="only print CALL/LEA/JMP lines")
    args = ap.parse_args()

    data = load_image(args.image)

    if args.at:
        va = int(args.at, 0)
        print(f"0x{va:x}: {read_str(data, va, limit=400)!r}")
        return
    if not args.va:
        ap.error("va required (or --at)")

    start = int(args.va, 0)
    nbytes = int(args.bytes, 0)
    off = start - IMAGE_BASE
    if off < 0 or off + nbytes > len(data):
        sys.exit(f"VA 0x{start:x} outside {IMAGES[args.image]}")

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    for insn in md.disasm(data[off : off + nbytes], start):
        note = annotate(data, insn)
        if args.calls and not (
            insn.mnemonic.startswith(("call", "jmp", "lea")) or note
        ):
            continue
        print(f"0x{insn.address:x}  {insn.mnemonic:<7} {insn.op_str}{note}")
        if insn.mnemonic == "ret":
            break


if __name__ == "__main__":
    main()
