#!/usr/bin/env python3
"""Carry a 1.16.2 data address (a global, a vtable, a table) onto 1.17.

Why the function map cannot do this
-----------------------------------
`build-1162-1170-function-map.py` pairs functions, and it works because a
function has content to compare. A global has no content: at rest it is eight
zero bytes like every other global, so nothing about the datum itself says
which one it is.

And they did move. Every `.data` global in the sibling's RVA bundle shifted
between 2.6.2.0 and 2.7.0.0 -- most by +0x4070, `runtime_heap_allocator` by
+0x4080, `multiplay_properties` by +0x4000, and `cs_system_step` backwards by
-0x17408. So a constant delta is not merely unproven, it is wrong, and the one
that breaks it is not an outlier anybody would have guessed.

Reading a stale global is quiet and then fatal. `GLOBAL_TEX_REPOSITORY_RVA`
went unread-and-unnoticed into `CreateTpfResCap`, which divided by zero at
`eldenring.exe+0x26537d0` and took the game down 894ms after load on
2026-08-29 -- with a perfectly correct, freshly translated function address
sitting one frame up, which is what made it look like the translation's fault.

How this works instead
----------------------
A global has no content, but the code that uses it does. So: find every
instruction in 1.16.2 `.text` that references the address rip-relatively, map
each of those functions onto 1.17 with the function map, decode the instruction
at the same position in the 1.17 function, and read where its displacement
points. Every reference casts a vote.

Agreement across independent call sites is the evidence. A single unopposed
vote is reported as weak rather than silently promoted, because one reference
inside a function that happens to have been edited is exactly how a confident
wrong address gets produced.

Calibrated against the eleven `.data` fields whose 1.16.2 and 1.17 values are
both known from the sibling's own generator -- including `cs_system_step`,
whose backwards move any delta-based method gets wrong.
"""

from __future__ import annotations

import argparse
import re
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import function_extent  # noqa: E402  (needs the sys.path line above)
# the one place in this file that DELETES.
#
# `refresh()` rewrites the data map wholesale. A row it does not reproduce is either a stray, which
# stops the write, or retired, which is dropped -- and "retired" was decided by asking whether the
# `CONST`/`ALIAS` scan below still produced the address. That scan is name-filtered (`*RVA*`),
# typed (`usize|u32|u64`), and knows exactly two shapes; it has no bare `rva: 0x..` table-field
# form and no `pub use .. as ..` form. An address written in any spelling it does not know was
# classed `retired (nothing declares 0x%x any more)` and deleted from a tracked ledger at exit 0.
#
# What that costs: the row's absence afterwards reads as an address that was never mapped, not as
# one that was deleted, and the code holding that address then reads a 1.16.2 value on 1.17 with no
# refusal line and no fault. This file's own header calls that failure "quiet and then fatal".
#
# So the drop is now gated on `rva_symbols`, which resolves values rather than spellings and
# reports `proven_unclaimed` as a fact separate from `found_nothing`. Only the former may delete.
try:  # noqa: E402 - repo-local; the sys.path line above is what makes it work
    import const_fold
    import rva_symbols
    # The population's fourth declaration form: a bare hex literal handed to the address resolver
    # with no constant name anywhere. Four such addresses held no row in any ledger until
    # 2026-08-31 -- neither verified nor reported as unverified.
    import rva_usage
except ImportError as missing:  # a resolver that cannot load must stop the delete, not degrade
    raise ImportError(
        "scripts/rva_symbols.py could not be imported, so this script cannot tell whether an "
        "address is still declared anywhere -- and it DELETES rows from a tracked ledger on that "
        "answer. Fix the import rather than restoring a local name-filtered scan."
    ) from missing

BASE = 0x140000000
CS_OP_IMM_TYPE: tuple = ()
CHUNK = 1 << 22


def _ensure(module: str):
    try:
        __import__(module)
    except ImportError:
        import os

        if os.environ.get("_MAPDATA_UNDER_UV"):
            raise SystemExit(f"{module} is still missing under uv")
        os.environ["_MAPDATA_UNDER_UV"] = "1"
        os.execvp(
            "uv",
            ["uv", "run", "--with", "capstone", "--with", "numpy", "python3", *sys.argv],
        )


class Image:
    def __init__(self, path: Path):
        self.data = path.read_bytes()
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        nsec = struct.unpack_from("<H", self.data, pe + 6)[0]
        optsz = struct.unpack_from("<H", self.data, pe + 20)[0]
        off = pe + 24 + optsz
        self.sections = {}
        for i in range(nsec):
            e = self.data[off + i * 40 : off + (i + 1) * 40]
            name = e[:8].rstrip(b"\0").decode("latin1")
            vsz, va, rsz, _ = struct.unpack_from("<IIII", e, 8)
            self.sections.setdefault(name, (va, max(vsz, rsz)))
        self.text = self.sections[".text"]
        self.pdata = self.sections[".pdata"]

    def function_starts(self) -> list[int]:
        va, size = self.pdata
        out = []
        for off in range(va, va + size, 12):
            begin, end, _ = struct.unpack_from("<III", self.data, off)
            if begin and end > begin and end - begin <= 0x20000:
                out.append(begin)
        out.sort()
        return out


# Bytes that can follow a rip-relative displacement before the instruction ends. The displacement
# is rip-relative to the end of the instruction, so a trailing immediate shifts the arithmetic by
# its own width -- and an immediate is not exotic here, it is the normal encoding for setting or
# testing a flag:
#
#     C6 05 dd dd dd dd 01     mov byte ptr [rip+dd], 1     <- trailing imm8
#     80 3D dd dd dd dd 00     cmp byte ptr [rip+dd], 0     <- trailing imm8
#     C7 05 dd dd dd dd ....   mov dword ptr [rip+dd], imm32
#
# Scanning only tail 4 therefore finds every read of a global and misses every write-an-immediate
# and compare-against-an-immediate. That is not a uniform loss: it lands hardest on exactly the
# globals that are single byte flags, whose entire use is `mov [x],1` / `cmp [x],0`. Measured
# 2026-08-29 on `TITLE_GLOBAL_ACCEPT_BYTE_RVA 0x4589bdc`, the zero-input title-advance flag: tail 4
# found nothing and the address was reported "no usable reference", while tail 5 finds its real
# references. The mod wrote to the stale 1.16.2 address for the whole of 1.17 and the title menu
# never opened.
IMMEDIATE_TAILS = (4, 5, 6, 8)


def references(image: Image, target: int) -> list[int]:
    """Byte offsets in .text whose 4-byte displacement could point at `target`.

    A rip-relative displacement `d` stored at offset `i` addresses `i + tail + d`, where `tail` is
    the number of bytes between the displacement and the end of the instruction. Every plausible
    tail is scanned (see `IMMEDIATE_TAILS`), which is one vectorised pass each rather than decoding
    forty-three megabytes of instructions.

    A wider scan admits candidates whose bytes happen to look right, so these are candidates only:
    `instruction_index` decodes each one and discards any whose instruction does not genuinely
    address `target`.
    """
    import numpy as np

    va, size = image.text
    hits: set[int] = set()
    for start in range(va, va + size, CHUNK):
        stop = min(start + CHUNK + 4, va + size)
        raw = np.frombuffer(image.data[start:stop], dtype=np.uint8).astype(np.uint32)
        if raw.size < 5:
            continue
        dw = raw[0:-3] | (raw[1:-2] << 8) | (raw[2:-1] << 16) | (raw[3:] << 24)
        idx = np.arange(start, start + dw.size, dtype=np.uint32)
        for tail in IMMEDIATE_TAILS:
            want = np.uint32((target - tail) & 0xFFFFFFFF) - idx
            for hit in np.nonzero(dw == want)[0]:
                hits.add(start + int(hit))
    return sorted(hits)


def range_references(image: Image, lo: int, hi: int) -> list[tuple[int, int]]:
    """`(displacement offset, addressed RVA)` for every candidate reference landing in `[lo, hi)`.

    `references` answers "who points at THIS address"; this answers "what does .text point at
    anywhere in this window", which is the same arithmetic run in the other direction and costs the
    same single vectorised pass. It exists so a bracket can be built: the addresses next to a
    candidate, and how far each of them moved, are the only local evidence available for a global
    that has exactly one reference of its own. Candidates only -- every hit still has to be decoded.
    """
    import numpy as np

    va, size = image.text
    out: list[tuple[int, int]] = []
    for start in range(va, va + size, CHUNK):
        stop = min(start + CHUNK + 4, va + size)
        raw = np.frombuffer(image.data[start:stop], dtype=np.uint8).astype(np.int64)
        if raw.size < 5:
            continue
        dw = raw[0:-3] | (raw[1:-2] << 8) | (raw[2:-1] << 16) | (raw[3:] << 24)
        dw = np.where(dw >= 1 << 31, dw - (1 << 32), dw)
        idx = np.arange(start, start + dw.size, dtype=np.int64)
        for tail in IMMEDIATE_TAILS:
            addr = idx + tail + dw
            for hit in np.nonzero((addr >= lo) & (addr < hi))[0]:
                out.append((start + int(hit), int(addr[hit])))
    return sorted(set(out))


def window_anchors(
    md, old: Image, new: Image, fmap: dict[int, int], lo: int, hi: int
) -> dict[int, dict[int, int]]:
    """Every 1.16.2 address in `[lo, hi)` that .text references, and where each reference re-reads.

    One decode pass, shared by every candidate in the window. The value is the same vote dict
    `carry` builds, so the same "two agreeing references" bar can be applied to an anchor.
    """
    old_starts = old.function_starts()
    votes: dict[int, dict[int, int]] = {}
    for disp_at, addressed in range_references(old, lo, hi):
        func = enclosing(old_starts, disp_at)
        if func is None or func not in fmap:
            continue
        found = instruction_index(md, old, func, disp_at, addressed)
        if found is None:
            continue
        index, at_offset = found
        moved = displacement_of(md, new, fmap[func], index, at_offset)
        if moved is None:
            continue
        bucket = votes.setdefault(addressed, {})
        bucket[moved] = bucket.get(moved, 0) + 1
    return votes


def bracket_confirms(
    md,
    old: Image,
    new: Image,
    fmap: dict[int, int],
    src: int,
    dst: int,
    radius: int = 0x400,
    min_votes: int = 2,
) -> tuple[bool, str]:
    """Whether the neighbourhood agrees with `src -> dst`, and a one-line reason either way.

    This is a test, never a source of an address. It answers yes or no about a candidate some other
    mechanism produced; it never proposes the majority delta as the answer. That distinction is
    load-bearing. `WORLD_NVM_MANAGER_GLOBAL_RVA 0x3d75870` really does move +0x4078 -- measured
    here at 323 of 327 of its own references, a third independent agreement with the ledger's
    266/270 -- while the nearest anchor above it (0x3d7588c, 4/4) moves +0x4070 and the ones nine
    bytes below (0x3d75879, 0x3d7587a) move +0x4062. Anything that "smoothed" it toward its
    neighbours would be wrong. This function cannot: it is a predicate, and a row with hundreds of
    its own votes never reaches it.

    An anchor counts only when its references agree unanimously and there are at least two of them
    -- the same bar `carry` promotes on. The test is the nearest such anchor on each side: the
    candidate has to sit inside a pair of independently-carried addresses that both moved by the
    delta being claimed, which is what the word bracket means.

    Why the nearest pair and not every anchor in the window: the strict "no anchor may disagree"
    form was tried first and rejected `NAV_COST_TABLE_RVA` on 0x3d61ee2, an UNALIGNED byte
    reference 0x122 above the target which moves +0x405d while its own immediate neighbours
    0x3d61ee3 and 0x3d61ee4 (18 and 7 votes) move +0x4060. That is a field shifting three bytes
    inside a structure, which says nothing about where the structure's base went, and every one of
    the ~40 other anchors within +-0x400 of the target -- including 222/223, 47/47, 43/43 and 20/20
    -- moves +0x4060. The relaxation is not a loosening toward the answer wanted: run against
    `WORLD_NVM_MANAGER`, the nearest-anchor form still fails (nearest above +0x4070, nearest below
    +0x4062, claim +0x4078), so it continues to reject exactly the discontinuity it exists to
    catch. Disagreeing anchors elsewhere in the window are still counted and reported.
    """
    delta = dst - src
    anchors = window_anchors(md, old, new, fmap, src - radius, src + radius)
    strong: list[tuple[int, int]] = []
    for addressed, bucket in sorted(anchors.items()):
        if addressed == src or len(bucket) != 1:
            continue
        moved, count = next(iter(bucket.items()))
        if count >= min_votes:
            strong.append((addressed, moved - addressed))
    below = [entry for entry in strong if entry[0] < src]
    above = [entry for entry in strong if entry[0] > src]
    if not below or not above:
        return False, (
            f"no bracket within +-0x{radius:x}: {len(below)} anchor(s) below, {len(above)} above"
        )
    lo_rva, lo_delta = below[-1]
    hi_rva, hi_delta = above[0]
    if lo_delta != delta or hi_delta != delta:
        return False, (
            f"nearest anchors disagree with +0x{delta:x}: "
            f"0x{lo_rva:x} moved +0x{lo_delta:x}, 0x{hi_rva:x} moved +0x{hi_delta:x}"
        )
    odd = [f"0x{a:x} +0x{d:x}" for a, d in strong if d != delta]
    note = f" ({len(odd)} other anchor(s) in the window differ: {', '.join(odd[:3])})" if odd else ""
    return True, (
        f"bracketed by 0x{lo_rva:x} and 0x{hi_rva:x}, both +0x{delta:x} "
        f"({len(strong)} anchor(s) within +-0x{radius:x}){note}"
    )


