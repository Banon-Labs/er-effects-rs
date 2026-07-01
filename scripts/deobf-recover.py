#!/usr/bin/env python3
"""Recover a whole Arxan-obfuscated function's CFG by multi-path emulation.

scripts/deobf-emulate.py resolves ONE executed path. This drives full-function
recovery: it explores every path with generational search -- run under Unicorn
forcing a schedule of real-branch directions; each real conditional branch beyond
the schedule spawns a child schedule with that branch flipped. Unicorn resolves the
Arxan gadget dispatch on every path, so the union of paths is the real function's
control-flow graph over game-.text instructions (Arxan scaffolding folded to edges).

Output: recovered basic blocks (linearized real instructions) with their successors,
plus the real call targets -- a readable reconstruction of the deobfuscated function.

Approximate by construction: branch directions are FORCED (flags ignored), so a path
may reach a state the real inputs never would; such paths derail and are dropped, but
their blocks up to the derail are still recovered. Genuine VM stubs saturate the path
budget without converging. Bounds: --paths (default 400), per-path 1500 insns.
unicorn+capstone via `uv run --with unicorn --with capstone`.
"""
import argparse, os, sys
from collections import Counter, defaultdict, deque

try:
    import unicorn, capstone  # noqa
except ImportError:
    if os.environ.get("_DR2_BOOT") != "1":
        os.environ["_DR2_BOOT"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "unicorn", "--with", "capstone",
                         "python3", os.path.abspath(__file__)] + sys.argv[1:])
    sys.exit("unicorn/capstone unavailable and uv bootstrap failed")

from unicorn import (Uc, UC_ARCH_X86, UC_MODE_64, UC_PROT_ALL, UcError, UC_HOOK_CODE,
                     UC_HOOK_MEM_READ_UNMAPPED, UC_HOOK_MEM_WRITE_UNMAPPED, UC_HOOK_MEM_FETCH_UNMAPPED)
