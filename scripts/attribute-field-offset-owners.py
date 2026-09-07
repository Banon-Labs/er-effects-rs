#!/usr/bin/env python3
"""Keep the field offsets nobody can attribute visible, and shrink that set on purpose.

The problem this is the residual of
-----------------------------------
A struct field offset is the only completely silent failure class in the 1.16.2 -> 1.17
migration. A stale detour target is refused by `er-hook` and logged. An unmapped data RVA
resolves to 0 and the caller says so. A moved field returns a plausible number of the right width,
forever, with no fault and no log line.

Two gates already measure that class, and both bottom out in the same place:

  * `scripts/check-object-field-offsets-1170.py` re-measures 31 frozen witness rows across six
    objects: `CS::PlayerGameData` and `CS::PlayerIns`, the two the migration actually took apart;
    `FD4::FD4PadDevice` / `FD4::FD4PadManager`, where the question was which object a write lands
    in; and `CS::CSSystemStep`, where the question was whether an offset had ever been a field
    (`CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` said 0x40 against a field at 0x48, in both builds).
  * `scripts/check-singleton-field-offsets.py` clears any offset whose owner is one of the seven
    singleton-rooted classes it can reach through a global.

Everything else is UNATTRIBUTED, and an offset whose owning object cannot be named cannot be
measured at all: joining a repo constant to a drift row on the number is worthless in both
directions, because `0x50`, `0x88`, `0x90` and `0xb0c` are field offsets in dozens of unrelated
structures. `0xb0c` moved in 1.17 -- in `MoWwiseManImp`, the Wwise audio manager -- while
`DIALOG_SLOT_CURSOR_B0C_OFFSET` is a title dialog at the same number and is unaffected.

So the unattributed set is not a to-do list that can be closed by measuring harder. It is closed
one constant at a time, by reading the reverse engineering recorded beside each one and putting
its owner in the shared table. This file's job is to make sure that set is known, that it can
only shrink, and that a new unattributed constant has to be justified rather than merely added.

What counts as attributed
-------------------------
Exactly what the two existing gates already consult, imported rather than copied:

  1. an `offset_of!(T, field)` whose `T` this workspace declares, or one of the unambiguous name
     prefixes -- both from `scripts/detect-struct-field-drift.py` (`inventory()` / `struct_for`);
  2. an entry in `scripts/adjudicate-autoload-offsets.py::OWNERS`, the repo's one hand-read
     constant -> class table. A `None` there is an attribution too: it records "this is a Windows
     / MSVC / PE ABI structure a FromSoftware patch cannot move", with the reason beside it;
  3. an entry in that same file's `NAMED_WITNESS`, for an object with no vtable of its own --
     a stack buffer, a leaf, a singleton reached through a global. The owner there is a named
     consumer function plus the register that provably holds the object inside it, which is
     weaker than RTTI and is reported as cleared-by-named-witness rather than cleared, but it is
     still an owner and the offset is still measurable.

Naming an owner is not a clearance. `CS::CSMenuProfModelRend + 0x756` is attributed and still
returns still-unknown from `scripts/clear-fields-by-object.py`. Attribution is what makes the
offset measurable; the measurement is a separate step and is reported separately.

What this file asserts
----------------------
  Ratchet (always runs, reads only repo source).  Every unattributed constant is listed in
      `docs/recon/unattributed-field-offsets.txt`. A constant that is unattributed today and is
      not in that file fails the gate. Rows may disappear; they may not appear.

  Blindness floors.  Nine "audits" in this repo have reported zero findings from a matcher that
      had gone blind, because `assert bad == 0` passes over an empty set. A shrinking unattributed
      list is exactly what a blinded inventory or a lost owners import produces, and it would read
      as progress. So the population size and the attributed count carry floors: if either falls
      through the floor the gate goes red and says the matcher went blind, not that the set got
      smaller.

  Witnesses (runs when the two de-Arxan'd images are present, SKIPs loudly when they are not).
      Frozen function-pair alignments re-measured from the images on every run, in the shape
      `scripts/check-object-field-offsets-1170.py` established: a row that cannot be measured is a
      failure, not a pass.

Usage
    scripts/attribute-field-offset-owners.py                  # the gate
    scripts/attribute-field-offset-owners.py --refresh        # rewrite the ratchet doc
    scripts/attribute-field-offset-owners.py --list           # the unattributed set, by crate
    scripts/attribute-field-offset-owners.py --prose name     # the RE prose that names its owner
    scripts/attribute-field-offset-owners.py --triage TSV [--verdicts V] [--written]
                                                              # bulk prose digest for a triage run
    scripts/attribute-field-offset-owners.py --selftest
"""
from __future__ import annotations

