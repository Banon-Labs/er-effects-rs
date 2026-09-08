#!/usr/bin/env python3
"""Locate a 1.16.2 function in the 1.17 image by masking its entire declared body.

Why a whole-body signature, when a prologue signature already exists
--------------------------------------------------------------------
`scripts/map-rvas-1162-to-1170.py` searches a short window at the function's entry. That window is
where MSVC's boilerplate lives -- `mov [rsp+8], rbx; push rdi; sub rsp, 0x20` opens thousands of
functions -- so its answer is routinely "9 shape matches, no anchor nearby" and the address is
recorded unresolved. Seven of `er-build-import-runtime`'s thirty-seven addresses failed exactly
that way, including all six `MsgRepositoryImp::Get*Name` getters, and being unresolved is what made
"Load Build from URL" inert on 1.17.

The body is where a function is actually distinctive, and for these getters the distinguishing
bytes are data CONSTANTS: each one passes its own FMG category ids (`0x73`, `0x136`, `0x19a` for
weapons) to `MsgRepositoryImp::LookupEntry`. Those ids are properties of the game's message
archive, not of the code layout, so a patch does not renumber them. Masking only the operands that
a relayout is forced to re-encode -- `call`/`jmp` rel32 and RIP-relative disp32 -- and keeping every
other byte literal turns the whole body into a signature that carries those constants.

What a hit does and does not prove
----------------------------------
One hit in 1.17 and one hit in 1.16.2 (the original itself) means the pattern is unique in both
images: no other function in 1.17 has this body. That is strong. It is not sufficient on its own in
two situations, and the tool reports both rather than hiding them:

  * Self-hits > 1 -- the function has a byte-identical TWIN. `GetGemName` does: 1.16.2 carries two
    copies 0x60 apart that COMDAT folding did not merge, and 1.17 carries the same two. Byte
    evidence is structurally incapable of choosing between them, because their being identical is
    the premise. Resolve those with `scripts/refs-to-va-1162-1170.py` and pair by reference
    topology, never by bytes.
  * Zero hits -- the body genuinely changed. Fall back to caller votes or bracketing.

Every masked window is reported by byte offset from the function's entry, never by instruction
index, so the evidence survives 1.17 inserting an instruction ahead of it.

The extent comes from each image's own `.pdata`, so it is FromSoftware's declaration of where the
function ends, not a decode that might stop early at a tail call. It is taken through
`verify-rva-map-1170.py`'s own `function_regions`, imported rather than reimplemented, because a
`.pdata` table holds one record per region and MSVC splits functions into chunks: reading the first
record alone gives 0x33 bytes of `GetSlotIndexByItemIndex` where the run is 0xdf, and a signature
built from a fifth of a body while the docstring says "whole" is precisely the false coverage claim
this tool exists to avoid making. Sharing the implementation also means a later fix to chunk
handling cannot land in one tool and not the other.

Usage
    uv run --with capstone python3 scripts/whole-body-signature-1170.py 0x140d11370
    uv run --with capstone python3 scripts/whole-body-signature-1170.py --tsv out.tsv 0x1402470e0 ...
"""

import argparse
import os
import re
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
OLD_IMAGE = os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin"))
NEW_IMAGE = os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin"))

try:
    from capstone import CS_ARCH_X86, CS_MODE_64, CS_OP_MEM, Cs, x86_const
except ImportError:  # provision capstone ephemerally, as the repo's other tools do
    os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])