def call_references(image: Image, target: int) -> list[int]:
    """Offsets of the rel32 in every `call`/`jmp` in .text whose target is `target`.

    A rel32 branch is relative to the end of the instruction, and both `E8 rel32` and `E9 rel32`
    put the rel32 last, so the arithmetic is the same as a trailing displacement: the four bytes
    at `i` name `target` when `i + 4 + rel32 == target`. Candidates only -- `call_index` decodes
    them and discards anything that is not really a branch to `target`.
    """
    import numpy as np

    va, size = image.text
    hits: set[int] = set()
    for start in range(va, va + size, CHUNK):
        stop = min(start + CHUNK + 4, va + size)
        raw = np.frombuffer(image.data[start:stop], dtype=np.uint8).astype(np.uint32)
        if raw.size < 5:
            continue
        dw = raw[0:-3] | (raw[1:-2] << 8) | (raw[2:-1] << 16) | (raw[3:] << 24)
        idx = np.arange(start, start + dw.size, dtype=np.uint32)
        want = np.uint32((target - 4) & 0xFFFFFFFF) - idx
        for hit in np.nonzero(dw == want)[0]:
            hits.add(start + int(hit))
    return sorted(hits)


def call_index(md, image: Image, func: int, rel_at: int, target: int) -> int | None:
    """Which instruction of `func` is the branch whose rel32 sits at `rel_at` and reaches `target`."""
    window = image.data[func : rel_at + 16]
    for n, insn in enumerate(md.disasm(window, BASE + func)):
        pos = insn.address - BASE - func
        if insn.mnemonic in ("call", "jmp") and func + pos + insn.size - 4 == rel_at:
            operands = insn.operands
            if len(operands) != 1 or operands[0].type != CS_OP_IMM_TYPE[0]:
                return None
            return n if operands[0].imm - BASE == target else None
        if func + pos > rel_at:
            return None
    return None


def call_target_of(md, image: Image, func: int, index: int) -> int | None:
    """Where instruction `index` of `func` branches to, as an RVA."""
    # `index` was counted in the other image. A 1.17 function shorter than its 1.16.2
    # counterpart supplies that index out of its neighbour, and this returns a branch target
    # that was never assembled. Bound by the extent and refuse when it runs out, rather than
    # decoding a byte budget into whatever follows. Found by check-decode-extent-bounds.py.
    end = function_extent.body_slice_end(image.data, BASE + func, 0x800)
    if end is None:
        return None
    window = image.data[func:end]
    for n, insn in enumerate(md.disasm(window, BASE + func)):
        if n == index:
            if insn.mnemonic not in ("call", "jmp"):
                return None
            operands = insn.operands
            if len(operands) != 1 or operands[0].type != CS_OP_IMM_TYPE[0]:
                return None
            return operands[0].imm - BASE
    return None


def carry_code(md, old: Image, new: Image, fmap: dict[int, int], target: int):
    """Carry a `.text` address by its callers rather than by its own bytes.

    The body-signature mappers -- this repo's function map and
    `map-rvas-1162-to-1170.py` -- both identify a function by what it looks like, so both go
    silent on the one case that matters most: a function whose body genuinely changed in 1.17.
    Measured 2026-08-29, six addresses the running game refused were absent from the 128,603-row
    function map and unresolvable by masked signature, including the now-loading helper `Update`
    that the loading bar reads (1,513 refusals in a single boot).

    A caller is different evidence. If the caller maps, the call instruction at the same index in
    the 1.17 caller points at wherever the callee moved to -- and that holds however much the
    callee itself was rewritten. Every caller votes, exactly as the data path votes.
    """
    old_starts = old.function_starts()
    votes: dict[int, int] = {}
    seen = 0
    for rel_at in call_references(old, target):
        func = enclosing(old_starts, rel_at)
        if func is None:
            continue
        index = call_index(md, old, func, rel_at, target)
        if index is None:
            continue
        if func not in fmap:
            continue
        seen += 1
        moved = call_target_of(md, new, fmap[func], index)
        if moved is not None:
            votes[moved] = votes.get(moved, 0) + 1
    if not votes:
        # Say why there is no caller: "the callers are unmapped" and "nothing branches here at
        # all" are different problems with different next steps -- improve the function map, or
        # go looking for a vtable slot / runtime-built function pointer.
        cands = len(call_references(old, target))
        decoded = 0
        for rel_at in call_references(old, target):
            func = enclosing(old_starts, rel_at)
            if func is None or call_index(md, old, func, rel_at, target) is None:
                continue
            decoded += 1
            # Name the site. "The caller is unmapped" is only actionable if you are told which
            # caller, so the next step -- map that one function -- is one command away.
            print(f"    branch at 0x{rel_at:x} in fn 0x{func:x}  {'IN MAP' if func in fmap else 'NOT IN FUNCTION MAP'}")
        return None, (
            f"no usable caller ({cands} candidate branch site(s), {decoded} real, {seen} in the map)"
        ), votes
    best = max(votes, key=lambda k: votes[k])
    if len(votes) > 1:
        return best, f"CONTESTED {len(votes)} answers from {seen} callers", votes
    if votes[best] < 2:
        return best, f"WEAK (one caller of {seen})", votes
    return best, f"agreed by {votes[best]} callers", votes


def enclosing(starts: list[int], rva: int) -> int | None:
    import bisect

    i = bisect.bisect_right(starts, rva) - 1
    return starts[i] if i >= 0 else None


def instruction_index(md, image: Image, func: int, disp_at: int, target: int) -> tuple[int, int] | None:
    """`(instruction index, byte offset from function start)` for the reference at `disp_at`.

    Both anchors are returned because neither survives on its own, and trusting only the index
    silently loses references. `displacement_of` re-reads the paired 1.17 function by walking to
    the same instruction index -- so if any earlier instruction changed length, the walk arrives at
    a different instruction and the reference is dropped or, worse, votes for whatever that
    instruction points at. Measured 2026-08-30 on `RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA`
    (0x3d6c5e8) and `SAVE_SERIALIZE_BYTES_RVA` (0x3d69920): both enclosing functions are in the
    map, and both were reported "no usable reference" purely because index #271 and #84 land
    elsewhere in 1.17 while the byte offset of the reference is unchanged.

    The target check is what makes a multi-tail candidate scan safe. A candidate offset only means
    "these four bytes would be the right displacement if the instruction ended a certain number of
    bytes later"; decoding says where the instruction really points, and a candidate that lands on
    a real displacement pointing somewhere else is a coincidence, not a reference.
    """
    window = image.data[func : disp_at + 16]
    for n, insn in enumerate(md.disasm(window, BASE + func)):
        pos = insn.address - BASE - func
        if insn.disp_size == 4 and func + pos + insn.disp_offset == disp_at:
            reaches = insn.address - BASE + insn.size + insn.disp
            return (n, pos) if reaches == target else None
        if func + pos > disp_at:
            return None
    return None


def displacement_of(md, image: Image, func: int, index: int, at_offset: int | None = None) -> int | None:
    """Where the paired instruction in `func` points, rip-relatively.

    Anchored on the byte offset from the function start when one is supplied, falling back to the
    instruction index. Byte offset is the better anchor: an instruction that changes length shifts
    every later index by one but leaves the offsets of everything before it alone, and a patch
    usually edits one instruction rather than inserting one. Neither anchor is sound alone, so a
    disagreement between them is not resolved here -- the offset simply wins, and the vote across
    independent references is what catches a bad answer.
    """
    # The window must reach the reference, and 0x400 does not: `MENU_PUMP_KICK_PTR_RVA`'s sole
    # reference sits at byte +0x779 of its function, so a 0x400 decode stopped short, found neither
    # the offset nor the index, and the address was reported "no usable reference" -- the same
    # sentence a genuinely unreferenced address gets. Measured 2026-08-30: with the window opened to
    # cover the offset the reference decodes, matches shape, and votes. Decoding past the function
    # end is harmless because only an instruction that starts exactly at `at_offset` is consumed.
    limit = max(0x400, (at_offset + 0x20) if at_offset is not None else 0)
    window = image.data[func : func + limit]
    # The sentence above is true of the `at_offset` arm ONLY: that arm consumes an instruction
    # that starts exactly at a known offset, so a wide window cannot invent one. The `n == index`
    # fallback below can match an instruction past the function end, where the decoder has
    # resynchronised on padding. Bound that arm by the extent; refuse it when the extent is
    # unknown rather than falling back to the byte budget. Found by check-decode-extent-bounds.py.
    body_end = function_extent.body_slice_end(image.data, BASE + func)
    by_index = None
    for n, insn in enumerate(md.disasm(window, BASE + func)):
        pos = insn.address - BASE - func
        if at_offset is not None and pos == at_offset and insn.disp_size == 4:
            return insn.address - BASE + insn.size + insn.disp
        if n == index and insn.disp_size == 4:
            if body_end is not None and (func + pos) < body_end:
                by_index = insn.address - BASE + insn.size + insn.disp
    return by_index


def carry(md, old: Image, new: Image, fmap: dict[int, int], target: int):
    old_starts, new_starts = old.function_starts(), new.function_starts()
    votes: dict[int, int] = {}
    seen_functions = 0
    for disp_at in references(old, target):
        func = enclosing(old_starts, disp_at)
        if func is None or func not in fmap:
            continue
        found = instruction_index(md, old, func, disp_at, target)
        if found is None:
            continue
        index, at_offset = found
        seen_functions += 1
        moved = displacement_of(md, new, fmap[func], index, at_offset)
        if moved is not None:
            votes[moved] = votes.get(moved, 0) + 1
    if not votes:
        return None, "no usable reference", votes
    best = max(votes, key=lambda k: votes[k])
    if len(votes) > 1:
        return best, f"CONTESTED {len(votes)} answers from {seen_functions} references", votes
    if votes[best] < 2:
        return best, f"WEAK (one reference of {seen_functions})", votes
    return best, f"agreed by {votes[best]} references", votes



# --- RTTI rescue for vtables ----------------------------------------------------------------
# Reference voting needs two agreeing references, and a vtable referenced from only one place is
# withheld even when its identity is not in doubt. A vtable carries its own name: MSVC puts a
# CompleteObjectLocator at `vtable[-1]` whose type descriptor holds the mangled class. If the same
# mangled name sits at the candidate in the new image and at the source in the old one -- and at
# neither of the crossed positions -- that is a unique-name match, which is stronger evidence than
# any number of agreeing displacements.
#
# Measured 2026-08-29: `FUNCTOR_VTABLE_RVA 0x2ac3ea8 -> 0x2ac6f28` was dropped as weak (one
# reference), and RTTI confirms it outright -- both ends carry
# `.?AV?$_Func_impl@V<lambda_e1e7...>@@...PEAVMenuWindow@CS@@AEAVSceneProxy@5@@std@@`, a name that
# occurs once per image.
RTTI_TYPE_DESCRIPTOR_OFFSET = 0x0C
RTTI_NAME_OFFSET = 0x10