import argparse
import collections
import csv
import importlib.util
import io
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RATCHET = REPO / "docs" / "recon" / "unattributed-field-offsets.txt"
IMAGE_1162 = REPO / "eldenring-deobf.bin"
IMAGE_1170 = REPO / "eldenring-deobf-1.17.bin"
MATCHER = REPO / "scripts" / "pair-object-field-drift.py"
IMAGE_BASE = 0x140000000

# ---------------------------------------------------------------------------------------------
# Blindness floors.
#
# Measured 2026-08-31: 813 included game-struct-field offset sites, 370 of them attributed. Both
# numbers move whenever repo source lands, so they are floors with HEADROOM rather than exact
# pins -- a ratchet that goes red because somebody deleted a constant is a ratchet people route
# around (the same reasoning as MIN_CLEARED_CONSTANTS in check-singleton-field-offsets.py).
#
# They exist for one failure only, and it is the one that matters here: the unattributed list
# shrinking because the inventory went blind or the owners import silently returned nothing.
# Either would look like progress. Neither can get past these.
# ---------------------------------------------------------------------------------------------
MIN_INCLUDED_SITES = 700
MIN_ATTRIBUTED = 320

# ---------------------------------------------------------------------------------------------
# Frozen witnesses.
#
# Each row: (object, label, offset_1162, offset_1170, witness, how). The witness is a pair of
# function bodies that align instruction-for-instruction across the two images, so instruction k
# on each side is the same access to the same field and a displacement difference is that field
# moving, by exactly that much. `bases` restricts what is counted to registers that provably hold
# the object -- never left empty. An empty base filter means "count every register base", which is
# how foreign objects leak in as false held witnesses; it produced spurious "held" readings at
# PlayerIns 0x480/0x508/0x530, all inside a band that had demonstrably moved.
# ---------------------------------------------------------------------------------------------

# `CS::CSMenuProfModelRend::CSMenuProfModelRend`. `mov %rcx,%r14` in the prologue and then
# `lea 0x142b80128(%rip),%rax ; mov %rax,(%r14)` -- it stores this class'S own VTABLE at
# `[r14+0]`, so r14 is `this` and the object identity does not rest on the function map at all.
# The 1.17 body stores 0x142b831d8 at the corresponding instruction, which is exactly the 1.17
# vtable `scripts/rtti-classmap-both.py` pairs to the same mangled class name. 64/64 instructions
# align with zero inserts, deletes or replacements.
PROF_MODEL_REND_CTOR = dict(
    va16=0x140BBDF20, len16=0x111, va17=0x140BBF5F0, len17=0x111, bases=("r14",)
)
# The orbit -> view-matrix builder the camera prose names (`PROFILE_CAM_BUILD_MATRIX_RVA`), a pure
# math leaf: `fn(renderer /rcx/, out /rdx/)`. 295/295 instructions align, again with no structural
# difference, and `rcx` is the renderer by the x64 calling convention.
PROF_MODEL_REND_BUILD_MATRIX = dict(
    va16=0x140BBE390, len16=0x4C2, va17=0x140BBFA60, len17=0x4C2, bases=("rcx",)
)
# The FD4PadDevice per-frame poll. Its interesting property here is the BASE: two instructions
# above the read it does `mov rax,[rip -> 0x14485dc18]`, the DLUID input-device-manager singleton
# whose 1.17 counterpart 0x4861d28 carries 84/84 agreeing references in
# docs/recon/rva-map-1162-to-1170.data.tsv. So `rax` holds that singleton by construction, not by
# inference, and 0x88d is a field of whatever object lives in it.
DLUID_INPUT_GATE = dict(
    va16=0x141F6BAD0, len16=0x86, va17=0x141F6D8D0, len17=0x86, bases=("rax",)
)

WITNESSES = (
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_TARGET (0x9b4, movups of the whole Vec4)",
        0x9B4,
        0x9B4,
        PROF_MODEL_REND_CTOR,
        "constructor stores its own vtable at [r14+0]",
    ),
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_DISTANCE (0x9c4; the qword store also covers YAW at 0x9c8)",
        0x9C4,
        0x9C4,
        PROF_MODEL_REND_CTOR,
        "constructor",
    ),
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_PITCH (0x9cc)",
        0x9CC,
        0x9CC,
        PROF_MODEL_REND_CTOR,
        "constructor",
    ),
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_PERSCAM (0x9d0, the embedded CSPersCam sub-object)",
        0x9D0,
        0x9D0,
        PROF_MODEL_REND_CTOR,
        "constructor; `lea 0x9d0(%r14),%rcx` into the CSPersCam constructor",
    ),
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_ASPECT (0xa24)",
        0xA24,
        0xA24,
        PROF_MODEL_REND_CTOR,
        "constructor",
    ),
    (
        "CS::CSMenuProfModelRend",
        "renderer flag byte at 0x971 (bounds the camera block from below)",
        0x971,
        0x971,
        PROF_MODEL_REND_CTOR,
        "constructor",
    ),
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_TARGET, second witness through a different function",
        0x9B4,
        0x9B4,
        PROF_MODEL_REND_BUILD_MATRIX,
        "orbit -> view-matrix builder, renderer in rcx",
    ),
    (
        "CS::CSMenuProfModelRend",
        "PROFILE_CAM_YAW (0x9c8), read directly by the matrix builder",
        0x9C8,
        0x9C8,
        PROF_MODEL_REND_BUILD_MATRIX,
        "orbit -> view-matrix builder",
    ),
    (
        "DLUID input-device manager singleton",
        "DLUID_INPUT_ACTIVE_FLAG (0x88d)",
        0x88D,
        0x88D,
        DLUID_INPUT_GATE,
        "rax loaded from DLUID_SINGLETON_RVA two instructions above the read",
    ),
    (
        "DLUID input-device manager singleton",
        "the neighbouring gate byte at 0x88e",
        0x88E,
        0x88E,
        DLUID_INPUT_GATE,
        "same function, same base",
    ),
)

