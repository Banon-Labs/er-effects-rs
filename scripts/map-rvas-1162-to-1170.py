#!/usr/bin/env python3
"""Map 1.16.2 game addresses onto ELDEN RING 1.17 by relocation-aware byte content.

ELDEN RING 1.17 (PE FileVersion 2.7.0.0) shipped 2026-08-27 and moved code. Every game address
in this workspace is a 1.16.2 RVA, and `er-hook`'s build gate now REFUSES to install a detour on
an unrecognised build rather than corrupt it -- so what used to be one crash per launch is a list
of addresses to re-point. This turns that list into a table.

WHY THE MATCHER LIVES HERE
--------------------------
`build_masked_pattern` below is this script's own; nothing is imported from elsewhere. An earlier
version of this note said the matcher came from `scripts/dump-deobf-shift.py`. That tool was
DELETED on 2026-08-31 and this file never imported it -- the claim was stale on both counts.

It is worth recording WHY that tool is gone, because the trap it fell into is the one this script
exists to avoid. It mapped `dump-exec.bin` <-> `eldenring-deobf.bin`, and its dump side was still
the **1.16.1** runtime dump: cross-version by two patches, with a region table describing a shift
staircase that does not exist between the images anyone still uses. It could not be repaired,
because a 1.16.2 dump cannot be re-exported -- that program survives only as the already-imported
Ghidra project `proj1162`, and no 1.16.2 `.gzf` exists on this machine.

The region assist is deliberately absent here for the same reason: there is no measured
1.16.2->1.17 region table, and an *estimate* is exactly the kind of plausible-but-wrong address
that lands mid-function.

WHAT A RESULT MEANS
-------------------
A mapping is a CANDIDATE, not a licence to hook. The matcher wildcards relocation-sensitive
operand bytes and requires a unique hit, which is strong, but a function whose body genuinely
changed in 1.17 can still match on its unchanged prologue while its behaviour differs -- and a
mid-function match is worse than no match. Before any mapped address is written into code, read
the 1.17 function and confirm it does the same job.

WHERE THE ANCHORS COME FROM (2026-09-01)
----------------------------------------
Pass 2 settles an ambiguous address by the delta of the nearest mapping it trusts. Until this
date the only mappings it trusted were other addresses in the SAME invocation that happened to
match uniquely -- so mapping one address alone could never use an anchor at all, and the answer
depended on what else the caller had typed. It now also reads `VERIFIED_LEDGER`, where every pair
has been compared instruction by instruction by `verify-rva-map-1170.py`. Measured over every
`.pdata` entry of the three regions `er-effects-rs-4uw5.13` names, one address at a time:
0x87xxxx 67/278 -> 271/278, 0x92xxxx 37/501 -> 355/501, 0x9axxxx 24/342 -> 265/342.
`scripts/measure-1170-anchor-coverage.py` is what produced those numbers and will produce them
again for any range.

USAGE
    python3 scripts/map-rvas-1162-to-1170.py 0x1407ada40 0x14025f5f0
    python3 scripts/map-rvas-1162-to-1170.py --from-refusal-log <er-quickload-autoload-debug.log>
    python3 scripts/map-rvas-1162-to-1170.py --tsv docs/recon/rva-map-1162-to-1170.tsv 0x...
    python3 scripts/map-rvas-1162-to-1170.py --no-anchors 0x...   # ledger ignored

The refusal-log mode reads the addresses `er-hook` refused at runtime, which is the authoritative
work list: it is what this build actually tried to hook, not what someone thought it hooks.
"""

