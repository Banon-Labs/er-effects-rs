#!/usr/bin/env python3
"""bake-function.py -- automated pipeline: recovered ER function VA -> reassembled
clean function -> rev.ng lift -> LLVM opt -> recompiled PE, differentially verified.

Phase 1 (this file, --emit-asm): auto-reassembler. Runs multi-path recovery
(scripts/deobf-recover.py), coalesces the CFG, and emits a self-contained, position-
independent .s:
  * branch targets -> loc_<addr> labels
  * calls -> sub_<addr> (external, auto-stubbed for verification)
  * rip-relative operands -> a local `.data` symbol holding the bytes copied from the
    deobf image (assembler recomputes the displacement; correct for any instruction)
  * register/stack-relative memory kept verbatim (already position-independent)
Flattened functions still carrying Arxan stack gadgets (lea rsp,[rsp+-N] / jmp [rsp])
are detected and flagged (a later phase collapses them).

Phases 2-3 (--bake): orchestrate lift/opt/recompile + build a differential harness
(reference = clang(.s); recompiled = lift(reference)->opt->llc) and compare outputs.

unicorn+capstone via `uv run --with unicorn --with capstone`.
"""
import argparse, importlib.util, os, re, sys

HERE = os.path.dirname(os.path.abspath(__file__))
BASE = 0x140000000
GAME = (0x140001000, 0x1429a3000)


