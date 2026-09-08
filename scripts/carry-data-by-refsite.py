#!/usr/bin/env python3
"""Carry a 1.16.2 data address onto 1.17 by the byte offset of its reference inside a mapped
function -- the fallback for when the instruction index has shifted.

What this fixes
---------------
`map-data-rvas-1162-to-1170.py` locates a reference by counting instructions from the function
entry, then reads the displacement of the instruction at the same index in the 1.17 function.
That is exact when the two bodies decode to the same instruction stream and useless the moment
one extra instruction is inserted ahead of the reference: the index then names a different
instruction and the tool reports `no displacement there`, which is how a global with a perfectly
good reference ends up in the unused list.

Measured case: `MENU_PUMP_KICK_PTR_RVA 0x3b37c98`.  Its single reference sits at 1.16.2 `0x9b2c59`
inside `0x9b24e0`, and the function map pairs `0x9b24e0 -> 0x9b3730`.  By index (#485) the 1.17
instruction has no displacement.  By byte offset the reference sits at `+0x779` in both bodies --
the same offset, the same mnemonic, the same operand shape -- and its 1.17 displacement reaches
`0x3b3bca8`.

Why byte offset is safe here and index is not
---------------------------------------------
Neither is safe alone, which is why this asks for agreement rather than trusting the offset:
the instruction found at the same byte offset must have the same MNEMONIC and the same OPERAND
shape as the 1.16.2 one, and it must be an instruction boundary in a decode started from the
function's own entry -- not a byte picked out of the middle of a longer instruction.  When those
hold, the two references are the same reference and its displacement is the answer.  When they
do not, this reports nothing.  It never falls back to a delta.
"""

from __future__ import annotations

import argparse
import importlib.util
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000


def load_tool():
    path = ROOT / "scripts" / "map-data-rvas-1162-to-1170.py"
    spec = importlib.util.spec_from_file_location("_mapdata", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def decode_at(md, image: bytes, func: int, end: int):
    return list(md.disasm_lite(image[func:end], BASE + func))


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rvas", nargs="+")
    ap.add_argument("--map", default="docs/recon/rva-map-1162-to-1170.functions.tsv")
    args = ap.parse_args()

    tool = load_tool()
    import capstone

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    old = tool.Image(ROOT / "eldenring-deobf.bin")
    new = tool.Image(ROOT / "eldenring-deobf-1.17.bin")
    fmap: dict[int, int] = {}
    for line in (ROOT / args.map).read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        a, b = line.split("\t")[:2]
        fmap[int(a, 16)] = int(b, 16)
    old_starts = old.function_starts()
    new_ends = {}
    va, size = new.pdata
    for off in range(va, va + size, 12):
        b0, b1, _ = struct.unpack_from("<III", new.data, off)
        if b0 and b1 > b0:
            new_ends.setdefault(b0, b1)
    old_ends = {}
    va, size = old.pdata
    for off in range(va, va + size, 12):
        b0, b1, _ = struct.unpack_from("<III", old.data, off)
        if b0 and b1 > b0:
            old_ends.setdefault(b0, b1)

    for text in args.rvas:
        target = int(text, 16)
        if target >= BASE:
            target -= BASE
        print(f"=== {target:#x}")
        sites = tool.references(old, target)
        if not sites:
            print("    no rip-relative reference site in 1.16.2 .text")
            continue
        for disp_at in sites:
            func = tool.enclosing(old_starts, disp_at)
            if func is None:
                print(f"    {disp_at:#x}: no enclosing function")
                continue
            nf = fmap.get(func)
            if nf is None:
                print(f"    {disp_at:#x}: in fn {func:#x}, NOT in the function map")
                continue
            a_end = old_ends.get(func, func + 0x4000)
            b_end = new_ends.get(nf, nf + 0x4000)
            da = decode_at(md, old.data, func, min(a_end, func + 0x8000))
            db = decode_at(md, new.data, nf, min(b_end, nf + 0x8000))
            hit = None
            for addr, sz, mn, op in da:
                start = addr - BASE
                if start <= disp_at < start + sz:
                    hit = (start - func, sz, mn, op)
                    break
            if hit is None:
                print(f"    {disp_at:#x}: not an instruction boundary in fn {func:#x}")
                continue
            off_in_fn, sz, mn, op = hit
            match = [x for x in db if x[0] - BASE - nf == off_in_fn]
            print(f"    {disp_at:#x}: fn {func:#x} -> {nf:#x}, reference at +{off_in_fn:#x}")
            print(f"        1.16.2  {mn} {op}")
            if not match:
                print("        1.17    NOT an instruction boundary at the same offset -> no answer")
                continue
            b_addr, b_sz, b_mn, b_op = match[0]
            print(f"        1.17    {b_mn} {b_op}   @{b_addr - BASE:#x}")
            if b_mn != mn:
                print("        MNEMONIC DIFFERS -> no answer")
                continue
            sa, ma = tool_shape(op)
            sb, mb = tool_shape(b_op)
            if sa != sb:
                print("        OPERAND SHAPE DIFFERS -> no answer")
                continue
            for (base_a, d_a), (base_b, d_b) in zip(ma, mb):
                if base_a == "rip":
                    src = disp_at  # sanity: recompute the 1.16.2 target
                    tgt_a = (hit[0] + func) + sz + d_a
                    tgt_b = (b_addr - BASE) + b_sz + d_b
                    ok = "" if tgt_a == target else f"  (1.16.2 recompute {tgt_a:#x} != {target:#x} -- SKIP)"
                    print(f"        ANSWER  {target:#x} -> {tgt_b:#x}   delta {tgt_b - target:+#x}{ok}")
    return 0


_dsfd = None


def tool_shape(op: str):
    global _dsfd
    if _dsfd is None:
        path = ROOT / "scripts" / "detect-struct-field-drift.py"
        spec = importlib.util.spec_from_file_location("_dsfd2", path)
        _dsfd = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(_dsfd)
    return _dsfd.split_memory(op)


if __name__ == "__main__":
    raise SystemExit(main())
