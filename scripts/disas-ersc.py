#!/usr/bin/env python3
"""Disassemble a window of the installed Seamless Co-op `ersc.dll` around given rvas.

ersc.dll is a normal PE, not a flat de-Arxan'd image, so file offset does not equal
rva and every lookup has to go through the section table -- the mistake this exists to
prevent. Function bounds come from `.pdata`, so a return address recovered from a crash
log resolves to the whole function that owns it.

    uv run --with capstone python3 scripts/disas-ersc.py 0x2591f 0xf8e7e 0x158827
    uv run --with capstone python3 scripts/disas-ersc.py --before 0x200 --after 0x40 0x2591f

`--raw` decodes from the address itself instead of refusing it. Not every function has a `.pdata`
entry -- Seamless\'s small option predicates are `endbr64`, four instructions and a `ret`, with no
unwind info at all -- and without this they cannot be read here.

    uv run --with capstone python3 scripts/disas-ersc.py --raw --after 0x40 0x26b40

Whole-module questions ("who calls this", "who touches this field") live in the companion
`scripts/ersc-xrefs.py`, which walks every function once and keeps the result.
"""

from __future__ import annotations

import argparse
import bisect
import os
import struct
import sys

DEFAULT_DLL = os.environ.get(
    "ER_ERSC_DLL",
    "/home/banon/.local/share/Steam/steamapps/common/ELDEN RING/Game/SeamlessCoop/ersc.dll",
)


class Image:
    def __init__(self, path: str) -> None:
        self.data = open(path, "rb").read()
        d = self.data
        pe = struct.unpack_from("<I", d, 0x3C)[0]
        nsec = struct.unpack_from("<H", d, pe + 6)[0]
        optsize = struct.unpack_from("<H", d, pe + 20)[0]
        self.base = struct.unpack_from("<Q", d, pe + 24 + 24)[0]
        sect = pe + 24 + optsize
        self.secs = []
        for i in range(nsec):
            o = sect + i * 40
            name = d[o : o + 8].rstrip(b"\0").decode(errors="replace")
            vs, va, rs, rp = struct.unpack_from("<IIII", d, o + 8)
            self.secs.append((name, va, vs, rp, rs))
        pd_rva, pd_size = struct.unpack_from("<II", d, pe + 24 + 112 + 3 * 8)
        self.funcs = []
        po = self.off(pd_rva)
        for i in range(pd_size // 12):
            s, e, _u = struct.unpack_from("<III", d, po + i * 12)
            if s == 0:
                break
            self.funcs.append((s, e))
        self.funcs.sort()
        self.starts = [s for s, _ in self.funcs]
        self._overlapping = None

    def off(self, rva: int):
        for _n, va, vs, rp, rs in self.secs:
            if va <= rva < va + max(vs, rs):
                return rp + (rva - va)
        return None

    def section_of(self, rva: int):
        for n, va, vs, rp, rs in self.secs:
            if va <= rva < va + max(vs, rs):
                return n
        return None

    def entry_is_sound(self, fn) -> bool:
        """Whether this `.pdata` entry is disjoint from every other one.

        A well-formed exception directory partitions the code: no two `RUNTIME_FUNCTION`
        ranges overlap. The obfuscated `ERSC` section breaks that -- 7591 entries, one of
        them spanning `0x240000..0x333fd1`, swallowing the rest. `fn_of` still returns the
        swallowing entry, so `--whole` prints a confident boundary that is a fiction. An
        entry that overlaps a neighbour is not a boundary and this reports it as one.
        """
        if self._overlapping is None:
            bad = set()
            reach = 0
            prev = None
            for start, end in self.funcs:
                if start < reach:
                    bad.add((start, end))
                    if prev is not None:
                        bad.add(prev)
                if end > reach:
                    reach, prev = end, (start, end)
            self._overlapping = bad
        return fn not in self._overlapping

    def fn_of(self, rva: int):
        i = bisect.bisect_right(self.starts, rva) - 1
        if i >= 0 and self.funcs[i][0] <= rva < self.funcs[i][1]:
            return self.funcs[i]
        return None


def _iter_code(img, md):
    """Yield `(fn_lo, fn_hi, insn)` for every instruction in every function `.pdata` declares."""
    for lo, hi in img.funcs:
        o = img.off(lo)
        if o is None:
            continue
        for ins in md.disasm(img.data[o : o + (hi - lo)], img.base + lo):
            yield lo, hi, ins


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("rvas", nargs="*")
    ap.add_argument("--dll", default=DEFAULT_DLL)
    ap.add_argument("--before", default="0xc0")
    ap.add_argument("--after", default="0x30")
    ap.add_argument("--whole", action="store_true", help="print the whole owning function")
    ap.add_argument(
        "--raw",
        action="store_true",
        help="decode from the rva itself, ignoring `.pdata` -- for a target with no entry",
    )
    a = ap.parse_args()

    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    img = Image(a.dll)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = False
    before, after = int(a.before, 0), int(a.after, 0)
    refused = 0


    for raw in a.rvas:
        rva = int(raw, 0)
        fn = img.fn_of(rva)
        if a.raw:
            o = img.off(rva)
            n = min(after, 0x400)
            print(f"\n===== raw {rva:#x} (section {img.section_of(rva)}, va {img.base + rva:#x}) =====")
            for ins in md.disasm(img.data[o : o + n], img.base + rva):
                print(f"    {ins.address - img.base:#08x}  {ins.mnemonic:<9} {ins.op_str}")
            continue
        if fn is not None and not img.entry_is_sound(fn):
            print(
                f"\n===== {rva:#x} refused: its .pdata entry {fn[0]:#x}..{fn[1]:#x} "
                f"(span {fn[1] - fn[0]:#x}) overlaps others =====\n"
                f"      Section {img.section_of(rva)}. A well-formed exception directory has disjoint\n"
                f"      ranges; this one does not, so the boundary is a fiction and `--whole` would\n"
                f"      print a function that is not there. Decode from the address itself:\n"
                f"        uv run --with capstone python3 scripts/disas-ersc.py --raw --after 0x40 {rva:#x}\n"
                f"      and align the start by hand -- the mutated section desynchronises linear decode."
            )
            refused += 1
            continue
        if fn is None:
            print(f"\n===== {rva:#x} has no .pdata entry (section {img.section_of(rva)}) =====")
            continue
        lo, hi = fn
        print(
            f"\n===== rva {rva:#x} -> function {lo:#x}..{hi:#x} "
            f"(size {hi - lo:#x}, va {img.base + lo:#x}) ====="
        )
        start = lo if a.whole else max(lo, rva - before)
        end = hi if a.whole else min(hi, rva + after)
        o = img.off(start)
        for ins in md.disasm(img.data[o : o + (end - start)], img.base + start):
            r = ins.address - img.base
            print(f"{'==>' if r == rva else '   '} {r:#08x}  {ins.mnemonic:<9} {ins.op_str}")
    return 2 if refused else 0


if __name__ == "__main__":
    sys.exit(main())
