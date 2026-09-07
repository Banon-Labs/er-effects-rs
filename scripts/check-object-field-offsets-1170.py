#!/usr/bin/env python3
"""Make ELDEN RING 1.16.2 -> 1.17 STRUCT-field drift loud instead of silent.

The failure class this exists for
---------------------------------
The 1.17 migration has three ways to be wrong about an address and only two of them speak:

  * a stale detour target      -> `er-hook` refuses it and logs `HOOK REFUSED`;
  * an unmapped CALL/data RVA  -> the resolver returns 0 and the caller says so;
  * a stale STRUCT field offset -> `*(this + 0xNN)` quietly returns the neighbouring field.

The third has no refusal, no fault and no log line. It returns a plausible value of the right
width, forever. This gate is the missing alarm for it.

What it measures, and why this method
-------------------------------------
Not a displacement census. A census answers "which offsets does the image read off this object",
which cannot say which field lives at an offset -- every interior byte of a big nested member is
witnessed too -- and it cannot see a move at all when both the old and new offsets happen to be
read somewhere. It produces plausible-looking wrong answers and they feel like confirmation.

This gate instead ALIGNS one function'S two bodies (scripts/pair-object-field-drift.py). When the
instruction sequences agree except for memory displacements, the code did not change, so
instruction k in 1.16.2 and instruction k in 1.17 are the same access to the same field -- and a
displacement difference is that field moving, by exactly that much. Each row below names the
witness function pair that produced its number.

What was measured (2026-08-31), and the correction it carries
--------------------------------------------------------------
`CS::PlayerGameData` grew 8 bytes in net size (0xae8 -> 0xaf0), but its fields did not all move by
8, and the difference is exactly the kind that a mechanical "+8 above the insertion" fix gets
wrong. 1.17 inserted one four-byte slot at 0x960 (a new byte field plus padding, in front of what
was `damage_negation_physical`). The 0x118-byte stat sub-object that used to start at 0x960 now
starts at 0x964 and is otherwise byte-identical (its own constructor aligns with zero moved
offsets), so it ends at 0xa7c instead of 0xa78; the pointer member that follows needs 8-byte
alignment, so it lands at 0xa80 rather than 0xa7c. Hence:

    [0x000, 0x960)   held        e.g. equipment 0x2b0, face_data 0x760, is_main_player 0x8f0
    [0x960, 0xa78)   +4          e.g. resistance_gauges 0x9c8 -> 0x9cc
    [0xa78, 0xae8)   +8          e.g. scadutree override 0xab4 -> 0xabc

What the new field at 0x960 is was established independently, from the other end: 1.17 also adds
`CS::MoveMapStep::_UpdateHorseType` (commit "The insertion was benign", bd er-effects-rs-xci9),
which re-applies the mount after a map move and reads `PlayerGameData+0x960` to make that
idempotent. Two derivations that share no evidence -- a constructor alignment here, a new callee
read there -- land on the same byte.

`FD4::FD4PadDevice` did not move a byte, and settling it took correcting its owner first. The
census left exactly one offset that was both written through and unsettled -- `VK_ARRAY_88_OFFSET`
in the input harness -- attributed to `CS::CSInGamePad`, a class that yields 2 usable paired bodies
out of 40. It is not that class. All four call sites of the array's only writer (1.16.2
0x1426634a0, `mov byte [rcx+rdx*2+0x88],1`) compute `rcx` as `*(FD4PadManager + 0x18 + dev*8)` =
`padDevices[dev]`, and `FD4PadManager::Init` fills that array with `HeapAlloc(0x3c0)` +
`FD4PadDevice::FD4PadDevice` + `FD4PadDevice::vftable`. With the owner right, the measurement is
easy and 0x88 holds. The CSInGamePad is one indirection away: it holds the device at its own +0x10.

That correction was not cosmetic. The harness had been writing `0x88 + (id-1000)*2` onto the
CSInGamePad, which is `HeapAlloc(0x98)` = 152 bytes, so every id from 1008 up wrote past the end of
a live game allocation.

`CS::PlayerIns` did not grow at all: 8 bytes were inserted in (0x398, 0x3a8] and 8 bytes removed
in (0x560, 0x580], so the band between them shifts +8 while the object size is unchanged and both
ends hold. A "+8 above the insertion" rule applied here would have corrupted
`PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET` = 0x6b8, which is witnessed held twice.

The other half of the QUESTION: Which object
--------------------------------------------
Everything above is about where a field sits, and for a read that is the whole question. For a
write it is the smaller half. A wrong offset returns the neighbouring member; a wrong object
corrupts the heap, and until 2026-08-31 nothing in this tree checked the second thing.

The bug that proved it: `stamp_vk_direct` wrote `object + 0x88 + (id - 1000) * 2` for ids up to
1080 -- byte 0x128 -- into what it believed was a `CS::CSInGamePad`, which is `HeapAlloc(0x98)` =
152 bytes. The offset 0x88 was right and had not moved. The class was wrong. It never fired only
because the TypeID needles were `.data` RVAs with no 1.17 mapping, which is a reason it was never
observed, not a reason it was safe.

So `ALLOC_WITNESSES` pins each written-into object by the two facts that can refute a
misattribution: the literal the game's own allocator is called with, and an identity anchor in the
same decoded window -- the constructor that allocation is handed to, or the vtable it is stamped
with. `WRITE_REACH` then recomputes, from the repo's own live literals, the highest byte this
repo's writes can reach in that object, and fails if it is not inside. Raising a bound (a slot
count, a virtual-key ceiling, a field offset) now fails here instead of at the far end of a
`HeapAlloc`.

Two of the measured objects have almost no slack, which is why the reach is computed rather than
eyeballed: `CS::ProfileSummary` is `0x18 + 10 * 0x2a0 = 0x1a58` and the repo's record writes reach
its final byte exactly (slot 10 would overrun by a whole 0x2a0 record, and only a `slot <
PROFILE_SUMMARY_SLOT_COUNT` clamp stands in the way), and `CS::MoveMapStep` writes 0x4b8 of a
0x4c0 object -- seven bytes to spare.

What the gate asserts
---------------------
  1. Image half -- every frozen witness row re-measures to the same pair, live, from the two
     de-Arxan'd images. A row that cannot be measured is a failure, not a pass: nine "audits" in
     this repo have reported zero findings from a matcher that had gone blind.
  2. Source half -- each repo constant that names a field of these two objects still holds the
     1.16.2 literal this gate verified, at the file and line where it lives. A constant derived
     from `offset_of!`/`size_of!` carries no literal at its definition, so its `const _: () =
     assert!(name == 0x..)` counts as the pin -- otherwise the whole `CS::ProfileSummary` typed
     layout would sit unwatched behind an expression.
  3. Object half -- every allocation size above re-decodes to the same literal in both images and
     is still tied to its class by a constructor call or a vtable reference within the same
     window, and no repo write reaches past it. Unmeasurable is a failure here too: a size
     instruction that no longer decodes says nothing about whether a write fits.
  4. The latent one, which is the reason to have a gate rather than a report. 44 sites compute
     `offset_of!(PlayerGameData, ...)` against the sibling `fromsoftware-rs` binding, which is a
     1.16.2 model. Every field referenced today sits below 0x960, so nothing is wrong -- but the
     mechanism looks maximally trustworthy (the compiler computed it) and is one added field
     reference away from silently reading a neighbour. So: any `offset_of!(PlayerGameData, X)`
     whose field is not in the verified-and-below-the-boundary set fails the build.

Usage
    python3 scripts/check-object-field-offsets-1170.py
    python3 scripts/check-object-field-offsets-1170.py --selftest   # prove it can go red
    ER_DEOBF_1162=... ER_DEOBF_1170=... python3 scripts/check-object-field-offsets-1170.py

The image half skips when the two images are absent (they are gitignored game-derived binaries);
the source half always runs, and `--selftest` requires the images so a green selftest can never
mean "the image half never ran".

Wired into `scripts/check.sh` on 2026-08-31, beside its sibling `check-singleton-field-offsets.py`.
Until then it ran nowhere, which is why the paragraph below could happen at all: a gate that no
suite invokes catches nothing, however good its rows are. The plain run takes a few seconds; the
`--selftest` re-aligns every frozen row once per perturbation and takes minutes, so only the plain
run is in the suite and the selftest is left to `scripts/audit-selftest-vacuity.py` and to
deliberate operator runs, the same split `check.sh` uses for other slow selftests.

The other half of the question, added 2026-08-31: Was it ever right
--------------------------------------------------------------------
Every row above this date asks "did 1.17 move this field". `CS::CSSystemStep` asks the question
that comes first and had been skipped everywhere: is the declared offset a field of that object in
either build. `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` was 0x40 from its introduction; the object's
constructor writes 0x48 and never 0x40, and 0x40 holds a live `DLAllocator*`. So
`oracle_system_step_label` read a pointer's low half, missed its `0..=20` range test and printed
`"?"` with `oracle_system_step_state = -95247096` on every run, on 1.16.2 as well as 1.17 -- a
legal i32 out of a legal read, with no fault, no log line and no drift for a drift check to see.

That is a second failure mode, not a variant of the first, and the fix for it is the same
measurement pointed at a different question: the row freezes 0x48 -> 0x48 (held) and the source
half pins the repo constant to 0x48, so a constant that was never measured now cannot be
introduced -- pinning it requires producing a witness function whose two bodies agree on it.

The sweep for siblings of that bug (2026-08-31), and its clean negative
-----------------------------------------------------------------------
0x40 was not a typo. It was an ARGUMENT: the sibling `fromsoftware-rs` binding declares `unk48`
after `requested_state`, so `current_state` "must" be 0x40. That is a whole reasoning style, and
it leaves a recognisable trace in a comment -- a member name, an `unkNN`, a `#[repr(C)]` walk, an
`offset_of!`, "matches the layout in ...". So the tree was swept for that trace rather than for
wrong numbers, since a wrong number cannot be recognised by reading source.

Six constructors and 15 constants later: Every name-derived offset in this tree measures correct.
That is a real result and the rows below are what makes it a durable one, because "right" and
"measured" were two different states and only the second survives an edit. The three things the
sweep also settled, each of which had been a way to be wrong:

  * Absence from a displacement set is not absence of a field. `GameMan+0xbc9` never appears as a
    displacement in `GameMan::GameMan` -- the ctor initialises it with a DWORD store at 0xbc8 --
    yet it is a real `bool` with its own byte-width getter, `IsServerConnectionEnabled`. Only a
    byte-coverage question (does anything wider cover this byte) separates the two cases, which is
    what `scripts/audit-name-derived-offsets.py --cover` exists for. What made 0x40's absence
    meaningful was that nothing covered it, in a constructor that accounts for its whole object.
  * A CONSTRUCTOR is only a whole-object witness if it constructs the whole object.
    `CSMenuManImp::CSMenuManImp` is 552 bytes for a 0x8a0 object and never touches 0x13c, which
    Ghidra names `disableSaveMenu`. Its silence about an offset means nothing.
  * The base-register filter is part of the measurement, not a convenience. Reading `FieldArea`'s
    ctor with rbx and rdi admitted reported 101 offsets; those two registers hold constructor
    arguments, and three of the extra offsets belonged to other objects. `~GameDataMan` is worse:
    its second instruction is `mov (%rcx),%rcx`, so rcx stops being `this` immediately.

The audit that produced the sweep is reproducible: `scripts/audit-name-derived-offsets.py` buckets
every field-offset constant in the tree by the provenance its own comment claims (measured / name
/ none). It is a report, not a gate; the findings live here.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
IMAGE_BASE = 0x140000000
IMAGE_1162 = Path(os.environ.get("ER_DEOBF_1162", REPO / "eldenring-deobf.bin"))
IMAGE_1170 = Path(os.environ.get("ER_DEOBF_1170", REPO / "eldenring-deobf-1.17.bin"))
MATCHER = REPO / "scripts" / "pair-object-field-drift.py"

# Directories that are copies of this tree, not this tree.
EXCLUDED_DIRS = (".git", "target", "node_modules", ".worktrees", ".claude")

# --------------------------------------------------------------------------------------------
# The measurement. Each row: which object, which offset, what 1.17 did to it, and the witness --
# a function whose two bodies align instruction-for-instruction, so the displacement pair is the
# same access to the same field in both builds. `bases` restricts the reading to memory operands
# on registers that provably hold `this` in that function.
#
# Held rows are not decoration. They are the frozen negatives: a matcher that has become
# over-broad (reporting every offset as +8) fails them, and `--selftest` proves that by perturbing
# each of them in the other direction.
# --------------------------------------------------------------------------------------------
PGD_CTOR = dict(va16=0x14025D580, len16=1199, va17=0x14025D550, len17=1199, bases=("rbx", "rcx"))
PLAYER_INS_CTOR = dict(va16=0x14064FE40, len16=2143, va17=0x140650C90, len17=2143, bases=("rbx",))
PGD_COPY_CHR_NAME = dict(
    va16=0x1402610C0, len16=120, va17=0x1402610D0, len17=120, bases=("rcx", "rbx", "rdi", "rsi")
)
# FD4::FD4PadDevice. `this` is rbx after `mov rbx,rcx` in both bodies; the base filter matters here
# because the constructor also dispatches through vtables on r8/r9/rax, and counting those would
# manufacture "held" readings for foreign objects (the mistake that produced spurious PlayerIns
# witnesses at 0x480/0x508/0x530).
FD4_PAD_DEVICE_CTOR = dict(
    va16=0x142663880, len16=661, va17=0x142666090, len17=661, bases=("rbx", "rcx")
)
# FD4::FD4PadManager. `this` is rsi after `mov rsi,rcx`.
FD4_PAD_BUILDER_A = dict(
    va16=0x140240E70, len16=690, va17=0x140240E70, len17=690, bases=("rsi", "rcx")
)
# `DLUID::PadDevice<DLKR::DLMultiThreadingPolicy>::Poll` -- a different class from
# `FD4::FD4PadDevice` above, and the whole reason these two sit next to each other here. `this` is
# rdi after `mov rdi,rcx`; rcx is admitted for the first three instructions before the move.
#
# This is the function `crates/er-quickload/src/experiments/can_move_probe.rs` detours, and the
# object it writes `+0x89c`/`+0x8a0` into is this function's own `this`. The class is established
# from the image, not from a name: the pointer to this function occurs exactly once in each flat
# image (1.16.2 0x1430ca030, 1.17 0x1430cd170), both at `+0x128` of the vtable at 0x1430c9f08 ->
# 0x1430cd048, whose RTTI complete-object-locator has `offset == 0` and whose type descriptor
# spells `.?AV?$PadDevice@VDLMultiThreadingPolicy@DLKR@@@DLUID@@` in both builds. `offset == 0` is
# what makes `this` the allocation base rather than a secondary-base sub-object, and the
# constructor agrees: `mov qword ptr [r14], rax` with rax = that vtable, r14 = rcx.
PAD_DEVICE_POLL = dict(
    va16=0x141F6BAD0, len16=2911, va17=0x141F6D8D0, len17=2911, bases=("rdi", "rcx")
)
# `FD4::FD4StepTemplateBase<CS::CSSystemStep, FD4::FD4TaskBase>`, the base sub-object at offset 0
# of `CS::CSSystemStep`. `this` is rsi after `mov rsi,rcx`. Reached as
# `CSSystemStep::CSSystemStep` (1.16.2 0x140dec7c0) -> 0x140dec620 -> here; the 1.17 pair is the
# same chain shifted +0x1e00 and is corroborated three ways that share nothing: identical body
# length (226) and identical instruction count (57/57 aligned), the ledger row
# 0xdec7c0 -> 0xdee5c0 for the caller, and the singleton store this ctor's grandparent performs
# landing on the mapped global (`48 89 05` at 0x140dec268 -> 0x143d85680; at 0x140dee068 ->
# 0x143d89700).
CS_SYSTEM_STEP_TEMPLATE_CTOR = dict(
    va16=0x140DEC6D0, len16=226, va17=0x140DEE4D0, len17=226, bases=("rsi", "rcx")
)
# ---- the name-provenance sweep of 2026-08-31 ------------------------------------------------
# Six more constructors, added because the constants they settle had a name for provenance rather
# than a measurement -- the shape that produced the 0x40 error below. Each 1.17 pair comes from
# `docs/recon/rva-map-1162-to-1170.functions.tsv` and is corroborated by identical body length and
# a full instruction alignment.
#
# `CS::ChrAsm::ChrAsm`. `this` is rcx, then rsi after `mov %rcx,%rsi`. It constructs the whole
# object front to back, so its offset set is the layout: 0x0, 0x4, 0x24, 0x7c, 0xd4, 0xdc, ending
# at 0xe8 = size_of::<ChrAsm>().
CHR_ASM_CTOR = dict(va16=0x1403BE1B0, len16=161, va17=0x1403BE1C0, len17=161, bases=("rcx", "rsi"))
# `CS::FieldArea::FieldArea`. `this` is rcx, then rsi after `mov %rcx,%rsi`. The base filter is
# load-bearing here: rbx and rdi hold CONSTRUCTOR arguments, and counting them manufactures
# "held" readings for foreign objects -- it reported 101 offsets instead of the true 98, three of
# which were nested members' own fields.
FIELD_AREA_CTOR = dict(
    va16=0x140618BF0, len16=1566, va17=0x140619A40, len17=1566, bases=("rsi", "rcx")
)
# `CS::CSMenuManImp::CSMenuManImp`. `this` is rcx, then rbx after `mov %rcx,%rbx`. NOTE it is 552
# bytes for a 0x8a0 object, so it does not touch every field: `disableSaveMenu` at 0x13c is a real
# member of Ghidra's type and has no access here at all. Absence from this witness's offset set is
# therefore not evidence that a byte is not a field -- unlike the step-template ctor above, which
# accounts for its whole base sub-object and is why 0x40's absence there meant something.
CS_MENU_MAN_IMP_CTOR = dict(
    va16=0x1407650A0, len16=552, va17=0x140765EF0, len17=552, bases=("rcx", "rbx")
)
# `CS::GameMan::GameMan`. `this` is rcx, then rsi. 1296 instructions, 142 field offsets, zero
# moved -- the largest object in this table and the one with the most repo constants on it.
GAME_MAN_CTOR = dict(
    va16=0x140675EA0, len16=6895, va17=0x140676CF0, len17=6895, bases=("rsi", "rcx")
)
# `CS::GameDataMan::~GameDataMan`. The DESTRUCTOR rather than the constructor, because that is
# what Ghidra has a symbol for on this class; it releases each owned sub-object through `this`, so
# the pointer members are all witnessed. `this` is rbx ONLY: the second instruction is
# `mov (%rcx),%rcx`, which rebinds rcx to the first member, so admitting rcx as a base would count
# a foreign object's fields from that point on.
GAME_DATA_MAN_DTOR = dict(va16=0x140254D40, len16=1103, va17=0x140254D10, len17=1103, bases=("rbx",))
# `CS::WorldBlockInfo::WorldBlockInfo`. `this` is rcx, then rbx; rdx is the `BlockId*` argument.
# Another whole-object ctor: 28 offsets from 0x0 to 0xd4 in a 0xe0-byte object.
WORLD_BLOCK_INFO_CTOR = dict(
    va16=0x1406610E0, len16=244, va17=0x140661F30, len17=244, bases=("rcx", "rbx")
)

WITNESSES = (
    # ---- CS::PlayerGameData -----------------------------------------------------------------
    ("PlayerGameData", "equipment", 0x2B0, 0x2B0, PGD_CTOR, "constructor stores its own vtable at [this+0]"),
    ("PlayerGameData", "face_data", 0x760, 0x760, PGD_CTOR, "constructor"),
    ("PlayerGameData", "chr_name_string_a (0x8e8)", 0x8E8, 0x8E8, PGD_CTOR, "constructor"),
    ("PlayerGameData", "is_main_player", 0x8F0, 0x8F0, PGD_CTOR, "constructor"),
    # The autoload identity path. The character name lives in three PGD storages -- the raw
    # wchar_t[17] at 0x9c and two CSWordCheckedStringInternal* at 0x8e8 / 0x8f8 -- and CopyChrName
    # is the native that writes all three, so one aligned function witnesses the whole identity
    # surface the loading screen and the save-slot list read.
    ("PlayerGameData", "character_name (raw)", 0x9C, 0x9C, PGD_COPY_CHR_NAME, "CopyChrName"),
    ("PlayerGameData", "chr_name_string_b (0x8f8)", 0x8F8, 0x8F8, PGD_COPY_CHR_NAME, "CopyChrName"),
    ("PlayerGameData", "is_main_player (second witness)", 0x8F0, 0x8F0, PGD_COPY_CHR_NAME, "CopyChrName"),
    ("PlayerGameData", "old_mount_handle (last held)", 0x958, 0x958, PGD_CTOR, "constructor"),
    ("PlayerGameData", "stat sub-object start", 0x960, 0x964, PGD_CTOR, "constructor; 1.17 adds `mov byte [this+0x960],0` before it"),
    ("PlayerGameData", "menu_ref_special_effect_1", 0xA78, 0xA80, PGD_CTOR, "constructor"),
    (
        "PlayerGameData",
        "item_replenish_tracker",
        0x5E8,
        0x5E8,
        dict(va16=0x140786430, len16=179, va17=0x1407872B0, len17=179, bases=("rcx", "rbx", "rdi", "rsi")),
        "SetItemReplenishState, the function er-better-refills detours",
    ),
    (
        "PlayerGameData",
        "resistance_gauges",
        0x9C8,
        0x9CC,
        dict(va16=0x14025FA60, len16=10, va17=0x14025FA70, len17=10, bases=("rcx",)),
        "GetResistanceGauge leaf accessor -- independent of the constructor",
    ),
    (
        "PlayerGameData",
        "proc_status_timer_max",
        0xA38,
        0xA3C,
        dict(va16=0x14025FA10, len16=12, va17=0x14025FA20, len17=12, bases=("rcx",)),
        "GetProcStatusTimerMax leaf accessor",
    ),
    (
        "PlayerGameData",
        "scadutree_blessing_override",
        0xAB4,
        0xABC,
        dict(va16=0x14025F5F0, len16=24, va17=0x14025F5D0, len17=24, bases=("rcx",)),
        "GetScadutreeBlessing; the pair is also map-rvas-1162-to-1170.py's KNOWN_MAPPINGS control",
    ),
    (
        "PlayerGameData",
        "scadutree_blessing (held INSIDE a function that also moved)",
        0xFC,
        0xFC,
        dict(va16=0x14025F5F0, len16=24, va17=0x14025F5D0, len17=24, bases=("rcx",)),
        "GetScadutreeBlessing reads 0xfc and 0xab4 in the same 5 instructions; only one moved",
    ),
    # ---- CS::PlayerIns ----------------------------------------------------------------------
    ("PlayerIns", "held below the insertion", 0x368, 0x368, PLAYER_INS_CTOR, "constructor stores its own vtable at [this+0]"),
    ("PlayerIns", "held above the removal", 0x580, 0x580, PLAYER_INS_CTOR, "constructor"),
    ("PlayerIns", "session_manager_player_entry", 0x6B8, 0x6B8, PLAYER_INS_CTOR, "constructor"),
    (
        "PlayerIns",
        "session_manager_player_entry (second witness)",
        0x6B8,
        0x6B8,
        dict(va16=0x1406507A0, len16=913, va17=0x1406515F0, len17=913, bases=("rcx", "rbx", "rdi", "rsi")),
        "~PlayerIns",
    ),
    (
        "PlayerIns",
        "field in the shifted band (0x532)",
        0x532,
        0x53A,
        dict(va16=0x140653290, len16=600, va17=0x1406540E0, len17=600, bases=("rcx", "rdi")),
        "vtable slot 89 of CS::PlayerIns",
    ),
    (
        "PlayerIns",
        "field in the shifted band (0x538)",
        0x538,
        0x540,
        dict(va16=0x1403F09F0, len16=200, va17=0x1403F0C20, len17=200, bases=("rcx", "rdi")),
        "vtable slot 154 of CS::PlayerIns",
    ),
    # ---- FD4::FD4PadDevice ------------------------------------------------------------------
    # The virtual-key array the input harness writes. A write through a moved offset does not
    # return a wrong value, it corrupts whatever now lives there -- and this is the path every
    # agent-driven menu navigation runs on, so a wrong write poisons runtime evidence rather than
    # producing one bad log line.
    #
    # The census could not settle this row (`CS::CSInGamePad` yields 2 usable paired bodies out of
    # 40) because the owner was misattributed. It is `FD4::FD4PadDevice`: all four call sites of
    # the writer below load `rcx` from `FD4PadManager::padDevices[dev]`, and `FD4PadManager::Init`
    # fills that array with `HeapAlloc(0x3c0)` + `FD4PadDevice::FD4PadDevice` + its vftable.
    (
        "FD4PadDevice",
        "virtual_key_array (VK_ARRAY_88_OFFSET)",
        0x88,
        0x88,
        dict(va16=0x1426634A0, len16=29, va17=0x142665CB0, len17=29, bases=("rcx",)),
        "the ONLY writer of the array, `mov byte [rcx+rdx*2+0x88],1`; paired by a masked signature "
        "that is unique in BOTH images with the displacement AND the bound wildcarded, and "
        "independently by call-graph topology (0 callees, the same 4 callers)",
    ),
    (
        "FD4PadDevice",
        "held immediately below the key array (0x80)",
        0x80,
        0x80,
        FD4_PAD_DEVICE_CTOR,
        "FD4PadDevice constructor, 168/168 aligned with zero moved offsets; brackets 0x88 from below",
    ),
    (
        "FD4PadDevice",
        "held (0x68)",
        0x68,
        0x68,
        FD4_PAD_DEVICE_CTOR,
        "FD4PadDevice constructor",
    ),
    # ---- FD4::FD4PadManager -----------------------------------------------------------------
    # The base the write goes through. Pinning the field without pinning the pointer that reaches
    # it would leave half the address unmeasured.
    (
        "FD4PadManager",
        "pad_devices (PAD_MGR_DEVICES_18_OFFSET)",
        0x18,
        0x18,
        FD4_PAD_BUILDER_A,
        "virtual-key builder A, 195/195 aligned; it computes the writer's `this` as "
        "`*(manager + 0x18 + dev*8)`",
    ),
    (
        "FD4PadManager",
        "pad_devices.count (PAD_DEVICES_COUNT_40_OFFSET)",
        0x40,
        0x40,
        FD4_PAD_BUILDER_A,
        "virtual-key builder A; the bound the game itself checks `dev` against",
    ),
    (
        "FD4PadManager",
        "pad_maps (0x48)",
        0x48,
        0x48,
        FD4_PAD_BUILDER_A,
        "virtual-key builder A; the frozen negative for the padDevices rows -- 0x18 and 0x48 are "
        "adjacent members of the same object and a matcher that confused them would fail here",
    ),
    # The two members of `FD4::FD4PadDevice` that `inject_all_pad_devices` walks to reach the
    # analog-stick write below. Pinning the field without pinning the pointer chain that reaches it
    # would leave half the address unmeasured -- the same argument the FD4PadManager rows above
    # make, one indirection further down.
    (
        "FD4PadDevice",
        "devices vector entries (FD4PADDEVICE_DEVICES_OFFSET)",
        0x10,
        0x10,
        FD4_PAD_DEVICE_CTOR,
        "FD4PadDevice constructor, 168/168 aligned with zero moved offsets; `lea r14,[rbx+0x10]` "
        "is the DLFixedVector<DLUID::device*,4> element base it fills from device types 3..6",
    ),
    (
        "FD4PadDevice",
        "devices vector count (FD4PADDEVICE_DEVICE_COUNT_OFFSET)",
        0x38,
        0x38,
        FD4_PAD_DEVICE_CTOR,
        "FD4PadDevice constructor, `mov qword ptr [rcx+0x38],rdi` with rdi = 0; it is the counter "
        "the ctor's own `if (4 < count + 1) DLPanic(\"out of memory.\")` bounds, which is where "
        "the capacity 4 comes from",
    ),
    # ---- DLUID::PadDevice<DLKR::DLMultiThreadingPolicy> --------------------------------------
    # The rows that settle a "THIS LOOKS LIKE AN OUT-OF-BOUNDS WRITE" report.
    #
    # `can_move_probe` writes two floats at `+0x89c`/`+0x8a0`, and read beside the FD4PadDevice
    # rows above -- `HeapAlloc(0x3c0)` = 960 bytes -- that reads as 1244 bytes past the end of a
    # live allocation, i.e. exactly the `stamp_vk_direct` defect again. It is not. They are two
    # different classes that both end in "PadDevice", and the FD4 one holds the DLUID ones:
    #
    #     FD4::FD4PadDevice                 HeapAlloc(0x3c0),  vftable 0x143295998
    #       +0x08  -> DLUID::VirtualMultiDevice   HeapAlloc(0x7f8)   (factory type 7)
    #       +0x10  -> DLUID::PadDevice[0..count]  HeapAlloc(0xa68)   (factory types 3..6)
    #
    # The four sizes come from one factory (`DLUserInputManagerImpl` device factory, 1.16.2
    # 0x141f28a80 -> 1.17 0x141f2a880) and are frozen in ALLOC_WITNESSES below; a write ending at
    # 0x8a4 fits in only the 0xa68 one. The rows here are the other half: that `0x89c` and `0x8a0`
    # are fields of that class in both builds, and that the game itself stores to them.
    #
    # "The game itself stores to them" is the decisive form of the argument. An offset the engine's
    # own code writes on this same `this` is in-bounds by construction -- no allocation arithmetic
    # required to believe it, and no way for the repo to be writing into padding or off the end.
    (
        "DLUID::PadDevice",
        "left_stick_x (PAD_STICK_LX_OFFSET)",
        0x89C,
        0x89C,
        PAD_DEVICE_POLL,
        "the device poll itself, 616/616 aligned with 72 field offsets and ZERO moved. It STORES "
        "to 0x89c on its own `this` from all three input sources -- DirectInput "
        "(`(axis - centre) / range`), XInput (`GetState` thumb LX) and ScePad -- so this is the "
        "engine's own left-stick X, not a byte the repo picked",
    ),
    (
        "DLUID::PadDevice",
        "left_stick_y (PAD_STICK_LY_OFFSET) -- the forward stick the move probe writes",
        0x8A0,
        0x8A0,
        PAD_DEVICE_POLL,
        "the device poll; it stores 0x8a0 in the same three branches as 0x89c, one dword above it",
    ),
    (
        "DLUID::PadDevice",
        "held immediately below the stick block (0x894, the XInput user index)",
        0x894,
        0x894,
        PAD_DEVICE_POLL,
        "the device poll; `cmp dword [this+0x894],0` is the branch that chooses DirectInput over "
        "XInput. Brackets the stick pair from below",
    ),
    (
        "DLUID::PadDevice",
        "highest offset the poll touches (0xa60)",
        0xA60,
        0xA60,
        PAD_DEVICE_POLL,
        "the device poll; 0xa60 is the top of its witnessed set and sits 8 bytes below the 0xa68 "
        "the factory allocates, which is the independent corroboration that the object reaching "
        "this hook is the 0xa68 class and not one of the three smaller ones the same factory hands "
        "out (0x8f0 KeyboardDevice, 0x810, 0x7f8 VirtualMultiDevice)",
    ),
    # ---- CS::CSSystemStep (its FD4StepTemplateBase base sub-object) --------------------------
    # The row that exists because a constant was never measured at all.
    #
    # Every other row here answers "did 1.17 move this field". This one answers the question that
    # comes first and had been skipped: is the offset a field of this object in either build.
    # `CS_SYSTEM_STEP_CURRENT_STATE_OFFSET` said 0x40 from its introduction until 2026-08-31. The
    # constructor never writes 0x40; it writes 0x48. 0x40 is
    # `FD4ComponentAttachSystem_Step::allocator`, a live `DLAllocator*`, so `oracle_system_step_
    # label` read a pointer's low half, failed the `0..=20` range test and emitted `"?"` with
    # `oracle_system_step_state = -95247096` (= 0xfa52a508) forever. Nothing faulted: a legal i32
    # out of a legal read is exactly the silent class this gate exists for, and it was silent on
    # 1.16.2 too -- so a drift-only check (this gate's usual question) could never have caught it.
    #
    # The wrong value came from back-solving the layout off a field name. The sibling
    # `fromsoftware-rs` `FD4StepTemplateBase` has a member spelled `unk48` right after
    # `requested_state`; "unk48 is at 0x48" puts `current_state` at 0x40. That member is misnamed
    # (it sits at 0x50) and the Rust struct's own computed layout was right. The measurement below
    # is what a name cannot be.
    (
        "CSSystemStep",
        "current_state (CS_SYSTEM_STEP_CURRENT_STATE_OFFSET); requested_state is the adjacent +0x4c",
        0x48,
        0x48,
        CS_SYSTEM_STEP_TEMPLATE_CTOR,
        "the step-template constructor, 57/57 aligned with zero moved offsets; it zeroes "
        "current_state and requested_state together as one qword (`48 89 5e 48`, byte-identical "
        "at 0x140dec744 and 0x140dee544)",
    ),
    (
        "CSSystemStep",
        "step_done_flag (0x50) -- the field `fromsoftware-rs` misnames `unk48`",
        0x50,
        0x50,
        CS_SYSTEM_STEP_TEMPLATE_CTOR,
        "step-template constructor; brackets current_state from above and is the frozen negative "
        "for the 0x40-vs-0x48 confusion -- a matcher off by one field would land here",
    ),
    (
        "CSSystemStep",
        "stepper_fn_table (0x10)",
        0x10,
        0x10,
        CS_SYSTEM_STEP_TEMPLATE_CTOR,
        "step-template constructor stores the 21-entry StepperFn table here (1.16.2 0x143d85760, "
        "1.17 0x143d897e0), which is where the state LABELS come from",
    ),
    (
        "CSSystemStep",
        "debug_state_label (0xa0)",
        0xA0,
        0xA0,
        CS_SYSTEM_STEP_TEMPLATE_CTOR,
        'step-template constructor stores L"NotExecuting" here; the highest witnessed field, so it '
        "is what bounds this object's SAFE_REGIONS entry",
    ),
    # ---- The name-provenance sweep (2026-08-31) ---------------------------------------------
    # Every row below settles a constant whose stated provenance was a NAME: a sibling-crate
    # member, an `offset_of!` the compiler evaluated against that binding, or a hand walk down a
    # `#[repr(C)]` declaration counting `unkNN` members. All of them turned out right. They are
    # frozen anyway, because "right" and "measured" were two different states until now, and the
    # gap between them is exactly where 0x40 lived for months.
    #
    # ---- CS::ChrAsm -------------------------------------------------------------------------
    # `CHR_ASM_UNKD4_OFFSET` is computed as `equipment_param_ids + 22 * 4` because the member is
    # private upstream and cannot be reached by `offset_of!` -- a layout walk, the same argument
    # shape that produced 0x40. The number is right; the argument was never evidence.
    (
        "ChrAsm",
        "unk4 (0x4) -- the lower bracket for CHR_ASM_EQUIPMENT_OFFSET",
        0x04,
        0x04,
        CHR_ASM_CTOR,
        "`mov %ebx,0x4(%rcx)` with ebx = 0. `equipment` itself has NO displacement to freeze -- "
        "the ctor reaches it as `add $0x8,%rcx ; call CS::ChrAsmEquipment::ChrAsmEquipment`, so "
        "the member is identified by a NAMED callee taking this+8 as its `this` rather than by a "
        "field access. What this row can freeze is the bracket, and the bracket here is exact: "
        "0x4 below, 0x24 above, and size_of::<ChrAsmEquipment>() = 0x1c fills 0x8..0x24 with "
        "nothing left over. The 0x8 literal is pinned in the SOURCE half regardless",
    ),
    (
        "ChrAsm",
        "gaitem_handles (CHR_ASM_GAITEM_HANDLES_OFFSET)",
        0x24,
        0x24,
        CHR_ASM_CTOR,
        "`lea 0x24(%rsi),%rcx` with edx=4 and r8d=0x16 into the array-ctor iterator: 22 elements "
        "of 4 bytes, which is also the second independent witness for ENTRY_COUNT = 22",
    ),
    (
        "ChrAsm",
        "equipment_param_ids (CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET)",
        0x7C,
        0x7C,
        CHR_ASM_CTOR,
        "`lea 0x7c(%rsi),%rdi ; mov $0x16,%ecx ; rep stos %eax` with eax = -1",
    ),
    (
        "ChrAsm",
        "unkd4 + unkd8 (CHR_ASM_UNKD4_OFFSET, CHR_ASM_UNKD8_OFFSET)",
        0xD4,
        0xD4,
        CHR_ASM_CTOR,
        "`movq $-1,0xd4(%rsi)` -- EIGHT bytes, so the one instruction sets both dwords; 0xd8 has "
        "no displacement of its own and is witnessed only as the upper half of this store",
    ),
    (
        "ChrAsm",
        "bolt_loaded_states (0xdc) -- the frozen negative that brackets unkd8 from above",
        0xDC,
        0xDC,
        CHR_ASM_CTOR,
        "`lea 0xdc(%rsi),%rax` + a 12-iteration byte loop ending at 0xe8 = size_of::<ChrAsm>(); a "
        "layout walk that mis-sized the param-id array would land here instead of on 0xd4",
    ),
    # ---- CS::FieldArea ----------------------------------------------------------------------
    # `FIELD_AREA_WORLD_INFO_OWNER_OFFSET`'s comment said "both the CI-pinned and local binding
    # layouts pin this field at the same offset" -- two copies of one declaration agreeing with
    # each other, which is not a measurement. 0x10 and 0x18 are each other's frozen negative: they
    # are adjacent `WorldInfoOwner*` members and are not interchangeable (the native block lookups
    # take the second).
    (
        "FieldArea",
        "world_info_owner (FIELD_AREA_WORLD_INFO_OWNER_OFFSET)",
        0x10,
        0x10,
        FIELD_AREA_CTOR,
        "FieldArea constructor, 311/311 aligned with 98 offsets and zero moved; Ghidra's 1.16.2 "
        "type names it `worldInfoOwner` at 0x10",
    ),
    (
        "FieldArea",
        "world_info_owner2 (FIELD_AREA_WORLD_INFO_OWNER2_OFFSET)",
        0x18,
        0x18,
        FIELD_AREA_CTOR,
        "FieldArea constructor; the adjacent `worldInfoOwner2`, and the frozen negative for the "
        "row above -- a matcher confusing the two would fail here",
    ),
    # ---- CS::WorldBlockInfo -----------------------------------------------------------------
    # The most literal instance of the failure shape the sweep found: the comment derives 0x48 by
    # walking the upstream `#[repr(C)]` and counting members -- `unk3c 0x3c`, `unk40 0x40`,
    # `unk41[7] 0x41` -> `msb_res_cap 0x48`. That chain is only as good as the `unkNN` names in it.
    (
        "WorldBlockInfo",
        "msb_res_cap (WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET)",
        0x48,
        0x48,
        WORLD_BLOCK_INFO_CTOR,
        "WorldBlockInfo constructor, 55/55 aligned with zero moved offsets; Ghidra's 1.16.2 type "
        "names it `msbResCap` at 0x48 and `CS::WorldAreaInfo::GetBlockInfoByMsbResCap` "
        "(0x14062f010) compares that member against its argument",
    ),
    (
        "WorldBlockInfo",
        "held below msb_res_cap (0x40) -- the frozen negative for the layout walk",
        0x40,
        0x40,
        WORLD_BLOCK_INFO_CTOR,
        "WorldBlockInfo constructor; a walk that mis-sized `unk41[7]` by one member lands on 0x40 "
        "or 0x50 rather than 0x48, so pinning the neighbour is what makes the walk refutable",
    ),
    # ---- CS::GameDataMan --------------------------------------------------------------------
    # `GAME_DATA_MAN_PLAYER_OFFSET` is a bare 0x08 in three crates, and the same field is reached
    # a fourth time as `offset_of!(GameDataMan, main_player_game_data)`. Four homes, one of them
    # the compiler's answer to the same 1.16.2 binding, and no measurement behind any of them.
    (
        "GameDataMan",
        "main_player_game_data (GAME_DATA_MAN_PLAYER_OFFSET)",
        0x08,
        0x08,
        GAME_DATA_MAN_DTOR,
        "~GameDataMan, 359/359 aligned with 17 offsets and zero moved; Ghidra's 1.16.2 type names "
        "it `mainPlayerGameData` at 0x8",
    ),
    (
        "GameDataMan",
        "menu_system_save_load (0x60) -- frozen negative",
        0x60,
        0x60,
        GAME_DATA_MAN_DTOR,
        "~GameDataMan; keeps the pointer-member spacing honest, since a whole-object shift would "
        "move 0x8 and 0x60 together and neither did",
    ),
    # ---- CS::GameMan ------------------------------------------------------------------------
    # `GAME_MAN_SAVE_STATE_B80_OFFSET` cited `offset_of!(GameMan, save_state)` in the Ghidra type
    # plus "the same +0xb80 store in every 1.17 wrapper body" -- half a measurement, with no
    # address attached to it. Here is the address.
    (
        "GameMan",
        "save_state (GAME_MAN_SAVE_STATE_B80_OFFSET)",
        0xB80,
        0xB80,
        GAME_MAN_CTOR,
        "GameMan constructor, 1296/1296 aligned with 142 offsets and zero moved; the store is "
        "`mov %r14d,0xb80(%rsi)` at 0x14067616f (1.17 0x140676fbf). Second, independent witness "
        "for the NAME (2026-08-31): `IsSaveState1` (0x14067a010) and `IsSaveState2` "
        "(0x140679ff0) are two-instruction leaves that `cmp dword ptr [rax+0xb80],N` off the "
        "GameMan singleton, so the field's spelling is in the image. Three constants called this "
        "LOAD_IN_PROGRESS / LOAD_PHASE / LOAD_FSM until then; a complete scan of the 37 accesses "
        "shows the SAVE lane stamps 1 and the LOAD lane stamps 2, so it is neither lane's flag",
    ),
    (
        "GameMan",
        "is_in_online_mode (0xbc8); server_connection_enabled is the BYTE above it at 0xbc9",
        0xBC8,
        0xBC8,
        GAME_MAN_CTOR,
        "the ctor writes `mov $1,0xbc8(%rsi)` as a DWORD, so 0xbc9 has no displacement of its own "
        "and the ctor alone cannot separate the two bools -- absence from a displacement set is "
        "not absence of a field when something WIDER covers the byte. The separator is "
        "`IsServerConnectionEnabled` (0x14067a190) = `mov 0x143d69918(%rip),%rax ; movzbl "
        "0xbc9(%rax),%eax ; ret`, a byte-width read at 0xbc9 off the GameMan singleton that is "
        "unique in the image; Ghidra names the field `serverConnectionEnabled` there",
    ),
    # The rest of `CS::GameMan` (2026-08-31). Seventeen more GameMan offsets were spelled by hand
    # across six crates with no provenance at all. All seventeen are accounted for by the one
    # constructor pairing below and none of them moved. Seven are not displacements of their own:
    # a wider store covers them, which is the distinction `--cover` exists to make and the reason
    # each row below names the byte its store covers.
    (
        "GameMan",
        "warp_requested (GAME_MAN_WARP_REQUESTED_10_OFFSET)",
        0x10,
        0x10,
        GAME_MAN_CTOR,
        "`mov $0,0x10(%rcx)` at 0x140675ee5 (1.17 0x140676d35), a BYTE store in the prologue "
        "before the ctor switches to %rsi",
    ),
    (
        "GameMan",
        "save_slot (GAME_MAN_SAVE_SLOT_AC0_OFFSET)",
        0xAC0,
        0xAC0,
        GAME_MAN_CTOR,
        "`mov %r15d,0xac0(%rsi)` at 0x140676049 (1.17 0x140676e99)",
    ),
    (
        "GameMan",
        "load_target_map_id (GAME_MAN_LOAD_TARGET_MAP_ID_AC8_OFFSET)",
        0xAC8,
        0xAC8,
        GAME_MAN_CTOR,
        "`movl $-1,0xac8(%rsi)` at 0x140676057 (1.17 0x140676ea7) -- a map id initialised to the "
        "no-map sentinel, which is the shape the constant's consumer expects",
    ),
    (
        "GameMan",
        "0xb5c, the dword covering GAME_MAN_SUBMIT_GATE_B5E_OFFSET (0xb5e)",
        0xB5C,
        0xB5C,
        GAME_MAN_CTOR,
        "`movl $0,0xb5c(%rsi)` at 0x140676134 (1.17 0x140676f84); 0xb5e is the third byte of that "
        "dword and has no displacement of its own",
    ),
    (
        "GameMan",
        "0xb70, the dword covering the save-request bytes at 0xb72 and 0xb73",
        0xB70,
        0xB70,
        GAME_MAN_CTOR,
        "`movl $0,0xb70(%rsi)` at 0x14067614c (1.17 0x140676f9c). Five constants in three crates "
        "name bytes inside this one store: GAME_MAN_SAVE_REQUEST_B72/B73_OFFSET, "
        "GAME_MAN_SAVE_REQUESTED_B72_OFFSET, GAME_MAN_FIELD_B73_OFFSET and "
        "GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET",
    ),
    (
        "GameMan",
        "requested_slot (GAME_MAN_REQUESTED_SLOT_B78_OFFSET)",
        0xB78,
        0xB78,
        GAME_MAN_CTOR,
        "`mov %r15d,0xb78(%rsi)` at 0x14067615f (1.17 0x140676faf); it is the dword immediately "
        "below save_state at 0xb80, so a walk that mis-sized either lands on the other",
    ),
    (
        "GameMan",
        "0xb7c, the word covering GAME_MAN_ENDING_FLAG_B7D_OFFSET (0xb7d)",
        0xB7C,
        0xB7C,
        GAME_MAN_CTOR,
        "`movw $0,0xb7c(%rsi)` at 0x140676166 (1.17 0x140676fb6) -- a TWO-byte store, so it "
        "covers 0xb7d and nothing wider does",
    ),
    (
        "GameMan",
        "DLDateTime at 0xb98 (GAME_MAN_DLDATETIME_B98_OFFSET and its upper half at 0xba0)",
        0xB98,
        0xB98,
        GAME_MAN_CTOR,
        "`lea 0xb98(%rsi),%rbx` at 0x1406761a3 (1.17 0x140676ff3), then `mov %r14,(%rbx)` and "
        "`and %r12,0x8(%rbx)` -- 0xba0 is written through %rbx, never through `this`, so the "
        "displacement set is silent there. Ghidra's 1.16.2 GameMan type names 0xb98 and 0xba8 "
        "`DLDateTime`; the pair was called a `LOAD_HANDLE` in this repo until it was measured",
    ),
    (
        "GameMan",
        "quit_phase (GAME_MAN_QUIT_PHASE_OFFSET, GAME_MAN_QUIT_PHASE_BC4_OFFSET)",
        0xBC4,
        0xBC4,
        GAME_MAN_CTOR,
        "`mov %r14d,0xbc4(%rsi)` at 0x140676213 (1.17 0x140677063), the dword directly below the "
        "online-mode pair at 0xbc8",
    ),
    (
        "GameMan",
        "eventWorldType (0xbca), covered by the dword store at 0xbc8",
        0xBC8,
        0xBC8,
        GAME_MAN_CTOR,
        "`movl $1,0xbc8(%rsi)` at 0x14067621a (1.17 0x14067706a) spans 0xbc8..0xbcc, so it covers "
        "0xbca as well as the two bools the row above separates. The ctor cannot NAME it, and the "
        "constant was GAME_MAN_SUBMIT_GATE_BCA_OFFSET until 2026-08-31 -- named after one "
        "consumer, the submit path 0x14067dc00. The separators are its own accessors: "
        "`EventWorldType` (0x140679820) = `movzx eax,0xbca(%rax)` and `SetWorldEventType` "
        "(0x14067aeb0) = `mov %cl,0xbca(%rax)`, both byte-width off the GameMan singleton; "
        "`IsMyWorld` (0x140679f90) tests it against 0. fromsoftware-rs types it `EventWorldType`",
    ),
    (
        "GameMan",
        "loadingScreenTextState (0xbf5), covered by the word store at 0xbf4",
        0xBF4,
        0xBF4,
        GAME_MAN_CTOR,
        "`mov $0x100,0xbf4(%rsi)` at 0x140676242 (1.17 0x140677092) is a TWO-byte store, so it "
        "covers 0xbf5 and nothing wider does -- little-endian, it initialises disableMapEnterAnim "
        "(0xbf4) to 0 and loadingScreenTextState (0xbf5) to 1. Added 2026-08-31 with the rename "
        "of GAME_MAN_LOADING_MODE_BF5_OFFSET, whose name claimed the field was the loading MODE. "
        "The setter `FUN_14067a860` is a one-line `GameMan->loadingScreenTextState = arg`; the "
        "reader `FUN_14067a320` normalises mode 2 to 0 when it is false before writing "
        "CSMenuMan->loadingScreenData+0x8",
    ),
    (
        "GameMan",
        "stayInMultipleAreaBlockId (GAME_MAN_STAY_IN_MULTIPLAY_AREA_BLOCK_ID_C30_OFFSET, "
        "GAME_MAN_SAVED_MAP_C30_OFFSET)",
        0xC30,
        0xC30,
        GAME_MAN_CTOR,
        "`movl $-1,0xc30(%rsi)` at 0x14067628d (1.17 0x1406770dd) -- a BlockId initialised to the "
        "no-map sentinel. The 2026-08-31 audit renamed the er-reload-trace constant off "
        "\"current_map\": the map-move target is moveMapStepBlockId at +0x14, which "
        "`SetMoveMapStepBlockId` (0x14067abd0) writes FROM this field via the getter "
        "`FUN_140679560`. Four writers: the slot deserializer `FUN_14067bd70` (slot body+0x04, "
        "the read `er_save_loader::bnd4::slot_saved_map` performs), the serializer "
        "`FUN_14067dc00`, and the stay-in-multiplay update/reset pair `FUN_14067afa0` / "
        "`FUN_14067aac0`",
    ),
    (
        "GameMan",
        "0xcb0, the dword covering GAME_MAN_SUBMIT_GATE_CB1_OFFSET and _CB2_OFFSET",
        0xCB0,
        0xCB0,
        GAME_MAN_CTOR,
        "`mov %eax,0xcb0(%rsi)` at 0x14067630e (1.17 0x14067715e); 0xcb1 and 0xcb2 are bytes two "
        "and three of it",
    ),
    (
        "GameMan",
        "FD4FilePathBase at 0xdd0, whose DLString length is GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET",
        0xDD0,
        0xDD0,
        GAME_MAN_CTOR,
        "`lea 0xdd0(%rsi),%rdi` at 0x14067644b (1.17 0x14067729b) hands the whole 0xdd0..0xe08 "
        "region to `DLString<wchar_t>* InitAllocator` (0x140120390) and `CopyFromU16Array` "
        "(0x14011b190); the length store is `mov %r14,0x10(%rax)` with %rax = %rsi+0xde0, i.e. "
        "GameMan+0xdf0. The ctor never touches 0xdf0 through `this`, so a displacement census is "
        "silent there -- this repo called it RESIDENT_DEVICE until it was measured. Ghidra's "
        "1.16.2 GameMan type independently names 0xdd0 `FD4FilePathBase`, ending at 0xe08",
    ),
    # ---- CS::CSMenuManImp -------------------------------------------------------------------
    # `CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET`'s comment says the field is "unnamed in
    # fromsoftware-rs `unk748`" -- its provenance is that a filler array spans the byte, which
    # says nothing about where a member starts.
    (
        "CSMenuManImp",
        "in_game_menu_job (CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET)",
        0x798,
        0x798,
        CS_MENU_MAN_IMP_CTOR,
        "CSMenuManImp constructor, 121/121 aligned with 30 offsets and zero moved, and 0x790 and 0x7a0 are both witnessed too, so the member is bracketed on both sides; `lea "
        "0x798(%rbx),%rax` at 0x14076517b. That establishes 0x798 as a member BOUNDARY in both "
        "builds. It does NOT establish that the member is a `MenuJob*`, which is a separate claim "
        "this gate deliberately does not make",
    ),
    (
        "CSMenuManImp",
        "menu_data (CS_MENU_MAN_MENU_DATA_OFFSET) -- four crates hold their own copy of it",
        0x08,
        0x08,
        CS_MENU_MAN_IMP_CTOR,
        "CSMenuManImp constructor, `mov %rdi,0x8(%rcx)`; Ghidra names it `menuData` at 0x8",
    ),
)

# --------------------------------------------------------------------------------------------
# The object, not just the offset.
#
# Everything above measures where a field sits. That is the wrong half of the question for a
# write. A wrong offset misinforms -- `*(this + 0xNN)` returns the neighbouring member. A wrong
# object corrupts the heap: `stamp_vk_direct` wrote `object + 0x88 + (id - 1000) * 2` for ids up to
# 1080, which reaches byte 0x128, into an object that turned out to be `HeapAlloc(0x98)` = 152
# bytes. The offset 0x88 was correct and had not moved. The class was wrong, and nothing in this
# tree checked the class.
#
# So each row below pins an object this repo writes into by the only two facts that can refute a
# misattribution:
#
#   * its size -- the literal operand of the allocation the game makes for it, re-decoded from
#     both images every run. Not Ghidra's inferred `getStructure` size, which is an analysis
#     guess: the number the allocator is actually called with.
#   * its identity -- an anchor within the same decoded window tying that allocation to this
#     class: the constructor the allocation is handed to, or the vtable it is stamped with. A size
#     without an identity anchor is just a number at an address, and an address is exactly what
#     was wrong last time.
#
# `WRITE_REACH` below then asserts the arithmetic that actually matters: the highest byte this
# repo's writes can reach in that object, recomputed from the repo's own live literals, is inside
# the measured allocation. Two of these have almost no slack (`CS::ProfileSummary` reaches its
# final byte exactly; `CS::MoveMapStep` leaves 7 bytes), which is the whole reason to compute the
# reach rather than eyeball it.
ALLOC_IMM = "mov reg32, imm32"
ALLOC_DISP = "lea reg32, [reg + disp]"
IDENT_CALL = "call"
IDENT_RIP = "rip-relative reference"


def alloc_row(
    obj, size, form, va16, va17, ident_kind, ident16, ident17, how, scan16=None, scan17=None,
    scan_len=0x40,
):
    """One measured allocation: `size` at `va`, tied to `obj` by `ident` in the same window."""
    return dict(
        obj=obj,
        size=size,
        form=form,
        va=(va16, va17),
        ident_kind=ident_kind,
        ident=(ident16, ident17),
        scan=(scan16 if scan16 is not None else va16, scan17 if scan17 is not None else va17),
        scan_len=scan_len,
        how=how,
    )


ALLOC_WITNESSES = (
    alloc_row(
        "Scaleform::MemoryFile",
        0x30,
        ALLOC_DISP,
        0x1411645FC,
        0x1411663FC,
        IDENT_RIP,
        0x142BA4C80,
        0x142BA7D70,
        "`lea 0x30(%r12),%edx` (r12 is zero here) into the Scaleform allocator's vtable slot +0x50;"
        " 0x44 bytes later the object is stamped with `Scaleform::MemoryFile::vftable`, and the"
        " SAME constructor writes the three fields this repo overwrites -- data@0x18 (`mov"
        " %r15,0x18(%rdi)`), len@0x20, cursor@0x24. Every one of the 14 repo write sites gates on"
        " that vtable being the resolved MemoryFile vtable for the running build, so the identity"
        " is enforced at runtime as well as measured here",
        scan_len=0x60,
    ),
    alloc_row(
        "CS::ProfileSummary",
        0x1A58,
        ALLOC_IMM,
        0x140254B12,
        0x140254AE2,
        IDENT_CALL,
        0x1402619E0,
        0x1402619F0,
        "`mov $0x1a58,%ecx` in `CS::GameData`, handed straight to"
        " `CS::ProfileSummary::ProfileSummary`; the result is stored at `GameDataMan + 0x78`,"
        " which is exactly where this repo reads it from",
    ),
    alloc_row(
        "CS::ProfileSummary record stride",
        0x2A0,
        ALLOC_IMM,
        0x140261A2F,
        0x140261A3F,
        IDENT_RIP,
        0x1429E60E8,
        0x1429E90E8,
        "the `_eh_vector_constructor_iterator_` stride inside `CS::ProfileSummary::ProfileSummary`,"
        " whose own vtable store anchors the window. This is the bound the repo's `slot <"
        " PROFILE_SUMMARY_SLOT_COUNT` clamp is protecting: 0x18 + 10 * 0x2a0 == 0x1a58 leaves"
        " ZERO slack, so slot 10 would overrun by a whole record",
        scan16=0x1402619E0,
        scan17=0x1402619F0,
        scan_len=0x60,
    ),
    alloc_row(
        "CS::ProfileSummary record count",
        0xA,
        ALLOC_IMM,
        0x140261A34,
        0x140261A44,
        IDENT_RIP,
        0x1429E60E8,
        0x1429E90E8,
        "the element count passed beside the stride above -- the game's own answer to `how many"
        " slots`, and the number `PROFILE_SUMMARY_SLOT_COUNT` claims",
        scan16=0x1402619E0,
        scan17=0x1402619F0,
        scan_len=0x60,
    ),
    alloc_row(
        "CS::GameMan",
        0xE80,
        ALLOC_IMM,
        0x1406798A0,
        0x14067A6F0,
        IDENT_CALL,
        0x140675EA0,
        0x140676CF0,
        "`mov $0xe80,%ecx` then `GameMan::GameMan`; the result is stored to the singleton this"
        " repo resolves as GAME_MAN_SINGLETON_RVA (0x3d69918 -> 0x3d6d988, which the store"
        " immediately after the constructor call independently confirms)",
    ),
    alloc_row(
        "CS::CSMenuManImp",
        0x8A0,
        ALLOC_IMM,
        0x140DEFC58,
        0x140DF1A58,
        IDENT_CALL,
        0x1407650A0,
        0x140765EF0,
        "`mov $0x8a0,%ecx` then `CSMenuManImp::CSMenuManImp`; stored to CS_MENU_MAN_GLOBAL_RVA"
        " (0x3d6b7b0 -> 0x3d6f820), again confirmed by the store beside the call",
    ),
    alloc_row(
        "CS::CSMenuData",
        0xF0,
        ALLOC_IMM,
        0x140765248,
        0x140766098,
        IDENT_CALL,
        0x140767430,
        0x1407682B0,
        "`mov $0xf0,%ecx` then `CSMenuData::CSMenuData`, inside the CSMenuManImp constructor that"
        " stores it at `CSMenuManImp + 0x8` -- the path this repo walks to reach it",
    ),
    alloc_row(
        "CS::MoveMapStep",
        0x4C0,
        ALLOC_IMM,
        0x140AEC1CA,
        0x140AED4DA,
        IDENT_CALL,
        0x140AF28D0,
        0x140AF3BE0,
        "`mov $0x4c0,%ecx` then `MoveMapStep::MoveMapStep`, in `STEP_MoveMap_Init`",
    ),
    # ---- the pad device family, which is where both of this branch's heap overruns lived --------
    # `DLUserInputManagerImpl`'s device factory (1.16.2 0x141f28a80 -> 1.17 0x141f2a880) hands out
    # four differently-sized classes from one call, and a write ending at 0x8a4 fits in only two of
    # them. That is the entire bug: `+0x89c`/`+0x8a0` are fields of `DLUID::PadDevice`, and
    # `can_move_probe` was writing them into the object at `FD4PadDevice + 0x8`, which the
    # FD4PadDevice constructor fills from the factory with type 7 -- the 0x7f8 VirtualMultiDevice.
    # Both sizes are frozen here so the pair can never silently converge or diverge unnoticed.
    alloc_row(
        "DLUID::PadDevice",
        0xA68,
        ALLOC_IMM,
        0x141F28C2A,
        0x141F2AA2A,
        IDENT_CALL,
        0x141F6AF00,
        0x141F6CD00,
        "`mov $0xa68,%ecx` then `DLUID::PadDevice::PadDevice` in the device factory. This is the "
        "class that owns the analog-stick floats at +0x89c/+0x8a0 -- the game's own poll "
        "(0x141f6bad0 -> 0x141f6d8d0, the ONLY vtable slot referencing that function, in "
        "DLUID::PadDevice's vtable 0x1430c9f08 -> 0x1430cd048) writes both of them on its `this`",
        scan_len=0x50,
    ),
    alloc_row(
        "DLUID::VirtualMultiDevice",
        0x7F8,
        ALLOC_IMM,
        0x141F28CBB,
        0x141F2AABB,
        IDENT_CALL,
        0x141F6DF20,
        0x141F6FD20,
        "`mov $0x7f8,%ecx` then `DLUID::VirtualMultiDevice::VirtualMultiDevice` -- the factory's "
        "answer to device type 7, which is what `FD4PadDevice + 0x8` holds. 2040 bytes, so a "
        "float written at +0x8a0 lands 168..171 bytes PAST IT. Frozen as the negative: this row "
        "exists to make the wrong target's size visible next to the right one",
        scan_len=0x50,
    ),
    alloc_row(
        "FD4::FD4PadDevice",
        0x3C0,
        ALLOC_IMM,
        0x142667490,
        0x142669CA0,
        IDENT_CALL,
        0x142663880,
        0x142666090,
        "`mov $0x3c0,%ecx` then `FD4::FD4PadDevice::FD4PadDevice` in `FD4PadManager::Init`. The "
        "virtual-key array the input harness stamps lives at +0x88 of THIS object; the class it "
        "was attributed to until 2026-08-31 (`CS::CSInGamePad`) is HeapAlloc(0x98), which is why "
        "ids from 1008 up wrote off the end",
        scan_len=0x30,
    ),
    alloc_row(
        "CS::CSInGamePad",
        0x98,
        ALLOC_IMM,
        0x14024168F,
        0x14024168F,
        IDENT_CALL,
        0x1426647A0,
        0x142666FB0,
        "`mov $0x98,%ecx` then, 0xa7 bytes later, `CS::CSInGamePad::CSInGamePad`. THE HISTORICAL "
        "WRONG OBJECT, frozen so its size sits beside the right one: 152 bytes, against a "
        "virtual-key stamp reaching 0x129. Note both builds allocate it at the SAME address, "
        "which is precisely why an address alone is not identity",
        scan_len=0xC0,
    ),
    alloc_row(
        "CS::CSMenuProfModelRend",
        0xA30,
        ALLOC_IMM,
        0x1409AF421,
        0x1409B0671,
        IDENT_CALL,
        0x140BBDF20,
        0x140BBF5F0,
        "`mov $0xa30,%ecx` then the CSMenuProfModelRend constructor -- the same constructor that"
        " stores its own vtable at [this+0], which is how the class was established in the first"
        " place. The loading-screen portrait camera writes reach 0xa24 of this 0xa30 object",
    ),
)

# What this repo's writes can reach in each object above, recomputed from the repo's own live
# literals rather than from a number typed here. Each term is `addend + sum(coefficient * literal)`
# and the result must be <= the measured allocation size.
#
# Why compute it: the reach is what turns a measured size into a safety claim, and it moves when
# someone edits a constant. `FD4PadDevice` is the worked example -- its reach is
# `0x88 + (VK_ID_MAX - VK_ID_MIN) * 2 + 1`, so raising VK_ID_MAX past 1435 would walk off the end
# of the device and this gate would say so before the game did.
WRITE_REACH = (
    (
        "Scaleform::MemoryFile",
        4,
        ((1, "MEMORY_FILE_CURSOR_OFFSET"),),
        "cursor is the highest of the three fields; +4 for its u32 width",
    ),
    (
        "CS::ProfileSummary",
        0,
        ((1, "PROFILE_SUMMARY_RECORD_BASE"), (10, "PROFILE_SUMMARY_RECORD_STRIDE")),
        "record base plus all ten records -- the repo zeroes and rewrites whole records, so the"
        " reach is the end of slot 9. The coefficient 10 is PROFILE_SUMMARY_SLOT_COUNT, pinned"
        " separately below so a changed slot count cannot slip through as a changed coefficient",
    ),
    (
        "CS::GameMan",
        1,
        ((1, "SERVER_CONNECTION_ENABLED_BC9_OFFSET"),),
        "0xbc9 is the highest byte any repo write touches in GameMan",
    ),
    (
        "CS::CSMenuManImp",
        1,
        ((1, "CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET"),),
        "the only field this repo writes in CSMenuManImp",
    ),
    (
        "CS::CSMenuData",
        1,
        ((1, "CS_MENU_DATA_ENDING_FLAG_5E_OFFSET"),),
        "the higher of the two bytes this repo clears (0x5d / 0x5e)",
    ),
    (
        "CS::MoveMapStep",
        1,
        ((1, "MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET"),),
        "0x4b8 in a 0x4c0 object: SEVEN bytes of headroom, the tightest write in the tree after"
        " ProfileSummary",
    ),
    (
        "CS::CSMenuProfModelRend",
        4,
        ((1, "PROFILE_CAM_FOV_OFFSET"),),
        "the view matrix at 0x9e0 spans 64 bytes to 0xa20 and fov is written at 0xa20 itself, so"
        " fov+4 is the reach",
    ),
    (
        "FD4::FD4PadDevice",
        1,
        ((1, "VK_ARRAY_88_OFFSET"), (2, "VK_ID_MAX"), (-2, "VK_ID_MIN")),
        "`0x88 + (id - 1000) * 2` for every id the harness will accept. This is the row that would"
        " have caught the original bug had the object been right: against `CS::CSInGamePad`'s 0x98"
        " it reads 0x129 > 0x98 and goes red on sight",
    ),
    (
        "DLUID::PadDevice",
        4,
        ((1, "PAD_STICK_LY_OFFSET"),),
        "the forward-stick float the move probe writes; LY is the higher of the two (LX is 0x89c)",
    ),
)

# The drift model the witnesses above establish, expressed as the safe region per object: an
# offset in one of these ranges is the same field in both builds. Anything outside needs a
# version-aware constant, which this workspace does not have for either object.
SAFE_REGIONS = {
    # Nothing at or above 0x960 held: 0x958 is the highest witnessed-held offset and 0x960 is the
    # lowest witnessed-moved one, from the same function, so the boundary is exact.
    "PlayerGameData": ((0x0, 0x960),),
    # 8 bytes inserted in (0x398,0x3a8] and 8 removed in (0x560,0x580]; the object size is
    # unchanged and both ends hold, so the hazard is the band between them, not the whole struct.
    "PlayerIns": ((0x0, 0x3A0), (0x568, 0x760)),
    # Nothing in either pad object moved: the FD4PadDevice constructor aligns 168/168 and its
    # destructor 99/99 with zero moved offsets, the builder aligns 195/195, the writer 7/7, the
    # object is still `HeapAlloc(0x3c0)` and the vtable pairs slot for slot. The regions stop at
    # the highest offset actually witnessed, because "no witness moved" is not "no field moved" --
    # `CS::PlayerIns` is the standing counterexample, where a compensating insert/remove pair moved
    # the interior of a bracket while both ends held.
    "FD4PadDevice": ((0x0, 0x89),),
    "FD4PadManager": ((0x0, 0xA9),),
    # Nothing in `DLUID::PadDevice` moved either: its poll aligns 616/616 with all 72 of the
    # offsets it touches held, its constructor stamps the same vtable at [this+0] in both builds,
    # and the factory still calls `HeapAlloc(0xa68)` for it. The region stops just past 0xa60, the
    # highest witnessed offset (a byte), for the same reason as the pad rows above -- "no witness
    # moved" is not "no field moved", and the eight bytes from 0xa61 to the end of the allocation
    # have no witness either way.
    "DLUID::PadDevice": ((0x0, 0xA61),),
    # Nothing in the step template moved: the constructor aligns 57/57 and all 13 field offsets it
    # touches -- 0x0, 0x10, 0x18, 0x48, 0x50, 0x58, 0x60, 0x68, 0x69, 0x70, 0xa0, 0xa8, 0xac -- are
    # held. The region stops just past the highest witnessed offset (0xac, a dword), per the rule
    # the pad objects follow: "no witness moved" is not "no field moved", and `CS::PlayerIns` is
    # the standing counterexample.
    "CSSystemStep": ((0x0, 0xB0),),
}

# --------------------------------------------------------------------------------------------
# Source half. Each entry: the constant, the file that defines it, and the literal this gate
# verified against the images above.
# --------------------------------------------------------------------------------------------
PINNED_CONSTANTS = (
    ("PLAYER_GAME_DATA_EQUIP_GAME_DATA_OFFSET", "crates/er-better-refills/src/lib.rs", 0x2B0, "PlayerGameData"),
    ("PLAYER_GAME_DATA_ITEM_REPLENISH_TRACKER_OFFSET", "crates/er-better-refills/src/lib.rs", 0x5E8, "PlayerGameData"),
    ("PLAYER_GAME_DATA_EQUIP_OFFSET", "crates/er-build-import-runtime/src/grant.rs", 0x2B0, "PlayerGameData"),
    ("PLAYER_GAME_DATA_IS_MAIN_PLAYER_OFFSET", "crates/er-player-name-filter/src/lib.rs", 0x8F0, "PlayerGameData"),
    ("PLAYER_INS_SESSION_MANAGER_PLAYER_ENTRY_OFFSET", "crates/er-player-name-filter/src/lib.rs", 0x6B8, "PlayerIns"),
    ("VK_ARRAY_88_OFFSET", "crates/er-input-harness/src/pad_inject.rs", 0x88, "FD4PadDevice"),
    ("PAD_MGR_DEVICES_18_OFFSET", "crates/er-input-harness/src/pad_inject.rs", 0x18, "FD4PadManager"),
    ("PAD_DEVICES_COUNT_40_OFFSET", "crates/er-input-harness/src/pad_inject.rs", 0x40, "FD4PadManager"),
    # The loading-substep oracle's field. Two crates read it (the telemetry emitter and the
    # loading-bar sub-progression) and both now alias this one definition, so there is exactly one
    # literal in the tree to watch -- which is the state the 0x40 bug did not have: it was written
    # twice, in two crates, and drifted from nothing because it was born wrong.
    ("CS_SYSTEM_STEP_CURRENT_STATE_OFFSET", "crates/er-game-base/src/rva.rs", 0x48, "CSSystemStep"),
    # The frozen negative that is not about this object at all. These sit numerically inside the
    # band where PlayerGameData moved +4 (0x960..0xa78) and belong to `CS::CSMenuProfModelRend`,
    # whose constructor aligns 64/64 with every one of them held. A "+4 everything in that range"
    # sweep would have corrupted the loading-screen portrait camera; pinning them here makes that
    # sweep red instead of silent.
    ("PROFILE_CAM_YAW_OFFSET", "crates/er-loading-portrait-core/src/portrait_camera.rs", 0x9C8, "CSMenuProfModelRend"),
    ("PROFILE_CAM_PITCH_OFFSET", "crates/er-loading-portrait-core/src/portrait_camera.rs", 0x9CC, "CSMenuProfModelRend"),
    ("PROFILE_CAM_ASPECT_OFFSET", "crates/er-loading-portrait-core/src/portrait_camera.rs", 0xA24, "CSMenuProfModelRend"),
    # ---- the write-target constants, whose literals feed WRITE_REACH above -------------------
    # Duplicated deliberately across crates that ship independently; the gate looks for the name
    # everywhere and checks every definition it finds, so a drifted copy fails even if the
    # original is right.
    ("MEMORY_FILE_DATA_OFFSET", "crates/er-invasion-warp/src/map_gfx.rs", 0x18, "Scaleform::MemoryFile"),
    ("MEMORY_FILE_LEN_OFFSET", "crates/er-invasion-warp/src/map_gfx.rs", 0x20, "Scaleform::MemoryFile"),
    ("MEMORY_FILE_CURSOR_OFFSET", "crates/er-invasion-warp/src/map_gfx.rs", 0x24, "Scaleform::MemoryFile"),
    ("SCALEFORM_MEMORY_FILE_DATA_OFFSET", "crates/er-loading-portrait-core/src/layout.rs", 0x18, "Scaleform::MemoryFile"),
    ("SCALEFORM_MEMORY_FILE_LEN_OFFSET", "crates/er-loading-portrait-core/src/layout.rs", 0x20, "Scaleform::MemoryFile"),
    ("SCALEFORM_MEMORY_FILE_CURSOR_OFFSET", "crates/er-quickload/src/constants/stats_panel_text.rs", 0x24, "Scaleform::MemoryFile"),
    # `CS::ProfileSummary`. These four are `offset_of!`/`size_of!` expressions, so their literal
    # lives in the `const _: () = assert!(NAME == 0x..)` beside them -- which this gate reads as a
    # pin in its own right. The record write reaches the final byte of the allocation exactly, so
    # every one of these is load-bearing.
    ("GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET", "crates/er-game-base/src/profile_summary.rs", 0x78, "CS::GameDataMan"),
    ("PROFILE_SUMMARY_ACTIVE_FLAGS_OFFSET", "crates/er-game-base/src/profile_summary.rs", 0x08, "CS::ProfileSummary"),
    ("PROFILE_SUMMARY_RECORD_BASE", "crates/er-game-base/src/profile_summary.rs", 0x18, "CS::ProfileSummary"),
    ("PROFILE_SUMMARY_RECORD_STRIDE", "crates/er-game-base/src/profile_summary.rs", 0x2A0, "CS::ProfileSummary"),
    ("PROFILE_SUMMARY_TOTAL_BYTES", "crates/er-game-base/src/profile_summary.rs", 0x1A58, "CS::ProfileSummary"),
    ("PROFILE_SUMMARY_SLOT_COUNT", "crates/er-game-base/src/profile_summary.rs", 10, "CS::ProfileSummary"),
    ("SERVER_CONNECTION_ENABLED_BC9_OFFSET", "crates/er-title-flow/src/product_autoload_gates.rs", 0xBC9, "CS::GameMan"),
    ("CS_MENU_MAN_DISABLE_SAVE_MENU_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0x13C, "CS::CSMenuManImp"),
    ("CS_MENU_DATA_ENDING_FLAG_5E_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0x5E, "CS::CSMenuData"),
    ("MOVEMAPSTEP_ADVANCE_GATE_LO_4B8_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0x4B8, "CS::MoveMapStep"),
    ("PROFILE_CAM_FOV_OFFSET", "crates/er-loading-portrait-core/src/portrait_camera.rs", 0xA20, "CSMenuProfModelRend"),
    # The two bounds that keep the virtual-key stamp inside `FD4PadDevice`. They are the reach
    # itself, not a field offset: `0x88 + (VK_ID_MAX - VK_ID_MIN) * 2 + 1`.
    ("VK_ID_MIN", "crates/er-input-harness/src/pad_inject.rs", 1000, "FD4PadDevice"),
    ("VK_ID_MAX", "crates/er-input-harness/src/pad_inject.rs", 1080, "FD4PadDevice"),
    # The analog-stick field the move probe writes. It belongs to `DLUID::PadDevice` (0xa68) and
    # to nothing else in that four-class family, which is what the sweep's vtable test enforces.
    ("PAD_STICK_LY_OFFSET", "crates/er-quickload/src/experiments/can_move_probe.rs", 0x8A0, "DLUID::PadDevice"),
    ("PAD_STICK_LX_OFFSET", "crates/er-quickload/src/experiments/can_move_probe.rs", 0x89C, "DLUID::PadDevice"),
    # The pointer walk that reaches those two floats: FD4PadManager -> padDevices[i] ->
    # FD4PadDevice -> devices[slot] -> DLUID::PadDevice. `er-input-harness` spells the manager's
    # two offsets under different names (pinned above), and this gate looks a name up tree-wide, so
    # the can_move_probe copies were unwatched until 2026-09-01 despite being the base half of a
    # write. The capacity is here for the same reason `VK_ID_MAX` is: it is a bound, and the loop
    # that clamps to it is what keeps the walk inside the game's own DLFixedVector<_,4>.
    ("PAD_MGR_DEVICES_OFFSET", "crates/er-quickload/src/experiments/can_move_probe.rs", 0x18, "FD4PadManager"),
    ("PAD_MGR_DEVICE_COUNT_OFFSET", "crates/er-quickload/src/experiments/can_move_probe.rs", 0x40, "FD4PadManager"),
    ("FD4PADDEVICE_DEVICES_OFFSET", "crates/er-quickload/src/experiments/can_move_probe.rs", 0x10, "FD4PadDevice"),
    ("FD4PADDEVICE_DEVICE_COUNT_OFFSET", "crates/er-quickload/src/experiments/can_move_probe.rs", 0x38, "FD4PadDevice"),
    ("FD4PADDEVICE_DEVICES_CAPACITY", "crates/er-quickload/src/experiments/can_move_probe.rs", 4, "FD4PadDevice"),
    # ---- The name-provenance sweep (2026-08-31) ---------------------------------------------
    # Constants whose only stated provenance was a name. The sweep measured all of them and all of
    # them were right, so nothing here is a correction -- these rows exist so that the next edit
    # to one of them has to produce a witness instead of a declaration. Four of the ChrAsm names
    # have no literal at their definition (`offset_of!` / a layout-walk expression), so their pin
    # is the `const _: () = assert!(NAME == 0x..)` this sweep added beside them, which this gate
    # reads as a definition in its own right.
    ("CHR_ASM_EQUIPMENT_OFFSET", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 0x08, "CS::ChrAsm"),
    ("CHR_ASM_GAITEM_HANDLES_OFFSET", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 0x24, "CS::ChrAsm"),
    ("CHR_ASM_EQUIPMENT_PARAM_IDS_OFFSET", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 0x7C, "CS::ChrAsm"),
    ("CHR_ASM_UNK0_OFFSET", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 0x00, "CS::ChrAsm"),
    ("CHR_ASM_UNKD4_OFFSET", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 0xD4, "CS::ChrAsm"),
    ("CHR_ASM_UNKD8_OFFSET", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 0xD8, "CS::ChrAsm"),
    # Not an offset but the multiplier inside the unkd4 layout walk: 22 is what turns
    # `equipment_param_ids` into 0xd4, so leaving it unpinned would leave the walk free to move.
    ("CHR_ASM_EQUIPMENT_ENTRY_COUNT", "crates/er-loading-portrait-core/src/chr_asm_layout.rs", 22, "CS::ChrAsm"),
    ("FIELD_AREA_WORLD_INFO_OWNER_OFFSET", "crates/er-invasion-warp-core/src/msb_invasion_points.rs", 0x10, "CS::FieldArea"),
    ("FIELD_AREA_WORLD_INFO_OWNER2_OFFSET", "crates/er-invasion-warp-core/src/msb_invasion_points.rs", 0x18, "CS::FieldArea"),
    ("WORLD_BLOCK_INFO_MSB_RES_CAP_OFFSET", "crates/er-invasion-warp-core/src/msb_invasion_points.rs", 0x48, "CS::WorldBlockInfo"),
    # Three crates spell this 0x08 by hand and a fourth reaches the same field through
    # `offset_of!(GameDataMan, main_player_game_data)`; the gate looks the name up tree-wide, so
    # every copy is checked and a drifted one fails even if the others are right.
    ("GAME_DATA_MAN_PLAYER_OFFSET", "crates/er-build-import-runtime/src/grant.rs", 0x08, "CS::GameDataMan"),
    ("GAME_DATA_MAN_PLAYER_GAME_DATA_08_OFFSET", "crates/er-game-base/src/rva.rs", 0x08, "CS::GameDataMan"),
    ("GAME_DATA_MAN_MENU_SAVELOAD_60_OFFSET", "crates/er-title-flow/src/constants_own_load_pump.rs", 0x60, "CS::GameDataMan"),
    ("GAME_MAN_SAVE_STATE_B80_OFFSET", "crates/er-save-suppress/src/save_state_device.rs", 0xB80, "CS::GameMan"),
    ("CS_MENU_MAN_IN_GAME_MENU_JOB_798_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0x798, "CS::CSMenuManImp"),
    ("CS_MENU_MAN_MENU_DATA_OFFSET", "crates/er-game-base/src/rva.rs", 0x08, "CS::CSMenuManImp"),
    # ---- The UNPROVENANCED-offset sweep (2026-08-31) ----------------------------------------
    # `CS::GameMan` carried more hand-spelled offsets with no stated provenance than any other
    # object in the tree -- seventeen, across six crates. The witness rows above measured all of
    # them against the same constructor pairing; these pins stop the literals drifting away from
    # that measurement, wherever in the tree they are spelled.
    ("GAME_MAN_WARP_REQUESTED_10_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0x10, "CS::GameMan"),
    ("GAME_MAN_SAVE_SLOT_AC0_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xAC0, "CS::GameMan"),
    ("GAME_MAN_LOAD_TARGET_MAP_ID_AC8_OFFSET", "crates/er-title-flow/src/constants_return_title.rs", 0xAC8, "CS::GameMan"),
    ("GAME_MAN_SUBMIT_GATE_B5E_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xB5E, "CS::GameMan"),
    ("GAME_MAN_SAVE_REQUEST_B72_OFFSET", "crates/er-quickload/build.rs", 0xB72, "CS::GameMan"),
    ("GAME_MAN_SAVE_REQUESTED_B72_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xB72, "CS::GameMan"),
    ("GAME_MAN_SAVE_REQUEST_B73_OFFSET", "crates/er-quickload/build.rs", 0xB73, "CS::GameMan"),
    ("GAME_MAN_FIELD_B73_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xB73, "CS::GameMan"),
    ("GAME_MAN_SAVE_REQUEST_COMPANION_B73_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0xB73, "CS::GameMan"),
    ("GAME_MAN_REQUESTED_SLOT_B78_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xB78, "CS::GameMan"),
    ("GAME_MAN_ENDING_FLAG_B7D_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0xB7D, "CS::GameMan"),
    ("GAME_MAN_SAVE_STATE_B80_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xB80, "CS::GameMan"),
    ("GAME_MAN_SAVE_STATE_B80_OFFSET", "crates/er-input-harness/src/game_mem.rs", 0xB80, "CS::GameMan"),
    ("GAME_MAN_DLDATETIME_B98_OFFSET", "crates/er-quickload/src/experiments/own_stepper/bootstrap_drive.rs", 0xB98, "CS::GameMan"),
    ("GAME_MAN_DLDATETIME_B98_UPPER_BA0_OFFSET", "crates/er-quickload/src/experiments/own_stepper/bootstrap_drive.rs", 0xBA0, "CS::GameMan"),
    ("GAME_MAN_QUIT_PHASE_OFFSET", "crates/er-save-suppress/build.rs", 0xBC4, "CS::GameMan"),
    ("GAME_MAN_QUIT_PHASE_BC4_OFFSET", "crates/er-save-suppress/src/lib.rs", 0xBC4, "CS::GameMan"),
    ("GAME_MAN_SERVER_CONNECTION_ENABLED_BC9_OFFSET", "crates/er-title-flow/src/constants_return_title.rs", 0xBC9, "CS::GameMan"),
    ("GAME_MAN_EVENT_WORLD_TYPE_BCA_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xBCA, "CS::GameMan"),
    # Pinned 2026-08-31 with the rename off GAME_MAN_LOADING_MODE_BF5_OFFSET: the ctor's
    # two-byte `mov $0x100,0xbf4(%rsi)` is the covering witness row above.
    ("GAME_MAN_LOADING_SCREEN_TEXT_STATE_BF5_OFFSET", "crates/er-title-flow/src/constants_moved.rs", 0xBF5, "CS::GameMan"),
    ("GAME_MAN_STAY_IN_MULTIPLAY_AREA_BLOCK_ID_C30_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xC30, "CS::GameMan"),
    ("GAME_MAN_SUBMIT_GATE_CB1_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xCB1, "CS::GameMan"),
    ("GAME_MAN_SUBMIT_GATE_CB2_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xCB2, "CS::GameMan"),
    ("GAME_MAN_FILE_PATH_STRING_LEN_DF0_OFFSET", "crates/er-reload-trace/src/lib.rs", 0xDF0, "CS::GameMan"),
)

# Every `PlayerGameData` field this workspace reaches through `offset_of!`, with the offset the
# 1.16.2 binding computes for it. A field not in this table fails the gate: that is the whole
# point -- a new field reference must be measured against the images before it may be used, not
# trusted because the compiler was willing to compute it.
#
# The 25 marked `pinned` are additionally const-asserted in crates/er-game-base/src/pgd.rs. The 8
# marked `bracketed` are not const-asserted there and deliberately so: the 1.17 image never
# witnesses their offset, and each is only bracketed one or two slots wide by both-witnessed
# neighbours. A bracket is not a proof -- `CS::PlayerIns` is the counterexample, where a
# compensating insert/remove pair moved the interior of a bracket while both ends held. They are
# admitted here because every one of them is far below 0x960, which is the only claim this gate
# needs to make about them.
PGD_REFERENCED_FIELDS = {
    "current_hp": (0x10, "pinned"),
    "current_max_hp": (0x14, "pinned"),
    "base_max_hp": (0x18, "bracketed"),
    "current_fp": (0x1C, "pinned"),
    "current_max_fp": (0x20, "pinned"),
    "base_max_fp": (0x24, "bracketed"),
    "current_stamina": (0x2C, "pinned"),
    "current_max_stamina": (0x30, "pinned"),
    "base_max_stamina": (0x34, "bracketed"),
    "vigor": (0x3C, "pinned"),
    "mind": (0x40, "pinned"),
    "endurance": (0x44, "pinned"),
    "strength": (0x48, "pinned"),
    "dexterity": (0x4C, "pinned"),
    "intelligence": (0x50, "bracketed"),
    "faith": (0x54, "bracketed"),
    "arcane": (0x58, "bracketed"),
    "base_hero_point": (0x5C, "bracketed"),
    "level": (0x68, "pinned"),
    "rune_count": (0x6C, "pinned"),
    "rune_memory": (0x70, "pinned"),
    "chr_type": (0x98, "pinned"),
    "gender": (0xBE, "pinned"),
    "archetype": (0xBF, "pinned"),
    "voice_type": (0xC2, "pinned"),
    "starting_gift": (0xC3, "pinned"),
    "unlocked_talisman_slots": (0xC6, "pinned"),
    "matchmaking_spirit_ashes_level": (0xC7, "bracketed"),
    "matching_weapon_level": (0xE2, "pinned"),
    "max_hp_flask": (0x101, "pinned"),
    "max_fp_flask": (0x102, "pinned"),
    "equipment": (0x2B0, "pinned"),
    "face_data": (0x760, "pinned"),
}

OFFSET_OF_PGD = re.compile(r"offset_of!\s*\(\s*PlayerGameData\s*,\s*([A-Za-z0-9_]+)")
# Two ways a pinned literal appears in this tree, and both count. A plain definition is the usual
# one; but a constant derived from `offset_of!`/`size_of!` has no literal at its definition and its
# real pin is the `const _: () = assert!(NAME == 0x..)` beside it. Reading only the first form
# would leave every typed-layout constant -- the whole `CS::ProfileSummary` ABI -- unwatched.
CONST_DEF = r"const\s+{name}\s*:\s*[A-Za-z0-9_]+\s*=\s*(0x[0-9a-fA-F]+|[0-9]+)\s*;"
CONST_ASSERT = r"assert!\(\s*{name}\s*==\s*(0x[0-9a-fA-F]+|[0-9]+)\s*[,)]"


_MATCHER = []


def load_matcher(fresh=False):
    """The single alignment implementation, shared with scripts/pair-object-field-drift.py.

    Cached: it holds the two 98 MB images in memory once loaded, and the selftest aligns the
    frozen rows hundreds of times.
    """
    if _MATCHER and not fresh:
        return _MATCHER[0]
    spec = importlib.util.spec_from_file_location("pair_object_field_drift", MATCHER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    if not fresh:
        _MATCHER.append(module)
    return module


def rust_files():
    for root, dirs, files in os.walk(REPO):
        dirs[:] = [d for d in dirs if d not in EXCLUDED_DIRS]
        for name in files:
            if name.endswith(".rs"):
                yield Path(root) / name


def _int_literal(text):
    return int(text, 16) if text.lower().startswith("0x") else int(text, 10)


_TREE_SCAN = []


def _scan_tree(read_text):
    """One pass over every `.rs` file: pinned-constant definitions and `offset_of!` references.

    The definition is looked for everywhere rather than only at its recorded home, because these
    constants are actively being consolidated into `er-game-base::rva` and a gate that goes quiet
    when a constant moves file is a gate that stops watching exactly when someone edits it.

    Cached for the default reader only. `--selftest` runs the source half ~95 times against an
    unchanged tree, and re-walking it each time was 65 of the 82 seconds the selftest took -- past
    the 25s `scripts/audit-selftest-vacuity.py` allows per script, so this gate's selftest could
    not be judged for vacuity at all. That is lost coverage, not a speed preference. A caller that
    passes its own `read_text` is deliberately feeding a tree that is not this one (an empty read,
    a perturbed literal), so those never read or write the cache.
    """
    if read_text is None and _TREE_SCAN:
        return _TREE_SCAN[0]
    read = read_text or (lambda p: p.read_text(encoding="utf-8", errors="replace"))
    definitions = {name: [] for name, _rel, _expected, _obj in PINNED_CONSTANTS}
    patterns = {
        name: (
            re.compile(CONST_DEF.format(name=re.escape(name))),
            re.compile(CONST_ASSERT.format(name=re.escape(name))),
        )
        for name, _rel, _expected, _obj in PINNED_CONSTANTS
    }
    referenced = {}
    for path in rust_files():
        text = read(path)
        where = str(path.relative_to(REPO))
        for name, (definition, assertion) in patterns.items():
            for match in definition.finditer(text):
                definitions[name].append((where, _int_literal(match.group(1))))
            for match in assertion.finditer(text):
                definitions[name].append((where, _int_literal(match.group(1))))
        for match in OFFSET_OF_PGD.finditer(text):
            referenced.setdefault(match.group(1), set()).add(where)
    if read_text is None:
        _TREE_SCAN.append((definitions, referenced))
    return definitions, referenced


def source_findings(read_text=None, reach_rows=WRITE_REACH, alloc_rows=ALLOC_WITNESSES):
    """Constant pins, the reach arithmetic and the `offset_of!` guard. Never touches the images."""
    findings = []
    definitions, referenced = _scan_tree(read_text)
    for name, rel, expected, _obj in PINNED_CONSTANTS:
        found = definitions[name]
        if not found:
            findings.append(
                f"{name}: neither `const {name}: .. = <literal>;` nor `assert!({name} == ..)` "
                f"anywhere in the tree (last seen in {rel}); this gate can no longer watch it"
            )
        for where, value in found:
            if value != expected:
                findings.append(
                    f"{name}: {where} says {value:#x}, this gate verified {expected:#x} against "
                    "both images -- re-measure before changing it"
                )
    findings += reach_findings(definitions, reach_rows=reach_rows, alloc_rows=alloc_rows)
    lo, hi = SAFE_REGIONS["PlayerGameData"][0]
    for field, where in sorted(referenced.items()):
        known = PGD_REFERENCED_FIELDS.get(field)
        if known is None:
            findings.append(
                f"offset_of!(PlayerGameData, {field}) at {sorted(where)[0]} is NOT in this gate's "
                "verified field table. The sibling binding is a 1.16.2 model; measure this field "
                "against both images and add it before reading it at runtime"
            )
            continue
        offset, _how = known
        if not lo <= offset < hi:
            findings.append(
                f"offset_of!(PlayerGameData, {field}) = {offset:#x} is at or above {hi:#x}, where "
                f"1.17 moved the fields. Used by {sorted(where)[0]}"
            )
    return findings, len(referenced)


def reach_findings(definitions, reach_rows=WRITE_REACH, alloc_rows=ALLOC_WITNESSES):
    """Assert the repo's highest write into each object stays inside the measured allocation.

    The reach is recomputed from the literals actually in the tree, so raising a bound (a slot
    count, a virtual-key id ceiling, a field offset) fails here rather than at the far end of a
    `HeapAlloc`. A row whose constants cannot be read is a failure: an unreadable reach is not a
    small reach.
    """
    sizes = {row["obj"]: row["size"] for row in alloc_rows}
    findings = []
    for obj, addend, terms, how in reach_rows:
        size = sizes.get(obj)
        if size is None:
            findings.append(
                f"{obj}: WRITE_REACH claims a reach for an object ALLOC_WITNESSES does not "
                "measure, so the comparison has nothing to be against"
            )
            continue
        reach, blind = addend, None
        for coefficient, name in terms:
            found = definitions.get(name) or []
            values = {value for _where, value in found}
            if len(values) != 1:
                blind = (
                    f"{name} reads as {sorted(values) or 'nothing'} in the tree, so the reach "
                    "cannot be computed -- which is a failure, not a pass"
                )
                break
            reach += coefficient * values.pop()
        if blind:
            findings.append(f"{obj}: {blind}")
            continue
        if reach > size:
            findings.append(
                f"{obj}: this repo's writes reach byte {reach:#x} of an allocation the game makes "
                f"{size:#x} bytes long -- {reach - size:#x} bytes PAST THE END of a live object. "
                f"Reach: {how}"
            )
    return findings


def alloc_findings(matcher, capstone, md, rows=ALLOC_WITNESSES):
    """Re-decode each object's allocation size and its identity anchor from both images.

    Unmeasurable is a failure, exactly as for the field rows: an allocation whose size instruction
    no longer decodes tells you nothing about whether a write fits inside it.
    """
    findings, measured = [], 0
    for row in rows:
        ok = True
        for build, index, path in (("1.16.2", 0, IMAGE_1162), ("1.17", 1, IMAGE_1170)):
            image = matcher.image(str(path))
            start = row["scan"][index]
            body = image[start - IMAGE_BASE : start - IMAGE_BASE + row["scan_len"]]
            decoded = list(md.disasm(body, start))
            at = {insn.address: insn for insn in decoded}
            here = at.get(row["va"][index])
            if here is None:
                ok = False
                findings.append(
                    f"{row['obj']} [{build}]: nothing decodes at {row['va'][index]:#x} from "
                    f"{start:#x} -- the size measurement went blind, which is not a clean result. "
                    f"Witness: {row['how']}"
                )
                continue
            value = _size_literal(capstone, here, row["form"])
            if value is None:
                ok = False
                findings.append(
                    f"{row['obj']} [{build}]: {row['va'][index]:#x} decodes as "
                    f"`{here.mnemonic} {here.op_str}`, not the declared `{row['form']}` carrying "
                    "an allocation size -- went blind"
                )
                continue
            if value != row["size"]:
                ok = False
                findings.append(
                    f"{row['obj']} [{build}]: the allocation at {row['va'][index]:#x} is "
                    f"{value:#x} bytes, this gate is frozen at {row['size']:#x}. Every write "
                    "bound for this object was computed against the frozen number"
                )
                continue
            if not any(_anchors(capstone, insn, row["ident_kind"], row["ident"][index]) for insn in decoded):
                ok = False
                findings.append(
                    f"{row['obj']} [{build}]: no {row['ident_kind']} to {row['ident'][index]:#x} "
                    f"within {row['scan_len']:#x} bytes of {start:#x}, so the size at "
                    f"{row['va'][index]:#x} is no longer tied to this class -- a size without an "
                    "identity is just a number at an address"
                )
        if ok:
            measured += 1
    return findings, measured


def _size_literal(capstone, insn, form):
    """The allocation size an instruction carries, or `None` if it is not that shape at all."""
    if len(insn.operands) != 2:
        return None
    destination, source = insn.operands
    if destination.type != capstone.x86.X86_OP_REG:
        return None
    if form == ALLOC_IMM:
        if insn.mnemonic != "mov" or source.type != capstone.x86.X86_OP_IMM:
            return None
        return source.imm
    if form == ALLOC_DISP:
        # `lea 0x30(%r12),%edx` with r12 held at zero -- MSVC's way of passing a small constant
        # when a register is already known to be zero. Refuse an indexed form: that would be real
        # address arithmetic rather than a constant in disguise.
        if insn.mnemonic != "lea" or source.type != capstone.x86.X86_OP_MEM:
            return None
        if source.mem.index != 0:
            return None
        return source.mem.disp
    return None


def _anchors(capstone, insn, kind, target):
    """Whether `insn` ties its window to `target` -- a call to it, or a rip-relative reference."""
    if kind == IDENT_CALL:
        return insn.mnemonic == "call" and any(
            operand.type == capstone.x86.X86_OP_IMM and operand.imm == target
            for operand in insn.operands
        )
    if kind == IDENT_RIP:
        return any(
            operand.type == capstone.x86.X86_OP_MEM
            and insn.reg_name(operand.mem.base) == "rip"
            and insn.address + insn.size + operand.mem.disp == target
            for operand in insn.operands
        )
    return False


_ALIGNMENTS = {}


def _aligned(matcher, capstone, md, witness, label):
    """`matcher.compare` for one witness, memoised on (implementation, witness).

    An alignment is a pure function of the two frozen images and the witness, and several rows
    share a witness (four on the step-template constructor alone) while `--selftest` re-runs the
    whole set once per perturbation -- so the same alignment was being recomputed ~95 times. That
    is where the selftest's time went, and it mattered beyond patience: the blinded replay in
    `scripts/audit-selftest-vacuity.py` costs a `sys._getframe` + `abspath` on every one of the
    900k regex calls underneath, which pushed it past that tool's 25s budget and made this gate
    UNMEASURABLE for vacuity.

    The key carries the bound `compare` itself, not just the witness. The selftest's final control
    hands in a matcher whose `compare` returns nothing and requires the result to be reported as a
    failure; a cache keyed on the witness alone would serve it the real matcher's answer and that
    control would go quietly green -- the gate's own vacuity, introduced by its speed-up.
    """
    key = (
        matcher.compare,
        witness["va16"],
        witness["len16"],
        witness["va17"],
        witness["len17"],
        witness["bases"],
    )
    if key not in _ALIGNMENTS:
        _ALIGNMENTS[key] = matcher.compare(
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
    return _ALIGNMENTS[key]


def image_findings(matcher, capstone, md, rows=WITNESSES):
    """Re-measure every frozen row from the two images. Unmeasurable == failure."""
    findings, measured = [], 0
    for obj, label, old, new, witness, how in rows:
        pairs, _ins, _del, _rep = _aligned(matcher, capstone, md, witness, label)
        seen = {o: n for o, n, _a, _b, _t in pairs}
        if old not in seen:
            findings.append(
                f"{obj}::{label}: witness ({how}) no longer reads {old:#x} at all -- the "
                "measurement went blind, which is not the same as a clean result"
            )
            continue
        measured += 1
        if seen[old] != new:
            findings.append(
                f"{obj}::{label}: {old:#x} -> {seen[old]:#x} measured, but this gate is frozen at "
                f"{old:#x} -> {new:#x}. Witness: {how}"
            )
    for obj, regions in SAFE_REGIONS.items():
        for _o, label, old, new, _w, _h in rows:
            if _o != obj or old == new:
                continue
            if any(lo <= old < hi for lo, hi in regions):
                findings.append(
                    f"{obj}::{label}: {old:#x} moved, yet {old:#x} is inside a region this gate "
                    "calls safe. SAFE_REGIONS and the witnesses disagree"
                )
    return findings, measured


def images_present():
    return IMAGE_1162.exists() and IMAGE_1170.exists()


def run(quiet=False, rows=WITNESSES, read_text=None, alloc_rows=ALLOC_WITNESSES, reach_rows=WRITE_REACH):
    findings, referenced = source_findings(
        read_text=read_text, reach_rows=reach_rows, alloc_rows=alloc_rows
    )
    if not quiet:
        print(
            f"source: {len(PINNED_CONSTANTS)} constant pins, {referenced} PlayerGameData fields "
            f"referenced, {len(reach_rows)} write-reach bounds recomputed from the tree's literals"
        )
    measured = 0
    if images_present():
        matcher = load_matcher()
        capstone, md = matcher._capstone()
        image, measured = image_findings(matcher, capstone, md, rows=rows)
        findings += image
        alloc, alloc_measured = alloc_findings(matcher, capstone, md, rows=alloc_rows)
        findings += alloc
        if not quiet:
            print(f"image:  {measured}/{len(rows)} frozen witness rows re-measured from both images")
            print(
                f"object: {alloc_measured}/{len(alloc_rows)} allocation sizes re-decoded from both "
                "images, each tied to its class by a constructor call or a vtable reference"
            )
    elif not quiet:
        print(f"image:  SKIPPED -- {IMAGE_1162.name} / {IMAGE_1170.name} absent (gitignored)")
    return findings, measured


def _selftest_mutants(matcher):
    """Perturbations that must make the gate red. A gate that survives them proves nothing."""
    cases = []
    for index, row in enumerate(WITNESSES):
        obj, label, old, new, witness, how = row
        # A moved row perturbed by another +4, and a held row perturbed to old+8. The second is
        # the frozen negative: a matcher that reported everything as moved would still pass the
        # first, and fails this one.
        bad = new + 4 if old != new else old + 8
        mutant = list(WITNESSES)
        mutant[index] = (obj, label, old, bad, witness, how)
        kind = "moved" if old != new else "HELD (frozen negative)"
        cases.append((f"{obj}::{label} [{kind}] expected {bad:#x}", tuple(mutant)))
    return cases


def _selftest_alloc_mutants():
    """Perturbations of the object rows. Four kinds, because they fail four different ways.

    A size row that survives a changed size is measuring nothing. One that survives a changed
    identity is measuring an address rather than a class -- which is exactly the mistake that put
    a write into a 152-byte object. One that survives its 1.17 witness being left at the 1.16.2
    address has not looked at 1.17 at all. And one that survives its witness being pointed four
    bytes into the body is decoding garbage and calling it clean.
    """
    cases = []
    for index, row in enumerate(ALLOC_WITNESSES):
        def variant(**changes):
            mutant = list(ALLOC_WITNESSES)
            copy = dict(row)
            copy.update(changes)
            mutant[index] = copy
            return tuple(mutant)

        name = row["obj"]
        cases.append((f"{name}: size {row['size']:#x} -> {row['size'] + 8:#x}", variant(size=row["size"] + 8)))
        cases.append(
            (
                f"{name}: identity anchor moved off the class",
                variant(ident=(row["ident"][0] + 0x10, row["ident"][1] + 0x10)),
            )
        )
        # The 1.17 witness left at its 1.16.2 address. `scan` moves with it: a row that only
        # relocated the size instruction would still decode the 1.16.2 window and look fine.
        # Skipped where the two builds genuinely allocate at the same address (CS::CSInGamePad
        # does), because there the mutant is the unmutated row and a green result would be
        # correct rather than vacuous -- the identity anchor is what carries that row.
        if row["va"][0] != row["va"][1]:
            cases.append(
                (
                    f"{name}: 1.17 witness left at the 1.16.2 address",
                    variant(va=(row["va"][0], row["va"][0]), scan=(row["scan"][0], row["scan"][0])),
                )
            )
        cases.append(
            (
                f"{name}: 1.17 witness pointed 4 bytes into the body",
                variant(va=(row["va"][0], row["va"][1] + 4)),
            )
        )
    return cases


def _selftest_reach_mutants():
    """A reach row must go red when the repo's own bound is raised past the allocation.

    The last two cases are not synthetic. They are the two bugs this table was built after,
    restated as rows: point the virtual-key stamp at `CS::CSInGamePad` (which is what the code did
    from 2026-07-23), or point the analog-stick write at `DLUID::VirtualMultiDevice` (which is what
    `can_move_probe` did until 2026-08-31), and the gate must say so. If either of these ever goes
    green again, this whole table has stopped meaning anything.
    """
    sizes = {row["obj"]: row["size"] for row in ALLOC_WITNESSES}
    cases = []
    for index, (obj, addend, terms, how) in enumerate(WRITE_REACH):
        mutant = list(WRITE_REACH)
        mutant[index] = (obj, addend + sizes[obj], terms, how)
        cases.append((f"{obj}: reach raised past the allocation", tuple(mutant)))
    historical = {
        "FD4::FD4PadDevice": ("CS::CSInGamePad", "the 2026-07-23 virtual-key stamp"),
        "DLUID::PadDevice": ("DLUID::VirtualMultiDevice", "the 2026-08-31 analog-stick sweep"),
    }
    for index, (obj, addend, terms, how) in enumerate(WRITE_REACH):
        if obj not in historical:
            continue
        wrong, what = historical[obj]
        mutant = list(WRITE_REACH)
        mutant[index] = (wrong, addend, terms, how)
        cases.append((f"{what}, re-aimed at {wrong} ({sizes[wrong]:#x} bytes)", tuple(mutant)))
    return cases


def selftest():
    if not images_present():
        print(
            "SELFTEST CANNOT RUN: this gate's whole claim is that it re-measures the images, so a "
            f"green selftest without {IMAGE_1162.name} / {IMAGE_1170.name} would be the exact "
            "vacuity it exists to prevent"
        )
        return 1
    failures = []
    findings, measured = run(quiet=True)
    if findings:
        failures.append(f"the unmutated tree is already red: {findings[0]}")
    if measured != len(WITNESSES):
        failures.append(f"only {measured}/{len(WITNESSES)} witness rows measured on a clean run")

    matcher = load_matcher()
    for name, mutant in _selftest_mutants(matcher):
        mutant_findings, _ = run(quiet=True, rows=mutant)
        if not mutant_findings:
            failures.append(f"mutant survived: {name}")
    for name, mutant in _selftest_alloc_mutants():
        mutant_findings, _ = run(quiet=True, alloc_rows=mutant)
        if not mutant_findings:
            failures.append(f"object mutant survived: {name}")
    for name, mutant in _selftest_reach_mutants():
        mutant_findings, _ = run(quiet=True, reach_rows=mutant)
        if not mutant_findings:
            failures.append(f"reach mutant survived: {name}")

    # A lobotomised matcher must not read as clean.
    real_compare = matcher.compare
    try:
        blind = load_matcher(fresh=True)
        blind.compare = lambda *a, **k: ([], [], [], [])
        capstone, md = blind._capstone()
        blind_findings, blind_measured = image_findings(blind, capstone, md)
        if not blind_findings or blind_measured:
            failures.append("a matcher that measures nothing was not reported as a failure")
    finally:
        matcher.compare = real_compare

    # A source read that returns nothing must not read as clean either.
    blind_source, blind_referenced = source_findings(read_text=lambda _p: "")
    if not blind_source or blind_referenced:
        failures.append("a source half that reads empty files was not reported as a failure")

    # And a perturbed constant must be caught where it actually lives.
    name, rel, expected, _obj = PINNED_CONSTANTS[0]
    def swapped(path):
        text = path.read_text(encoding="utf-8", errors="replace")
        return text.replace(f"{expected:#x}", f"{expected + 8:#x}") if str(path).endswith(rel) else text
    perturbed, _ = source_findings(read_text=swapped)
    if not any(name in f for f in perturbed):
        failures.append(f"a changed literal for {name} was not caught")

    if failures:
        for line in failures:
            print(f"SELFTEST FAILED: {line}")
        return 1
    print(
        f"selftest ok: {len(WITNESSES)} field rows and {len(ALLOC_WITNESSES)} allocation rows "
        f"re-measure clean; each of {len(_selftest_mutants(matcher))} field perturbations, "
        f"{len(_selftest_alloc_mutants())} object perturbations (size, identity, 1.17 witness left "
        f"behind, 1.17 witness 4 bytes in) and {len(_selftest_reach_mutants())} raised write "
        "bounds goes red, and a blind matcher, an empty source read and a changed constant literal "
        "are all reported"
    )
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    findings, _measured = run()
    if findings:
        print(f"\n{len(findings)} FINDING(S):")
        for line in findings:
            print(f"  * {line}")
        return 1
    print("ok: no repo constant sits on a field 1.17 moved, and every witness still measures")
    return 0


if __name__ == "__main__":
    sys.exit(main())
