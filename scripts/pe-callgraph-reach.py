#!/usr/bin/env python3
"""Answer "can function A still reach function B?" from a built PE, not from the source.

Written for bd `er-effects-rs-uuly`, the P0 where a garbage non-NULL `key` from Steam/Seamless
walked into MSVC `strlen` and took the game down. The fix (`er_game_base::mem::safe_read_cstr`)
is a source change; the question a reviewer actually needs answered is whether the emitted code
still has the edge. Reading the source back cannot answer that -- inlining, tail calls and
`memcmp`/`strlen` idiom recognition all happen after the source is written.

Method: `.pdata` (the RUNTIME_FUNCTION table every x64 PE carries) gives exact function bounds
without needing symbols. Each range is disassembled, direct `call rel32` and function-leaving
`jmp rel32` (tail calls -- an edge the naive "grep for call" version misses) are collected, and
a BFS bounded to N levels asks whether any target is reachable from any root.

RIP-relative indirect calls are reported per-root rather than followed: they are import-thunk
slots, and the point of the bound is that an unfollowed edge is visible instead of silent.

Usage:
  uv run --with capstone python3 scripts/pe-callgraph-reach.py \
      <file.dll> <syms.tsv> <targets> <roots> [max_depth]

  syms.tsv  Either a `.pdb` (public symbols are read via `llvm-pdbutil` and their
            section:offset converted to RVA), or a prepared `name<TAB>0xRVA` file.
            Used only for readable output and for `--find`; reachability does not depend on it.

  --find SUBSTR   Print the RVA of every symbol whose name contains SUBSTR and exit. Use this to
                  turn a Rust path into the address the walk needs. A detour body often has no
                  public symbol at all (it is only ever referenced by address), in which case find
                  its install function and read the `lea` that passes the detour to `MhHook::new`.
  targets   comma-separated `0xRVA=label` -- the functions that must not be reachable.
  roots     comma-separated `0xRVA=label` -- where to start the walk.
"""

import bisect
import collections
import struct
import sys

IMAGE_DIRECTORY_ENTRY_EXCEPTION = 3
RUNTIME_FUNCTION_SIZE = 12
PE32PLUS_MAGIC = 0x20B
DEFAULT_MAX_DEPTH = 5
# Every non-game operation in this repo is hard-capped (scripts/check-no-timeouts.py); a PDB dump
# of a 3 MB symbol file takes under a second, so this is a fail-fast backstop, not a budget.
PDB_DUMP_TIMEOUT_SECONDS = 25


def parse_pe(data):
    """Sections, image base, and the .pdata directory. Enough to locate code; nothing more."""
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe_off : pe_off + 4] != b"PE\0\0":
        raise SystemExit("not a PE file")
    coff = pe_off + 4
    nsec = struct.unpack_from("<H", data, coff + 2)[0]
    opt_size = struct.unpack_from("<H", data, coff + 16)[0]
    opt = coff + 20
    if struct.unpack_from("<H", data, opt)[0] != PE32PLUS_MAGIC:
        raise SystemExit("not PE32+ (x86-64)")
    image_base = struct.unpack_from("<Q", data, opt + 24)[0]
    dirs = opt + 112
    exc_rva, exc_size = struct.unpack_from("<II", data, dirs + IMAGE_DIRECTORY_ENTRY_EXCEPTION * 8)
    sec_tbl = opt + opt_size
    sections = []
    for i in range(nsec):
        o = sec_tbl + i * 40
        name = data[o : o + 8].rstrip(b"\0").decode("ascii", "replace")
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, o + 8)
        sections.append((name, vaddr, vsize, rawptr, rawsize))
    return image_base, sections, exc_rva, exc_size


def make_rva2off(sections):
    def rva2off(rva):
        for _name, va, vsize, rawptr, rawsize in sections:
            if va <= rva < va + max(vsize, rawsize):
                delta = rva - va
                if delta < rawsize:
                    return rawptr + delta
        return None

    return rva2off


