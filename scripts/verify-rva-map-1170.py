#!/usr/bin/env python3
"""Judge whether a mapped 1.17 address is really the same function as its 1.16.2 original.

`map-rvas-1162-to-1170.py` finds where a signature RE-occurs. That is where the evidence stops:
the signature is short and its operands are wildcarded, so a match proves the opening bytes have
the same shape, not that the function still does the same job. This script asks the follow-up
question by decoding much further into both functions and comparing them instruction by
instruction, normalised the same way the matcher masks: mnemonic plus register operands, with
displacements, immediates and branch targets dropped.

That normalisation is the point. ELDEN RING 1.17's dominant change is layout drift -- a struct
grew, so `[rcx+0xab5]` became `[rcx+0xabd]` -- and a function whose every instruction is identical
except for such displacements is the same function. A function that has gained a branch, lost a
call, or reordered its body is not, whatever its prologue says.

A verdict is evidence for a human, not permission. `IDENTICAL` over a long body is strong;
`DIVERGES` names the first instruction index where the two disagree so the reader knows where to
look; a short body is reported as short, because 6 matching instructions is not a lot of evidence
no matter how clean the ratio looks.

VERDICTS, and the one distinction the vocabulary was missing
    The verdicts split on a question that had no word for it until 2026-08-30: was the comparison
    a prefix of the two bodies, or all of them?

    A prefix match stops for a reason that has nothing to do with where the function ends -- the
    instruction limit, a `ret` in a body that continues past it -- so it says nothing whatsoever
    about the instruction after the last one compared. `MIN_VERIFIED_INSNS` in
    `er-game-base/build.rs` exists to put a floor under that ignorance.

    An exhaustive match has no instruction after the last one. It is not a longer prefix; it is a
    different kind of claim, and it also asserts the two bodies are the same length, which no
    prefix can. Both failure modes the floor produced follow from conflating the two:

      * a leaf of 3-11 instructions verifies at ratio 1.000 over its entire body and is thrown
        away for being shorter than 12. Five were: 0x67a810 (the GameMan save-slot setter),
        0x67a980 (er-save-suppress's quit-phase settle), 0xd4cc50 (GET_PARAM_RESCAP), 0x26634a0
        (er-input-harness's DLUID writer) and 0x4f9940 (the SpecialEffect null-container guard,
        which is why that Seamless crash fix stayed refused on 1.17);
      * `STEP_MoveMap` matched over 120 instructions of 975, scored `IDENTICAL 1.000`, cleared the
        floor easily and was promoted to detour-safe -- while the two instructions 1.17 actually
        inserted sat at index 873, and both images' `.pdata` declared extents differing by 8
        bytes, which nobody was reading.

    So: `BYTE-IDENTICAL` (whole declared body, byte for byte), `IDENTICAL-WHOLE` (whole declared
    body, normalised), `IDENTICAL-LEAF` (whole DECODED body of a function with no `.pdata` entry
    in either image) are exhaustive and take no floor. `IDENTICAL` is a prefix and keeps the
    floor. `IDENTICAL-PREFIX` is a prefix that ran out of budget and is accepted nowhere.
    `IDENTICAL-SHORT`, `NEAR`, `DIVERGES` and `UNDECODABLE` are unchanged.

    And the other distinction the vocabulary was missing, added the same day: a body that grew,
    somewhere a detour never reaches. `PATCH-SITE-IDENTICAL` is that answer. It is not a member of
    the exhaustive family -- those assert the two streams are equal and this one asserts they are
    not -- and it is not a weaker `NEAR`. It is a claim about the patch SITE: both images declare
    a function starting at the two addresses, the comparison covered both bodies in full, MinHook
    builds a trampoline at each and consumes the same instructions doing it, nothing branches into
    the bytes its JMP overwrites, and every instruction the two bodies disagree about lies strictly
    after the last one it relocates. `STEP_MoveMap` is why: 1.17 inserts two instructions at index
    873 of 975, its `.pdata` extent grows by 8 bytes, and the first eight bytes of its prologue are
    identical. Refusing that detour cost the autoload gate-hold on 1.17. See `patch_site_drift`.

    And the mirror image of that one, added the same day: a body proved and still un-hookable.
    `IDENTICAL-LEAF-NOPATCH` is an `IDENTICAL-LEAF` whose 3-byte body has nowhere to put a 5-byte
    jump, with MinHook's own ported rules refusing the site in both images rather than this file's
    arithmetic saying so. It is admitted to the call map and to nothing else. It exists because
    those were one decision until 2026-08-30: such a leaf reported `IDENTICAL-SHORT`, and
    `build.rs` seeds the call map from `detourable_pairs`, so a refusal about hooking withdrew the
    address from comparing too -- and `0x7add70` (`CS::MenuItem`'s constant-false accept
    predicate) and `0x1c92f30` (`CTRL_SUBOBJECT_RELEASE_RVA`) were lost to their features on that
    technicality. See `leaf_verdict`.

Usage
    uv run --with capstone python3 scripts/verify-rva-map-1170.py            # whole table
    uv run --with capstone python3 scripts/verify-rva-map-1170.py 0x1407ada40
    uv run --with capstone python3 scripts/verify-rva-map-1170.py --tsv <out> --min-ratio 0.98

    There is one behaviour and no flag that selects it. Leaf extents are always derived (see the
    comment in `main`); `--leaf-extents` is accepted and ignored so an older written-down command
    still runs. It was an opt-in until 2026-08-30, which meant three correct rows of the ledger
    `er-game-base/build.rs` reads held their verdict only because somebody remembered to type it,
    and the next regeneration that forgot would have deleted them at exit 0.

`--tsv` TRUNCATES its target. It writes what this run verified and nothing else, so pointing it
at `docs/recon/rva-map-1162-to-1170.verified.tsv` -- the ledger hand-derived pairs are put in
because nothing regenerates it -- is a rewrite, not an update: 65 of that file's 99 addresses do
not come from the default candidate map and would not come back. `preserve_unverified` now
carries such rows forward verbatim under a `# CARRIED FORWARD` banner and names every one on
stderr, and refuses the write outright if a row's pair disagrees with this run's. A row still
leaves the file the moment a human deletes its line; nothing here restores it.
"""

import argparse
import difflib
import importlib.util
import os
import subprocess
import sys

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

OLD_IMAGE = _deobf_image("ER_DEOBF_BIN", "eldenring-deobf.bin")
NEW_IMAGE = _deobf_image("ER_DEOBF_BIN_1170", "eldenring-deobf-1.17.bin")
MAP_TSV = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.tsv")
BASE = 0x140000000
# Instructions decoded per side. Long enough to run past a prologue into the body that actually
# distinguishes two similar functions, short enough that one wrong decode does not cascade.
DECODE_LIMIT = 120
# Bytes handed to the decoder to reach that many instructions.
DECODE_BYTES = 0x400
# Below this many compared instructions the verdict is reported as thin evidence regardless of
# how well it matched -- several ELDEN RING getters are 6 instructions long and identical.
#
# It is a proxy, and only ever was one. `IDENTICAL over N instructions` is a claim about a PREFIX:
# the decode stopped for a reason unrelated to where the function ends, so nothing is known about
# instruction N+1, and the floor stands in for "enough of the body was seen to be worth
# something". Where the comparison is exhaustive the proxy has nothing left to do, which is what
# IDENTICAL_WHOLE and IDENTICAL_LEAF below are for.
THIN_EVIDENCE = 12
# Bytes MinHook relocates when it installs a detour. `branch_into_prologue` checks this window.
PATCH_BYTES = 5
# Marks the writer's own carried forward banner inside the file body. `preserve_unverified`
# needs to tell that block apart from hand-written prose so a re-run does not nest a second
# copy of it; the writer emits this exact text.
CARRIED_BANNER_MARK = "# CARRIED FORWARD --"

# The two exhaustive VERDICTS. Both mean: the comparison covered every instruction of both
# functions, the two bodies are the same length, and the normalised streams are equal. That is
# strictly stronger evidence than any prefix match of any length, because it also asserts there is
# no instruction the comparison did not see -- a claim `IDENTICAL` cannot make at 12 instructions
# or at 120. `er-game-base/build.rs` therefore waives `MIN_VERIFIED_INSNS` for them, exactly as it
# already does for `BYTE-IDENTICAL`, and for the same reason: a 15-byte function proved over all
# 15 bytes is proved harder than a 4-KB one sampled over its first 120 instructions.
#
# They are two names rather than one because the two claims rest on different foundations, and a
# reader who later doubts the weaker one needs to be able to find exactly the rows that used it.
IDENTICAL_WHOLE = "IDENTICAL-WHOLE"
# ...the end of the body came from each image's own `.pdata`. Nothing to doubt.
IDENTICAL_LEAF = "IDENTICAL-LEAF"
# ...neither image declares a `.pdata` entry -- the x64 ABI omits unwind data for a function that
# allocates no stack and calls nothing -- so the end was DECODED by `leaf_extent`. Three
# independent facts back that decode: it stopped on a real terminator past every forward branch
# target, the two images were decoded separately and arrived at the same byte length, and the
# normalised streams agree over all of it. It additionally carries the relocation check in
# `branch_into_prologue`, which is the claim `.pdata` would have made and cannot here.
IDENTICAL_PREFIX = "IDENTICAL-PREFIX"
# ...and the counterweight: the streams matched, but a decode ran out of budget instead of
# reaching an end, so this is a prefix of unknown coverage. Nothing accepts it. It exists so that
# a truncated comparison can never again be written into a table as plain `IDENTICAL`, which is
# how `STEP_MoveMap` was promoted to detour-safe on 120 of its 975 instructions.

# The verdicts that compared everything there was, and so need no instruction floor. Kept beside
# the constants rather than spelled out at each use, because `er-game-base/build.rs` holds the
# matching list and the two drifting apart is how a rescued row would quietly go back to being
# discarded.
EXHAUSTIVE_VERDICTS = frozenset(("BYTE-IDENTICAL", IDENTICAL_WHOLE, IDENTICAL_LEAF))

# The one verdict that takes an address away. `er-game-base/build.rs::refuted_sources()` keys on
# this literal string and subtracts the row from `VERIFIED_1162_TO_1170` -- the call map, not just
# the detour map. Every other unhappy verdict merely fails to add a row; this one removes one that
# was already there, and it does so with no log line.
#
# That asymmetry is why it is a named constant rather than a string typed at each use. A missing
# address costs a feature loudly (`failed to resolve`); a wrongly refuted one deletes a working
# address silently. `refutation_withheld` below is the rule that follows from it.
REFUTED = "DIVERGES"

# The verdict for a body that grew somewhere else. Not a member of EXHAUSTIVE_VERDICTS: those
# three assert the two instruction streams are equal, and this one asserts they are not.
#
# What it claims, and it claims nothing else: the two bodies were compared in full, both images'
# own `.pdata` declare a function to start at these two addresses, MinHook will build a trampoline
# at each of them, it consumes the same instructions at both, and every instruction the two bodies
# disagree about lies strictly after the last instruction MinHook relocates. The detour therefore
# overwrites the same prologue it overwrote on 1.16.2 and the trampoline returns into the same
# instruction; what changed is somewhere the hook never touches.
#
# Why this is not "RE-CHECK THE PROLOGUE AND SHIP IT". A prologue re-check is what the impostor at
# 1.16.2 `0x140aec480` would have passed: it verified `IDENTICAL 1.000` over 56 instructions while
# sitting `+0x360` inside another function. Three of the six clauses exist for that address alone
# -- it is not a `.pdata` start in either image, so the entry clause refuses it before any byte is
# compared -- and the whole-body clauses are what stop a long agreeing prefix from being mistaken
# for evidence about the rest. Volume of agreement is not confidence; coverage is.
PATCH_SITE_IDENTICAL = "PATCH-SITE-IDENTICAL"
# How many separate places the two bodies may disagree, and how many instructions may be inserted
# or deleted across all of them, before the difference stops being a localised edit.
#
# Set from the image, not from the row that needed rescuing. Surveyed on 2026-08-30 across all
# 128,602 pairs in `docs/recon/rva-map-1162-to-1170.functions.tsv`: 199 pairs differ at all once
# byte-identical and stream-identical bodies are removed, and 26 of those differ by insertions and
# deletions only. Their hunk counts are 1 (20 pairs), 2 (5) and 4 (1); their total inserted-plus-
# deleted instruction counts are 1, 2, 3, 6, 7, 8 -- and then 21, 30, 92. Both ceilings are placed
# at the top of that cluster, where the data has a real gap, rather than at the one row this
# verdict was written for (`STEP_MoveMap`, 1 hunk of 2 instructions).
#
# These are a policy line, and deliberately a tight one: a body that gained thirty instructions is
# not a body that grew "somewhere else", and refusing it costs a feature while admitting it wrongly
# costs five bytes written into a live function. A row above the line is not refused forever -- it
# is referred to a human, who can derive the pair by hand into the curated ledger.
MAX_DRIFT_HUNKS = 2
MAX_DRIFT_INSNS = 8

# The verdict for a body that is proved and still cannot be hooked. The mirror image of
# patch-site-IDENTICAL: that one says the streams differ and licenses the detour anyway; this one
# says the streams are equal over the whole of both bodies and refuses the detour anyway. Neither
# belongs in EXHAUSTIVE_VERDICTS, for opposite halves of the same reason -- that list is what
# `er-game-base/build.rs` admits to the detour table, and this verdict must never reach it.
#
# What it CLAIMS: everything identical-leaf claims, minus the hook. Both extents were DECODED
# because neither image declares a `.pdata` entry, the two decodes arrived at the same byte
# length, the normalised streams are equal over all of both bodies, and no branch inside either
# body targets the bytes a patch would overwrite. What it adds is the refusal, in MinHook's own
# words: the body is shorter than the five bytes `MH_CreateHook` writes, and the ported
# `CreateTrampolineFunction` confirms it will not install there.
#
# Why the refusal had to become a verdict rather than stay a FALLBACK. Before 2026-08-30 such a
# leaf fell back to `IDENTICAL`/`IDENTICAL-SHORT`, and `leaf_fits_patch` described that as keeping
# the row CALLABLE and refusing only the detour. It did not: `build.rs` seeds the call map from
# `detourable_pairs(VERIFIED_MAP)`, so the two decisions were one, and `IDENTICAL-SHORT` withdrew
# the address from comparing as well as from hooking. Two rows were losing a feature that way:
#
#   * `0x7add70 -> 0x7aebf0`, `CS::MenuItem`'s constant-false accept predicate (`33 c0 c3`,
#     `xor eax,eax; ret`). er-quickload only ever compares a row's `+0xf8` against it;
#   * `0x1c92f30 -> 0x1c94d30`, `CTRL_SUBOBJECT_RELEASE_RVA` (`c2 00 00`, `ret 0`), which
#     er-invasion-path calls while tearing an effect down -- and its `let ... else { return }`
#     abandons the whole teardown when the address will not resolve.
#
# Both are 3-byte bodies. A 3-byte function is a perfectly good thing to call and to compare
# against; it is nowhere to put a 5-byte jump. The two claims are now separate.
IDENTICAL_LEAF_NOPATCH = "IDENTICAL-LEAF-NOPATCH"
# Verdicts admitted to `VERIFIED_1162_TO_1170` (call and read) and to nothing else. Mirrored as
# `CALLABLE_ONLY_VERDICTS` in `er-game-base/build.rs`; `build_rs_lists_agree` asserts the two
# files' three lists are identical and that this one is disjoint from the other two, so the
# separation is checked rather than remembered.
CALLABLE_ONLY_VERDICTS = frozenset((IDENTICAL_LEAF_NOPATCH,))
# Entry-evidence verdicts, written into the table's last column and required by
# er-game-base/build.rs before a row may carry a detour.
ENTRY_BOTH = "BOTH-ENTRIES"
ENTRY_DEST_NOT = "DEST-NOT-ENTRY"
ENTRY_SRC_NOT = "SRC-NOT-ENTRY"
ENTRY_NEITHER = "NEITHER-ENTRY"


def runtime_functions(image):
    """Every `RUNTIME_FUNCTION` in the image's exception directory: `(begin, end, unwind)` RVAs.

    x86-64 PE stores one of these per function region for stack unwinding. Regions, not functions
    -- which is the distinction the rest of this file is built on and which cost two rows before
    anyone read the `unwind` field.
    """
    import struct

    e_lfanew = struct.unpack_from("<I", image, 0x3C)[0]
    magic = struct.unpack_from("<H", image, e_lfanew + 24)[0]
    # PE32+ optional header is 112 bytes before the data directories; PE32 is 96.
    directories = e_lfanew + 24 + (112 if magic == 0x20B else 96)
    # Data directory 3 is IMAGE_DIRECTORY_ENTRY_EXCEPTION.
    table_rva, table_size = struct.unpack_from("<II", image, directories + 3 * 8)
    out = []
    for offset in range(table_rva, table_rva + table_size, 12):
        begin, end, unwind = struct.unpack_from("<III", image, offset)
        if begin or end:
            out.append((begin, end, unwind))
    return out


# `UNW_FLAG_CHAININFO` in `UNWIND_INFO`. Byte 0 packs `Version` in bits 0-2 and `Flags` in bits
# 3-7; the flag says "this region is a continuation of another function, and my unwind data is the
# other one's". It is the image's own statement that a `.pdata` entry is not a function.
UNWIND_CHAINED = 0x4


def unwind_is_chained(image, unwind_rva):
    if unwind_rva <= 0 or unwind_rva >= len(image):
        return False
    return bool(((image[unwind_rva] >> 3) & 0x1F) & UNWIND_CHAINED)


def function_regions(image):
    """`({begin: end of the whole chunk run}, {every primary begin})`, chunk runs merged.

    MSVC splits a function into chunks and gives each chunk its own `.pdata` entry. Only the
    first is a real function start; the rest carry `UNW_FLAG_CHAININFO`, which points their
    unwind data back at the primary and marks them as continuations. Reading the table without
    that flag produces two errors at once, and this file was making both:

      * the extent of a chunked function is the first chunk only, which is typically a couple of
        dozen bytes. Measured: `0x140afbad0` (er-reload-trace's `movemap_do_save_stuff`) declares
        0x16 bytes where the real run is 0x25c, and `0x1411a8900` (`SCALEFORM_HANDLER_DTOR_RVA`,
        the System>Quit ownership guard) declares 0x20 where the run is 0xe5. Both then compared
        6 and 7 instructions, reported `IDENTICAL` with `whole_body` true, and failed
        `MIN_VERIFIED_INSNS` -- a confident verdict over 18% and 13% of the body that was also
        not enough to be accepted. Over the full run they are 33 and 54 instructions, ratio
        1.0000, and both clear the floor with nothing relaxed;
      * a continuation chunk's begin is not a function start, but `function_starts` was returning
        it as one. That is the exact input `entry_evidence` must not be given: it would report
        `BOTH-ENTRIES` for a pair of addresses sitting in the middle of a function, which is the
        one thing that column exists to refuse.

    The run is extended only while the next entry is both contiguous with the current end and
    flagged as a continuation. Contiguity alone is not enough -- two unrelated functions packed
    flush against each other satisfy it, and merging them would over-extend the body, which is
    the mirror-image mistake.
    """
    entries = sorted(runtime_functions(image))
    chained = [unwind_is_chained(image, unwind) for _, _, unwind in entries]
    extents, starts = {}, set()
    for index, (begin, end, _unwind) in enumerate(entries):
        if chained[index]:
            continue
        run_end = end
        follower = index + 1
        while (
            follower < len(entries)
            and chained[follower]
            and entries[follower][0] == run_end
        ):
            run_end = entries[follower][1]
            follower += 1
        extents[begin] = run_end
        starts.add(begin)
    return extents, starts