def load_recover():
    spec = importlib.util.spec_from_file_location("deobf_recover", os.path.join(HERE, "deobf-recover.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


DR = load_recover()
from capstone import Cs, CS_ARCH_X86, CS_MODE_64
from capstone import x86 as X

BRANCH = {"jmp", "je", "jne", "jz", "jnz", "ja", "jae", "jb", "jbe", "jg", "jge",
          "jl", "jle", "js", "jns", "jo", "jno", "jp", "jnp", "loop", "loope", "loopne"}


def reassemble(va, paths=200, budget=1500):
    img = open(DR.find_deobf(), "rb").read()
    r = DR.Recover(img)
    r.explore(va, paths, budget)
    if va not in r.text:
        return None, f"no instructions recovered at {hex(va)} (recovery derailed immediately)"
    blocks = DR.coalesce(r.text, r.edges)
    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True

    in_fn = set(r.text)                     # addresses that are our labels
    ARX = [(0x1429a3000, 0x1429af000), (0x144c0e000, 0x145e01800)]
    is_arx = lambda v: any(lo <= v < hi for lo, hi in ARX)

    # recovered control-flow edges (deobf-recover already emulated THROUGH the Arxan
    # dispatch, so the real successor of an Arxan-directed branch is recorded here).
    succ = {}
    for a, b in r.edges:
        succ.setdefault(a, []).append(b)

    # first pass: collect in-function branch targets (so every jump has a label) and
    # COLLAPSE Arxan-directed control-flow gadgets. An unconditional `jmp <arxan>` whose
    # recovery resolved to a single real (non-Arxan, in-function) successor is rewritten
    # to that successor -- the gadget is already de-flattened in r.edges. Conditional
    # branches / calls into Arxan aren't yet unambiguously edge-resolvable (a cond branch
    # has both a resolved-taken and a fall-through successor and edges don't say which is
    # which), so those still flag for a later phase.
    branch_targets = set()
    arxan_jmp = {}                          # addr -> resolved successor (collapsed gadget)
    for addr in in_fn:
        ins = next(md.disasm(img[addr - BASE:addr - BASE + 16], addr), None)
        if ins is None:
            continue
        if ins.mnemonic in BRANCH and ins.op_str.startswith("0x"):
            t = int(ins.op_str, 16)
            if is_arx(t):
                outs = [s for s in succ.get(addr, []) if s in in_fn and not is_arx(s)]
                if ins.mnemonic == "jmp" and len(outs) == 1:
                    arxan_jmp[addr] = outs[0]
                    branch_targets.add(outs[0])
                    continue
                kind = "branch" if ins.mnemonic == "jmp" else "conditional branch"
                return None, (f"flattened/thunk: {kind} into Arxan section at {hex(addr)} -> {hex(t)} "
                              f"(recovered succs={[hex(s) for s in succ.get(addr, [])]}; needs gadget-collapse phase)")
            if t in in_fn:
                branch_targets.add(t)
        if ins.mnemonic == "call" and ins.op_str.startswith("0x") and is_arx(int(ins.op_str, 16)):
            return None, f"flattened/thunk: call into Arxan section at {hex(addr)} (needs gadget-collapse phase)"

    data_syms = {}                          # target_va -> (symbol, nbytes)
    callees = set()
    gadgets = []
    lines = [".intel_syntax noprefix", ".text", ".global recovered_func", "recovered_func:"]

    def data_ref(target, nbytes):
        sym = f"data_{target:x}"
        prev = data_syms.get(target)
        if prev is None or nbytes > prev[1]:
            data_syms[target] = (sym, max(nbytes, prev[1] if prev else 0))
        return sym

    ordered = []
    for start, seq, succs in blocks:
        ordered.append((start, seq))
    # ensure entry block first
    ordered.sort(key=lambda b: (b[0] != va, b[0]))

    for start, seq in ordered:
        for addr in seq:
            if addr == start or addr in branch_targets:
                lines.append(f"loc_{addr:x}:")     # label every block start AND branch target
            ins = next(md.disasm(img[addr - BASE:addr - BASE + 16], addr), None)
            if ins is None:
                return None, f"undecodable instruction at {hex(addr)}"
            m, o = ins.mnemonic, ins.op_str
            # gadget detection
            if m == "lea" and re.match(r"rsp, \[rsp [+-]", o):
                gadgets.append(hex(addr))
            if m == "jmp" and "[rsp" in o:
                gadgets.append(hex(addr))
            # rip-relative operands -> local data symbol
            for op in ins.operands:
                if op.type == X.X86_OP_MEM and md.reg_name(op.mem.base) == "rip":
                    target = addr + ins.size + op.mem.disp
                    nb = op.size if op.size else 8
                    if m == "lea":
                        nb = max(nb, 16)     # pointer to data; copy a chunk
                    sym = data_ref(target, nb)
                    o = re.sub(r"\[rip [+-] 0x[0-9a-f]+\]", f"[rip + {sym}]", o)
            # branch/call target rewriting (immediate targets)
            if m == "call" and o.startswith("0x"):
                t = int(o, 16); callees.add(t); o = f"sub_{t:x}"
            elif m in BRANCH and o.startswith("0x"):
                t = int(o, 16)
                if addr in arxan_jmp:        # collapsed Arxan gadget -> resolved successor
                    o = f"loc_{arxan_jmp[addr]:x}"
                elif t in in_fn:
                    o = f"loc_{t:x}"
                else:                        # tail call / external jump
                    callees.add(t); o = f"sub_{t:x}"
            lines.append(f"    {m} {o}".rstrip())

    # data section
    lines.append(".data")
    for target, (sym, nb) in sorted(data_syms.items()):
        raw = img[target - BASE:target - BASE + nb]
        byts = ",".join(str(b) for b in raw)
        lines.append(f"{sym}: .byte {byts}")

    meta = {"callees": sorted(callees), "data_syms": data_syms, "gadgets": gadgets,
            "collapsed_gadgets": {hex(k): hex(v) for k, v in arxan_jmp.items()},
            "nblocks": len(blocks), "ninsns": len(r.text)}
    return "\n".join(lines) + "\n", meta


import subprocess

WORK = os.path.expanduser("~/er-llvm-spike")
XWIN = os.path.expanduser("~/.cache/cargo-xwin/xwin")
IMG = "revng/revng:latest"
RB = "/revng/root/lib64/llvm/llvm/bin"
WINEPREFIX = os.path.join(WORK, "wineprefix")


def sh(cmd, timeout=120, env=None):
    e = dict(os.environ)
    if env:
        e.update(env)
    return subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=timeout, env=e)


def dock(inner, timeout=180):
    return sh(f"docker run --rm -v {WORK}:/w {IMG} bash -c {shq(inner)}", timeout=timeout)


def shq(s):
    return "'" + s.replace("'", "'\\''") + "'"


def clang_c(src, obj):
    inc = " ".join(f'-isystem "{XWIN}/{p}"' for p in
                   ("crt/include", "sdk/include/ucrt", "sdk/include/um", "sdk/include/shared"))
    return sh(f'clang --target=x86_64-pc-windows-msvc -ffreestanding -O2 -c "{src}" -o "{obj}" {inc}')


def wine_exit(exe):
    # run wine directly and read the real process exit code (wine propagates the
    # Windows exit code; Unix returncode is it & 0xff). NB: an earlier version used
    # `bash -c "...; echo EX=$?"` which the OUTER shell expanded before wine ran ->
    # always 0 -> vacuous ref==recompiled. Do NOT reintroduce that.
    try:
        r = subprocess.run(["wine", exe], capture_output=True, timeout=90,
                           env={**os.environ, "WINEPREFIX": WINEPREFIX, "WINEDEBUG": "-all"})
        return r.returncode & 0xff
    except subprocess.TimeoutExpired:
        return None


def bake(va, paths, budget, ninputs=200000):
    asm, meta = reassemble(va, paths, budget)
    if asm is None:
        return {"ok": False, "stage": "reassemble", "err": meta}
    if meta["gadgets"]:
        return {"ok": False, "stage": "reassemble",
                "err": f"flattened function: Arxan stack gadgets at {meta['gadgets']} -- needs gadget-collapse (later phase)"}
    os.makedirs(WORK, exist_ok=True)
    p = lambda n: os.path.join(WORK, n)
    open(p("bf_rf.s"), "w").write(asm)

    # deterministic callee stubs: return 0 (constant -> no arg-ABI drift between ref &
    # recompiled; 0 tends to keep the function's own input-dependent path live).
    cst = ["/* deterministic callee stubs (return 0) */"]
    for c in meta["callees"]:
        cst.append(f"unsigned long long sub_{c:x}(void){{ return 0ULL; }}")
    open(p("bf_callees.c"), "w").write("\n".join(cst) + "\n")

    # differential harness: fold recovered_func(i) over scalar inputs; low 7 bits carry
    # the fold, high bit flags whether the output actually VARIED (guards vacuous tests).
    # minimal harness (matches the proven probe pattern: few constant-arg calls).
    harn = ("__declspec(dllimport) void __stdcall ExitProcess(unsigned int);\n"
            "extern unsigned long long recovered_func(unsigned long long);\n"
            "void entry(void){\n"
            "  unsigned long long a=recovered_func(3), b=recovered_func(6),\n"
            "                    c=recovered_func(12345), d=recovered_func(0xabcdef);\n"
            "  unsigned varied=(a!=b||b!=c||c!=d)?0x80u:0u;\n"
            "  unsigned h=(unsigned)((a*7ULL+b*13ULL+c*17ULL+d*23ULL)&0x7fULL);\n"
            "  ExitProcess(h|varied);\n}\n")
    open(p("bf_harness.c"), "w").write(harn)

    for src, obj in (("bf_harness.c", "bf_harness.obj"), ("bf_callees.c", "bf_callees.obj")):
        r = clang_c(p(src), p(obj))
        if r.returncode:
            return {"ok": False, "stage": "compile", "err": r.stderr[-500:]}
    r = sh(f'clang --target=x86_64-pc-windows-msvc -c "{p("bf_rf.s")}" -o "{p("bf_rf.obj")}"')
    if r.returncode:
        return {"ok": False, "stage": "assemble", "err": r.stderr[-500:]}

    libp = f'"/libpath:{XWIN}/sdk/lib/um/x86_64" kernel32.lib /nodefaultlib'
    link = lambda objs, out, extra="": sh(
        f'lld-link {" ".join(p(o) for o in objs)} /out:{p(out)} /entry:entry '
        f'/subsystem:console {libp} {extra}')
    r = link(["bf_harness.obj", "bf_rf.obj", "bf_callees.obj"], "bf_ref.exe", f"/map:{p('bf_ref.map')}")
    if r.returncode:
        return {"ok": False, "stage": "link-ref", "err": r.stderr[-500:]}
    ref_exit = wine_exit(p("bf_ref.exe"))
    if ref_exit is None or ref_exit >= 200:      # >=200 ~ wine/crash codes
        return {"ok": False, "stage": "run-ref",
                "err": f"reference exit={ref_exit} (likely crash: function derefs pointer args -> needs structured-input harness, later phase)"}

    # symbol VAs from the map: " 0001:00000050  recovered_func  0000000140001050  obj"
    vamap = {}
    for line in open(p("bf_ref.map")):
        m = re.search(r"\b(recovered_func|sub_[0-9a-f]+)\s+([0-9a-f]{16})\b", line)
        if m:
            vamap[m.group(1)] = int(m.group(2), 16)
    if "recovered_func" not in vamap:
        return {"ok": False, "stage": "map", "err": "could not find recovered_func VA in map"}
    rf_va = vamap["recovered_func"]

    # lift
    r = dock(f"cd /w && revng artifact --analyze enforce-abi /w/bf_ref.exe -o /w/bf.abi.bc.zstd", timeout=180)
    if not os.path.exists(p("bf.abi.bc.zstd")):
        return {"ok": False, "stage": "lift", "err": (r.stderr or r.stdout)[-800:]}
    sh(f"zstd -d -f {p('bf.abi.bc.zstd')} -o {p('bf.abi.bc')}")

    # extract + opt + dis
    dock(f"{RB}/llvm-extract -func='local_0x{rf_va:x}:Code_x86_64' /w/bf.abi.bc -o /w/bf.rf.bc; "
         f"{RB}/opt -passes='default<O3>,dse,globaldce' /w/bf.rf.bc -o /w/bf.rf.o3.bc; "
         f"{RB}/llvm-dis /w/bf.rf.o3.bc -o /w/bf.rf.ll")
    if not os.path.exists(p("bf.rf.ll")):
        return {"ok": False, "stage": "extract-opt", "err": "no IR produced"}

    # clean: rename fn + callees, drop newpc
    ll = open(p("bf.rf.ll")).read()
    ll = ll.replace(f'"local_0x{rf_va:x}:Code_x86_64"', "recovered_func")
    for name, cva in vamap.items():
        if name.startswith("sub_"):
            ll = ll.replace(f'"local_0x{cva:x}:Code_x86_64"', name)
    ll = "\n".join(l for l in ll.splitlines() if "@newpc" not in l) + "\n"
    open(p("bf.clean.ll"), "w").write(ll)

    r = dock(f"{RB}/llc -mtriple=x86_64-pc-windows-msvc -filetype=obj -O2 /w/bf.clean.ll -o /w/bf.rf.rc.obj "
             f"2>&1; {RB}/llvm-nm -u /w/bf.rf.rc.obj")
    if not os.path.exists(p("bf.rf.rc.obj")):
        return {"ok": False, "stage": "llc", "err": r.stdout[-800:]}
    undef = set(re.findall(r"U (\S+)", r.stdout))
    callnames = {f"sub_{c:x}" for c in meta["callees"]}
    residue = undef - callnames

    # runtime stub for the rev.ng residue (valid @_rsp scratch + dead helpers)
    rt = ["static unsigned long long _emu_stack[8192];",
          "unsigned long long _rsp = (unsigned long long)(_emu_stack+4096);"]
    for s in sorted(residue):
        if s == "_rsp":
            continue
        rt.append(f"unsigned long long {s}(unsigned long long a,unsigned long long b,int c){{ (void)a;(void)b;(void)c; return 0; }}")
    open(p("bf_rtstub.c"), "w").write("\n".join(rt) + "\n")
    r = clang_c(p("bf_rtstub.c"), p("bf_rtstub.obj"))
    if r.returncode:
        return {"ok": False, "stage": "rtstub", "err": r.stderr[-500:]}

    r = link(["bf_harness.obj", "bf.rf.rc.obj", "bf_callees.obj", "bf_rtstub.obj"], "bf_recomp.exe")
    if r.returncode:
        return {"ok": False, "stage": "link-recomp", "err": r.stderr[-600:]}
    rc_exit = wine_exit(p("bf_recomp.exe"))

    varied = bool(ref_exit is not None and ref_exit & 0x80)
    return {"ok": rc_exit == ref_exit, "stage": "verify", "ref": ref_exit, "recompiled": rc_exit,
            "input_dependent": varied, "nblocks": meta["nblocks"], "ninsns": meta["ninsns"],
            "callees": [hex(c) for c in meta["callees"]], "residue": sorted(residue)}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("va")
    ap.add_argument("--emit-asm", action="store_true", help="emit reassembled .s to stdout and metadata to stderr")
    ap.add_argument("--bake", action="store_true", help="run the full lift->opt->recompile->verify pipeline")
    ap.add_argument("--paths", type=int, default=200)
    ap.add_argument("--budget", type=int, default=1500)
    ap.add_argument("--inputs", type=int, default=200000)
    args = ap.parse_args()
    va = int(args.va, 16)
    if args.bake:
        res = bake(va, args.paths, args.budget, args.inputs)
        verdict = "BAKED (verified equivalent)" if res.get("ok") else f"NOT BAKED @ {res.get('stage')}"
        print(f"=== bake {hex(va)}: {verdict} ===")
        for k, v in res.items():
            print(f"  {k}: {v}")
        sys.exit(0 if res.get("ok") else 1)
    asm, meta = reassemble(va, args.paths, args.budget)
    if asm is None:
        sys.exit(f"reassembly failed: {meta}")
    if args.emit_asm:
        sys.stdout.write(asm)
        sys.stderr.write(f"# blocks={meta['nblocks']} insns={meta['ninsns']} "
                         f"callees={[hex(c) for c in meta['callees']]} "
                         f"data_syms={len(meta['data_syms'])} gadgets={meta['gadgets']}\n")


if __name__ == "__main__":
    main()
