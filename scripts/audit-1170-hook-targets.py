#!/usr/bin/env python3
"""Audit the translated 1.17 detour targets offline, before any of them is installed.

`scripts/verify-rva-map-1170.py` answers "is the code at the mapped address the same function?".
This answers three questions it never asks, each of which corrupts the process rather than
faulting cleanly, and none of which needs the game to run:

  Entry     is the 1.17 target a function entry, or did the signature re-occur mid-function?
            A mapper finds where bytes recur; an inlined copy of a prologue is a perfectly good
            match and a perfectly fatal detour. Evidence is forward-derived: `call`/`jmp rel32`
            instructions elsewhere in the image whose computed destination is exactly this
            address, plus absolute 8-byte pointers to it (vtable slots, jump tables), plus the
            image's own `.pdata` -- which both declares entries and, by declaring none that
            contains the address, identifies an unwindless leaf. Those last two are different
            answers and reading them as one flagged good leaf targets as mid-function.
            An earlier version of this check decoded backwards from the address looking for
            padding or a terminator, and was deleted: run against the 1.16.2 image at the 27
            addresses this project has hooked successfully for months, it called 20 of them
            mid-function. Backward decoding desynchronises, and a de-Arxan'd image does not
            carry the int3 padding the check assumed. A test that fails on known-good input is
            not a strict test, it is a broken one.
  Patch     do the whole instructions MinHook must relocate fit, and does anything jump into the
            five bytes it overwrites? A short jump landing inside the patch returns into a JMP's
            operand bytes -- an execute-fault into an address in no module, with no unwind.
  Overlap   do two targets land within 16 bytes of each other? The second MH_CreateHook then
            reads a prologue the first one has already replaced with a JMP, and trampolines it.

    python3 scripts/audit-1170-hook-targets.py              # audit the translated pairs
    python3 scripts/audit-1170-hook-targets.py --calibrate  # same checks on 1.16.2 known-good
    python3 scripts/audit-1170-hook-targets.py --selftest
"""

import argparse
import importlib.util
import os
import re
import struct
import subprocess
import sys
import tempfile

try:
    import capstone
except ImportError:  # provisioned ephemerally; there is no system pip here
    os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import function_extent  # noqa: E402 - repo-local; see the sys.path line above