def function_extents(image):
    """`{begin RVA: end RVA}` for every function the image's .pdata declares.

    The end is what makes a short function verifiable. Without it `compare` stops at the first
    `ret` and reports how many instructions it managed, which reads as thin evidence -- but a
    21-byte function compared over all 21 bytes has been compared completely, and calling that
    thin confuses "few instructions" with "not much of the function". The extent is how the two
    are told apart.
    """
    return function_regions(image)[0]


def function_starts(image):
    """Every address the image itself declares as a function start, from its .pdata.

    x86-64 PE stores one RUNTIME_FUNCTION per function in the exception directory: begin RVA,
    end RVA, unwind info. That table is the image's own answer to "does a function start here",
    written by the linker for stack unwinding, and it is not a heuristic the way counting
    forward references is. A detour needs its target to be a function entry -- MinHook relocates
    the first five bytes and they had better be a prologue -- so this is the check that
    `scripts/audit-1170-hook-targets.py` was approximating.

    What it does not say: that the function is the same function. That is what `compare` is for,
    and the two are required together.

    A continuation chunk is not a start. MSVC splits a function into chunks and gives each its
    own `.pdata` entry, so a table read naively answers "yes, a function starts here" about
    addresses that are the middle of one. Those entries are excluded by `function_regions`, which
    reads the `UNW_FLAG_CHAININFO` bit the linker set to say so.
    """
    return function_regions(image)[1]


def branch_target(insn):
    """The rel8/rel32 destination of a branch, or `None` for anything that is not one.

    Only a branch whose destination is encoded as an immediate has a destination this file can
    see. `jmp rax` and `jmp qword ptr [rax*8 + table]` -- a switch dispatch -- do not, and that
    distinction is load-bearing rather than pedantic: a switch's cases sit immediately after the
    dispatch and are reached only through a table in `.rdata`, so treating an indirect `jmp` as a
    visible branch would let the boundary rules below conclude that nothing follows it and
    truncate the function at its own switch.
    """
    from capstone.x86 import X86_OP_IMM

    if not insn.mnemonic.startswith("j"):
        return None
    operands = insn.operands
    if operands and operands[0].type == X86_OP_IMM:
        return operands[0].imm
    return None


def leaf_extent(image, va, starts, limit=0x100):
    """Where a `.pdata`-less leaf function ends, decoded rather than declared.

    The x64 ABI lets a function omit unwind data when it allocates no stack and calls nothing, so
    ELDEN RING's many small getters have no `.pdata` entry at all. `compare` therefore cannot
    obtain a whole-body extent for them, falls back to the normalised decode, and -- because the
    body is a handful of instructions -- reports `IDENTICAL-SHORT`. `er-game-base/build.rs` then
    drops the row: it accepts `BYTE-IDENTICAL`, or `IDENTICAL` over at least `MIN_VERIFIED_INSNS`,
    and a leaf can reach neither however completely it was compared. The address stays unmapped and
    the feature behind it dies quietly, which is the exact outcome this migration exists to
    prevent.

    The first `ret` is not the end. Stopping there -- the obvious rule, and the one tried first --
    gets several of these wrong, because a getter with a range check is `cmp / ja / <compute> / ret
    / xor eax,eax / ret`: the early `ret` is the fast path and the branch target sits behind it.
    Measured against the 1.16.2 Ghidra dump's own function sizes, first-`ret` truncates
    `0x140261b80` to 0x17 of its 0x1a bytes and `0x140262250` to 0x10 of its 0x13. So the sweep
    carries a watermark of the furthest forward branch target seen, and only a `ret`/tail-`jmp`
    that ends beyond every such target ends the function.

    The extent never includes the padding that follows, which is what keeps two builds with
    different pad runs in phase. Returns `None` when no terminator is reached inside `limit`.

    A tail call out of the function ends it, whatever follows. The `jmp` clause below used to
    require that the fall-through byte be padding or a declared function start, and in the
    de-Arxan'd images that test cannot fire at all across large stretches of `.text`: the gaps
    between functions there hold the deobfuscator's leftover bytes rather than `cc`/`90` runs, and
    a `.pdata`-less region has no declared start to land on either. Measured at the three addresses
    that sent this back for repair on 2026-08-30 -- the gap before 1.16.2 `0x14090a0a0` is
    `82 cd ac aa 32 3e 47 4b`, and before 1.17 `0x14090b240` it is `05 00 00 00 00 00 00 00`.
    Neither is padding, so the sweep walked straight through the gap and kept going:

    * `0x14090a0a0 -> 0x14090b240` (`LOADING_SCREEN_GFX_FADEOUT_RVA`) decoded `LEAF:0x45/0x45` --
      the 23-byte thunk, 9 bytes of gap, the whole next thunk, 9 more bytes of gap and into a
      third function -- and then reported `DIVERGES 0.86`, because the two images' gap bytes
      differ. Ghidra puts both functions at 23 bytes, in both builds.
    * `0x14090a0c0 -> 0x14090b260` (`KNOWLEDGE_TIP_ADVANCE_ENABLED_RVA`) decoded `LEAF:0x25/0x25`
      and reported `DIVERGES 0.75`. Ghidra: 23 bytes, both builds.

    That is the worst available failure. `refuted_sources` in `er-game-base/build.rs` reads
    `DIVERGES` as positive evidence the pair is wrong and subtracts the address from the call map
    as well as the detour map, so a sweep artefact would not merely fail to rescue these rows, it
    would delete them -- and delete a correct address on the strength of comparing one image's
    dead gap bytes against another's.

    So a direct `jmp` whose target lies outside the sweep window ends the body on its own. Such a
    jump leaves the function by construction; nothing inside can branch back over it (the
    watermark already refuses the stop if anything did), and the bytes after it are unreachable by
    fall-through whether they are padding, leftovers or the next function. The narrowness is
    deliberate and is what separates this from "any `jmp` ends the body": an intra-function jump --
    around a block, forward to the epilogue -- targets an address inside the window and still has
    to satisfy the original padding/start test. Indirect jumps have no immediate at all, so
    `branch_target` returns `None` and a jump table is untouched by this clause.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    rva = va - BASE
    if rva < 0 or rva >= len(image):
        return None
    watermark = rva
    for insn in md.disasm(bytes(image[rva : rva + limit]), rva):
        after = insn.address + insn.size
        target = branch_target(insn)
        if target is not None and rva <= target < rva + limit:
            watermark = max(watermark, target)
        if insn.mnemonic == "ret" and after > watermark:
            return after
        if insn.mnemonic == "jmp" and after > watermark:
            # A tail call out of the function: the destination is past the window this sweep can
            # reach, so the jump cannot be intra-function control flow and the body ends here
            # regardless of what the gap after it holds. See the docstring for the three rows a
            # padding-only test lost to the de-Arxan'd images' leftover gap bytes.
            if target is not None and not (rva <= target < rva + limit):
                return after
            # A tail call at a function boundary: the next byte is padding or another function.
            if after in starts or (after < len(image) and image[after] in FUNCTION_PAD_BYTES):
                return after
    return None


def pdata_regions(image):
    """Sorted `(begin, end)` of every `.pdata` region, continuation chunks included.

    Deliberately not `function_regions`, which merges chunk runs and drops continuations because
    it answers "does a function BEGIN here". This answers "is this address INSIDE one", and for
    that a continuation chunk counts: it is the interior of a function whatever the linker filed
    it under. A chained chunk not contiguous with its primary is absent from the merged view
    entirely, and the interior of one is exactly where a mis-mapped address hides best.
    """
    return sorted((begin, end) for begin, end, _unwind in runtime_functions(image))


def inside_pdata(regions, rva):
    """Is `rva` within some region's half-open `[begin, end)`?"""
    import bisect

    index = bisect.bisect_right(regions, (rva, 1 << 62))
    # Regions nest and are not ordered by end, so walk back a bounded window rather than trusting
    # the single nearest predecessor.
    return any(begin <= rva < end for begin, end in regions[max(0, index - 16) : index])


def add_leaf_extents(image, extents, starts, vas):
    """Give `extents` a decoded entry for each `va` the `.pdata` table does not describe at all.

    Returns the set of RVAs whose extent was derived here rather than declared by the image. That
    set is what keeps the provenance visible downstream: `compare` needs it to say
    `IDENTICAL-LEAF` instead of `IDENTICAL-WHOLE`, and without it the two claims would be
    indistinguishable in the table the build reads.

    Not described is a stronger test than not declared, and this asks the stronger one. Until
    2026-08-30 the skip was `rva in extents` -- "no `.pdata` entry BEGINS here" -- which is not
    the premise `IDENTICAL-LEAF` rests on. An address 0x10 bytes into a declared function begins
    nothing, so it passed that test, took a decoded extent, and could reach the one verdict that
    issues its own detour licence: a hook landing in the middle of a function, with the
    `NEITHER-ENTRY` clause reporting no objection because neither side is an entry. The premise
    the verdict actually needs is that the linker described no function here, so that is what is
    checked.

    Measured when the check was tightened: zero rows in any current map move. Every derived
    extent already belonged to an address no `.pdata` region covers in either image, so this
    closes a latent hole rather than fixing a live one -- and
    `scripts/check-leaf-extent-pdata-coverage.py` keeps it closed from the outside as well.
    """
    regions = pdata_regions(image)
    added = set()
    for va in vas:
        rva = va - BASE
        if rva in extents or inside_pdata(regions, rva):
            continue
        end = leaf_extent(image, va, starts)
        if end is not None:
            extents[rva] = end
            added.add(rva)
    return added


def entry_evidence(old_starts, new_starts, old_va, new_va):
    """Which side of the pair the image declares to be a function start."""
    src = (old_va - BASE) in old_starts
    dst = (new_va - BASE) in new_starts
    if src and dst:
        return ENTRY_BOTH
    if src:
        return ENTRY_DEST_NOT
    if dst:
        return ENTRY_SRC_NOT
    return ENTRY_NEITHER


def normalise(insn):
    """Mnemonic + register-only operand shape: what survives a patch that moves data around."""
    from capstone import CS_OP_MEM, CS_OP_REG

    parts = [insn.mnemonic]
    for operand in insn.operands:
        if operand.type == CS_OP_REG:
            parts.append(insn.reg_name(operand.reg) or "reg")
        elif operand.type == CS_OP_MEM:
            base = insn.reg_name(operand.mem.base) if operand.mem.base else "-"
            index = insn.reg_name(operand.mem.index) if operand.mem.index else "-"
            # The displacement itself is dropped: that is exactly what 1.17 changed.
            parts.append(f"[{base}+{index}*{operand.mem.scale}]")
        else:
            parts.append("imm")
    return " ".join(parts)


def decode(image, va, limit=DECODE_LIMIT, end_rva=None):
    """Normalised instructions of the function at `va`. See `decode_status` for the details."""
    return decode_status(image, va, limit=limit, end_rva=end_rva)[0]


def decode_status(image, va, limit=DECODE_LIMIT, end_rva=None):
    """`(normalised instructions, stop reason, RVA reached, per-instruction start RVAs)`.

    The fourth element is what turns an instruction index back into a byte offset, which is the
    only way to ask whether a difference at instruction 873 lies inside or outside the handful of
    bytes MinHook is going to relocate. `PATCH_SITE_IDENTICAL` needs exactly that question
    answered; nothing else uses it, and `decode` still returns the stream alone.

    The stop reason is the half that was missing until 2026-08-30, and its absence is what let a
    truncated comparison pass itself off as a complete one. `compare` had no way to ask "did this
    decode reach the end of the function, or did it merely run out of budget", so it inferred
    coverage from the extent's byte length -- which answers a different question. Measured
    consequence: `STEP_MoveMap` (0x140af7cf0 -> 0x140af9000) is 975 instructions and 1.17 inserts
    two of them at instruction 873; the first 120 matched, the row read `IDENTICAL 1.000`, and it
    was promoted into `DETOUR_SAFE_1162_TO_1170` on evidence covering 12% of the body.

    Reasons, in the order they can occur:
      * `STOP_EXTENT` -- the decode reached the declared end. The comparison is complete.
      * `STOP_TERMINATOR` -- an unbounded decode ended on a `ret` or a tail `jmp` at a boundary.
        Complete as far as this file can tell, but nothing declared where the end was.
      * `STOP_LIMIT` -- the decode ran out of `DECODE_LIMIT` or `DECODE_BYTES`. The result is a
        prefix of unknown coverage, and `compare` refuses to call a prefix identical.

    Knowing where to stop is the whole accuracy of this tool. Stopping only at `ret` -- which is
    what this did until 2026-08-30 -- silently walks off the end of any function that ends in a
    tail call, and a great many do. Past the end sits inter-function padding whose length differs
    between builds (measured: 3 bytes in 1.16.2 against 4 in 1.17 after
    `CS::MenuWindowJob::~MenuWindowJob`), so the two decodes fall out of phase and every
    instruction after that point compares unequal. The result is a confident `DIVERGES` on a
    function that is byte-identical in its own body.

    That false negative is not merely noise: `build.rs::refuted_sources()` treats `DIVERGES` as
    positive evidence that an address is wrong and subtracts the row from `VERIFIED_1162_TO_1170`
    -- the call map, not just the detour map. So a decoding artifact removes a working address and
    the feature dies with a `failed to resolve` line. Three independent reviews on 2026-08-30
    found the same artifact behind 12 of 12 non-clean rows, with zero changed immediates and zero
    changed struct offsets among them.

    Stops, in order of authority:
      1. `end_rva` -- the `.pdata` extent. The image's own declaration of where the function ends;
         nothing beats it, so it is used whenever both images declare one.
      2. `ret`.
      3. An unconditional `jmp` immediately followed by a pad byte -- `int3` (0xCC) or `nop`
         (0x90); see `FUNCTION_PAD_BYTES`, and note the two builds do not always pad the same
         function the same way. A `jmp` in the middle of a body (a loop, a branch to a shared
         epilogue) is followed by real code and must not stop the decode.
      4. An unconditional `jmp` with a rel8/rel32 destination, past the forward-branch WATERMARK.
         For the shape rules 1-3 all miss: a 5-byte `jmp` THUNK. It has no `.pdata` entry in
         either image (so rule 1 never fires), no `ret` (rule 2), and the game's thunks are packed
         flush against each other, so the byte after the `jmp` is the next thunk's first
         instruction rather than padding (rule 3). Measured on `UPDATE_TROPHY_STATS_RVA`
         0x24a1a0, which is `e9 1b 14 00 00` at the same address in both images and was
         nonetheless reported `DIVERGES 0.05, first diff at insn 1` -- insn 1 being the first
         instruction of the next thunk, a different one in each build.

         How rule 4 tells a tail call from a mid-body `jmp`. It carries a watermark: the furthest
         forward destination of any branch decoded so far, this `jmp` included. The rule fires
         only when the byte after the `jmp` is beyond that watermark, and the three cases
         separate cleanly:
           * a `jmp` to a shared epilogue branches forward, so its own destination raises the
             watermark past the following byte and the rule does not fire;
           * a loop back-edge branches backward -- but then whatever follows it can only be
             entered by an earlier forward branch, whose destination already raised the watermark
             past the following byte, so the rule does not fire there either. Control does not
             fall through an unconditional `jmp`, so a following instruction that no decoded
             branch reaches is not part of this function;
           * a tail call or thunk leaves the function, nothing decoded so far reaches the
             following byte, and the decode stops exactly at the end.
         Indirect `jmp`s are excluded by `branch_target` returning `None` for them, which is what
         stops a switch dispatch from truncating its own cases.

    When an extent is declared it is the only stop that applies, and `DECODE_LIMIT` /
    `DECODE_BYTES` / `ret` are all suspended for it. Both of the suspended rules were producing
    silently partial comparisons that the caller could not distinguish from complete ones:

      * `ret` truncates any function with an early-return fast path. Measured on
        `MMS_STEP_FINISH` (0x140aec050), where the first `ret` ends the decode at 35 of the
        function's 46 instructions even though the `.pdata` extent runs on;
      * the limit truncates every long body. Measured across the two verdict tables: 150 of 383
        rows never reached their own declared end, `STEP_MoveMap` at 120 of 975.

    The limits exist to bound a decode that has no declared end -- there they are the only thing
    standing between this tool and the rest of the image -- and they keep applying in that case.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    offset = va - BASE
    if offset < 0 or offset >= len(image):
        return [], STOP_LIMIT, offset, []
    declared = end_rva is not None and end_rva > offset
    window = (end_rva - offset) if declared else DECODE_BYTES
    out = []
    starts_at = []
    reason = STOP_LIMIT
    reached = offset
    # Furthest forward branch destination seen so far. Only destinations inside the decode window
    # count: a branch that leaves the window is not this function's own control flow.
    watermark = va
    for insn in md.disasm(bytes(image[offset : offset + window]), va):
        if declared and insn.address - BASE >= end_rva:
            break
        out.append(normalise(insn))
        starts_at.append(insn.address - BASE)
        reached = insn.address - BASE + insn.size
        if declared:
            # The extent is authoritative: nothing short of it ends the decode. Reaching the last
            # declared byte is what earns the "complete" claim; falling short of it leaves the
            # reason at STOP_LIMIT and hands `compare` the residue to account for.
            if reached >= end_rva:
                reason = STOP_EXTENT
            continue
        if len(out) >= limit:
            break
        if insn.mnemonic == "ret":
            reason = STOP_TERMINATOR
            break
        target = branch_target(insn)
        if target is not None and va <= target < va + window:
            watermark = max(watermark, target)
        if insn.mnemonic == "jmp":
            if reached < len(image) and image[reached] in FUNCTION_PAD_BYTES:
                reason = STOP_TERMINATOR
                break
            if target is not None and reached + BASE > watermark:
                reason = STOP_TERMINATOR
                break
    return out, reason, reached, starts_at


# Why a decode stopped. `compare` treats only the first two as complete evidence.
STOP_EXTENT = "extent"
STOP_TERMINATOR = "terminator"
STOP_LIMIT = "limit"


# MSVC pads between functions with either `int3` (0xCC) or `nop` (0x90), and it does not always
# make the same choice in two builds of the same function. A `jmp` followed by one of these is a
# tail call at a function boundary; a `jmp` followed by real code is inside a body.
#
# Both bytes are needed, and here is what happens with only 0xCC. The Quit-tab row thunks are
# 9 bytes -- `add rcx,8; jmp <handler>` -- and have no `.pdata` extent, so the pad byte is the
# only boundary signal. 1.16.2 pads 0x140961649 with `nop`; 1.17 pads 0x140962789 with `int3`.
# Recognising only `int3` stops the 1.17 decode at 2 instructions and lets the 1.16.2 decode run
# on, so the two lengths differ, and the verdict drops from identical to near on a pair whose
# every compared instruction matched. Near is not in {byte-identical, identical}, so the row stays
# callable but never becomes detour-safe -- and these two thunks are detour targets: the Quit
# tab's Save Game and Return-to-Desktop row actions, the second guarding an irreversible action.
# A pad-byte preference is not a reason to refuse a hook.
FUNCTION_PAD_BYTES = frozenset((0xCC, 0x90))


def whole_function_bytes(image, extents, va):
    """The function's entire body, or `None` when the image does not declare one here."""
    begin = va - BASE
    end = extents.get(begin)
    if end is None or end <= begin:
        return None
    return bytes(image[begin:end])