import argparse
import importlib.util
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def _resolve_image(env_var, filename):
    """Locate a deobf image: explicit env override, then this checkout, then the MAIN worktree.

    The two deobf images are gitignored multi-hundred-MB RE inputs that live beside the primary
    checkout and are never copied per worktree. Running this script from a `git worktree` therefore
    used to die with `missing image: <worktree>/eldenring-deobf.bin` and a suggestion to REGENERATE
    it -- advice that is both expensive and wrong, since the file exists and is one directory away.
    Falling back to the main worktree is what makes the mapper usable from an agent worktree at all;
    the env override matches `ER_DEOBF_BIN` in `scripts/find-deobf-bytes.py` so the two tools take
    the same spelling for the same idea.
    """
    override = os.environ.get(env_var)
    if override:
        return override
    local = os.path.join(ROOT, filename)
    if os.path.exists(local):
        return local
    # `git rev-parse --git-common-dir` points at the PRIMARY checkout's `.git` from inside a
    # linked worktree (and at our own `.git` otherwise), so its parent is the main working tree.
    try:
        import subprocess

        common = subprocess.run(
            ["git", "-C", ROOT, "rev-parse", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if common.returncode == 0:
            main_root = os.path.dirname(os.path.abspath(os.path.join(ROOT, common.stdout.strip())))
            candidate = os.path.join(main_root, filename)
            if os.path.exists(candidate):
                return candidate
    except Exception:
        pass
    return local


SRC_IMAGE = _resolve_image("ER_DEOBF_BIN", "eldenring-deobf.bin")
DST_IMAGE = _resolve_image("ER_DEOBF_BIN_1170", "eldenring-deobf-1.17.bin")
BASE = 0x140000000
# Signature lengths tried, longest first. A long signature is the strong evidence, but it also
# reaches past short functions into whatever follows them -- `GetScadutreeBlessing` is 25 bytes,
# so a 40-byte signature drags in the next function and fails to match when THAT moved. Falling
# back through shorter windows recovers those; the length that succeeded is reported, because a
# 16-byte match is materially weaker evidence than a 40-byte one.
SIGNATURE_LADDER = (40, 32, 24, 16)
# Signature length handed to the matcher when the caller pins one.
DEFAULT_SIGNATURE_BYTES = SIGNATURE_LADDER[0]
# Ground truth from the er-ersc-sigshim work: both were established by reading the LIVE 1.17
# process and disassembling both images, so they are facts, not predictions. `--selftest` asserts
# the mapper reproduces them -- a matcher that cannot re-derive a known answer cannot be trusted
# with an unknown one.
KNOWN_MAPPINGS = {
    # GetScadutreeBlessing(PlayerGameData*) -- the flag/override fields moved +0xab5/+0xab4 ->
    # +0xabd/+0xabc, so only the shape around them is stable.
    0x14025F5F0: 0x14025F5D0,
    # The call site in GetWwiseSettings that Seamless Co-op uses to locate GetAllocator.
    0x1422222D8: 0x142224238,
}
# The four addresses er-effects-rs-4uw5.13 was filed about, with their 1.17 values established
# INDEPENDENTLY of this matcher: `docs/recon/rva-map-1162-to-1170.needed-verified.tsv` carries all
# four at IDENTICAL-WHOLE, ratio 1.000, BOTH-ENTRIES, derived from the whole-image `.pdata`
# alignment rather than from any byte search.
#
# Every one of them was UNRESOLVED here until 2026-09-01, and every one of them resolves now ONLY
# because the ledger supplies a nearby delta. That is what makes this fixture worth its runtime:
# delete one of the eight anchor rows added to `rva-map-1162-to-1170.verified.tsv` and `--selftest`
# goes red naming the address, which is the only thing standing between those rows and somebody
# tidying away a table entry that nothing appears to read.
ANCHORED_MAPPINGS = {
    0x140875590: 0x140876580,  # PROFILE_SELECT_LIST_BUILDER_RVA
    0x140920C90: 0x140921E30,  # SYSTEM_QUIT_DUPLICATE_ADD_CANCEL_BUTTON_RVA
    0x1409A4670: 0x1409A5810,  # PROFILE_LOAD_ACTIVATE_RVA / CAP_LOAD_ACTIVATE_RVA
    0x1409A4ED0: 0x1409A6070,  # PROFILE_LOAD_DIALOG_LIST_REBUILD_RVA
}
# The matcher searches +-window around the source offset first, then widens. 1.17 moved code by
# far more than the intra-version staircase this default was tuned for, so start wide.
DEFAULT_WINDOW = 0x200000


def addresses_from_refusal_log(path):
    """Game-image addresses `er-hook` refused, in first-seen order.

    This is the authoritative work list: what the build actually tried to hook, rather than what
    a reader of the source thinks it hooks.
    """
    text = open(path, encoding="utf-8", errors="replace").read()
    seen = []
    for match in re.finditer(r"HOOK REFUSED \(\w+(?:::\w+)? (0x[0-9a-fA-F]+)\)", text):
        address = int(match.group(1), 16)
        # OS-DLL addresses are resolved through GetProcAddress and carry no version assumption;
        # the gate no longer refuses them, but logs from before it was narrowed still list them.
        if address < BASE or address >= BASE + 0x10000000:
            continue
        if address not in seen:
            seen.append(address)
    return seen


def build_masked_pattern(image, offset, want_bytes, rip_only=False, stop_at_return=True):
    """Decode from `offset` and return `(pattern, mask)` with every version-fragile operand byte
    wildcarded.

    Three classes of byte move when the game is patched even though the code is "the same":

    * RIP-relative displacements and branch targets -- the code moved, so the delta changed.
    * **Memory displacements on a register base** -- 1.17's dominant change. `GetScadutreeBlessing`
      is byte-identical except `[rcx+0xab5]` -> `[rcx+0xabd]`, because `PlayerGameData` grew 8
      bytes. A matcher that does not mask these cannot re-derive either mapping this repo already
      knows by hand, which is why `build_masked_pattern` masks them.
    * Immediates -- ids and sizes get retuned across patches.

    What survives is opcode shape and register allocation, which is what identifies a function.

    SEARCHING vs. GATING -- what `rip_only` is for
    ----------------------------------------------
    The masking above is tuned for FINDING a function: a candidate set is fine, and the caller
    then reads the match. A detour's install-time prologue GATE has the opposite need. It already
    knows the address and is asking "is this the right function", so masking a register-base
    displacement would make `SAVE_REQUEST_RETRACT_B72_SIG` (`mov byte [rax+0xb72],0`) accept
    `..._B73_SIG` -- the field offset is the ONLY thing telling the two apart. `rip_only=True`
    narrows the mask to the strict subset that a gate can justify: the displacement of a
    RIP-relative memory operand, which re-encodes on every build because both the instruction and
    the global it names move. Everything else stays compared.

    `build-support/prologue_build.rs` derives exactly that subset with `iced-x86` at build time;
    `scripts/verify-prologue-masks-1170.py` calls this function to check the two agree, so the
    rule has one statement and two independent decoders confirming it.

    `stop_at_return=False` keeps decoding past a `ret`/`jmp`, which a gate needs: a generated
    prologue may legitimately contain one (`QUIT_PHASE_SETTLE_SIG` ends on a `jne`) and the mask
    must cover the whole pin, not the part before the first terminator.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs, x86_const

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    pattern = bytearray()
    mask = bytearray()
    consumed = 0
    for insn in md.disasm(bytes(image[offset : offset + want_bytes * 3]), 0):
        if consumed >= want_bytes:
            break
        raw = bytearray(insn.bytes)
        keep = bytearray(b"\x01" * len(raw))
        encoding = getattr(insn, "encoding", None)
        if rip_only:
            rip_relative = any(
                operand.type == x86_const.X86_OP_MEM
                and operand.mem.base == x86_const.X86_REG_RIP
                for operand in insn.operands
            )
            fields = (
                ((encoding.disp_offset, encoding.disp_size),)
                if encoding is not None and rip_relative
                else ()
            )
        elif encoding is not None:
            fields = (
                (encoding.disp_offset, encoding.disp_size),
                (encoding.imm_offset, encoding.imm_size),
            )
        else:
            fields = ()
        for at, size in fields:
            if size:
                for i in range(at, min(at + size, len(keep))):
                    keep[i] = 0
        pattern += raw
        mask += keep
        consumed += len(raw)
        # A `ret`/`jmp` is a natural end; stopping there keeps a short function's signature from
        # reaching into whatever follows it, which is exactly what defeated the 40-byte default.
        if stop_at_return and insn.mnemonic in ("ret", "jmp"):
            break
    if not pattern:
        return None, None
    return bytes(pattern), bytes(mask)


# How many shape matches may be collected before the signature is treated as identifying nothing.
#
# THIS USED TO BE 9, AND THE 9 WAS COSTING CORRECT ANSWERS. The loop below appended `while
# len(hits) <= 8`, so a signature matching 51 times reported the first 9 and pass 2 chose among
# those. Measured on the address this ticket is about: `0x1409a4670`'s real 1.17 address is
# `0x1409a5810`, which is the THIRTY-NINTH of 51 shape matches at the 40-byte rung -- thirty past
# where the list ended. `rva-map-1162-to-1170.tsv` records the consequence in its own words,
# "UNRESOLVED: 9 shape matches, none at the nearest anchor's delta (+0x11a0)", and +0x11a0 is
# exactly the delta `0x1409a5810` sits at. The anchor was right, the delta was right, and the
# answer had been trimmed off the list before pass 2 could see it.
#
# The ceiling is kept, because an over-wildcarded pattern really can match thousands of times and
# collecting them all is a memory problem rather than evidence. What CHANGED with it is what
# reaching it MEANS: a list this long is now a refusal in `map_one` rather than a list, and the
# reason is that letting it through made the answer depend on the cutoff.
#
# THE ARBITRARINESS THAT FORCED THAT, measured over all 1,121 `.pdata` entries of the three regions
# this ticket names. 227 of them produce a list that reaches the ceiling -- compiler-generated
# unwind funclets and vtable stubs, e.g. `0x1409a07a0` and fourteen neighbours 0x40 apart, whose
# masked shape is `sub rsp,N; mov [rsp],-2; ...; lea rax,[rip+X]; mov [rdx],rax` with every
# displacement wildcarded. Pass 2 picks the candidate at the anchor's delta, and since a delta
# names exactly one address, that reduces to "is `va + delta` in the list" -- which for a truncated
# list means "is it among the 2048 LOWEST-ADDRESSED matches". 148 of the 227 were resolved on that
# basis and 79 were not, and nothing distinguishes the two groups except where the scan stopped.
#
# The alternative was to test `va + delta` against the pattern directly, which is sound and would
# resolve more. It is not taken because it answers the wrong question: when a shape occurs
# thousands of times, "the bytes at the predicted address have that shape too" is nearly free, and
# the resolution would rest entirely on the region delta with the byte check contributing nothing
# while appearing to corroborate. The region-delta method already exists and says so plainly --
# `docs/recon/rva-map-1162-to-1170.functions.tsv`, paired by masked-signature identity ACROSS
# `.pdata` -- and that is where an address of this shape should be looked up.
#
# ONE KNOB, TWO INDEPENDENT FIXES, MERGED 2026-09-03. `main` reached the same diagnosis from the
# same bd ticket on the same day and raised the cap to `MAX_SHAPE_CANDIDATES = 512`, which is the
# same knob under another name; the merge keeps ONE, and keeps this one, because 512 still
# TRUNCATES -- it hands pass 2 the 512 lowest-addressed matches and says nothing -- whereas
# reaching 2048 here is a refusal. Both names cannot survive: `scripts/check.sh` and
# `docs/recon/rva-map-1162-to-1170.verified.tsv` both name `CANDIDATE_CEILING` and its 2048, and a
# second constant would be a second answer to "how many is too many". `main`'s measurements are
# not discarded with its name -- they are folded into `find_unique` below, including the one that
# licenses any ceiling at all: the widest LEGITIMATE list ever observed is 83.
CANDIDATE_CEILING = 2048


def find_unique(haystack, pattern, mask):
    """Every offset where `pattern` matches under `mask`, up to `CANDIDATE_CEILING` of them.

    THE CAP MUST NOT TRUNCATE IN IMAGE ORDER, and until 2026-09-01 it did. It was 9, and
    `bytes.find` walks the image low-to-high, so what reached the second pass was "the first nine
    matches in the 1.17 image", not "the matches". Any function whose shape recurs nine or more
    times BELOW its own address was unresolvable no matter how good the regional anchor was: the
    right answer had already been cut off before the anchor was consulted, and the note read
    `9 shape matches, none at the nearest anchor's delta`, which reads like a disagreement and was
    a truncation.

    Measured on the ProfileSelect activate function `0x1409a4670`: its 46-byte signature matches 51
    places in 1.17 and the true counterpart `0x1409a5810` -- `IDENTICAL-WHOLE` over 381
    instructions, both `.pdata` extents `0x5a6` -- is hit 39 in image order. The same truncation was
    hiding `0x140875590`, `0x1409a4ed0` and `0x140920c90`, the other three addresses
    bd er-effects-rs-4uw5.13 was filed about; all four are the `ANCHORED_MAPPINGS` fixture above.

    AND THIS IS WHY A CEILING IS STILL SAFE: across the whole work list the widest list a genuine
    signature produced is 83, at the ladder's shortest 16-byte rung. `CANDIDATE_CEILING` sits an
    order of magnitude past that, so nothing legitimate can fall off the end -- a list that reaches
    it is a statement about the compiler's shapes, not about a function, and `map_one` refuses it
    rather than truncating it."""
    anchor_at = next((i for i, keep in enumerate(mask) if keep), None)
    if anchor_at is None:
        return []
    # Anchor the scan on the longest run of kept bytes: `bytes.find` on that run is C-speed, and
    # the masked comparison then runs on a handful of candidates instead of 98 MB of image.
    best_start, best_len, run_start = 0, 0, None
    for i, keep in enumerate(mask + b"\x00"):
        if keep and run_start is None:
            run_start = i
        elif not keep and run_start is not None:
            if i - run_start > best_len:
                best_start, best_len = run_start, i - run_start
            run_start = None
    anchor = pattern[best_start : best_start + best_len]
    hits = []
    at = haystack.find(anchor)
    while at >= 0 and len(hits) < CANDIDATE_CEILING:
        start = at - best_start
        if start >= 0 and start + len(pattern) <= len(haystack):
            window = haystack[start : start + len(pattern)]
            if all(not keep or window[i] == pattern[i] for i, keep in enumerate(mask)):
                hits.append(start)
        at = haystack.find(anchor, at + 1)
    return hits


def map_one(source, target, va, args):
    """Map one address by masked content. Returns `(new_va, note)`."""
    offset = va - BASE
    if offset < 0 or offset >= len(source):
        return None, "source VA outside the 1.16.2 image"
    lengths = [args.want] if args.want_pinned else list(SIGNATURE_LADDER)
    note = "no unique match"
    for length in lengths:
        pattern, mask = build_masked_pattern(source, offset, length)
        if pattern is None:
            return None, "source bytes did not decode"
        hits = find_unique(target, pattern, mask)
        kept = sum(1 for keep in mask if keep)
        if len(hits) == 1:
            return BASE + hits[0], f"unique, {len(pattern)}B signature, {kept}B fixed"
        if len(hits) >= CANDIDATE_CEILING:
            # Not a candidate list -- a statement that this shape is everywhere. Refused here so
            # pass 2 never sees it, because pass 2 would answer from the anchor's delta alone
            # while the printed note read as though a byte match had confirmed it. See the
            # measurement in `CANDIDATE_CEILING`'s comment, and use the region-delta map
            # (`rva-map-1162-to-1170.functions.tsv`) for addresses of this shape.
            return None, (
                f"UNRESOLVED: over-wildcarded, >= {CANDIDATE_CEILING} shape matches at "
                f"{len(pattern)}B ({kept}B fixed); this signature identifies nothing"
            )
        if len(hits) > 1:
            # Two functions can share a shape -- ELDEN RING has more than one getter built exactly
            # like `GetScadutreeBlessing`. Report every candidate and let the second pass choose
            # by evidence; picking the nearest here looked reasonable and was measurably bad
            # (it resolved addresses to sites 0x400000 away, and only 2 of 51 results landed on a
            # function boundary).
            return None, ("candidates:" + ",".join(f"{BASE + h:#x}" for h in hits))
        note = f"no match at {len(pattern)}B"
    return None, note



# How far away a unique mapping may be and still speak for this address. The 1.16.2 -> 1.17 shift
# is locally constant but changes FAST: measured around `GetScadutreeBlessing`, a neighbour
# 0x320 bytes away shifts by -0x20 while one 0xb27 bytes away shifts by +0x10. So the anchor is
# the SINGLE nearest unique mapping, not a consensus of everything in a wide window -- an early
# version of this demanded unanimity over 0x40000 and resolved nothing.
REGION_RADIUS = 0x8000

# The curated ledger, read as an ANCHOR POOL rather than as a lookup table.
#
# WHY IT IS READ AT ALL. Until this was wired up, pass 2's anchors came only from OTHER addresses
# in the same invocation that happened to map uniquely -- so mapping one address alone could never
# use an anchor, and the tool's answer depended on what else the caller happened to type on the
# command line. Every pair in this file was verified instruction-by-instruction by
# `verify-rva-map-1170.py`, which is far better evidence than a same-run byte match, and it was
# being thrown away. The file already held six rows inside `0x9axxxx`, one of them 0x720 from the
# address that reported "no anchor nearby".
#
# WHAT IT IS NOT. It is not consulted for the answer. An anchor contributes only its DELTA, and
# only to arbitrate between shape candidates the matcher found in the image itself; a row for the
# very address being mapped is skipped (see `regional_delta`), because reading an address out of a
# table is a lookup and reporting it as a derivation would launder one into the other.
#
# AND WHY `rva-map-1162-to-1170.needed-verified.tsv` IS NOT THE DEFAULT, though it is far larger
# and `--anchors` will happily take it. Its pairs come from `functions.tsv`, which is the
# whole-image REGION-DELTA alignment. Anchoring on them would feed the region delta back in as the
# evidence for choosing by region delta -- exactly the circularity that makes an over-wildcarded
# signature worthless here (see `CANDIDATE_CEILING`). The curated ledger's rows were each
# established by a comparison of their own.
VERIFIED_LEDGER = os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.verified.tsv")

# Verdicts whose row may anchor. Every one of these asserts the two WHOLE bodies were compared and
# agreed, which is what makes the pair -- and therefore its delta -- a fact worth deferring to.
#
# The prefix verdicts are deliberately absent. `IDENTICAL` stopped at a `ret` or at the decode
# limit and says nothing about the rest of the body; `IDENTICAL-SHORT` and `NEAR` say less than
# that. Anchoring on one would let a pair established by a short prefix match arbitrate between
# candidates for its neighbour -- the matcher's own weakness, promoted to a tiebreaker.
ANCHOR_VERDICTS = frozenset(
    (
        "BYTE-IDENTICAL",
        "IDENTICAL-WHOLE",
        "IDENTICAL-LEAF",
        # Proved over its whole body and refused a detour for being too short to hold a JMP. The
        # refusal is about MinHook, not about whether the pair is right, so it anchors.
        "IDENTICAL-LEAF-NOPATCH",
        # Bodies differ, patch sites do not -- and the comparison still covered both bodies in
        # full, so the pair is established.
        "PATCH-SITE-IDENTICAL",
    )
)


def load_anchors(paths):
    """`(old_va, new_va)` pairs from verdict ledgers, for pass 2 to take deltas from.

    Rows whose verdict is not in `ANCHOR_VERDICTS` are skipped rather than trusted quietly: a
    `DIVERGES` row is evidence AGAINST its pair, and anchoring on it would spread one wrong
    address across a whole region.
    """
    pairs = []
    for path in paths:
        if not os.path.exists(path):
            continue
        for line in open(path, encoding="utf-8"):
            if line.startswith("#") or not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 3 or fields[2] not in ANCHOR_VERDICTS:
                continue
            try:
                old_va, new_va = int(fields[0], 16), int(fields[1], 16)
            except ValueError:
                continue
            # The ledgers are written in VAs; the whole-image maps are keyed by RVA. Both are
            # unambiguous because the image base is 0x140000000 and no RVA reaches it.
            if old_va < BASE:
                old_va += BASE
            if new_va < BASE:
                new_va += BASE
            pairs.append((old_va, new_va))
    return pairs


def map_all(source, target, addresses, args):
    """Pass 1 alone: `(results, pending)`, where `pending` holds the shape-candidate lists.

    Split out from `resolve_all` because pass 1 is the expensive half -- one masked scan of a 98 MB
    image per address per ladder rung -- while pass 2 is arithmetic over its output. A caller that
    wants to compare two anchor policies over the same addresses (see
    `scripts/measure-1170-anchor-coverage.py`) must not pay for the scan twice, and must be sure
    the two policies saw byte-for-byte the same candidate lists, which re-running cannot promise.
    """
    results = {}
    pending = {}
    for va in addresses:
        mapped, note = map_one(source, target, va, args)
        if mapped is not None:
            results[va] = (mapped, note)
        elif note.startswith("candidates:"):
            pending[va] = [int(c, 16) for c in note[len("candidates:"):].split(",")]
        else:
            results[va] = (None, note)
    return results, pending


def settle(results, pending, anchors=()):
    """Pass 2: choose among each pending address's candidates by the nearest anchor's delta.

    `results` is updated in place and returned. `anchors` are verified `(old, new)` pairs from
    `load_anchors`, which this treats exactly like a pass-1 unique result: as a delta, and only as
    a delta.
    """

    def regional_delta(va):
        """Delta of the nearest established mapping within `REGION_RADIUS`, or `None`.

        `other != va` is the whole of the circularity guard. A ledger row FOR this address would
        otherwise be distance 0, win every time, and hand back the ledger's own answer dressed as
        a derivation -- which is the one thing a reader of this table must be able to rule out.
        """
        candidates = [
            (abs(other - va), mapped - other, kind)
            for other, mapped, kind in (
                [(o, m, "unique") for o, (m, n) in results.items()
                 if m is not None and n.startswith("unique")]
                + [(o, m, "ledger") for o, m in anchors]
            )
            if other != va and abs(other - va) <= REGION_RADIUS
        ]
        return min(candidates)[1:] if candidates else (None, None)

    for va, candidates in pending.items():
        delta, kind = regional_delta(va)
        agreeing = [c for c in candidates if delta is not None and c - va == delta]
        if len(agreeing) == 1:
            results[va] = (
                agreeing[0],
                # `kind` is in the note because the two are not equally strong and the reader
                # cannot tell them apart afterwards: `unique` is one byte match made by this run,
                # `ledger` is a pair a full instruction-by-instruction comparison accepted.
                f"nearest-anchor delta {delta:+#x} ({kind}), {len(candidates)} shape candidates",
            )
        else:
            results[va] = (
                None,
                f"UNRESOLVED: {len(candidates)} shape matches, none at the nearest anchor's "
                f"delta ({'no anchor nearby' if delta is None else f'{delta:+#x}'})",
            )
    return results


def resolve_all(source, target, addresses, args, anchors=()):
    """Map every address, then use the unambiguous results to settle the ambiguous ones.

    Pass 1 keeps only signatures that matched exactly once -- strong evidence on its own. Pass 2
    takes each remaining address's candidate list and keeps the candidate whose delta equals the
    delta the nearest established mapping agrees on. Anything that survives neither pass is
    reported UNRESOLVED, which is the honest answer: a wrong address here is a mid-function
    detour, and a blank cell costs a Ghidra lookup while a wrong one costs a crash.
    """
    results, pending = map_all(source, target, addresses, args)
    return settle(results, pending, anchors)


def selftest(source, target, args):
    """Assert the mapper reproduces mappings established without it, by three routes."""
    failures = []
    # `GetScadutreeBlessing` shares its shape with another getter, so it is only resolvable with a
    # nearby anchor -- exactly the situation the two-pass design exists for. 0x14025f2d0 maps
    # uniquely 0x320 bytes away with the same -0x20 delta; supplying it is what a real run gets
    # for free from the rest of the work list.
    #
    # Deliberately run with NO ledger, so this half stays a test of the byte path alone.
    anchor_fixture = 0x14025F2D0
    resolved = resolve_all(
        source, target, list(KNOWN_MAPPINGS) + [anchor_fixture], args
    )
    for old_va, expected in KNOWN_MAPPINGS.items():
        found, note = resolved[old_va]
        state = "ok  " if found == expected else "FAIL"
        print(f"  {state} {old_va:#x} -> expected {expected:#x}, got "
              f"{'UNMAPPED' if found is None else f'{found:#x}'}  ({note})")
        if found != expected:
            failures.append(old_va)

    # THE CANDIDATE CEILING, pinned to the measurement that motivated raising it. If this ever
    # stops holding, the number in `CANDIDATE_CEILING`'s comment has gone stale and the claim that
    # a 9-deep list was losing answers no longer has anything behind it.
    ceiling_va, ceiling_expected = 0x1409A4670, 0x1409A5810
    pattern, mask = build_masked_pattern(source, ceiling_va - BASE, SIGNATURE_LADDER[0])
    hits = [BASE + h for h in find_unique(target, pattern, mask)]
    rank = hits.index(ceiling_expected) if ceiling_expected in hits else None
    if rank is None or rank < 9:
        print(f"  FAIL {ceiling_va:#x}: expected {ceiling_expected:#x} past the old 9-deep cap, "
              f"found at index {rank} of {len(hits)}")
        failures.append(ceiling_va)
    else:
        print(f"  ok   {ceiling_va:#x}: {ceiling_expected:#x} is match {rank + 1} of {len(hits)}, "
              "which the old 9-deep candidate cap cut off")

    # THE LEDGER PATH, and the reason the anchor rows are not decoration. Each address is resolved
    # ALONE -- one-element work list -- so nothing but `verified.tsv` can supply the delta.
    anchors = load_anchors([VERIFIED_LEDGER])
    for old_va, expected in ANCHORED_MAPPINGS.items():
        found, note = resolve_all(source, target, [old_va], args, anchors)[old_va]
        state = "ok  " if found == expected else "FAIL"
        print(f"  {state} {old_va:#x} -> expected {expected:#x}, got "
              f"{'UNMAPPED' if found is None else f'{found:#x}'}  ({note})")
        if found != expected:
            failures.append(old_va)

    if failures:
        print(f"selftest FAILED for {len(failures)} known mapping(s)")
        return 1
    print(
        f"selftest passed: {len(KNOWN_MAPPINGS)} live-process mappings re-derived from bytes "
        f"alone, and {len(ANCHORED_MAPPINGS)} more re-derived one at a time from "
        f"{len(anchors)} ledger anchors"
    )
    return 0

def main():
    parser = argparse.ArgumentParser(
        description="Map 1.16.2 addresses onto 1.17 by relocation-aware byte content."
    )
    parser.add_argument("vas", nargs="*", help="1.16.2 VAs (hex 0x... or decimal)")
    parser.add_argument(
        "--from-refusal-log",
        metavar="LOG",
        help="read the addresses er-hook refused from a runtime log",
    )
    parser.add_argument("--tsv", metavar="PATH", help="also write the table here")
    parser.add_argument(
        "--selftest",
        action="store_true",
        help="assert the mapper re-derives the two mappings established from the live 1.17 process",
    )
    parser.add_argument(
        "--bytes",
        dest="want",
        type=lambda s: int(s, 0),
        default=DEFAULT_SIGNATURE_BYTES,
        help=f"signature length in decoded bytes (default {DEFAULT_SIGNATURE_BYTES})",
    )
    parser.add_argument(
        "--window",
        type=lambda s: int(s, 0),
        default=DEFAULT_WINDOW,
        help=f"initial +- search window (default {DEFAULT_WINDOW:#x})",
    )
    parser.add_argument(
        "--anchors",
        action="append",
        metavar="TSV",
        help="verdict ledger whose verified pairs may supply pass-2 deltas; repeatable "
        f"(default: {os.path.relpath(VERIFIED_LEDGER, ROOT)})",
    )
    parser.add_argument(
        "--no-anchors",
        action="store_true",
        help="use only anchors this run derived itself, ignoring every ledger. What the tool did "
        "before the ledger was wired in, kept so the difference can be measured rather than "
        "asserted",
    )
    args = parser.parse_args()

    # RE-EXEC UNDER uv IF capstone IS ABSENT, the bootstrap `check-leaf-extent-pdata-coverage.py`
    # and `verify-thunk-rva-1170.py` already carry. There is no system pip here, so
    # `build_masked_pattern`'s decoder import died with a bare ImportError at exit 1 --
    # indistinguishable from a real finding, and the reason `--selftest` could not be a check.sh
    # gate. Doing it here rather than making check.sh spell `uv run --with capstone` keeps the step
    # matching check.sh's own `python3 ...` step pattern, which does not recognise a `uv` command
    # and would drop the gate from the summary and the total.
    try:
        import capstone  # noqa: F401
    except ImportError:
        try:
            os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])
        except OSError:
            print("skipped: capstone unavailable and `uv` is not on PATH")
            return 0

    for image in (SRC_IMAGE, DST_IMAGE):
        if not os.path.exists(image):
            sys.exit(
                f"missing image: {image}\n"
                "Point at an existing copy with ER_DEOBF_BIN (1.16.2) / ER_DEOBF_BIN_1170 (1.17); "
                "the main worktree is already searched automatically.\n"
                "Only if neither exists, generate with scripts/dearxan-deobfuscate.rs against the "
                "matching eldenring.exe (cargo run --release --example deobfuscate -- <exe> <out>)."
            )
    source = open(SRC_IMAGE, "rb").read()
    target = open(DST_IMAGE, "rb").read()
    # `--bytes` was given explicitly only if it differs from the ladder's first rung; that is the
    # signal to pin one length instead of walking the ladder.
    args.want_pinned = args.want != DEFAULT_SIGNATURE_BYTES

    if args.selftest:
        return selftest(source, target, args)

    wanted = [int(v, 0) for v in args.vas]
    if args.from_refusal_log:
        wanted += [a for a in addresses_from_refusal_log(args.from_refusal_log) if a not in wanted]
    if not wanted:
        sys.exit("no addresses: pass VAs or --from-refusal-log")

    anchors = () if args.no_anchors else load_anchors(args.anchors or [VERIFIED_LEDGER])
    if anchors:
        print(f"anchor pool: {len(anchors)} verified pairs")
    resolved = resolve_all(source, target, wanted, args, anchors)
    rows = []
    mapped = 0
    for va in wanted:
        new_va, note = resolved[va]
        if new_va is None:
            rows.append((va, None, None, note))
        else:
            rows.append((va, new_va, new_va - va, note))
            mapped += 1

    width = max(len(f"{va:#x}") for va, *_ in rows)
    for va, new_va, delta, note in rows:
        if new_va is None:
            print(f"{va:#0{width}x}  ->  UNMAPPED           {note}")
        else:
            print(f"{va:#0{width}x}  ->  {new_va:#012x}  delta {delta:+#x}  ({note})")
    print(f"\nmapped {mapped}/{len(rows)}; every mapping is a CANDIDATE -- read the 1.17 function "
          "before hooking it")

    if args.tsv:
        with open(args.tsv, "w", encoding="utf-8") as handle:
            handle.write("# 1.16.2 VA\t1.17 VA\tdelta\tmethod-or-error\n")
            handle.write(
                "# Generated by scripts/map-rvas-1162-to-1170.py. Candidates, not verified "
                "hook sites: a match on an unchanged prologue does not prove the body is\n"
                "# unchanged. Regenerate rather than hand-editing.\n"
            )
            for va, new_va, delta, note in rows:
                if new_va is None:
                    handle.write(f"{va:#x}\t-\t-\t{note}\n")
                else:
                    handle.write(f"{va:#x}\t{new_va:#x}\t{delta:+#x}\t{note}\n")
        print(f"wrote {args.tsv}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
