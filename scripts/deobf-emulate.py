#!/usr/bin/env python3
"""Emulation-driven Arxan dispatch resolver for eldenring-deobf.bin.

Static following (scripts/deobf-read.py) cannot resolve Arxan's opaque dispatch:
the real control transfers are `jmp qword [rsp-8]` ret-gadgets whose target was
computed at runtime via `movabs reg,<addr>` + stack shuffles. This tool *executes*
the function under Unicorn, so every gadget dispatch resolves to a concrete target,
recovering the real linearized instruction stream automatically.

Method: map the deobf image at 0x140000000, give the function a scratch stack with a
sentinel return address, run it under Unicorn, and record the executed instruction
stream. Direct calls into game .text are recorded and SKIPPED (we resolve THIS
function, not its callees); Arxan-internal calls/jumps/ret-gadgets are executed so
their dispatch is concretized. Unmapped reads are backed by zero pages on demand.
Output flags each resolved indirect dispatch (`==> DISPATCH`) — the payoff over
static analysis — plus real call targets and where control returns.

This is a concrete single-path trace (whatever the scratch register state selects at
data-dependent branches), not a full CFG recovery. Data-INDEPENDENT Arxan dispatch
(the common case) resolves regardless; genuinely VM-interpreted stubs will spin in an
interpreter loop and hit the loop guard — that is the signal they need real
devirtualization.

USAGE
  scripts/deobf-emulate.py 0x140110820          # resolve one function's real flow
  scripts/deobf-emulate.py --budget 400 0x...   # allow more executed instructions
  scripts/deobf-emulate.py --arxan 0x...        # also print the [arxan] scaffolding insns
unicorn + capstone auto-provision via `uv run --with unicorn --with capstone`.
"""
import argparse, os, sys
from collections import Counter

try:
    import unicorn, capstone  # noqa
except ImportError:
    if os.environ.get("_DE_BOOT") != "1":
        os.environ["_DE_BOOT"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "unicorn", "--with", "capstone",
                         "python3", os.path.abspath(__file__)] + sys.argv[1:])
    sys.exit("unicorn/capstone unavailable and uv bootstrap failed")

from unicorn import (Uc, UC_ARCH_X86, UC_MODE_64, UC_PROT_ALL, UcError,
                     UC_HOOK_CODE, UC_HOOK_MEM_READ_UNMAPPED, UC_HOOK_MEM_WRITE_UNMAPPED,
                     UC_HOOK_MEM_FETCH_UNMAPPED)
from unicorn.x86_const import (UC_X86_REG_RSP, UC_X86_REG_RIP, UC_X86_REG_RAX,
                               UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9)
from capstone import Cs, CS_ARCH_X86, CS_MODE_64

BASE = 0x140000000
ARX = [(0x1429a3000, 0x1429af000), (0x144c0e000, 0x145e01800)]
GAME = (0x140001000, 0x1429a3000)
STACK, SSIZE = 0x7f0000000000, 0x200000
SCRATCH = 0x600000000000
SENTINEL = 0x1337133713370000


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