def rtti_class_name(image: bytes, vtable_rva: int, image_base: int = 0x140000000) -> str | None:
    """The mangled RTTI class name for a vtable at `vtable_rva`, or None if that is not a vtable."""
    if not 8 <= vtable_rva < len(image):
        return None
    locator = int.from_bytes(image[vtable_rva - 8 : vtable_rva], "little")
    if not image_base <= locator < image_base + len(image):
        return None
    locator -= image_base
    if locator + RTTI_TYPE_DESCRIPTOR_OFFSET + 4 > len(image):
        return None
    descriptor = int.from_bytes(
        image[locator + RTTI_TYPE_DESCRIPTOR_OFFSET : locator + RTTI_TYPE_DESCRIPTOR_OFFSET + 4],
        "little",
    )
    start = descriptor + RTTI_NAME_OFFSET
    if not 0 < start < len(image):
        return None
    end = image.find(b"\0", start, start + 256)
    if end < 0:
        return None
    name = image[start:end]
    return name.decode("ascii", "replace") if name.startswith(b".?A") else None


# MSVC tags each translation unit's anonymous NAMESPACE with a per-build hash, so the same class
# is `...@?A0x7c8d539b@@...` in 1.16.2 and `...@?A0x8fca6706@@...` in 1.17. Comparing the raw
# names therefore makes an anonymous-namespace vtable structurally unable to rescue itself here --
# and it declines silently, as "not the same class", which is indistinguishable from a genuinely
# wrong candidate. Measured 2026-08-30 on `MenuJobLoadContextVtable` (0x2ac71e0 -> 0x2aca260,
# renamed 2026-08-30 from `SELECTOR_STEP_VTABLE_RVA`; RTTI-confirmed a MenuJob vtable, not a
# "SelectorStep" one -- see scripts/rva-alias-allowlist.txt 0x2ac71e0):
# `MenuJobWithContext<LoadJobContext@?A0x...,lambda_1af212c9...>` differs between the images in the
# namespace tag and nothing else -- the lambda hash is stable across builds. That row happened to
# have two agreeing references and never needed the rescue; the next one may not.
#
# Only the namespace tag is masked. Class names, template arguments and lambda hashes are compared
# verbatim, so two genuinely different classes still differ, and the crossed-position guard below
# still runs on the masked names -- a region that did not move cannot pass by accident.
ANON_NAMESPACE = re.compile(r"\?A0x[0-9a-f]{8}")


def canonical_class(name: str | None) -> str | None:
    return None if name is None else ANON_NAMESPACE.sub("?A0x@ANON@", name)


def rtti_confirms(old_image: bytes, new_image: bytes, src_rva: int, dst_rva: int) -> str | None:
    """The shared mangled name when `src` in the old image and `dst` in the new are the same class.

    Requires the crossed positions not to carry that name, so a region that happens not to have
    moved cannot pass by accident.
    """
    src_name = canonical_class(rtti_class_name(old_image, src_rva))
    if src_name is None or src_name != canonical_class(rtti_class_name(new_image, dst_rva)):
        return None
    if (
        canonical_class(rtti_class_name(new_image, src_rva)) == src_name
        or canonical_class(rtti_class_name(old_image, dst_rva)) == src_name
    ):
        return None
    return src_name


# The type is not part of what makes something a game address, and requiring `usize` here made
# every `: u32` constant invisible to this scanner -- never scanned, never voted on, never
# written to the data map, and read stale at runtime with no log line. This is the same defect
# select-needed-1170-rows.py records and fixed for the function map (see its RVA_TYPE note); it
# survived here. Measured 2026-08-30: er-invasion-path declares NAV_COST_TABLE_RVA,
# HK_AI_MANAGER_GLOBAL_RVA, WORLD_NVM_MANAGER_GLOBAL_RVA and GLOBAL_CSSFX_RVA as `: u32`
# (navpath.rs:44,49,83; sfx.rs:28), so all four were absent from the data map while the nav
# request and the SFX read went to 1.16.2 addresses on 1.17. Reading a stale global is the
# failure this file's own header calls "quiet and then fatal".
RVA_TYPE = r"(?:usize|u32|u64)"
# The whole INITIALISER, not its first literal.
#
# The previous form stopped at `(0x[0-9a-fA-F_]+)`, which reads the first hex literal it meets and
# calls that the address. For `= 0x142658c60 - 0x140000000` that is the MINUEND -- an absolute VA,
# not the RVA the constant actually holds. Measured 2026-08-30 on
# `ADD_DEFAULT_FILE_LOAD_PROCESS_RVA`: the scraper recorded 0x142658c60, which is 1.1 GB past the
# end of the image, so nothing in `.text` could reference it and it was filed in this map's unused
# list as an unmappable data global. It is neither unmappable nor data: its real value is RVA
# 0x2658c60, a `.text` function that the function map already carries to 0x265b470.
#
# The failure mode is what makes this worth a regex change rather than a one-line exception. The
# tool did not refuse and did not warn; it produced a confident classification of an address that
# does not exist, in a file whose entire purpose is to be trusted about addresses. So the
# initialiser is now captured whole and evaluated, and an initialiser this cannot evaluate is
# reported rather than half-read.
CONST = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*" + RVA_TYPE + r"\s*=\s*([^;]+);"
)
LITERAL_ARITHMETIC = re.compile(r"\A\s*0x[0-9a-fA-F_]+(?:\s*[-+]\s*0x[0-9a-fA-F_]+)*\s*\Z")
# Empty: `LITERAL_ARITHMETIC` above has already established there is nothing to look up, so the
# evaluator needs no declaration index and this costs no scan.
_CONSTANTS = const_fold.Constants(root=Path(__file__).resolve().parent.parent)


def const_value(initialiser: str) -> int | None:
    """The value of an `*_RVA` initialiser, or None when it is not hex-literal arithmetic.

    Only `0x..`, `0x.. - 0x..` and `0x.. + 0x..` chains evaluate. Anything else -- an alias to
    another constant, an `as` cast, a decimal, a call -- returns None and is reported by the
    caller, because a partial read is how a subtrahend became an address.

    The arithmetic now runs in `scripts/const_fold.py`, which is the same evaluator
    `select-needed-1170-rows.py` and `detect-struct-field-drift.py` use. `LITERAL_ARITHMETIC`
    stays in front of it deliberately: the shared folder resolves named constants and enum
    variants too, and admitting those here would widen this map's population as a side effect of
    a refactor. Measured over all 732 `*_RVA` initialisers in the workspace, the delegated form
    and the hand-rolled loop it replaced disagree on zero.
    """
    if not LITERAL_ARITHMETIC.match(initialiser):
        return None
    folded = _CONSTANTS.fold(initialiser)
    return folded.value if folded.hex_rooted and folded.value >= 0 else None
# `pub const FOO_RVA: usize = SomeEnum::Variant as usize;` -- the value lives on the enum, so a
# literal scan never sees it. SESSION_SINGLETON_144588E98_RVA is written this way, wrapped across
# two lines, and its absence from this map stalled the 1.17 autoload at `session=0x0`: the title
# owner was found, state 10/10, and core readiness then waited forever on a singleton whose
# address had no 1.17 mapping. It maps cleanly once seen -- 0x4588e98 -> 0x458cf18, 28 agreeing
# references -- so the only thing missing was the scanner looking for this spelling.
ALIAS = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*" + RVA_TYPE + r"\s*=\s*(\w+)::(\w+)\s+as\s+" + RVA_TYPE
)
VARIANT = re.compile(r"^\s*(\w+)\s*=\s*(0x[0-9a-fA-F_]+)\s*,", re.M)
BOUND = re.compile(r"_(MIN|MAX|BOUND|BASE|SIZE|LEN|LENGTH|COUNT|END|START|STRIDE|ALIGN)$")
DATA_MAP = "docs/recon/rva-map-1162-to-1170.data.tsv"
# The banner the preserved rows sit under. Matched as a prefix when the file is re-read, so the
# rest of the wording can change without orphaning the rows it introduces.
PRESERVED_BANNER = "# PRESERVED"


def retirement_verdict(rva: int, claims, root: str = ""):
    """`(retire?, why)` -- `True` only on a proof that nothing in `crates/` declares `rva`.

    Split out of `refresh()` so the decision that DELETES a ledger row can be asserted directly.
    `claims is None` means the resolver could not run, which is not evidence of anything and must
    never read like a clean scan.
    """
    if claims is None:
        return False, "the crates/ resolver could not run; nothing is known"
    if claims.declarations or claims.literals:
        names = sorted({decl.qualified for decl in claims.declarations})
        where = [decl.where(root) for decl in claims.declarations][:2]
        where += [lit.where(root) for lit in claims.literals][:2]
        return False, (
            f"STILL DECLARED by {', '.join(names) or 'a bare literal'} "
            f"({', '.join(where[:3])}); the scan above cannot spell it"
        )
    if claims.proven_unclaimed:
        return True, "PROVEN unclaimed"
    return False, (
        f"NOT PROVEN: no declaration or literal FOUND, but {len(claims.residue)} of "
        f"{claims.universe} address-capable declarations could not be evaluated "
        f"(python3 scripts/rva_symbols.py --residue 0x{rva:x})"
    )


def claims_for(repo: Path, rva: int):
    """Who declares this address anywhere in `crates/`, resolved by value. None if the walk broke.

    A broken walk must never read like a clean one: the caller preserves the row on None rather
    than treating the silence as "nothing declares it".
    """
    try:
        return rva_symbols.index(repo / "crates").claims(rva)
    except (OSError, RecursionError) as failure:
        print(f"  (could not resolve crates/ symbols for 0x{rva:x}: {failure})", file=sys.stderr)
        return None

# Bracket-and-shape RESCUE: globals whose only reference lives in the dearxan'd image's trampoline
# rubble, where the enclosing `.pdata` entry maps to nine places or none and reference voting has
# nothing to vote with. Each entry here is carried by three independent facts, all reproducible
# from this script, and is listed with the command that reproduces them:
#
#   1. Bracket -- every mapped `.data` anchor on both sides of it moved by the same delta.
#   2. Source  -- in 1.16.2 exactly N sites of one masked instruction shape reach the address.
#   3. Target  -- in 1.17 exactly N sites of that identical shape reach the candidate.
#
# Fact 3 alone is weak (the `movzx eax, byte ptr [rip]; ret` getter shape reaches 405 addresses
# image-wide); the bracket is what selects the address and the shape is what confirms it. Kept as
# data rather than a hand-edited row so `--refresh` cannot silently drop it.
SHAPE_RESCUED = {
    # The title's zero-input menu-accept byte. The mod wrote it at the stale 1.16.2 address for
    # the whole of 1.17: `title-accept-byte: set [0x144589bdc]=1` logged success every boot while
    # the store landed on a moved global, so `TitleTopDialog::update` never ran the open-menu
    # registrar and the title menu never opened. Bracketed by five anchors that all move +0x4080
    # (0x4588e98, 0x4589390, 0x45896a8, 0x4589ad8, 0x458b890); one getter stub each side
    # (1.16.2 0xe85f50, 1.17 0xe87d50).
    #   python3 scripts/map-data-rvas-1162-to-1170.py 0x4589bdc --confirm 0x458dc5c
    0x4589BDC: (0x458DC5C, "TITLE_GLOBAL_ACCEPT_BYTE_RVA", "bracket+shape"),
    # The return-title rebuild flag. The single highest-volume refusal in the entire suite:
    # 339,684 in one session, because `write_telemetry.rs` reads it every tick. Its one reference
    # is `mov byte ptr [rip+d], 1` @0x78a990 -- a trailing-immediate encoding, and the setter is a
    # tail-jump target reached by exactly one control transfer in each image.
    #   python3 scripts/map-data-rvas-1162-to-1170.py 0x3d6c5e8 --confirm 0x3d70658
    0x3D6C5E8: (0x3D70658, "RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA", "shape"),
    # SAVE_SERIALIZE_BYTES_RVA, refused by er-save-disable. It is GAME_MAN_SINGLETON + 8 in both
    # images -- eight bytes from a 115-vote anchor -- and its one reference is the same
    # instruction at the same byte offset inside a .pdata-paired function.
    #   python3 scripts/map-data-rvas-1162-to-1170.py 0x3d69920 --confirm 0x3d6d990
    0x3D69920: (0x3D6D990, "SAVE_SERIALIZE_BYTES_RVA", "anchor+8+shape"),
    # `Game.Debug.IsEnableControlOnDisactiveWindow`'s backing byte, forced by er-focus-input so
    # the game keeps reading pad/mouse input while its window is unfocused. Its one read is the
    # getter stub's `movzx eax, byte ptr [rip+d]` (1.16.2 0x2e6853, 1.17 0x2e8853) -- the same
    # 405-address-wide shape the note above warns about, so the shape alone is not what selects
    # it. What selects it is the pair of callers, both already carried by the function map:
    # CS::CSPadStep::STEP_Update (0xe33aa0 -> 0xe358a0) and CS::CSPadStep::CSPadStep
    # (0xe328d0 -> 0xe346d0) reach the getter at the identical byte offsets +0xa3c and +0x7e in
    # both builds, and both 1.17 calls land on one stub reading 0x458cb71. Bracketed by five
    # anchors that all move +0x4080 (0x4588e98, 0x4589390, 0x45896a8, 0x4589ad8, 0x4589bdc), which
    # is exactly 0x4588af1 -> 0x458cb71.
    #   uv run --with capstone python3 scripts/find-debug-flag-getter.py 0x140e33aa0
    #   uv run --with capstone python3 scripts/find-debug-flag-getter.py 0x140e358a0 --image eldenring-deobf-1.17.bin
    #   python3 scripts/map-data-rvas-1162-to-1170.py 0x4588af1 --confirm 0x458cb71
    0x4588AF1: (
        0x458CB71,
        "GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA",
        "bracket+shape",
    ),
}