import rva_admission  # noqa: E402 - repo-local, and the sys.path line above is what makes it work

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
def _deobf_image(env_var: str, filename: str) -> str:
    """Where the flat de-Arxan'd image actually is, from any checkout of this repo.

    Three answers in priority order, and the middle one is the reason this exists:

    1. `$<env_var>`, for a copy kept somewhere else entirely.
    2. Beside this checkout -- the developer case, and the only one the plain
       `os.path.join(ROOT, ...)` this replaced could express.
    3. Beside the main checkout, when we are running from a `git worktree`. A worktree is a
       separate directory with its own `scripts/`, so `ROOT` points at a tree where these
       gitignored multi-hundred-MB artifacts were never copied. `--git-common-dir` names the
       original checkout's `.git`, whose parent is the tree they do live beside.
       `scripts/disas-deobf.sh` has resolved them this way for a while; the Python gates did not,
       so `check.sh` died with `FileNotFoundError` on a path that looks right the moment an agent
       ran it from a worktree.

    Falls back to the local path when every lookup misses, so the error message still names the
    place a developer would expect the file to be.
    """
    override = os.environ.get(env_var)
    if override:
        return override
    local = os.path.join(ROOT, filename)
    if os.path.exists(local):
        return local
    try:
        common = subprocess.run(
            ["git", "-C", ROOT, "rev-parse", "--path-format=absolute", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return local
    shared = os.path.join(os.path.dirname(common), filename) if common else ""
    return shared if shared and os.path.exists(shared) else local

IMAGE_1170 = _deobf_image("ER_DEOBF_BIN_1170", "eldenring-deobf-1.17.bin")
IMAGE_1162 = _deobf_image("ER_DEOBF_BIN", "eldenring-deobf.bin")
VERIFIED = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.verified.tsv")
NEEDED_VERIFIED = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.needed-verified.tsv")
# Both ledgers, because `emit_address_map` builds DETOUR_SAFE_1162_TO_1170 from BOTH:
#
#     let mut detour_safe: Vec<(u32, u32)> = verified_detourable;          // VERIFIED_MAP
#     detour_safe.extend(detourable_pairs(&...join(NEEDED_VERIFIED_MAP))); // NEEDED_VERIFIED_MAP
#
# Until 2026-08-31 this file read only the first of the two, so it judged ~100 of the ~450
# addresses that actually reach a detour and reported the other ~350 as if they had been
# examined. That is the same defect one level up as the vacuity bug `rva_admission` exists to
# stop: "no admitted row is mid-function" is a claim about the rows you looked at, and half the
# table was outside the quantifier. It is also exactly where the 2026-08-31 incident lived --
# `0x140001000` (er-hook's `FIRST_SECTION_RVA`, a PE section-boundary constant harvested as
# though it were a function address) is a needed-verified row, and the mid-function address the
# gate flagged that day, `0x140001050`, is 0x10 bytes into its neighbour.
DETOUR_SAFE_LEDGERS = (VERIFIED, NEEDED_VERIFIED)
BASE = 0x140000000
# er-game-base/build.rs admits exactly these rows; the audit must see the same set it will
# install, so the admission rule is read out of build.rs rather than transcribed here.
BUILD_RS = os.path.join(ROOT, "crates", "er-game-base", "build.rs")
# MinHook writes a 5-byte relative JMP over the entry and relocates whole instructions out of it.
PATCH_BYTES = 5
# Two entries closer than this share MinHook's patch/relocation window.
OVERLAP_BYTES = 16
# Enough of the function to see the short-range branches that could target its own prologue.
BRANCH_SCAN_BYTES = 0x400

# --- MinHook's own trampoline rules, ported ---------------------------------------------------
# Every constant and branch below is read out of `vendor/minhook/src/trampoline.c`
# (`CreateTrampolineFunction`, `IsCodePadding`), which is the code that will actually be asked to
# install these detours. Where this port cannot match it exactly, it is noted at the site.
#
# The rule that was missing, and what it cost: the previous accumulator had no stop condition but
# its own byte count, so a function that ended inside the five bytes was scored on whatever
# followed it -- padding, then the next function. Two real 1.17 addresses, `er-reload-trace`'s own
# former hook targets, came back "relocatable" on bytes belonging to their neighbours:
# 0x14067a520 (`8b c1 / c3`, a 3-byte leaf, reported 8B) and 0x14067a420 (`41 89 00 / c3`, 4
# bytes, reported 11B). Same decode-past-the-end bug class as the 12 false DIVERGES verdicts and
# 31 false shape-DIFFs of 2026-08-30.
JMP_REL_SIZE = 5  # sizeof(JMP_REL): the relative JMP MinHook writes over the entry
JMP_REL_SHORT_SIZE = 2  # sizeof(JMP_REL_SHORT): the 2-byte hop it writes instead when patching above
JMP_ABS_SIZE = 14  # sizeof(JMP_ABS), the form a relocated JMP grows into
CALL_ABS_SIZE = 16  # sizeof(CALL_ABS)
JCC_ABS_SIZE = 16  # sizeof(JCC_ABS)
MAX_TRAMPOLINE_IPS = 8  # ARRAYSIZE(TRAMPOLINE.oldIPs) -- more instructions than this and it gives up


def is_code_padding(blob, off, size):
    """`IsCodePadding`: `size` bytes of one filler, from {0x00, 0x90, 0xCC}.

    Note what this does not do, because getting it wrong would build a rule on sand: it does not
    look for a particular filler byte. MSVC pads with 0xCC in some places and 0x90 in others and
    does not make the same choice in 1.16.2 and 1.17, so `0xCC` is not the signal and neither is
    `0x90` -- the signal is a uniform run of any one of the three. MinHook needs the run uniform
    because it overwrites the whole of it.
    """
    if size <= 0:
        return True
    if off < 0 or off + size > len(blob):
        return False
    filler = blob[off]
    if filler not in (0x00, 0x90, 0xCC):
        return False
    return all(blob[off + i] == filler for i in range(1, size))


def _branch_target(insn):
    """Absolute destination of a relative branch, or None if the operand is not an immediate."""
    for op in insn.operands:
        if op.type == capstone.x86.X86_OP_IMM:
            return op.imm
    return None


def build_rs_admission():
    """Every admission rule read out of er-game-base/build.rs. Parsed, never transcribed.

    This audit's entire claim is that it judges the same address set `build.rs` is about to
    install, and a copy of build.rs's constants makes that claim only until someone edits one of
    them. That is not hypothetical: on 2026-08-30 the verifier split `IDENTICAL` into
    `BYTE-IDENTICAL` / `IDENTICAL-WHOLE` / `IDENTICAL-LEAF`, build.rs was updated, and this filter
    -- still asking for the single old word -- matched nothing. Nothing went red. The calibration
    printed `0 of 0 need a look` and the selftest's `assert bad == 0` passed over an empty set, so
    the gate stayed green while checking no address at all. A sibling agent's simulator hit the
    same drift from the other side and reported detour as 42 instead of 374.

    The parse now lives in one place for the whole repo (`scripts/rva_admission.py`, which defers
    in turn to `check-1170-translation-collisions.py`), because four audits each carrying their own
    copy of the parse is the same duplication one level up: this file went on comparing against a
    hard-coded `"IDENTICAL"` for the prefix arm even after it had learned to read the exhaustive
    list, and that literal is spelled in a match arm rather than a constant, so only a parse of the
    arm itself can track a rename of it.
    """
    return rva_admission.rules()


def rows(path=None, rule_set=None):
    """(1.16.2 VA, 1.17 VA) exactly as `er-game-base/build.rs::detourable_pairs` filters them.

    With no `path`, this is the whole detour table: both ledgers `emit_address_map` unions into
    `DETOUR_SAFE_1162_TO_1170`. See [`DETOUR_SAFE_LEDGERS`] for why reading one of them was a
    hole rather than a shortcut.

    `path`/`rule_set` exist so the selftest can point the same filter at a synthetic table and
    watch it go red; a gate whose real input is its only input cannot be shown to have teeth.
    """
    rule_set = rule_set or build_rs_admission()
    tables = [path] if path else list(DETOUR_SAFE_LEDGERS)
    out = set()
    for table in tables:
        # `admit_rows` refuses outright when a non-empty table yields no admitted rows -- the
        # vacuity this gate spent part of 2026-08-30 wearing as a green tick. Per ledger, not over
        # the union: a union stays comfortably non-empty while one of its two halves silently
        # stops matching, which is the same green tick with an extra step in front of it.
        admitted, _unknown = rva_admission.admit_rows(
            table,
            rule_set,
            label=f"detourable rows of {os.path.relpath(table, ROOT)}",
        )
        # A set, because the table records provenance as well as addresses and the same pair can be
        # written twice by two agents who found it two ways -- 0x1409b72b0 and 0x1408c47c0 each
        # appear with two different "how it was mapped" notes, and the two ledgers overlap outright.
        # Judged as a list they became two targets zero bytes apart, and the overlap check dutifully
        # reported each as colliding with itself.
        out.update((int(f[0], 16), int(f[1], 16)) for f in admitted)
    return sorted(out, key=lambda pair: pair[1])


def xref_targets(blob, wanted):
    """Addresses in `wanted` that something in the image CALLs, JMPs to, or stores a pointer to.

    One linear pass for the relative forms (every 0xE8/0xE9 byte is treated as a candidate
    opcode and its rel32 resolved -- false candidates resolve outside the image or miss the
    set), plus a search for the little-endian 8-byte encoding, which is how a vtable slot or a
    jump table names a function.

    The pointer search is shared, not per-address, and that is what makes the cost independent
    of how many rows the ledgers hold. Searching each address's own 8-byte needle meant one
    98 MB scan per address: fine at the 100 rows this file used to judge, 34s at the 422 it
    judges now, and past `audit-selftest-vacuity.py`'s 25s per-script cap -- i.e. widening the
    scope to close a coverage hole would have made a different gate go red, which is how a hole
    gets argued for on cost. Every needle for a VA in this image ends in the same five bytes
    (`<0x40|0x41|0x42> 01 00 00 00`), so scanning for those few suffixes and reconstructing the
    low three bytes at each hit finds exactly the same occurrences -- unaligned ones included --
    in a fixed handful of passes. Asserted equivalent against the per-address form over the live
    address set before replacing it.
    """
    hits = {va: {"call": 0, "jmp": 0, "ptr": 0} for va in wanted}
    limit = len(blob)
    for opcode, kind in ((0xE8, "call"), (0xE9, "jmp")):
        pos = blob.find(bytes([opcode]))
        while pos != -1 and pos + 5 <= limit:
            rel = int.from_bytes(blob[pos + 1 : pos + 5], "little", signed=True)
            dest = BASE + pos + 5 + rel
            if dest in hits:
                hits[dest][kind] += 1
            pos = blob.find(bytes([opcode]), pos + 1)
    # The distinct high halves actually present, so an address outside 0x140-0x142 (or a future
    # image with a fourth) still gets its own pass rather than being silently skipped.
    for suffix in {va.to_bytes(8, "little")[3:] for va in wanted}:
        pos = blob.find(suffix)
        while pos != -1:
            if pos >= 3:
                va = int.from_bytes(blob[pos - 3 : pos + 5], "little")
                if va in hits:
                    hits[va]["ptr"] += 1
            pos = blob.find(suffix, pos + 1)
    return hits


def _per_address_pointer_scan(blob, wanted):
    """The straightforward one-scan-per-address pointer search, kept as an equivalence fixture.

    `xref_targets` replaced it with a shared-suffix scan for speed, and a faster thing that
    quietly finds fewer references would turn entry-OK rows into mid-function ones -- the exact
    false positive this file was rebuilt to stop making. So the obvious implementation stays here
    and the selftest asserts the two agree, rather than the docstring asserting it once.
    """
    hits = {va: 0 for va in wanted}
    for va in wanted:
        needle = va.to_bytes(8, "little")
        pos = blob.find(needle)
        while pos != -1:
            hits[va] += 1
            pos = blob.find(needle, pos + 1)
    return hits


def pdata_functions(blob):
    """`(starts, spans)` from the image's own `.pdata`: declared entry RVAs, and `(begin, end)`."""
    pe = struct.unpack_from("<I", blob, 0x3C)[0]
    nsec = struct.unpack_from("<H", blob, pe + 6)[0]
    optsz = struct.unpack_from("<H", blob, pe + 20)[0]
    off = pe + 24 + optsz
    # Section header layout: name[8], VirtualSize, VirtualAddress, SizeOfRawData, PointerToRawData.
    entry = next(
        (
            blob[off + i * 40 : off + (i + 1) * 40]
            for i in range(nsec)
            if blob[off + i * 40 : off + i * 40 + 8].rstrip(b"\0") == b".pdata"
        ),
        None,
    )
    if entry is None:
        return frozenset(), []
    vsz, vaddr, rsz, _ = struct.unpack_from("<IIII", entry, 8)
    size = max(vsz, rsz)
    starts, spans = set(), []
    for at in range(vaddr, vaddr + size, 12):
        begin, end, _unwind = struct.unpack_from("<III", blob, at)
        if begin and end > begin:
            starts.add(begin)
            spans.append((begin, end))
    spans.sort()
    return frozenset(starts), spans


def pdata_entry_starts(blob):
    """Every RVA the image's own `.pdata` declares as a function start."""
    return pdata_functions(blob)[0]


# The extent rules live in `scripts/function_extent.py` -- One implementation for the whole repo,
# because a second one is the next divergence bug. They were written here and moved out when four
# more tools needed the same answer; these names stay so this file's own call sites and docstrings
# keep reading the way the incident reports that produced them do.
_inside_declared_function = function_extent.inside_declared_function
declared_functions = function_extent.declared_functions
verify_rules = function_extent.verify_rules


def entry_verdict(hit, pdata_starts=None, va=None, pdata_spans=None):
    """Is this address a function entry, by any positive evidence?

    Three kinds of evidence, and each was missing at some point on 2026-08-30:

      1. Something names it -- a `call`, a `jmp`, or a stored pointer. Direct and obvious.
      2. The image's own `.pdata` declares a function to start there. This is the authority, not a
         hint: it is the binary telling you where its functions begin, and the unwinder depends on
         it being right.
      3. `.pdata` declares no function containing it either. That is not the same as (2) failing,
         and reading the two as one answer is what this check got wrong.

    Judging on (1) alone reads "reached only indirectly" as "mid-function", which is a false
    positive with real cost -- a correct hook target gets flagged and the gate goes red.
    `MMS_CHILD_CLEANUP_RVA` (1.16.2 0xaf5750) was flagged exactly this way while `.pdata` declares
    an exact function entry of size 0x68 there in both builds. Plenty of functions are reached
    only through a vtable or a runtime-resolved pointer and have no literal reference anywhere.

    Case (3) is the leaf, and it is the whole point of the distinction. The x64 ABI omits unwind
    data for a function that allocates no stack and calls nothing, so ELDEN RING's small getters
    have no `.pdata` entry at all -- and `er-game-base/build.rs` admits exactly those through its
    `NEITHER-ENTRY` clause, under the `IDENTICAL-LEAF` verdict. Answering them "mid-function"
    conflates `inside somebody else's function`, which is fatal, with `inside nobody's`, which is
    ordinary. `0x1407add70` and `0x140c90080` were both flagged on that conflation.
    """
    total = hit["call"] + hit["jmp"] + hit["ptr"]
    if total:
        return True, f"{hit['call']} call, {hit['jmp']} jmp, {hit['ptr']} ptr"
    if pdata_starts is not None and va is not None and (va - BASE) in pdata_starts:
        return True, "no literal reference, but .pdata declares a function entry here"
    if pdata_spans is not None and va is not None:
        enclosing = _inside_declared_function(va - BASE, pdata_spans)
        if enclosing is None:
            return True, (
                "no literal reference and no .pdata entry, but no declared function contains it "
                "either -- an unwindless leaf, not a mid-function landing"
            )
        return False, (
            f"it is 0x{va - BASE - enclosing[0]:x} bytes INSIDE the function .pdata declares at "
            f"0x{BASE + enclosing[0]:x}"
        )
    return False, "nothing references it and .pdata declares no entry"


def instruction_boundary(blob, va, enclosing):
    """Is `va` even an instruction boundary of the function `.pdata` declares at `enclosing`?

    This is the DISCRIMINATOR the mid-function flag was missing, and without it that flag hands
    the reader a disjunction it cannot resolve. `entry_verdict` answering "inside a declared
    function" has two causes with opposite fixes:

      * the check is wrong -- `.pdata` merged a tail, or declared an extent covering a second
        real entry, so a perfectly good target reads as interior. That is not hypothetical: the
        entry check this file replaced called 20 of the project's 27 known-good hook targets
        mid-function, and a boundary source can be wrong the same way again; or
      * the data is wrong -- somebody put an address in a detour ledger that is not a function.

    Decoding forward from the declared start separates them, because an address that is not on an
    instruction boundary cannot be a function entry in any build, under any boundary source, no
    matter how wrong `.pdata` is. There is nothing to argue about and nothing to loosen: MinHook
    would relocate the tail of one instruction and the head of the next, and the trampoline would
    return into the middle of an operand.

    Measured on the address that prompted this (2026-08-31): 1.16.2 `0x140001050` sits 4 bytes
    into the 7-byte `lea rdx, [rip + 0x30a5c75]` at `0x14000104c`, inside `0x140001040`
    (`.pdata` 0x1040..0x111c; Ghidra's independent analysis says entry 0x140001040, body end
    0x14000111b -- three sources, one answer).

    Returns `(on_boundary, detail)`. `on_boundary` is None when the decode desynchronised or ran
    off the end before reaching `va`, which is an answer about the decode and must not be read as
    either verdict.
    """
    start = BASE + enclosing[0]
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = False
    previous = None
    for insn in md.disasm(blob[enclosing[0] : enclosing[1]], start):
        if insn.address == va:
            return True, f"on an instruction boundary of 0x{start:x} ({insn.mnemonic} {insn.op_str})"
        if insn.address > va:
            return False, (
                f"NOT an instruction boundary: it is {va - previous.address} bytes into the "
                f"{previous.size}-byte `{previous.mnemonic} {previous.op_str}` at "
                f"0x{previous.address:x}"
            )
        previous = insn
    return None, f"the decode of 0x{start:x} did not reach it -- boundary unknown"


def trampoline_walk(blob, va):
    """MinHook's own trampoline decision, and how much of the function it consumes.

    `(ok, relocated_bytes, patched_above, reason)`. A port of `CreateTrampolineFunction` from
    `vendor/minhook/src/trampoline.c` -- the copy that will be asked to install these detours --
    rather than a summary of it. The branch order, the `jmpDest` internal-branch tracking, the
    outright loop refusal and the padding fallbacks are all its own. Reading the C was not
    optional: a hand-reasoned version written first got two rules wrong in opposite directions,
    refusing five 1.16.2 addresses this project hooks successfully today (it stopped the byte
    count at a relocated `jmp` where MinHook counts through it) while missing that a short
    function can still be hooked when uniform padding follows it.

    `relocated_bytes` is `oldPos` at the moment the walk finishes: the bytes of the original
    function copied into the trampoline, and therefore also the offset the trampoline's trailing
    jump returns into. It is split out from `patch_safe` so that
    `verify-rva-map-1170.py::PATCH_SITE_IDENTICAL` can ask where the relocated region ends
    without re-deriving MinHook's rules a second time, badly. It is meaningless when `ok` is
    False and is returned as 0 there.
    """
    off = va - BASE
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    old_pos = 0  # MinHook's oldPos: bytes of the original function consumed so far
    jmp_dest = 0  # furthest destination of a branch that stays inside the five patched bytes
    relocated = 0  # bytes actually copied out (oldPos at the moment the walk finished)
    finished = False
    n_ip = 0
    while not finished:
        addr = va + old_pos
        decoded = list(md.disasm(blob[off + old_pos : off + old_pos + 16], addr, count=1))
        if not decoded:
            # HDE's `F_ERROR`. MinHook returns false without touching the target.
            return False, 0, False, f"undecodable byte at 0x{addr:x}, {old_pos}B in"
        insn = decoded[0]
        length = insn.size
        copy_size = length
        opcode, opcode2, modrm = insn.opcode[0], insn.opcode[1], insn.modrm
        if old_pos >= JMP_REL_SIZE:
            # Past the patch window: MinHook stops copying and jumps back into the original.
            finished = True
            relocated = old_pos
            copy_size = JMP_ABS_SIZE
        elif (modrm & 0xC7) == 0x05:
            # RIP-relative operand. Rewritten in place at the same length; an indirect JMP
            # (FF /4) also ends the function.
            if opcode == 0xFF and (modrm >> 3) & 7 == 4:
                finished = True
                relocated = old_pos + length
        elif opcode == 0xE8:
            copy_size = CALL_ABS_SIZE  # direct relative call, widened to an absolute one
        elif (opcode & 0xFD) == 0xE9:
            # Direct relative JMP (EB or E9). One that lands inside the patched bytes is copied
            # as-is and merely raises jmpDest; one that leaves ends the function.
            dest = _branch_target(insn)
            if dest is not None and va <= dest < va + JMP_REL_SIZE:
                jmp_dest = max(jmp_dest, dest)
            else:
                copy_size = JMP_ABS_SIZE
                finished = addr >= jmp_dest
                if finished:
                    relocated = old_pos + length
        elif (opcode & 0xF0) == 0x70 or (opcode & 0xFC) == 0xE0 or (opcode2 & 0xF0) == 0x80:
            # Direct relative Jcc, and the loop family that shares its encoding shape.
            dest = _branch_target(insn)
            if dest is not None and va <= dest < va + JMP_REL_SIZE:
                jmp_dest = max(jmp_dest, dest)
            elif (opcode & 0xFC) == 0xE0:
                # LOOPNZ/LOOPZ/LOOP/JCXZ/JECXZ leaving the window: MinHook returns false outright,
                # because an 8-bit displacement cannot be widened to reach out of the trampoline.
                return False, 0, False, (
                    f"{insn.mnemonic} at 0x{addr:x} branches out of the patch window, and "
                    "MinHook refuses LOOPNZ/LOOPZ/LOOP/JCXZ/JECXZ rather than relocating them"
                )
            else:
                copy_size = JCC_ABS_SIZE
        elif (opcode & 0xFE) == 0xC2:
            # RET (C2 or C3). Ends the function unless an earlier branch reaches past it.
            finished = addr >= jmp_dest
            if finished:
                relocated = old_pos + length
        if addr < jmp_dest and copy_size != length:
            return False, 0, False, (
                f"the instruction at 0x{addr:x} has to change length to relocate, and a branch "
                "inside the patch window jumps past it"
            )
        n_ip += 1
        if n_ip > MAX_TRAMPOLINE_IPS:
            return False, 0, False, f"more than {MAX_TRAMPOLINE_IPS} instructions in the patch window"
        old_pos += length
    patched_above = False
    if old_pos < JMP_REL_SIZE and not is_code_padding(blob, off + old_pos, JMP_REL_SIZE - old_pos):
        # The function is shorter than the JMP and is not followed by padding to spill into.
        if old_pos < JMP_REL_SHORT_SIZE and not is_code_padding(
            blob, off + old_pos, JMP_REL_SHORT_SIZE - old_pos
        ):
            return False, 0, False, f"only {old_pos}B long, with no padding after it for even a short jump"
        # Last resort: put the long jump in the padding above the entry and leave a 2-byte hop.
        # `IsExecutableAddress` is not modelled -- everything in a flat de-Arxan'd .text image
        # would answer yes -- so this arm is as permissive as the port gets. It matters only for
        # a function under five bytes preceded by five uniform padding bytes.
        if not is_code_padding(blob, off - JMP_REL_SIZE, JMP_REL_SIZE):
            return False, 0, False, (
                f"only {old_pos}B long, the bytes after it are not padding, and the {JMP_REL_SIZE}B "
                f"above 0x{va:x} are not padding either, so there is nowhere to put the jump"
            )
        patched_above = True
    return True, relocated, patched_above, ""


def body_end(blob, va):
    """The RVA one past the last byte of the function at `va`, or None if it cannot be told.

    `scripts/function_extent.body_end`, with this file's own scan cap. Kept as a named local so
    the reason it exists stays next to the code that needed it.

    Without this the branch scan reads the next function, and not even in phase. `patch_safe`
    used to decode a flat `BRANCH_SCAN_BYTES` from the entry, which for a 14-byte leaf is 0x3f2
    bytes of somebody else's code entered at an arbitrary offset. The de-Arxan'd images make that
    worse than a plain over-read: the gaps between functions hold the deobfuscator's leftover
    bytes rather than a uniform `cc`/`90` run, so the decode desynchronises on the way out and
    manufactures instructions that were never assembled -- including branches.

    Measured, and the whole reason this exists (2026-08-31). 1.16.2 `0x14067ac90`
    (`GameMan::SetSaveState`: `mov rax,[rip+0x36eec81] / mov [rax+0xb80],ecx / ret`, 14 bytes) is
    followed by `90 83` before the next leaf at `0x14067aca0`. Decoding through that pair yielded
    `or dword ptr [rax - 0x75], 5` at 0x14067ac9f and then a phantom `jno 0x14067ac91` at
    0x14067aca3 -- a branch into the patch operand, conjured out of two bytes of padding -- and
    the gate went red on a row four independent derivations had just confirmed. Its 1.17
    counterpart `0x14067bae0` is the same fourteen bytes; its junk pad byte happens to be `28`
    instead of `83`, so it desynchronised into a harmless `sub` and that side stayed green. Which
    image a correct row failed on was decided by one leftover byte.

    Measured over the 425 rows both detour ledgers admit: 359 declared, 0 enclosed, 66 decoded
    leaves, 0 unknown -- in both images. The None arm is a fallback nothing in the current tables
    takes.
    """
    return function_extent.body_end(blob, va, limit=BRANCH_SCAN_BYTES)


def patch_safe(blob, va):
    """Will MinHook build a trampoline here, and does anything jump into the patched bytes?

    The first half is `trampoline_walk`, the port of MinHook's own `CreateTrampolineFunction`.

    The second half is not MinHook. MinHook never looks at the rest of the body, so it cannot see
    a branch from later in the function landing on bytes 1..4 of the prologue -- which after the
    patch are the operand of a JMP. That control transfer faults into an address in no module,
    with no unwind. It is checked here because nothing else checks it.

    The scan stops at the end of the function. Only this function's own branches can be read as
    evidence about this function's prologue; bytes past its end belong to someone else, and in
    these images the decode does not even arrive at them in phase. See `body_end` for the row that
    proved it and `_unbounded_branch_scan` for the shape this replaced.
    """
    off = va - BASE
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    ok, relocated, patched_above, why = trampoline_walk(blob, va)
    if not ok:
        return False, why
    # Exactly the bytes MinHook overwrites, and no more. Byte 0 is the JMP's own opcode, and a
    # branch there is the ordinary case -- it goes through the hook. Bytes 1..4 are the operand,
    # and a branch into them executes an address. Bytes at 5 and beyond are not written even when
    # more than five were relocated, so they still hold their original instructions and a branch
    # to one of them merely bypasses the hook. The previous window ran to the relocated extent
    # and flagged those; on the current tables that difference is one address, 1.17 0x14067b010,
    # where a `jrcxz` at +0x4e targets +6.
    hot = range(va + 1, va + JMP_REL_SIZE)
    end = body_end(blob, va)
    # BRANCH_SCAN_BYTES survives as a cap, not as the window: a body longer than it is read only
    # that far (a branch to a prologue from 1KB away has never been seen), and an extent that
    # cannot be determined at all falls back to it rather than to nothing.
    limit = off + BRANCH_SCAN_BYTES if end is None else min(end, off + BRANCH_SCAN_BYTES)
    body = blob[off : max(limit, off)]
    for insn in md.disasm(body, va):
        if capstone.CS_GRP_JUMP not in insn.groups:
            continue
        for op in insn.operands:
            if op.type == capstone.x86.X86_OP_IMM and op.imm in hot:
                return False, f"{insn.mnemonic} at 0x{insn.address:x} targets 0x{op.imm:x}"
    if patched_above:
        return True, f"{relocated}B relocatable, via a jump patched into the padding ABOVE the entry"
    return True, f"{relocated}B relocatable"


# `--promote` and its two helpers were deleted on 2026-08-31, with the file they wrote.
#
# `wider_rows()` fed this audit `docs/recon/rva-map-1162-to-1170.needed.tsv` and
# `docs/recon/rva-map-1162-to-1170.data.tsv` -- the GLOBALS table -- and `promote()` wrote the
# survivors to `docs/recon/rva-1170-detour-audited.tsv` with prologue verdicts like `6B
# relocatable` beside them. That was not a caveat about the output; it was a category error in
# the input, and it produced the most extreme instance of the decode-past-the-end class this
# repo has found:
#
#   * Of the 85 rows promoted on the UNWINDLESS-leaf clause, all 85 named non-EXECUTABLE memory.
#     The clause could never have fired on a real leaf and could never have missed a global,
#     because `.pdata` declares no enclosing function for a `.data` address for exactly the same
#     reason it declares none for an unwindless leaf. Handed the globals table, `entry_verdict`
#     reads every global as a leaf, by construction.
#   * 87 of the 444 rows named non-executable destinations (`.data` 61, `.rdata` 26).
#   * The four "leaf functions" it would have newly promoted are 24 bytes of `00` in both images
#     -- the `.data` virtual tail past its raw size. `00 00` decodes as `add byte ptr [rax], al`;
#     three of those is the `6B`.
#   * The single `loopne` refusal was right by accident: `0x142aa5a35`'s `0xe0` is the low byte
#     of a stored pointer (`0x140745be0`) in an `.rdata` vtable. Its neighbour in the same table
#     stores `0x140735100`, low byte `0x00`, and was promoted. Adjacent slots of one vtable,
#     opposite detour verdicts, decided by the low byte of the address stored there.
#
# The lesson is now enforcement in two places rather than prose here: `function_extent.body_end`
# refuses a VA outside an executable section before it will resolve any extent, and
# `scripts/check-ledger-section-kind.py` holds every ledger to the section kind its consumer
# assumes. The tombstone rule (R3) that guarded against `--promote` regenerating the file was
# deleted with this command, because a rule guarding against nothing is worse than no rule.


def audit(image_path, pairs, column, label):
    """Run the three checks over `pairs`, judging the address in `column` of each."""
    blob = open(image_path, "rb").read()
    targets = sorted({pair[column] for pair in pairs})
    hits = xref_targets(blob, set(targets))
    starts, spans = pdata_functions(blob)
    bad = []
    previous = None
    print(f"{len(pairs)} pairs, judging the {label} address against "
          f"{os.path.basename(image_path)}\n")
    for pair in sorted(pairs, key=lambda p: p[column]):
        va = pair[column]
        flags = []
        ok, why = entry_verdict(hits[va], starts, va, spans)
        if not ok:
            # Say which of the flag's two causes it is, rather than leaving the reader to guess
            # and reach for the checker. See `instruction_boundary`.
            enclosing = _inside_declared_function(va - BASE, spans)
            if enclosing is not None:
                _on_boundary, boundary_why = instruction_boundary(blob, va, enclosing)
                why = f"{why}; {boundary_why}"
            flags.append(f"MID-FUNCTION ({why})")
        detail = why
        ok, why = patch_safe(blob, va)
        if not ok:
            flags.append(f"PATCH-UNSAFE ({why})")
        else:
            detail += f", {why}"
        if previous is not None and va - previous < OVERLAP_BYTES:
            flags.append(f"OVERLAP (0x{previous:x} is {va - previous}B away)")
        previous = va
        arrow = f"0x{pair[0]:x} -> 0x{pair[1]:x}"
        print(f"{arrow}  {'; '.join(flags) if flags else 'ENTRY-OK'}  [{detail}]")
        if flags:
            bad.append((arrow, "; ".join(flags)))
    print(f"\n{len(bad)} of {len(pairs)} need a look.")
    return bad


# --- regression fixtures for the bound above -------------------------------------------------
def _unbounded_walk(blob, va):
    """The buggy `patch_safe` walk this file shipped until 2026-08-30, kept as a fixture.

    It adds instruction sizes with no stop condition but the byte count, so it reads straight
    through a `ret` into the padding and the function after it. It is preserved -- and asserted
    against -- so the cases below cannot quietly stop discriminating: an address only earns a
    place in `PATCH_SAFE_CASES` as a refusal if this thing accepts it. That is the mutation test,
    made permanent instead of run once by hand. Reverting `patch_safe` to this shape makes the
    selftest fail, which is the whole point of keeping it.
    """
    off = va - BASE
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    covered = 0
    for insn in md.disasm(blob[off : off + 32], va):
        covered += insn.size
        if covered >= PATCH_BYTES:
            break
    return covered >= PATCH_BYTES, f"{covered}B"


# Addresses whose MinHook answer is settled by the bytes themselves. The three refusals are real
# 1.17 addresses this audit approved until 2026-08-30 -- the first two are `er-reload-trace`'s own
# former hook targets, and the byte counts in their old verdicts are longer than the functions.
# The acceptances are the control: a bound that refuses everything passes the refusals for free,
# so genuine prologues have to keep getting through, in both builds.
#
# `discriminating` marks the cases the old unbounded walk got wrong. Every refusal must be one,
# or it is not testing the new bound.
PATCH_SAFE_CASES = (
    ("1.17", 0x14067A520, False, True, "3-byte leaf `mov eax, ecx / ret`, once read as 8B"),
    ("1.17", 0x14067A420, False, True, "4-byte leaf `mov [r8], eax / ret`, once read as 11B"),
    ("1.17", 0x1407ACB00, False, True, "`push rbp` then a LOOPNE out of the window, which"
                                       " MinHook refuses outright"),
    ("1.17", 0x1407AE8C0, True, False, "a real 1.17 prologue: the bound must still let this"
                                       " through"),
    ("1.16.2", 0x1407AE8C0, True, False, "and the same address in the calibration build"),
    ("1.16.2", 0x140764B80, True, False, "`add rcx, 8` then a tail `jmp`: MinHook counts THROUGH"
                                         " the relocated jump, so this is 9B and hookable"),
)


def _unbounded_branch_scan(blob, va):
    """The buggy branch scan `patch_safe` shipped until 2026-08-31, kept as a fixture.

    A flat `BRANCH_SCAN_BYTES` from the entry with no notion of where the function ends. It is
    preserved for exactly the reason `_unbounded_walk` is: a case only earns a place in
    `BRANCH_SCAN_CASES` as a DISCRIMINATOR if this thing gets it wrong, so "the bound matters" is
    measured on every run instead of asserted once in a comment. Reverting `patch_safe` to this
    shape makes the selftest fail.
    """
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    off = va - BASE
    hot = range(va + 1, va + JMP_REL_SIZE)
    for insn in md.disasm(blob[off : off + BRANCH_SCAN_BYTES], va):
        if capstone.CS_GRP_JUMP not in insn.groups:
            continue
        for op in insn.operands:
            if op.type == capstone.x86.X86_OP_IMM and op.imm in hot:
                return False, f"{insn.mnemonic} at 0x{insn.address:x} targets 0x{op.imm:x}"
    return True, "no branch into the patched bytes"


# The branch-scan half of `patch_safe`, on addresses whose answer the bytes already settle.
# `discriminating` marks the ones the unbounded scan got wrong, and the pair of leaves below is
# the point: it is the same function in the two builds, and the unbounded scan refused one of them
# and passed the other on the strength of a byte that is padding in both.
BRANCH_SCAN_CASES = (
    ("1.16.2", 0x14067AC90, True, True,
     "GameMan::SetSaveState, a 14-byte leaf: the junk byte `83` after its `ret` desynchronises an"
     " unbounded decode into a phantom `jno 0x14067ac91` -- a branch into the patch operand that"
     " was never assembled"),
    ("1.17", 0x14067BAE0, True, False,
     "the SAME function in 1.17, whose junk byte is `28` and desynchronises into a harmless `sub`."
     " One leftover byte decided which build the row failed on"),
    ("1.17", 0x1407AE8C0, True, False,
     "a real 802-byte prologue: the bound must not turn the scan into a rubber stamp"),
)


def selftest():
    """Calibrate on input whose answer is already known, then assert the deliberate negatives.

    Most of the 1.16.2 source addresses are hooked successfully today, so a check that calls the
    long-standing ones mid-function or unpatchable is broken -- which is exactly how the previous
    entry check was caught, and how the first draft of the current patch check was caught. That
    inference is not a licence to read every calibration failure as "the check is broken": the set
    is every row both detour ledgers admit, it grows, and a new row can be genuinely bad. The
    positive control below plants exactly such a row and requires the gate to catch it.

    The negative controls are an address two bytes into a known entry, and the byte-dictated
    `PATCH_SAFE_CASES`.
    """
    pairs = rows()
    # An empty calibration passes `bad == 0` while checking nothing. That is how this gate spent
    # part of 2026-08-30 green: the verifier renamed its verdicts and `rows()` matched none of
    # them. There is no plausible state of these tables in which zero rows are detourable.
    assert pairs, (
        "neither "
        + " nor ".join(os.path.relpath(p, ROOT) for p in DETOUR_SAFE_LEDGERS)
        + f" yielded any detourable rows -- the admission rule read out of "
        f"{os.path.relpath(BUILD_RS, ROOT)} matched nothing. Realign it; do not let the "
        "calibration pass on an empty set."
    )
    # ...and both ledgers must contribute, because they are unioned. One of them quietly falling to
    # zero leaves a comfortably non-empty union and the same unexamined half the 2026-08-31
    # incident lived in.
    for ledger in DETOUR_SAFE_LEDGERS:
        assert rows(ledger), (
            f"{os.path.relpath(ledger, ROOT)} contributes NO detourable rows to "
            "DETOUR_SAFE_1162_TO_1170, so nothing in it is being audited. build.rs still reads it."
        )

    # The negative control for the VACUITY class. The control this class needs is not "plant a bad
    # row and see it caught" -- a filter matching zero rows catches nothing at all, planted or
    # otherwise, and reports it as clean. So: hand the real filter a table whose verdict column
    # carries a word the vocabulary cannot contain, and require the gate to go red. Run against
    # the pre-2026-08-30 code this assertion fails, because `rows()` returned [] and every
    # downstream `assert not bad` then passed over the empty set.
    with tempfile.TemporaryDirectory() as scratch:
        blind = rva_admission._synthetic(scratch, "IDENTICAL-SHORT")
        try:
            rows(blind)
            raise AssertionError(
                "NEGATIVE CONTROL FAILED: a verdict table whose every row the filter refuses was "
                "accepted as a clean audit. That is the vacuous-quantification bug itself -- the "
                "gate would report `0 of 0 need a look` and exit green having examined nothing."
            )
        except rva_admission.Vacuous as refusal:
            assert "IDENTICAL-SHORT" in str(refusal), (
                "the refusal must name the verdict word the table really carries, or the reader "
                f"cannot tell drift from an empty scope: {refusal}"
            )
        # ...and the control is only meaningful if the same filter accepts a table written in the
        # live vocabulary. Otherwise it would pass with the filter permanently broken.
        good = rva_admission._synthetic(os.path.join(scratch, "good"), pairs and "BYTE-IDENTICAL")
        assert len(rows(good)) == 8, "the filter rejects its own vocabulary; the control is inert"
    print("negative control: a zero-matching verdict filter is refused, not reported clean")
    images = {"1.17": open(IMAGE_1170, "rb").read(), "1.16.2": open(IMAGE_1162, "rb").read()}
    for build, va, want, discriminating, why in PATCH_SAFE_CASES:
        got, detail = patch_safe(images[build], va)
        assert got == want, (
            f"{build} 0x{va:x} ({why}): patch_safe returned {got} [{detail}], expected {want}"
        )
        # The mutation test, run every time rather than once by hand: a bound nobody has watched
        # fail is a claim, not a check.
        old, old_detail = _unbounded_walk(images[build], va)
        assert (old != want) == discriminating, (
            f"{build} 0x{va:x} ({why}): the pre-2026-08-30 walk said {old} [{old_detail}], so this "
            f"case is {'not ' if discriminating else ''}discriminating -- fix the flag or the case"
        )
    discriminators = sum(1 for case in PATCH_SAFE_CASES if case[3])
    assert discriminators >= 3, "too few cases actually exercise the new bound"
    print(f"patch_safe: {len(PATCH_SAFE_CASES)} byte-dictated cases agree, {discriminators} of "
          "them wrong under the walk this replaced")

    # The branch scan's own bound, held to the same standard, against the same kept fixture.
    for build, va, want, discriminating, why in BRANCH_SCAN_CASES:
        got, detail = patch_safe(images[build], va)
        assert got == want, (
            f"{build} 0x{va:x} ({why}): patch_safe returned {got} [{detail}], expected {want}"
        )
        old, old_detail = _unbounded_branch_scan(images[build], va)
        assert (old != want) == discriminating, (
            f"{build} 0x{va:x} ({why}): the pre-2026-08-31 scan said {old} [{old_detail}], so this "
            f"case is {'not ' if discriminating else ''}discriminating -- fix the flag or the case"
        )
    assert any(case[3] for case in BRANCH_SCAN_CASES), (
        "no BRANCH_SCAN_CASES row is wrong under the unbounded scan, so none of them exercises "
        "the bound"
    )

    # ...AND the bound must not have made the arm TOOTHLESS. Narrowing a scan can only ever make
    # it accept more, so accepting the two leaves above proves nothing on its own. There is no
    # natural specimen to point at -- no row in either ledger has a genuine branch into its own
    # patch window, which is why the only ones the old scan ever "found" were desync artefacts --
    # so one is MADE: take an address the audit accepts and write a short jump into the middle of
    # its body, aimed at entry+2. In phase, same length, inside the declared extent.
    donor = 0x1407AE8C0
    site = donor + 0x2A  # `33 f6` (xor esi, esi), two bytes, and within rel8 range of the entry
    planted_body = bytearray(images["1.17"])
    displacement = (donor + 2) - (site + 2)
    planted_body[site - BASE : site - BASE + 2] = bytes([0xEB, displacement & 0xFF])
    planted = bytes(planted_body)
    ok, why = patch_safe(planted, donor)
    assert not ok and f"0x{site:x}" in why and f"0x{donor + 2:x}" in why, (
        f"POSITIVE CONTROL FAILED: a `jmp 0x{donor + 2:x}` planted at 0x{site:x} lands on the "
        f"second byte of the JMP MinHook writes over 0x{donor:x}, and patch_safe returned "
        f"{ok} [{why}]. The bound has turned the branch scan into a rubber stamp."
    )
    assert patch_safe(images["1.17"], donor)[0], (
        "the control is inert: the UNPLANTED donor must be accepted, or the refusal above is "
        "about something other than the planted branch"
    )
    print(f"branch scan: {len(BRANCH_SCAN_CASES)} byte-dictated cases agree, and a planted "
          f"`jmp` into 0x{donor:x}'s patch window is still refused")

    bad = audit(IMAGE_1162, pairs, 0, "1.16.2")
    assert not bad, (
        f"{len(bad)} row(s) the verified table admits as DETOURABLE cannot carry a detour:\n"
        + "\n".join(f"    {arrow}  {why}" for arrow, why in bad)
        + "\n\nRead this failure carefully before touching anything, because it has two very "
        "different causes and only one of them is here.\n"
        "  * A MID-FUNCTION flag NOW CARRIES ITS OWN DISCRIMINATOR -- read to the end of the "
        "parenthesis. `NOT an instruction boundary` means the DATA is wrong and nothing here can "
        "or should be changed: an address that is not an instruction boundary cannot be a "
        "function entry in ANY build, under ANY boundary source. Find what put that row in the "
        "ledger. Measured example, 2026-08-31: 1.16.2 `0x140001050` is 4 bytes into the 7-byte "
        "`lea` at `0x14000104c`, inside the function `.pdata` declares at `0x140001040` -- "
        "confirmed identically by `.pdata`, by capstone, and by Ghidra's own analysis.\n"
        "  * `on an instruction boundary` (or `boundary unknown`) is the case where the CHECK may "
        "be at fault, because `.pdata` can merge a tail or cover a second real entry -- that is "
        "how the previous ENTRY check (20 of 27 known-good addresses called mid-function) and the "
        "first draft of this PATCH check were both caught. Fix it here.\n"
        "  * Do NOT read the calibration set as `the 27 addresses we hook today`. It is every row "
        "BOTH detour ledgers admit (see DETOUR_SAFE_LEDGERS), it grows as agents add rows, and a "
        "newly added bad row is now the likelier of the two explanations.\n"
        "  * If a row is flagged PATCH-UNSAFE on a short function, the DATA is wrong and this "
        "file is right. `IDENTICAL-LEAF` grants a detour licence after checking that no branch "
        "targets the patched bytes, but NOT that those five bytes exist: a 3-byte leaf with no "
        "uniform padding after it and none above it can never be detoured, in any build. That "
        "belongs in scripts/verify-rva-map-1170.py, which issues the verdict.\n"
        "Do NOT loosen patch_safe and do NOT drop rows from the calibration to get past this."
    )
    blob = images["1.17"]
    inside = 0x1407AE8C0 + 2
    hits = xref_targets(blob, {inside})
    # Negative control: two bytes into a known entry. Neither the exact-start test nor the
    # unwindless-leaf test may rescue it -- `.pdata` declares a function containing it.
    starts, spans = pdata_functions(blob)
    ok, why = entry_verdict(hits[inside], starts, inside, spans)
    assert not ok, f"an address inside a function must not read as an entry, got {why}"

    # The positive control, end to end, on the address from the 2026-08-31 incident. The control
    # above exercises `entry_verdict` in isolation; this one drives the whole path a bad ledger row
    # takes -- `audit()` over a planted pair -- and requires the gate to go red, name the containing
    # function, and say the row is not even an instruction boundary. Without it "the gate would
    # catch a mid-function row" is a claim nobody has watched come true, and the calibration going
    # green means only that today's rows are fine.
    #
    # 1.16.2 0x140001050 is 0x10 bytes into the function `.pdata` declares at 0x140001040
    # (0x1040..0x111c) and 4 bytes into the 7-byte `lea rdx, [rip + 0x30a5c75]` at 0x14000104c.
    # Ghidra's independent analysis of the same image: entry 0x140001040, body end 0x14000111b.
    planted = 0x140001050
    caught = audit(IMAGE_1162, [(planted, planted)], 0, "1.16.2 (positive control)")
    assert caught, (
        f"POSITIVE CONTROL FAILED: 0x{planted:x} is 4 bytes into a `lea` inside the function "
        "`.pdata` declares at 0x140001040, and the audit passed it. The gate cannot catch the "
        "defect it exists to catch."
    )
    _arrow, reason = caught[0]
    assert "MID-FUNCTION" in reason and "0x140001040" in reason, (
        f"the positive control was flagged, but not as a mid-function landing in 0x140001040, so "
        f"it is proving something else: {reason}"
    )
    assert "NOT an instruction boundary" in reason, (
        "the flag must say WHICH of its two causes this is, or the reader is sent to loosen the "
        f"checker over a row that is simply wrong: {reason}"
    )
    print("positive control: a genuinely mid-function ledger row is caught and named as bad DATA")

    # The pointer scan'S equivalence, measured rather than asserted in prose. A sample, because
    # the fixture is one 98MB scan per address and the whole point of the replacement was that
    # that does not scale; a handful is enough to catch a suffix/offset error, which would be
    # wrong for every address rather than for a rare one. Addresses whose only entry evidence is
    # a stored pointer are picked first, since those are the rows a missed hit would flip.
    sample = [old for old, _new in pairs[:6]]
    sample += [0x140001000, planted]  # the image-floor `.pdata` entry, and the planted interior
    reference = _per_address_pointer_scan(images["1.16.2"], set(sample))
    measured = xref_targets(images["1.16.2"], set(sample))
    for va in sample:
        assert measured[va]["ptr"] == reference[va], (
            f"the shared-suffix pointer scan disagrees with the per-address one at 0x{va:x}: "
            f"{measured[va]['ptr']} vs {reference[va]}. A scan that finds fewer references turns "
            "ENTRY-OK rows into MID-FUNCTION ones."
        )
    print(f"pointer scan: shared-suffix and per-address agree on {len(sample)} addresses")
    print("\nselftest OK")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument(
        "--calibrate",
        action="store_true",
        help="run the same checks on the 1.16.2 source addresses, which are known-good",
    )
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if args.calibrate:
        return 1 if audit(IMAGE_1162, rows(), 0, "1.16.2") else 0
    return 1 if audit(IMAGE_1170, rows(), 1, "1.17") else 0


if __name__ == "__main__":
    sys.exit(main())