def load_thunks():
    here = os.path.dirname(os.path.abspath(__file__))
    p = os.path.join(here, "arxan-thunks.tsv")
    m = {}
    if os.path.exists(p):
        for line in open(p).read().splitlines()[1:]:
            f = line.split("\t")
            if len(f) >= 5:
                m[int(f[0], 16)] = (int(f[2], 16), f[3], int(f[4], 16))
    return m


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("va")
    ap.add_argument("--budget", type=int, default=600)
    ap.add_argument("--arxan", action="store_true", help="also print [arxan] scaffolding instructions")
    args = ap.parse_args()
    va = int(args.va, 16)

    img = open(find_deobf(), "rb").read()
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    thunks = load_thunks()

    uc = Uc(UC_ARCH_X86, UC_MODE_64)
    imgsz = (len(img) + 0xFFF) & ~0xFFF
    uc.mem_map(BASE, imgsz, UC_PROT_ALL)
    uc.mem_write(BASE, img)
    uc.mem_map(STACK, SSIZE)
    uc.mem_map(SCRATCH, 0x100000)
    uc.mem_map(SENTINEL & ~0xFFF, 0x1000)
    rsp = STACK + SSIZE // 2
    uc.mem_write(rsp, SENTINEL.to_bytes(8, "little"))
    uc.reg_write(UC_X86_REG_RSP, rsp)
    for r in (UC_X86_REG_RCX, UC_X86_REG_RDX, UC_X86_REG_R8, UC_X86_REG_R9):
        uc.reg_write(r, SCRATCH + 0x1000)

    # Emulate from the true entry so the Arxan stub establishes correct stack state;
    # the resolved continuation is only used for annotation.
    entry = va
    if va in thunks:
        tgt, kind, res = thunks[va]
        print(f"; {hex(va)} ARXAN THUNK [{kind}] entry->{hex(tgt)} real code resumes @ {hex(res)}")

    trace = []              # (addr, mnem, op, is_arx)
    calls = []
    dispatches = []         # (from_addr, to_addr) resolved indirect transfers
    visited = Counter()
    prev = {"end": None, "addr": None, "was_direct_branch": False}

    def hook_code(uc, address, size, _):
        if address == SENTINEL:
            uc.emu_stop(); return
        # derailment guard: real flow stays in game/arxan .text
        if not (in_game(address) or in_arx(address)):
            trace.append((prev["addr"] or address, "; DERAILED to", hex(address), False))
            uc.emu_stop(); return
        visited[address] += 1
        if visited[address] > 40:         # runaway guard: real loops trace; VM dispatch spins
            trace.append((address, f"; loop/VM guard: {hex(address)} executed >40x", "", in_arx(address)))
            uc.emu_stop(); return
        code = bytes(uc.mem_read(address, size))
        insn = next(md.disasm(code, address), None)
        if insn is None:
            uc.emu_stop(); return
        m, o = insn.mnemonic, insn.op_str
        if m == "int3":               # unreachable after a noreturn call, or padding
            trace.append((address, "int3", "; end (noreturn/padding)", in_arx(address)))
            uc.emu_stop(); return
        # indirect dispatch detection: control arrived here NOT by fall-through and
        # NOT via a direct branch we could read statically
        if prev["end"] is not None and address != prev["end"] and not prev["was_direct_branch"]:
            dispatches.append((prev["addr"], address))
        trace.append((address, m, o, in_arx(address)))
        # skip direct calls into game .text (resolve THIS fn, not callees)
        if m == "call" and o.startswith("0x") and in_game(int(o, 16)):
            calls.append(int(o, 16))
            uc.reg_write(UC_X86_REG_RAX, 0)
            uc.reg_write(UC_X86_REG_RIP, address + size)
            prev.update(end=address + size, addr=address, was_direct_branch=False)
            return
        direct = (m in ("jmp", "je", "jne", "ja", "jb", "jae", "jbe", "jg", "jge",
                        "jl", "jle", "jz", "jnz", "call", "loop") and o.startswith("0x"))
        prev.update(end=address + size, addr=address, was_direct_branch=direct)

    def hook_unmapped(uc, access, address, size, value, _):
        try:
            uc.mem_map(address & ~0xFFF, 0x1000)
        except UcError:
            pass
        return True

    uc.hook_add(UC_HOOK_CODE, hook_code)
    uc.hook_add(UC_HOOK_MEM_READ_UNMAPPED | UC_HOOK_MEM_WRITE_UNMAPPED | UC_HOOK_MEM_FETCH_UNMAPPED,
                hook_unmapped)

    print(f"; emulating from {hex(entry)} (budget {args.budget} insns)")
    stopped = None
    try:
        uc.emu_start(entry, SENTINEL, count=args.budget)
    except UcError as e:
        stopped = f"UcError: {e} at rip={hex(uc.reg_read(UC_X86_REG_RIP))}"

    dset = {f: t for f, t in dispatches}
    shown = 0
    for addr, m, o, isx in trace:
        if isx and not args.arxan and m not in ("call",):
            continue  # fold Arxan scaffolding unless --arxan
        line = f"  {addr:x}: {m} {o}"
        if addr in dset:
            line += f"     ==> DISPATCH resolved to {hex(dset[addr])}"
        if m == "call" and o.startswith("0x") and in_game(int(o, 16)):
            line += "   <== real game call"
        print(line)
        shown += 1
    if not args.arxan:
        folded = sum(1 for _, _, _, isx in trace if isx)
        print(f"; ({folded} Arxan scaffolding insns folded; use --arxan to show)")
    print(f"; executed {len(trace)} insns, {len(dispatches)} resolved dispatches, "
          f"{len(set(calls))} distinct real calls")
    if calls:
        print(f"; real call targets: {', '.join(hex(c) for c in dict.fromkeys(calls))}")
    ret = SENTINEL in [t for _, t in dispatches] or (trace and trace[-1][0] == SENTINEL)
    print(f"; end: {'clean return to sentinel' if not stopped else stopped}")


if __name__ == "__main__":
    main()