# --- Single-reference rescue ------------------------------------------------------------------
# Ten globals the vote refuses on its own. Every one has exactly one rip-relative reference (or, for
# the task table, none at all), and one unopposed vote is precisely what this file declines to
# promote -- correctly, because a lone reference inside a function that happened to be edited is how
# a confident wrong address is produced.
#
# What makes them promotable is a second line of evidence that does not come from the reference at
# all, re-derived here at every `--refresh` from the two images rather than trusted from this table:
#
#   unique   the datum is a string that occurs exactly once in each image, at the source in 1.16.2
#            and at the candidate in 1.17. A name that occurs once per image is stronger evidence
#            than any number of agreeing displacements (the same argument the RTTI rescue rests on).
#   fnptr    the datum is a table of code pointers, and the entries that the function map can carry
#            all land exactly where they should, with the identical test at +-0x8 and +-0x10 failing.
#   bracket  the nearest independently-carried anchor on each side moved by the same delta being
#            claimed. See `bracket_confirms` for why this rejects `WORLD_NVM_MANAGER`.
#
# The table stores the answer, and the checks either confirm it or drop the row to unused; a check
# never supplies or adjusts an address. A row whose own reference votes for something other than the
# tabled address is refused outright rather than rescued -- that is a contradiction, not weak
# evidence, and this file's failure mode is a wrong address, not a missing one.
REFSITE_RESCUED = {
    # No rip-relative reference of any kind: an 8-entry table of menu-task update callbacks, reached
    # only through a register. Two of the eight entries are functions the function map carries, and
    # both land exactly right; the same test one slot either way scores 0 correct / 2 mismatch.
    0x2AC72A0: (0x2ACA320, "TRACE_MENU_TASK_UPDATE_TABLE_RVA", "fnptr+bracket"),
    # ASCII "PressStart", the title's press-any-button state name. One occurrence per image.
    0x2B26500: (0x2B29580, "TITLE_PRESS_START_NAME_RVA", "unique+bracket"),
    # ASCII "TosTitle/Text", the terms-of-service dialog's text path. One occurrence per image.
    0x2B27330: (0x2B2A3B0, "POLICY_TOS_TITLE_TEXT_PATH_RVA", "unique+bracket"),
    # UTF-16 "m60_42_34_00". One occurrence per image, and the only one of the ten with no usable
    # neighbourhood at all -- it sits in string soup where nothing else has two references -- so the
    # bracket cannot be asked for and the unique name is the whole of the corroboration.
    0x2B62C70: (0x2B65D20, "DEFAULT_MAP_STRING_RVA", "unique"),
    # Its one reference is at byte +0x779 of its function, which is why this was reported "no usable
    # reference" until the decode window was opened past 0x400. The stored code pointer moves
    # +0x1250 -- the same delta as the referencing function itself.
    0x3B37C98: (0x3B3BCA8, "MENU_PUMP_KICK_PTR_RVA", "bracket"),
    0x3B39848: (0x3B3D858, "PROFILE_OFFSCREEN_SIZE_TABLE_RVA", "bracket"),
    # The stored thunk sits 0xb0 past its referencing function in both images. Renamed from
    # STEAM_INTERFACE_GUARD_RVA 2026-08-31: it is an indirect-call slot in the SteamID64 accessor
    # (`MOV RAX,[0x143b48ff0]; CALL RAX` at 0x140e8d52a, its only reference), not a Steam interface
    # object, and not read by the save-dir builder at all.
    0x3B48FF0: (0x3B4D050, "STEAM_ID_ACCESSOR_CALL_SLOT_RVA", "bracket"),
    0x3D61DC0: (0x3D65E20, "NAV_COST_TABLE_RVA", "bracket"),
    # Slots 6 and 10 of the `TitleStep` step table, which `own_stepper_patch_once` writes our
    # handler into. Each has exactly one reference -- the store that fills it -- so the vote alone
    # refuses, and the `bracket` recorded here understates what is actually known. The initialiser
    # 0xa4f50 fills the whole table in one straight-line run of 24 stores, and both images decode
    # to the same 24 byte offsets, so every slot is aligned by byte offset rather than by a delta.
    # On top of that each slot carries two corroborations that owe nothing to its displacement:
    # the function pointer it stores carries through the function map to the value the 1.17
    # initialiser stores (9 of 12 in the map, 0 mismatched), and the neighbouring slot holds the
    # step's UTF-16 name -- "TitleStep::STEP_GameStepWait" for 6, "TitleStep::STEP_MenuJobWait"
    # for 10 -- each occurring exactly once per image, at the source in 1.16.2 and at candidate+8
    # in 1.17. The slot names itself; no delta is being trusted. Every neighbour at +-0x8/+-0x10 is
    # provably the destination of a different store in the same run, so no competing candidate has
    # any support at all. Re-derive the whole table, checks included:
    #   python3 scripts/carry-step-table-slots.py
    0x3D715E0: (0x3D75650, "TITLE_STEP_IDX6_SLOT_RVA", "bracket"),
    0x3D71620: (0x3D75690, "TITLE_STEP_IDX10_SLOT_RVA", "bracket"),
    # The lazy `CSEblFileManager` slot, and the only one of the menu tracer's five bare literals
    # the vote cannot carry. Its four references sit at file offsets 0x7e1d7, 0x1eed3b, 0x1efbdc
    # and 0x1efbf3 -- the same four offsets in both images, each re-reading the candidate -- but
    # none of the enclosing functions is in the function map, so `carry` has nothing to vote with
    # and reports "no usable reference". Both corroborations here are independent of that:
    #   Bracket -- 0x3d5b078 and 0x3d5b0f4 both move +0x4060, and so do 105 anchors within +-0x400.
    #   Shape   -- the masked referencing instruction reaches the source 10 times in 1.16.2 and
    #              the candidate 10 times in 1.17.
    # `None` for the name means "whatever the tree calls it today". It is written as a bare
    # literal with no constant name at all, so the harvest keys it `<file>:<line>`; freezing that
    # key here would rot on the next edit above line 1248, and freezing an invented constant name
    # would put a name in a tracked ledger that nothing in crates/ declares. If the literal ever
    # leaves the tree this entry produces no row, which is the right answer rather than a stale one.
    #   python3 scripts/map-data-rvas-1162-to-1170.py 0x3d5b088 --confirm 0x3d5f0e8
    0x3D5B088: (0x3D5F0E8, None, "bracket+shape"),
    # The diagnostic-only VTABLE, given a row on purpose. `MENUJOB_IFELSE_VTABLE_DUMP_VA` is
    # declared `= 0x142aa2958` in `er-quickload/src/constants/gaitem_restore.rs` and its every use
    # is a format argument: the install path logs "expect IfElseJob dump 0x..." beside the vtable
    # it actually read. Nothing resolves it, so it cannot be wrong at runtime -- and that is
    # exactly the argument that let `FIRST_SECTION_RVA` into `DETOUR_SAFE_1162_TO_1170`, where it
    # was also inert until it would not have been.
    #
    # It is not a candidate for `rva_role.NOT_AN_ADDRESS`: that list is for values proven to be
    # bounds, and this one is provably the opposite -- RTTI reads `.?AVIfElseJob@MenuJobSequence@CS@@`
    # at the source in 1.16.2 and at the candidate in 1.17. Calling a real vtable "not an address"
    # is the expensive direction, and its twin on the same log line
    # (`MENUJOB_LOADGAME_VTABLE_DUMP_VA`, RVA 0x2ac71e0) is already carried under another name, so
    # leaving this half out means one log line mixes a watched number with an unwatched one.
    #
    # It reaches the ledger through this table rather than through the harvest because neither of
    # the harvest's two questions can see it: its name carries no `RVA`, and its value is a full VA
    # rather than an RVA. Widening either test to admit it would admit far more than it.
    0x2AA2958: (0x2AA59D8, "MENUJOB_IFELSE_VTABLE_DUMP_VA", "rtti"),
}


def string_at(image: bytes, rva: int) -> bytes | None:
    """The NUL-terminated ASCII or UTF-16 string at `rva`, terminator included, or None.

    The terminator is part of the blob on purpose: without it "PressStart" would also match inside
    a hypothetical "PressStartButton", and the whole value of this check is that the match is
    unique.
    """
    for width, decoder in ((1, "ascii"), (2, "utf-16-le")):
        end = rva
        while end + width <= len(image) and image[end : end + width] != b"\0" * width:
            end += width
        text = image[rva:end]
        if len(text) < 6 * width or end == rva:
            continue
        try:
            decoded = text.decode(decoder)
        except UnicodeDecodeError:
            continue
        if decoded.isprintable() and not decoded.isspace():
            return image[rva : end + width]
    return None


def unique_content_confirms(old_image: bytes, new_image: bytes, src: int, dst: int) -> tuple[bool, str]:
    """`src` holds a string that occurs once per image, and in 1.17 that one occurrence is at `dst`."""
    blob = string_at(old_image, src)
    if blob is None:
        return False, "no printable string at the source"
    if old_image.count(blob) != 1:
        return False, f"{old_image.count(blob)} occurrences in 1.16.2, need exactly 1"
    if new_image.count(blob) != 1:
        return False, f"{new_image.count(blob)} occurrences in 1.17, need exactly 1"
    if new_image.find(blob) != dst:
        return False, f"the 1.17 occurrence is at 0x{new_image.find(blob):x}, not 0x{dst:x}"
    shown = blob[:-1].decode("ascii", "replace") if blob[-2:-1] != b"\0" else blob[:-2].decode("utf-16-le", "replace")
    return True, f'"{shown}" occurs exactly once in each image, at the source and at the candidate'


def fnptr_table_confirms(
    old_image: bytes, new_image: bytes, fmap: dict[int, int], src: int, dst: int, entries: int = 8
) -> tuple[bool, str]:
    """Code pointers stored at `src` land, in 1.17, exactly where the function map says they went."""

    def score(at: int) -> tuple[int, int]:
        ok = bad = 0
        for i in range(entries):
            a = struct.unpack_from("<Q", old_image, src + 8 * i)[0]
            b = struct.unpack_from("<Q", new_image, at + 8 * i)[0]
            rva = a - BASE
            if not 0 <= rva < len(old_image) or rva not in fmap:
                continue
            ok, bad = (ok + 1, bad) if b == BASE + fmap[rva] else (ok, bad + 1)
        return ok, bad

    good, bad = score(dst)
    if bad or not good:
        return False, f"{good} entry(ies) correct, {bad} mismatched at 0x{dst:x}"
    for step in (-0x10, -0x8, 0x8, 0x10):
        near_good, near_bad = score(dst + step)
        if near_good and not near_bad:
            return False, f"the same test also passes at 0x{dst + step:x}, so it does not select"
    return True, (
        f"{good} of {entries} entries carry through the function map with 0 mismatches, "
        "and the identical test fails at every +-0x8/+-0x10 neighbour"
    )