def load_verifier():
    """Import `verify-rva-map-1170.py` for its chunk-merging `.pdata` reader.

    The hyphenated filename cannot be `import`ed, and copying `function_regions` here would let the
    two drift -- which is how a chunked function comes to be measured two different ways by two
    tools that both claim to read the same table.
    """
    import importlib.util

    path = os.path.join(ROOT, "scripts", "verify-rva-map-1170.py")
    spec = importlib.util.spec_from_file_location("verify_rva_map_1170", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules["verify_rva_map_1170"] = module
    spec.loader.exec_module(module)
    return module


def mask_body(machine, body, start_rva):
    """Wildcard the operands a relayout must re-encode. Returns (tokens, masked_windows)."""
    tokens = ["%02x" % byte for byte in body]
    masked = []
    offset = 0
    for insn in machine.disasm(bytes(body), BASE + start_rva):
        if insn.group(x86_const.X86_GRP_JUMP) or insn.group(x86_const.X86_GRP_CALL):
            imm_offset = getattr(insn, "imm_offset", 0)
            imm_size = getattr(insn, "imm_size", 0)
            if imm_size:
                for step in range(imm_size):
                    tokens[offset + imm_offset + step] = "??"
                masked.append((offset + imm_offset, imm_size, "branch", insn.mnemonic))
        riprel = any(
            operand.type == CS_OP_MEM and operand.mem.base == x86_const.X86_REG_RIP
            for operand in insn.operands
        )
        disp_offset = getattr(insn, "disp_offset", 0)
        disp_size = getattr(insn, "disp_size", 0)
        if riprel and disp_size:
            for step in range(disp_size):
                tokens[offset + disp_offset + step] = "??"
            masked.append((offset + disp_offset, disp_size, "riprel", insn.mnemonic))
        offset += insn.size
        if offset >= len(body):
            break
    return tokens, masked


def compile_tokens(tokens):
    return re.compile(
        b"".join(b"." if t == "??" else re.escape(bytes([int(t, 16)])) for t in tokens),
        re.S,
    )


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("vas", nargs="+", help="1.16.2 VAs")
    parser.add_argument("--tsv", help="also write machine-readable results here")
    parser.add_argument("--quiet-windows", action="store_true", help="omit the masked-window list")
    args = parser.parse_args(argv)

    old = open(OLD_IMAGE, "rb").read()
    new = open(NEW_IMAGE, "rb").read()
    verifier = load_verifier()
    old_extents, _old_starts = verifier.function_regions(old)
    machine = Cs(CS_ARCH_X86, CS_MODE_64)
    machine.detail = True

    rows = []
    for text in args.vas:
        va = int(text, 0)
        rva = va - BASE
        end = old_extents.get(rva)
        if end is None:
            print(f"{va:#x}  NO .pdata ENTRY in 1.16.2 (leaf or thunk) -- use pair-leaf-functions")
            rows.append((va, None, None, "NO-PDATA"))
            continue
        begin = rva
        tokens, masked = mask_body(machine, old[begin:end], begin)
        pattern = compile_tokens(tokens)
        self_hits = [m.start() for m in pattern.finditer(old)]
        hits = [m.start() for m in pattern.finditer(new)]
        masked_bytes = sum(entry[1] for entry in masked)
        if len(hits) == 1 and len(self_hits) == 1:
            status = "UNIQUE-BOTH"
        elif len(hits) == 1:
            status = f"UNIQUE-1170-BUT-{len(self_hits)}-TWINS-IN-1162"
        elif not hits:
            status = "NO-HIT"
        else:
            status = f"AMBIGUOUS-{len(hits)}"
        new_va = BASE + hits[0] if len(hits) == 1 else None
        delta = new_va - va if new_va is not None else None
        print(
            f"{va:#x}  len={end - begin:#x}  literal={end - begin - masked_bytes}B "
            f"masked={masked_bytes}B  {status}"
        )
        print(f"    1.16.2 hits: {[hex(BASE + h) for h in self_hits]}")
        print(f"    1.17   hits: {[hex(BASE + h) for h in hits[:8]]}")
        if delta is not None:
            print(f"    -> {new_va:#x}  delta {delta:+#x}")
        if masked and not args.quiet_windows:
            rendered = ", ".join(
                f"+{off:#x}/{size}({kind})" for off, size, kind, _m in masked
            )
            print(f"    masked windows (byte offsets): {rendered}")
        rows.append((va, new_va, delta, status))

    if args.tsv:
        with open(args.tsv, "w", encoding="utf-8") as handle:
            handle.write("# 1.16.2 VA\t1.17 VA\tdelta\tstatus\n")
            for va, new_va, delta, status in rows:
                if new_va is None:
                    handle.write(f"{va:#x}\t-\t-\t{status}\n")
                else:
                    handle.write(f"{va:#x}\t{new_va:#x}\t{delta:+#x}\t{status}\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
