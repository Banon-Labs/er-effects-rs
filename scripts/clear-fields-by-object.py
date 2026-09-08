#!/usr/bin/env python3
"""Clear (or convict) a struct field offset per named object, per base register.

The rule this tool exists to OBEY
---------------------------------
A field offset is a number, and the same number is a field in dozens of unrelated structures.
`0x50`, `0x88`, `0x90`, `0xb8`, `0xd4` and `0xe0` are the repo's commonest small offsets, and one
Wwise init function that shifted its settings block by a uniform +0x38 in 1.17 "moves" all six at
once. Joining a repo constant to a drift row on the number therefore proves nothing, in either
direction: it neither convicts nor clears. Two discriminators built on 2026-08-30 did exactly
that -- "a hooked function reads that number" cleared 484 of 553 constants and anonymous
bracketing cleared all 48 -- and both had to be retracted.

A clearance is valid only when all three of these hold at once:
  * the object is named, and the same object is identified in both images independently;
  * the witness instruction reaches the field through a base register that provably holds a
    pointer to that object (`this`), not through some other pointer the same function walks;
  * the two function bodies are otherwise instruction-for-instruction identical, so a
    displacement that did not change did not change because the code did not change.

Where the object identity comes from
------------------------------------
MSVC RTTI. `vtable[-1]` is a CompleteObjectLocator, `COL+0x0c` a TypeDescriptor, `+0x10` its
mangled class name -- FromSoft's own metadata, embedded in each image separately. So finding
`.?AVMoveMapStep@CS@@` in 1.16.2 and again in 1.17 pairs the two vtables without consulting the
content-matched function map at all. Two consequences that matter here:

  Route a (virtual methods).  Vtable slot N is the same virtual method of the same class in both
      images, so slot N pairs two functions by object identity. This works for leaf functions,
      which MSVC gives no `.pdata` record and which the content map therefore cannot contain.
      In every such method `this` arrives in `rcx` by the x64 calling convention.

  Route B (constructors and other vtable users).  A function that stores the class's vtable into
      `[reg]` is constructing that object, and `reg` is `this`. Its 1.17 counterpart is taken
      from the function map -- but is then independently CONFIRMED: the paired body must store
      the 1.17 vtable of the same class at the corresponding instruction. A map pairing that
      happens to be wrong cannot satisfy that, because it would be storing some other class's
      vtable. Constructors touch many fields at once, so this is where the coverage is.

Which register holds `this`
---------------------------
Tracked, not assumed. `rcx` at entry; a `mov r64, <alias>` in the prologue (before the first
control-flow instruction, which is where MSVC parks `this` in a nonvolatile register) extends the
alias set; any write to a register removes it, and a `call` removes every volatile. Extending the
set is deliberately confined to the prologue: past the first branch a linear walk can pick up an
assignment from a path that does not reach the use, and a wrong alias is a wrong clearance. The
cost is coverage, which is the right way round -- a missed field is a lookup, a wrongly cleared
one is a write into a member the mod does not own.

VERDICTS
--------
  Cleared       the offset was read/written through `this` in >=1 otherwise-identical method pair
                of this class, and the displacement is the same in 1.17.
  Moved         the same, but the displacement changed. Old, new, and the witness are printed.
  Unknown       no witness. Not a clearance. Printed with the reason (no paired method touches
                it / every method that does has a changed body).

Usage
    --selftest
    --class CS::MoveMapStep [--offsets 0x4b8,0x50] [--routes ab]
    --classes-from file        one `class [= offset,offset]` per line. The separator is `=`,
                               not `:` -- a class name is full of colons. A line written with
                               `:` parses as one long class name, and every row then reports
                               `NO RTTI / class-not-in-rtti-join`, which reads exactly like
                               "this class does not exist in the images" (measured 2026-08-31,
                               18 classes silently reported as absent when all 18 were present).
    --control                  re-derive the one known 1.17 field move as a positive control
"""

from __future__ import annotations

import argparse
import collections
import importlib.util
import json
import os
import re
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000
# Resolved by scripts/struct_drift_out.py, not spelled here: this used to be a literal
# containing an agent session UUID, which is correct for exactly one session and wrong for
# every other one. `$ER_STRUCT_DRIFT_OUT` still overrides, and so does `--out-dir`.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import struct_drift_out  # noqa: E402 -- the path is set up on the line above

DEFAULT_OUT = struct_drift_out.default_out()


def _ensure_capstone():
    missing = []
    try:
        import capstone  # noqa: F401
    except ImportError:
        missing.append("capstone")
    try:
        import numpy  # noqa: F401
    except ImportError:
        missing.append("numpy")
    if missing:
        if os.environ.get("_OBJFIELD_UNDER_UV"):
            raise SystemExit(
                f"{', '.join(missing)} still missing under `uv run --with capstone --with numpy`"
            )
        os.environ["_OBJFIELD_UNDER_UV"] = "1"
        # numpy too: the vtable-xref index is a vectorised pass over 40 MB of `.text`, and uv's
        # ephemeral environment does not inherit the system site-packages that provide it.
        os.execvp(
            "uv",
            ["uv", "run", "--with", "capstone", "--with", "numpy", "python3", *sys.argv],
        )