def rescue_confirms(md, old: Image, new: Image, fmap, src: int, dst: int, kind: str):
    """Run every corroboration named in `kind`; all must pass."""
    reasons = []
    for part in kind.split("+"):
        if part == "unique":
            ok, why = unique_content_confirms(old.data, new.data, src, dst)
        elif part == "fnptr":
            ok, why = fnptr_table_confirms(old.data, new.data, fmap, src, dst)
        elif part == "bracket":
            ok, why = bracket_confirms(md, old, new, fmap, src, dst)
        elif part == "shape":
            # The same test the `SHAPE_RESCUED` loop runs, reachable from `REFSITE_RESCUED` too:
            # the masked referencing instruction reaches the candidate in 1.17 exactly as often as
            # it reached the source in 1.16.2. Weak alone (a `mov rax, [rip]` shape reaches
            # hundreds of addresses image-wide), which is why it only ever appears beside another
            # part -- it confirms an address the bracket or the content already selected.
            shapes = reference_shapes(md, old, src)
            src_sites = len(shape_sites(old, shapes).get(src, [])) if shapes else 0
            dst_sites = len(shape_sites(new, shapes).get(dst, [])) if shapes else 0
            ok = bool(src_sites) and src_sites == dst_sites
            why = f"{src_sites} source vs {dst_sites} target site(s) of the referencing shape"
        elif part == "rtti":
            # The strongest anchor this file has, and the only one that involves no matching at
            # all: the vtable carries its own mangled class name, the same name sits at the source
            # in 1.16.2 and at the candidate in 1.17, and at neither crossed position -- so a
            # region that happens not to have moved cannot pass by accident. Already used to
            # rescue a weak vote inside `refresh`; reachable here for a vtable the harvest never
            # produced a target for at all.
            confirmed = rtti_confirms(old.data, new.data, src, dst)
            ok = confirmed is not None
            why = confirmed if ok else "the mangled class name does not carry from source to candidate"
        else:
            ok, why = False, f"unknown corroboration {part!r}"
        if not ok:
            return False, f"{part} FAILED: {why}"
        reasons.append(f"{part}: {why}")
    return True, "; ".join(reasons)


def already_mapped(repo: Path) -> set[int]:
    """RVAs the function map and the byte verifier already answer for."""
    out: set[int] = set()
    for name in ("docs/recon/rva-map-1162-to-1170.needed.tsv", "docs/recon/rva-map-1162-to-1170.verified.tsv"):
        path = repo / name
        if not path.is_file():
            continue
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.startswith("#") or not line.strip():
                continue
            try:
                value = int(line.split("\t")[0], 16)
            except ValueError:
                continue
            out.add(value - BASE if value >= BASE else value)
    return out


def refresh(md, old: Image, new: Image, fmap: dict[int, int], repo: Path) -> int:
    """Carry every `*_RVA` constant that lives outside .text and is not already answered."""
    text_va, text_size = old.text
    done = already_mapped(repo)
    targets: dict[str, int] = {}
    # Enum variant values first: an alias and its enum are routinely in different files.
    enum_variants: dict[str, int] = {}
    for path in sorted((repo / "crates").glob("**/*.rs")):
        for variant, value in VARIANT.findall(path.read_text(encoding="utf-8", errors="replace")):
            enum_variants.setdefault(variant, int(value.replace("_", ""), 16))

    unevaluated: dict[str, str] = {}
    # A game address does not need a name. Collected across the whole tree first and merged after
    # the named loop, because the merge asks whether any constant already claims the value -- and
    # the constant and the bare literal are routinely in different files.
    bare: dict[str, int] = {}
    for path in sorted(repo.glob("crates/**/*.rs")):
        source = path.read_text(encoding="utf-8", errors="replace")
        literals = []
        for n, initialiser in CONST.findall(source):
            value = const_value(initialiser.strip())
            if value is None:
                # Reported, not silently dropped -- but only the dangerous class. An initialiser
                # with no hex literal in it at all is an alias or a decimal: `ALIAS` already
                # handles the enum spelling, and a path alias is carried under the constant it
                # names, so there is nothing to warn about and 180-odd such lines would bury
                # everything else. An initialiser that does contain a hex literal and still could
                # not be evaluated is the shape that produced the phantom address, so it prints.
                if "0x" in initialiser:
                    unevaluated.setdefault(n, initialiser.strip().replace("\n", " ")[:60])
                continue
            literals.append((n, value))
        for n, _enum, variant in ALIAS.findall(source):
            value = enum_variants.get(variant)
            if value is not None:
                literals.append((n, value))
        for name, rva in literals:
            if BOUND.search(name):
                continue
            if rva in done:
                continue
            # `.text` is excluded, and the exclusion was tested rather than assumed.
            #
            # It looked like it should not be. 75 of the 83 addresses the running game asked for
            # and could not be placed are in `.text` but absent from `.pdata` -- leaf functions
            # with no unwind data, structurally invisible to a map built from the function table.
            # And a `call rel32` encodes its target exactly as a rip-relative displacement does,
            # `dword[i] + i + 4`, so the reference scan finds their call sites for free.
            #
            # Measured 2026-08-29: allowing them took the table from 304 rows to 329 and killed
            # the game at +145ms, during DLL init, where 304 rows had survived past twenty
            # seconds. The contract this tool advertises -- never wrong, sometimes silent -- is
            # calibrated on eleven `.data` globals. Leaf functions are a class it has never been
            # calibrated on, and the runtime says the vote does not carry them. Re-enabling this
            # needs its own calibration set first.
            if text_va <= rva < text_va + text_size:
                continue
            targets.setdefault(name, rva)
        # Bare LITERALS handed to the address RESOLVER. See `rva_usage.bare_resolver_addresses`:
        # decided from the argument position, never from the value or the spelling, so the
        # thousands of offsets/flags/sanity bounds in these same files are not admitted. Test
        # scopes are skipped for the reason the named loop skips them: a test may name an address
        # precisely to assert the workspace does not use it.
        tests = rva_usage.test_module_spans(source)
        line_offsets = None
        for line_no, rva in rva_usage.bare_resolver_addresses(source):
            if line_offsets is None:
                line_offsets, cursor = [0], source.find("\n")
                while cursor >= 0:
                    line_offsets.append(cursor + 1)
                    cursor = source.find("\n", cursor + 1)
            offset = line_offsets[line_no - 1] if line_no - 1 < len(line_offsets) else 0
            if rva_usage.in_any_span(offset, tests):
                continue
            bare.setdefault(f"{path.relative_to(repo).as_posix()}:{line_no}", rva)

    # The merge, and the one thing it must not do. A synthetic `<file>:<line>` key is fragile by
    # construction -- every edit above the line renames the row -- so it is used only where nothing
    # else names the address. `0x3d5b0f8` is written both as `CSFILE_SINGLETON_RVA` and as a bare
    # literal three lines below its four unnamed siblings; carrying it twice would put a
    # line-number-churning duplicate beside a stable row for no gain.
    #
    # `.text` is reported rather than skipped. The named loop can drop a `.text` address in
    # silence because a constant that names one is visible to `select-needed-1170-rows.py`; a bare
    # literal is visible to nothing, so dropping it here would put it back in the third state this
    # whole harvest exists to end -- neither carried nor reported.
    named_values = set(targets.values())
    text_bare: list[tuple[str, int]] = []
    for name, rva in sorted(bare.items(), key=lambda kv: kv[1]):
        if rva in named_values or rva in done:
            continue
        if text_va <= rva < text_va + text_size:
            text_bare.append((name, rva))
            continue
        targets.setdefault(name, rva)

    # The declared constants are not the whole population; the running game is the authority on
    # what is actually reached. See scripts/record-1170-refusals.py.
    observed = repo / "docs/recon/rva-1170-observed-refusals.txt"
    if observed.is_file():
        for line in observed.read_text(encoding="utf-8").splitlines():
            if line.startswith("#") or not line.strip():
                continue
            try:
                rva = int(line, 16)
            except ValueError:
                continue
            if rva == 0 or rva in done or rva in targets.values():
                continue
            if text_va <= rva < text_va + text_size:
                continue
            targets.setdefault(f"(refused at runtime 0x{rva:x})", rva)

    rows, weak, rtti_rescued = [], [], []
    for name, rva in sorted(targets.items(), key=lambda kv: kv[1]):
        moved, note, votes = carry(md, old, new, fmap, rva)
        if moved is None:
            weak.append((name, rva, note))
            continue
        total = sum(votes.values())
        best = votes[moved]
        # A single unopposed reference, or a contested vote without a clear
        # majority, is reported and not used. The failure this guards against is
        # not a missing address -- that only costs a feature -- but a confident
        # wrong one, which is what put 0x3d6e278 in the first 2.7.0.0 bundle.
        if best < 2 or best * 5 < total * 3:
            # A vtable can rescue itself: it carries its own mangled class name, and a name that
            # occurs once per image is stronger evidence than any number of agreeing
            # displacements. Only for vtables, and only when the crossed positions do not carry
            # the name, so a region that happens not to have moved cannot pass by accident.
            confirmed = rtti_confirms(old.data, new.data, rva, moved)
            if confirmed:
                rtti_rescued.append((name, rva, moved, confirmed))
                rows.append((name, rva, moved, best, total))
                continue
            weak.append((name, rva, f"{note}, winner {best}/{total}"))
            continue
        rows.append((name, rva, moved, best, total))

    # Bracket-and-shape rescues, re-verified here rather than trusted from the table: the shape
    # counts must still match in the two images, so a future patch that changes the referencing
    # code drops the row instead of carrying a stale answer forward.
    shape_rescued = []
    for rva, (moved, name, kind) in sorted(SHAPE_RESCUED.items()):
        if any(row[1] == rva for row in rows):
            continue
        shapes = reference_shapes(md, old, rva)
        src_sites = len(shape_sites(old, shapes).get(rva, [])) if shapes else 0
        dst_sites = len(shape_sites(new, shapes).get(moved, [])) if shapes else 0
        if src_sites and src_sites == dst_sites:
            rows.append((name, rva, moved, src_sites, dst_sites))
            shape_rescued.append((name, rva, moved, kind, src_sites))
            # It failed the vote on the way in -- that is why it is here. Drop the withheld entry
            # so the file does not list the same address as both carried and unusable.
            weak[:] = [entry for entry in weak if entry[1] != rva]
        else:
            weak.append((name, rva, f"{kind} rescue FAILED: {src_sites} source vs {dst_sites} target site(s)"))

    # Single-reference rescues, re-derived here for the same reason: the corroboration is recomputed
    # from the two images on every refresh, so a future patch that moves the string, edits the task
    # table or shifts the neighbourhood drops the row instead of carrying a stale answer forward.
    refsite_rescued = []
    for rva, (moved, name, kind) in sorted(REFSITE_RESCUED.items()):
        if any(row[1] == rva for row in rows):
            continue
        if name is None:
            # The address has no constant name in the tree, so the row takes the harvest's
            # `<file>:<line>` key rather than a frozen invention. No key means nothing writes the
            # address any more and the entry retires itself instead of carrying a stale row.
            name = next((key for key, value in sorted(bare.items()) if value == rva), None)
            if name is None:
                print(
                    f"  refsite rescue 0x{rva:x}: nothing in crates/ writes this address any more, "
                    "so the entry produced no row. Delete it from REFSITE_RESCUED."
                )
                continue
        voted, _note, votes = carry(md, old, new, fmap, rva)
        if voted is not None and voted != moved:
            # A contradiction, not weak evidence. The table says one address and the code that
            # actually references the global says another; refusing is the only safe answer.
            weak.append((name, rva, f"REFUSED: reference votes 0x{voted:x}, table says 0x{moved:x}"))
            continue
        ok, why = rescue_confirms(md, old, new, fmap, rva, moved, kind)
        if not ok:
            weak.append((name, rva, f"{kind} rescue FAILED: {why}"))
            continue
        # `0/0` when the global has no rip-relative reference at all, which is the literal truth
        # and is why the suffix names the corroboration that actually carried it. Writing `1/1`
        # here would claim a reference vote that was never cast.
        best = votes.get(moved, 0)
        total = sum(votes.values())
        rows.append((name, rva, moved, best, total))
        refsite_rescued.append((name, rva, moved, kind, why))
        weak[:] = [entry for entry in weak if entry[1] != rva]
    rows.sort(key=lambda row: row[1])

    head = [
        "# 1.16.2 RVA\t1.17 RVA\tconstant\tvotes",
        "# Generated by scripts/map-data-rvas-1162-to-1170.py --refresh.",
        "# GENERATED WHOLESALE -- DO NOT HAND-EDIT A ROW INTO THIS FILE. --refresh rewrites every",
        "# line below from the two images. A row it cannot reproduce now STOPS the write and is",
        "# named, instead of being deleted at exit 0 with nothing to read afterwards but an address",
        "# that looks like it was never mapped. A row it does not WANT is only dropped when",
        "# scripts/rva_symbols.py can PROVE nothing in crates/ declares the address; otherwise it is",
        "# carried forward under the PRESERVED banner at the foot of the file. Hand knowledge",
        "# belongs in SHAPE_RESCUED /",
        "# REFSITE_RESCUED inside that script, where the corroboration is re-derived from both",
        "# images on every refresh; a hand-derived pair for a FUNCTION belongs in",
        "# rva-map-1162-to-1170.verified.tsv, the curated ledger.",
        "# Data has no content to compare, so each row is carried by the CODE that references it:",
        "# every rip-relative reference in 1.16.2 .text is mapped onto its 1.17 function and the",
        "# same instruction re-read there. `votes` is agreeing references / total. A row with",
        "# fewer than two agreeing references, or without a clear majority, is listed at the",
        "# bottom as UNUSED rather than promoted -- a missing address costs a feature, a confident",
        "# wrong one cost a boot (0x3d6e278, the first 2.7.0.0 cs_system_step).",
        "# A `rtti` suffix means a VTABLE carried itself: the same mangled class name sits at the",
        "# source in 1.16.2 and the destination in 1.17 and at neither crossed position. A name",
        "# that occurs once per image beats any number of agreeing displacements.",
        "# A `shape` suffix means the address is bracketed by anchors that all moved by the same",
        "# delta AND its one referencing instruction shape reaches the candidate exactly as often",
        "# in 1.17 as it reached the source in 1.16.2 -- the fallback for a global referenced only",
        "# from trampoline rubble, where there is no mappable function to vote with.",
        "# A `unique`, `fnptr` or `bracket` suffix means one unopposed reference was promoted by a",
        "# SECOND line of evidence that does not come from that reference: a string occurring exactly",
        "# once per image, a table of code pointers that carry through the function map, or the",
        "# nearest independently-carried anchor on each side having moved by the same delta. All",
        "# three are recomputed from the two images on every refresh -- see REFSITE_RESCUED.",
        "# A `<file>:<line>` in the constant column is an address written as a BARE HEX LITERAL at",
        "# its use site, with no constant name anywhere -- admitted because the workspace hands it",
        "# to the address resolver, never because of how it is spelled or how large it is. Before",
        "# 2026-08-31 the harvest read only `const *RVA*: usize = 0x..`, so four such addresses were",
        "# in neither the body nor the UNUSED list: not verified and not reported unverified, which",
        "# reads afterwards exactly like an address nobody had gotten to yet.",
    ]
    rescued_names = {name for name, _rva, _moved, _cls in rtti_rescued}
    shape_names = {name for name, _rva, _moved, _kind, _n in shape_rescued}
    refsite_kinds = {name: kind for name, _rva, _moved, kind, _why in refsite_rescued}
    body = [
        f"0x{rva:x}\t0x{moved:x}\t{name}\t"
        + (
            f"{best}/{total} rtti"
            if name in rescued_names
            else f"{best}/{total} shape"
            if name in shape_names
            else f"{best}/{total} {refsite_kinds[name]}"
            if name in refsite_kinds
            else f"{best}/{total}"
        )
        for name, rva, moved, best, total in rows
    ]
    tail = ["#", "# UNUSED -- not enough agreement to be worth trusting:"]
    tail += [f"# {name}\t0x{rva:x}\t{note}" for name, rva, note in weak]

    # A row this refresh did not produce is a row somebody typed in.
    #
    # This write is wholesale, and before 2026-08-30 that meant a hand-added pair vanished at exit
    # 0 with nothing naming it -- and the loss reads afterwards as an address that was never
    # mapped, not as one that was deleted. Unlike `select-needed-1170-rows.py`, this file does not
    # carry such a row forward: every line here is supposed to have been re-derived from the two
    # images by this run, and silently keeping a row that no longer earns its votes would turn the
    # ledger into a place where a wrong address can hide behind an old derivation. So it stops
    # instead, names the addresses, and points at the two tables where hand knowledge belongs --
    # `SHAPE_RESCUED` and `REFSITE_RESCUED`, which are re-checked against the images on every
    # refresh, which is exactly the property a hand-typed TSV row does not have.
    #
    # That is the stray class, and it is unchanged. The class below it is different and was the
    # dangerous one: a row this run does not produce and does not want. It was dropped outright,
    # and "does not want" was decided by the name-filtered scan above, so an address written in a
    # spelling that scan cannot read was deleted from a tracked ledger at exit 0. Since 2026-08-30
    # that drop needs a proof from `rva_symbols` -- values, not spellings -- and everything short
    # of a proof is carried forward under `PRESERVED_BANNER`, which is the `select-needed-*.py`
    # behaviour after all, for the class where it is right.
    produced = {rva: moved for _name, rva, moved, _best, _total in rows}
    withheld = {rva: note for _name, rva, note in weak}
    strays, retired, preserved = [], [], []
    seen_rows = set()
    for line in (repo / DATA_MAP).read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        try:
            rva, moved = int(fields[0], 16), int(fields[1], 16)
        except ValueError:
            continue
        rva = rva - BASE if rva >= BASE else rva
        moved = moved - BASE if moved >= BASE else moved
        if produced.get(rva) == moved:
            continue
        if rva in produced:
            # The file and this run disagree about where the datum went. One of the two is a
            # wrong address at a live-looking value, which is the failure that cost a boot.
            strays.append((rva, line, f"this refresh carries it to 0x{produced[rva]:x}"))
        elif rva in withheld:
            # Wanted, and this run declined to carry it. Either somebody typed the pair in to
            # override the vote, or the row used to carry and no longer does. Both are worth a
            # human; neither is worth a silent delete.
            strays.append((rva, line, f"this refresh WITHHELD it: {withheld[rva]}"))
        elif (rva, moved) in seen_rows:
            continue
        else:
            # Not produced by this run, and not wanted by it either. Before 2026-08-30 that was the
            # whole test and the row was deleted: "nothing declares it any more" meant "the
            # name-filtered `CONST`/`ALIAS` scan above did not produce it", which is a fact about a
            # regex, not about the tree. The scan has no bare `rva: 0x..` table-field form, no
            # `pub use .. as ..` form, and no way to see an address that only ever appears as an
            # element of a const array or inside a `Range` band -- all four spellings are in use in
            # `crates/` today.
            #
            # So the question is now put to `rva_symbols`, which resolves values, and only its
            # proven answer may delete. Everything else is carried forward under `PRESERVED_BANNER`
            # and named on stdout, because a row that turns out to be wanted is a feature and a row
            # that turns out to be stale is a line a human deletes in a second.
            seen_rows.add((rva, moved))
            drop, why = retirement_verdict(rva, claims_for(repo, rva), str(repo))
            if drop:
                retired.append((rva, line))
            else:
                preserved.append((rva, line, why))
    for rva, line in sorted(retired):
        print(f"  retired (PROVEN unclaimed, dropped): {line}")
    for rva, line, why in sorted(preserved):
        print(f"  preserved 0x{rva:x}: {why}")
    if preserved:
        print(
            f"  {len(preserved)} row(s) this refresh did not produce were KEPT under "
            f"'{PRESERVED_BANNER}' rather than dropped. Only an address PROVEN unclaimed by "
            f"scripts/rva_symbols.py is retired; the rest are a human's call. Delete a line there "
            f"to drop it -- --refresh will not put it back."
        )
    if strays:
        for rva, line, why in sorted(strays):
            print(f"UNREPRODUCED: 0x{rva:x}  {line}   -- {why}")
        print(
            f"REFUSING to write {DATA_MAP}: {len(strays)} row(s) in it were not produced by this "
            "refresh, and a wholesale rewrite would delete them with no diagnostic. If the pair is "
            "right, move it into SHAPE_RESCUED or REFSITE_RESCUED in this script, where its "
            "corroboration is re-derived from both images every time. If it is stale, delete the "
            "line. Do not let the truncate decide."
        )
        return 1

    kept_block = []
    if preserved:
        kept_block = [
            "#",
            f"{PRESERVED_BANNER} -- {len(preserved)} row(s) this refresh did not produce and could",
            "# NOT prove nothing wants. Kept, not dropped. Before 2026-08-30 a row here was deleted",
            "# at exit 0 because a name-filtered `*RVA*` regex had not produced it -- which is a",
            "# fact about the regex, not about the tree, and the loss reads afterwards as an",
            "# address that was never mapped rather than one that was deleted. The code holding it",
            "# then reads a 1.16.2 address on 1.17 with no refusal line and no fault.",
            "# A row leaves this block in one of three ways: the refresh reproduces it (it moves",
            "# back up into the body); the refresh produces a DIFFERENT pair for it (that is a",
            "# stray and the write stops); or a human deletes the line. --refresh never restores a",
            "# deleted one. The per-row reason is printed on every run, not stored here, so it is",
            "# re-derived from the current tree instead of ageing in place.",
            "# Only an address PROVEN unclaimed by scripts/rva_symbols.py -- every address-capable",
            "# declaration in crates/ evaluated, none of them this address, no bare literal of it",
            "# in code -- is retired and dropped.",
        ]
        kept_block += [line for _rva, line, _why in sorted(preserved)]
    (repo / DATA_MAP).write_text(
        "\n".join(head + body + kept_block + tail) + "\n", encoding="utf-8"
    )
    for name, rva, moved, cls in rtti_rescued:
        print(f"  rtti rescue: {name} 0x{rva:x} -> 0x{moved:x}  {cls[:90]}")
    for name, rva, moved, kind, sites in shape_rescued:
        print(f"  {kind} rescue: {name} 0x{rva:x} -> 0x{moved:x}  ({sites} matching site(s) each side)")
    for name, rva, moved, kind, why in refsite_rescued:
        print(f"  {kind} rescue: {name} 0x{rva:x} -> 0x{moved:x}  {why}")
    for name, initialiser in sorted(unevaluated.items()):
        print(f"  not a literal address, skipped: {name} = {initialiser}")
    for name, rva in text_bare:
        print(
            f"  EXCLUDED (bare literal in .text, not this map's population): 0x{rva:x} {name} -- "
            "a .text address belongs to rva-map-1162-to-1170.functions.tsv; this map is calibrated "
            "on .data globals only. Reported rather than dropped so it cannot be invisible."
        )
    if bare:
        carried = sum(1 for name in bare if name in targets)
        print(
            f"  bare literals handed to the address resolver: {len(bare)} found, {carried} carried "
            f"under a <file>:<line> key, {len(text_bare)} excluded (.text), "
            f"{len(bare) - carried - len(text_bare)} already claimed by a named constant"
        )
    print(
        f"wrote {DATA_MAP}: {len(rows)} usable row(s) "
        f"({len(rtti_rescued)} carried by RTTI), {len(weak)} withheld, "
        f"{len(preserved)} preserved, {len(retired)} retired (PROVEN unclaimed)"
    )
    return 0


