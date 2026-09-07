#!/usr/bin/env python3
"""Verify a 1.16.2 -> 1.17 pair whose function has no `.pdata` extent.

Why `verify-rva-map-1170.py` cannot do it
-----------------------------------------
That tool stops a decode at (1) the `.pdata` extent, (2) a `ret`, or (3) a `jmp` immediately
followed by an `int3`. A leaf THUNK satisfies none of them. MSVC emits no unwind data for a
function that neither allocates stack nor saves a register, so there is no extent; the thunk ends
in a tail call, so there is no `ret`; and in this de-Arxan'd image the alignment gap after the
tail call is not `int3` padding -- it is whatever the deobfuscator left there, which differs
between builds.

The decode therefore runs off the end, through the gap, into the next thunk and beyond, and the
first place the two builds' unrelated trailing bytes disagree is reported as the function
DIVERGING. That verdict is not merely useless: `er-game-base/build.rs::refuted_sources()` reads
`DIVERGES` as positive evidence the address is wrong and subtracts it from the call map too. So an
over-read on a thunk that did not change removes a working address.

Measured examples, both `CS::KnowledgeLoadingScreen` `_Func_impl` lambdas:

    0x14090a0a0 -> 0x14090b240   verifier: DIVERGES 0.86, first diff at insn 10
    0x14090a0c0 -> 0x14090b260   verifier: DIVERGES 0.75, first diff at insn 4

Both thunks are 23 bytes and differ in exactly four bytes: the `lea rdx,[rip+X]` displacement to a
`.rdata` label literal that moved, and the tail-call `rel32` to a callee that moved. Every diff the
verifier found was past byte 23.

What this does instead
----------------------
It bounds the decode by REACHABILITY, which is the property a `.pdata` extent stands in for: an
unconditional `jmp` ends the function unless something already decoded branches past it. Then it
compares the two bodies byte for byte with the relocation-sensitive operand fields (RIP-relative
displacements, branch targets) masked out, and reports which bytes remain different.

A clean result is stronger evidence than `IDENTICAL`, not weaker: nothing is normalised away
except the fields a patch is required to change. Immediates and struct displacements are compared
literally, so a retuned constant or a moved field shows up as a difference rather than vanishing.

Usage
    uv run --with capstone python3 scripts/verify-thunk-rva-1170.py 0x14090a0a0 0x14090b240
    uv run --with capstone python3 scripts/verify-thunk-rva-1170.py --pairs <file.tsv>
"""

import argparse
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OLD_IMAGE = os.path.join(ROOT, "eldenring-deobf.bin")
NEW_IMAGE = os.path.join(ROOT, "eldenring-deobf-1.17.bin")
BASE = 0x140000000
# No thunk this is meant for is anywhere near this long; the bound only stops a runaway decode.
MAX_BYTES = 0x200


def body(image, rva):
    """`(bytes, [(offset, size, [masked byte offsets])])` for the function at `rva`.

    The decode ends at the first `ret`, or at the first unconditional `jmp` that nothing decoded
    so far jumps past -- everything after such a `jmp` is unreachable from the entry, which is
    what "the function ends here" means.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs, x86_const

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    instructions = []
    furthest_branch = rva
    end = None
    for insn in md.disasm(bytes(image[rva : rva + MAX_BYTES]), rva):
        masked = []
        encoding = getattr(insn, "encoding", None)
        if encoding is not None:
            for at, size in (
                (encoding.disp_offset, encoding.disp_size),
                (encoding.imm_offset, encoding.imm_size),
            ):
                if not size:
                    continue
                # A displacement or immediate is only relocation-sensitive when it names an
                # ADDRESS: a RIP-relative memory reference or a branch target. A displacement on
                # a register base is a struct offset and must be compared literally -- that is
                # the difference between "the code moved" and "the struct changed".
                rip_relative = any(
                    operand.type == x86_const.X86_OP_MEM
                    and operand.mem.base == x86_const.X86_REG_RIP
                    for operand in insn.operands
                )
                branch = insn.group(x86_const.X86_GRP_JUMP) or insn.group(x86_const.X86_GRP_CALL)
                if (at == encoding.disp_offset and rip_relative) or (
                    at == encoding.imm_offset and branch
                ):
                    masked.extend(range(insn.address - rva + at, insn.address - rva + at + size))
        instructions.append((insn.address - rva, insn.size, masked))
        if insn.mnemonic == "ret":
            end = insn.address - rva + insn.size
            break
        # The reachability test must use branches decoded before this instruction. Folding the
        # tail call's own forward target into the reckoning is what made an early version run
        # 296 bytes past a 12-byte thunk: `jmp <helper>` jumps forward, so it appeared to keep
        # the following bytes reachable from itself.
        if insn.mnemonic == "jmp" and furthest_branch <= insn.address:
            end = insn.address - rva + insn.size
            break
        if insn.group(x86_const.X86_GRP_JUMP) or insn.group(x86_const.X86_GRP_CALL):
            for operand in insn.operands:
                if operand.type == x86_const.X86_OP_IMM:
                    furthest_branch = max(furthest_branch, operand.imm)
    if end is None:
        return None, None
    return bytes(image[rva : rva + end]), instructions


def verify(old_image, new_image, old_va, new_va):
    old_body, old_insns = body(old_image, old_va - BASE)
    new_body, new_insns = body(new_image, new_va - BASE)
    if old_body is None or new_body is None:
        return "NO-END", "the decode found no reachable end within the bound"
    if len(old_body) != len(new_body):
        return "LENGTH-DIFFERS", f"1.16.2 {len(old_body)} bytes vs 1.17 {len(new_body)} bytes"
    masked = {offset for _, _, fields in old_insns for offset in fields}
    masked &= {offset for _, _, fields in new_insns for offset in fields}
    differing = [i for i in range(len(old_body)) if old_body[i] != new_body[i]]
    unexplained = [i for i in differing if i not in masked]
    if unexplained:
        return "DIFFERS", (
            f"{len(old_body)} bytes, {len(unexplained)} differing outside the relocation fields "
            f"at {[hex(i) for i in unexplained[:8]]}"
        )
    if differing:
        return "RELOCATION-IDENTICAL", (
            f"{len(old_body)} bytes over {len(old_insns)} instructions; the only differing bytes "
            f"({[hex(i) for i in differing]}) are RIP-relative displacements and branch targets"
        )
    return "BYTE-IDENTICAL", f"{len(old_body)} bytes over {len(old_insns)} instructions"


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument("vas", nargs="*", help="pairs: <1.16.2 VA> <1.17 VA> ...")
    parser.add_argument("--pairs", help="TSV whose first two columns are the pair")
    arguments = parser.parse_args()

    try:
        import capstone  # noqa: F401
    except ImportError:
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

    pairs = []
    if arguments.pairs:
        for line in open(arguments.pairs, encoding="utf-8", errors="replace"):
            if line.startswith("#") or not line.strip():
                continue
            fields = line.split("\t")
            if len(fields) >= 2 and fields[1].strip().startswith("0x"):
                pairs.append((int(fields[0], 16), int(fields[1], 16)))
    for i in range(0, len(arguments.vas) - 1, 2):
        pairs.append((int(arguments.vas[i], 0), int(arguments.vas[i + 1], 0)))

    old_image = open(OLD_IMAGE, "rb").read()
    new_image = open(NEW_IMAGE, "rb").read()
    for old_va, new_va in pairs:
        verdict, note = verify(old_image, new_image, old_va, new_va)
        print(f"{old_va:#x} -> {new_va:#x}  {verdict:<21} {note}")


if __name__ == "__main__":
    main()