from unicorn.x86_const import (UC_X86_REG_RSP, UC_X86_REG_RIP, UC_X86_REG_RAX,
                               UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

BASE = 0x140000000
ARX = [(0x1429a3000, 0x1429af000), (0x144c0e000, 0x145e01800)]
GAME = (0x140001000, 0x1429a3000)
STACK, SSIZE = 0x7f0000000000, 0x200000
SCRATCH = 0x600000000000
SENT = 0x1337133713370000
JCC = {"je", "jne", "jz", "jnz", "ja", "jae", "jb", "jbe", "jg", "jge", "jl", "jle",
       "js", "jns", "jo", "jno", "jp", "jnp"}


def in_arx(v): return any(lo <= v < hi for lo, hi in ARX)
def in_game(v): return GAME[0] <= v < GAME[1]


def find_deobf():
    here = os.path.dirname(os.path.abspath(__file__))
    for p in (os.environ.get("ER_DEOBF"),
              os.path.join(os.path.dirname(here), "eldenring-deobf.bin"),
              "/home/banon/projects/er-effects-rs/eldenring-deobf.bin"):
        if p and os.path.exists(p):
            return p
    sys.exit("eldenring-deobf.bin not found (set ER_DEOBF=/path)")


class Recover:
    def __init__(self, img):
        self.img = img
        self.md = Cs(CS_ARCH_X86, CS_MODE_64)
        self.text = {}                 # real addr -> "mnem op"
        self.edges = set()             # (a,b) real control-flow edges
        self.calls = []
        self.terminals = {}            # real addr -> 'ret'|'int3'|'derail'
        self.new_this_gen = 0

    def run(self, entry, schedule, budget):
        uc = Uc(UC_ARCH_X86, UC_MODE_64)
        sz = (len(self.img) + 0xFFF) & ~0xFFF
        uc.mem_map(BASE, sz, UC_PROT_ALL); uc.mem_write(BASE, self.img)
        uc.mem_map(STACK, SSIZE); uc.mem_map(SCRATCH, 0x100000); uc.mem_map(SENT & ~0xFFF, 0x1000)
        rsp = STACK + SSIZE // 2
        uc.mem_write(rsp, SENT.to_bytes(8, "little")); uc.reg_write(UC_X86_REG_RSP, rsp)
        for r in (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9):
            uc.reg_write(r, SCRATCH + 0x1000)

        st = {"prev": None, "jidx": 0, "dirs": [], "visited": Counter(), "n": 0}

        def rec_edge(b):
            if st["prev"] is not None:
                if (st["prev"], b) not in self.edges:
                    self.edges.add((st["prev"], b))
            if b not in self.text:
                self.new_this_gen += 1
            st["prev"] = b

        def hc(uc, address, size, _):
            if address == SENT:
                if st["prev"] is not None: self.terminals[st["prev"]] = "ret"
                uc.emu_stop(); return
            if not (in_game(address) or in_arx(address)):
                if st["prev"] is not None: self.terminals[st["prev"]] = "derail"
                uc.emu_stop(); return
            st["visited"][address] += 1
            if st["visited"][address] > 40:
                uc.emu_stop(); return
            st["n"] += 1
            insn = next(self.md.disasm(bytes(uc.mem_read(address, size)), address), None)
            if insn is None:
                uc.emu_stop(); return
            m, o = insn.mnemonic, insn.op_str
            real = in_game(address)
            if m == "int3":
                if real:
                    self.text[address] = "int3"; rec_edge(address); self.terminals[address] = "int3"
                uc.emu_stop(); return
            if real:
                self.text.setdefault(address, f"{m} {o}")
                rec_edge(address)
            if m == "call" and o.startswith("0x") and in_game(int(o, 16)):
                if real: self.calls.append(int(o, 16))
                uc.reg_write(UC_X86_REG_RAX, 0)
                uc.reg_write(UC_X86_REG_RIP, address + size)
                return
            # force real conditional branches per schedule / generational default
            if real and m in JCC and o.startswith("0x"):
                target = int(o, 16)
                j = st["jidx"]; st["jidx"] += 1
                d = schedule[j] if j < len(schedule) else False
                st["dirs"].append(d)
                # record both potential successors as edges are discovered per path
                uc.reg_write(UC_X86_REG_RIP, target if d else address + size)
                return

        def hu(uc, acc, addr, size, val, _):
            try: uc.mem_map(addr & ~0xFFF, 0x1000)
            except UcError: pass
            return True

        uc.hook_add(UC_HOOK_CODE, hc)
        uc.hook_add(UC_HOOK_MEM_READ_UNMAPPED | UC_HOOK_MEM_WRITE_UNMAPPED | UC_HOOK_MEM_FETCH_UNMAPPED, hu)
        try:
            uc.emu_start(entry, SENT, count=budget)
        except UcError:
            if st["prev"] is not None and st["prev"] not in self.terminals:
                self.terminals[st["prev"]] = "derail"
        return st["dirs"]

    def explore(self, entry, max_paths, budget):
        seen = set()
        wl = deque([()])
        seen.add(())
        paths = 0
        dry = 0
        while wl and paths < max_paths:
            sched = wl.popleft()
            self.new_this_gen = 0
            dirs = self.run(entry, list(sched), budget)
            paths += 1
            dry = dry + 1 if self.new_this_gen == 0 else 0
            if dry > 60:
                break
            # generational: flip each branch beyond the given schedule
            for j in range(len(sched), len(dirs)):
                child = tuple(dirs[:j]) + (not dirs[j],)
                if child not in seen:
                    seen.add(child); wl.append(child)
        return paths


def coalesce(text, edges):
    succ = defaultdict(set); pred = defaultdict(set)
    for a, b in edges:
        succ[a].add(b); pred[b].add(a)
    starts = set()
    addrs = set(text)
    for a in addrs:
        if not pred[a] or len(pred[a]) > 1:
            starts.add(a)
        for p in pred[a]:
            if len(succ[p]) > 1:
                starts.add(a)
    if not starts and addrs:
        starts.add(min(addrs))
    blocks = []
    for s in sorted(starts):
        cur = s; seq = []
        while True:
            seq.append(cur)
            nx = succ[cur]
            if len(nx) == 1:
                n = next(iter(nx))
                if len(pred[n]) == 1 and n not in starts and n in text:
                    cur = n; continue
            break
        blocks.append((s, seq, sorted(succ[seq[-1]])))
    return blocks


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("va")
    ap.add_argument("--paths", type=int, default=400)
    ap.add_argument("--budget", type=int, default=1500)
    args = ap.parse_args()
    entry = int(args.va, 16)

    img = open(find_deobf(), "rb").read()
    r = Recover(img)
    paths = r.explore(entry, args.paths, args.budget)

    blocks = coalesce(r.text, r.edges)
    print(f"; RECOVERED FUNCTION @ {hex(entry)}  ({paths} paths explored)")
    print(f"; {len(r.text)} real instructions, {len(blocks)} basic blocks, "
          f"{len(set(r.calls))} distinct calls\n")
    def is_gadget(t):
        return (t.startswith("lea rsp, [rsp") or t.startswith("jmp qword ptr [rsp")
                or t.startswith("xchg qword ptr [rsp") or t == "nop"
                or (t.startswith("jmp 0x") and False))
    folded = 0
    for s, seq, succs in blocks:
        print(f"loc_{s:x}:")
        for a in seq:
            t = r.text.get(a, "?")
            if is_gadget(t) and a not in r.terminals:
                folded += 1
                continue
            note = ""
            if a in r.terminals: note = f"    ; {r.terminals[a]}"
            if t.startswith("call "):
                note = "    ; real call" + note
            elif t.startswith("movabs") and t.rstrip().endswith(tuple("0123456789abcdef")) and "0x14" in t:
                note = "    ; loads code addr (arxan dispatch target)"
            print(f"    {a:x}: {t}{note}")
        if succs:
            print(f"      -> {', '.join('loc_%x' % x for x in succs)}")
        print()
    if folded:
        print(f"; ({folded} Arxan stack-gadget lines folded)")
    if r.calls:
        print(f"; call targets: {', '.join(hex(c) for c in dict.fromkeys(r.calls))}")


if __name__ == "__main__":
    main()
