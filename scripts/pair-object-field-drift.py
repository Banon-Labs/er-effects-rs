#!/usr/bin/env python3
"""Align one function's body across the two de-Arxan'd ELDEN RING images and report field moves.

Why a whole-function alignment rather than a displacement census
----------------------------------------------------------------
A census ("which offsets does the image read off this object") cannot say which field lives at an
offset, and it cannot see a move at all when both the old and the new offset happen to be read
somewhere. What it can see is coincidence.

Aligning one function's two bodies is stronger and cheaper. If the instruction sequences agree
except for memory displacements, the code did not change -- so instruction k in 1.16.2 and
instruction k in 1.17 are the same access to the same field, and any displacement difference is
that field moving, by exactly that much. The alignment is done with difflib over
mnemonic+operand-shape (every numeric literal masked), so an inserted or deleted instruction shows
up as an insert/delete block instead of desynchronising every pair after it -- which is what makes
a 4-byte field insertion visible as a new store rather than as noise.

Usage
    scripts/pair-object-field-drift.py --pair 0x14025d580:1199 0x14025d550:1230 --label PGD::ctor
    scripts/pair-object-field-drift.py --pair 0x14025f5f0:24 0x14025f5d0:24 --base rcx
    scripts/pair-object-field-drift.py --selftest

`--base` restricts reported displacements to memory operands on the named register(s), which is
how you say "only count accesses through `this`". Without it every register base is reported and
the caller must judge. Both images are flat: file offset == RVA, VA = 0x140000000 + offset.
"""

import argparse
import difflib
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
IMAGE_1162 = os.path.join(ROOT, "eldenring-deobf.bin")
IMAGE_1170 = os.path.join(ROOT, "eldenring-deobf-1.17.bin")
BASE = 0x140000000
# Frame and instruction-pointer bases are never game-object fields.
NON_FIELD_BASES = ("rip", "rsp", "rbp", "esp", "ebp")
# GetScadutreeBlessing: the one 1.16.2 -> 1.17 field move established independently of this tool
# (scripts/map-rvas-1162-to-1170.py::KNOWN_MAPPINGS carries the same pair as ground truth), so it
# is the selftest's positive control.
SELFTEST_PAIR = (0x14025F5F0, 24, 0x14025F5D0, 24)
SELFTEST_EXPECTED = {0xAB5: 0xABD, 0xAB4: 0xABC, 0xFC: 0xFC}


def _capstone():
    try:
        import capstone  # noqa: F401
    except ImportError:
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3"] + sys.argv)
    import capstone

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    return capstone, md


def decode(capstone, md, image, rva, length):
    return list(md.disasm(image[rva : rva + length], BASE + rva))


def shape(insn):
    """Mnemonic plus operand shape with every numeric literal masked out."""
    return insn.mnemonic + " " + re.sub(r"0x[0-9a-f]+", "#", insn.op_str)


def field_disps(capstone, insn, bases):
    """`(base_register, displacement)` for each memory operand that could be an object field."""
    out = []
    for op in insn.operands:
        if op.type != capstone.x86.X86_OP_MEM or op.mem.base == 0:
            continue
        name = insn.reg_name(op.mem.base)
        if name in NON_FIELD_BASES:
            continue
        if bases and name not in bases:
            continue
        out.append((name, op.mem.disp))
    return out


_IMAGE_CACHE = {}


def image(path):
    """The 98 MB flat images, read once. A caller that aligns many pairs re-reads otherwise."""
    if path not in _IMAGE_CACHE:
        with open(path, "rb") as handle:
            _IMAGE_CACHE[path] = handle.read()
    return _IMAGE_CACHE[path]


