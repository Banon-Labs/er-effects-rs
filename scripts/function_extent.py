"""Where a function ends, for every offline tool in this repo that decodes forward.

The one rule this module exists to enforce
------------------------------------------
A linear x86-64 decode starting at an address in the de-Arxan'd images is trustworthy only
inside one function. Past the `ret` the decoder is reading inter-function padding and the
deobfuscator's leftover bytes -- not a uniform `cc`/`90` run -- so it RESYNCHRONISES into
plausible-looking instructions that were never assembled, including branches. Anything that
decides a verdict on those bytes is deciding it on noise.

The class has been found five times as of 2026-08-31, each time as a forward decode bounded by
a byte count instead of by a function EXTENT:

  1. `audit-1170-hook-targets.py::patch_safe` read a flat 0x400 from the hook target. On a
     14-byte leaf that is 0x3f2 bytes of the neighbours, and it manufactured a
     `jno 0x14067ac91` -- a branch into the patch operand -- out of one padding byte. The 1.17
     counterpart is the same fourteen bytes and stayed green only because its junk pad byte was
     `28` instead of `83`.
  2. 12 false `DIVERGE` verdicts (2026-08-30), a verifier decoding past a tail call.
  3. 31 false `SHAPE-DIFF`s, same date, same cause.
  4. A trampoline walk that counted bytes past its own `ret`.
  5. `check-singleton-field-offsets.py::_follow` walked five instructions from a
     `mov r64,[rip+SessionManager]` at 1.17 `0x140257d0f`, whose function `.pdata` ends at
     `0x140257d22`, and collected `lea edx,[rax+0x18]` at `0x140257d28` -- Six bytes into the
     next function, where `rax` holds something else. That phantom was the sole evidence for the
     gate's headline claim that "SessionManager gains one field in 1.17". The same walk invented
     `CS::PlayerGameData +0xe5` in both images, 0x17 bytes past the declared end each time --
     symmetric, so it never showed up as a lost field either.

Three sources, most authoritative first
---------------------------------------
  1. `.pdata` declares a function starting at the address -- the linker's own answer, with chunk
     runs merged by `function_regions` so a split function's extent is the whole run rather than
     its first couple of dozen bytes.
  2. `.pdata` declares one containing it. The address is then mid-function, which is a separate
     question, but the enclosing extent is still the right bound for reading its bytes.
  3. Neither: an unwindless leaf. The x64 ABI omits unwind data for a function that allocates no
     stack and calls nothing, so ELDEN RING's small getters have no `.pdata` entry at all --
     `.pdata` is blind across 146,715 holes and a missing entry is not a missing function. Its
     end is DECODED by `verify-rva-map-1170.py::leaf_extent`, the watermark rule that refuses to
     stop at a `ret` some earlier branch reaches past.

Measured over the 425 rows both detour ledgers admit, in both images: 359 declared, 0 enclosed,
66 decoded leaves, 0 unknown. `body_end` returning None is a fallback nothing in the current
tables takes, and a caller that gets None must refuse rather than substitute a byte count.

Why it lives here and not in the tool that first needed it
----------------------------------------------------------
It was written inside `scripts/audit-1170-hook-targets.py`. Four more tools then needed the same
answer, and a second implementation of extent resolution is the next divergence bug -- the rule's
own history is two earlier wrong versions (first-`ret` truncation, and a padding-only tail-`jmp`
test that walked straight through the de-Arxan'd images' leftover gap bytes). So the primitive
moved to a module with a plain importable name and no import side effects, and every consumer --
`audit-1170-hook-targets.py` included -- imports it from here.

`scripts/check-decode-extent-bounds.py` is the gate that keeps instance six from being written.
"""

from __future__ import annotations

import importlib.util
import os
import sys

BASE = 0x140000000
# A body longer than this is read only this far. It is a cap on top of an extent, never the
# window: `body_end` supplies the window, and this only stops a pathological decode of a
# 0x20000-byte function when a caller has no interest past the first kilobyte.
DEFAULT_SCAN_CAP = 0x400

_VERIFY = None


def verify_rules():
    """`scripts/verify-rva-map-1170.py`, imported for its function extent rules.

    Imported, not RE-derived. That file already imports `audit-1170-hook-targets.py` for
    `trampoline_walk`; both imports are lazy -- inside a function, on first use -- so neither
    module executes the other at import time and the cycle never closes. The sibling's name has
    hyphens in it, so it is loaded by path rather than by `import`.
    """
    global _VERIFY
    if _VERIFY is None:
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "verify-rva-map-1170.py")
        spec = importlib.util.spec_from_file_location("_verify_rva_map_1170", path)
        if spec is None or spec.loader is None:
            raise RuntimeError(f"cannot load {path}, which holds the function-extent rules")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        _VERIFY = module
    return _VERIFY


_SECTION_KIND = None


