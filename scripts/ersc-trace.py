#!/usr/bin/env python3
"""Follow control flow through the obfuscated `ERSC` section of `ersc.dll`.

The installed Seamless Co-op DLL keeps its `.text` intact but also carries an 11 MB
`ERSC` section holding mutated copies of some functions: real instructions chained by
unconditional jumps and padded with dead arithmetic and `push`/`pop` churn. A linear
disassembly from a `.pdata` start desyncs immediately, so this walks one instruction at
a time, follows every unconditional `jmp <imm>`, and prints what survives.

    uv run --with capstone python3 scripts/ersc-trace.py 0x2a3353 --steps 400
    uv run --with capstone python3 scripts/ersc-trace.py 0x2a3353 --all      # keep junk
    uv run --with capstone python3 scripts/ersc-trace.py 0x2a3353 --refs     # summary only

Junk suppression is a display filter only; `--all` shows every decoded instruction.

`--refs` prints no instructions at all. It walks the same path and reports only what a
mutated copy cannot hide: the direct `call` targets and the `rip`-relative operands it
reaches. Those land in the untouched `.text` and `.rdata`, so they identify which
original function this copy was made from -- which is how a frame in `ERSC` gets a name
when its `.pdata` entry is a fiction.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("disas_ersc", os.path.join(_HERE, "disas-ersc.py"))
_disas = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_disas)
Image = _disas.Image
DEFAULT_DLL = _disas.DEFAULT_DLL

# Instructions the mutator sprinkles between the real ones. Kept as a display filter so
# that hiding them can never change which addresses the walk visits.
JUNK = {"push", "pop", "nop", "test", "cmp", "clc", "stc", "cmc", "bt", "lahf", "sahf"}


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("rvas", nargs="+")
    ap.add_argument("--dll", default=DEFAULT_DLL)
    ap.add_argument("--steps", type=int, default=300)
    ap.add_argument("--all", action="store_true", help="print junk instructions too")
    ap.add_argument(
        "--refs",
        action="store_true",
        help="report only the calls and rip-relative operands the walk reaches",
    )
    a = ap.parse_args()

    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    img = Image(a.dll)
    md = Cs(CS_ARCH_X86, CS_MODE_64)

    for raw in a.rvas:
        rva = int(raw, 0)
        print(f"\n===== trace from {rva:#x} ({img.section_of(rva)}) =====")
        cur = rva
        seen = set()
        calls, refs = [], []
        for _ in range(a.steps):
            if cur in seen:
                print(f"   {cur:#08x}  <loop>")
                break
            seen.add(cur)
            o = img.off(cur)
            if o is None:
                print(f"   {cur:#08x}  <unmapped>")
                break
            ins = next(md.disasm(img.data[o : o + 16], img.base + cur), None)
            if ins is None:
                print(f"   {cur:#08x}  <undecodable {img.data[o:o+8].hex()}>")
                break
            mn, ops = ins.mnemonic, ins.op_str
            if mn == "call":
                tgt = int(ops, 0) - img.base if ops.startswith("0x") else None
                calls.append((cur, tgt, ops))
            if "rip +" in ops or "rip -" in ops:
                i = ops.find("rip ")
                j = ops.find("]", i)
                try:
                    disp = int(ops[i + 4 : j].replace(" ", ""), 0)
                except ValueError:
                    disp = None
                if disp is not None:
                    refs.append((cur, (ins.address + ins.size + disp) - img.base, f"{mn} {ops}"))
            show = (a.all or mn not in JUNK) and not a.refs
            if mn == "jmp" and ops.startswith("0x"):
                nxt = int(ops, 0) - img.base
                if img.off(nxt) is None:
                    print(f"   {cur:#08x}  jmp       {ops}  <unmapped, stop>")
                    break
                cur = nxt
                continue
            if show:
                sec = img.section_of(cur)
                mark = "" if sec == "ERSC" else f"  [{sec}]"
                print(f"   {cur:#08x}  {mn:<9} {ops}{mark}")
            if mn in ("ret", "int3") or (mn == "jmp" and not ops.startswith("0x")):
                break
            cur += ins.size
        if a.refs:
            print(f"  -- {len(calls)} calls --")
            for site, tgt, ops in calls:
                where = f"{tgt:#x} [{img.section_of(tgt)}]" if tgt is not None else f"indirect {ops}"
                print(f"   {site:#08x}  call {where}")
            print(f"  -- {len(refs)} rip-relative operands --")
            for site, tgt, text in refs:
                print(f"   {site:#08x}  -> {tgt:#08x} [{img.section_of(tgt)}]  {text}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