CALIBRATION = {
    "game_man": (0x3D69918, 0x3D6D988),
    "game_data_man": (0x3D5DF38, 0x3D61F98),
    "field_area_ptr": (0x3D691D8, 0x3D6D248),
    "cs_system_step": (0x3D85680, 0x3D89700),  # corrected: the bundle pattern was ambiguous
    "world_chr_man_dbg_flags": (0x3D661A0, 0x3D6A210),
    "multiplay_properties": (0x3B11230, 0x3B15230),
    "character_type_properties": (0x3B17C00, 0x3B1BC00),
    "runtime_heap_allocator": (0x4842D40, 0x4846DC0),
    "crypto_spi_registry": (0x4843038, 0x48470B8),
    "title_step_state_table": (0x3D71580, 0x3D755F0),
    "global_hinstance": (0x3D85688, 0x3D89708),
}


def explain(md, old: Image, new: Image, fmap: dict[int, int], target: int) -> None:
    """Print the fate of every candidate reference to `target`.

    "no usable reference" is a single word for four different situations, and they call for
    opposite responses: no candidate bytes at all means the global is not addressed
    rip-relatively from `.text` (look for a struct-relative access instead); candidates whose
    enclosing function is absent from the map means the function map is the thing to improve;
    candidates that decode to a different address mean the bytes were a coincidence and the
    scan is working as intended. Without this, all four read as "the tool cannot do it".
    """
    old_starts = old.function_starts()
    cands = references(old, target)
    print(f"0x{target:x}: {len(cands)} candidate displacement site(s) in .text")
    tally = {"no enclosing function": 0, "function not in map": 0, "decodes elsewhere": 0, "usable": 0}
    for disp_at in cands:
        func = enclosing(old_starts, disp_at)
        if func is None:
            tally["no enclosing function"] += 1
            continue
        # Decode before asking the function map. A candidate that decodes elsewhere is a
        # coincidence and says nothing about map coverage; reporting it as "function not in map"
        # would send the next reader off to improve the function map over bytes that were never a
        # reference at all.
        found = instruction_index(md, old, func, disp_at, target)
        if found is None:
            tally["decodes elsewhere"] += 1
            continue
        index, at_offset = found
        if func not in fmap:
            tally["function not in map"] += 1
            print(f"  0x{disp_at:x}  in fn 0x{func:x}  REAL reference, fn NOT IN FUNCTION MAP")
            continue
        moved = displacement_of(md, new, fmap[func], index, at_offset)
        tally["usable"] += 1
        print(
            f"  0x{disp_at:x}  in fn 0x{func:x} -> 0x{fmap[func]:x}  insn #{index}  "
            f"votes 0x{moved:x}" if moved is not None else
            f"  0x{disp_at:x}  in fn 0x{func:x} -> 0x{fmap[func]:x}  insn #{index}  no displacement there"
        )
    for reason, count in tally.items():
        print(f"  {count:5d}  {reason}")


