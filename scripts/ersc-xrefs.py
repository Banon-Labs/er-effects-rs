#!/usr/bin/env python3
"""Call-graph and cross-reference scanner over the installed Seamless Co-op `ersc.dll`.

Companion to `scripts/disas-ersc.py`, which prints one window at a time. This walks
every function `.pdata` declares, decodes it, and records every direct `call`/`jmp`
whose target is an immediate, so a question like "who locks this mutex" is answered by
reading the binary rather than by guessing from one site.

    uv run --with capstone python3 scripts/ersc-xrefs.py --to 0xf9828
    uv run --with capstone python3 scripts/ersc-xrefs.py --to 0xf9828 --depth 2
    uv run --with capstone python3 scripts/ersc-xrefs.py --data-xrefs 0x1f2a30

`--to` lists direct callers. `--depth N` walks callers-of-callers N levels up.
`--data-xrefs` finds `lea reg,[rip+disp]` and `mov reg,[rip+disp]` operands that resolve
to an rva, which is how a function pointer handed to a hook installer is found.

    uv run --with capstone python3 scripts/ersc-xrefs.py --sweep-refs 0x258d0
    uv run --with capstone python3 scripts/ersc-xrefs.py --field 0x150
    uv run --with capstone python3 scripts/ersc-xrefs.py --field-values 0x150

`--sweep-refs` answers the same question as `--data-xrefs` without trusting instruction
boundaries: it reads every 4-byte window in the code and data sections as a displacement
and reports the ones that resolve to the target. That matters here because a linear
disassembly desynchronises on jump tables, and one such desync hid the only reference to
the option action at `0x258d0` -- a `lea rax,[rip-0xb716]` inside a `0x10939`-byte
function -- from both `--data-xrefs` and a `.pdata`-driven decode. A sweep hit is a
candidate, not a reference: confirm it by disassembling the three bytes before it.

`--field` reports every instruction whose memory operand is `[<base reg> + OFF]`, grouped
by opcode. `--field-values` reports the constants that field is written with or compared
against, following the value through the register a `mov` loads it into, which is how a
state machine written as `mov eax,[rdi+OFF]` then a chain of `cmp eax,imm` is recovered.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
from collections import defaultdict

_HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("disas_ersc", os.path.join(_HERE, "disas-ersc.py"))
_disas = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_disas)
Image = _disas.Image
DEFAULT_DLL = _disas.DEFAULT_DLL
_iter_code = _disas._iter_code


class Graph:
    """Every direct control transfer with an immediate target, keyed both ways."""

    def __init__(self, img: Image) -> None:
        from capstone import CS_ARCH_X86, CS_MODE_64, Cs

        md = Cs(CS_ARCH_X86, CS_MODE_64)
        md.detail = False
        self.img = img
        self.callers = defaultdict(list)  # target rva -> [(site rva, owning fn, mnemonic)]
        self.calls = defaultdict(list)  # owning fn -> [(site rva, target rva, mnemonic)]
        self.riprefs = defaultdict(list)  # referenced rva -> [(site rva, owning fn, text)]
        base = img.base
        for lo, hi in img.funcs:
            o = img.off(lo)
            if o is None or hi <= lo or hi - lo > 0x20000:
                continue
            for ins in md.disasm(img.data[o : o + (hi - lo)], base + lo):
                mn = ins.mnemonic
                ops = ins.op_str
                site = ins.address - base
                if mn in ("call", "jmp") or mn.startswith("j"):
                    if ops.startswith("0x"):
                        try:
                            tgt = int(ops, 0) - base
                        except ValueError:
                            continue
                        if mn in ("call", "jmp"):
                            self.callers[tgt].append((site, lo, mn))
                            self.calls[lo].append((site, tgt, mn))
                elif "rip +" in ops or "rip -" in ops:
                    i = ops.find("rip ")
                    j = ops.find("]", i)
                    disp = int(ops[i + 4 : j].replace(" ", ""), 0)
                    ref = (ins.address + ins.size + disp) - base
                    self.riprefs[ref].append((site, lo, f"{mn} {ops}"))


def sweep_refs(img, targets) -> None:
    """Report every 4-byte window that resolves to one of `targets` as a displacement.

    Boundary-independent on purpose. The decoded scan above starts at each `.pdata` function
    and walks forward, so a jump table embedded in the code desynchronises it and everything
    after the desync is invisible. This reads bytes instead: at file position `i` inside a
    section, `rva(i) + 4 + int32(bytes)` is what a `rel32` or `rip`-relative operand there
    would resolve to, because both are measured from the end of the displacement field.
    """
    import struct

    want = {int(t, 0) if isinstance(t, str) else t for t in targets}
    hits = {t: [] for t in want}
    for name, va, _vs, rp, rs in img.secs:
        if name not in (".text", ".rdata", ".data", "ERSC"):
            continue
        blob = img.data[rp : rp + rs]
        for i in range(len(blob) - 4):
            disp = struct.unpack_from("<i", blob, i)[0]
            t = va + i + 4 + disp
            if t in want:
                hits[t].append((name, va + i))
    for t in sorted(want):
        print(f"\n===== displacement windows resolving to {t:#x} =====")
        for name, site in hits[t]:
            print(f"  {site:#08x}  section {name}  (displacement field; instruction starts before)")
        print(f"  ({len(hits[t])} candidates)")


def scan_field(img, md, offsets) -> None:
    from capstone import CS_OP_IMM, CS_OP_MEM

    for off in offsets:
        print(f"\n===== every instruction touching `[reg + {off:#x}]` =====")
        rows = []
        for lo, _hi, ins in _iter_code(img, md):
            for op in ins.operands:
                if op.type != CS_OP_MEM or op.mem.disp != off:
                    continue
                if op.mem.base == 0:
                    continue
                base = ins.reg_name(op.mem.base)
                if base == "rip" or op.mem.index != 0:
                    continue
                imm = None
                for other in ins.operands:
                    if other.type == CS_OP_IMM:
                        imm = other.imm
                rows.append((ins.address - img.base, lo, ins.mnemonic, ins.op_str, imm))
                break
        by_kind = {}
        for r in rows:
            by_kind.setdefault(r[2], []).append(r)
        for kind in sorted(by_kind):
            group = by_kind[kind]
            print(f"\n  -- {kind} ({len(group)} sites) --")
            for rva, lo, _m, ops, imm in group:
                extra = "" if imm is None else f"   imm={imm:#x} ({imm})"
                print(f"    {rva:#08x}  fn {lo:#08x}  {kind:<8} {ops}{extra}")
        values = sorted({r[4] for r in rows if r[4] is not None})
        print(f"\n  distinct immediates seen: {[hex(v) for v in values]}")


def scan_field_values(img, md, offsets) -> None:
    """Report every constant this field is written with or compared against.

    A direct `cmp dword [reg + OFF], imm` is the easy half. The half that decides a state
    machine is the indirect one -- `mov eax,[reg + OFF]` followed by a chain of `cmp eax, imm`
    -- so the field's value is followed through the register it lands in until that register is
    overwritten. Both halves are reported with the function that owns them.
    """
    from capstone import CS_OP_IMM, CS_OP_MEM, CS_OP_REG

    for off in offsets:
        print(f"\n===== constants used with `[reg + {off:#x}]` =====")
        writes, reads = [], []
        for lo, _hi, ins in _iter_code(img, md):
            if lo != getattr(scan_field_values, "_fn", None):
                scan_field_values._fn = lo
                tracked = {}
            ops = ins.operands
            mem = next(
                (
                    o
                    for o in ops
                    if o.type == CS_OP_MEM
                    and o.mem.disp == off
                    and o.mem.base != 0
                    and o.mem.index == 0
                    and ins.reg_name(o.mem.base) != "rip"
                ),
                None,
            )
            imm = next((o.imm for o in ops if o.type == CS_OP_IMM), None)
            if mem is not None:
                if ins.mnemonic == "mov" and ops[0].type == CS_OP_MEM and imm is not None:
                    writes.append((ins.address - img.base, lo, imm, "direct"))
                elif ins.mnemonic == "cmp" and imm is not None:
                    reads.append((ins.address - img.base, lo, imm, "direct"))
                elif ins.mnemonic in ("mov", "movzx") and ops[0].type == CS_OP_REG:
                    tracked[ops[0].reg] = ins.address - img.base
                continue
            if not tracked:
                continue
            if ins.mnemonic == "cmp" and ops[0].type == CS_OP_REG and imm is not None:
                src = tracked.get(ops[0].reg)
                if src is not None:
                    reads.append((ins.address - img.base, lo, imm, f"via load {src:#x}"))
            elif ops and ops[0].type == CS_OP_REG and ins.mnemonic not in ("cmp", "test"):
                tracked.pop(ops[0].reg, None)

        for title, rows in (("written", writes), ("compared", reads)):
            print(f"\n  -- {title} --")
            for rva, lo, imm, how in rows:
                print(f"    {rva:#08x}  fn {lo:#08x}  {imm:#x} ({imm})   [{how}]")
            print(f"  distinct {title}: {sorted({hex(r[2]) for r in rows})}")


def fmt(img, rva):
    fn = img.fn_of(rva)
    where = f" in {fn[0]:#x}" if fn and fn[0] != rva else ""
    return f"{rva:#08x}{where}"


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--dll", default=DEFAULT_DLL)
    ap.add_argument("--to", action="append", default=[], help="list direct callers of this rva")
    ap.add_argument("--data-xrefs", action="append", default=[], help="list rip-relative refs")
    ap.add_argument("--depth", type=int, default=1)
    ap.add_argument(
        "--sweep-refs",
        action="append",
        default=[],
        help="byte-level displacement sweep for this rva, immune to a decode desync",
    )
    ap.add_argument(
        "--field", action="append", default=[], help="report every access to `[reg + this]`"
    )
    ap.add_argument(
        "--field-values",
        action="append",
        default=[],
        help="report the constants `[reg + this]` is written with or compared against",
    )
    ap.add_argument("--json", action="store_true")
    a = ap.parse_args()

    img = Image(a.dll)
    if a.sweep_refs:
        sweep_refs(img, [int(v, 0) for v in a.sweep_refs])
    if a.field or a.field_values:
        from capstone import CS_ARCH_X86, CS_MODE_64, Cs

        detailed = Cs(CS_ARCH_X86, CS_MODE_64)
        detailed.detail = True
        if a.field:
            scan_field(img, detailed, [int(v, 0) for v in a.field])
        if a.field_values:
            scan_field_values(img, detailed, [int(v, 0) for v in a.field_values])
    if not (a.to or a.data_xrefs):
        return 0
    g = Graph(img)

    out = {}
    for raw in a.to:
        tgt = int(raw, 0)
        seen = {tgt}
        level = [tgt]
        rows = []
        for d in range(a.depth):
            nxt = []
            for t in level:
                for site, owner, mn in g.callers.get(t, []):
                    rows.append((d, site, owner, mn, t))
                    if owner not in seen:
                        seen.add(owner)
                        nxt.append(owner)
            level = nxt
        out[raw] = rows
        if not a.json:
            print(f"\n===== direct control transfers to {tgt:#x} (depth {a.depth}) =====")
            for d, site, owner, mn, t in rows:
                print(f"  d{d} {mn:<4} at {site:#08x}  owner fn {owner:#08x}  -> {t:#x}")
            print(f"  ({len(rows)} sites)")

    for raw in a.data_xrefs:
        tgt = int(raw, 0)
        refs = g.riprefs.get(tgt, [])
        out[raw] = refs
        if not a.json:
            print(f"\n===== rip-relative references to {tgt:#x} =====")
            for site, owner, text in refs:
                print(f"  {site:#08x}  owner fn {owner:#08x}  {text}")
            print(f"  ({len(refs)} sites)")

    if a.json:
        print(json.dumps(out, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