def _drift_module():
    """Import `detect-struct-field-drift.py` rather than reimplementing its decoder.

    Its `compare_bodies` encodes two corrections that cost real debugging: the DECODED leaf
    extent (guessing at the next `.pdata` start runs the decode through inter-function padding,
    which is `0xCC` in one build and `0x90` in the other, desynchronising the two sides and
    manufacturing shape-DIFFs), and the split between field displacements, stack slots,
    rip-relative operands and image-base-relative globals. A second implementation of either
    would drift from the first and nobody would re-check it.
    """
    path = ROOT / "scripts" / "detect-struct-field-drift.py"
    spec = importlib.util.spec_from_file_location("_er_struct_drift", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# ---------------------------------------------------------------------------------------------
# `this` tracking
# ---------------------------------------------------------------------------------------------
_R64 = [
    "rax", "rcx", "rdx", "rbx", "rsp", "rbp", "rsi", "rdi",
    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15",
]
_SUB = {}
for _i, _r in enumerate(_R64):
    _SUB[_r] = _r
    if _i < 8:
        _SUB["e" + _r[1:]] = _r
        _SUB[_r[1:]] = _r                      # ax, cx, dx, bx, sp, bp, si, di
        _SUB[_r[1] + "l"] = _r                 # al, cl, dl, bl
        _SUB[_r[1:] + "l"] = _r                # spl, bpl, sil, dil
    else:
        _SUB[_r + "d"] = _r
        _SUB[_r + "w"] = _r
        _SUB[_r + "b"] = _r
_SUB["ah"], _SUB["ch"], _SUB["dh"], _SUB["bh"] = "rax", "rcx", "rdx", "rbx"

# Volatile under the Windows x64 ABI: a call may leave anything in these.
_VOLATILE = {"rax", "rcx", "rdx", "r8", "r9", "r10", "r11"}
# Mnemonics that read their first operand without writing it.
_NON_WRITING = {
    "cmp", "test", "push", "jmp", "ret", "nop", "int3", "call", "ud2",
    "cmpsb", "cmpsw", "cmpsd", "cmpsq", "bt", "prefetch", "prefetchw",
}
_CFLOW = re.compile(r"^(j\w+|call|ret|loop\w*|int3|ud2)$")
_MOV_REG_REG = re.compile(r"^([a-z0-9]+),\s*([a-z0-9]+)$")


_JCC = re.compile(r"^j[a-z]+$")
_HEX = re.compile(r"^0x[0-9a-f]+$")


def _writes(mnemonic: str, op_str: str) -> set[str]:
    """Registers this instruction may clobber (over-approximate: a miss would be unsound)."""
    if mnemonic == "call":
        return set(_VOLATILE)
    if mnemonic in _NON_WRITING:
        return set()
    out: set[str] = set()
    first = _SUB.get(op_str.split(",")[0].strip())
    if first:
        out.add(first)
    if mnemonic not in ("mov", "lea", "movzx", "movsx", "movsxd"):
        # `xchg`, `div`, `mul`, the string ops and the shift-by-cl forms write more than their
        # first operand. Rather than model each one, treat every bare register they mention as
        # clobbered: over-approximating a clobber can only lose a witness, while missing one
        # would keep a stale alias and hand back a wrong object.
        for token in re.findall(r"\b([a-z][a-z0-9]{1,3})\b", op_str):
            reg = _SUB.get(token)
            if reg:
                out.add(reg)
    return out


def _transfer(insns, lo: int, hi: int, alias: set[str]) -> list[set[str]]:
    """Alias sets for instructions `[lo, hi)`, given the set on entry to the block."""
    out = []
    for k in range(lo, hi):
        _addr, _size, mnemonic, op_str = insns[k]
        out.append(set(alias))
        if mnemonic == "mov":
            m = _MOV_REG_REG.match(op_str.strip())
            if m:
                dst, src = _SUB.get(m.group(1)), _SUB.get(m.group(2))
                if dst and src:
                    # A copy of a value already being tracked -- sound anywhere inside a block.
                    if src in alias:
                        alias.add(dst)
                    else:
                        alias.discard(dst)
                    continue
        alias -= _writes(mnemonic, op_str)
    return out


def this_aliases(insns) -> list[set[str]]:
    """Per-instruction set of registers that provably hold `this`, by basic-block dataflow.

    Why not a linear walk. A linear walk over the instructions in address order is not sound in
    the presence of branches, in both directions. A forward jump can skip the very `mov rbx, rcx`
    the walk just believed, so at the merge point `rbx` holds whatever it held before. A backward
    jump can re-enter above a write the walk has not reached yet, so a register the walk still
    calls `this` was reassigned on the path that actually got there. Either one hands back a base
    register pointing at some other object, and a field cleared against the wrong object is
    exactly the failure this whole exercise exists to prevent -- worse than no clearance, because
    nobody re-checks a clearance.

    So: split on direct branch targets, propagate the alias set per block, and meet at MERGES by
    intersection -- a register is `this` at a join only if it is `this` on every incoming path.
    A block with no known predecessor starts empty. If the function contains an indirect jump (a
    jump table, which the step machines are full of) its targets are unknown, so nothing outside
    the entry block is trusted at all.
    """
    n = len(insns)
    if not n:
        return []
    addr_index = {ins[0]: k for k, ins in enumerate(insns)}
    starts = {0}
    edges: dict[int, set[int]] = collections.defaultdict(set)
    indirect = False
    for k, (_addr, _size, mnemonic, op_str) in enumerate(insns):
        if mnemonic == "ret":
            if k + 1 < n:
                starts.add(k + 1)
            continue
        if mnemonic == "jmp" or _JCC.match(mnemonic):
            target = op_str.strip()
            if _HEX.match(target) and int(target, 16) in addr_index:
                t = addr_index[int(target, 16)]
                starts.add(t)
                edges[k].add(t)
            elif mnemonic == "jmp":
                indirect = True
            if k + 1 < n:
                starts.add(k + 1)
                if mnemonic != "jmp":
                    edges[k].add(k + 1)
            continue
    block_starts = sorted(starts)
    block_of = {}
    bounds = []
    for bi, start in enumerate(block_starts):
        end = block_starts[bi + 1] if bi + 1 < len(block_starts) else n
        bounds.append((start, end))
        for k in range(start, end):
            block_of[k] = bi
    preds: dict[int, set[int]] = collections.defaultdict(set)
    for bi, (start, end) in enumerate(bounds):
        last = end - 1
        _a, _s, mnemonic, _o = insns[last]
        if last in edges:
            for t in edges[last]:
                preds[block_of[t]].add(bi)
        elif mnemonic not in ("ret", "jmp") and end < n:
            preds[block_of[end]].add(bi)

    ALL = set(_R64)
    entry = {"rcx"}
    succs: dict[int, set[int]] = collections.defaultdict(set)
    for bi, (start, end) in enumerate(bounds):
        last = end - 1
        _a, _s, mnemonic, _o = insns[last]
        if last in edges:
            for t in edges[last]:
                succs[bi].add(block_of[t])
        elif mnemonic not in ("ret", "jmp") and end < n:
            succs[bi].add(block_of[end])
    reachable = {0}
    stack = [0]
    while stack:
        bi = stack.pop()
        for nxt in succs[bi]:
            if nxt not in reachable:
                reachable.add(nxt)
                stack.append(nxt)

    if indirect:
        # Unknown jump-table targets: only the entry block's state can be trusted.
        result: list[set[str]] = []
        for bi, (start, end) in enumerate(bounds):
            state = set(entry) if bi == 0 else set()
            result.extend(_transfer(insns, start, end, state))
        return result

    # A must analysis, so the lattice top is "every register" and unreachable blocks stay there:
    # a path that cannot execute must not remove an alias at a join it feeds into.
    ins_state = [set(ALL) for _ in bounds]
    out_state = [set(ALL) for _ in bounds]
    ins_state[0] = set(entry)
    for _round in range(len(bounds) + 2):
        changed = False
        for bi, (start, end) in enumerate(bounds):
            if bi == 0:
                incoming = set(entry)
            elif preds[bi]:
                incoming = ALL.copy()
                for p in preds[bi]:
                    incoming &= out_state[p]
            else:
                incoming = set(ALL)  # unreachable: contributes nothing to any meet
            exit_state = set(incoming)
            _transfer_exit(insns, start, end, exit_state)
            if incoming != ins_state[bi] or exit_state != out_state[bi]:
                ins_state[bi], out_state[bi] = incoming, exit_state
                changed = True
        if not changed:
            break
    result = []
    for bi, (start, end) in enumerate(bounds):
        # Dead code contributes no witnesses: its state is lattice-top, not knowledge.
        state = set(ins_state[bi]) if bi in reachable else set()
        result.extend(_transfer(insns, start, end, state))
    return result


def _transfer_exit(insns, lo: int, hi: int, alias: set[str]) -> None:
    """`_transfer` for its side effect on `alias` -- the state after the last instruction."""
    for k in range(lo, hi):
        _addr, _size, mnemonic, op_str = insns[k]
        if mnemonic == "mov":
            m = _MOV_REG_REG.match(op_str.strip())
            if m:
                dst, src = _SUB.get(m.group(1)), _SUB.get(m.group(2))
                if dst and src:
                    if src in alias:
                        alias.add(dst)
                    else:
                        alias.discard(dst)
                    continue
        alias -= _writes(mnemonic, op_str)


# ---------------------------------------------------------------------------------------------
# PE section table (the images are FLAT: file offset == RVA)
# ---------------------------------------------------------------------------------------------
def text_range(data: bytes) -> tuple[int, int]:
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    n_sections = struct.unpack_from("<H", data, pe + 6)[0]
    opt_size = struct.unpack_from("<H", data, pe + 20)[0]
    table = pe + 24 + opt_size
    for i in range(n_sections):
        entry = table + i * 40
        name = data[entry : entry + 8].rstrip(b"\x00")
        if name == b".text":
            vsize = struct.unpack_from("<I", data, entry + 8)[0]
            vaddr = struct.unpack_from("<I", data, entry + 12)[0]
            return vaddr, vaddr + vsize
    raise SystemExit("no .text section")


def vtable_slots(data: bytes, vt_va: int, lo: int, hi: int, limit: int = 512) -> list[int]:
    """Function pointers at `vt_va`, stopping at the first qword that is not code."""
    out = []
    off = vt_va - BASE
    for _ in range(limit):
        if off + 8 > len(data):
            break
        v = struct.unpack_from("<Q", data, off)[0]
        rva = v - BASE
        if not (lo <= rva < hi):
            break
        out.append(v)
        off += 8
    return out


# ---------------------------------------------------------------------------------------------
# evidence
# ---------------------------------------------------------------------------------------------
class Evidence:
    def __init__(self):
        self.held: dict[int, list[dict]] = collections.defaultdict(list)
        self.moved: dict[int, list[dict]] = collections.defaultdict(list)
        self.pairs = 0
        self.usable = 0
        self.skipped: collections.Counter = collections.Counter()
        self.witnesses: list[dict] = []
        # (function-pair tag, base register) -> the displacements held there. A bracket is only
        # an argument inside one of these: a field held below X and a field held above X, on the
        # same base register in the same object, proves nothing was inserted between them. Held
        # sets pooled across functions or across registers prove nothing at all -- that is the
        # anonymous bracketing that had to be retracted on 2026-08-30.
        self.spans: dict[tuple[str, str], set[int]] = collections.defaultdict(set)


_RIP = re.compile(r"\[rip \+ (0x[0-9a-f]+)\]|\[rip - (0x[0-9a-f]+)\]")
_DIRECT_CALL = re.compile(r"^0x[0-9a-f]+$")


def rip_target(addr: int, size: int, op_str: str) -> int | None:
    m = _RIP.search(op_str)
    if not m:
        return None
    disp = int(m.group(1), 16) if m.group(1) else -int(m.group(2), 16)
    return addr + size + disp


_VT_STORE = re.compile(r"^qword ptr \[([a-z0-9]+)\],\s*([a-z0-9]+)$")
# How far after the `lea` the matching store may sit. MSVC emits the two within a handful of
# instructions in a constructor; a wide window would start matching an unrelated later store.
VT_STORE_WINDOW = 24


def stores_vtable_into_this(insns, aliases, lea_index: int) -> bool:
    """Does the `lea` at `lea_index` end up written to `[this + 0]`?

    That store is what makes the function a CONSTRUCTOR of `this` rather than a function that
    merely mentions the class. `[this + 0x30]` is deliberately not accepted: that is an embedded
    base or member sub-object, so the enclosing `this` is a different type and its other fields
    must not be attributed to this class.
    """
    dest = insns[lea_index][3].split(",")[0].strip()
    dest = _SUB.get(dest)
    if not dest:
        return False
    for j in range(lea_index + 1, min(len(insns), lea_index + 1 + VT_STORE_WINDOW)):
        _addr, _size, mnemonic, op_str = insns[j]
        if mnemonic != "mov":
            # The register holding the vtable must survive to the store.
            first = _SUB.get(op_str.split(",")[0].strip())
            if first == dest:
                return False
            continue
        m = _VT_STORE.match(op_str.strip())
        if not m:
            if _SUB.get(op_str.split(",")[0].strip()) == dest:
                return False  # overwritten before it was stored
            continue
        base, src = _SUB.get(m.group(1)), _SUB.get(m.group(2))
        if src == dest and base in aliases[j]:
            return True
    return False


def harvest_pair(state, a_va: int, b_va: int, tag: str, ev: Evidence, depth: int,
                 confirm: tuple[int, int, int] | None = None) -> list[tuple[int, int, str]]:
    """Compare one function pair, harvest displacements taken off `this`, return callee pairs.

    `confirm` is `(vtable_1162, vtable_1170, insn_va)` for route B: the 1.17 body must reference
    the 1.17 vtable of the same class at the instruction where the 1.16.2 body references the
    1.16.2 one. A function-map mispairing cannot satisfy that -- it would be pointing at a
    different class's vtable -- so the map is used to propose the counterpart and RTTI to accept
    it.
    """
    drift, md = state["drift"], state["md"]
    a_rva, b_rva = a_va - BASE, b_va - BASE
    ev.pairs += 1
    a_end = drift.extent_of(a_rva, state["ends_old"], state["old"].data, state["starts_old"],
                            state["leaf_extent"])
    b_end = drift.extent_of(b_rva, state["ends_new"], state["new"].data, state["starts_new"],
                            state["leaf_extent"])
    if a_end is None or b_end is None:
        ev.skipped["no-extent"] += 1
        return []
    if a_end - a_rva > drift.MAX_FUNCTION_BYTES or b_end - b_rva > drift.MAX_FUNCTION_BYTES:
        ev.skipped["too-big"] += 1
        return []
    a_body = state["old"].data[a_rva:a_end]
    b_body = state["new"].data[b_rva:b_end]
    cmp = drift.compare_bodies(md, a_body, a_va, b_body, b_va)
    if cmp.verdict == drift.SHAPE_DIFF:
        ev.skipped["body-changed"] += 1
        return []
    insns = list(md.disasm_lite(a_body, a_va))
    b_insns = list(md.disasm_lite(b_body, b_va))
    aliases = this_aliases(insns)
    if confirm is not None:
        vt_a, vt_b, insn_va = confirm
        i = next((k for k, ins in enumerate(insns) if ins[0] == insn_va), None)
        if i is None:
            ev.skipped["vtable-insn-lost"] += 1
            return []
        got = rip_target(b_insns[i][0], b_insns[i][1], b_insns[i][3])
        if got != vt_b:
            ev.skipped["vtable-not-confirmed"] += 1
            return []
        if not stores_vtable_into_this(insns, aliases, i):
            ev.skipped["vtable-not-stored-into-this"] += 1
            return []
    ev.usable += 1
    ev.witnesses.append({"tag": tag, "va_1162": f"{a_va:#x}", "va_1170": f"{b_va:#x}",
                         "insns": len(insns), "verdict": cmp.verdict})

    callees: list[tuple[int, int, str]] = []
    for i, (addr, _sz, mnemonic, aop) in enumerate(insns):
        if mnemonic == "call" and depth > 0:
            # `this` is still argument 1 -> the callee's `rcx` is provably the same object, and
            # the counterpart is the call at the same index of an otherwise-identical body. That
            # is object identity propagated without the function map.
            if "rcx" in aliases[i] and _DIRECT_CALL.match(aop.strip()):
                b_op = b_insns[i][3].strip()
                if _DIRECT_CALL.match(b_op):
                    callees.append((int(aop, 16), int(b_op, 16), f"{tag}>callee@{addr:#x}"))
            continue
        if "[" not in aop:
            continue
        a_mem = drift.split_memory(aop)[1]
        b_mem = drift.split_memory(b_insns[i][3])[1]
        if len(a_mem) != len(b_mem):
            continue
        for (a_base, a_disp), (_b_base, b_disp) in zip(a_mem, b_mem):
            if not a_base or a_base == "rip" or a_base in drift.STACK_BASES:
                continue
            if not (0 < a_disp < drift.GLOBAL_DISPLACEMENT_MIN):
                continue
            if a_base not in aliases[i]:
                continue
            row = {"tag": tag, "va_1162": f"{addr:#x}", "va_1170": f"{b_insns[i][0]:#x}",
                   "base": a_base, "insn": f"{mnemonic} {aop}"}
            if a_disp == b_disp:
                ev.held[a_disp].append(row)
                ev.spans[(tag, a_base)].add(a_disp)
            else:
                row["new"] = b_disp
                ev.moved[a_disp].append(row)
    return callees


def analyse_class(state, class_name: str, routes: str = "abc", depth: int = 2,
                  max_pairs: int = 400) -> Evidence:
    """Every function pair that provably operates on `class_name`, and what it does to `this`."""
    ev = Evidence()
    joined = state["joined"]
    if class_name not in joined:
        ev.skipped["class-not-in-rtti-join"] += 1
        return ev
    vt_a, vt_b = joined[class_name]
    seeds: list[tuple[int, int, str, tuple | None]] = []

    if "a" in routes:
        slots_a = vtable_slots(state["old"].data, vt_a, *state["text_a"])
        slots_b = vtable_slots(state["new"].data, vt_b, *state["text_b"])
        for i in range(min(len(slots_a), len(slots_b))):
            seeds.append((slots_a[i], slots_b[i], f"vslot[{i}]", None))

    if "b" in routes:
        for func_va, insn_va in state["xrefs"](vt_a):
            paired = state["fmap"].get(func_va - BASE)
            if paired is None:
                ev.skipped["vtable-user-unmapped"] += 1
                continue
            seeds.append((func_va, BASE + paired, f"vtuser@{func_va:#x}",
                          (vt_a, vt_b, insn_va)))

    propagate = "c" in routes
    seen: set[tuple[int, int]] = set()
    queue = [(a, b, tag, conf, depth if propagate else 0) for a, b, tag, conf in seeds]
    while queue:
        if ev.pairs >= max_pairs:
            ev.skipped["pair-budget-exhausted"] += 1
            break
        a_va, b_va, tag, conf, d = queue.pop(0)
        if (a_va, b_va) in seen:
            continue
        seen.add((a_va, b_va))
        for ca, cb, ctag in harvest_pair(state, a_va, b_va, tag, ev, d, conf):
            if (ca, cb) not in seen:
                queue.append((ca, cb, ctag, None, d - 1))
    return ev


# ---------------------------------------------------------------------------------------------
# vtable xrefs
# ---------------------------------------------------------------------------------------------
def rip_lea_index(data: bytes, lo: int, hi: int) -> dict[int, list[int]]:
    """`{target_va: [instruction VAs]}` for every `lea r64, [rip + disp]` in `.text`.

    A vtable is referenced by a rip-relative `lea`, never by a bare RVA operand: searching the
    image for the vtable's 4-byte RVA finds nothing at all (measured: 0 hits for
    `CS::PlayerGameData`'s vtable; 2 hits once the same bytes are decoded as rip-leas). The
    7-byte encoding is `REX.W 8D modrm(mod=00,rm=101) disp32`, so the whole index is one
    vectorised pass rather than a decode of 40 MB of `.text`.
    """
    import numpy as np

    b = np.frombuffer(data, dtype=np.uint8)
    n = b.size - 8
    rex = (b[:n] == 0x48) | (b[:n] == 0x4C)
    cand = np.nonzero(rex & (b[1 : n + 1] == 0x8D) & ((b[2 : n + 2] & 0xC7) == 0x05))[0]
    cand = cand[(cand >= lo) & (cand < hi)]
    i = cand + 3
    disp = (
        b[i].astype(np.int64)
        | (b[i + 1].astype(np.int64) << 8)
        | (b[i + 2].astype(np.int64) << 16)
        | (b[i + 3].astype(np.int64) << 24)
    )
    disp = np.where(disp >= 2**31, disp - 2**32, disp)
    target = cand + 7 + disp
    index: dict[int, list[int]] = collections.defaultdict(list)
    for pos, tgt in zip(cand.tolist(), target.tolist()):
        index[BASE + tgt].append(BASE + pos)
    return index


def make_xref_lookup(data: bytes, starts_sorted: list[int], ends: dict[int, int],
                     lo: int, hi: int):
    """`vtable_va -> [(owning_function_va, instruction_va)]`, owner from `.pdata` extents."""
    import bisect

    index = rip_lea_index(data, lo, hi)

    def xrefs_for(vt_va: int) -> list[tuple[int, int]]:
        out: set[tuple[int, int]] = set()
        for insn_va in index.get(vt_va, ()):
            pos = insn_va - BASE
            k = bisect.bisect_right(starts_sorted, pos) - 1
            if k < 0:
                continue
            start = starts_sorted[k]
            end = ends.get(start)
            if end is not None and start <= pos < end:
                out.add((BASE + start, insn_va))
        return sorted(out)

    return xrefs_for


def build_state(routes: str = "ab"):
    _ensure_capstone()
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    drift = _drift_module()
    old, new = drift.Image(drift.IMAGE_1162), drift.Image(drift.IMAGE_1170)
    ends_old, ends_new = old.function_ends(), new.function_ends()
    joined_path = DEFAULT_OUT / "rtti-joined.tsv"
    if not joined_path.is_file():
        raise SystemExit(
            f"missing {joined_path}; run `python3 scripts/rtti-classmap-both.py` first"
        )
    joined = {}
    for line in joined_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        name, a, b = line.split("\t")
        joined[name] = (int(a, 16), int(b, 16))
    fmap = dict(drift.load_pairs(drift.FUNCTION_MAP))
    starts_old, starts_new = set(ends_old), set(ends_new)
    return {
        "drift": drift,
        "md": Cs(CS_ARCH_X86, CS_MODE_64),
        "old": old,
        "new": new,
        "ends_old": ends_old,
        "ends_new": ends_new,
        "starts_old": starts_old,
        "starts_new": starts_new,
        "leaf_extent": drift._sibling_leaf_extent(),
        "text_a": text_range(old.data),
        "text_b": text_range(new.data),
        "joined": joined,
        "fmap": fmap,
        "xrefs": make_xref_lookup(
            old.data, sorted(starts_old), ends_old, *text_range(old.data)
        ),
    }


def report_class(state, class_name: str, offsets: list[int] | None, routes: str,
                 depth: int = 2, max_pairs: int = 400, quiet: bool = False) -> dict:
    ev = analyse_class(state, class_name, routes, depth, max_pairs)
    vt = state["joined"].get(class_name)
    if not quiet:
        print(f"\n=== {class_name}"
              + (f"   vtable {vt[0]:#x} / {vt[1]:#x}" if vt else "  (NO RTTI)"))
        print(f"    {ev.usable} usable of {ev.pairs} paired function bodies; "
              f"skipped {dict(ev.skipped)}")
    wanted = offsets if offsets else sorted(set(ev.held) | set(ev.moved))
    rows = []
    for off in wanted:
        moved, held = ev.moved.get(off, []), ev.held.get(off, [])
        if moved:
            verdict = "MOVED"
            detail = "; ".join(
                f"{off:#x} -> {r['new']:#x} via {r['base']} at {r['va_1162']} ({r['tag']}: "
                f"{r['insn']})"
                for r in moved[:3]
            )
        elif held:
            verdict = "CLEARED"
            bases = sorted({r["base"] for r in held})
            detail = (f"{len(held)} instruction(s) on base {'/'.join(bases)} across "
                      f"{len({r['tag'] for r in held})} paired method(s); e.g. "
                      f"{held[0]['tag']} {held[0]['va_1162']}: {held[0]['insn']}")
        else:
            verdict = "STILL-UNKNOWN"
            detail = ("no paired, otherwise-identical method of this class touches this "
                      "displacement through `this`")
        if not quiet:
            pass
        rows.append({"class": class_name, "offset": off, "verdict": verdict,
                     "detail": detail,
                     "moved": [{k: v for k, v in r.items()} for r in moved],
                     "held_count": len(held)})
        if not quiet:
            print(f"    {verdict:<14} {off:#x}   {detail}")
    return {"class": class_name, "vtable_1162": f"{vt[0]:#x}" if vt else None,
            "vtable_1170": f"{vt[1]:#x}" if vt else None,
            "usable_pairs": ev.usable, "paired": ev.pairs, "skipped": dict(ev.skipped),
            "rows": rows,
            "witnesses": ev.witnesses[:40],
            "all_held": {f"{k:#x}": len(v) for k, v in sorted(ev.held.items())},
            "all_moved": {f"{k:#x}": [r["new"] for r in v] for k, v in sorted(ev.moved.items())},
            # Route a only -- the class's own virtual methods (and what they call with `this`).
            # This is the only evidence that is sound for a class other classes derive from: a
            # method of `C` may only touch `C`'s own members, whatever the dynamic type is,
            # whereas a constructor reached by route B may well be a derived class's ctor
            # storing the base vtable on its way past -- in which case `this` is the derived
            # object and its later field writes are derived fields.
            "vslot_held": {f"{k:#x}": sum(1 for r in v if r["tag"].startswith("vslot"))
                           for k, v in sorted(ev.held.items())
                           if any(r["tag"].startswith("vslot") for r in v)},
            "vslot_moved": {f"{k:#x}": [r["new"] for r in v if r["tag"].startswith("vslot")]
                            for k, v in sorted(ev.moved.items())
                            if any(r["tag"].startswith("vslot") for r in v)},
            # One entry per (function pair, base register): the displacements that held there.
            # This is the only shape in which a bracket argument is valid.
            "spans": [{"tag": tag, "base": base, "held": sorted(v)}
                      for (tag, base), v in sorted(ev.spans.items()) if len(v) > 1]}




# A named consumer function may have been edited in 1.17 without its object changing shape:
# `CS::MoveMapStep::STEP_MoveMap` gained exactly two instructions (`mov rcx, rbx; call ...`) and
# is otherwise identical. Refusing the whole function over that discards 973 perfectly good
# witness instructions, so the witness mode ALIGNS the two bodies and reads only the instructions
# that matched. `compare_bodies` deliberately does not do this -- for the exhaustive scan an
# equal-length identity check is the conservative choice -- but for one hand-named function whose
# edit is visible in the diff, the aligned instructions are exactly as trustworthy.
MIN_ALIGNED_FRACTION = 0.9


def align_bodies(drift, a_insns, b_insns):
    """`[(i, j)]` for instructions that match on mnemonic and operand shape, or None."""
    if not a_insns or not b_insns:
        return None
    if len(a_insns) == len(b_insns):
        return list(zip(range(len(a_insns)), range(len(b_insns))))
    import difflib

    def key(ins):
        return (ins[2], drift.split_memory(ins[3])[0])

    sm = difflib.SequenceMatcher(None, [key(i) for i in a_insns], [key(i) for i in b_insns],
                                 autojunk=False)
    out = []
    for i, j, n in sm.get_matching_blocks():
        out.extend((i + k, j + k) for k in range(n))
    if len(out) < MIN_ALIGNED_FRACTION * len(a_insns):
        return None
    return out


def witness_pair(state, a_va: int, b_va: int | None, label: str,
                 sink: list | None = None, length: int | None = None) -> int:
    """Per-base-register held/moved report for one named consumer function.

    For an object with no vtable of its own -- `CSMenuMan.menuData`, the DLUI input device, a
    `std::u16string` -- RTTI cannot name it, so the object identity comes from the repo's own RE
    (recorded in the doc comment above the constant). That is weaker than RTTI and is labelled as
    such wherever it is used. What this mode still supplies, and what a bare number-join never
    does, is the other two halves of a valid clearance: a single base register inside a single
    function, and a body that is otherwise instruction-for-instruction identical between the
    builds. Every displacement on one base register in one function is one object, whatever that
    object turns out to be called -- so a field that held still there held still in that object.
    """
    drift, md = state["drift"], state["md"]
    if b_va is None:
        paired = state["fmap"].get(a_va - BASE)
        if paired is None:
            print(f"{label}: {a_va:#x} has no 1.17 pairing in the function map -- STILL-UNKNOWN")
            return 1
        b_va = BASE + paired
    ev = Evidence()
    harvest_all_bases(state, a_va, b_va, label, ev, length=length)
    print(f"\n=== {label}   1.16.2 {a_va:#x}  ->  1.17 {b_va:#x}")
    if not ev.usable:
        print(f"    NO WITNESS: {dict(ev.skipped)}")
        return 1
    per_base: dict[str, dict[str, list]] = collections.defaultdict(
        lambda: {"held": [], "moved": []}
    )
    for disp, rows in ev.held.items():
        for r in rows:
            per_base[r["base"]]["held"].append(disp)
    for disp, rows in ev.moved.items():
        for r in rows:
            per_base[r["base"]]["moved"].append((disp, r["new"], r["insn"]))
    for base in sorted(per_base):
        held = sorted(set(per_base[base]["held"]))
        moved = sorted(set(per_base[base]["moved"]))
        print(f"    base {base:<5} HELD  {' '.join(f'{d:#x}' for d in held) or '(none)'}")
        if moved:
            for old, new, insn in moved:
                print(f"    base {base:<5} MOVED {old:#x} -> {new:#x}   {insn}")
        if sink is not None:
            sink.append({"label": label, "va_1162": f"{a_va:#x}", "va_1170": f"{b_va:#x}",
                         "base": base, "held": held,
                         "moved": [[o, n] for o, n, _i in moved]})
    return 0


def harvest_all_bases(state, a_va: int, b_va: int, tag: str, ev: Evidence,
                      length: int | None = None) -> None:
    """`harvest_pair` without the `this` filter: every non-stack, non-rip base register."""
    drift, md = state["drift"], state["md"]
    a_rva, b_rva = a_va - BASE, b_va - BASE
    ev.pairs += 1
    if length is None:
        a_end = drift.extent_of(a_rva, state["ends_old"], state["old"].data, state["starts_old"],
                                state["leaf_extent"])
        b_end = drift.extent_of(b_rva, state["ends_new"], state["new"].data, state["starts_new"],
                                state["leaf_extent"])
        if a_end is None or b_end is None:
            ev.skipped["no-extent"] += 1
            return
    else:
        if length <= 0:
            ev.skipped["bad-length"] += 1
            return
        a_end, b_end = a_rva + length, b_rva + length
        if a_end > len(state["old"].data) or b_end > len(state["new"].data):
            ev.skipped["length-out-of-range"] += 1
            return
    a_body = state["old"].data[a_rva:a_end]
    b_body = state["new"].data[b_rva:b_end]
    insns = list(md.disasm_lite(a_body, a_va))
    b_insns = list(md.disasm_lite(b_body, b_va))
    pairs = align_bodies(drift, insns, b_insns)
    if pairs is None:
        ev.skipped["body-changed"] = f"instruction count {len(insns)} vs {len(b_insns)}"
        return
    ev.usable += 1
    for i, j in pairs:
        addr, _sz, mnemonic, aop = insns[i]
        if "[" not in aop:
            continue
        a_mem = drift.split_memory(aop)[1]
        b_mem = drift.split_memory(b_insns[j][3])[1]
        if len(a_mem) != len(b_mem):
            continue
        for (a_base, a_disp), (_b, b_disp) in zip(a_mem, b_mem):
            if not a_base or a_base == "rip" or a_base in drift.STACK_BASES:
                continue
            if not (0 < a_disp < drift.GLOBAL_DISPLACEMENT_MIN):
                continue
            row = {"tag": tag, "va_1162": f"{addr:#x}", "base": a_base,
                   "insn": f"{mnemonic} {aop}"}
            if a_disp == b_disp:
                ev.held[a_disp].append(row)
            else:
                row["new"] = b_disp
                ev.moved[a_disp].append(row)


# ---------------------------------------------------------------------------------------------
def control(state) -> int:
    """Positive control -- the one field move this migration has confirmed by hand.

    `PlayerGameData` grew 8 bytes in 1.17: `GetScadutreeBlessing` is byte-identical between the
    builds except `[rcx+0xab5] -> [rcx+0xabd]` and `[rcx+0xab4] -> [rcx+0xabc]`. A method that
    cannot rediscover that is not a method, so this runs the same per-`this` engine over that
    pair and requires it to come back moved on `rcx`.
    """
    drift, md = state["drift"], state["md"]
    known = drift.KNOWN
    ev = Evidence()
    a_va, b_va = known["va_1162"], known["va_1170"]
    length = known["length"]
    a_rva, b_rva = a_va - BASE, b_va - BASE
    a_body = state["old"].data[a_rva : a_rva + length]
    b_body = state["new"].data[b_rva : b_rva + length]
    cmp = drift.compare_bodies(md, a_body, a_va, b_body, b_va)
    insns = list(md.disasm_lite(a_body, a_va))
    b_insns = list(md.disasm_lite(b_body, b_va))
    aliases = this_aliases(insns)
    found = []
    for i, (addr, _sz, mn, aop) in enumerate(insns):
        if "[" not in aop:
            continue
        for (ab, ad), (_bb, bd) in zip(
            drift.split_memory(aop)[1], drift.split_memory(b_insns[i][3])[1]
        ):
            if ab in aliases[i] and ad != bd:
                found.append((ab, ad, bd))
    print("POSITIVE CONTROL -- GetScadutreeBlessing / CS::PlayerGameData")
    print(f"  compare verdict: {cmp.verdict}")
    for base, old_d, new_d in found:
        print(f"  MOVED on `this` via {base}: {old_d:#x} -> {new_d:#x}")
    want = {(b, o, n) for b, o, n in known["expect_drift"]}
    ok = want <= set(found)
    print(f"  expected {sorted(want)}")
    print("  CONTROL", "REPRODUCED" if ok else "NOT REPRODUCED")
    return 0 if ok else 1


def selftest() -> int:
    ok = True
    Cs = None
    _ensure_capstone()
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs  # noqa: F811

    md = Cs(CS_ARCH_X86, CS_MODE_64)

    def decode(hexstr):
        return list(md.disasm_lite(bytes.fromhex(hexstr), BASE))

    # 1. `this` survives a prologue save into a nonvolatile register.
    #    48 89 5c 24 08   mov [rsp+8], rbx
    #    48 8b d9         mov rbx, rcx
    #    48 8b 43 10      mov rax, [rbx+0x10]
    insns = decode("48895c2408488bd9488b4310c3")
    al = this_aliases(insns)
    if "rbx" not in al[2]:
        print(f"FAIL: prologue `mov rbx, rcx` did not alias this ({al})")
        ok = False
    else:
        print("ok: prologue `mov rbx, rcx` makes rbx an alias of `this`")

    # MUTATION: if the alias rule is deleted, rbx must not be an alias. Emulate by feeding a
    # body where rbx is loaded from somewhere else instead.
    #    48 8b da         mov rbx, rdx      <- rdx is not `this`
    insns = decode("48895c2408488bda488b4310c3")
    if "rbx" in this_aliases(insns)[2]:
        print("FAIL: mutation -- rbx aliased `this` when it was loaded from rdx")
        ok = False
    else:
        print("ok: mutation (`mov rbx, rdx`) does not alias `this`")

    # 2. A call must drop the volatile alias, because the callee may return anything in rcx.
    #    ff 15 ..         call [rip+..]
    #    48 8b 41 10      mov rax, [rcx+0x10]
    insns = decode("ff1500000000488b4110c3")
    if "rcx" in this_aliases(insns)[1]:
        print("FAIL: rcx still aliased `this` after a call")
        ok = False
    else:
        print("ok: a call clears the volatile `this` alias")

    # 3. Overwriting rcx drops it.
    #    48 8b c8         mov rcx, rax
    insns = decode("488bc8488b4110c3")
    if "rcx" in this_aliases(insns)[1]:
        print("FAIL: rcx still aliased `this` after being overwritten from rax")
        ok = False
    else:
        print("ok: overwriting rcx drops the alias")

    # 4. A forward jump that skips the alias-establishing move must not leave `rbx` trusted at
    #    the merge -- the whole reason a linear walk is unsound.
    #      eb 03            jmp +3        (over the `mov rbx, rcx`)
    #      48 8b d9         mov rbx, rcx
    #      48 8b 43 10      mov rax, [rbx + 0x10]      <- merge point
    insns = decode("eb03488bd9488b4310c3")
    al = this_aliases(insns)
    merge = next(k for k, i2 in enumerate(insns) if i2[3].startswith("rax"))
    if "rbx" in al[merge]:
        print("FAIL: a forward jump skipping `mov rbx, rcx` still left rbx trusted")
        ok = False
    else:
        print("ok: intersection at a merge drops an alias not established on every path")

    # 4b. ... but when both paths establish it, the merge must keep it, or the tracker is so
    #     conservative it witnesses nothing.
    #      48 8b d9         mov rbx, rcx
    #      eb 03            jmp +3
    #      48 8b d9         mov rbx, rcx
    #      48 8b 43 10      mov rax, [rbx + 0x10]
    insns = decode("488bd9eb03488bd9488b4310c3")
    al = this_aliases(insns)
    merge = next(k for k, i2 in enumerate(insns) if i2[3].startswith("rax"))
    if "rbx" not in al[merge]:
        print("FAIL: an alias established on every incoming path was dropped at the merge")
        ok = False
    else:
        print("ok: an alias established on every path survives the merge")

    # 4c. An indirect jump means unknown targets, so nothing past the entry block is trusted.
    #      ff e0            jmp rax
    #      48 8b d9         mov rbx, rcx
    #      48 8b 43 10      mov rax, [rbx + 0x10]
    insns = decode("ffe0488bd9488b4310c3")
    al = this_aliases(insns)
    if any("rbx" in a for a in al[1:]):
        print("FAIL: a jump table left aliases trusted outside the entry block")
        ok = False
    else:
        print("ok: an indirect jump restricts trust to the entry block")

    # 5. Sub-register write kills the whole 64-bit alias (writing ebx zeroes rbx).
    insns = decode("48895c2408488bd9bb01000000488b4310c3")
    if "rbx" in this_aliases(insns)[3]:
        print("FAIL: `mov ebx, 1` did not kill the rbx alias")
        ok = False
    else:
        print("ok: a 32-bit write kills the 64-bit `this` alias")

    # 6. Route B's object test: the vtable must land in `[this + 0]`, not merely be mentioned.
    #    48 8b d9              mov rbx, rcx           (prologue: rbx aliases `this`)
    #    48 8d 05 00 00 00 00  lea rax, [rip]
    #    48 89 03              mov [rbx], rax         <- stored at this+0  => CONSTRUCTOR
    ctor = decode("488bd9488d0500000000488903c3")
    if not stores_vtable_into_this(ctor, this_aliases(ctor), 1):
        print("FAIL: control -- `mov [rbx], rax` after the lea was not recognised as a ctor")
        ok = False
    else:
        print("ok: a vtable stored at [this+0] identifies the object")

    #    Mutation 1: store at `[rbx + 0x30]` -- an embedded sub-object, so `this` is not this
    #    class. Accepting it is exactly the FD4Time false attribution (385 of 390 witnesses).
    sub = decode("488bd9488d0500000000488943 30".replace(" ", ""))
    if stores_vtable_into_this(sub, this_aliases(sub), 1):
        print("FAIL: mutation -- a vtable stored at [this+0x30] was accepted as `this`")
        ok = False
    else:
        print("ok: a vtable stored at [this+0x30] is rejected (embedded sub-object)")

    #    Mutation 2: store into a register that does not hold `this`.
    #    48 8b d9  mov rbx,rcx / lea rax,[rip] / 48 89 07  mov [rdi], rax
    other = decode("488bd9488d0500000000488907c3")
    if stores_vtable_into_this(other, this_aliases(other), 1):
        print("FAIL: mutation -- a vtable stored through a non-`this` register was accepted")
        ok = False
    else:
        print("ok: a vtable stored through a non-`this` register is rejected")

    #    Mutation 3: the class is merely mentioned -- lea, no store at all.
    mention = decode("488bd9488d0500000000488b4310c3")
    if stores_vtable_into_this(mention, this_aliases(mention), 1):
        print("FAIL: mutation -- merely mentioning the vtable was accepted as construction")
        ok = False
    else:
        print("ok: mentioning a vtable without storing it is rejected")

    # 7. The witness aligner: an inserted instruction must not discard the whole function, but a
    #    body that is mostly different must still be refused.
    drift = _drift_module()
    a = decode("488b4110488b4918488b5120c3")          # 3 loads + ret
    b = decode("488b411090488b4918488b5120c3")        # same, with a nop inserted
    got = align_bodies(drift, a, b)
    if not got or len(got) < len(a):
        print(f"FAIL: aligner dropped a body that differs only by an inserted nop ({got})")
        ok = False
    else:
        print("ok: an inserted instruction does not discard the surrounding witnesses")

    #    MUTATION: two bodies with almost nothing in common must be refused, not aligned.
    c = decode("50515253545556c3")                    # 7 pushes + ret
    if align_bodies(drift, a, c) is not None:
        print("FAIL: aligner accepted two bodies with almost no matching instructions")
        ok = False
    else:
        print(f"ok: a body sharing under {MIN_ALIGNED_FRACTION:.0%} of its instructions is refused")

    # 8. text_range on the real image, if present.
    img = ROOT / "eldenring-deobf.bin"
    if img.is_file():
        head = img.open("rb").read(0x1000)
        lo, hi = text_range(head)
        if not (0x1000 <= lo < hi < 0x8000000):
            print(f"FAIL: implausible .text range {lo:#x}..{hi:#x}")
            ok = False
        else:
            print(f"ok: .text {lo:#x}..{hi:#x} parsed from the PE header")

    print("SELFTEST", "PASS" if ok else "FAIL")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--control", action="store_true")
    ap.add_argument("--class", dest="klass")
    ap.add_argument("--offsets", default="")
    ap.add_argument("--classes-from", type=Path,
                    help="one `Class[= off,off]` per line; separator is `=`, never `:`")
    ap.add_argument("--routes", default="abc",
                    help="a=vtable slots, b=vtable users/ctors, c=propagate into callees")
    ap.add_argument("--depth", type=int, default=2,
                    help="callee propagation depth for route c")
    ap.add_argument("--max-pairs", type=int, default=400)
    ap.add_argument("--quiet", action="store_true")
    ap.add_argument("--witness-out", type=Path,
                    help="write the --witness results as JSON for the adjudicator to read")
    ap.add_argument("--witness", action="append", default=[],
                    help="`0x<va_1162>[,0x<va_1170>[,+0xlen]]=<object label>` -- per-base-register "
                         "held/moved for one named consumer function or fixed-length block")
    ap.add_argument("--out", type=Path)
    args = ap.parse_args()
    if args.selftest:
        return selftest()
    state = build_state(args.routes)
    if args.control:
        return control(state)
    if args.witness:
        rc = 0
        sink: list = []
        for spec in args.witness:
            addrs, _sep, label = spec.partition("=")
            parts = [x.strip() for x in addrs.split(",") if x.strip()]
            a = int(parts[0], 16)
            b = int(parts[1], 16) if len(parts) > 1 else None
            length = int(parts[2], 0) if len(parts) > 2 else None
            rc |= witness_pair(state, a, b, label.strip() or f"{a:#x}", sink, length=length)
        if args.witness_out:
            args.witness_out.write_text(json.dumps(sink, indent=1), encoding="utf-8")
            print(f"\nwrote {args.witness_out}")
        return rc
    jobs: list[tuple[str, list[int] | None]] = []
    if args.klass:
        offs = [int(x, 0) for x in args.offsets.split(",") if x.strip()] or None
        jobs.append((args.klass, offs))
    if args.classes_from:
        for line in args.classes_from.read_text(encoding="utf-8").splitlines():
            line = line.split("#")[0].strip()
            if not line:
                continue
            # `CS::MoveMapStep = 0x4b8,0x50`. The separator is `=`, not `:` -- a class name is
            # full of colons and splitting on one turns `CS::MoveMapStep` into an offset list.
            name, _sep, rest = line.partition("=")
            offs = [int(x, 0) for x in rest.split(",") if x.strip()] or None
            jobs.append((name.strip(), offs))
    if not jobs:
        ap.print_help()
        return 2
    out = []
    for k, (name, offs) in enumerate(jobs, 1):
        if args.quiet and k % 25 == 0:
            print(f"  ... {k}/{len(jobs)}", flush=True)
        out.append(
            report_class(state, name, offs, args.routes, args.depth, args.max_pairs, args.quiet)
        )
    if args.out:
        args.out.write_text(json.dumps(out, indent=1), encoding="utf-8")
        print(f"\nwrote {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