def pdata_functions(data, rva2off, exc_rva, exc_size):
    """Every function range the unwinder knows about, sorted. No symbols required."""
    base = rva2off(exc_rva)
    if base is None:
        raise SystemExit(".pdata directory is not backed by file data")
    funcs = []
    for i in range(exc_size // RUNTIME_FUNCTION_SIZE):
        begin, end, _unwind = struct.unpack_from("<III", data, base + i * RUNTIME_FUNCTION_SIZE)
        if begin == 0 and end == 0:
            continue
        funcs.append((begin, end))
    funcs.sort()
    return funcs


def build_edges(data, rva2off, funcs):
    """Direct call/tail-call edges, plus the RIP-relative call slots left unfollowed."""
    from capstone import CS_ARCH_X86, CS_MODE_64, CS_OP_IMM, CS_OP_MEM, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    edges = collections.defaultdict(set)
    indirect = collections.defaultdict(set)
    for begin, end in funcs:
        off = rva2off(begin)
        if off is None:
            continue
        for ins in md.disasm(data[off : off + (end - begin)], begin):
            if not (ins.mnemonic == "call" or ins.mnemonic.startswith("j")):
                continue
            ops = ins.operands
            if not ops:
                continue
            op = ops[0]
            if op.type == CS_OP_IMM:
                target = op.imm
                # A branch that stays inside the function is control flow, not a call edge.
                if ins.mnemonic == "call" or not (begin <= target < end):
                    edges[begin].add(target)
            elif op.type == CS_OP_MEM and ins.mnemonic == "call":
                if op.mem.base == 0 or md.reg_name(op.mem.base) == "rip":
                    indirect[begin].add(ins.address + ins.size + op.mem.disp)
    return edges, indirect


def load_symbols(syms_path, sections):
    """`name -> RVA`, from a `.pdb` directly or from a prepared `name<TAB>0xRVA` file.

    Reading the PDB here rather than in a one-off shell pipeline is the difference between a tool
    the next person can run and a recipe they have to reconstruct: PDB publics are `section:offset`
    and mean nothing until each section's virtual address is added back.
    """
    if not syms_path.lower().endswith(".pdb"):
        syms = {}
        with open(syms_path) as fh:
            for line in fh:
                parts = line.rstrip("\n").split("\t")
                if len(parts) == 2:
                    syms[int(parts[1], 16)] = parts[0]
        return syms

    import re
    import subprocess

    dump = subprocess.run(
        ["llvm-pdbutil", "dump", "--publics", syms_path],
        capture_output=True,
        text=True,
        timeout=PDB_DUMP_TIMEOUT_SECONDS,
    )
    if dump.returncode != 0:
        raise SystemExit(f"llvm-pdbutil failed on {syms_path}: {dump.stderr[:400]}")
    pattern = r"S_PUB32 \[size = \d+\] `([^`]*)`\s*\n\s*flags = [^,]*, addr = (\d+):(\d+)"
    syms = {}
    for name, section, offset in re.findall(pattern, dump.stdout):
        index = int(section) - 1
        if index < len(sections):
            syms[sections[index][1] + int(offset)] = name
    return syms


def main():
    if len(sys.argv) >= 4 and sys.argv[2] == "--find":
        data = open(sys.argv[1], "rb").read()
        _base, sections, _rva, _size = parse_pe(data)
        # `--find` takes the PDB in argv[3] and the substring in argv[4].
        for rva, name in sorted(load_symbols(sys.argv[3], sections).items()):
            if sys.argv[4] in name:
                print(f"{rva:#x}\t{name}")
        return 0

    if len(sys.argv) < 5:
        raise SystemExit(__doc__)
    dll_path, syms_path, targets_arg, roots_arg = sys.argv[1:5]
    max_depth = int(sys.argv[5]) if len(sys.argv) > 5 else DEFAULT_MAX_DEPTH

    data = open(dll_path, "rb").read()
    image_base, sections, exc_rva, exc_size = parse_pe(data)
    rva2off = make_rva2off(sections)
    funcs = pdata_functions(data, rva2off, exc_rva, exc_size)
    starts = [b for b, _ in funcs]

    def owner(rva):
        """The .pdata range containing `rva`, so an edge into a function body still attributes."""
        i = bisect.bisect_right(starts, rva) - 1
        if i < 0:
            return None
        begin, end = funcs[i]
        return begin if begin <= rva < end else None

    syms = load_symbols(syms_path, sections)

    def name_of(rva):
        label = syms.get(rva)
        if label is None:
            o = owner(rva)
            label = syms.get(o) if o is not None else None
        return f"{label}@{rva:#x}" if label else f"sub_{rva:#x}"

    def parse_pairs(arg):
        out = {}
        for item in arg.split(","):
            addr, _, label = item.partition("=")
            out[int(addr, 16)] = label or addr
        return out

    targets = parse_pairs(targets_arg)
    roots = parse_pairs(roots_arg)

    edges, indirect = build_edges(data, rva2off, funcs)
    graph = collections.defaultdict(set)
    for src, tgts in edges.items():
        s = owner(src) or src
        for t in tgts:
            o = owner(t)
            graph[s].add(o if o is not None else t)

    print(f"{dll_path}")
    print(f"  image_base={image_base:#x}  .pdata entries={len(funcs)}  call edges={sum(len(v) for v in graph.values())}")

    failures = 0
    for root_rva, root_label in roots.items():
        root = owner(root_rva) or root_rva
        print(f"\n=== BFS from {root_label} {root_rva:#x} (pdata owner {root:#x}), depth <= {max_depth} ===")
        depth = {root: 0}
        parent = {}
        queue = collections.deque([(root, 0)])
        while queue:
            fn, d = queue.popleft()
            if d >= max_depth:
                continue
            for t in sorted(graph.get(fn, ())):
                if t not in depth:
                    depth[t] = d + 1
                    parent[t] = fn
                    queue.append((t, d + 1))
        print(f"  reached {len(depth)} functions")
        for t_rva, t_label in targets.items():
            t = owner(t_rva) or t_rva
            if t in depth:
                chain, cur = [], t
                while cur in parent:
                    chain.append(name_of(cur))
                    cur = parent[cur]
                chain.append(name_of(cur))
                print(f"  REACHES {t_label} ({t_rva:#x}) at depth {depth[t]}: " + " <- ".join(chain))
                failures += 1
            else:
                print(f"  ok: {t_label} ({t_rva:#x}) NOT reachable within depth {max_depth}")
        print("  direct callees:")
        for t in sorted(graph.get(root, ())):
            print(f"    -> {name_of(t)}")
        slots = sorted(indirect.get(root, ()))
        print("  unfollowed rip-relative call slots:", [hex(s) for s in slots] or "none")

    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
