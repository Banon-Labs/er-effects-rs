"""Static-analysis helpers for Seamless Co-op's ersc.dll (imagebase 0x180000000).

Read-only. ersc.dll is Themida-packed, but only the entry point / IAT are
protected: the `.text` section is plaintext and has complete `.pdata` unwind
coverage (4839 functions), so normal function-boundary + xref analysis works.

Usage (capstone comes from uv, there is no system pip):
    uv run --with capstone python3 scripts/ersc_static.py --help

As a library:
    import ersc_static as E
    E.FUNCS, E.disas_func(rva), E.xrefs_to(rva), E.cstr(rva)

Override the DLL path with the ERSC env var. Data artifacts are written to
/tmp (or --out); never to the repo.
"""
import argparse
import bisect
import json
import os
import re
import struct
import sys
from collections import defaultdict

PATH = os.environ.get("ERSC", "/home/banon/Elden/ersc.dll")
BASE = 0x180000000

with open(PATH, "rb") as fh:
    D = fh.read()

# (name, rawaddr, rawsize, virtaddr, virtsize)
SECS = []


def _parse_secs():
    e = struct.unpack_from("<I", D, 0x3C)[0]
    nsec = struct.unpack_from("<H", D, e + 6)[0]
    optsz = struct.unpack_from("<H", D, e + 20)[0]
    sh = e + 24 + optsz
    for i in range(nsec):
        o = sh + i * 40
        nm = D[o : o + 8].rstrip(b"\x00").decode("latin1")
        vs, va, rs, ra = struct.unpack_from("<IIII", D, o + 8)
        SECS.append((nm, ra, rs, va, vs))


_parse_secs()


def r2f(rva):
    """RVA -> file offset, or None when unmapped."""
    for _nm, ra, rs, va, _vs in SECS:
        if va <= rva < va + rs:
            return ra + (rva - va)
    return None


def f2r(fo):
    for _nm, ra, rs, va, _vs in SECS:
        if ra <= fo < ra + rs:
            return va + (fo - ra)
    return None


def sec(rva):
    for nm, ra, rs, va, vs in SECS:
        if va <= rva < va + max(vs, rs):
            return nm
    return "?"


def read(rva, n):
    f = r2f(rva)
    return D[f : f + n] if f is not None else None


def u32(rva):
    return struct.unpack_from("<I", D, r2f(rva))[0]


def u64(rva):
    return struct.unpack_from("<Q", D, r2f(rva))[0]


def cstr(rva, maxn=400):
    f = r2f(rva)
    if f is None:
        return None
    e = D.find(b"\x00", f, f + maxn)
    if e < 0:
        e = f + maxn
    return D[f:e].decode("latin1")


def load_funcs():
    """Function (start, end) RVAs from the .pdata unwind table."""
    fns = []
    for nm, ra, _rs, _va, vs in SECS:
        if nm == ".pdata":
            for o in range(ra, ra + vs, 12):
                s, e, _u = struct.unpack_from("<III", D, o)
                if s:
                    fns.append((s, e))
    fns.sort()
    return fns


FUNCS = load_funcs()
FSTARTS = [f[0] for f in FUNCS]


def func_of(rva):
    """(start, end) of the .pdata function containing rva, else None."""
    i = bisect.bisect_right(FSTARTS, rva) - 1
    if i < 0:
        return None
    s, e = FUNCS[i]
    return (s, e) if s <= rva < e else None


def strings(secname=None, minlen=4):
    out = []
    for nm, ra, rs, _va, _vs in SECS:
        if secname and nm != secname:
            continue
        b = D[ra : ra + rs]
        va = [s[3] for s in SECS if s[0] == nm][0]
        for m in re.finditer(rb"[\x20-\x7e]{%d,}" % minlen, b):
            out.append((va + m.start(), m.group().decode("latin1")))
    return out


_md = None


def md():
    global _md
    if _md is None:
        from capstone import CS_ARCH_X86, CS_MODE_64, Cs

        _md = Cs(CS_ARCH_X86, CS_MODE_64)
        _md.detail = True
        _md.skipdata = True
    return _md


def disas(rva, n):
    return list(md().disasm(read(rva, n), rva))


def disas_func(start, end=None, limit=6000):
    if end is None:
        fe = func_of(start)
        end = fe[1] if fe else start + 0x200
    return list(md().disasm(read(start, end - start), start))[:limit]


X86_OP_IMM, X86_OP_MEM, X86_REG_RIP = 2, 3, 41
_XREF = None


def build_xrefs():
    """target_rva -> [referencing insn rva] across .text (rip-rel + call/jmp)."""
    global _XREF
    if _XREF is not None:
        return _XREF
    x = defaultdict(list)
    for nm, ra, rs, va, vs in SECS:
        if nm != ".text":
            continue
        for ins in md().disasm(D[ra : ra + min(rs, vs + 0x100)], va):
            if ins.id == 0:  # skipdata pseudo-instruction: no operand detail
                continue
            for op in ins.operands:
                if op.type == X86_OP_MEM and op.mem.base == X86_REG_RIP:
                    x[ins.address + ins.size + op.mem.disp].append(ins.address)
                elif op.type == X86_OP_IMM and ins.mnemonic in ("call", "jmp"):
                    x[op.imm].append(ins.address)
    _XREF = dict(x)
    return _XREF


def xrefs_to(rva):
    return build_xrefs().get(rva, [])


def funcs_referencing(rva):
    out = set()
    for a in xrefs_to(rva):
        f = func_of(a)
        if f:
            out.add(f[0])
    return sorted(out)


def _cmd_index(args):
    x = build_xrefs()
    out = {
        "path": PATH,
        "funcs": FUNCS,
        "xrefs": {str(k): v for k, v in x.items()},
    }
    with open(args.out, "w") as fh:
        json.dump(out, fh)
    print(f"funcs={len(FUNCS)} xref-targets={len(x)} -> {args.out}")


def _cmd_strings(args):
    for a, s in strings(args.section, args.minlen):
        if args.match and args.match not in s:
            continue
        print(f"{sec(a):9s} {a:#010x}  {s}")


def _cmd_disas(args):
    rva = int(args.rva, 0)
    for i in disas_func(rva, rva + args.n if args.n else None):
        print(f"{i.address:#010x}  {i.mnemonic:<8s} {i.op_str}")


def _cmd_xrefs(args):
    rva = int(args.rva, 0)
    for a in xrefs_to(rva):
        f = func_of(a)
        print(f"{a:#010x}  in func {f[0]:#010x}" if f else f"{a:#010x}  (no func)")


def main():
    p = argparse.ArgumentParser(description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    s = sub.add_parser("index", help="dump function + xref index as JSON")
    s.add_argument("--out", default="/tmp/ersc_index.json")
    s.set_defaults(fn=_cmd_index)

    s = sub.add_parser("strings", help="list strings")
    s.add_argument("--section")
    s.add_argument("--minlen", type=int, default=4)
    s.add_argument("--match")
    s.set_defaults(fn=_cmd_strings)

    s = sub.add_parser("disas", help="disassemble a function")
    s.add_argument("rva")
    s.add_argument("-n", type=int, default=0)
    s.set_defaults(fn=_cmd_disas)

    s = sub.add_parser("xrefs", help="who references this rva")
    s.add_argument("rva")
    s.set_defaults(fn=_cmd_xrefs)

    args = p.parse_args()
    args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