def compare(
    old_image,
    new_image,
    old_va,
    new_va,
    old_extents=None,
    new_extents=None,
    old_derived=frozenset(),
    new_derived=frozenset(),
    old_starts=None,
    new_starts=None,
):
    """Judge one pair. See the module docstring's VERDICTS section for what each answer means.

    `old_starts`/`new_starts` are each image's `.pdata` function-start set. Without them the
    entry evidence cannot be computed here and [`PATCH_SITE_IDENTICAL`] can never be issued --
    the one verdict that turns on a body being different fails closed rather than guessing. Every
    caller in this file supplies them; `result["patch_site"]` says so out loud when one does not,
    so a silently unreachable gate reads as a refusal with a reason rather than as a clean pass.
    """
    old_rva, new_rva = old_va - BASE, new_va - BASE
    old_end = old_extents.get(old_rva) if old_extents is not None else None
    new_end = new_extents.get(new_rva) if new_extents is not None else None
    bounded = old_end is not None and new_end is not None
    # An extent this file DECODED rather than read out of `.pdata`. The distinction is the whole
    # difference between identical-whole and identical-leaf; see `add_leaf_extents`.
    derived = old_rva in old_derived or new_rva in new_derived
    extents = extent_note(old_rva, old_end, new_rva, new_end, derived)
    delta = (new_end - new_rva) - (old_end - old_rva) if bounded else None
    # Computed here, above every return, because a verdict table's entry column must be present on
    # every row -- including the two the shortcuts below return early. `None` means the caller did
    # not supply the `.pdata` start sets, which is the only state in which the column is absent.
    entry = (
        entry_evidence(old_starts, new_starts, old_va, new_va)
        if old_starts is not None and new_starts is not None
        else None
    )
    entry_column = {"entry": entry} if entry is not None else {}
    patch_site = (
        "no .pdata start sets supplied; PATCH-SITE-IDENTICAL cannot be reached"
        if entry is None
        else "not asked: the two instruction streams are equal"
    )

    # A whole-function byte comparison, where both images declare an extent, settles the question
    # outright: same length, same bytes, nothing left to interpret. It is worth trying first
    # because the normalised comparison deliberately throws away displacements and immediates, so
    # it can only ever say "the same up to what 1.17 was expected to change" -- a weaker claim
    # than the one available for free when a function did not change at all.
    #
    # A derived extent is deliberately excluded from this shortcut. "The bytes are equal" is only
    # as good as the two endpoints, and if the same decoding rule truncated both leaves at the
    # same wrong place their equal prefixes prove nothing about the rest. Those pairs go the long
    # way round and come back identical-leaf, which carries that provenance in its name.
    if bounded and not derived:
        left_body = whole_function_bytes(old_image, old_extents, old_va)
        right_body = whole_function_bytes(new_image, new_extents, new_va)
        if left_body and right_body and left_body == right_body:
            return {
                "verdict": "BYTE-IDENTICAL",
                "ratio": 1.0,
                "compared": len(left_body),
                "first_diff": None,
                "left_len": len(left_body),
                "right_len": len(right_body),
                "whole_body": True,
                "covered": True,
                "extents": extents,
                "extent_delta": delta,
                "patch_site": "the bodies are byte-identical; no patch-site question arises",
                **entry_column,
            }
    # Bound each decode by that image's own extent when there is one. This is what keeps the two
    # instruction streams in phase: without it a tail-call function runs into padding of a
    # different length in each build and everything after diverges.
    left, left_stop, left_reached, left_at = decode_status(old_image, old_va, end_rva=old_end)
    right, right_stop, right_reached, right_at = decode_status(new_image, new_va, end_rva=new_end)
    if not left or not right:
        return {
            "verdict": "UNDECODABLE",
            "ratio": 0.0,
            "compared": 0,
            "first_diff": None,
            "whole_body": False,
            "covered": False,
            "extents": extents,
            "extent_delta": delta,
            "patch_site": "nothing decoded; there is no patch site to compare",
            **entry_column,
        }
    compared = min(len(left), len(right))
    first_diff = next((i for i in range(compared) if left[i] != right[i]), None)
    if len(left) == len(right):
        ratio = sum(1 for i in range(compared) if left[i] == right[i]) / compared
    else:
        # The streams are different lengths, so index-against-index is measuring the wrong thing.
        # An insertion shifts every later instruction by one and an index-wise ratio reads the
        # entire tail as changed: `STEP_MoveMap`, whose 1.17 body gains exactly two instructions
        # at index 873 of 975, scores 0.898 that way and lands on DIVERGES -- which
        # `build.rs::refuted_sources()` reads as proof the address is wrong and subtracts from the
        # call map. An alignment-aware ratio scores the same pair 0.999 and lands on NEAR: the row
        # keeps its call mapping and is refused only the detour, which is the honest answer for a
        # function that really did gain code. Equal-length pairs keep the index-wise ratio so no
        # existing verdict moves.
        ratio = difflib.SequenceMatcher(None, left, right, autojunk=False).ratio()

    # Did the comparison cover both functions in full? Three things have to hold, and each of them
    # was independently observed being false while the old `whole_body` flag said true:
    #   * both ends are known -- otherwise there is no "in full" to speak of;
    #   * the two extents are the same length. A different length is a positive statement that the
    #     bodies differ, and it costs no decoding at all to notice;
    #   * everything between the entry and that end is accounted for -- the instructions by the
    #     decode (150 of 383 rows used to stop short of their own declared end, `STEP_MoveMap` at
    #     120 instructions of 975) and any trailing non-instruction bytes by `residue_agrees`.
    covered = bool(
        bounded
        and residue_agrees(
            old_image[left_reached:old_end],
            new_image[right_reached:new_end],
            old_rva,
            new_rva,
        )
    )
    # `covered` is "the comparison saw all of both bodies"; `whole_body` is that and "the two
    # bodies are the same length". They were one flag until 2026-08-30, which is why a length
    # delta had nowhere to be reported except the extent column nobody was reading. Splitting
    # them changes no existing verdict -- `whole_body` is the same expression it was -- and gives
    # PATCH_SITE_IDENTICAL the only footing it can honestly stand on: full coverage of a pair
    # whose lengths differ on purpose.
    whole_body = bool(covered and delta == 0)
    truncated = not whole_body and STOP_LIMIT in (left_stop, right_stop)

    if first_diff is None and len(left) == len(right):
        if whole_body:
            # A leaf's extent is this file's own conclusion, so it also owes the claims that
            # `.pdata` would have supplied and cannot here: that the first bytes a detour
            # overwrites are not a branch target, and that there are five of them to overwrite.
            # `leaf_verdict` holds both, and separates "proved and hookable" from "proved and
            # not hookable" instead of collapsing the second into a refusal to answer at all.
            if derived:
                verdict, patch_site = leaf_verdict(
                    old_image, new_image, old_va, new_va, old_end, new_end, compared
                )
            else:
                verdict = IDENTICAL_WHOLE
        elif truncated:
            verdict = IDENTICAL_PREFIX
        elif compared >= THIN_EVIDENCE:
            verdict = "IDENTICAL"
        else:
            verdict = "IDENTICAL-SHORT"
    else:
        # The streams are not equal. Today's answer first, so that a gate which declines leaves
        # every existing verdict exactly where it was.
        verdict = "NEAR" if ratio >= 0.95 else REFUTED
        if bounded and not derived and covered and entry == ENTRY_BOTH:
            admitted, patch_site = patch_site_drift(
                old_image, new_image, old_va, new_va, left, right, left_at, right_at
            )
            if admitted:
                verdict = PATCH_SITE_IDENTICAL
        elif entry is not None:
            patch_site = (
                "refused before the diff: "
                + ", ".join(
                    reason
                    for reason, held in (
                        ("no .pdata extent on both sides", not bounded),
                        ("an extent this file DECODED, not one .pdata declared", derived),
                        ("the comparison did not cover both bodies in full", not covered),
                        (f"entry evidence is {entry}, not {ENTRY_BOTH}", entry != ENTRY_BOTH),
                    )
                    if held
                )
            )
    # A boundary this file DECODED may not refute an address. See [`refutation_withheld`]: the
    # verdict that removes a row has to rest on evidence stronger than this file's own guess about
    # where a function ends, and a decoded extent is exactly that guess.
    if derived and verdict == REFUTED:
        return refutation_withheld(
            old_image,
            new_image,
            old_va,
            new_va,
            old_extents,
            new_extents,
            old_derived,
            new_derived,
            old_starts,
            new_starts,
            extents,
            ratio,
        )
    result = {
        "verdict": verdict,
        "ratio": ratio,
        "compared": compared,
        "first_diff": first_diff,
        "left_len": len(left),
        "right_len": len(right),
        "whole_body": whole_body,
        "covered": covered,
        "extents": extents,
        "extent_delta": delta,
        "patch_site": patch_site,
        **entry_column,
    }
    return result


def refutation_withheld(
    old_image,
    new_image,
    old_va,
    new_va,
    old_extents,
    new_extents,
    old_derived,
    new_derived,
    old_starts,
    new_starts,
    decoded_note,
    decoded_ratio,
):
    """Re-judge a pair whose [`REFUTED`] verdict rested on a DECODED extent, without that extent.

    The ASYMMETRY this ENCODES. A verdict that fails to accept a row costs a feature loudly: the
    address stays unmapped and the DLL logs `failed to resolve`. [`REFUTED`] is the only verdict
    that goes the other way -- `build.rs::refuted_sources()` subtracts the row from the call map as
    well as the detour map, so a correct address that was already working disappears, with nothing
    printed. Deleting a right answer is strictly worse than declining to add one, so the evidence
    required for it has to be correspondingly stronger.

    A DECODED extent is not that evidence. `.pdata` is the image's own statement about where a
    function ends; `leaf_extent` is this file's conclusion about it, reached by a sweep whose stop
    rules have been wrong before. Measured, and the reason this exists: until 2026-08-30 the sweep
    ended a body at a `jmp` only when the following byte was `0xCC`/`0x90` padding or a declared
    `.pdata` start. In these de-Arxan'd images the inter-function gaps hold the deobfuscator's
    residue instead -- `48 8d 64 24 08 ff 64 24 f8` after 1.16.2 `0x14090a0b7` -- so the sweep ran
    through the gap into the next function and compared unrelated code. `0x14090a0a0 ->
    0x14090b240` took extents of 0x45/0x45 instead of 0x17/0x17 and came back `DIVERGES 0.86`; a
    23-byte thunk that is the same function in both builds would have been SUBTRACTED from the call
    map on the strength of comparing one image's dead gap bytes against another's.

    The stop rule is fixed. This is the rule that makes the next such bug cost a dropped row rather
    than a deleted one: the pair is re-judged with the decoded extent withdrawn, which is the
    answer the file would have given had it never guessed at a boundary. That answer is a prefix
    comparison, so it can only reach `IDENTICAL`/`IDENTICAL-SHORT`/`NEAR` -- verdicts nothing
    accepts and nothing subtracts -- unless the UNDECODED comparison refutes the pair on its own
    terms, in which case the refutation is kept, because then it never rested on the guess.

    Returns the re-judged result, carrying `refutation_withheld` when the withdrawal actually
    changed the answer, so `main` can say out loud that a refutation was declined. A rule that
    quietly suppressed one would be its own silent failure.
    """
    old_rva, new_rva = old_va - BASE, new_va - BASE
    bare_old = old_extents
    if old_extents is not None and old_rva in old_derived:
        bare_old = {rva: end for rva, end in old_extents.items() if rva != old_rva}
    bare_new = new_extents
    if new_extents is not None and new_rva in new_derived:
        bare_new = {rva: end for rva, end in new_extents.items() if rva != new_rva}
    result = compare(
        old_image,
        new_image,
        old_va,
        new_va,
        bare_old,
        bare_new,
        frozenset(),
        frozenset(),
        old_starts,
        new_starts,
    )
    if result["verdict"] != REFUTED:
        result["refutation_withheld"] = (
            f"a DECODED extent ({decoded_note}) made the bodies {REFUTED} at ratio "
            f"{decoded_ratio:.2f}; a boundary this file inferred may not delete an address, so the "
            f"pair was re-judged with that extent withdrawn and came back {result['verdict']}"
        )
    return result


def residue_agrees(left, right, old_rva, new_rva):
    """Do the trailing bytes neither decode reached account for each other?

    A `.pdata` extent is not always all code. MSVC parks a switch's jump table inside the function
    it belongs to, after the last instruction, and capstone stops when it runs into it -- so a
    decode bounded by the extent legitimately ends short. Two rows do exactly this:
    `SL_POLL_SAVE_STATUS` (0x140e6e430) trails 104 bytes and `0x140afa6d0` trails 36, identically
    in both builds. Demanding the decode land exactly on the declared end called both of them
    truncated and refused a pair whose every instruction matched.

    The table is not comparable byte for byte, because its entries are addresses and 1.17 moved
    the function. They are comparable relative to the function, which is the same move `normalise`
    makes for displacements: entry 0 of `SL_POLL_SAVE_STATUS` is `0xe6e5d4` in 1.16.2 and
    `0xe703d4` in 1.17, both exactly `+0x1a4` from their own function start. All 26 entries agree
    that way, and so do all 9 of the other row's.

    Fails closed. Different lengths, a length that is not a whole number of 4-byte entries, or one
    entry that does not line up, and the residue is not accounted for -- which leaves the verdict
    at `IDENTICAL-PREFIX`, where nothing accepts it.
    """
    if len(left) != len(right):
        return False
    if not left:
        return True
    if left == right:
        return True
    if len(left) % 4:
        return False
    for offset in range(0, len(left), 4):
        a = int.from_bytes(left[offset : offset + 4], "little")
        b = int.from_bytes(right[offset : offset + 4], "little")
        if a - old_rva != b - new_rva:
            return False
    return True


def extent_note(old_rva, old_end, new_rva, new_end, derived):
    """The extent column: where each end came from, and whether the two agree on length.

    A human reading a verdict table has to be able to see the extent-length delta without
    re-deriving it, because it is the cheapest available evidence that a body changed and it needs
    no decoding whatsoever. `PDATA:0x120b/0x1213+8` says the two `.pdata` entries disagree by eight
    bytes; that row is not identical however well its first hundred instructions matched.
    """
    if old_end is None and new_end is None:
        return "NONE"
    if old_end is None or new_end is None:
        return "PARTIAL"
    left, right = old_end - old_rva, new_end - new_rva
    source = "LEAF" if derived else "PDATA"
    delta = right - left
    return f"{source}:{left:#x}/{right:#x}" + (f"{delta:+d}" if delta else "")


def leaf_fits_patch(rva, end_rva, window=PATCH_BYTES):
    """Is this leaf even long enough for MinHook to patch?

    The other half of the claim `.pdata` would have made. [`branch_into_prologue`] asks whether
    the bytes a detour overwrites are a branch target; this asks whether those bytes are inside
    the function at all. A `.pdata` extent shorter than the patch is refused by
    `scripts/audit-1170-hook-targets.py::patch_safe` for the same reason, so a derived extent has
    to be held to it too -- otherwise the one verdict that issues its own detour licence issues a
    licence to write five bytes into a three-byte body, and past its end into whatever follows.

    Measured 2026-08-30, on the regeneration that first admitted leaves: two 3-byte bodies reached
    `DETOUR_SAFE_1162_TO_1170` this way. `0x1407add70 -> 0x1407aebf0` (`LEAF:0x3/0x3`, body
    `33 c0 c3` = `xor eax,eax; ret`) and `0x141c92f30 -> 0x141c94d30`
    (`CTRL_SUBOBJECT_RELEASE_RVA`, body `c2 00 00` = `ret 0`). The hook audit flagged both
    patch-unsafe with the reason spelled out -- 3 bytes long, the bytes after are not padding and
    the five above are not padding either, so there is nowhere to put the jump. Neither would have
    installed; the defect is that the map claimed they would.

    A leaf that fails this is refused the detour only, as [`IDENTICAL_LEAF_NOPATCH`] -- a verdict
    `build.rs` admits to the call map and to nothing else. It is not a fallback to
    `IDENTICAL`/`IDENTICAL-SHORT`: that is what this docstring used to claim, and it was false.
    `build.rs` seeded the call map from `detourable_pairs`, so `IDENTICAL-SHORT` withdrew a
    three-byte getter from comparing as well as from hooking, and both addresses above were lost
    to their features entirely. Only when MinHook's own ported rules also refuse the site, so the
    "no room" half of the claim is its answer rather than this file's arithmetic; a leaf that
    fails for any other reason still falls back to `IDENTICAL`/`IDENTICAL-SHORT` and is accepted
    nowhere.
    """
    return end_rva is not None and end_rva - rva >= window


