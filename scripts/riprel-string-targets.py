#!/usr/bin/env python3
"""Print the UTF-16 literal each RIP-relative `lea` inside a function points at.

Why this is evidence
--------------------
A masked whole-body signature has to wildcard RIP-relative displacements, because the data those
`lea`s point at moved when the image was relaid out. That is exactly the wrong thing to throw away
when the data is a STRING: `L"?WeaponName?"` occurs once in each image and is a property of the C++
source, not of the layout. Reading the displacement back and dereferencing it turns a wildcard into
the single strongest fact available about a non-virtual function -- the same class of evidence as
RTTI, for the same reason.

So: after a masked match pairs two addresses, run this on both and check the strings agree. If the
1.17 candidate's `lea` lands on a different literal, the match is an impostor no matter how many
instructions verified.

Usage
    uv run --with capstone python3 scripts/riprel-string-targets.py 1162:0x140d0fda0 1170:0x140d11470
"""

import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}

try:
    from capstone import CS_ARCH_X86, CS_MODE_64, CS_OP_MEM, Cs, x86_const
except ImportError:  # provision capstone ephemerally, as the repo's other tools do
    os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])


def sections(data):
    lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    nsec = struct.unpack_from("<H", data, lfanew + 6)[0]
    opt = struct.unpack_from("<H", data, lfanew + 20)[0]
    table = lfanew + 24 + opt
    out = []
    for index in range(nsec):
        entry = table + index * 40
        name = data[entry : entry + 8].rstrip(b"\0").decode("ascii", "replace")
        vsize, va, _rsize, _rptr = struct.unpack_from("<IIII", data, entry + 8)
        out.append((name, va, vsize))
    return out


def pdata_extent(data, rva):
    for name, va, vsize in sections(data):
        if name != ".pdata":
            continue
        for index in range(vsize // 12):
            begin, end, _unwind = struct.unpack_from("<III", data, va + index * 12)
            if begin == rva:
                return begin, end
    return None


def read_utf16(data, rva, limit=120):
    out = []
    for step in range(limit):
        offset = rva + step * 2
        unit = struct.unpack_from("<H", data, offset)[0]
        if unit == 0:
            break
        out.append(chr(unit))
    return "".join(out)


def read_ascii(data, rva, limit=120):
    out = []
    for step in range(limit):
        byte = data[rva + step]
        if byte == 0:
            break
        if byte < 0x20 or byte > 0x7E:
            return None
        out.append(chr(byte))
    return "".join(out)


def main(argv):
    if not argv:
        sys.exit(__doc__)
    machine = Cs(CS_ARCH_X86, CS_MODE_64)
    machine.detail = True
    for spec in argv:
        build, _, text = spec.partition(":")
        va = int(text, 0)
        rva = va - BASE
        data = open(IMAGES[build], "rb").read()
        extent = pdata_extent(data, rva)
        if extent is None:
            end = rva + 0x100
            note = " (no .pdata entry; decoding 0x100 bytes)"
        else:
            end = extent[1]
            note = ""
        print(f"{build} {va:#x} .. {BASE + end:#x}{note}")
        body = data[rva:end]
        for insn in machine.disasm(body, va):
            riprel = any(
                operand.type == CS_OP_MEM and operand.mem.base == x86_const.X86_REG_RIP
                for operand in insn.operands
            )
            if not riprel:
                continue
            offset_in_fn = insn.address - va
            target = insn.address + insn.size + insn.disp
            trva = target - BASE
            wide = read_utf16(data, trva)
            narrow = read_ascii(data, trva)
            rendered = f'L"{wide}"' if wide else (f'"{narrow}"' if narrow else "<not a string>")
            print(
                f"    +{offset_in_fn:#06x}  {insn.mnemonic} {insn.op_str}"
                f"   -> {target:#x}  {rendered}"
            )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
