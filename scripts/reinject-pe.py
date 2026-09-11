#!/usr/bin/env python3
# TODO: WIP capstone reinjection tooling -- structurally proven, NOT yet in-game validated.
#       (Comment marker is `#` not `//` because this is Python.) Known gaps before this can
#       validate a reinjected function in the offline game -- see bd memories
#       arxan-antitamper-neuter-for-reinjection-2026-07-01 and
#       phase4-measurement-and-open-items-2026-07-01:
#
#   1. PAYLOAD IS RECOVERED CODE, NOT THE rev.ng-OPTIMIZED OBJECT. The optimized object
#      (bf.rf.rc.obj) carries undefined `_rsp` (rev.ng emulated-stack scratch) + one
#      `revng_undefined__<reg>` helper per uninitialized-at-entry register, and the code
#      actually calls/derefs them -- it's a *verification model*, not standalone machine
#      code. So this injects the clean clang-of-reassembly object (bf_rf.obj) instead.
#      Reinjecting the literal LLVM-optimized payload needs a residue-elimination step
#      that does not exist yet.
#
#   2. A NAIVE STATIC .text PATCH TRIPS ARXAN ANTI-TAMPER. Arxan has ~1428 checksum/
#      anti-tamper stubs that run pre-main; it can detect the changed bytes (crash) and/or
#      re-decrypt/overwrite the region. So DO NOT launch a statically-patched exe expecting
#      a clean result. CLEAN PATH: a DLL loaded pre-entry that calls
#      dearxan::disabler::neuter_arxan() to disable Arxan, THEN applies the thunk redirect as
#      an IN-MEMORY patch (VirtualProtect + write) -- not this on-disk section-append.
#      Adapt /home/banon/projects/dearxan/test_dll (it already calls neuter_arxan) as the
#      harness; add the memory patch in its post-neuter callback.
#
#   3. REAL GLOBALS ARE NOT RESOLVED. Only self-contained functions (data_syms=0, no
#      callees) are safe to inject as-is. A function reading a mutable game global would
#      read a frozen local copy here -- wrong. Needs rip-relative refs pointing at the real
#      game VAs (the bake pipeline's "globals resolution", not yet built).
#
#   4. PROVEN ONLY STRUCTURALLY: valid PE, thunk redirected (E9 rel32 -> injected section),
#      injected bytes decode to the recovered function. NOT validated in-game. The single
#      demo target is 0x140df2230 (self-contained, returns 0). e9tool (built at
#      $CLAUDE_JOB_DIR/tmp/e9patch) is an alternative injector but its PE + arbitrary-code
#      injection path was not driven to completion; this hand-rolled section-append is the
#      working static mechanism.
"""Statically reinject a clean self-contained recovered function into a COPY of
eldenring.exe: append a new PE section with the function's machine code, then
redirect the function's thunk entry (its E9 rel32 jmp) to the injected code.
Only touches the copy. Verifies the output PE structurally."""
import struct, subprocess, os

FUNC_VA = 0x140df2230
OBJ = "/home/banon/er-llvm-spike/inj_140df2230/bf_rf.obj"          # clean recovered code
SRC = "/home/banon/er-llvm-spike/er_copy.exe"                      # a COPY of eldenring.exe
DST = "/home/banon/er-llvm-spike/er_reinjected.exe"

# 1) extract the function's raw machine code (.text of the clean reference object)
code = "/home/banon/er-llvm-spike/inj_code.bin"
subprocess.run(["llvm-objcopy", "--dump-section", f".text={code}", OBJ, "/dev/null"], check=True)
payload = open(code, "rb").read()
print(f"payload = {len(payload)} bytes: {payload.hex()}")

d = bytearray(open(SRC, "rb").read())
e = struct.unpack_from("<I", d, 0x3c)[0]
assert d[e:e+4] == b"PE\x00\x00"
coff = e + 4
nsec = struct.unpack_from("<H", d, coff+2)[0]
optsz = struct.unpack_from("<H", d, coff+16)[0]
opt = coff + 20
image_base = struct.unpack_from("<Q", d, opt+24)[0]
sect_align = struct.unpack_from("<I", d, opt+32)[0]
file_align = struct.unpack_from("<I", d, opt+36)[0]
sizeofheaders = struct.unpack_from("<I", d, opt+60)[0]
secs = opt + optsz

def align(x, a): return (x + a - 1) & ~(a - 1)

# room for a new 40-byte section header?
hdr_end = secs + nsec*40
assert hdr_end + 40 <= sizeofheaders, f"no room for new section header ({hdr_end+40} > {sizeofheaders})"

# last section -> compute new section VA / raw offset
last = secs + (nsec-1)*40
lvs, lva, lraw, lroff = struct.unpack_from("<IIII", d, last+8)
new_va = align(lva + lvs, sect_align)
new_roff = align(len(d), file_align)
raw_sz = align(len(payload), file_align)

# 2) resolve thunk file offset + confirm it's an E9 rel32
rva = FUNC_VA - image_base
for i in range(nsec):
    o = secs + i*40; vs, va, rs, ro = struct.unpack_from("<IIII", d, o+8)
    if va <= rva < va+vs:
        thunk_off = ro + (rva - va); break
assert d[thunk_off] == 0xE9, f"thunk not E9 jmp: {d[thunk_off]:#x}"
old_rel = struct.unpack_from("<i", d, thunk_off+1)[0]
old_target = FUNC_VA + 5 + old_rel
print(f"thunk @ file 0x{thunk_off:x}: E9 rel32 -> 0x{old_target:x} (was Arxan)")

# 3) new E9 rel32 -> injected code at new_va
new_func_va = image_base + new_va
new_rel = new_func_va - (FUNC_VA + 5)
assert -0x80000000 <= new_rel < 0x80000000
struct.pack_into("<i", d, thunk_off+1, new_rel)
print(f"redirected thunk -> injected code at 0x{new_func_va:x} (rel32={new_rel:#x})")

# 4) write new section header
struct.pack_into("<8sIIII", d, hdr_end, b".erinj", len(payload), new_va, raw_sz, new_roff)
struct.pack_into("<IIHHI", d, hdr_end+24, 0, 0, 0, 0, 0x60000020)  # CODE|EXECUTE|READ
struct.pack_into("<H", d, coff+2, nsec+1)                          # NumberOfSections++
struct.pack_into("<I", d, opt+56, align(new_va + len(payload), sect_align))  # SizeOfImage

# 5) append the section raw data (payload padded to file alignment)
d += b"\x00" * (new_roff - len(d))
d += payload + b"\x00" * (raw_sz - len(payload))

open(DST, "wb").write(d)
print(f"\nwrote {DST} ({len(d)} bytes, +1 section '.erinj')")

# 6) structural verify
v = open(DST, "rb").read()
e2 = struct.unpack_from("<I", v, 0x3c)[0]
print("verify: MZ", v[:2] == b"MZ", "/ PE", v[e2:e2+2] == b"PE",
      "/ nsec", struct.unpack_from("<H", v, e2+6)[0],
      "/ thunk now E9->", hex(FUNC_VA+5+struct.unpack_from("<i", v, thunk_off+1)[0]))