def branch_into_prologue(image, va, end_rva, window=PATCH_BYTES):
    """Does any branch inside this body target a byte strictly inside its first `window` bytes?

    The claim `.pdata` makes and a leaf cannot. `DETOURABLE_ENTRY_EVIDENCE` in
    `er-game-base/build.rs` accepts a pair only when both images' `.pdata` agree about the two
    endpoints, and the reason is not that the code matches -- `compare` answers that -- but that
    MinHook is about to relocate the first five bytes and needs them to be a relocatable entry.
    A `.pdata`-declared function start is the linker's own statement that one begins there.

    A leaf has no `.pdata` entry in either image, so it reaches that rule through the
    `NEITHER-ENTRY` branch, which was written for a different situation (a deliberate fixed offset
    into a known function, symmetric in both builds) and carries no such statement. Letting leaves
    in on that clause alone would widen the detour gate by accident. So the missing claim is made
    directly instead, by checking the property MinHook actually needs: no branch may land inside
    the bytes the patch overwrites, because after the patch those bytes are half a `jmp`.

    Measured on the five leaves this exists for -- 0x67a810, 0x67a980, 0xd4cc50, 0x26634a0 and
    0x4f9940 -- none has a branch target in its first five bytes, in either build.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    rva = va - BASE
    if end_rva is None or end_rva <= rva:
        return False
    for insn in md.disasm(bytes(image[rva:end_rva]), va):
        target = branch_target(insn)
        if target is not None and va < target < va + window:
            return True
    return False


_MINHOOK = None


def minhook_port():
    """`scripts/audit-1170-hook-targets.py`, imported for its port of MinHook's own rules.

    Imported, not RE-derived. `trampoline_walk` there is a line-by-line port of
    `CreateTrampolineFunction` from `vendor/minhook/src/trampoline.c` -- the copy that will be
    asked to install these detours -- and it took two wrong hand-reasoned versions to get there.
    A second approximation living here would be a third. The module also carries the branch-into-
    the-patched-bytes scan MinHook itself does not do, in `patch_safe`.

    The sibling's name has hyphens in it, so it is loaded by path rather than by `import`.
    """
    global _MINHOOK
    if _MINHOOK is None:
        path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "audit-1170-hook-targets.py")
        spec = importlib.util.spec_from_file_location("_audit_1170_hook_targets", path)
        if spec is None or spec.loader is None:
            raise RuntimeError(f"cannot load {path}, which holds the ported MinHook rules")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        _MINHOOK = module
    return _MINHOOK


def minhook_refusal(image, va):
    """MinHook's own answer about this address, unabridged, or `None` when it would install.

    Imported, not RE-derived -- see [`minhook_port`]. `patch_safe` runs the ported
    `CreateTrampolineFunction` and then the branch-into-the-patched-bytes scan MinHook itself does
    not do. Asking it is the difference between "this file thinks three bytes is too few" and "the
    code that will be asked to install the hook says it cannot": MinHook has a padding fallback
    that can hook a sub-five-byte function when uniform padding follows it or sits above the
    entry, so length alone is not the answer, and a hand-reasoned version of this loop refused
    five addresses the project hooks successfully today.
    """
    audit = minhook_port()
    ok, why = audit.patch_safe(image, va)
    return None if ok else why


def leaf_verdict(old_image, new_image, old_va, new_va, old_end, new_end, compared):
    """Which leaf verdict a pair with equal whole bodies earns. `(verdict, note)`.

    The caller has already established the part both answers share: both extents were DECODED by
    `leaf_extent` (neither image declares a `.pdata` entry), the two decodes arrived at the same
    byte length, and the normalised streams are equal over all of both bodies. What is left is the
    hook, and the two clauses below decide it. Each is named, each is checked independently, and
    each has been observed to fail on its own in `leaf_nopatch_selftest`.

      1. Prologue -- no branch inside either body targets the bytes a patch overwrites
         (`branch_into_prologue`, both images). This is the claim a `.pdata` entry stands in for
         and a leaf cannot make; without it a leaf reaches `DETOURABLE_ENTRY_EVIDENCE` through
         `NEITHER-ENTRY`, a clause written for a different situation entirely.
      2. Room -- the body is at least the five bytes MinHook writes (`leaf_fits_patch`, both
         images; equal extents mean both sides answer alike), and, when it is not, MinHook's own
         ported rules confirm they will not install there (`minhook_refusal`, both images).

    Three outcomes, and the middle one is why this function exists:

      * both clauses hold -> [`IDENTICAL_LEAF`], which carries its own detour licence;
      * Prologue holds, room does not, and MinHook refuses both sites for itself ->
        [`IDENTICAL_LEAF_NOPATCH`]: the identity is proved and the hook is refused, and those are
        now two answers instead of one. `build.rs` admits it to the call map only;
      * anything else -> `IDENTICAL`/`IDENTICAL-SHORT`, accepted nowhere. A leaf whose prologue is
        a branch target gets no verdict of its own: the refusal is not about room, so naming it
        NOPATCH would misdescribe it, and a body that can be branched into is a shape nobody has
        examined rather than one that has been cleared for calling.

    Why MINHOOK is asked at all when clause 2's first half already answered. Because the two can
    disagree, in the direction that matters: MinHook will hook a sub-five-byte function when
    uniform padding follows it or precedes the entry, so "shorter than five bytes" is this file's
    arithmetic and not a refusal. Requiring the real refusal keeps the verdict's name true, and
    keeps a row whose site MinHook would actually accept out of a verdict that says it would not.
    """
    old_rva, new_rva = old_va - BASE, new_va - BASE
    clear = not branch_into_prologue(old_image, old_va, old_end) and not branch_into_prologue(
        new_image, new_va, new_end
    )
    fits = leaf_fits_patch(old_rva, old_end) and leaf_fits_patch(new_rva, new_end)
    if clear and fits:
        return IDENTICAL_LEAF, "the leaf is long enough to patch and nothing branches into it"
    if clear and not fits:
        refusals = [
            (label, minhook_refusal(image, va))
            for label, image, va in (
                ("1.16.2", old_image, old_va),
                ("1.17", new_image, new_va),
            )
        ]
        if all(why is not None for _label, why in refusals):
            return IDENTICAL_LEAF_NOPATCH, "; ".join(
                f"{label} refuses: {why}" for label, why in refusals
            )
        installable = [label for label, why in refusals if why is None]
        note = (
            f"the body is under {PATCH_BYTES} bytes, but MinHook would install at "
            f"{' and '.join(installable)} anyway, so this is not a NOPATCH leaf"
        )
    else:
        note = "a branch inside the body targets the bytes a patch would overwrite"
    return ("IDENTICAL" if compared >= THIN_EVIDENCE else "IDENTICAL-SHORT"), note


def relocated_prefix(image, va, starts_at):
    """How much of the function at `va` a detour disturbs: `(insns, bytes, refusal or None)`.

    `bytes` is MinHook's `oldPos` when its trampoline walk finishes -- the run of original bytes
    copied into the trampoline, and so also the offset its trailing jump returns into. `insns` is
    how many of this decode's instructions that run covers, which is the unit the diff below is
    measured in. The two are not interchangeable: a displacement that widened from disp8 to disp32
    changes the byte count of an instruction without changing the instruction, and the instruction
    count is the claim worth making.

    The refusal is `patch_safe`'s, unabridged, and covers both halves: MinHook declining to build
    a trampoline at all, and a branch elsewhere in the body landing on the four operand bytes of
    the JMP that is about to replace the prologue.

    Why `relocated` and not simply `PATCH_BYTES`, given they usually agree. MinHook's walk stops
    at the first instruction boundary at or past five bytes, so every instruction it consumed
    starts below five and the two expressions return the same count for any function at least five
    bytes long -- measured: zero of ~25,000 sampled 1.16.2 `.pdata` functions distinguish them, and
    a mutation swapping one for the other is therefore invisible to the selftest. They part company
    for a function shorter than the patch, where MinHook falls back to a two-byte hop and
    `relocated` is the honest smaller number while `PATCH_BYTES` would claim instructions past the
    end of the body.
    """
    audit = minhook_port()
    ok, why = audit.patch_safe(image, va)
    if not ok:
        return None, 0, why
    walked, relocated, _patched_above, walk_why = audit.trampoline_walk(image, va)
    if not walked:  # patch_safe agreed; this cannot happen, and is not assumed not to
        return None, 0, walk_why
    end = va - BASE + relocated
    return sum(1 for at in starts_at if at < end), relocated, None


def patch_site_drift(old_image, new_image, old_va, new_va, left, right, left_at, right_at):
    """Do the two bodies differ only beyond the region a detour disturbs? `(admitted, note)`.

    The claim, in the order it is checked. Every clause is about the detour; none of them is
    about how much of the body happens to agree.

      1. MinHook will build a trampoline at both addresses, and nothing in either body branches
         into the bytes its JMP overwrites (`relocated_prefix` -> `patch_safe`). A pair that fails
         here has no detour to license, whatever its bodies look like.
      2. It relocates the same number of instructions at both. A different number means the two
         walks consumed different code, which is a changed patch site by definition -- and it is
         checked in instructions rather than bytes so that a widened displacement, which 1.17 is
         full of, is not mistaken for one.
      3. The two normalised streams are aligned with `difflib`, and the first place they disagree
         is strictly after the last relocated instruction, in both streams. Instruction `r` -- the
         one the trampoline returns into -- therefore sits inside the equal prefix, so the bytes
         the patch overwrites and the instruction control comes back to are the same in both
         builds. This is the clause the whole verdict is named for, and it is checked before the
         two below so that a refusal names the patch site when the patch site is what moved.
      4. No `replace` hunk. Displacement and immediate drift is already normalised away, so a
         surviving replace is a real change of opcode or register at an aligned position -- a
         different instruction doing a different thing. Measured across the whole image: 173 of
         the 199 differing pairs carry one, and refusing them is what keeps this verdict about
         insertions rather than about rewrites.
      5. The difference stays inside `MAX_DRIFT_HUNKS` places and `MAX_DRIFT_INSNS` instructions.

    What it cannot license, and why. A moved patch site fails clause 1, 2 or 3: the entry region
    is compared instruction for instruction against 1.16.2's, and any insertion, deletion or
    substitution inside it -- or before it -- puts the first hunk at or below `r` and the verdict
    is refused however small the total diff. A pair landing mid-function never reaches this
    function at all: `compare` requires `BOTH-ENTRIES` from each image's own `.pdata` before
    calling it, and a mid-function address is a start in neither image.

    On `0x140aec480`, and what this verdict does not claim about it. That address -- `identical
    1.000` over 56 instructions, `+0x360` inside `0x140aec120..0x140aec567` -- fails the
    `BOTH-ENTRIES` requirement, so it cannot reach here; but it would not have reached here
    anyway, because its two streams are equal and this verdict is only ever asked about bodies
    that differ. What actually stopped it was deleting its ledger row. Do not read
    patch-site-identical as the thing that catches an impostor; read it as a claim that does not
    widen the gate for one.

    And a body that changed near the entry is refused even when the change is a single
    instruction, because clause 3 is a position test, not a size test.
    """
    old_insns, old_bytes, why = relocated_prefix(old_image, old_va, left_at)
    if old_insns is None:
        return False, f"1.16.2 patch site refused: {why}"
    new_insns, new_bytes, why = relocated_prefix(new_image, new_va, right_at)
    if new_insns is None:
        return False, f"1.17 patch site refused: {why}"
    if old_insns != new_insns:
        return False, (
            f"MinHook relocates {old_insns} instruction(s) on 1.16.2 and {new_insns} on 1.17, so "
            "the two patch sites are not the same site"
        )
    hunks = [
        op
        for op in difflib.SequenceMatcher(None, left, right, autojunk=False).get_opcodes()
        if op[0] != "equal"
    ]
    if not hunks:  # `compare` only calls this when the streams differ
        return False, "the streams are equal; this is not the verdict for that"
    # The clause the verdict is named for, checked before the shape and size ones so that a
    # reader debugging a refusal is told about the patch site first when the patch site is what
    # moved. Instruction `old_insns` is the one the trampoline returns into, so the difference has
    # to start strictly after it -- `>`, never `>=`.
    if hunks[0][1] <= old_insns or hunks[0][3] <= new_insns:
        return False, (
            f"the first difference is at instruction {hunks[0][1]}/{hunks[0][3]}, at or inside the "
            f"{old_insns} instruction(s) MinHook relocates -- this is a changed PATCH SITE"
        )
    kinds = {op[0] for op in hunks}
    if "replace" in kinds:
        first = next(op for op in hunks if op[0] == "replace")
        return False, (
            f"instruction {first[1]} is REPLACED, not inserted or deleted "
            f"({left[first[1]]!r} -> {right[first[3]]!r}); normalisation already forgives "
            "displacement and immediate drift, so this is a real change"
        )
    if len(hunks) > MAX_DRIFT_HUNKS:
        return False, f"{len(hunks)} separate differences, above the {MAX_DRIFT_HUNKS} allowed"
    drift = sum((op[2] - op[1]) + (op[4] - op[3]) for op in hunks)
    if drift > MAX_DRIFT_INSNS:
        return False, f"{drift} instructions inserted or deleted, above the {MAX_DRIFT_INSNS} allowed"
    where = "/".join(f"{op[0]}@{op[1]}" for op in hunks)
    return True, (
        f"relocates {old_bytes}B/{old_insns} insn at 1.16.2 and {new_bytes}B/{new_insns} at 1.17; "
        f"{len(hunks)} hunk(s), {drift} insn(s), {where}, all past the patch site"
    )


def load_map(path=None):
    """Pairs to verify, from `path` or the original byte-search table.

    Two shapes are accepted, because the maps that produce candidates now outnumber the one
    this started with. The original table carries a How-it-was-mapped note in column 4; the
    function, data and needed maps carry a constant name or a vote count there, or nothing at
    all. Either way the first two columns are the pair, which is all the verification needs,
    and anything after the second column is passed through as the note.
    """
    pairs = []
    for line in open(path or MAP_TSV, encoding="utf-8"):
        if line.startswith("#") or not line.strip():
            continue
        fields = line.rstrip("\n").split("\t")
        if len(fields) < 2 or fields[1] == "-":
            continue
        try:
            old_va, new_va = int(fields[0], 16), int(fields[1], 16)
        except ValueError:
            continue
        # The newer maps are keyed by RVA; this one by VA. Both are unambiguous because the
        # image base is 0x140000000 and no RVA reaches it.
        if old_va < BASE:
            old_va += BASE
        if new_va < BASE:
            new_va += BASE
        note = fields[3] if len(fields) >= 4 else (fields[2] if len(fields) >= 3 else "")
        pairs.append((old_va, new_va, note))
    return pairs


def role_note(path):
    """The one thing this particular ledger's header must say about where hand work goes.

    Emitted by the writer rather than typed into the file, because the writer truncates: a line a
    human adds to the header is gone at the next `--tsv`, which is the same class of silent loss
    the row guard exists for.
    """
    name = os.path.basename(path or "")
    if name.startswith("rva-map-1162-to-1170.verified"):
        return (
            "# This is the CURATED ledger. A pair derived by hand goes HERE, with its derivation\n"
            "# in the `how` column -- NOT in rva-map-1162-to-1170.needed.tsv, which\n"
            "# scripts/select-needed-1170-rows.py regenerates wholesale from functions.tsv.\n"
        )
    if name.startswith("rva-map-1162-to-1170.needed-verified"):
        return (
            "# This is the verdict table over rva-map-1162-to-1170.needed.tsv, which is itself\n"
            "# regenerated wholesale from functions.tsv. A pair derived by hand belongs in\n"
            "# rva-map-1162-to-1170.verified.tsv, the curated ledger, not in either of these.\n"
        )
    return ""


def preserve_unverified(path, rows):
    """Rows already in the `--tsv` target that this run would not write.

    `--tsv` truncates. That is fine when the target is a scratch file and ruinous when it is
    `rva-map-1162-to-1170.verified.tsv`, which is where hand-derived pairs are put precisely
    because nothing regenerates it -- as of 2026-08-30, 65 of its 99 addresses are not in
    `rva-map-1162-to-1170.tsv` and would not come back. `verify-rva-map-1170.py --tsv
    docs/recon/rva-map-1162-to-1170.verified.tsv` reads like a refresh and is a deletion of
    two thirds of the file, at exit 0, with no line of output naming what went.

    So: a pair the target holds and this run did not produce is carried forward verbatim (its
    verdict columns were produced by this same verifier on an earlier, narrower run, so the line
    is still true) and listed on stderr. A pair the target maps somewhere else than this run does
    is a conflict -- one of the two is a wrong address at a live-looking value -- and the caller
    refuses to write rather than pick.

    Comment lines are carried too, and were not until 2026-09-01. That omission was the same
    silent loss this function exists to prevent, wearing different clothing: the guard counted
    rows, so `docs/recon/rva-map-1162-to-1170.verified.tsv` came through a narrow run with all
    103 data rows intact and 181 of its 209 comment lines gone, at exit 0, with nothing on
    stderr naming them. Measured on the 2026-09-01 ProfileSelect run: 312 lines in, 174 out.

    What went is not decoration. Those interleaved blocks are where this ledger records the
    reasoning a row cannot hold -- why 0x140aec480 was removed (mid-function, +0x360 inside
    0xaec120, and mapping it relocated the bug rather than fixing it), why 0xcf9300 is
    deliberately absent, that ersc.dll RVAs are struck because 1.17 cannot move them, and the
    four mid-function addresses whose rows were refused precisely because they verify
    identical. Delete those and the next agent re-adds the row that was removed on purpose,
    with the verifier agreeing beautifully about the wrong thing.

    The leading comment block is the exception: everything before the first data line is the
    header this writer regenerates (see `role_note`), so carrying it would duplicate it. From
    the first data row onward every line is kept in its original order -- comments, blanks and
    unreproduced rows alike -- so a block stays attached to the rows it annotates. A row this
    run does reproduce moves up into the fresh section while its comment stays put; that
    separates a note from its row, which is a readability cost and not a loss, and it is the
    price of never deleting one.

    Returns `(kept_lines, conflicts)`.
    """
    if not path or not os.path.exists(path):
        return [], []
    produced = {}
    for old_va, new_va, _how, _result in rows:
        produced.setdefault(old_va, set()).add(new_va)
    kept, conflicts, seen = [], [], set()
    body_started = False
    in_generated_banner = False
    for line in open(path, encoding="utf-8"):
        text = line.rstrip("\n")
        fields = text.split("\t")
        pair = None
        if not text.startswith("#") and text.strip() and len(fields) >= 2:
            try:
                old_va, new_va = int(fields[0], 16), int(fields[1], 16)
            except ValueError:
                pair = None
            else:
                if old_va < BASE:
                    old_va += BASE
                if new_va < BASE:
                    new_va += BASE
                pair = (old_va, new_va)
        if pair is None:
            # Two comment blocks in this file are written by this script and must not be carried,
            # or each run would nest another copy of them: the leading header (see `role_note`),
            # and the `# CARRIED FORWARD` banner, which sits in the body and so is not excluded by
            # `body_started` alone. Everything else after the first row is hand-written prose and
            # is kept -- see this function's docstring for what that prose is holding.
            if CARRIED_BANNER_MARK in text:
                in_generated_banner = True
                # The writer prints a bare `#` separator immediately above the banner. Without
                # dropping it too, every run carries one more of them forward and the file grows
                # by a line each time -- which is how you find out a "verbatim" carry is not
                # idempotent. `--tsv` twice in a row must produce the same bytes.
                while kept and kept[-1].strip() in ("#", ""):
                    kept.pop()
                continue
            if in_generated_banner:
                if text.startswith("#"):
                    continue
                in_generated_banner = False
            if not body_started:
                continue
            kept.append(text)
            continue
        in_generated_banner = False
        body_started = True
        old_va, new_va = pair
        if new_va in produced.get(old_va, ()):
            continue
        if old_va in produced:
            conflicts.append((old_va, new_va, sorted(produced[old_va])[0]))
            continue
        # Keyed on the whole line, not the pair. `verified.tsv` currently holds two addresses
        # twice, each with a different derivation in the `how` column, and collapsing them here
        # would throw one derivation away -- a smaller version of the same silent loss. Whether a
        # duplicate pair should exist is a question for whoever added the second one.
        if text in seen:
            continue
        seen.add(text)
        kept.append(text)
    while kept and not kept[-1].strip():
        kept.pop()
    return kept, conflicts


def main():
    parser = argparse.ArgumentParser(
        description="Verify mapped 1.17 addresses are the same function as their 1.16.2 original."
    )
    parser.add_argument("vas", nargs="*", help="1.16.2 VAs to check (default: the whole table)")
    parser.add_argument("--tsv", metavar="PATH", help="write the verdicts here")
    parser.add_argument(
        "--map",
        metavar="PATH",
        help="read candidate pairs from this table instead of the byte-search one",
    )
    parser.add_argument(
        "--min-ratio",
        type=float,
        default=1.0,
        help="ratio at or above which a pair is listed as accepted (default 1.0)",
    )
    parser.add_argument(
        "--leaf-extents",
        action="store_true",
        help="DEPRECATED and ignored: leaf extents are always derived now. Accepted so a "
        "regeneration command written down while this was an opt-in still runs, and still "
        "produces the same table it did then",
    )
    parser.add_argument(
        "--selftest",
        action="store_true",
        help="assert the verdicts that the 2026-08-29 crash bisect established",
    )
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    for image in (OLD_IMAGE, NEW_IMAGE):
        if not os.path.exists(image):
            sys.exit(f"missing image: {image}")
    old_image = open(OLD_IMAGE, "rb").read()
    new_image = open(NEW_IMAGE, "rb").read()
    old_starts = function_starts(old_image)
    new_starts = function_starts(new_image)
    old_extents = function_extents(old_image)
    new_extents = function_extents(new_image)

    pairs = load_map(args.map)
    if args.vas:
        wanted = {int(v, 0) for v in args.vas}
        pairs = [p for p in pairs if p[0] in wanted]
    if not pairs:
        sys.exit("nothing to verify")

    # Unconditional since 2026-08-30, and it used to be the opt-in `--leaf-extents`.
    #
    # Why the OPT-in had to go. Three rows of the ledger `er-game-base/build.rs` reads --
    # LOADING_SCREEN_GFX_FADEOUT_RVA, KNOWLEDGE_TIP_ADVANCE_ENABLED_RVA and
    # PLAYER_GAME_DATA_NAME_GETTER_RVA -- carry identical-leaf, and they carry it only because
    # somebody remembered to type the flag. Nothing recorded that they had, nothing enforced it,
    # and `--tsv` TRUNCATES: the next regeneration by anyone who did not know would have written
    # those three back as identical-short, a verdict `build.rs` accepts nowhere, and the three
    # addresses would have left the call map at exit 0 with nothing naming them. A correct address
    # deleted by a forgotten command-line flag is not a failure mode worth keeping.
    #
    # Why making it the default is safe rather than merely convenient. Measured over both candidate
    # maps on 2026-08-30: 15 rows move, every one of them from identical-short or identical to
    # identical-leaf, and none in the other direction -- so the flag was never selecting between
    # two answers, it was selecting between an answer and a shrug. The cost is 0.3s on a 305-pair
    # run, because a decode is attempted only for an address no `.pdata` region describes at all.
    #
    # And there is no OPT-out, deliberately. An opt-out is the same footgun with a longer name: the
    # dangerous mode would still be one word away and would still leave no trace in the table it
    # wrote. Reproducing the old behaviour for an investigation needs no flag -- call `compare`
    # with `.pdata`-only extents and empty derived sets, exactly as `selftest` does.
    if args.leaf_extents:
        print(
            "--leaf-extents is deprecated and ignored: leaf extents are always derived now, so "
            "this run produces the same table the flag used to produce"
        )
    old_derived = add_leaf_extents(old_image, old_extents, old_starts, [p[0] for p in pairs])
    new_derived = add_leaf_extents(new_image, new_extents, new_starts, [p[1] for p in pairs])
    print(
        f"leaf extents decoded: {len(old_derived)} in 1.16.2, {len(new_derived)} in 1.17 "
        "(functions the .pdata table does not declare)"
    )

    rows = []
    for old_va, new_va, how in pairs:
        result = compare(
            old_image,
            new_image,
            old_va,
            new_va,
            old_extents,
            new_extents,
            old_derived,
            new_derived,
            old_starts,
            new_starts,
        )
        rows.append((old_va, new_va, how, result))
        diff = "" if result["first_diff"] is None else f", first diff at insn {result['first_diff']}"
        print(
            f"{old_va:#x} -> {new_va:#x}  {result['verdict']:<21} "
            f"{result['ratio']:.2f} over {result['compared']} insns{diff}  "
            f"{result['entry']}  {result['extents']}   [{how}]"
        )
        # The two verdicts whose evidence is not visible in the columns: what MinHook relocates
        # and where the difference actually sits, or MinHook's own reason for refusing the site
        # outright. A reader should never have to take either on the verdict's own word.
        if result["verdict"] in (PATCH_SITE_IDENTICAL, IDENTICAL_LEAF_NOPATCH):
            print(f"    patch site: {result['patch_site']}")
        # A refutation this run declined to make. Printed because withholding one silently would
        # be the same class of failure the withholding exists to prevent -- see
        # `refutation_withheld`. It is also the loudest signal available that `leaf_extent` has
        # gone wrong on a new shape, so it is worth a human's attention rather than a suppressed
        # verdict.
        if "refutation_withheld" in result:
            print(f"    refutation withheld: {result['refutation_withheld']}")

    # A row that compared its whole body is accepted on that basis alone; the instruction floor is
    # a stand-in for coverage and exhaustive coverage does not need one. Identical-leaf-NOPATCH
    # compared its whole body too -- it is refused the detour, not the comparison -- so counting it
    # as thin would report a proved row as unproved.
    def covered(result):
        return (
            result["verdict"] in EXHAUSTIVE_VERDICTS
            or result["verdict"] in CALLABLE_ONLY_VERDICTS
            or result["compared"] >= THIN_EVIDENCE
        )

    accepted = [r for r in rows if r[3]["ratio"] >= args.min_ratio and covered(r[3])]
    thin = [r for r in rows if r[3]["ratio"] >= args.min_ratio and not covered(r[3])]
    rejected = [r for r in rows if r[3]["ratio"] < args.min_ratio]
    print(
        f"\n{len(accepted)} accepted, {len(thin)} accepted-but-thin (<{THIN_EVIDENCE} insns), "
        f"{len(rejected)} rejected, of {len(rows)}"
    )
    # Counted separately because the ratio triage above cannot describe it: a body that grew has
    # a ratio below 1.0 by construction, so a patch-site-identical row lands in `rejected` there
    # while being detour-safe here. Reporting only the triage would read as a refusal.
    promoted = [r for r in rows if r[3]["verdict"] == PATCH_SITE_IDENTICAL]
    if promoted:
        print(
            f"{len(promoted)} row(s) {PATCH_SITE_IDENTICAL}: bodies differ, patch sites do not"
        )
        for old_va, new_va, _how, result in promoted:
            print(f"  {old_va:#x} -> {new_va:#x}  {result['patch_site']}")
    # The other verdict the triage above describes wrongly, and in the opposite direction: these
    # are accepted -- for calling -- and a reader who sees only "accepted" would take that as a
    # detour licence, which is the one thing this verdict withholds.
    callable_only = [r for r in rows if r[3]["verdict"] in CALLABLE_ONLY_VERDICTS]
    if callable_only:
        print(
            f"{len(callable_only)} row(s) {IDENTICAL_LEAF_NOPATCH}: whole body proved, "
            "CALL/READ only -- MinHook refuses the site and these must never carry a detour"
        )
        for old_va, new_va, _how, result in callable_only:
            print(f"  {old_va:#x} -> {new_va:#x}  {result['patch_site']}")

    if args.tsv:
        # Before the truncating open, not after: `--tsv` over an existing ledger is a wholesale
        # rewrite, and the rows it would not reproduce are exactly the hand-derived ones.
        carried, conflicts = preserve_unverified(args.tsv, rows)
        for old_va, new_va, other in sorted(conflicts):
            print(
                f"CONFLICT: {os.path.basename(args.tsv)} pairs {old_va:#x} with {new_va:#x}, "
                f"this run pairs it with {other:#x}"
            )
        if conflicts:
            print(
                f"REFUSING to write {args.tsv}: {len(conflicts)} row(s) disagree with this run. "
                "One of the two addresses is wrong and reads as live either way. Delete the row "
                "to accept this run's pair, or re-derive it -- do not let the truncate decide."
            )
            return 1
        carried_rows = [t for t in carried if not t.startswith("#") and t.strip()]
        for text in carried_rows:
            fields = text.split("\t")
            print(f"  carried forward (not in this run): {fields[0]} {fields[1]}")
        if carried:
            carried_prose = len(carried) - len(carried_rows)
            print(
                f"{len(carried_rows)} row(s) and {carried_prose} comment/blank line(s) already in "
                f"{os.path.basename(args.tsv)} were not produced by this run and are kept verbatim "
                "under '# CARRIED FORWARD'. Delete a line there to drop it; no run of this script "
                "will restore it."
            )
        with open(args.tsv, "w", encoding="utf-8") as handle:
            handle.write(
                "# 1.16.2 VA\t1.17 VA\tverdict\tratio\tinsns compared\thow it was mapped\t"
                "entry\textent\n"
            )
            handle.write(
                "# Written by scripts/verify-rva-map-1170.py --tsv, which TRUNCATES this file and\n"
                "# writes only what that run verified. Rows it did not produce are carried forward\n"
                "# verbatim under '# CARRIED FORWARD' at the foot and named on stderr, so a narrow\n"
                "# run cannot quietly delete a hand-derived pair -- but a row still leaves the\n"
                "# moment someone deletes its line, and nothing restores it.\n"
                + role_note(args.tsv)
                + "#\n"
                "# A verdict is evidence, not permission: IDENTICAL means the normalised\n"
                "# instruction sequences agree, which cannot see a change in what a called\n"
                "# function does.\n"
                "#\n"
                "# IDENTICAL vs IDENTICAL-WHOLE/IDENTICAL-LEAF is the difference between a PREFIX\n"
                "# and an EXHAUSTIVE comparison. IDENTICAL stopped somewhere -- at a `ret`, at the\n"
                "# instruction limit -- and says nothing about the instruction after it, which is\n"
                "# why er-game-base/build.rs holds it to MIN_VERIFIED_INSNS. The other two covered\n"
                "# every instruction of both bodies and found them the same length, so there is no\n"
                "# next instruction to be unsure about and no floor to impose. IDENTICAL-LEAF is\n"
                "# the same claim where the end had to be DECODED because neither image declares a\n"
                "# .pdata entry; it additionally carries the relocation check .pdata would have\n"
                "# stood in for. IDENTICAL-PREFIX is what a truncated comparison says instead of\n"
                "# lying: streams matched, coverage unknown, nothing accepts it.\n"
                "#\n"
                "# IDENTICAL-LEAF-NOPATCH is an IDENTICAL-LEAF that cannot be hooked: same whole-\n"
                "# body proof, same decoded extents, same relocation check -- and the body is\n"
                "# shorter than the five bytes MinHook writes, with MinHook's own ported rules\n"
                "# refusing the site in both images. er-game-base/build.rs admits it to\n"
                "# VERIFIED_1162_TO_1170 (CALL and READ) and to NOTHING else. It exists because\n"
                "# those two decisions used to be one: a three-byte getter was reported\n"
                "# IDENTICAL-SHORT, which withdrew it from comparing as well as from hooking.\n"
                "#\n"
                "# PATCH-SITE-IDENTICAL is none of those: the two bodies DIFFER, and the\n"
                "# difference is nowhere near the detour. Both images' .pdata declare a function\n"
                "# starting at the pair, the comparison covered both bodies in full, MinHook's own\n"
                "# trampoline walk (ported from vendor/minhook) succeeds at both and consumes the\n"
                "# same instructions, and every differing instruction lies strictly after the last\n"
                "# one it relocates -- so the patch overwrites the same prologue and the trampoline\n"
                "# returns into the same instruction. The difference is additionally capped at a\n"
                "# localised edit, and any substituted instruction refuses it outright. Re-run\n"
                "# `verify-rva-map-1170.py <va>` on such a row to see the hunks and the window.\n"
                "#\n"
                "# The entry column is the OTHER half of a detour's licence: whether each image's own\n"
                "# .pdata declares a function to start at that address. er-game-base/build.rs\n"
                "# requires BOTH-ENTRIES before a row may carry a detour, because IDENTICAL over a\n"
                "# body says the code is the same and says nothing about whether MinHook may\n"
                "# relocate the five bytes it is about to overwrite.\n"
                "#\n"
                "# The extent column shows where each body's end came from and, when the two\n"
                "# disagree, by how much: PDATA:0x120b/0x1213+8 is a body that GREW, which is\n"
                "# evidence of a change that costs no decoding to see.\n"
            )
            for old_va, new_va, how, result in rows:
                handle.write(
                    f"{old_va:#x}\t{new_va:#x}\t{result['verdict']}\t{result['ratio']:.3f}\t"
                    f"{result['compared']}\t{how}\t{result['entry']}\t{result['extents']}\n"
                )
            if carried:
                handle.write(
                    "#\n"
                    f"# CARRIED FORWARD -- {len(carried_rows)} row(s) and "
                    f"{len(carried) - len(carried_rows)} comment/blank line(s) the run that last\n"
                    "# wrote this file did not produce. `--tsv` TRUNCATES, so a pair verified in an\n"
                    "# earlier, narrower run -- `verify-rva-map-1170.py 0x1407ada40 --tsv <this\n"
                    "# file>` -- would otherwise disappear at exit 0 with nothing naming it, and the\n"
                    "# address would read afterwards as one that was never verified. Each row below\n"
                    "# is the verbatim line this verifier wrote when it was checked;\n"
                    "# er-game-base/build.rs reads them exactly like the rows above. The prose\n"
                    "# between them is carried for the same reason and matters as much: it is where\n"
                    "# this ledger records why a row was REMOVED, or is deliberately ABSENT, or was\n"
                    "# refused for being mid-function despite verifying IDENTICAL. Until 2026-09-01\n"
                    "# only rows were carried and 181 of those lines were dropped by one narrow run.\n"
                    "# To drop one, delete its line.\n"
                )
                for text in carried:
                    handle.write(text + "\n")
        print(f"wrote {args.tsv}")
    return 0


# Pairs whose patch-site-identical answer is settled by the two images, one clause each.
#
# `clause` names the one thing each pair is here to exercise. A control that fails for two reasons
# at once tests neither of them, so each refusal below was checked to fail on the clause named and
# to pass everything checked before it. The two acceptances are the control on the controls: a
# gate that refuses everything satisfies every refusal for free, and would be worse than no gate
# because it would read as strictness.
PATCH_SITE_CASES = (
    (
        0x140AF7CF0, 0x140AF9000, True, "MOVEMAPSTEP_STEP_MOVEMAP: 1.17 inserts `mov rcx,rbx; "
        "call _UpdateHorseType` at instruction 873 of 975, 0x1055 bytes past the prologue",
    ),
    (
        0x142226FD0, 0x142228FB0, True, "a second real acceptance, so the positive side is not one "
        "row: two inserted runs totalling 6 instructions, the first at 20",
    ),
    (
        0x140533C70, 0x140534BE0, False, "REPLACE: instruction 3 is `mov [rsp+..],rdi` in 1.16.2 "
        "and `mov [rsp+..],r9` in 1.17 -- a different register, which normalisation does not hide",
    ),
    (
        0x1404CA790, 0x1404CB280, False, "DRIFT: a single insertion, but 30 instructions of it "
        "into a 168-instruction body. Pure insert/delete, so only the size bound refuses it",
    ),
    (
        0x1403E7940, 0x1403E7970, False, "DRIFT again, from the other side of the NEAR line: one "
        "insertion of 21 instructions at index 340, ratio 0.971",
    ),
    (
        0x1403E81C0, 0x1403E8240, False, "HUNKS: four separate insertions. Localised is the claim; "
        "four places is not localised",
    ),
    (
        0x140AF7CF0, 0x140AF9008, False, "ENTRY: the accepted pair above with its DESTINATION "
        "moved 8 bytes into the body. One variable changes -- the 1.17 address stops being a "
        ".pdata start -- and the verdict must go with it",
    ),
    (
        0x140AEC480, 0x140AED790, False, "THE IMPOSTOR, and the SCOPE of this verdict rather than "
        "a demonstration of its strength: `IDENTICAL 1.000` over 56 instructions, 0x360 bytes "
        "INSIDE 0x140aec120..0x140aec567. Its two streams are EQUAL, so it is judged by the "
        "IDENTICAL family and never reaches this verdict at all. Pinned so nobody reads "
        "PATCH-SITE-IDENTICAL as the thing that stops it -- what stops it is that the row was "
        "deleted from the ledger",
    ),
)


def patch_site_selftest(old_image, new_image, old_extents, new_extents, old_starts, new_starts):
    """Prove [`PATCH_SITE_IDENTICAL`] decides, rather than merely agreeing with the answer.

    Returns a list of failure strings.

    The thing this has to rule out is a gate that says yes to everything, or one whose clauses have
    quietly stopped being reachable -- six audits in this repo were caught reporting false greens
    on 2026-08-30 alone, all of them by filtering to an empty set and then finding nothing wrong
    with it. So there are three layers, and the third is the one that matters:

      1. `PATCH_SITE_CASES`: real pairs from the two images, two accepted and five refused, each
         refusal landing on a named clause.
      2. the refusal reason is asserted, not just the boolean. A pair refused for the wrong reason
         is a gate that happens to agree with the answer.
      3. Mutation. Each clause is broken in turn -- the position test, the drift ceiling, the hunk
         ceiling, the entry requirement, MinHook's own walk -- and `STEP_MoveMap`, which passes
         all of them, must flip to a refusal, then flip back when the mutation is undone. A clause
         that cannot be made to fail is not doing anything.
    """
    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    def verdict_of(old_va, new_va):
        return compare(
            old_image, new_image, old_va, new_va, old_extents, new_extents,
            frozenset(), frozenset(), old_starts, new_starts,
        )

    # The window itself, pinned against MinHook's own numbers. Every clause below is stated
    # relative to "the instructions MinHook relocates", so a wrong count makes every one of them
    # measure the wrong thing while still looking decisive -- and a count faked to zero is the
    # most permissive possible window, refusing only a difference at instruction 0. Two sizes are
    # pinned so the value cannot be a constant that happens to fit.
    for label, image, extents, va, want_insns, want_bytes in (
        ("STEP_MoveMap 1.16.2", old_image, old_extents, 0x140AF7CF0, 3, 5),
        ("STEP_MoveMap 1.17", new_image, new_extents, 0x140AF9000, 3, 5),
        # A site where MinHook has to take a whole seven bytes, so a window hard-coded at
        # PATCH_BYTES would be wrong here and this case says so.
        ("0x140e6e060 1.16.2", old_image, old_extents, 0x140E6E060, 1, 7),
    ):
        end = extents.get(va - BASE)
        _, _, _, spans = decode_status(image, va, end_rva=end)
        insns, relocated, why = relocated_prefix(image, va, spans)
        check(f"{label}: MinHook relocates instructions", insns, want_insns)
        check(f"{label}: MinHook relocates bytes", relocated, want_bytes)
        check(f"{label}: and does not refuse the site", why, None)

    # Why `not derived` in `compare` cannot be mutation-tested, stated as a checked fact rather
    # than assumed. A derived extent belongs to an address `add_leaf_extents` accepted, and it
    # accepts only addresses no `.pdata` region describes -- so such an address can never be a
    # `.pdata` start, and the ENTRY_BOTH clause already excludes it. The `not derived` clause is
    # therefore defence in depth, and dropping it changes no verdict. That is only true while the
    # premise holds, so the premise is what gets asserted.
    for leaf in (0x14067A810, 0x14067A980, 0x140D4CC50, 0x1426634A0, 0x1404F9940):
        check(
            f"{leaf:#x} is a leaf, so it is not a .pdata start",
            entry_evidence(old_starts, new_starts, leaf, leaf) == ENTRY_BOTH,
            False,
        )

    accepted = 0
    for old_va, new_va, want, why in PATCH_SITE_CASES:
        result = verdict_of(old_va, new_va)
        got = result["verdict"] == PATCH_SITE_IDENTICAL
        accepted += 1 if got else 0
        check(f"{old_va:#x} -> {new_va:#x} ({why})", got, want)
    check("the acceptances in PATCH_SITE_CASES", accepted, 2)

    # The refusal reason, one clause at a time. Asserting only the boolean would pass on a gate
    # that refuses everything, and pass on a gate that refuses the right rows for the wrong cause.
    for old_va, new_va, fragment in (
        (0x140533C70, 0x140534BE0, "is REPLACED"),
        (0x1404CA790, 0x1404CB280, f"above the {MAX_DRIFT_INSNS} allowed"),
        (0x1403E81C0, 0x1403E8240, f"above the {MAX_DRIFT_HUNKS} allowed"),
        (0x140AF7CF0, 0x140AF9008, f"entry evidence is {ENTRY_DEST_NOT}"),
    ):
        note = verdict_of(old_va, new_va)["patch_site"]
        check(f"{old_va:#x} is refused for the right reason ({fragment})", fragment in note, True)

    # ------------------------------------------------------------------ Mutation
    # Break one clause, watch the known-good row fall, put it back, watch it stand again. The
    # restore half is not ceremony: a mutation that is never undone leaves the rest of the suite
    # testing a broken gate, and a "the control failed" that was going to fail anyway proves
    # nothing.
    good = (0x140AF7CF0, 0x140AF9000)
    check("the mutation control passes before anything is broken",
          verdict_of(*good)["verdict"], PATCH_SITE_IDENTICAL)

    mutations = []

    def mutate(name, apply_it, undo_it):
        apply_it()
        try:
            broke = verdict_of(*good)["verdict"] != PATCH_SITE_IDENTICAL
        finally:
            undo_it()
        mutations.append(name)
        check(f"MUTATION {name}: the control must FAIL while the clause is broken", broke, True)
        check(f"MUTATION {name}: and pass again once it is restored",
              verdict_of(*good)["verdict"], PATCH_SITE_IDENTICAL)

    keep_insns, keep_hunks = MAX_DRIFT_INSNS, MAX_DRIFT_HUNKS

    def set_insns(value):
        global MAX_DRIFT_INSNS
        MAX_DRIFT_INSNS = value

    def set_hunks(value):
        global MAX_DRIFT_HUNKS
        MAX_DRIFT_HUNKS = value

    mutate("drift ceiling", lambda: set_insns(1), lambda: set_insns(keep_insns))
    mutate("hunk ceiling", lambda: set_hunks(0), lambda: set_hunks(keep_hunks))

    # The position clause, which is the whole verdict and has no real counter-example in either
    # image -- the earliest first difference among all 128,602 pairs is 7 instructions in, and
    # MinHook relocates 1 to 3. So it is mutated instead: tell the gate the relocated window
    # swallows the entire body, and the difference is then inside the patch site by construction.
    #
    # The same lie on both sides, on purpose. The first draft of this mutation reported
    # `len(starts_at)`, which is 973 for 1.16.2 and 975 for 1.17 -- so the row was refused by the
    # count-equality clause, and the mutation passed while the position clause was deleted.
    # A mutation only tests the clause it is the sole possible cause of.
    real_prefix = globals()["relocated_prefix"]
    LONGER_THAN_ANY_BODY = 10_000

    def swallow(image, va, starts_at):
        insns, relocated, why = real_prefix(image, va, starts_at)
        return (None if insns is None else LONGER_THAN_ANY_BODY), relocated, why

    mutate(
        "position of the first difference",
        lambda: globals().__setitem__("relocated_prefix", swallow),
        lambda: globals().__setitem__("relocated_prefix", real_prefix),
    )

    # The clause that says the two walks consumed the same instructions. No pair in either image
    # exercises it -- MinHook relocates the same count on both sides everywhere it was measured --
    # so the skew is injected: one extra instruction claimed on the 1.17 side and nothing else
    # touched.
    def skew(image, va, starts_at):
        insns, relocated, why = real_prefix(image, va, starts_at)
        if insns is not None and image is new_image:
            insns += 1
        return insns, relocated, why

    mutate(
        "relocated-instruction-count equality",
        lambda: globals().__setitem__("relocated_prefix", skew),
        lambda: globals().__setitem__("relocated_prefix", real_prefix),
    )

    # MinHook's own walk, in two places, because breaking it in one leaves the other untested.
    #
    # First: does `patch_site_drift` react to a refusal at all? Replace the whole lookup.
    def refuse(image, va, starts_at):
        return None, 0, "MUTATED: MinHook refuses this site"

    mutate(
        "reaction to a refused patch site",
        lambda: globals().__setitem__("relocated_prefix", refuse),
        lambda: globals().__setitem__("relocated_prefix", real_prefix),
    )

    # Second, and finer: is `patch_safe` actually consulted inside `relocated_prefix`? Mutating
    # the wrapper above cannot tell -- it answers for the wrapper. This mutates the ported MinHook
    # rule itself, so a `relocated_prefix` that had stopped reading its answer stays green above
    # and goes red here. That is the shape two three-byte bodies exploited on 2026-08-30, when a
    # licence was issued for a function with nowhere to put the jump.
    audit = minhook_port()
    real_patch_safe = audit.patch_safe

    mutate(
        "MinHook's own patch_safe is consulted",
        lambda: setattr(audit, "patch_safe", lambda image, va: (False, "MUTATED: refused")),
        lambda: setattr(audit, "patch_safe", real_patch_safe),
    )

    # Full coverage. `covered` is the clause that says the comparison saw all of both bodies --
    # including the trailing bytes neither decode reached, which `residue_agrees` accounts for as
    # a relocated jump table. Without it a truncated comparison could carry this verdict, which is
    # the exact defect (`IDENTICAL` over 120 of 975 instructions) that made STEP_MoveMap
    # detour-safe in the first place. No pair in either image fails it while passing everything
    # else, so the residue check is made to disagree instead.
    real_residue = globals()["residue_agrees"]

    mutate(
        "full coverage of both bodies",
        lambda: globals().__setitem__("residue_agrees", lambda *a, **k: False),
        lambda: globals().__setitem__("residue_agrees", real_residue),
    )

    # The entry requirement, mutated by moving the destination 8 bytes into its own body. One
    # variable, and the cleanest available demonstration that the clause decides: the same source
    # address, the same bodies, the same instructions -- and the 1.17 address is no longer
    # something the linker declared a function to start at.
    inside = verdict_of(good[0], good[1] + 8)
    check("a destination 8 bytes into the body is refused",
          inside["verdict"] == PATCH_SITE_IDENTICAL, False)
    check("...and refused on the ENTRY clause, not incidentally",
          f"entry evidence is {ENTRY_DEST_NOT}" in inside["patch_site"], True)
    mutations.append("entry evidence")

    check("every clause was mutated", len(mutations), 8)
    print(
        f"patch site: {len(PATCH_SITE_CASES)} image-dictated cases agree, "
        f"{len(mutations)} clauses mutated and observed to fail and recover, "
        f"{len(failures)} failure(s)"
    )
    return failures


# The pairs [`IDENTICAL_LEAF_NOPATCH`] exists for: proved over their whole bodies, and refused a
# detour by MinHook itself. Both are three bytes. Both were `IDENTICAL-SHORT` before 2026-08-30,
# which took them out of the call map as well, and both have a live consumer that only ever needs
# the address for a comparison or a call.
NOPATCH_CASES = (
    (
        0x1407ADD70, 0x1407AEBF0, "MENU_ITEM_ACCEPT_IDLE: `33 c0 c3`, the constant-false accept "
        "predicate a CS::MenuItem row carries at +0xf8. er-quickload only compares against it",
    ),
    (
        0x141C92F30, 0x141C94D30, "CTRL_SUBOBJECT_RELEASE: `c2 00 00` (`ret 0`), which "
        "er-invasion-path CALLS while tearing an effect down",
    ),
)
# The five leaves that do fit the patch. They are the control on the new verdict: a rule that
# answered NOPATCH for every leaf would satisfy both cases above for free and would quietly
# withdraw five working detours, which is a worse failure than the one it fixes.
FITTING_LEAVES = {
    0x14067A810: 0x14067B660,
    0x14067A980: 0x14067B7D0,
    0x140D4CC50: 0x140D4E990,
    0x1426634A0: 0x142665CB0,
    0x1404F9940: 0x1404FA710,
}


def build_rs_lists_agree(path=None):
    """Do this file's verdict lists and `er-game-base/build.rs`'s still say the same thing?

    Returns a list of failure strings.

    The drift this closes was previously a comment asking two files to be changed together.
    `build.rs` holds three lists -- the detour-admitting `EXHAUSTIVE_VERDICTS` and
    `PATCH_SITE_VERDICTS`, and the call-only `CALLABLE_ONLY_VERDICTS` -- and this file writes the
    strings they match on. A verdict renamed here and not there is not an error anywhere: the
    rows simply stop being admitted, silently, which is how the leaves were lost the first time.

    It also asserts the property the whole verdict rests on, in the file that would have to be
    edited to break it: the call-only list is DISJOINT from both detour lists. That is what makes
    "cannot reach the detour table" a checked fact rather than a description of today's code.
    """
    if path is None:
        path = os.path.join(ROOT, "crates", "er-game-base", "build.rs")
    failures = []
    try:
        text = open(path, encoding="utf-8").read()
    except OSError as error:
        return [f"cannot read {path}: {error}"]
    import re

    def literal(name):
        match = re.search(rf"const {name}: \[&str; \d+\] = \[([^\]]*)\]", text)
        return None if match is None else frozenset(re.findall(r'"([^"]+)"', match.group(1)))

    build_exhaustive = literal("EXHAUSTIVE_VERDICTS")
    build_patch_site = literal("PATCH_SITE_VERDICTS")
    build_callable = literal("CALLABLE_ONLY_VERDICTS")
    for name, here, there in (
        ("EXHAUSTIVE_VERDICTS", EXHAUSTIVE_VERDICTS, build_exhaustive),
        ("PATCH_SITE_VERDICTS", frozenset((PATCH_SITE_IDENTICAL,)), build_patch_site),
        ("CALLABLE_ONLY_VERDICTS", CALLABLE_ONLY_VERDICTS, build_callable),
    ):
        if there is None:
            failures.append(f"build.rs has no {name}; a verdict list this file writes is unread")
        elif there != here:
            failures.append(f"{name} differs: this file {sorted(here)}, build.rs {sorted(there)}")
    if build_exhaustive is not None and build_patch_site is not None and build_callable is not None:
        overlap = build_callable & (build_exhaustive | build_patch_site)
        if overlap:
            failures.append(
                f"build.rs admits {sorted(overlap)} to BOTH the CALL-only list and a detour list, "
                "so a verdict that refuses a hook would license one"
            )
    return failures


def leaf_nopatch_selftest(old_image, new_image, old_extents, new_extents, old_starts, new_starts):
    """Prove [`IDENTICAL_LEAF_NOPATCH`] decides, one clause at a time.

    Returns `(failures, per_clause)` -- the failure strings, and `{clause: failures}` so a run can
    report which clause was not doing anything rather than only that something was wrong.

    The shape of the lie this rules out. A verdict that is issued unconditionally satisfies both
    `NOPATCH_CASES` for free, and would take the five leaves in `FITTING_LEAVES` with it -- five
    working detours withdrawn by a change advertised as adding two callable rows. So the positive
    cases are checked, then the five that must not get this verdict, then each clause is broken in
    turn and the control must lose the verdict and get it back. A clause that cannot be made to
    fail is not a clause.
    """
    failures = []
    per_clause = {}

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")
            return 1
        return 0

    leaf_old, leaf_new = dict(old_extents), dict(new_extents)
    old_derived = add_leaf_extents(old_image, leaf_old, old_starts, [p[0] for p in NOPATCH_CASES])
    new_derived = add_leaf_extents(new_image, leaf_new, new_starts, [p[1] for p in NOPATCH_CASES])

    def verdict_of(old_va, new_va, extents=None, derived=None):
        left, right = extents or (leaf_old, leaf_new)
        od, nd = derived or (old_derived, new_derived)
        return compare(
            old_image, new_image, old_va, new_va, left, right, od, nd, old_starts, new_starts
        )

    # The premises, asserted rather than assumed, so a future build that stops having this shape
    # says so instead of passing on a different one.
    audit = minhook_port()
    for old_va, new_va, why in NOPATCH_CASES:
        end_old, end_new = leaf_old[old_va - BASE], leaf_new[new_va - BASE]
        check(f"{old_va:#x} extent was DECODED, not declared", (old_va - BASE) in old_derived, True)
        check(f"{old_va:#x} body is under the patch size", end_old - (old_va - BASE) < PATCH_BYTES, True)
        check(f"{old_va:#x} both bodies are the same length",
              end_old - (old_va - BASE), end_new - (new_va - BASE))
        check(f"{old_va:#x} MinHook itself refuses 1.16.2", audit.patch_safe(old_image, old_va)[0], False)
        check(f"{new_va:#x} MinHook itself refuses 1.17", audit.patch_safe(new_image, new_va)[0], False)
        check(f"{old_va:#x} nothing branches into the 1.16.2 prologue",
              branch_into_prologue(old_image, old_va, end_old), False)
        check(f"{new_va:#x} nothing branches into the 1.17 prologue",
              branch_into_prologue(new_image, new_va, end_new), False)
        result = verdict_of(old_va, new_va)
        check(f"{old_va:#x} verdict ({why})", result["verdict"], IDENTICAL_LEAF_NOPATCH)
        check(f"{old_va:#x} is NOT admitted to a detour list",
              result["verdict"] in EXHAUSTIVE_VERDICTS or result["verdict"] == PATCH_SITE_IDENTICAL,
              False)
        check(f"{old_va:#x} IS admitted to the CALL list",
              result["verdict"] in CALLABLE_ONLY_VERDICTS, True)
        check(f"{old_va:#x} says WHY, in MinHook's words",
              "nowhere to put the jump" in result["patch_site"], True)
        check(f"{old_va:#x} extents agree on length", result["extent_delta"], 0)

    # ...AND the five it must not touch. Same family, same derived extents, room for the patch.
    fit_old, fit_new = dict(old_extents), dict(new_extents)
    fit_od = add_leaf_extents(old_image, fit_old, old_starts, list(FITTING_LEAVES))
    fit_nd = add_leaf_extents(new_image, fit_new, new_starts, list(FITTING_LEAVES.values()))
    for old_va, new_va in FITTING_LEAVES.items():
        result = verdict_of(old_va, new_va, (fit_old, fit_new), (fit_od, fit_nd))
        check(f"{old_va:#x} keeps its detourable leaf verdict", result["verdict"], IDENTICAL_LEAF)

    # ------------------------------------------------------------------ Mutation
    control_old, control_new = NOPATCH_CASES[0][0], NOPATCH_CASES[0][1]
    per_clause["control before any mutation"] = check(
        "the control carries the verdict before anything is broken",
        verdict_of(control_old, control_new)["verdict"],
        IDENTICAL_LEAF_NOPATCH,
    )

    def mutate(clause, apply_it, undo_it, becomes, run=None):
        """Break one clause; the control must lose the verdict, and regain it when restored.

        `becomes` is asserted, not just "not NOPATCH": a clause whose removal produced some other
        refusal has not been shown to be the clause that was deciding. Breaking the room clause in
        particular must produce identical-leaf -- the DETOURABLE verdict -- which is the whole
        thing it holds back, and asserting merely "not NOPATCH" would pass on a fallback too.
        """
        count = 0
        apply_it()
        try:
            broken = (run or verdict_of)(control_old, control_new)["verdict"]
        finally:
            undo_it()
        count += check(f"MUTATION {clause}: the control must lose the verdict", broken, becomes)
        count += check(
            f"MUTATION {clause}: and regain it once restored",
            verdict_of(control_old, control_new)["verdict"],
            IDENTICAL_LEAF_NOPATCH,
        )
        per_clause[clause] = count

    real_fits, real_branch = globals()["leaf_fits_patch"], globals()["branch_into_prologue"]
    real_residue, real_decode = globals()["residue_agrees"], globals()["decode_status"]
    real_patch_safe = audit.patch_safe

    # Room, and what it is holding back. With the length test satisfied the pair becomes
    # identical-leaf, which build.rs admits to DETOUR_SAFE_1162_TO_1170 -- five bytes into a
    # three-byte body. This mutation is the measurement of the cost of getting the clause wrong.
    mutate(
        "room: the body is shorter than the patch",
        lambda: globals().__setitem__("leaf_fits_patch", lambda *a, **k: True),
        lambda: globals().__setitem__("leaf_fits_patch", real_fits),
        IDENTICAL_LEAF,
    )
    # MinHook's own refusal, which is a separate claim from the length: it has a padding fallback
    # that can hook a sub-five-byte function, so a site it would accept must not be called NOPATCH.
    mutate(
        "MinHook's ported rules are consulted",
        lambda: setattr(audit, "patch_safe", lambda image, va: (True, "MUTATED: installable")),
        lambda: setattr(audit, "patch_safe", real_patch_safe),
        "IDENTICAL-SHORT",
    )
    # Prologue. A body something branches into is not cleared for anything, so it gets no verdict
    # of its own -- the refusal is not about room and must not be named after room.
    mutate(
        "prologue: nothing branches into the patched bytes",
        lambda: globals().__setitem__("branch_into_prologue", lambda *a, **k: True),
        lambda: globals().__setitem__("branch_into_prologue", real_branch),
        "IDENTICAL-SHORT",
    )
    # Full coverage. Without it a truncated comparison could carry the verdict, which is the
    # defect that made STEP_MoveMap detour-safe on 12% of its body.
    mutate(
        "coverage: the comparison saw all of both bodies",
        lambda: globals().__setitem__("residue_agrees", lambda *a, **k: False),
        lambda: globals().__setitem__("residue_agrees", real_residue),
        "IDENTICAL-SHORT",
    )

    # Equal EXTENTS. One variable: the 1.17 end moves by a byte, so the two decodes no longer
    # agree on the body's length and `whole_body` is false. No monkeypatch -- the extent tables
    # are this function's own inputs. The 1.17 stream gains an instruction the 1.16.2 one has not,
    # so the raw comparison lands on DIVERGES: a length disagreement is not a near miss.
    #
    # And then it is withheld, which is the half this test now also pins. `build.rs::
    # refuted_sources()` subtracts a DIVERGES row from both maps, so under the old behaviour a
    # leaf whose end was decoded one byte long did not merely lose its verdict, it deleted the
    # address -- and it deleted it on the strength of a boundary this file inferred. That is
    # exactly what happened to LOADING_SCREEN_GFX_FADEOUT_RVA when the sweep over-ran a
    # de-Arxan'd gap. `refutation_withheld` now re-judges such a pair with the decoded extent
    # withdrawn, so a wrong decode costs a dropped row (identical-short, accepted nowhere,
    # subtracted from nowhere) instead of a deleted one. The clause is still load-bearing: the
    # verdict is still lost.
    skewed_new = dict(leaf_new)
    skewed_new[control_new - BASE] += 1
    skewed = verdict_of(control_old, control_new, (leaf_old, skewed_new))
    per_clause["extents: both decodes agree on the length"] = check(
        "MUTATION extents: a 1-byte disagreement loses the verdict",
        skewed["verdict"],
        "IDENTICAL-SHORT",
    ) + check(
        "MUTATION extents: ...without deleting the address",
        skewed["verdict"] == REFUTED,
        False,
    ) + check(
        "MUTATION extents: ...and the withheld refutation is reported",
        "refutation_withheld" in skewed,
        True,
    ) + check(
        "MUTATION extents: and the unskewed table still carries it",
        verdict_of(control_old, control_new)["verdict"],
        IDENTICAL_LEAF_NOPATCH,
    )

    # Leaf provenance, and the sharpest measurement in this block. Told that the same two extents
    # came from `.pdata` rather than from a decode, the pair takes the byte-comparison shortcut and
    # comes back byte-identical -- which `build.rs` admits to DETOUR_SAFE_1162_TO_1170 with no
    # length check anywhere in its path. The bodies really are byte-equal; three bytes of them.
    # So the `derived` flag is the only thing standing between this address and a five-byte jmp
    # written into a three-byte function, and the room clause is reachable only through it.
    #
    # That is a live shape, not a hypothetical: both images declare 110 `.pdata` regions shorter
    # than five bytes, so a real function can be too short and still be judged by the `.pdata`
    # family. Measured 2026-08-30 across both verdict ledgers: of the 444 rows currently admitted
    # to a detour, zero have a `.pdata` extent under five bytes and zero are refused by the ported
    # `patch_safe`. The hole is empty today; it is not closed, and it is not this verdict's to
    # close -- widening the length check to the `.pdata` families would move the detour count.
    per_clause["provenance: the extents were DECODED, not declared"] = check(
        "MUTATION provenance: a leaf presented as a .pdata function takes the .pdata verdict",
        verdict_of(control_old, control_new, None, (frozenset(), frozenset()))["verdict"],
        "BYTE-IDENTICAL",
    ) + check(
        "MUTATION provenance: and the real provenance still carries the verdict",
        verdict_of(control_old, control_new)["verdict"],
        IDENTICAL_LEAF_NOPATCH,
    )

    # Streams equal. The leaf branch is only reached when the two normalised streams match, so the
    # clause is structural and has no counter-example among three-byte bodies that are all
    # `xor eax,eax; ret` or `ret 0`. One instruction of the 1.17 decode is altered instead.
    def altered(image, va, limit=DECODE_LIMIT, end_rva=None):
        insns, stop, reached, spans = real_decode(image, va, limit=limit, end_rva=end_rva)
        if image is new_image and insns:
            insns = ["MUTATED"] + list(insns[1:])
        return insns, stop, reached, spans

    mutate(
        "streams: the two bodies are equal instruction for instruction",
        lambda: globals().__setitem__("decode_status", altered),
        lambda: globals().__setitem__("decode_status", real_decode),
        "DIVERGES",
    )

    # The lists this verdict's separation lives in, checked in the file that would have to be
    # edited to break it.
    drift = build_rs_lists_agree()
    failures.extend(drift)
    per_clause["build.rs verdict lists agree and stay disjoint"] = len(drift)

    print(f"leaf NOPATCH: {len(NOPATCH_CASES)} accepted, {len(FITTING_LEAVES)} fitting leaves "
          f"unaffected, {len(per_clause)} clause(s) exercised")
    for clause, count in per_clause.items():
        print(f"    clause {clause}: {count} failure(s)")
    return failures, per_clause


def selftest():
    """Pin the decode boundary, and record that one 2026-08-29 verdict was retracted.

    The RETRACTION, because a test that asserts a wrong answer is worse than no test. This
    selftest used to require `HUD_WEAPON_SLOT_UPDATE` (0x1408d2110 -> 0x1408d32b0) to come back
    `DIVERGES` at "18% of its instruction shape", and treated that as the lesson of the
    2026-08-29 crash bisect. The 18% was an artifact of this FILE: the function is 86 bytes and
    ends in a tail-call `jmp`, `decode()` stopped only at `ret`, and roughly 98 of the 120
    instructions it compared belonged to the next function. Bounded by the `.pdata` extent the two
    bodies differ in four bytes, both of them halves of `call rel32` displacements, and the
    verdict is identical. Three independent reviews found the same artifact behind 12 of 12
    non-clean rows, with zero changed immediates and zero changed struct offsets among them.

    What is not RETRACTED: the crash was real. Its cause is now unknown and must be re-derived --
    an independent look at the same run found the game dying in FromSoftware's own `DL_PANIC`
    ("未初期化のシングルトンにアクセスしました", FD4Singleton.h) for an uninitialised singleton,
    which points at a stale `.data` global rather than at a detour. Do not read the identical
    verdict below as "that address was fine all along"; read it as "the reason we gave was wrong".

    So what is asserted here is the boundary, which is the thing that was actually broken: a
    function ending in a tail call must compare over its own body and no further.
    """
    old_image = open(OLD_IMAGE, "rb").read()
    new_image = open(NEW_IMAGE, "rb").read()
    old_starts = function_starts(old_image)
    new_starts = function_starts(new_image)
    old_extents = function_extents(old_image)
    new_extents = function_extents(new_image)

    failures = []

    def check(name, got, want):
        if got != want:
            failures.append(f"{name}: got {got!r}, want {want!r}")

    # `--tsv` TRUNCATES, and the rows it would not reproduce are the hand-derived ones.
    # Asserted on a temporary file so it keeps holding once the real ledger's hand rows are all
    # reproducible. The pair used is the one a merge agent hand-derived into `verified.tsv` on
    # 2026-08-30 -- absent from the default candidate map, and so absent from any full run.
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "verified.tsv")
        hand = "0x140764290\t0x1407650e0\tIDENTICAL-WHOLE\t1.000\t79\tby hand\tBOTH-ENTRIES\tPDATA"
        with open(path, "w", encoding="utf-8") as handle:
            handle.write("# header\n")
            handle.write("0x1408d0900\t0x1408d1aa0\tIDENTICAL-WHOLE\t1.000\t297\tx\ty\tz\n")
            handle.write(hand + "\n")
        this_run = [(0x1408D0900, 0x1408D1AA0, "x", {})]
        carried, clash = preserve_unverified(path, this_run)
        check("a hand-derived row survives a narrower --tsv run", carried, [hand])
        check("an agreeing row is not carried twice", clash, [])
        # And a row the run pairs somewhere else is a conflict, not something to merge.
        moved = [(0x1408D0900, 0x140999999, "x", {})]
        _keep, clash2 = preserve_unverified(path, moved)
        check(
            "a contradicting pair raises a conflict",
            clash2,
            [(0x1408D0900, 0x1408D1AA0, 0x140999999)],
        )
        check("a missing target carries nothing", preserve_unverified(path + ".nope", this_run), ([], []))

    # Populated, and populated with functions. Roughly a quarter of the 235,823 `.pdata` entries
    # in 1.16.2 -- 60,624 of them -- are continuation chunks of a function that starts elsewhere,
    # so a table read without the chain flag overstates the function count by 35% and hands
    # `entry_evidence` tens of thousands of mid-function addresses to call function starts.
    check("1.16.2 .pdata is populated", len(old_starts) > 150_000, True)
    check("1.17 .pdata is populated", len(new_starts) > 150_000, True)
    for name, image, starts in (
        ("1.16.2", old_image, old_starts),
        ("1.17", new_image, new_starts),
    ):
        raw = runtime_functions(image)
        check(f"{name} has chunked functions to exclude", len(raw) - len(starts) > 50_000, True)
        check(f"{name} keeps most entries", len(starts) > len(raw) * 0.7, True)

    # The four that are genuinely the same function, and the one that is not.
    same = {
        0x1408D0900: 0x1408D1AA0,  # HUD_SCENE_UPDATE
        0x1408D1D00: 0x1408D2EA0,  # HUD_WEAPON_SLOT_CTOR
        0x1408D1E30: 0x1408D2FD0,  # HUD_CHILD_BINDER
        0x1408FF470: 0x140900610,  # TILE_POPULATE
    }
    for old_va, new_va in same.items():
        result = compare(
            old_image, new_image, old_va, new_va, old_extents, new_extents
        )
        # These read `IDENTICAL` until 2026-08-30 and now read `IDENTICAL-WHOLE`, which is not a
        # relabelling: the decode used to stop at 120 instructions and now runs to each body's
        # declared end. HUD_SCENE_UPDATE is 297 instructions, so the old verdict covered 40% of it.
        check(f"{old_va:#x} verdict", result["verdict"], IDENTICAL_WHOLE)
        check(f"{old_va:#x} extents agree on length", result["extent_delta"], 0)
        check(
            f"{old_va:#x} entry",
            entry_evidence(old_starts, new_starts, old_va, new_va),
            ENTRY_BOTH,
        )

    killer = compare(
        old_image, new_image, 0x1408D2110, 0x1408D32B0, old_extents, new_extents
    )
    # Retracted 2026-08-29 verdict -- see this function's docstring. Bounded by .pdata, the two
    # bodies agree; the old DIVERGES was this file decoding the following function.
    check("HUD_WEAPON_SLOT_UPDATE verdict", killer["verdict"], IDENTICAL_WHOLE)
    check("HUD_WEAPON_SLOT_UPDATE compares its own body only", killer["compared"] <= 40, True)
    # The regression guard that matters. A tail-call function must not decode past its own end.
    # Without the .pdata bound this returned ~120 instructions for an 86-byte body.
    tail_call_body = decode(
        old_image,
        0x1408D2110,
        end_rva=old_extents.get(0x1408D2110 - BASE),
    )
    check("tail-call body stays inside its .pdata extent", len(tail_call_body) <= 40, True)
    check("tail-call body is not empty", len(tail_call_body) > 0, True)

    # Short functions the extent rule rescues: 21 and 38 bytes, byte-for-byte unchanged in 1.17.
    # Before extents were read these were identical-short over 7 instructions and excluded, which
    # is what left er-armament-icons' PROXY_IS_BOUND detour refused on a function that did not
    # change at all.
    for old_va, new_va in ((0x140733150, 0x140733FA0), (0x140733EF0, 0x140734D40)):
        whole = compare(
            old_image, new_image, old_va, new_va, old_extents, new_extents
        )
        check(f"{old_va:#x} whole-function verdict", whole["verdict"], "BYTE-IDENTICAL")

    # A mid-function address is not an entry, whatever the bytes around it say. Five bytes into a
    # known function start is by construction not a function start.
    midway = entry_evidence(old_starts, new_starts, 0x1408D0905, 0x1408D1AA5)
    check("mid-function pair", midway, ENTRY_NEITHER)

    # The truncated comparison that read as a clean one. `STEP_MoveMap` is the only pair in either
    # verdict table whose two `.pdata` extents differ in length -- 0x120b against 0x1213 -- and
    # 1.17 spends those 8 bytes on two instructions inserted at index 873 of 975. Decoding 120 and
    # calling it `IDENTICAL 1.000` promoted it into DETOUR_SAFE_1162_TO_1170 on 12% of the body.
    #
    # The preconditions are asserted first so that a future build which no longer has this shape
    # says so, instead of passing vacuously.
    move_map_old, move_map_new = 0x140AF7CF0, 0x140AF9000
    old_len = old_extents[move_map_old - BASE] - (move_map_old - BASE)
    new_len = new_extents[move_map_new - BASE] - (move_map_new - BASE)
    check("STEP_MoveMap 1.16.2 extent", old_len, 0x120B)
    check("STEP_MoveMap 1.17 extent is 8 bytes longer", new_len - old_len, 8)
    move_map = compare(
        old_image, new_image, move_map_old, move_map_new, old_extents, new_extents
    )
    check("STEP_MoveMap compares its whole body", move_map["compared"] > 900, True)
    check("STEP_MoveMap extent delta is reported", move_map["extent_delta"], 8)
    check("STEP_MoveMap extent note", move_map["extents"], "PDATA:0x120b/0x1213+8")
    # Not identical -- a body that gained two instructions is not the same body...
    check("STEP_MoveMap is not identical", move_map["verdict"] in EXHAUSTIVE_VERDICTS, False)
    check("STEP_MoveMap is not IDENTICAL either", move_map["verdict"] == "IDENTICAL", False)
    # ...and not refuted, which matters just as much. `build.rs::refuted_sources()` keys on the
    # literal string DIVERGES and subtracts such a row from the call map, and an index-against-
    # index ratio scores an insertion at 0.898 and lands there. Aligned, the same pair scores
    # 0.999.
    #
    # Near is what the comparison says without the `.pdata` start sets, and it is pinned here
    # because that is the fail-closed path: a caller who does not supply them cannot be handed a
    # detour licence by accident. The verdict with them supplied is asserted below.
    check("STEP_MoveMap without .pdata starts is NEAR", move_map["verdict"], "NEAR")
    check("STEP_MoveMap alignment-aware ratio", move_map["ratio"] > 0.99, True)
    check(
        "and it says WHY the patch-site verdict was unreachable",
        "no .pdata start sets supplied" in move_map["patch_site"],
        True,
    )

    failures.extend(
        patch_site_selftest(old_image, new_image, old_extents, new_extents, old_starts, new_starts)
    )

    # The leaf, and the whole point of `--leaf-extents`. These five have no `.pdata` entry in
    # either image -- MSVC emits no unwind data for a function that allocates no stack and calls
    # nothing -- so no extent can be read, no byte-identical can be awarded, and their 3 to 13
    # instructions cannot clear MIN_VERIFIED_INSNS. Each is byte-for-byte or shape-for-shape
    # unchanged over its entire body and each was being discarded anyway.
    leaves = {
        0x14067A810: 0x14067B660,  # GameMan save-slot setter
        0x14067A980: 0x14067B7D0,  # er-save-suppress quit-phase settle
        0x140D4CC50: 0x140D4E990,  # GET_PARAM_RESCAP
        0x1426634A0: 0x142665CB0,  # er-input-harness DLUID writer
        0x1404F9940: 0x1404FA710,  # SpecialEffect::HasSpecialEffectId (the Seamless guard)
    }
    old_regions, new_regions = pdata_regions(old_image), pdata_regions(new_image)
    for old_va, new_va in leaves.items():
        check(f"{old_va:#x} has no .pdata extent in 1.16.2", (old_va - BASE) in old_extents, False)
        check(f"{old_va:#x} has no .pdata extent in 1.17", (new_va - BASE) in new_extents, False)
        # The premise the verdict actually rests on, which "no entry begins here" is not. A leaf
        # is a function the linker described nowhere; an address merely lacking an entry of its
        # own can be the interior of one, and the interior is where a detour must never land.
        check(
            f"{old_va:#x} is described by no .pdata region in 1.16.2",
            inside_pdata(old_regions, old_va - BASE),
            False,
        )
        check(
            f"{new_va:#x} is described by no .pdata region in 1.17",
            inside_pdata(new_regions, new_va - BASE),
            False,
        )
        # Default behaviour, unchanged: no extent, so a short body is reported as short.
        default = compare(
            old_image, new_image, old_va, new_va, old_extents, new_extents
        )
        check(
            f"{old_va:#x} is exhaustive by default",
            default["verdict"] in EXHAUSTIVE_VERDICTS,
            False,
        )

    # With --leaf-extents the ends are derived, and the verdict says so by name.
    leaf_old = dict(old_extents)
    leaf_new = dict(new_extents)
    old_derived = add_leaf_extents(old_image, leaf_old, old_starts, list(leaves))
    new_derived = add_leaf_extents(new_image, leaf_new, new_starts, list(leaves.values()))
    for old_va, new_va in leaves.items():
        derived = compare(
            old_image, new_image, old_va, new_va, leaf_old, leaf_new, old_derived, new_derived
        )
        check(f"{old_va:#x} leaf verdict", derived["verdict"], IDENTICAL_LEAF)
        # The two images were decoded independently and agreed on the byte length. That agreement
        # is most of why a decoded extent is trustworthy at all, so it is asserted rather than
        # assumed.
        check(f"{old_va:#x} leaf extents agree on length", derived["extent_delta"], 0)
        check(f"{old_va:#x} leaf extent note", derived["extents"].startswith("LEAF:"), True)
        # A leaf carries no .pdata entry, so it reaches the detour gate through neither-entry --
        # a clause written for a different case. `branch_into_prologue` supplies the claim that
        # clause does not make: the five bytes MinHook overwrites are not a branch target.
        check(
            f"{old_va:#x} prologue is relocatable in 1.16.2",
            branch_into_prologue(old_image, old_va, leaf_old[old_va - BASE]),
            False,
        )
        check(
            f"{old_va:#x} prologue is relocatable in 1.17",
            branch_into_prologue(new_image, new_va, leaf_new[new_va - BASE]),
            False,
        )
    # A leaf too short to patch is not DETOURABLE, however completely it was compared, and until
    # 2026-08-30 it was not CALLABLE either -- which is the defect `IDENTICAL-LEAF-NOPATCH` exists
    # to separate. Clause-by-clause, with each clause broken and observed to fail, next door.
    #
    # Without the `.pdata` start sets, the fail-closed path: those are what `add_leaf_extents`
    # needs, so the pair is judged with no extent at all and comes back short. Pinned here so a
    # caller who omits them can never be handed a verdict by accident.
    for old_va, new_va, _why in NOPATCH_CASES:
        bare = compare(old_image, new_image, old_va, new_va, old_extents, new_extents)
        check(f"{old_va:#x} without derived extents is IDENTICAL-SHORT", bare["verdict"], "IDENTICAL-SHORT")
    nopatch_failures, _clauses = leaf_nopatch_selftest(
        old_image, new_image, old_extents, new_extents, old_starts, new_starts
    )
    failures.extend(nopatch_failures)
    # The control: every leaf the accepted block above takes is at or over the patch size, so the
    # length floor refuses only what it was written to refuse.
    for old_va, new_va in leaves.items():
        check(
            f"{old_va:#x} accepted leaf still fits the patch",
            leaf_fits_patch(old_va - BASE, leaf_old[old_va - BASE])
            and leaf_fits_patch(new_va - BASE, leaf_new[new_va - BASE]),
            True,
        )

    # A continuation wearing an entry'S clothes, and the reason `add_leaf_extents` asks whether a
    # `.pdata` region contains an address rather than whether one begins at it.
    #
    # 1.16.2 `0xc57666` is a `.pdata` record carrying `UNW_FLAG_CHAININFO`: the middle of the
    # function that starts 0x86 bytes earlier at `0xc575e0`. `function_regions` correctly refuses
    # to call it a start and correctly merges its bytes into the primary's run -- which means it
    # appears in neither `starts` nor `extents`, and the old skip (`rva in extents`) therefore let
    # it through. It would have taken a decoded extent of 0xc57666..0xc576ae and been eligible for
    # `IDENTICAL-LEAF`, the one verdict that issues its own detour licence, with `NEITHER-ENTRY`
    # raising no objection because neither side is an entry. A hook into the middle of a function,
    # arrived at through two correct decisions.
    #
    # It is not hypothetical: `scripts/classify-1170-entry-kind.py` calls this address entry and it
    # is already a row in functions.tsv. The preconditions are asserted first, so that if the game
    # ever stops chaining here the test says so rather than passing on a changed premise.
    chained_continuation = 0x140C57666
    continuation_rva = chained_continuation - BASE
    check("continuation is not a .pdata start", continuation_rva in old_starts, False)
    check("continuation is not a merged extent key", continuation_rva in old_extents, False)
    check("continuation IS inside a .pdata region", inside_pdata(old_regions, continuation_rva), True)
    # The old rule's answer, kept as the measurement rather than as prose.
    check(
        "an unguarded decode would have found an extent here",
        leaf_extent(old_image, chained_continuation, old_starts) is not None,
        True,
    )
    check(
        "add_leaf_extents refuses the continuation",
        add_leaf_extents(old_image, dict(old_extents), old_starts, [chained_continuation]),
        set(),
    )
    # ...and the control, so the guard is refusing the interior rather than everything: the
    # primary this continuation belongs to is a declared start and is refused for that reason.
    check(
        "add_leaf_extents refuses the primary too",
        add_leaf_extents(old_image, dict(old_extents), old_starts, [0x140C575E0]),
        set(),
    )

    # A leaf whose extent had to be decoded never claims byte-identical even when the bytes are
    # equal (0xd4cc50, 0x26634a0 and 0x4f9940 all are). Equal bytes over two extents derived by
    # the same rule prove nothing about a body that rule truncated, so the provenance stays in the
    # name.
    byte_equal = compare(
        old_image, new_image, 0x140D4CC50, 0x140D4E990, leaf_old, leaf_new, old_derived, new_derived
    )
    check("byte-equal leaf keeps its LEAF verdict", byte_equal["verdict"], IDENTICAL_LEAF)

    # The de-ARXAN'D GAP: a `jmp` followed by DEOBFUSCATOR leftovers must end the body.
    #
    # `leaf_extent` used to end a body at a `jmp` only when the byte after it was `0xCC`/`0x90`
    # padding or a declared `.pdata` start. Between functions in these images that byte is
    # routinely neither -- the de-Arxan pass leaves its own residue in the gaps -- so the sweep
    # walked through the gap into the next function and compared unrelated code. The bodies below
    # are 23 bytes; the sweep took 0x45 and 0x25 and reported `DIVERGES` at 0.86 and 0.75.
    #
    # What is pinned, and why it is not simply "the verdict is right today":
    #   * the terminator'S ground truth. The byte after each body is asserted to be neither a pad
    #     byte nor a declared start, so the old rule provably cannot fire here and the clause under
    #     test is the only thing that can end the body. Without this the test would pass on an
    #     image where the gaps happened to be padded, which is the shape that hid the bug.
    #   * the length, as a frozen literal. 0x17 is 23, which is what Ghidra independently reports
    #     for these functions in both dumps. It is written here rather than recomputed from
    #     `leaf_extent`, because a number the code under test produced cannot check that code.
    #   * the old answer, also frozen. 0x45 and 0x25 are what the previous rule returned, measured
    #     before it was replaced, and they are used below to drive the fail-closed path.
    #
    # Ghidra's sizes are the outside check: 1.16.2 and 1.17 both put these two functions at 23
    # bytes, and the two images' decodes agree with that and with each other.
    GAP_LEAVES = (
        # 1.16.2, 1.17, true extent, what the old padding-only rule returned
        (0x14090A0A0, 0x14090B240, 0x17, 0x45),  # LOADING_SCREEN_GFX_FADEOUT_RVA
        (0x14090A0C0, 0x14090B260, 0x17, 0x25),  # KNOWLEDGE_TIP_ADVANCE_ENABLED_RVA
    )
    for old_va, new_va, extent, _overrun in GAP_LEAVES:
        gap_old, gap_new = dict(old_extents), dict(new_extents)
        derived_old = add_leaf_extents(old_image, gap_old, old_starts, [old_va])
        derived_new = add_leaf_extents(new_image, gap_new, new_starts, [new_va])
        for label, image, starts, regions, va, ends in (
            ("1.16.2", old_image, old_starts, old_regions, old_va, gap_old),
            ("1.17", new_image, new_starts, new_regions, new_va, gap_new),
        ):
            rva = va - BASE
            # The premise a derived extent rests on: the linker described no function here.
            check(f"{va:#x} is described by no .pdata region in {label}", inside_pdata(regions, rva), False)
            end = ends.get(rva)
            check(f"{va:#x} leaf extent in {label}", None if end is None else end - rva, extent)
            # The ground truth that broke the old rule. Neither test it applied can fire here.
            check(
                f"{va:#x} is followed by deobfuscator leftovers, not padding, in {label}",
                end is not None and image[end] in FUNCTION_PAD_BYTES,
                False,
            )
            check(
                f"{va:#x} is not followed by a declared .pdata start in {label}",
                end is not None and end in starts,
                False,
            )
        gap = compare(
            old_image, new_image, old_va, new_va, gap_old, gap_new, derived_old, derived_new,
            old_starts, new_starts,
        )
        check(f"{old_va:#x} gap-leaf verdict", gap["verdict"], IDENTICAL_LEAF)
        check(f"{old_va:#x} gap-leaf extents", gap["extents"], f"LEAF:{extent:#x}/{extent:#x}")
        check(f"{old_va:#x} gap-leaf extents agree on length", gap["extent_delta"], 0)
        # ...and it is not quietly the fail-closed path dressed as a pass.
        check(f"{old_va:#x} gap-leaf withheld nothing", "refutation_withheld" in gap, False)

    # The control for the same LANDMINE, and it is a control for a different reason.
    # PLAYER_GAME_DATA_NAME_GETTER_RVA was in the same rescued set of three, but its body ends on a
    # real `0xCC` pad byte, so the old rule got it right and the sweep was never the problem there.
    # It was lost to the OPT-in alone: no derived extent, no identical-leaf, identical-short, gone.
    # Pinned so the two failure modes stay told apart -- fixing the sweep would not have saved it,
    # and making derivation unconditional is what does.
    pad_old, pad_new = 0x14025F8E0, 0x14025F8F0
    padded_old, padded_new = dict(old_extents), dict(new_extents)
    pad_derived_old = add_leaf_extents(old_image, padded_old, old_starts, [pad_old])
    pad_derived_new = add_leaf_extents(new_image, padded_new, new_starts, [pad_new])
    pad_end = padded_old[pad_old - BASE]
    check("padded leaf extent", pad_end - (pad_old - BASE), 0xC)
    check("padded leaf really is followed by padding", old_image[pad_end] in FUNCTION_PAD_BYTES, True)
    padded = compare(
        old_image, new_image, pad_old, pad_new, padded_old, padded_new,
        pad_derived_old, pad_derived_new, old_starts, new_starts,
    )
    check("padded leaf verdict", padded["verdict"], IDENTICAL_LEAF)
    check("padded leaf extents", padded["extents"], "LEAF:0xc/0xc")

    # A DECODED boundary may not delete an address. `refutation_withheld`, driven with the exact
    # extents the old sweep produced, so the failure being guarded against is reproduced rather
    # than imagined: 0x45 bytes on both sides of the fadeout pair, which is the 23-byte thunk plus
    # 9 bytes of gap plus the whole of the next thunk plus 9 more bytes of gap.
    #
    # The second half is the point. The same over-long extents, presented as though `.pdata` had
    # declared them, must still come back refuted -- because then the refutation rests on the
    # image's own statement about where the function ends, not on this file's guess. If both halves
    # answered the same way the rule would be doing nothing, and this test would be measuring the
    # image instead of the code.
    overrun_old, overrun_new, _extent, overrun = GAP_LEAVES[0]
    o_rva, n_rva = overrun_old - BASE, overrun_new - BASE
    bad_old = {o_rva: o_rva + overrun}
    bad_new = {n_rva: n_rva + overrun}
    withheld = compare(
        old_image, new_image, overrun_old, overrun_new, bad_old, bad_new,
        frozenset((o_rva,)), frozenset((n_rva,)), old_starts, new_starts,
    )
    check("an over-run DECODED extent does not refute", withheld["verdict"] == REFUTED, False)
    check("...it falls back to the undecoded answer", withheld["verdict"], "IDENTICAL-SHORT")
    check("...and says so out loud", "refutation_withheld" in withheld, True)
    declared = compare(
        old_image, new_image, overrun_old, overrun_new, bad_old, bad_new,
        frozenset(), frozenset(), old_starts, new_starts,
    )
    check("the same extents DECLARED by .pdata still refute", declared["verdict"], REFUTED)

    # The THUNK, the boundary artifact rules 1-3 cannot reach (`decode` rule 4).
    # `UPDATE_TROPHY_STATS_RVA` is a 5-byte `jmp` at the same address in both images with the same
    # bytes, and it used to come back `DIVERGES 0.05 over 22 insns, first diff at insn 1` -- insn 1
    # being the first instruction of the next thunk. The preconditions are asserted first, so that
    # if the game ever stops having a thunk here the test says so instead of quietly passing on a
    # different shape.
    thunk_rva = 0x24A1A0
    check(
        "thunk bytes are identical in both images",
        old_image[thunk_rva : thunk_rva + 5] == new_image[thunk_rva : thunk_rva + 5],
        True,
    )
    check("thunk has no .pdata extent in 1.16.2", thunk_rva in old_extents, False)
    check("thunk has no .pdata extent in 1.17", thunk_rva in new_extents, False)
    # Rule 3 cannot fire: the next thunk is packed flush against this one, so the byte after the
    # `jmp` is code (0x8b in 1.16.2, 0x33 in 1.17), not padding.
    check(
        "byte after the thunk is not padding in either image",
        old_image[thunk_rva + 5] in FUNCTION_PAD_BYTES
        or new_image[thunk_rva + 5] in FUNCTION_PAD_BYTES,
        False,
    )
    for name, image in (("1.16.2", old_image), ("1.17", new_image)):
        check(f"thunk decodes to one instruction in {name}", len(decode(image, BASE + thunk_rva)), 1)
    thunk = compare(
        old_image, new_image, BASE + thunk_rva, BASE + thunk_rva, old_extents, new_extents
    )
    # The point of the whole exercise: `build.rs::refuted_sources()` keys on the literal string
    # DIVERGES and subtracts such a row from the call map. A decoding artifact must never be able
    # to delete an address that is byte-for-byte unchanged.
    check("thunk verdict", thunk["verdict"], "IDENTICAL-SHORT")

    # Negative controls for rule 4: a `jmp` in the middle of a body must not stop the decode. Both
    # are `.pdata`-declared functions, decoded with `end_rva=None` so that only rules 2-4 are in
    # play; the invariant is that the unbounded decode reaches exactly as far as the image's own
    # extent takes it. Truncating either is how rule 4 would manufacture a false verdict of its
    # own, so this is the guard against over-stopping.
    #   0x140001120 branches forward to a shared epilogue -- the jmp's own destination raises the
    #     watermark past the following byte.
    #   0x140002020 is a loop whose back-edge is followed by code an earlier forward branch
    #     reaches, which raised the watermark first.
    for control_va in (0x140001120, 0x140002020):
        end = old_extents.get(control_va - BASE)
        check(f"{control_va:#x} has a .pdata extent", end is not None, True)
        bounded = decode(old_image, control_va, end_rva=end)
        unbounded, stop, _, _ = decode_status(old_image, control_va, end_rva=None)
        check(
            f"{control_va:#x} mid-body jmp does not truncate the decode",
            len(unbounded),
            len(bounded),
        )
        # ...and it stopped because it ran out of function, not because it ran out of budget.
        check(f"{control_va:#x} unbounded decode ends on a terminator", stop, STOP_TERMINATOR)

    # The CHUNKED function. MSVC splits these and gives every chunk its own `.pdata` entry, so a
    # table read without the `UNW_FLAG_CHAININFO` bit reports the first chunk as the whole
    # function. Both of these were then compared over 6 and 7 instructions, reported `IDENTICAL`
    # with a whole-body flag that was not true, and dropped by MIN_VERIFIED_INSNS -- confident and
    # discarded at the same time. The chunk counts are pinned so that a build which stops chunking
    # them says so rather than passing on a different shape.
    for old_va, new_va, first_chunk, run in (
        (0x140AFBAD0, 0x140AFCDF0, 0x16, 0x25C),  # er-reload-trace movemap_do_save_stuff
        (0x1411A8900, 0x1411AA700, 0x20, 0xE5),  # SCALEFORM_HANDLER_DTOR_RVA
    ):
        raw = {begin: end for begin, end, _ in runtime_functions(old_image)}
        check(f"{old_va:#x} first chunk", raw[old_va - BASE] - (old_va - BASE), first_chunk)
        check(f"{old_va:#x} chunk run", old_extents[old_va - BASE] - (old_va - BASE), run)
        check(f"{new_va:#x} chunk run", new_extents[new_va - BASE] - (new_va - BASE), run)
        chunked = compare(
            old_image, new_image, old_va, new_va, old_extents, new_extents
        )
        check(f"{old_va:#x} chunked verdict", chunked["verdict"], IDENTICAL_WHOLE)
        check(f"{old_va:#x} chunked covers past the first chunk", chunked["compared"] > 30, True)
    # A continuation chunk is not a function start. `0x140afbae6` is chunk 2 of the function above,
    # and reporting it as an entry is how a detour would be licensed into the middle of a body.
    check(
        "continuation chunk is not a function start",
        (0x140AFBAE6 - BASE) in old_starts,
        False,
    )
    check(
        "...though the raw .pdata table does list it",
        (0x140AFBAE6 - BASE) in {begin for begin, _, _ in runtime_functions(old_image)},
        True,
    )

    # The jump table inside the extent. `SL_POLL_SAVE_STATUS` parks a 104-byte switch table after
    # its last instruction and inside its own `.pdata` extent, so a decode bounded by that extent
    # legitimately stops 104 bytes short. Requiring the decode to land exactly on the end called
    # this pair truncated; `residue_agrees` reads the table as 26 self-relative entries instead
    # and finds all 26 at the same offset from their own function start.
    table_old, table_new = 0x140E6E430, 0x140E70230
    table_old_end = old_extents[table_old - BASE]
    table_new_end = new_extents[table_new - BASE]
    _, old_stop, old_reached, _ = decode_status(old_image, table_old, end_rva=table_old_end)
    _, _, new_reached, _ = decode_status(new_image, table_new, end_rva=table_new_end)
    check("switch table leaves a residue", table_old_end - old_reached, 104)
    check("the residue stops the decode short", old_stop, STOP_LIMIT)
    check(
        "residue is the same table relocated",
        residue_agrees(
            old_image[old_reached:table_old_end],
            new_image[new_reached:table_new_end],
            table_old - BASE,
            table_new - BASE,
        ),
        True,
    )
    # ...but only because it is the same table. Point one side at the other function's table and
    # the entries no longer line up.
    check(
        "a mismatched residue is refused",
        residue_agrees(
            old_image[old_reached:table_old_end],
            new_image[new_reached:table_new_end],
            table_old - BASE,
            table_new - BASE + 4,
        ),
        False,
    )
    with_table = compare(
        old_image, new_image, table_old, table_new, old_extents, new_extents
    )
    check("switch-table function verdict", with_table["verdict"], IDENTICAL_WHOLE)

    # A decode that ran out of budget is not an identical one. Take a `.pdata` extent away from a
    # long function and the decode stops at DECODE_LIMIT with matching streams on both sides; the
    # verdict must record that its coverage is unknown, not report a clean match.
    prefix = compare(old_image, new_image, 0x1408D0900, 0x1408D1AA0, {}, {})
    check("unbounded long body stops at the limit", prefix["compared"], DECODE_LIMIT)
    check("truncated compare is not IDENTICAL", prefix["verdict"], IDENTICAL_PREFIX)

    for line in failures:
        print(f"selftest FAIL {line}")
    print(f"selftest: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