# The offsets those witnesses are the evidence for, so a constant renamed out from under a witness
# is visible rather than merely absent. `constant -> (owner, offset)`.
WITNESSED_CONSTANTS = {
    "PROFILE_CAM_TARGET_OFFSET": ("CS::CSMenuProfModelRend", 0x9B4),
    "PROFILE_CAM_DISTANCE_OFFSET": ("CS::CSMenuProfModelRend", 0x9C4),
    "PROFILE_CAM_YAW_OFFSET": ("CS::CSMenuProfModelRend", 0x9C8),
    "PROFILE_CAM_PITCH_OFFSET": ("CS::CSMenuProfModelRend", 0x9CC),
    "PROFILE_CAM_PERSCAM_OFFSET": ("CS::CSMenuProfModelRend", 0x9D0),
    "PROFILE_CAM_ASPECT_OFFSET": ("CS::CSMenuProfModelRend", 0xA24),
    "DLUID_INPUT_ACTIVE_FLAG_OFFSET": ("DLUID input-device manager singleton", 0x88D),
}


# ---------------------------------------------------------------------------------------------
# imports of the two existing tables, never copies of them
# ---------------------------------------------------------------------------------------------
def _load(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


_CACHE: dict[str, object] = {}


def inventory_module():
    if "inv" not in _CACHE:
        _CACHE["inv"] = _load(
            "_afoo_detect_struct_field_drift", REPO / "scripts" / "detect-struct-field-drift.py"
        )
    return _CACHE["inv"]


def owners() -> dict:
    """The two curated owner tables in `scripts/adjudicate-autoload-offsets.py`, merged.

    `OWNERS` maps a constant to an RTTI class (or `None` for an ABI structure); `NAMED_WITNESS`
    maps a constant whose owner has no vtable of its own to a consumer function and the register
    that holds the object there. Both are attributions, so both count here.

    Imported rather than copied -- a copied ownership table is a second claim about the same fact
    that drifts silently. A failure to import is a hard error, not a shrug: a gate that quietly
    loses its ownership table reports more unattributed constants, which this ratchet would then
    refuse -- but it would refuse them with a nonsense reason, so say the real one.
    """
    if "own" not in _CACHE:
        module = _load("_afoo_adjudicate", REPO / "scripts" / "adjudicate-autoload-offsets.py")
        merged = dict(module.OWNERS)
        for name, (function, base) in module.NAMED_WITNESS.items():
            merged.setdefault(name, f"@named-witness {function}[{base}]")
        _CACHE["own"] = merged
    return _CACHE["own"]


def classify(rows=None, owner_table=None):
    """(attributed, unattributed) over every included game-struct-field offset site."""
    rows = inventory_module().inventory() if rows is None else rows
    table = owners() if owner_table is None else owner_table
    attributed, unattributed = [], []
    for row in rows:
        if not row["included"]:
            continue
        if row["struct"]:
            attributed.append((row, "offset_of/prefix", row["struct"]))
        elif row["name"] in table:
            value = table[row["name"]]
            if value is None:
                attributed.append((row, "curated", "(not a game structure)"))
            elif isinstance(value, tuple):
                attributed.append((row, "curated", " | ".join(value)))
            elif isinstance(value, str) and value.startswith("@named-witness"):
                attributed.append((row, "named-witness", value))
            else:
                attributed.append((row, "curated", value))
        else:
            unattributed.append(row)
    return attributed, unattributed


def ratchet_key(row) -> str:
    value = "" if row["resolved"] is None else f"{row['resolved']:#x}"
    return f"{row['name']}\t{row['file']}\t{value}"


# ---------------------------------------------------------------------------------------------
# the ratchet document
# ---------------------------------------------------------------------------------------------
HEADER = """\
# ELDEN RING field-offset constants whose OWNING OBJECT nobody has established.
#
# Generated by scripts/attribute-field-offset-owners.py --refresh. One line per
# (constant, file, value), so moving a constant within its file does not churn the list.
# THIS SET MAY SHRINK. GROWTH IS REFUSED.
#
# WHY IT MATTERS. A stale detour target is refused by er-hook and logged; an unmapped data RVA
# resolves to 0 and the caller reports it; a stale STRUCT FIELD OFFSET reads the neighbouring
# field and says nothing, forever. An offset whose owner cannot be named cannot be measured
# either, because joining a repo constant to a 1.17 drift row on the NUMBER proves nothing in
# either direction -- 0xb0c moved in 1.17, in the Wwise audio manager, while
# DIALOG_SLOT_CURSOR_B0C_OFFSET is a title dialog at the same number and is unaffected.
#
# CLEAR A ROW by reading the reverse engineering recorded in the doc comment above the constant
# (`--prose NAME` prints it) and adding `"NAME": "CS::SomeClass"` to
# scripts/adjudicate-autoload-offsets.py::OWNERS -- the one table this gate,
# check-singleton-field-offsets.py and adjudicate-autoload-offsets.py all import. Use `None`
# there, with a reason in NON_GAME_STRUCT_REASONS, for a Windows / MSVC / PE ABI structure a
# FromSoftware patch cannot move. Then measure it:
#     python3 scripts/clear-fields-by-object.py --class CS::SomeClass --offsets 0xNN
# and re-run --refresh.
#
# NAMING THE OWNER IS NOT A CLEARANCE. It is what makes the offset measurable. Several attributed
# offsets still return STILL-UNKNOWN, and that is a truthful answer, not a failure.
#
# This list is NOT the same population as check-singleton-field-offsets.py's
# `UNATTRIBUTED-NO-OWNER`, which counts every *_OFFSET symbol in the rva_symbols index including
# save-file and host-only-crate offsets. This one counts only sites that tool classifies as live
# GAME STRUCT FIELDS.
#
# HOW MUCH OF THIS IS ACTUALLY DANGEROUS, measured 2026-08-31 by joining these rows against
# `scripts/detect-struct-field-drift.py --resolve-unknown` (an exhaustive displacement census over
# the mapped half of the image: 128602 function pairs). That census is structure-INDEPENDENT --
# "this displacement changed in 0 of the N otherwise-identical pairs that use it" is a statement
# about every object the scan can see, ours included -- so it settles most of the list without
# ever naming an owner:
#
#     NOT-MOVED-ANYWHERE          257   (11 of them written through)
#     MOVED-ELSEWHERE              88   (moved only inside objects 1.17 is known to have grown,
#                                        chiefly the Wwise audio block, which alone "moves"
#                                        0x50/0x88/0x90/0xb8/0xd4/0xe0 -- the repo's commonest
#                                        small offsets -- via ONE settings struct shifted +0x38)
#     MOVED-SOMEWHERE              18   (1 written; the number moved in an object nobody has
#                                        named, so these are the ones that need an owner most)
#     OFFSET-ZERO-UNMEASURABLE     12   (a displacement of 0 carries no byte to compare; a new
#                                        LEADING member or a new base class would move it and this
#                                        method could not see it. ONE ROUTE EXISTS for a polymorphic
#                                        owner: `[this+0]` is the vtable slot, so pairing the class's
#                                        vtable slot-for-slot and confirming the constructor's
#                                        `mov [this+0],<vtable>` is aligned in both bodies proves
#                                        nothing was inserted in front of it. FD4PadDevice was
#                                        settled that way on 2026-08-31 -- 0x143295998 -> 0x143298c58,
#                                        5 slots, 3 identical and 2 at the region's own +0x2810.)
#     value not resolvable         37   (the constant is an expression this inventory cannot fold,
#                                        so it is not even in the census)
#
# Read that as: the list is long, the live hazard in it is not. But NOT-MOVED-ANYWHERE is not a
# clearance either -- the census is blind to `.pdata`-less leaf accessors, which is exactly where
# the one PlayerGameData move this migration found actually lives. It is evidence, ranked; the
# owner is what makes it measurable.
#
# THE TOP OF THAT RANKING WAS SETTLED ON 2026-08-31, AND THE OWNER WAS THE WRONG CLASS.
# The only row that was BOTH written through AND MOVED-SOMEWHERE was `VK_ARRAY_88_OFFSET` = 0x88
# (er-input-harness/src/pad_inject.rs). It was attributed to `CS::CSInGamePad`, which yields 2
# usable paired method bodies out of 40 -- and that starvation is why it would not settle. It is
# not that class. Its writer, 1.16.2 0x1426634a0 (`mov byte [rcx+rdx*2+0x88],1`, bound
# `cmp eax,0x50` on id-1000), has exactly four call sites (0x140240e70, 0x140241130, 0x140e321b0,
# 0x140e32470) and EVERY one loads `rcx` from `*(manager + 0x18 + dev*8)` =
# `FD4PadManager::padDevices[dev]`; `FD4PadManager::Init` fills that array with `HeapAlloc(0x3c0)`
# + `FD4PadDevice::FD4PadDevice` + `FD4PadDevice::vftable`. So the owner is `FD4::FD4PadDevice`,
# and the CSInGamePad merely HOLDS the device at its own +0x10.
#
# 0x88 HELD on 1.17. The writer pairs to 0x142665cb0 -- a masked signature that wildcards BOTH the
# displacement and the bound is unique in each image, and the call graph agrees independently (0
# callees, the same four callers, two of them at identical addresses) -- and the two bodies are
# byte-identical, so 0x88 is MEASURED on 1.17 rather than carried. Second witness: the
# `FD4PadDevice` constructor 0x142663880 -> 0x142666090 aligns 168/168 with zero moved offsets,
# holding 0x80 immediately below the array, and the allocation size is still 0x3c0.
#
# The prose that stood here before said WRITER_RVA "lands MID-FUNCTION inside a different 1.17
# function, so that region genuinely changed". The region did move -- uniformly, by +0x2810, which
# is why the naive same-address carry lands mid-function -- but nothing in it changed: all ten
# `disp == 0x88` sites in 0x142660000..0x14266a000 reappear at exactly +0x2810 with identical
# operands, and the class vtable pairs slot for slot (0x143295998 -> 0x143298c58).
#
# `.pdata` blindness was the real obstacle and call-graph topology is what got past it. The two
# padMaps offsets beside it were already settled: builder 0x140240e70 is byte-identical between
# the builds (195/195 aligned at the same address) and holds 0x18, 0x40 and 0x48 on the manager.
#
# NOT WIRED INTO scripts/check.sh. Two other agents held that file when this landed, so the gate
# runs only when invoked. Adding it is one line there, in column 1, like every other step:
#     python3 "$repo_root/scripts/attribute-field-offset-owners.py" --selftest
#     python3 "$repo_root/scripts/attribute-field-offset-owners.py"
"""


def render(unattributed) -> str:
    out = io.StringIO()
    out.write(HEADER)
    by_crate = collections.Counter(row["crate"] for row in unattributed)
    out.write(f"#\n# Currently {len(unattributed)} site(s) in {len(by_crate)} crate(s):\n")
    for crate, count in by_crate.most_common():
        out.write(f"#   {count:>4}  {crate}\n")
    out.write("#\n# constant\tfile\tvalue\n")
    for key in sorted(ratchet_key(row) for row in unattributed):
        out.write(key + "\n")
    return out.getvalue()


def load_ratchet() -> set[str] | None:
    if not RATCHET.is_file():
        return None
    return {
        line
        for line in RATCHET.read_text(encoding="utf-8").splitlines()
        if line and not line.startswith("#")
    }


# ---------------------------------------------------------------------------------------------
# the image half
# ---------------------------------------------------------------------------------------------
_MATCHER: list = []


def load_matcher(fresh=False):
    if _MATCHER and not fresh:
        return _MATCHER[0]
    spec = importlib.util.spec_from_file_location("_afoo_pair_object_field_drift", MATCHER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if not fresh:
        _MATCHER.append(module)
    return module


def images_present() -> bool:
    return IMAGE_1162.is_file() and IMAGE_1170.is_file()


def image_findings(matcher, capstone, md, rows=WITNESSES):
    findings, measured = [], 0
    for obj, label, old, new, witness, how in rows:
        pairs, _ins, _del, _rep = matcher.compare(
            capstone,
            md,
            witness["va16"] - IMAGE_BASE,
            witness["len16"],
            witness["va17"] - IMAGE_BASE,
            witness["len17"],
            witness["bases"],
            label,
            quiet=True,
        )
        seen = {o: n for o, n, _a, _b, _t in pairs}
        if old not in seen:
            findings.append(
                f"{obj} :: {label}: the witness ({how}) no longer reads {old:#x} through "
                f"{'/'.join(witness['bases'])} at all -- the measurement went blind, which is "
                "not the same as a clean result"
            )
            continue
        measured += 1
        if seen[old] != new:
            findings.append(
                f"{obj} :: {label}: measured {old:#x} -> {seen[old]:#x}, but this gate is frozen "
                f"at {old:#x} -> {new:#x}. Witness: {how}"
            )
    return findings, measured


CONST_DEF = r"const\s+{name}\s*(?::\s*[A-Za-z0-9_:<>]+\s*)?=\s*(0x[0-9a-fA-F_]+)\s*[;,]"


def witnessed_constant_findings(unattributed_by_name, attributed_by_name, read_text=None):
    """Each witnessed constant must still exist, still hold its measured literal, and be owned."""
    read_text = read_text or (lambda path: path.read_text(encoding="utf-8", errors="replace"))
    texts = {}
    for path in REPO.joinpath("crates").rglob("*.rs"):
        texts[path] = read_text(path)
    findings = []
    for name, (obj, value) in sorted(WITNESSED_CONSTANTS.items()):
        pattern = re.compile(CONST_DEF.format(name=re.escape(name)))
        seen = []
        for path, text in texts.items():
            for match in pattern.finditer(text):
                seen.append((path, int(match.group(1).replace("_", ""), 16)))
        if not seen:
            findings.append(
                f"{name}: this gate measures {obj} + {value:#x} from the images on every run, but "
                "the constant it is evidence for is not defined anywhere. Either it was renamed "
                "(update WITNESSED_CONSTANTS) or the witness now proves nothing"
            )
            continue
        for path, found in seen:
            if found != value:
                findings.append(
                    f"{name} = {found:#x} at {path.relative_to(REPO)}, but the images witness "
                    f"{obj} + {value:#x} unchanged in 1.17"
                )
        # Any unattributed site is a finding, not "no site at all". The same constant name is
        # frequently redeclared per crate: `DLUID_INPUT_ACTIVE_FLAG_OFFSET` exists four times, and
        # one of them resolves through an `offset_of!` layout while the other three are bare
        # literals. Asking "is the NAME attributed anywhere" would have reported that as owned.
        if name in unattributed_by_name:
            findings.append(
                f"{name}: measured on {obj} + {value:#x} by this gate, yet at least one of its "
                "definition sites is UNATTRIBUTED. Name its owner in "
                "scripts/adjudicate-autoload-offsets.py (OWNERS, or NAMED_WITNESS when the object "
                "has no vtable of its own)"
            )
    return findings


# ---------------------------------------------------------------------------------------------
# the gate
# ---------------------------------------------------------------------------------------------
def run(out=sys.stdout, rows=None, owner_table=None, read_text=None, ratchet=None):
    attributed, unattributed = classify(rows=rows, owner_table=owner_table)
    total = len(attributed) + len(unattributed)
    findings = []

    print(
        f"included game-struct-field offset sites: {total}\n"
        f"  attributed   {len(attributed)}\n"
        f"  UNATTRIBUTED {len(unattributed)}   "
        f"({len({r['name'] for r in unattributed})} distinct constants)",
        file=out,
    )
    if total < MIN_INCLUDED_SITES:
        findings.append(
            f"only {total} offset sites found, floor is {MIN_INCLUDED_SITES}. The inventory went "
            "blind; a shrinking unattributed list from a blind scan reads as progress and is not"
        )
    if len(attributed) < MIN_ATTRIBUTED:
        findings.append(
            f"only {len(attributed)} attributed, floor is {MIN_ATTRIBUTED}. The OWNERS import "
            "lost entries, or the offset_of!/prefix resolver stopped resolving"
        )

    baseline = load_ratchet() if ratchet is None else ratchet
    if baseline is None:
        findings.append(f"{RATCHET.relative_to(REPO)} is missing; run --refresh")
    else:
        current = {ratchet_key(row) for row in unattributed}
        added = sorted(current - baseline)
        removed = sorted(baseline - current)
        if removed:
            print(f"  {len(removed)} row(s) attributed since the last refresh:", file=out)
            for line in removed[:20]:
                print("    - " + line.replace("\t", "  "), file=out)
            print("  re-run --refresh to bank that", file=out)
        for line in added:
            findings.append(
                "NEW unattributed field offset, and the ratchet refuses growth: "
                + line.replace("\t", "  ")
                + "  -- name its owning object in scripts/adjudicate-autoload-offsets.py::OWNERS"
            )

    by_name_un = {row["name"] for row in unattributed}
    by_name_at = {row["name"] for row, _how, _owner in attributed}
    findings += witnessed_constant_findings(by_name_un, by_name_at, read_text=read_text)

    if images_present():
        matcher = load_matcher()
        capstone, md = matcher._capstone()
        image, measured = image_findings(matcher, capstone, md)
        findings += image
        print(f"  witnesses re-measured from the two images: {measured}/{len(WITNESSES)}", file=out)
    else:
        print(
            "  SKIP: eldenring-deobf.bin / eldenring-deobf-1.17.bin absent (they are game-derived "
            "and gitignored), so the frozen witnesses were NOT re-measured. This run did not "
            "check the images.",
            file=out,
        )

    for finding in findings:
        print("FAIL: " + finding, file=out)
    return 1 if findings else 0


def refresh() -> int:
    _attributed, unattributed = classify()
    RATCHET.parent.mkdir(parents=True, exist_ok=True)
    RATCHET.write_text(render(unattributed), encoding="utf-8")
    print(f"wrote {RATCHET.relative_to(REPO)}: {len(unattributed)} unattributed site(s)")
    return 0


# ---------------------------------------------------------------------------------------------
# prose helpers -- the way a row actually gets cleared
# ---------------------------------------------------------------------------------------------
_COMMENT = re.compile(r"^\s*(///|//!|//|#\[)")


def comment_block_above(lines: list[str], index: int) -> list[str]:
    out: list[str] = []
    i = index - 1
    while i >= 0 and _COMMENT.match(lines[i]):
        out.append(lines[i].rstrip())
        i -= 1
    return list(reversed(out))


def section_prose(lines: list[str], index: int, limit: int = 60) -> list[str]:
    """The nearest banner comment above the constant, when its own block says nothing.

    Reverse-engineered offsets here are usually recorded once per block ("All offsets are byte
    offsets from the renderer (CSMenuProfModelRend) base."), with each constant carrying only its
    own field's meaning. Printed with a different marker so the two are never confused.
    """
    out: list[str] = []
    i = index - 1
    seen_code = False
    while i >= 0 and index - i < limit:
        if _COMMENT.match(lines[i]):
            if seen_code:
                out.append(lines[i].rstrip())
        elif lines[i].strip():
            if out:
                break
            seen_code = True
        i -= 1
    return list(reversed(out))


def emit_prose(constant: str, value: str, note: str, site: str, tail: int, out=sys.stdout) -> None:
    path, _, line_no = site.rpartition(":")
    try:
        lines = (REPO / path).read_text(encoding="utf-8", errors="replace").split("\n")
    except OSError as exc:
        print(f"### {constant}  -- unreadable site {site}: {exc}\n", file=out)
        return
    index = int(line_no) - 1 if line_no.isdigit() else 0
    own = comment_block_above(lines, index)
    print(f"### {constant} = {value}   [{note}]", file=out)
    print(f"    {site}", file=out)
    for text in own[-tail:]:
        print("    | " + text.strip()[:200], file=out)
    if not own:
        for text in section_prose(lines, index)[-tail:]:
            print("    ~ " + text.strip()[:200], file=out)
    print(file=out)


def prose_for(names: set[str], tail: int) -> int:
    _attributed, unattributed = classify()
    rows = [r for r in inventory_module().inventory() if r["included"]]
    unattributed_names = {r["name"] for r in unattributed}
    shown = 0
    for row in rows:
        if row["name"] not in names:
            continue
        state = "UNATTRIBUTED" if row["name"] in unattributed_names else "attributed"
        value = "?" if row["resolved"] is None else f"{row['resolved']:#x}"
        emit_prose(row["name"], value, state, f"{row['file']}:{row['line']}", tail)
        shown += 1
    if not shown:
        print("no such included offset constant")
        return 2
    return 0


def digest(triage: Path, verdicts, written, max_held, tail) -> int:
    rows = list(csv.DictReader(triage.open(encoding="utf-8"), delimiter="\t"))
    keep = set(verdicts.split(",")) if verdicts else None
    seen, shown = set(), 0
    for row in rows:
        if keep and row["verdict"] not in keep:
            continue
        if written and row["written"] != "W":
            continue
        if max_held and int(row["held"]) > max_held:
            continue
        key = (row["constant"], row["site"])
        if key in seen:
            continue
        seen.add(key)
        note = (
            f"{row['verdict']} held={row['held']} moves={row['moves']} "
            f"uses={row['uses']} {row['written']}{row['autoload']}"
        )
        emit_prose(row["constant"], row["offset"], note, row["site"], tail)
        shown += 1
    print(f"-- {shown} constant(s)")
    return 0


def listing() -> int:
    _attributed, unattributed = classify()
    by_crate: dict[str, list] = collections.defaultdict(list)
    for row in unattributed:
        by_crate[row["crate"]].append(row)
    for crate in sorted(by_crate, key=lambda c: -len(by_crate[c])):
        print(f"\n{crate}  ({len(by_crate[crate])})")
        for row in sorted(by_crate[crate], key=lambda r: (r["file"], r["name"])):
            value = "?" if row["resolved"] is None else f"{row['resolved']:#x}"
            print(f"   {row['name']:<58} {value:>9}  {row['file']}:{row['line']}")
    return 0


# ---------------------------------------------------------------------------------------------
# non-vacuity
# ---------------------------------------------------------------------------------------------
def _red(label, **kwargs) -> tuple[bool, str]:
    buf = io.StringIO()
    rc = run(out=buf, **kwargs)
    return rc != 0, f"{label}: {'red' if rc else 'GREEN'}"


def selftest() -> int:
    ok = True

    def check(condition, message):
        nonlocal ok
        if not condition:
            ok = False
            print(f"FAIL: {message}")

    attributed, unattributed = classify()
    real_rows = inventory_module().inventory()
    baseline = {ratchet_key(row) for row in unattributed}

    # 0. The gate is green on the tree as it stands. Everything below asserts it can go red, which
    #    means nothing unless it is green now.
    buf = io.StringIO()
    check(run(out=buf, ratchet=baseline) == 0, f"gate is not green as it stands:\n{buf.getvalue()}")

    # 1. Growth is refused. Drop one row from the baseline and the gate must name it.
    if baseline:
        victim = sorted(baseline)[0]
        red, note = _red("growth", ratchet=baseline - {victim})
        check(red, f"removing {victim!r} from the baseline did not go red ({note})")

    # 2. A lost owner table. If owners silently returns nothing, more constants become
    #    unattributed -- growth -- And the attributed floor is breached. Both must fire.
    red, note = _red("empty OWNERS", owner_table={}, ratchet=baseline)
    check(red, f"an empty OWNERS table did not go red ({note})")

    # 3. A blind inventory. An empty scan makes the unattributed list shrink, which is exactly
    #    what progress looks like. The population floor is the only thing standing in the way.
    red, note = _red("blind inventory", rows=[], ratchet=baseline)
    check(red, f"an empty inventory did not go red ({note})")

    # 4. A changed constant. If a witnessed constant's literal is edited on disk, say so.
    target = "PROFILE_CAM_YAW_OFFSET"
    def mutate(path):
        text = path.read_text(encoding="utf-8", errors="replace")
        return text.replace(f"{target}: usize = 0x9c8", f"{target}: usize = 0x9d0")
    red, note = _red("edited constant", read_text=mutate, ratchet=baseline)
    check(red, f"editing {target} to a wrong literal did not go red ({note})")

    # 5. The image half, and the frozen negatives that go with it. Every witness row above is a
    #    held row, so perturbing its 1.17 value to old+8 is precisely the mutant a matcher that
    #    blanket-reported "+8 above the insertion" would be. Each must go red on its own.
    if images_present():
        matcher = load_matcher()
        capstone, md = matcher._capstone()
        findings, measured = image_findings(matcher, capstone, md)
        check(not findings, f"the frozen witnesses do not reproduce: {findings}")
        check(
            measured == len(WITNESSES),
            f"only {measured} of {len(WITNESSES)} witness rows could be measured; an unmeasurable "
            "row is a failure, not a pass",
        )
        for index, row in enumerate(WITNESSES):
            obj, label, old, new, witness, how = row
            mutant = list(WITNESSES)
            mutant[index] = (obj, label, old, new + 8, witness, how)
            bad, _m = image_findings(matcher, capstone, md, rows=tuple(mutant))
            check(bad, f"frozen negative: {obj}::{label} frozen at old+8 was not caught")
            mutant[index] = (obj, label, old + 3, new, witness, how)
            bad, _m = image_findings(matcher, capstone, md, rows=tuple(mutant))
            check(bad, f"blind witness: {obj}::{label} at an offset nothing reads was not caught")

        # 6. A LOBOTOMISED MATCHER. `compare` returning nothing must be a failure, not a pass.
        class Blind:
            @staticmethod
            def compare(*_a, **_k):
                return [], [], [], []

        bad, _m = image_findings(Blind, capstone, md)
        check(len(bad) == len(WITNESSES), "a matcher that reports nothing did not fail every row")
    else:
        print("SKIP: images absent; the witness half of the selftest did not run")

    check(len(real_rows) >= MIN_INCLUDED_SITES, "the real inventory is below its own floor")
    check(len(attributed) >= MIN_ATTRIBUTED, "the real attributed count is below its own floor")

    if ok:
        print(
            f"ok: ratchet growth, empty OWNERS, blind inventory, an edited constant, "
            f"{len(WITNESSES)} frozen-negative (+8) perturbations, "
            f"{len(WITNESSES)} blind-offset perturbations and a lobotomised matcher all go red"
        )
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument("--refresh", action="store_true", help="rewrite the ratchet document")
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument("--list", action="store_true", help="the unattributed set, by crate")
    ap.add_argument("--prose", help="comma-separated constants: print the RE prose beside them")
    ap.add_argument("--triage", type=Path, help="unknown-struct-triage.tsv: bulk prose digest")
    ap.add_argument("--verdicts", help="--triage: comma-separated verdicts to keep")
    ap.add_argument("--written", action="store_true", help="--triage: only constants written to")
    ap.add_argument("--max-held", type=int, default=0, help="--triage: only rows with held <= N")
    ap.add_argument("--tail", type=int, default=14, help="comment lines to print")
    args = ap.parse_args()

    if args.selftest:
        return selftest()
    if args.refresh:
        return refresh()
    if args.list:
        return listing()
    if args.prose:
        return prose_for(set(args.prose.split(",")), args.tail)
    if args.triage:
        return digest(args.triage, args.verdicts, args.written, args.max_held, args.tail)
    return run()


if __name__ == "__main__":
    sys.exit(main())