def compare(capstone, md, rva16, len16, rva17, len17, bases, label, quiet=False):
    """Align the two bodies; return `(pairs, inserts, deletes, replaces)`."""
    img16 = image(IMAGE_1162)
    img17 = image(IMAGE_1170)
    a = decode(capstone, md, img16, rva16, len16)
    b = decode(capstone, md, img17, rva17, len17)
    sa = [shape(i) for i in a]
    sb = [shape(i) for i in b]
    opcodes = difflib.SequenceMatcher(a=sa, b=sb, autojunk=False).get_opcodes()
    pairs, inserts, deletes, replaces = [], [], [], []
    for tag, i1, i2, j1, j2 in opcodes:
        if tag == "equal":
            for k in range(i2 - i1):
                ia, ib = a[i1 + k], b[j1 + k]
                da = field_disps(capstone, ia, bases)
                db = field_disps(capstone, ib, bases)
                if len(da) != len(db):
                    continue
                for (ra, va), (rb, vb) in zip(da, db):
                    if ra == rb:
                        pairs.append((va, vb, ia.address, ib.address, f"{ia.mnemonic} {ia.op_str}"))
        elif tag == "insert":
            inserts += [(b[k].address, f"{b[k].mnemonic} {b[k].op_str}") for k in range(j1, j2)]
        elif tag == "delete":
            deletes += [(a[k].address, f"{a[k].mnemonic} {a[k].op_str}") for k in range(i1, i2)]
        else:
            replaces.append(
                (
                    [f"{a[k].mnemonic} {a[k].op_str}" for k in range(i1, i2)],
                    [f"{b[k].mnemonic} {b[k].op_str}" for k in range(j1, j2)],
                )
            )
    if not quiet:
        equal = sum(i2 - i1 for t, i1, i2, _, _ in opcodes if t == "equal")
        print(f"=== {label}  1.16.2 {rva16 + BASE:#x} ({len(a)} insn)  1.17 {rva17 + BASE:#x} ({len(b)} insn)")
        print(f"    aligned-identical instructions: {equal}")
        for addr, text in inserts:
            print(f"    INSERTED IN 1.17   {addr:#x}  {text}")
        for addr, text in deletes:
            print(f"    ABSENT FROM 1.17   {addr:#x}  {text}")
        for old, new in replaces:
            print(f"    REPLACED  16: {old}")
            print(f"              17: {new}")
        moved = sorted({(o, n) for o, n, _, _, _ in pairs if o != n})
        held = sorted({o for o, n, _, _, _ in pairs if o == n})
        print(f"    HELD  ({len(held)}): {', '.join(hex(v) for v in held)}")
        print(f"    MOVED ({len(moved)}): {', '.join(f'{o:#x}->{n:#x} (+{n - o:#x})' for o, n in moved)}")
    return pairs, inserts, deletes, replaces


def selftest(capstone, md):
    rva16, len16, rva17, len17 = SELFTEST_PAIR
    pairs, _, _, _ = compare(
        capstone, md, rva16 - BASE, len16, rva17 - BASE, len17, ("rcx",), "selftest", quiet=True
    )
    seen = {old: new for old, new, _, _, _ in pairs}
    missing = {k: v for k, v in SELFTEST_EXPECTED.items() if seen.get(k) != v}
    if missing:
        print(f"SELFTEST FAILED: GetScadutreeBlessing did not reproduce {missing}; measured {seen}")
        return 1
    print(f"selftest ok: GetScadutreeBlessing reproduces {len(SELFTEST_EXPECTED)} known field pairs")
    return 0


def parse_pair(text):
    addr, _, length = text.partition(":")
    return int(addr, 16) - BASE, int(length, 0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pair", nargs=2, metavar=("VA162:LEN", "VA170:LEN"))
    ap.add_argument("--base", action="append", default=[], help="restrict to this base register")
    ap.add_argument("--label", default="pair")
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    capstone, md = _capstone()
    if args.selftest:
        return selftest(capstone, md)
    if not args.pair:
        ap.error("--pair is required unless --selftest")
    rva16, len16 = parse_pair(args.pair[0])
    rva17, len17 = parse_pair(args.pair[1])
    compare(capstone, md, rva16, len16, rva17, len17, tuple(args.base), args.label)
    return 0


if __name__ == "__main__":
    sys.exit(main())
