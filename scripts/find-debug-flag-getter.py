#!/usr/bin/env python3
"""Find the global byte behind a FromSoft `Game.Debug.*` accessor, from its caller.

Why this exists
---------------
FromSoft's debug switches compile to a three-instruction stub:

    movzx eax, byte ptr [rip + disp32]     ; the flag
    lea   rsp, [rsp + 8]
    jmp   qword ptr [rsp - 8]

The stub is byte-identical between builds except for that one relocated displacement, so
`scripts/map-rvas-1162-to-1170.py` cannot map it -- it reported *52* shape matches for
`Game.Debug.IsEnableControlOnDisactiveWindow` and refused to pick one, which is the correct
refusal and also a dead end. The caller is not ambiguous, though: a 3 KB function maps
uniquely, and `docs/recon/rva-map-1162-to-1170.functions.tsv` already carries the pair.

So: decode the caller, follow each `call rel32` (through a one-instruction `jmp rel32`
thunk, which is how these accessors are reached), and report every callee that is such a
stub together with the absolute VA of the flag byte it reads. Run it on both images and the
1.16.2 answer identifies which stub is which, while the 1.17 answer is the address to use.

Usage
    uv run --with capstone python3 scripts/find-debug-flag-getter.py 0x140e33aa0
    uv run --with capstone python3 scripts/find-debug-flag-getter.py 0x140e358a0 \\
        --image eldenring-deobf-1.17.bin
"""

from __future__ import annotations

import argparse
import pathlib
import sys

REPO_ROOT = pathlib.Path(__file__).resolve().parent.parent
IMAGE_BASE = 0x140000000
# A caller big enough to hold several accessor calls; decoding stops at the first RET/INT3 run
# anyway, so this is only an upper bound.
MAX_FUNCTION_BYTES = 0x2000
STUB_MAX_INSNS = 4


def _va_to_offset(va: int) -> int:
    return va - IMAGE_BASE


def _decode_stub(decoder, data: bytes, va: int):
    """Return the absolute VA of the byte a `Game.Debug.*` stub reads, or None."""
    import capstone

    offset = _va_to_offset(va)
    if not 0 <= offset < len(data):
        return None
    insns = list(decoder.disasm(data[offset : offset + 32], va, count=STUB_MAX_INSNS))
    if not insns:
        return None
    # A one-instruction thunk: `jmp rel32` straight into the real stub.
    if insns[0].mnemonic == "jmp" and len(insns[0].operands) == 1:
        target = insns[0].operands[0]
        if target.type == capstone.x86.X86_OP_IMM:
            return _decode_stub(decoder, data, target.imm)
    first = insns[0]
    if first.mnemonic != "movzx":
        return None
    for op in first.operands:
        if op.type == capstone.x86.X86_OP_MEM and op.mem.base == capstone.x86.X86_REG_RIP:
            return first.address + first.size + op.mem.disp
    return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("caller", help="VA of the calling function, e.g. 0x140e33aa0")
    parser.add_argument("--image", default="eldenring-deobf.bin", help="flat de-Arxan'd image")
    args = parser.parse_args()

    try:
        import capstone
    except ImportError:
        print(
            "capstone is not importable. There is no system pip; run this under uv:\n"
            f"    uv run --with capstone python3 scripts/{pathlib.Path(__file__).name} ...",
            file=sys.stderr,
        )
        return 2

    image_path = pathlib.Path(args.image)
    if not image_path.is_absolute():
        image_path = REPO_ROOT / image_path
    data = image_path.read_bytes()

    decoder = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    decoder.detail = True

    caller = int(args.caller, 0)
    start = _va_to_offset(caller)
    print(f"image={image_path.name} caller=0x{caller:x}")
    found = 0
    for insn in decoder.disasm(data[start : start + MAX_FUNCTION_BYTES], caller):
        if insn.mnemonic != "call" or len(insn.operands) != 1:
            continue
        operand = insn.operands[0]
        if operand.type != capstone.x86.X86_OP_IMM:
            continue
        flag = _decode_stub(decoder, data, operand.imm)
        if flag is None:
            continue
        found += 1
        print(
            f"  call site 0x{insn.address:x} (+0x{insn.address - caller:x})"
            f"  -> stub 0x{operand.imm:x}  reads flag byte 0x{flag:x}"
            f"  (RVA 0x{flag - IMAGE_BASE:x}, current value 0x{data[_va_to_offset(flag)]:02x})"
        )
    if not found:
        print("  no Game.Debug.* accessor stub reached from this function")
    return 0


if __name__ == "__main__":
    sys.exit(main())