def reference_shapes(md, image: Image, target: int) -> list[tuple[str, str]]:
    """`(mnemonic+operand shape, masked bytes)` for every instruction that really addresses `target`.

    The displacement is blanked, so what is left is what a patch does not change: the opcode and
    the registers. Two addresses in two builds that are referenced by the same multiset of shapes,
    from the same number of sites, are the same datum -- or a coincidence that has to repeat itself
    once per reference.
    """
    starts = image.function_starts()
    out = []
    for disp_at in references(image, target):
        func = enclosing(starts, disp_at)
        if func is None:
            continue
        window = image.data[func : disp_at + 16]
        for insn in md.disasm(window, BASE + func):
            pos = insn.address - BASE - func
            if insn.disp_size == 4 and func + pos + insn.disp_offset == disp_at:
                if insn.address - BASE + insn.size + insn.disp != target:
                    break
                raw = bytearray(insn.bytes)
                for i in range(insn.disp_offset, min(insn.disp_offset + 4, len(raw))):
                    raw[i] = 0
                out.append((f"{insn.mnemonic} {insn.op_str.split(',')[0]}", raw.hex()))
                break
            if func + pos > disp_at:
                break
    return sorted(out)


def shape_search(md, old: Image, new: Image, target: int, lo: int, hi: int) -> int:
    """Find `target`'s 1.17 address by looking for the same instruction shape in the new image.

    The last resort, for a global whose only reference lives in a trampoline stub. Reference
    voting needs the enclosing function to be mappable; the dearxan'd image's stub regions are
    `jmp`/`int3` rubble that maps to nine places or none, so a getter that lives there can never
    be voted on. But the getter's own bytes are still a signature: `movzx eax, byte ptr [rip+d];
    ret` with the displacement blanked. Search the new image for that exact masked shape, read
    where each hit points, and keep the ones landing in the plausible window.

    This is weaker than voting and is reported as such -- one hit in the window is a candidate,
    several is a genuine ambiguity, none says the shape itself changed. It is offered because the
    alternative for such a global is a delta guess, and a guess that lands on a live neighbouring
    byte is far worse than an honest "unknown".
    """
    shapes = reference_shapes(md, old, target)
    if not shapes:
        print(f"0x{target:x}: no reference to take a shape from")
        return 1
    print(f"0x{target:x}: searching 1.17 for {len(shapes)} reference shape(s), window 0x{lo:x}..0x{hi:x}")
    va, size = new.text
    found: dict[int, list[str]] = {}
    for shape, hexbytes in shapes:
        raw = bytes.fromhex(hexbytes)
        # The blanked displacement bytes are the zero run; everything else must match exactly.
        disp_at = raw.find(b"\x00\x00\x00\x00")
        if disp_at < 0:
            continue
        head, tail = raw[:disp_at], raw[disp_at + 4 :]
        at = new.data.find(head, va, va + size)
        while at >= 0:
            end = at + len(raw)
            if new.data[at + disp_at + 4 : end] == tail:
                disp = int.from_bytes(new.data[at + disp_at : at + disp_at + 4], "little", signed=True)
                points_at = at + len(raw) + disp
                if lo <= points_at <= hi:
                    found.setdefault(points_at, []).append(f"{shape} @0x{at:x}")
            at = new.data.find(head, at + 1, va + size)
    if not found:
        print("  no shape match points into the window -- the referencing code itself changed")
        return 1
    for address, where in sorted(found.items()):
        print(f"  0x{address:x}  from {len(where)} site(s): {', '.join(where[:4])}")
    if len(found) == 1:
        only = next(iter(found))
        print(f"CANDIDATE 0x{only:x} (single shape match in the window; weaker than a reference vote)")
        return 0
    print(f"AMBIGUOUS: {len(found)} addresses match the shape in this window")
    return 1


def shape_sites(image: Image, shapes: list[tuple[str, str]]) -> dict[int, list[str]]:
    """`{address: [site, ...]}` for every masked shape occurrence anywhere in `.text`.

    Deliberately boundary-FREE: it scans bytes and never decodes from a `.pdata` function start.
    That matters because the addresses this fallback exists for are referenced from the dearxan'd
    image's trampoline rubble, where decoding forward from the enclosing `.pdata` entry
    desynchronises long before it reaches the instruction -- so a boundary-based count reports
    zero references to an address that plainly has one.
    """
    va, size = image.text
    found: dict[int, list[str]] = {}
    for shape, hexbytes in shapes:
        raw = bytes.fromhex(hexbytes)
        disp_at = raw.find(b"\x00\x00\x00\x00")
        if disp_at < 0:
            continue
        head, tail = raw[:disp_at], raw[disp_at + 4 :]
        at = image.data.find(head, va, va + size)
        while at >= 0:
            if image.data[at + disp_at + 4 : at + len(raw)] == tail:
                disp = int.from_bytes(
                    image.data[at + disp_at : at + disp_at + 4], "little", signed=True
                )
                found.setdefault(at + len(raw) + disp, []).append(f"{shape} @0x{at:x}")
            at = image.data.find(head, at + 1, va + size)
    return found


def confirm(md, old: Image, new: Image, target: int, candidate: int) -> int:
    """Compare how `target` is referenced in 1.16.2 with how `candidate` is referenced in 1.17.

    The fallback for a global whose only reference sits in a function the function map could not
    pair -- the reference vote has nothing to vote with, but the reference itself is still
    evidence, and so is how rare its shape is.
    """
    shapes = reference_shapes(md, old, target)
    if not shapes:
        print(f"0x{target:x}: no reference to take a shape from")
        return 1
    before = shape_sites(old, shapes).get(target, [])
    everywhere = shape_sites(new, shapes)
    after = everywhere.get(candidate, [])
    # The shape's operand text comes from the 1.16.2 instruction in both lines -- that is inherent
    # to matching on a masked shape, since the displacement is exactly what was blanked. Labelling
    # it plainly, because printed unqualified it reads as though the two images decoded to the
    # same displacement, which would be a much stronger claim than this test actually makes. The
    # site addresses are per-image and are the real content of these lines.
    print(f"1.16.2 0x{target:x}: {len(before)} site(s) at  {', '.join(before[:4])}")
    print(f"1.17   0x{candidate:x}: {len(after)} site(s) at  {', '.join(after[:4])}")
    print("       (operand text above is the 1.16.2 shape; its displacement is masked by design)")
    print(f"1.17   the same shape reaches {len(everywhere)} distinct address(es) image-wide")
    if before and len(before) == len(after):
        print("CONFIRMED: the shape references the candidate exactly as often as it referenced the source")
        return 0
    print("NOT CONFIRMED: the reference counts differ")
    return 1


# The shapes an `*_RVA` initialiser is written in, and what each must evaluate to. The subtraction
# is the whole reason this exists: `ADD_DEFAULT_FILE_LOAD_PROCESS_RVA: usize = 0x142658c60 -
# 0x140000000` was read as 0x142658c60 -- the MINUEND -- and that phantom address, 1.1 GB past the
# image, was published in this map's unused list as an unmappable data global for as long as the
# list has existed. It is a `.text` function the function map already carries. A scraper that
# mis-classifies without complaining is the same failure family as an audit that reports zero while
# real sites exist, so the shape is pinned rather than left to a regex nobody re-reads.
CONST_PARSER_CASES = [
    ("pub const A_RVA: usize = 0x142658c60 - 0x140000000;", "A_RVA", 0x2658C60),
    ("const B_RVA: u32 = 0x48464a8;", "B_RVA", 0x48464A8),
    ("const C_RVA: usize = 0x3d8_567c;", "C_RVA", 0x3D8567C),
    ("const D_RVA: usize = 0x140000000 + 0x1000;", "D_RVA", 0x140001000),
    ("const E_RVA: usize = 0x2000 - 0x1000 - 0x500;", "E_RVA", 0xB00),
    ("const F_RVA: usize = er_game_base::rva::G_RVA;", "F_RVA", None),
    ("const G_RVA: usize = 50;", "G_RVA", None),
    ("const H_RVA: usize = SIZE_RVA as usize;", "H_RVA", None),
    ("const I_RVA: usize = 0x1000 - 0x2000;", "I_RVA", None),
]


def selftest_const_parser() -> int:
    """Every `*_RVA` initialiser shape evaluates whole, or is refused -- never half-read."""
    bad = 0
    for source, name, want in CONST_PARSER_CASES:
        found = CONST.findall(source)
        if not found:
            print(f"SELFTEST FAIL: {name}: the declaration regex did not match {source!r}")
            bad += 1
            continue
        got_name, initialiser = found[0]
        got = const_value(initialiser.strip())
        if got_name != name or got != want:
            print(
                f"SELFTEST FAIL: {source!r} -> ({got_name}, "
                f"{got if got is None else hex(got)}), want ({name}, {want if want is None else hex(want)})"
            )
            bad += 1
    print(f"  const parser: {len(CONST_PARSER_CASES) - bad}/{len(CONST_PARSER_CASES)} shapes correct")
    return bad


# --------------------------------------------------------------------------------------------
# The positive control for the one code path that deletes
# --------------------------------------------------------------------------------------------

# The pre-2026-08-30 scan, frozen as LITERALS. `refresh()` decided a row was `retired` -- and
# deleted it -- when this scan no longer produced the address. These are its patterns, spelled out
# rather than composed from the live `CONST` / `ALIAS` / `VARIANT` / `BOUND` / `LITERAL_ARITHMETIC`
# objects above.
#
# Composing them would destroy the proof. A control assembled from the live pieces widens whenever
# they widen, so "the old scan could not see this spelling" quietly becomes "the new one cannot see
# it either" -- the opposite claim, asserted in the same words. That nearly happened to
# `check-stale-rva-calls.py`, whose controls were built from its live pattern.
LEGACY_CONST = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*([^;]+);"
)
LEGACY_ALIAS = re.compile(
    r"const\s+([A-Z0-9_]*RVA[A-Z0-9_]*)\s*:\s*(?:usize|u32|u64)\s*=\s*(\w+)::(\w+)\s+as\s+"
    r"(?:usize|u32|u64)"
)
LEGACY_VARIANT = re.compile(r"^\s*(\w+)\s*=\s*(0x[0-9a-fA-F_]+)\s*,", re.M)
LEGACY_BOUND = re.compile(r"_(MIN|MAX|BOUND|BASE|SIZE|LEN|LENGTH|COUNT|END|START|STRIDE|ALIGN)$")
LEGACY_LITERAL_ARITHMETIC = re.compile(
    r"\A\s*0x[0-9a-fA-F_]+(?:\s*[-+]\s*0x[0-9a-fA-F_]+)*\s*\Z"
)


def legacy_declared(text: str) -> set[int]:
    """Every address the pre-fix scan produced, over one blob of source."""
    out: set[int] = set()
    variants: dict[str, int] = {}
    for variant, value in LEGACY_VARIANT.findall(text):
        variants.setdefault(variant, int(value.replace("_", ""), 16))
    for name, initialiser in LEGACY_CONST.findall(text):
        if LEGACY_BOUND.search(name) or not LEGACY_LITERAL_ARITHMETIC.match(initialiser.strip()):
            continue
        total, sign = 0, 1
        for token in re.findall(r"[-+]|0x[0-9a-fA-F_]+", initialiser):
            if token == "+":
                sign = 1
            elif token == "-":
                sign = -1
            else:
                total += sign * int(token.replace("_", ""), 16)
        if total >= 0:
            out.add(total)
    for name, _enum, variant in LEGACY_ALIAS.findall(text):
        if LEGACY_BOUND.search(name):
            continue
        value = variants.get(variant)
        if value is not None:
            out.add(value)
    return out