def section_rules():
    """`scripts/check-ledger-section-kind.py`, imported for its PE section table reader.

    Imported, not RE-derived, for the same reason `verify_rules` is: that file already owns the
    "is this VA executable" question for ledger rows, including the detail that decides the whole
    thing -- section size is `max(virtual, raw)`, so the 1.17 `.data`'s zero-filled tail (VSZ
    0xd51bc4 against RSZ 0x249e00) counts as inside `.data` rather than outside every section.
    Loaded by path, and lazily, because the name has hyphens and because that module must not run
    at import time.
    """
    global _SECTION_KIND
    if _SECTION_KIND is None:
        path = os.path.join(
            os.path.dirname(os.path.abspath(__file__)), "check-ledger-section-kind.py"
        )
        spec = importlib.util.spec_from_file_location("_check_ledger_section_kind", path)
        if spec is None or spec.loader is None:
            raise RuntimeError(f"cannot load {path}, which holds the section-table reader")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        _SECTION_KIND = module
    return _SECTION_KIND


_SECTIONS = {}


def executable_at(blob, va):
    """Is `va` inside a section this image marks IMAGE_SCN_MEM_EXECUTE?

    The other half of "DO NOT DECODE THIS". A decode that does not establish where the function
    ends and a decode that does not establish that the destination is code at all are the same
    disease, and the second one produced the most extreme instance found so far: the deleted
    `docs/recon/rva-1170-detour-audited.tsv` promoted 85 rows on an "unwindless leaf" clause, and
    all 85 named non-executable memory. The clause cannot fire on a real leaf by accident and it
    could not miss a global: `.pdata` declares no enclosing function for a `.data` address for
    exactly the same reason it declares none for an unwindless leaf, so feeding that ledger the
    GLOBALS table made every global look like a leaf. Four of them were 24 bytes of `00` in the
    `.data` virtual tail; `00 00` decodes as `add byte ptr [rax], al`, and three of those is the
    "6B relocatable" the ledger recorded.

    Returns None when the image cannot be parsed, which callers must treat as "do not know",
    never as "yes".
    """
    key = id(blob)
    cached = _SECTIONS.get(key)
    if cached is None or cached[0] is not blob:
        rules = section_rules()
        try:
            cached = (blob, rules.sections(blob))
        except Exception:  # noqa: BLE001 - Refuse is defined in the sibling module
            cached = (blob, None)
        _SECTIONS[key] = cached
    secs = cached[1]
    if secs is None:
        return None
    _name, executable = section_rules().classify(secs, va)
    return executable


_REGIONS = {}


def declared_functions(blob):
    """`({begin: end}, {begin}, [(begin, end)])` for `blob`: chunk runs merged, once per image.

    Cached because callers ask per address while the parse walks the whole `.pdata` table and the
    sorted span list is 175k entries. Re-parsing `.pdata` per candidate row is what turned a
    409-row run into minutes. The blob itself is kept in the cache value so its `id` cannot be
    recycled under the key while the entry is alive.
    """
    cached = _REGIONS.get(id(blob))
    if cached is None or cached[0] is not blob:
        extents, starts = verify_rules().function_regions(blob)
        cached = (blob, extents, starts, sorted(extents.items()))
        _REGIONS[id(blob)] = cached
    return cached[1], cached[2], cached[3]


def inside_declared_function(rva, spans):
    """The `(begin, end)` of a declared function that strictly contains `rva`, or None."""
    lo, hi = 0, len(spans)
    while lo < hi:
        mid = (lo + hi) // 2
        if spans[mid][0] <= rva:
            lo = mid + 1
        else:
            hi = mid
    if lo and spans[lo - 1][0] < rva < spans[lo - 1][1]:
        return spans[lo - 1]
    return None


def body_end(blob, va, limit=DEFAULT_SCAN_CAP):
    """The RVA one past the last byte of the function at or containing `va`.

    `None` when it cannot be told -- which is an answer, and callers must treat it as a refusal
    rather than falling back to a byte count. See the module docstring for the three sources and
    for the five times this was got wrong by not asking.

    `limit` bounds only the decode arm (source 3), where there is no declaration to stop at and
    it is the one thing standing between the sweep and the rest of the image.
    """
    # Not code is not a function. Checked first, because the leaf arm below is exactly where a
    # data address gets mistaken for a function: `.pdata` declares nothing for either, so the
    # sweep happily decodes `.data` and hands back an "extent". See `executable_at` for the 85
    # rows that reached a detour verdict that way.
    if executable_at(blob, va) is False:
        return None
    extents, starts, spans = declared_functions(blob)
    rva = va - BASE
    if rva in extents:
        return extents[rva]
    enclosing = inside_declared_function(rva, spans)
    if enclosing is not None:
        return enclosing[1]
    return verify_rules().leaf_extent(blob, va, starts, limit=limit)


def body_slice_end(blob, va, cap=None):
    """`body_end` expressed as an absolute file offset for slicing, capped.

    Returns the offset one past the function's last byte, or `None` when the extent is unknown.
    `cap`, when given, is an upper bound in bytes from `va` applied on top of the extent -- a cap
    is legitimate, a cap standing in for the extent is the bug.
    """
    end = body_end(blob, va)
    if end is None:
        return None
    if cap is not None:
        return min(end, (va - BASE) + cap)
    return end
