#!/usr/bin/env python3
"""Map every MSVC `std::mutex` in the installed Seamless Co-op `ersc.dll`.

Third tool in the `ersc.dll` set, beside `scripts/disas-ersc.py` (one window) and
`scripts/ersc-xrefs.py` (call graph and field access). This one answers the two questions
a `_Mtx_lock` failure raises, neither of which a single window can settle:

`--locks` reports, for every call to `_Mtx_lock` and `_Mtx_unlock`, which object is
being locked -- the `lea reg,[base + disp]` that fed `rcx` is resolved by walking
backwards from the call, so `ersc+0x258d0` is reported as locking `[rcx+0x58] + 0x100` and
`ersc+0x241a0` as locking a different mutex at `[rcx] + 0x120`. Confusing the two is how a
recursive-lock story gets told about a mutex that was never the one involved.

`--ctors` reports every place a `_Mtx_internal_imp_t` is constructed in place, and the
`_Type` it is given. MSVC's `_Mutex_base` ctor is `constexpr`, so there is no `_Mtx_init`
call to find: it inlines as a store of `_Flags | _Mtx_try` at the mutex base and a store of
`-1` to `_Thread_id` at base `+0x48`. Pairing those two stores inside one function recovers
the type, and the type decides everything -- `_Mtx_lock` on a mutex carrying `_Mtx_recursive`
(`0x100`) cannot return nonzero at all, so a build that throws `resource_deadlock_would_occur`
proves the object it was handed did not carry that bit.

    uv run --with capstone python3 scripts/ersc-mutex-map.py --locks
    uv run --with capstone python3 scripts/ersc-mutex-map.py --locks --disp 0x100
    uv run --with capstone python3 scripts/ersc-mutex-map.py --ctors

Coverage is what `.pdata` declares, same as the other two tools. The obfuscated `ERSC`
section's entries overlap each other, so a construction that lives only there is not
reported; absence here is not proof of absence in the module.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import sys
from collections import defaultdict

_HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("disas_ersc", os.path.join(_HERE, "disas-ersc.py"))
_disas = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_disas)
Image = _disas.Image
DEFAULT_DLL = _disas.DEFAULT_DLL

# `_Mtx_internal_imp_t` on x64, recovered from this build's own unlock at 0xf9830:
#   `sub dword [rcx+0x4c],1 ; jne out ; or dword [rcx+0x48],-1 ; add rcx,0x10 ; LeaveCriticalSection`
THREAD_ID_OFFSET = 0x48
COUNT_OFFSET = 0x4C

# The two entry points, by rva in the supported build. `_Mtx_lock` is the `xor edx,edx` thunk in
# front of `_Mtx_do_lock` at 0xf9850, and `_Mtx_unlock` is the whole of 0xf9830. There is no
# third: 0xf9840 is inside `_Mtx_unlock`, not a `_Mtx_trylock` beside it.
KNOWN_ENTRIES = {
    0xF9828: "_Mtx_lock",
    0xF9830: "_Mtx_unlock",
}


def _resolve_mutex_operand(img, md, fn_lo, fn_hi, site):
    """Recover the `[base + disp]` expression whose address was in `rcx` at `site`.

    Walks the owning function forward once, keeping the last `lea rcx,[b+d]` / `mov rcx,reg`
    seen before the call, which is enough for the shapes MSVC emits here -- the address is
    always formed by a `lea` into `rcx` or into a register that is then moved to `rcx`. Returns
    a printable expression, or `None` when the address did not come from a `lea` this can see.
    """
    from capstone import CS_OP_MEM, CS_OP_REG

    o = img.off(fn_lo)
    leas: dict[int, str] = {}
    moves: dict[int, int] = {}
    rcx = None
    for ins in md.disasm(img.data[o : o + (fn_hi - fn_lo)], img.base + fn_lo):
        if ins.address - img.base >= site:
            break
        ops = ins.operands
        if ins.mnemonic == "lea" and ops and ops[0].type == CS_OP_REG:
            m = ops[1].mem
            if m.base and m.index == 0:
                leas[ops[0].reg] = f"[{ins.reg_name(m.base)}{m.disp:+#x}]"
                moves.pop(ops[0].reg, None)
            else:
                leas.pop(ops[0].reg, None)
        elif ins.mnemonic == "mov" and len(ops) == 2 and ops[0].type == CS_OP_REG:
            if ops[1].type == CS_OP_REG:
                moves[ops[0].reg] = ops[1].reg
                if ops[1].reg in leas:
                    leas[ops[0].reg] = leas[ops[1].reg]
                else:
                    leas.pop(ops[0].reg, None)
            else:
                leas.pop(ops[0].reg, None)
                if ops[1].type == CS_OP_MEM:
                    m = ops[1].mem
                    if m.base and m.index == 0:
                        # `mov rdi,[rcx+0x58]` -- remember the load so a later
                        # `lea rsi,[rdi+0x100]` prints as `[rcx+0x58]+0x100`.
                        moves[ops[0].reg] = None
                        leas[ops[0].reg] = f"*[{ins.reg_name(m.base)}{m.disp:+#x}]"
                        leas[ops[0].reg] = f"@{ins.reg_name(m.base)}{m.disp:+#x}"
        rcx = leas.get(_RCX)
    return rcx


_RCX = None  # filled in once capstone is imported


def scan_locks(img, md, want_disp) -> None:
    global _RCX
    from capstone import CS_OP_MEM, CS_OP_REG, x86_const

    _RCX = x86_const.X86_REG_RCX
    rows = []
    for lo, hi in img.funcs:
        if hi <= lo or hi - lo > 0x20000 or not img.entry_is_sound((lo, hi)):
            continue
        o = img.off(lo)
        if o is None:
            continue
        for ins in md.disasm(img.data[o : o + (hi - lo)], img.base + lo):
            if ins.mnemonic not in ("call", "jmp") or not ins.op_str.startswith("0x"):
                continue
            tgt = int(ins.op_str, 0) - img.base
            if tgt not in KNOWN_ENTRIES:
                continue
            site = ins.address - img.base
            expr = _resolve_mutex_operand(img, md, lo, hi, site) or "<unresolved>"
            rows.append((KNOWN_ENTRIES[tgt], lo, site, expr))
    if want_disp is not None:
        needle = f"{want_disp:+#x}]"
        rows = [r for r in rows if r[3].endswith(needle)]
    by_expr = defaultdict(list)
    for kind, lo, site, expr in rows:
        by_expr[expr].append((kind, lo, site))
    print(f"===== {len(rows)} mutex entry-point call sites, grouped by the object locked =====")
    for expr in sorted(by_expr, key=lambda e: (-len(by_expr[e]), e)):
        group = by_expr[expr]
        print(f"\n  -- {expr}  ({len(group)} sites) --")
        for kind, lo, site in sorted(group, key=lambda g: g[2]):
            print(f"    {site:#08x}  fn {lo:#08x}  {kind}")


def scan_ctors(img, md) -> None:
    """Every in-place `_Mtx_internal_imp_t` construction, and the `_Type` it is given."""
    from capstone import CS_OP_IMM, CS_OP_MEM

    stores = defaultdict(list)  # (fn, base reg, disp) -> [(site, imm, size)]
    for lo, _hi, ins in _disas._iter_code(img, md):
        if ins.mnemonic != "mov":
            continue
        ops = ins.operands
        if len(ops) != 2 or ops[0].type != CS_OP_MEM or ops[1].type != CS_OP_IMM:
            continue
        m = ops[0].mem
        if m.base == 0 or m.index != 0:
            continue
        reg = ins.reg_name(m.base)
        if reg == "rip":
            continue
        stores[(lo, reg, m.disp)].append((ins.address - img.base, ops[1].imm, ops[0].size))
    kinds = {
        1: "_Mtx_plain",
        2: "std::mutex (_Mtx_try)",
        0x101: "_Mtx_plain | _Mtx_recursive",
        0x102: "std::recursive_mutex",
    }
    found = coincidences = on_object = 0
    print("===== in-place `std::mutex` constructions (a `_Type` store paired with `_Thread_id = -1`) =====")
    for (lo, reg, disp), sites in sorted(stores.items()):
        if not any(imm in (-1, 0xFFFFFFFF) for _s, imm, _z in sites):
            continue
        base = disp - THREAD_ID_OFFSET
        for site, imm, _z in stores.get((lo, reg, base), []):
            if imm not in kinds:
                # Two unrelated stores 0x48 apart in one function. Counted, not printed: a
                # displacement coincidence is not a mutex and listing it invites a reader to
                # treat noise as a finding.
                coincidences += 1
                continue
            print(
                f"  fn {lo:#08x}  mutex at [{reg}{base:+#x}]  _Type={imm:#x}  ({kinds[imm]})"
                f"   type store {site:#x}"
            )
            found += 1
            if reg not in ("rbp", "rsp") and base >= 0:
                on_object += 1
    print(f"  ({found} constructions, {coincidences} displacement coincidences rejected)")
    if on_object == 0:
        print(
            "  None of them constructs a mutex inside a heap object. MSVC's ctor is `constexpr`,\n"
            "  so it inlines as two plain stores that can sit anywhere -- including the mutated\n"
            "  `ERSC` section, which `.pdata` cannot enumerate. A `_Type` this cannot find is not\n"
            "  a `_Type` that is absent: read it out of the live object rather than inferring it."
        )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--dll", default=DEFAULT_DLL)
    ap.add_argument("--locks", action="store_true")
    ap.add_argument("--ctors", action="store_true")
    ap.add_argument("--disp", default=None, help="only report objects locked at this displacement")
    a = ap.parse_args()

    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    img = Image(a.dll)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    if not a.locks and not a.ctors:
        a.locks = a.ctors = True
    if a.locks:
        scan_locks(img, md, int(a.disp, 0) if a.disp else None)
    if a.ctors:
        print()
        scan_ctors(img, md)
    return 0


if __name__ == "__main__":
    sys.exit(main())