# Four addresses and the spelling each one is written in. Frozen source, so the control keeps
# meaning what it means after the tree moves on:
#
#   0x111000  an ordinary `const *_RVA: usize = 0x..`   -- Both scans see it. Present only to prove
#             the frozen legacy scan still works; a control set the old scan finds nothing in makes
#             every "the old one missed it" assertion vacuous.
#   0x222000  a `_MAX`-suffixed name, removed by `BOUND`. This is the real
#             GX_CMD_QUEUE_WRAPPER_RVA_MAX spelling; the sibling ledger carries a container row for
#             it that read "delete the line" until this fix.
#   0xb0d400  an enum discriminant used inline, with no aliasing constant -- the real
#             `MenuTraceRva::MenuJobWait`, three live use sites on the autoload path.
#   0x333000  a bare `rva: 0x..` field in a HookSpec table, with no constant name at all. 53 of
#             these exist in crates/ (39 in er-reload-trace alone) and the scan has no rule for the
#             shape, so every one of them looked retired.
CONTROL_SOURCE = """
pub const NAV_COST_TABLE_RVA: usize = 0x111000;
pub const GX_CMD_QUEUE_WRAPPER_RVA_MAX: usize = 0x222000;
#[repr(u32)]
pub enum MenuTraceRva {
    TaskEnqueue = 0x007a7b60,
    MenuJobWait = 0x00b0d400,
}
pub fn drive(base: usize) -> usize {
    base + MenuTraceRva::MenuJobWait as usize
}
pub const SPECS: &[HookSpec] = &[HookSpec { rva: 0x333000, name: "trace" }];
"""


def selftest_retirement_gate() -> int:
    """The `retired` drop fires only on a proof, and the proof is not the old regex's silence."""
    import tempfile

    failures = []
    scratch = Path(tempfile.mkdtemp()) / "crates" / "a" / "src"
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "lib.rs").write_text(CONTROL_SOURCE, encoding="utf-8")
    fixture = rva_symbols.Index.build(root=str(scratch.parent.parent.parent))

    # Non-VACUITY first, before any claim about contents. An empty set makes every `not in` below
    # true and every assertion pass for the wrong reason.
    old_sees = legacy_declared(CONTROL_SOURCE)
    if old_sees != {0x111000}:
        failures.append(
            f"the frozen legacy scan is broken, so 'the old one missed it' proves nothing: it "
            f"found {sorted(hex(v) for v in old_sees)}, expected exactly ['0x111000']"
        )
    if fixture.files_read < 1 or fixture.universe_size() < 4:
        failures.append(
            f"the control fixture did not parse: {fixture.files_read} file(s), "
            f"{fixture.universe_size()} address-capable declaration(s)"
        )

    for address, spelling in (
        (0x222000, "a _MAX-suffixed constant"),
        (0xB0D400, "an inline enum discriminant"),
        (0x333000, "a bare `rva:` table field with no constant name"),
    ):
        if address in old_sees:
            failures.append(
                f"control is worthless: the OLD scan already produced 0x{address:x} ({spelling})"
            )
        drop, why = retirement_verdict(address, fixture.claims(address))
        if drop:
            failures.append(f"0x{address:x} ({spelling}) is STILL dropped as retired: {why}")

    # ...and retirement must remain REACHABLE, or the fix has merely disabled the mechanism. In a
    # tree the resolver understands completely, an address nothing declares is proven unclaimed.
    drop, why = retirement_verdict(0x999000, fixture.claims(0x999000))
    if not drop:
        failures.append(
            f"an address nothing declares is no longer retired in a fully-resolved tree ({why}); "
            "the gate can never delete anything, which is a different bug"
        )
    # A broken walk is not a clean walk.
    if retirement_verdict(0x111000, None)[0]:
        failures.append("a row is dropped when the resolver could not run at all")

    # The live tree. A resolver that only ever runs against its own fixture is a fixture.
    repo = Path(__file__).resolve().parent.parent
    live = claims_for(repo, 0xB0D400)
    if live is None or live.files_read < 200:
        failures.append("the live resolver did not read the real tree")
    elif retirement_verdict(0xB0D400, live, str(repo))[0]:
        failures.append("0xb0d400 is retired against the LIVE tree; three use sites reach it")
    if live is not None:
        print(
            f"  retirement gate: live resolver read {live.files_read} sources, "
            f"{live.universe} address-capable declarations, {len(live.residue)} unevaluated and "
            f"wide enough to hold a .text RVA."
            + (
                ""
                if not live.residue
                else " While that is non-zero NO address can be PROVEN unclaimed, so no row can be"
                " retired from this ledger at all and every unreproduced row is preserved."
            )
        )
    for line in failures:
        print(f"SELFTEST FAIL: {line}")
    print(f"  retirement gate: {len(failures)} failure(s)")
    return len(failures)


def _untracked_input(repo: Path, relative: str) -> Path:
    """Locate one of this tool's three gitignored inputs: here first, then the main worktree.

    The two deobf images and `rva-map-1162-to-1170.functions.tsv` are all gitignored, and a
    gitignored file is never copied into a `git worktree`. So every one of the three is absent
    from a linked checkout, and the missing-input branch prints `SKIP: missing <path>` and returns
    0 -- a tool reporting that it did not run, at the exit code of a tool that ran and found
    nothing. That is the same fallback `map-rvas-1162-to-1170.py` and
    `check-ledger-section-kind.py` already take, for the same reason and by the same route.

    `--old`, `--new` and `--map` still win: this is only what the defaults resolve to.
    """
    local = repo / relative
    if local.exists():
        return local
    # `git rev-parse --git-common-dir` resolves to the primary checkout's `.git` from inside a
    # linked worktree, and to our own otherwise, so its parent is the main working tree.
    try:
        import subprocess

        common = subprocess.run(
            ["git", "-C", str(repo), "rev-parse", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if common.returncode == 0:
            main_root = (repo / common.stdout.strip()).resolve().parent
            candidate = main_root / relative
            if candidate.is_file():
                return candidate
    except Exception:
        pass
    return local


def main() -> int:
    repo = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rvas", nargs="*", help="1.16.2 data RVAs or VAs (hex)")
    # `--refresh` REWRITES a tracked ledger. Without a way to point it at a scratch tree the only
    # way to exercise that path is to run it on the real one, which is how a destructive tool gets
    # shipped untested. Everything else defaults relative to whatever `--repo` names.
    ap.add_argument("--repo", type=Path, default=repo, help="tree to read crates/ and docs/ from")
    ap.add_argument("--old", type=Path, default=None)
    ap.add_argument("--new", type=Path, default=None)
    ap.add_argument("--map", type=Path, default=None)
    ap.add_argument("--refresh", action="store_true", help="rewrite the tracked data map")
    ap.add_argument(
        "--explain",
        action="store_true",
        help="show every candidate reference and why it was kept or dropped",
    )
    ap.add_argument(
        "--confirm",
        metavar="NEW_RVA",
        help="check a PREDICTED 1.17 address by comparing reference shapes in both images",
    )
    ap.add_argument(
        "--shape-search",
        metavar="LO:HI",
        help="last resort: find the 1.17 address by matching the reference instruction's shape, "
        "restricted to this 1.17 RVA window (hex, e.g. 0x458c000:0x458e000)",
    )
    ap.add_argument(
        "--callers",
        action="store_true",
        help="carry a .text address by its CALLERS -- for a function whose body changed, which no "
        "body-signature mapper can identify",
    )
    ap.add_argument("--selftest", action="store_true")
    # The source contracts, RUNNABLE with nothing installed. The const parser and the retirement
    # gate read `crates/` and nothing else; making them need capstone (and therefore uv, and
    # therefore the network) is what kept them out of `scripts/check.sh`, and a selftest no gate
    # runs is a selftest that rots. `--selftest` still continues into the image calibration.
    ap.add_argument(
        "--selftest-source",
        action="store_true",
        help="run only the source-parsing contracts (no capstone, no images)",
    )
    args = ap.parse_args()
    repo = args.repo
    if args.selftest or args.selftest_source:
        # Before `_ensure`, for both flags. These two read `crates/` and nothing else, so running
        # them first means a checkout with no capstone still fails on a regressed scraper or on a
        # retirement gate that has gone back to deleting rows on a regex's silence.
        #
        # It also keeps the tool measurable by `scripts/audit-selftest-vacuity.py`, which runs
        # gates in-process with `re` neutered: `_ensure` re-execs under uv, and the re-exec threw
        # the blinding away with the process, so the sweep could only report UNMEASURED for the one
        # tool in this repo that deletes ledger rows.
        broken = selftest_const_parser() + selftest_retirement_gate()
        if args.selftest_source:
            return 1 if broken else 0
        if broken:
            return 1

    # Everything past this point decodes instructions.
    _ensure("capstone")
    _ensure("numpy")
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs
    from capstone.x86 import X86_OP_IMM

    global CS_OP_IMM_TYPE
    CS_OP_IMM_TYPE = (X86_OP_IMM,)

    args.old = args.old or _untracked_input(repo, "eldenring-deobf.bin")
    args.new = args.new or _untracked_input(repo, "eldenring-deobf-1.17.bin")
    args.map = args.map or _untracked_input(repo, "docs/recon/rva-map-1162-to-1170.functions.tsv")

    for path in (args.old, args.new, args.map):
        if not path.is_file():
            print(f"SKIP: missing {path}")
            return 0

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    old, new = Image(args.old), Image(args.new)
    fmap = {}
    for line in args.map.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        a, b = line.split()[:2]
        fmap[int(a, 16)] = int(b, 16)

    if args.selftest:
        # The contract is "NEVER WRONG", not "ALWAYS ANSWERS".
        #
        # A missing address costs a feature: the caller refuses and says so. A wrong
        # address costs the session -- that is what 0x3d6e278 did, and it looked
        # authoritative the whole way down. So a miss is reported and tolerated; a
        # disagreement fails the run.
        wrong, missed = [], []
        for name, (src, want) in CALIBRATION.items():
            got, note, _votes = carry(md, old, new, fmap, src)
            if got == want:
                status = "ok"
            elif got is None:
                status = "miss"
                missed.append(f"{name} (0x{src:x}): {note}")
            else:
                status = "WRONG"
                wrong.append(f"{name}: got 0x{got:x}, want 0x{want:x} ({note})")
            print(f"  {status:5s} {name:28s} 0x{src:x} -> {got and hex(got)}  [{note}]")
        for line in wrong:
            print(f"SELFTEST FAIL: {line}")
        for line in missed:
            print(f"  unresolved (tolerated): {line}")
        print(
            f"selftest: {len(CALIBRATION) - len(wrong) - len(missed)}/{len(CALIBRATION)} carried, "
            f"{len(missed)} unresolved, {len(wrong)} WRONG"
        )
        return 1 if wrong else 0

    if args.refresh:
        return refresh(md, old, new, fmap, repo)

    if not args.rvas:
        ap.error("give at least one RVA, --refresh, or --selftest")
    if args.callers:
        worst = 0
        for text in args.rvas:
            value = int(text, 16)
            rva = value - BASE if value >= BASE else value
            moved, note, votes = carry_code(md, old, new, fmap, rva)
            if moved is None:
                print(f"0x{rva:x}  ->  UNMAPPED   {note}")
                worst = 1
            else:
                print(f"0x{rva:x}  ->  0x{moved:x}   {note}  delta +0x{moved - rva:x}")
        return worst

    if args.shape_search:
        if len(args.rvas) != 1:
            print("--shape-search takes exactly one 1.16.2 RVA")
            return 2
        value = int(args.rvas[0], 16)
        lo_s, hi_s = args.shape_search.split(":")
        return shape_search(
            md, old, new,
            value - BASE if value >= BASE else value,
            int(lo_s, 16), int(hi_s, 16),
        )

    if args.confirm:
        if len(args.rvas) != 1:
            print("--confirm takes exactly one 1.16.2 RVA")
            return 2
        value = int(args.rvas[0], 16)
        cand = int(args.confirm, 16)
        return confirm(
            md,
            old,
            new,
            value - BASE if value >= BASE else value,
            cand - BASE if cand >= BASE else cand,
        )

    if args.explain:
        for text in args.rvas:
            value = int(text, 16)
            explain(md, old, new, fmap, value - BASE if value >= BASE else value)
        return 0

    for text in args.rvas:
        value = int(text, 16) if text.startswith("0x") else int(text)
        rva = value - BASE if value >= BASE else value
        got, note, votes = carry(md, old, new, fmap, rva)
        if got is None:
            print(f"0x{rva:x}  ->  UNMAPPED   {note}")
        else:
            print(f"0x{rva:x}  ->  0x{got:x}   {note}  votes={ {hex(k): v for k, v in votes.items()} }")
    return 0


if __name__ == "__main__":
    sys.exit(main())
