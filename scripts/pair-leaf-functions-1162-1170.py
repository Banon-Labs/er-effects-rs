#!/usr/bin/env python3
"""Pair the leaf functions of ELDEN RING 1.16.2 with their 1.17 counterparts.

Why this exists
---------------
Every cross-build function pairing in this repo descends from `.pdata`
(`scripts/build-1162-1170-function-map.py` walks the exception directory of both
images and matches RUNTIME_FUNCTION records by masked signature).  `.pdata` holds
one record per function *with unwind data*, and the x64 ABI lets a function omit
unwind data entirely when it allocates no stack and calls nothing.  So the
128,602-pair map -- and therefore every drift statement built on it -- contains
**no leaf functions at all**.

That is not a theoretical hole.  A leaf is exactly the shape of a field getter,
and `GetScadutreeBlessing` (1.16.2 `0x14025f5f0`) is this migration's one
confirmed ground-truth field move: `[rcx+0xab5] -> [rcx+0xabd]`.  It has no
`.pdata` record on either side, so no sweep in this repo has ever looked at it.
"N of 553 offsets cleared" was silent about the whole leaf population.

What a pairing rests on, strongest first
----------------------------------------
1. `CALLER-VOTE` -- the leaf is reached by a direct `call`/`jmp` from a function
   this repo has already paired, at a byte offset that is identical on both
   sides of an identically-shaped body.  The caller pins the callee: it composes
   with the existing map instead of re-deriving it, and it is the only evidence
   here that does not look at the leaf's own bytes.  Its own correctness is
   measurable, because most vote targets are not leaves -- they are ordinary
   `.pdata` functions whose pairing `functions.tsv` already states.  Agreement on
   those is the positive control (`--audit-votes`).
2. `BYTE-IDENTICAL` -- the two bodies are equal byte for byte over an extent
   decoded by the shared `leaf_extent` rule, and that byte string is unique among
   the leaves of both images.
3. `MASKED-SIG` -- equal after `scripts/map-rvas-1162-to-1170.py`'s masking of
   rip-relative displacements and immediates, unique on both sides.  This is the
   rung that pairs `GetScadutreeBlessing`, whose only difference is the field
   displacement.
4. `BRACKET` -- a masked signature that matched but was not unique, disambiguated
   by lying between the same two already-paired anchors on both sides.

A leaf no rung reaches is `UNPAIRED`, and stays that way.  Bracketing is only ever
a tie-break between shape-equal candidates: position alone never pairs anything
here, because a confident wrong pairing reads downstream as a live value and
corrupts silently, while a missing one only costs coverage.

What poisons this and how it is avoided
---------------------------------------
* **Padding.** MSVC pads with `int3` (0xCC) *or* `nop` (0x90) and does not make
  the same choice in two builds.  Any extent that runs into inter-function
  padding desyncs at the first pad byte and everything after compares unequal.
  Extents come from `scripts/verify-rva-map-1170.py::leaf_extent`, imported, not
  copied -- it stops at the terminator and never includes padding.
* **Joining on a number instead of an object.** The drift pass below reports a
  displacement move only per base register inside one aligned pair, never "some
  function somewhere reads 0x50".

Usage
    uv run --with capstone python3 scripts/pair-leaf-functions-1162-1170.py --selftest
    ... --scan                 # phase A: decode both images, cache (minutes)
    ... --pair                 # phase B: enumerate + pair leaves, write TSV
    ... --audit-votes          # positive control: votes vs functions.tsv
    ... --drift                # field-offset drift over the paired leaves
    ... --explain 0x14025f5f0  # everything known about one leaf
    ... --mutate <rule>        # disable a rule; --selftest must then FAIL
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import pickle
import struct
import sys
from collections import Counter, defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMG_1162 = os.path.join(ROOT, "eldenring-deobf.bin")
IMG_1170 = os.path.join(ROOT, "eldenring-deobf-1.17.bin")
FUNCTIONS_TSV = os.path.join(ROOT, "docs/recon/rva-map-1162-to-1170.functions.tsv")

CACHE_DIR = os.environ.get(
    "LEAFMAP_CACHE",
    "/tmp/claude-1000/-home-banon-projects-er-mods-rs/"
    "f1b1f237-c4a5-4649-9833-a40666da21bb/scratchpad/leafmap",
)

# Ground truth, established by hand from the live 1.17 process and recorded in
# `scripts/map-rvas-1162-to-1170.py::KNOWN_MAPPINGS`. This is the positive control
# for the entire pass: it is a leaf on both sides, it is not in `.pdata` on either
# side, and its field move is already known. A pass that cannot rediscover it is
# broken, and `--selftest` says so rather than reporting the leaves as clean.
SCADUTREE_1162 = 0x14025F5F0
SCADUTREE_1170 = 0x14025F5D0
SCADUTREE_EXTENT = 0x19
SCADUTREE_MOVES = {0xAB5: 0xABD, 0xAB4: 0xABC}

# Rules that `--mutate` can switch off, so that "the selftest would have caught
# this" is an observation rather than a claim.
MUTATIONS = (
    "mask",  # E3 stops masking operand bytes
    "unanimity",  # caller votes no longer have to agree
    "injective",  # two leaves may claim the same 1.17 leaf
    "bracket-needs-shape",  # bracketing pairs on position alone
    "extent",  # extents come from the retired "next .pdata start" guess
    "frame-bases",  # stack-frame and global displacements count as struct fields again
)
MUTATED: set[str] = set()


def mutated(name: str) -> bool:
    return name in MUTATED


# --------------------------------------------------------------------------- #
# shared rules, imported rather than reimplemented
# --------------------------------------------------------------------------- #


def _load_sibling(relpath: str, modname: str):
    path = os.path.join(ROOT, relpath)
    spec = importlib.util.spec_from_file_location(modname, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[modname] = module
    spec.loader.exec_module(module)
    return module


_VERIFY = None
_MAPPER = None


def leaf_extent_rule():
    """`verify-rva-map-1170.py::leaf_extent` -- the extent rule validated against
    Ghidra's own 1.16.2 function sizes (49 match / 0 mismatch)."""
    global _VERIFY
    if _VERIFY is None:
        _VERIFY = _load_sibling("scripts/verify-rva-map-1170.py", "_verify_rva_map_1170")
    return _VERIFY.leaf_extent


def masking_rule():
    """`map-rvas-1162-to-1170.py::build_masked_pattern` -- the repo's one masking
    implementation. Reused so a subtle rule cannot drift apart in two copies."""
    global _MAPPER
    if _MAPPER is None:
        _MAPPER = _load_sibling("scripts/map-rvas-1162-to-1170.py", "_map_rvas_1162_1170")
    return _MAPPER.build_masked_pattern


# --------------------------------------------------------------------------- #
# images
# --------------------------------------------------------------------------- #


class Image:
    """A flat, de-Arxan'd ELDEN RING image: file offset == RVA, base 0x140000000."""

    def __init__(self, path: str):
        self.path = path
        with open(path, "rb") as handle:
            self.data = handle.read()
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        if self.data[pe : pe + 4] != b"PE\0\0":
            raise SystemExit(f"{path}: not a PE image")
        nsec = struct.unpack_from("<H", self.data, pe + 6)[0]
        optsz = struct.unpack_from("<H", self.data, pe + 20)[0]
        magic = struct.unpack_from("<H", self.data, pe + 24)[0]
        dirs = pe + 24 + (112 if magic == 0x20B else 96)
        self.pdata_rva, self.pdata_size = struct.unpack_from("<II", self.data, dirs + 3 * 8)
        off = pe + 24 + optsz
        self.code_ranges = []
        for i in range(nsec):
            entry = self.data[off + i * 40 : off + (i + 1) * 40]
            name = entry[:8].rstrip(b"\0").decode("latin1")
            vsz, va, rsz, _rp = struct.unpack_from("<IIII", entry, 8)
            if name.startswith(".text"):
                self.code_ranges.append((va, va + max(vsz, rsz)))
        self._pdata()

    def _pdata(self):
        self.extents: dict[int, int] = {}
        self.starts: set[int] = set()
        for off in range(self.pdata_rva, self.pdata_rva + self.pdata_size, 12):
            begin, end, _unwind = struct.unpack_from("<III", self.data, off)
            if not begin and not end:
                continue
            self.starts.add(begin)
            if end > begin and end - begin <= 0x20000:
                # A chunked function emits several records; the first is the entry.
                self.extents.setdefault(begin, end)

    def is_code(self, rva: int) -> bool:
        return any(lo <= rva < hi for lo, hi in self.code_ranges)


# --------------------------------------------------------------------------- #
# phase A -- decode every declared function, harvest direct branch targets
# --------------------------------------------------------------------------- #

# `call rel32` and the direct jumps. Indirect forms carry no immediate and are
# deliberately ignored: a vtable call names no address here, and a switch table's
# `jmp rax` would otherwise invent one.
BRANCH_MNEMONICS = {"call", "jmp"}


def scan_image(image: Image, tag: str, verbose=True):
    """`{function start rva: [(offset in body, mnemonic, target rva), ...]}`.

    Only functions `.pdata` declares are walked, because a linear sweep from an
    arbitrary byte desynchronises; inside a declared extent the decode is anchored.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = False
    data = image.data
    calls: dict[int, list] = {}
    starts = sorted(image.extents)
    for index, start in enumerate(starts):
        end = image.extents[start]
        found = []
        for addr, _size, mnem, ops in md.disasm_lite(data[start:end], start):
            if mnem not in BRANCH_MNEMONICS:
                continue
            if not ops.startswith("0x"):
                continue
            try:
                target = int(ops, 16)
            except ValueError:
                continue
            found.append((addr - start, mnem, target))
        if found:
            calls[start] = found
        if verbose and index % 40000 == 0 and index:
            print(f"  [{tag}] {index}/{len(starts)}", flush=True)
    return calls


def cache_path(name: str) -> str:
    os.makedirs(CACHE_DIR, exist_ok=True)
    return os.path.join(CACHE_DIR, name)


def do_scan(force=False):
    for tag, path in (("1162", IMG_1162), ("1170", IMG_1170)):
        out = cache_path(f"calls-{tag}.pickle")
        if os.path.exists(out) and not force:
            print(f"[{tag}] cache present: {out}")
            continue
        image = Image(path)
        print(f"[{tag}] decoding {len(image.extents)} declared functions ...", flush=True)
        calls = scan_image(image, tag)
        with open(out, "wb") as handle:
            pickle.dump(calls, handle, protocol=4)
        total = sum(len(v) for v in calls.values())
        print(f"[{tag}] wrote {out}: {len(calls)} functions, {total} direct branches")
    return 0


def load_calls(tag: str):
    path = cache_path(f"calls-{tag}.pickle")
    if not os.path.exists(path):
        raise SystemExit(f"missing {path}; run --scan first (it takes minutes -- background it)")
    with open(path, "rb") as handle:
        return pickle.load(handle)


# --------------------------------------------------------------------------- #
# leaf enumeration
# --------------------------------------------------------------------------- #

PAD_BYTES = {0xCC, 0x90}


def gap_entries(image: Image):
    """The first non-pad byte of every hole between one `.pdata` extent and the
    next declared start, per code section.

    A hole is where undeclared code lives.  This does not claim the byte is a
    function entry -- it is an enumeration floor, used to bound how much of the
    leaf population the call-target harvest could be missing.
    """
    out = []
    for lo, hi in image.code_ranges:
        spans = sorted((s, e) for s, e in image.extents.items() if lo <= s < hi)
        if not spans:
            continue
        watermark = spans[0][0]
        for start, end in spans:
            if start > watermark:
                cursor = watermark
                while cursor < start and image.data[cursor] in PAD_BYTES:
                    cursor += 1
                if cursor < start:
                    out.append((cursor, start))
            watermark = max(watermark, end)
    return out


def enumerate_leaves(image: Image, calls) -> dict:
    """Every address that is branched to directly but that `.pdata` does not
    declare: the leaf population, derived from its callers.

    A second wave decodes the leaves found in the first and harvests their direct
    branches, because MSVC emits `.pdata`-less tail-call thunks that chain.
    """
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    referenced = defaultdict(set)  # target -> {caller}
    for caller, branches in calls.items():
        end = image.extents.get(caller, caller)
        for _off, mnem, target in branches:
            # A `jmp` inside the caller's own extent is a local branch, not a function.
            # Counting those made the first run report 211,372 "leaves" in 1.16.2 -- nearly
            # one per `.pdata` record -- which is the population of basic blocks, not of
            # functions. A `call` always names an entry; a `jmp` only does when it leaves.
            if mnem == "jmp" and caller <= target < end:
                continue
            referenced[target].add(caller)
    leaves = {t: set(c) for t, c in referenced.items() if t not in image.starts and image.is_code(t)}

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = False
    frontier = list(leaves)
    for _wave in range(3):
        fresh = {}
        for leaf in frontier:
            end = extent_of(image, leaf)
            if end is None:
                continue
            for _addr, _size, mnem, ops in md.disasm_lite(image.data[leaf:end], leaf):
                if mnem not in BRANCH_MNEMONICS or not ops.startswith("0x"):
                    continue
                try:
                    target = int(ops, 16)
                except ValueError:
                    continue
                if mnem == "jmp" and leaf <= target < end:
                    continue
                if target in image.starts or not image.is_code(target):
                    continue
                if target not in leaves:
                    fresh.setdefault(target, set()).add(leaf)
        if not fresh:
            break
        for target, callers in fresh.items():
            leaves[target] = callers
        frontier = list(fresh)
    return leaves


def region_kinds(image: Image, rvas):
    """Classify each address by where it sits relative to `.pdata`'s own declarations.

    Not every branch target `.pdata` fails to declare is a function. Three cases, and the
    difference matters because a `MID-EXTENT` row presented as a leaf function is an
    invitation to detour the middle of somebody else's body:

    * `HOLE` -- inside a gap between declared extents. This is a real `.pdata`-less
      function: the linker declared its neighbours and not it, which is exactly what the
      x64 ABI does for a leaf. `GetScadutreeBlessing` is one.
    * `MID-EXTENT` -- inside an extent `.pdata` does declare, but not at its start. A
      branch into the middle of a function (or into an overlapping Arxan chunk record).
      The address still maps, and the mapping is still useful for reading a call site,
      but it is not a function entry and must never be treated as one.
    * `UNDECLARED-REGION` -- in a code section `.pdata` barely covers at all, chiefly the
      second `.text` at 0x4c0e000 that the de-Arxan pass leaves behind.
    """
    import bisect

    spans = sorted((s, e) for s, e in image.extents.items())
    starts = [s for s, _ in spans]
    covered = []
    for lo, hi in image.code_ranges:
        local = [sp for sp in spans if lo <= sp[0] < hi]
        if local:
            covered.append((local[0][0], max(e for _s, e in local)))
    out = {}
    for rva in rvas:
        i = bisect.bisect_right(starts, rva) - 1
        hit = None
        j = i
        # Records overlap in this image, so look back a bounded distance rather than
        # trusting the single nearest start.
        while j >= 0 and j > i - 64:
            s, e = spans[j]
            if s < rva < e:
                hit = (s, e)
                break
            j -= 1
        if hit:
            out[rva] = "MID-EXTENT"
        elif any(lo <= rva < hi for lo, hi in covered):
            out[rva] = "HOLE"
        else:
            out[rva] = "UNDECLARED-REGION"
    return out


_EXTENT_CACHE: dict[tuple[int, int], int] = {}


def extent_of(image: Image, rva: int):
    """End RVA of a leaf, via the shared `leaf_extent` rule. `None` if unbounded."""
    # Keyed by the image's path, never by `id(image)`: CPython reuses an address after an
    # object is freed, so a cache keyed on `id` can hand one image's extent to another.
    key = (image.path, rva)
    hit = _EXTENT_CACHE.get(key)
    if hit is not None:
        return hit or None
    if mutated("extent"):
        # The retired guess this migration removed: run to the next declared start,
        # capped. It walks straight through inter-function padding, whose byte
        # differs between builds, and everything after the first pad compares unequal.
        nxt = min((s for s in image.starts if s > rva), default=rva + 0x400)
        end = min(nxt, rva + 0x400)
    else:
        end = leaf_extent_rule()(image.data, BASE + rva, image.starts)
    _EXTENT_CACHE[key] = end or 0
    return end


# --------------------------------------------------------------------------- #
# signatures
# --------------------------------------------------------------------------- #


def full_masked_signature(image: Image, start: int, end: int):
    """Masked pattern covering the whole body, built by repeated application of the
    repo's own `build_masked_pattern`.

    That helper stops at the first `ret`/`jmp`, which truncates a range-checked
    getter (`cmp / ja / <compute> / ret / xor eax,eax / ret`) at its fast path.
    Resuming from where it stopped covers the rest without a second copy of the
    masking rule.
    """
    build = masking_rule()
    pattern, mask = bytearray(), bytearray()
    cursor = start
    while cursor < end:
        piece, keep = build(image.data, cursor, end - cursor)
        if not piece:
            return None, None
        piece, keep = piece[: end - cursor], keep[: end - cursor]
        pattern += piece
        mask += keep
        cursor += len(piece)
    if mutated("mask"):
        # Masking off: rip-relative displacements and field offsets are compared
        # literally, so GetScadutreeBlessing (whose only difference is 0xab5 ->
        # 0xabd) can no longer be paired by shape.
        mask = bytearray(b"\x01" * len(pattern))
    return bytes(pattern), bytes(mask)


def signature_key(image: Image, start: int, end: int):
    """Hashable form of the masked body: wildcarded bytes zeroed, plus the mask itself.

    The mask is a keep/drop flag per byte (0x01 / 0x00), so the obvious `pattern & mask`
    does not select bytes -- it keeps their low bit and discards the other seven. Written
    that way the signature was 8x weaker than intended: `GetScadutreeBlessing`'s `0xb5` and
    its 1.17 `0xbd` both reduce to 1, so the `--mutate mask` control could not tell masking
    apart from no masking at all, and the shape-collision count was an artefact of the bug.
    The mutation test is what surfaced this; the rule read correctly.
    """
    pattern, mask = full_masked_signature(image, start, end)
    if pattern is None:
        return None
    return (bytes(byte if keep else 0 for byte, keep in zip(pattern, mask)), bytes(mask))


# --------------------------------------------------------------------------- #
# pairing
# --------------------------------------------------------------------------- #


def load_function_map():
    """The existing `.pdata`-derived pairing this pass composes with."""
    pairs = {}
    with open(FUNCTIONS_TSV, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            a, b = line.split("\t")[:2]
            pairs[int(a, 16)] = int(b, 16)
    return pairs


def caller_votes(img162, img170, calls162, calls170, fmap):
    """Votes cast by already-paired callers whose bodies have the same shape.

    A caller qualifies only when its declared extent length is equal on both
    sides and its direct branches sit at identical byte offsets with identical
    mnemonics.  That is much stronger than "same number of calls": identical
    offsets means every instruction before each call had the same length, so the
    i-th branch really is the same branch.
    """
    votes = defaultdict(Counter)
    voters = defaultdict(set)
    qualified = 0
    for a, b in fmap.items():
        ba, bb = calls162.get(a), calls170.get(b)
        if not ba or not bb or len(ba) != len(bb):
            continue
        ea, eb = img162.extents.get(a), img170.extents.get(b)
        if ea is None or eb is None or (ea - a) != (eb - b):
            continue
        if [(o, m) for o, m, _ in ba] != [(o, m) for o, m, _ in bb]:
            continue
        qualified += 1
        for (_o, mnem, ta), (_o2, _m2, tb) in zip(ba, bb):
            # A `jmp` that stays inside the caller is a basic block, not a callee; voting on
            # those would let a block address claim a 1.17 leaf under the injectivity rule.
            if mnem == "jmp" and a <= ta < ea:
                continue
            votes[ta][tb] += 1
            voters[ta].add(a)
    return votes, voters, qualified


def resolve_votes(votes, voters, restrict=None):
    """Unanimous votes only, and injective: two sources may not claim one target."""
    resolved = {}
    refused = {}
    for target, counter in votes.items():
        if restrict is not None and target not in restrict:
            continue
        if len(counter) != 1 and not mutated("unanimity"):
            top = counter.most_common()
            refused[target] = (
                "SPLIT-VOTE: " + ",".join(f"{BASE + t:#x}x{n}" for t, n in top[:4])
            )
            continue
        dest, n = counter.most_common(1)[0]
        resolved[target] = (dest, n, len(voters[target]))
    if not mutated("injective"):
        claimed = defaultdict(list)
        for src, (dst, _n, _v) in resolved.items():
            claimed[dst].append(src)
        for dst, srcs in claimed.items():
            if len(srcs) > 1:
                for src in srcs:
                    refused[src] = "CONFLICT: shares 1.17 target {:#x} with {}".format(
                        BASE + dst, ",".join(f"{BASE + s:#x}" for s in srcs if s != src)
                    )
                    resolved.pop(src, None)
    return resolved, refused


def bracket_pick(src, dsts, key, keyof170, anchor_src, anchor_dst):
    """Tie-break among shape-equal 1.17 candidates by the anchors that surround them.

    Bracketing is not a pairing rung on its own.  It answers "which of these
    equally-shaped candidates", never "what is this".  A candidate whose masked
    signature is not the source's is rejected even when it is the only thing in
    the bracket -- position alone must never manufacture a pairing, because a
    confident wrong address reads downstream as a live value.
    """
    import bisect

    i = bisect.bisect_left(anchor_src, src)
    before = anchor_src[i - 1] if i else None
    after = anchor_src[i] if i < len(anchor_src) else None
    if before is None or after is None:
        return None, "no anchor on both sides"
    lo, hi = anchor_dst[before], anchor_dst[after]
    if lo >= hi:
        return None, "anchors are not ordered in 1.17"
    inside = [d for d in dsts if lo < d < hi]
    if len(inside) != 1:
        return None, f"{len(inside)} candidates inside the bracket"
    if not mutated("bracket-needs-shape") and keyof170.get(inside[0]) != key:
        return None, "the bracketed candidate has a different shape"
    return inside[0], f"between {BASE + before:#x} and {BASE + after:#x}"


def pair_leaves(img162, img170, leaves162, leaves170, calls162, calls170, fmap, verbose=True,
                skip_rung1=False):
    """Run the four rungs in order; each leaf takes the strongest that reaches it.

    `skip_rung1` withholds caller voting so `--crossvalidate` can ask the weaker rungs
    to re-derive, unaided, the pairings the strongest rung produced.  A rung that
    disagrees with the votes is measurably wrong, and that number belongs in the
    report rather than an assurance that bracketing "seems reasonable".
    """
    result = {}  # src -> (dst, evidence, detail)
    refused = {}  # src -> reason
    taken = set()  # 1.17 leaves already claimed

    def claim(src, dst, evidence, detail):
        if dst in taken and not mutated("injective"):
            refused.setdefault(src, f"TARGET-TAKEN: {BASE + dst:#x} already paired")
            return False
        result[src] = (dst, evidence, detail)
        taken.add(dst)
        return True

    # ---- rung 1: caller voting -------------------------------------------- #
    votes, voters, qualified = caller_votes(img162, img170, calls162, calls170, fmap)
    resolved, vote_refusals = resolve_votes(votes, voters, restrict=set(leaves162))
    if skip_rung1:
        resolved, vote_refusals, qualified = {}, {}, 0
    for src, (dst, n, nv) in sorted(resolved.items()):
        if dst not in leaves170:
            # The vote landed on something 1.17 declares in `.pdata`, or on a byte
            # nothing else references. Accepting it would assert a leaf pairing the
            # 1.17 side does not agree is a leaf.
            refused.setdefault(src, f"VOTE-TARGET-NOT-A-1170-LEAF: {BASE + dst:#x}")
            continue
        claim(src, dst, "CALLER-VOTE", f"{n} call sites from {nv} paired callers")
    for src, why in vote_refusals.items():
        if src in leaves162 and src not in result:
            refused.setdefault(src, why)
    if verbose:
        print(f"  rung 1 CALLER-VOTE: {len(result)} paired ({qualified} qualified callers)")

    # extents + signatures for everything still open
    ext162 = {leaf: extent_of(img162, leaf) for leaf in leaves162}
    ext170 = {leaf: extent_of(img170, leaf) for leaf in leaves170}
    open162 = [leaf for leaf in leaves162 if leaf not in result and ext162[leaf]]
    open170 = [leaf for leaf in leaves170 if leaf not in taken and ext170[leaf]]

    # ---- rung 2: byte-identical body, unique on both sides ---------------- #
    body162, body170 = defaultdict(list), defaultdict(list)
    for leaf in open162:
        body162[img162.data[leaf : ext162[leaf]]].append(leaf)
    for leaf in open170:
        body170[img170.data[leaf : ext170[leaf]]].append(leaf)
    n2 = 0
    for body, srcs in body162.items():
        dsts = body170.get(body)
        if len(srcs) == 1 and dsts and len(dsts) == 1:
            if claim(srcs[0], dsts[0], "BYTE-IDENTICAL", f"{len(body)}B body, unique both sides"):
                n2 += 1
    if verbose:
        print(f"  rung 2 BYTE-IDENTICAL: {n2} paired")

    # ---- rung 3: masked signature, unique on both sides ------------------- #
    open162 = [leaf for leaf in open162 if leaf not in result]
    open170 = [leaf for leaf in open170 if leaf not in taken]
    sig162, sig170 = defaultdict(list), defaultdict(list)
    keyof162, keyof170 = {}, {}
    for leaf in open162:
        key = signature_key(img162, leaf, ext162[leaf])
        if key:
            sig162[key].append(leaf)
            keyof162[leaf] = key
    for leaf in open170:
        key = signature_key(img170, leaf, ext170[leaf])
        if key:
            sig170[key].append(leaf)
            keyof170[leaf] = key
    n3 = 0
    for key, srcs in sig162.items():
        dsts = sig170.get(key)
        if len(srcs) == 1 and dsts and len(dsts) == 1:
            detail = f"{len(key[0])}B masked, {sum(1 for k in key[1] if k)}B fixed"
            if claim(srcs[0], dsts[0], "MASKED-SIG", detail):
                n3 += 1
    if verbose:
        print(f"  rung 3 MASKED-SIG: {n3} paired")

    # ---- rung 4: bracketing, but only as a tie-break ---------------------- #
    # Anchors: everything already paired, from `.pdata` and from rungs 1-3.
    anchors = sorted(list(fmap.items()) + [(s, d) for s, (d, _e, _x) in result.items()])
    anchor_src = [s for s, _ in anchors]
    anchor_dst = {s: d for s, d in anchors}
    import bisect

    n4 = 0
    ambiguous = 0
    for key, srcs in sig162.items():
        dsts = sig170.get(key)
        if not dsts:
            continue
        srcs = [s for s in srcs if s not in result]
        dsts = [d for d in dsts if d not in taken]
        if not srcs or not dsts or (len(srcs) == 1 and len(dsts) == 1):
            continue
        ambiguous += len(srcs)
        for src in srcs:
            pick, detail = bracket_pick(src, dsts, key, keyof170, anchor_src, anchor_dst)
            if pick is None:
                continue
            if claim(src, pick, "BRACKET", detail):
                dsts.remove(pick)
                n4 += 1
    if verbose:
        print(f"  rung 4 BRACKET: {n4} paired ({ambiguous} shape-ambiguous leaves considered)")

    for leaf in leaves162:
        if leaf not in result and leaf not in refused:
            if not ext162.get(leaf):
                refused[leaf] = "NO-EXTENT: no terminator inside the decode limit"
            elif leaf not in keyof162:
                refused[leaf] = "NO-SIGNATURE: body did not decode"
            elif len(sig162.get(keyof162[leaf], ())) > 1:
                refused[leaf] = (
                    f"SHAPE-AMBIGUOUS: {len(sig162[keyof162[leaf]])} 1.16.2 leaves share this "
                    f"shape, {len(sig170.get(keyof162[leaf], ()))} in 1.17, no bracket separates them"
                )
            elif keyof162[leaf] not in sig170:
                refused[leaf] = "NO-COUNTERPART: no 1.17 leaf has this shape"
            else:
                refused[leaf] = (
                    f"AMBIGUOUS-1170-SIDE: {len(sig170[keyof162[leaf]])} 1.17 leaves share the shape"
                )
    return result, refused, ext162, ext170, votes


# --------------------------------------------------------------------------- #
# drift over the paired leaves
# --------------------------------------------------------------------------- #


def decode_detail(image: Image, start: int, end: int):
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs
    from capstone.x86 import X86_OP_MEM

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    out = []
    for insn in md.disasm(bytes(image.data[start:end]), start):
        mems = []
        for op in insn.operands:
            if op.type != X86_OP_MEM:
                continue
            mem = op.mem
            if mem.base == 0 or insn.reg_name(mem.base) in (None, "rip"):
                continue
            mems.append((insn.reg_name(mem.base), mem.disp))
        out.append((insn.mnemonic, tuple(insn.reg_name(r) or "?" for r in insn.regs_access()[0]), mems))
    return out


# A displacement on these bases is a stack frame slot, not a struct field. MSVC re-lays a
# frame freely between builds, and a leaf's `[rsp+K]` reads the caller's spill area -- so
# counting those as field drift manufactures a move for every common small offset. The first
# run of this pass did exactly that: `0x30` "moved" in 21 leaf instructions, 14 of them on
# `rsp`, and it implicated ten unrelated repo constants at once.
FRAME_BASES = {"rsp", "rbp", "esp", "ebp"}
# A displacement this large is not a member of an object; it is an offset from a register
# holding the image base (`[rbx + 0x3030aa0]`), i.e. a global. Those move when a section
# grows and say nothing about any structure.
GLOBAL_DISPLACEMENT = 0x100000


def classify_displacement(base, disp):
    if mutated("frame-bases"):
        # The bug this rule exists to stop: with stack slots counted as fields, `0x30`
        # "moves" in 21 leaf instructions, 14 of them `[rsp+0x30]`.
        return "FIELD"
    if base in FRAME_BASES:
        return "STACK"
    if abs(disp) >= GLOBAL_DISPLACEMENT:
        return "GLOBAL"
    return "FIELD"


def leaf_drift(img162, img170, src, dst, e162, e170):
    """Field displacements that moved between one leaf and its counterpart.

    Reported per base register inside a single aligned pair -- never "some
    function reads 0x50". The two bodies must decode to the same mnemonic
    sequence; otherwise the pair is shape-diff and no displacement claim is made,
    because position N on one side is not position N on the other.

    Returns `(verdict, field_moves, held, other_moves)`. Stack-frame and global
    displacements are kept but segregated: they are real changes and worth seeing,
    and they are not evidence about any structure.
    """
    a = decode_detail(img162, src, e162)
    b = decode_detail(img170, dst, e170)
    if len(a) != len(b) or [x[0] for x in a] != [x[0] for x in b]:
        return "SHAPE-DIFF", [], [], []
    moves, held, other = [], [], []
    for (mn, _ra, ma), (_mn2, _rb, mb) in zip(a, b):
        if len(ma) != len(mb):
            return "SHAPE-DIFF", [], [], []
        for (base_a, disp_a), (base_b, disp_b) in zip(ma, mb):
            if base_a != base_b:
                return "SHAPE-DIFF", [], [], []
            kind = classify_displacement(base_a, disp_a)
            if disp_a == disp_b:
                if disp_a and kind == "FIELD":
                    held.append((base_a, disp_a))
            elif kind == "FIELD":
                moves.append((mn, base_a, disp_a, disp_b))
            else:
                other.append((kind, mn, base_a, disp_a, disp_b))
    return ("MOVED" if moves else "STABLE"), moves, held, other


# --------------------------------------------------------------------------- #
# drivers
# --------------------------------------------------------------------------- #


PAIR_CACHE = "pairs.pickle"


def build_everything(verbose=True, use_cache=True, skip_rung1=False, force=False):
    """Enumerate and pair, or reload the last pairing.

    The pairing takes minutes, and `--drift` / `--explain` / `--crossvalidate` all
    want the same answer; recomputing it invites the two consumers to disagree.
    """
    img162, img170 = Image(IMG_1162), Image(IMG_1170)
    cached = cache_path(PAIR_CACHE)
    if use_cache and not force and not skip_rung1 and not MUTATED and os.path.exists(cached):
        with open(cached, "rb") as handle:
            state = pickle.load(handle)
        state["img162"], state["img170"] = img162, img170
        if verbose:
            print(f"loaded {cached}: {len(state['result'])} leaf pairs")
        return state
    calls162, calls170 = load_calls("1162"), load_calls("1170")
    if verbose:
        print("enumerating leaves ...", flush=True)
    leaves162 = enumerate_leaves(img162, calls162)
    leaves170 = enumerate_leaves(img170, calls170)
    if verbose:
        print(f"  1.16.2 leaves: {len(leaves162)}   1.17 leaves: {len(leaves170)}")
    fmap = load_function_map()
    if verbose:
        print(f"  existing .pdata pairs: {len(fmap)}")
        print("pairing ...", flush=True)
    result, refused, ext162, ext170, votes = pair_leaves(
        img162, img170, leaves162, leaves170, calls162, calls170, fmap, verbose,
        skip_rung1=skip_rung1,
    )
    state = dict(
        leaves162={k: sorted(v)[:4] for k, v in leaves162.items()},
        leaves170={k: [] for k in leaves170},
        result=result, refused=refused, ext162=ext162, ext170=ext170,
    )
    if not skip_rung1 and not MUTATED:
        with open(cached, "wb") as handle:
            pickle.dump(state, handle, protocol=4)
    state["img162"], state["img170"] = img162, img170
    state["fmap"] = fmap
    return state


def cmd_pair(args):
    state = build_everything(force=True)
    result, refused = state["result"], state["refused"]
    leaves162 = state["leaves162"]
    by_evidence = Counter(e for _d, e, _x in result.values())
    print()
    print(f"LEAVES 1.16.2 {len(leaves162)}   1.17 {len(state['leaves170'])}")
    print(f"PAIRED  {len(result)}  ({100.0 * len(result) / max(1, len(leaves162)):.1f}%)")
    for evidence, n in by_evidence.most_common():
        print(f"   {evidence:16s} {n}")
    print(f"UNPAIRED {len(refused)}")
    for why, n in Counter(r.split(":")[0] for r in refused.values()).most_common():
        print(f"   {why:26s} {n}")

    kinds = region_kinds(state["img162"], sorted(result))
    print("\nwhere the paired addresses sit relative to .pdata's own declarations:")
    for kind, n in Counter(kinds.values()).most_common():
        print(f"   {kind:20s} {n}")
    print("   MID-EXTENT is a branch INTO a declared function, not a function entry -- the "
          "mapping is\n   usable for reading a call site and must never be detoured.")

    control = SCADUTREE_1162 - BASE
    print()
    if control in result:
        dst, evidence, detail = result[control]
        ok = (BASE + dst) == SCADUTREE_1170
        print(f"POSITIVE CONTROL GetScadutreeBlessing {SCADUTREE_1162:#x} -> {BASE + dst:#x} "
              f"[{evidence}: {detail}]  {'OK' if ok else 'WRONG'}")
    else:
        print(f"POSITIVE CONTROL GetScadutreeBlessing NOT PAIRED: "
              f"{refused.get(control, 'not even enumerated as a leaf')}")

    out = args.out or cache_path("leaf-pairs.tsv")
    with open(out, "w", encoding="utf-8") as handle:
        handle.write("# 1.16.2 VA\t1.17 VA\tdelta\tevidence\tkind\textent\tdetail\n")
        kinds = region_kinds(state["img162"], sorted(result))
        for src in sorted(result):
            dst, evidence, detail = result[src]
            end = state["ext162"].get(src)
            # A caller-vote pairing needs no extent: the caller pins the callee. Say so
            # rather than crash, and rather than print a number that was never measured.
            extent = f"{end - src:#x}" if end else "-"
            handle.write(
                f"{BASE + src:#x}\t{BASE + dst:#x}\t{dst - src:+#x}\t{evidence}\t"
                f"{kinds[src]}\t{extent}\t{detail}\n"
            )
    print(f"wrote {out}")
    ref = cache_path("leaf-unpaired.tsv")
    with open(ref, "w", encoding="utf-8") as handle:
        handle.write("# 1.16.2 VA\treason\n")
        for src in sorted(refused):
            handle.write(f"{BASE + src:#x}\t{refused[src]}\n")
    print(f"wrote {ref}")
    with open(cache_path("state.pickle"), "wb") as handle:
        pickle.dump(
            {k: state[k] for k in ("result", "refused", "ext162", "ext170")}, handle, protocol=4
        )
    return 0


def cmd_audit_votes(args):
    """Positive control for rung 1: most vote targets are ordinary `.pdata`
    functions whose pairing `functions.tsv` already states independently."""
    img162, img170 = Image(IMG_1162), Image(IMG_1170)
    calls162, calls170 = load_calls("1162"), load_calls("1170")
    fmap = load_function_map()
    votes, voters, qualified = caller_votes(img162, img170, calls162, calls170, fmap)
    resolved, refusals = resolve_votes(votes, voters)
    agree = disagree = novel = 0
    disagreements = []
    for src, (dst, _n, _v) in resolved.items():
        if src in fmap:
            if fmap[src] == dst:
                agree += 1
            else:
                disagree += 1
                disagreements.append((src, fmap[src], dst))
        elif src in img162.starts:
            novel += 1
    print(f"qualified callers      {qualified}")
    print(f"vote targets           {len(votes)}  (resolved {len(resolved)}, refused {len(refusals)})")
    print(f"POSITIVE CONTROL vs functions.tsv:  agree {agree}   DISAGREE {disagree}")
    print(f"  .pdata starts the vote pairs that functions.tsv left unpaired: {novel}")
    rate = disagree / max(1, agree + disagree)
    for src, want, got in disagreements[:10]:
        print(f"   {BASE + src:#x}: functions.tsv {BASE + want:#x}, votes {BASE + got:#x}")
    print(f"disagreement rate {rate * 100:.4f}%")
    return 0 if rate < 0.001 else 1


def cmd_drift(args):
    state = build_everything(verbose=not args.quiet)
    img162, img170 = state["img162"], state["img170"]
    result, ext162, ext170 = state["result"], state["ext162"], state["ext170"]
    verdicts = Counter()
    nonfield = Counter()
    moved_pairs = []
    all_moves = Counter()
    # The leaf-population counterpart of not-moved-ANYWHERE: how many paired leaves read
    # this displacement at the same value in both builds. A number with many holds and no
    # moves is as clear as this method can make it; one with neither is unmeasured.
    all_held = Counter()
    for src in sorted(result):
        dst = result[src][0]
        verdict, moves, held, other = leaf_drift(
            img162, img170, src, dst, ext162[src], ext170[dst]
        )
        for _base, disp in held:
            all_held[disp] += 1
        verdicts[verdict] += 1
        for kind, *_rest in other:
            nonfield[kind] += 1
        if verdict == "MOVED":
            moved_pairs.append((src, dst, moves))
            for _mn, base, a, b in moves:
                all_moves[(a, b)] += 1
    print()
    print("LEAF FIELD-OFFSET DRIFT over the paired leaves")
    for verdict, n in verdicts.most_common():
        print(f"   {verdict:12s} {n}")
    print(f"   (segregated, NOT field evidence: {dict(nonfield)})")
    print(f"\ndistinct (old -> new) FIELD displacement moves: {len(all_moves)}")
    for (a, b), n in all_moves.most_common(60):
        print(f"   {a:#x} -> {b:#x}   ({b - a:+#x})  in {n} leaf instruction(s)")

    control = SCADUTREE_1162 - BASE
    found = {}
    for src, dst, moves in moved_pairs:
        if src == control:
            found = {a: b for _mn, base, a, b in moves if base == "rcx"}
    ok = all(found.get(a) == b for a, b in SCADUTREE_MOVES.items())
    print()
    print(f"POSITIVE CONTROL GetScadutreeBlessing rediscovered field moves: "
          f"{ {hex(a): hex(b) for a, b in found.items()} }  {'OK' if ok else 'FAILED'}")
    out = cache_path("leaf-drift.tsv")
    with open(out, "w", encoding="utf-8") as handle:
        handle.write("# 1.16.2 VA\t1.17 VA\tevidence\tbase\told\tnew\tdelta\tmnemonic\n")
        for src, dst, moves in moved_pairs:
            for mn, base, a, b in moves:
                handle.write(
                    f"{BASE + src:#x}\t{BASE + dst:#x}\t{result[src][1]}\t{base}\t{a:#x}\t{b:#x}"
                    f"\t{b - a:+#x}\t{mn}\n"
                )
    print(f"wrote {out}")
    held_out = cache_path("leaf-held.tsv")
    with open(held_out, "w", encoding="utf-8") as handle:
        handle.write("# displacement\tleaf instructions holding it unchanged\n")
        for disp, n in sorted(all_held.items()):
            handle.write(f"{disp:#x}\t{n}\n")
    print(f"wrote {held_out}")
    return 0 if ok else 1


def cmd_recheck_inventory(args):
    """Re-ask the drift sweep's question over the half of the image it could not see.

    `detect-struct-field-drift.py --resolve-unknown` cleared 362 constants as
    not-moved-anywhere.  That verdict is explicitly a statement about every structure
    the scan can see -- which is why it clears without naming a type.  The scan is
    `.pdata`-derived, so "everywhere" was never the whole image: no leaf was in it.

    This reads the same constants against the leaf drift and reports which of those
    "never moved anywhere" numbers do move in a paired leaf.

    The result is an ANNOTATION, not a verdict.  It joins on a number, and the same
    small offset lives in unrelated structures -- the exact fallacy that made "a hooked
    function reads that number" light up 484 of 553.  A hit here does not say the repo's
    field moved; it says the clearance was computed without looking at this witness, so
    the constant goes back to needing a per-object read.  A row with no hit is not
    cleared either: 69,058 leaves are unpaired and unmeasured.
    """
    import csv

    drift_path = args.leaf_drift or cache_path("leaf-drift.tsv")
    if not os.path.exists(drift_path):
        raise SystemExit(f"missing {drift_path}; run --drift first")
    moves = defaultdict(list)
    with open(drift_path, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#"):
                continue
            src, dst, evidence, base, old, new, delta, mnem = line.rstrip("\n").split("\t")
            moves[int(old, 16)].append((src, dst, evidence, base, new, mnem))
    held = {}
    held_path = cache_path("leaf-held.tsv")
    if os.path.exists(held_path):
        for line in open(held_path, encoding="utf-8"):
            if line.startswith("#"):
                continue
            disp, count = line.split("\t")
            held[int(disp, 16)] = int(count)
    with open(args.recheck_inventory, encoding="utf-8") as handle:
        rows = list(csv.DictReader(handle, delimiter="\t"))
    print(f"leaf pairs contributing moves: {len({m[0] for v in moves.values() for m in v})}")
    print(f"distinct moved displacements in leaves: {len(moves)}")
    buckets = defaultdict(list)
    for row in rows:
        try:
            offset = int(row["offset"], 16)
        except ValueError:
            continue
        buckets[row["verdict"]].append((row["constant"], offset, moves.get(offset, [])))
    for verdict in sorted(buckets):
        entries = buckets[verdict]
        hit = [e for e in entries if e[2]]
        # Rungs 2-4 have a measured error rate against the caller-vote key
        # (`--crossvalidate`), so an annotation resting only on those is weaker than one
        # a vote carries. Report the split rather than one number.
        voted = [e for e in hit if any(w[2] == "CALLER-VOTE" for w in e[2])]
        silent = [e for e in entries if not e[2] and not held.get(e[1])]
        print(f"\n{verdict}: {len(entries)} constants, {len(hit)} whose number ALSO moves "
              f"in a paired leaf ({len(voted)} of them in a CALLER-VOTE pair); "
              f"{len(entries) - len(hit) - len(silent)} held by a leaf and never moved; "
              f"{len(silent)} that no paired leaf touches at all")
        for constant, offset, witnesses in sorted(hit, key=lambda e: -len(e[2]))[:args.show]:
            kinds = Counter(w[3] for w in witnesses)
            print(f"   {constant} = {offset:#x}  in {len(witnesses)} leaf insn(s), bases "
                  f"{dict(kinds)}")
            for src, dst, evidence, base, new, mnem in witnesses[:3]:
                print(f"      {src} -> {dst} [{evidence}]  [{base}+{offset:#x}] -> [{base}+{new}]"
                      f"   {mnem}")
    out = cache_path("inventory-vs-leaves.tsv")
    with open(out, "w", encoding="utf-8") as handle:
        handle.write("# constant\toffset\tprior verdict\tleaf instructions moving this number"
                     "\tleaf instructions HOLDING it\tbases\tstrongest pairing evidence"
                     "\tfirst leaf pair\n")
        for verdict, entries in sorted(buckets.items()):
            for constant, offset, witnesses in sorted(entries):
                bases = ",".join(sorted({w[3] for w in witnesses}))
                strongest = "-"
                for rung in ("CALLER-VOTE", "BYTE-IDENTICAL", "MASKED-SIG", "BRACKET"):
                    if any(w[2] == rung for w in witnesses):
                        strongest = rung
                        break
                first = f"{witnesses[0][0]}->{witnesses[0][1]}" if witnesses else "-"
                handle.write(f"{constant}\t{offset:#x}\t{verdict}\t{len(witnesses)}\t"
                             f"{held.get(offset, 0)}\t{bases or '-'}\t{strongest}\t{first}\n")
    print(f"\nwrote {out}")
    return 0


def cmd_crossvalidate(args):
    """Ask rungs 2-4 to re-derive, unaided, what caller voting already decided.

    Rung 1 is the strongest evidence and it composes with a map built by a
    different method, so it is the closest thing to an answer key that exists
    here.  Withholding it and measuring how often the weaker rungs land on the
    same 1.17 address is the only honest number for how much a `BRACKET`
    pairing is worth -- "bracketing looks reasonable" is not a measurement.
    """
    reference = build_everything(verbose=not args.quiet)
    print("\nre-running WITHOUT caller voting ...", flush=True)
    blind = build_everything(verbose=not args.quiet, use_cache=False, skip_rung1=True)
    key = {src: dst for src, (dst, ev, _d) in reference["result"].items() if ev == "CALLER-VOTE"}
    tally = defaultdict(lambda: [0, 0])
    wrong = []
    for src, (dst, evidence, _detail) in blind["result"].items():
        if src not in key:
            continue
        row = tally[evidence]
        if dst == key[src]:
            row[0] += 1
        else:
            row[1] += 1
            wrong.append((src, key[src], dst, evidence))
    print(f"\ncaller-vote pairings used as the key: {len(key)}")
    total_ok = total_bad = 0
    for evidence in sorted(tally):
        ok, bad = tally[evidence]
        total_ok += ok
        total_bad += bad
        rate = 100.0 * bad / max(1, ok + bad)
        print(f"   {evidence:16s} agrees {ok:6d}   DISAGREES {bad:5d}   ({rate:.2f}% wrong)")
    for src, want, got, evidence in wrong[:15]:
        print(f"     {BASE + src:#x}: votes {BASE + want:#x}, {evidence} {BASE + got:#x}")
    overall = 100.0 * total_bad / max(1, total_ok + total_bad)
    print(f"   overall {overall:.2f}% wrong on the covered key")
    return 0


def cmd_explain(args):
    va = int(args.explain, 0)
    rva = va - BASE
    state = build_everything(verbose=not args.quiet)
    img162 = state["img162"]
    print(f"\n{va:#x}")
    print(f"  .pdata declares a function here (1.16.2): {rva in img162.starts}")
    leaves = state["leaves162"]
    if rva in leaves:
        callers = sorted(leaves[rva])[:8]
        print(f"  enumerated as a leaf, referenced by {len(leaves[rva])} site(s): "
              + ", ".join(f"{BASE + c:#x}" for c in callers))
    else:
        print("  NOT in the enumerated leaf population")
    end = state["ext162"].get(rva)
    print(f"  extent: {(end - rva):#x} bytes" if end else "  extent: unknown")
    if rva in state["result"]:
        dst, evidence, detail = state["result"][rva]
        print(f"  PAIRED -> {BASE + dst:#x} [{evidence}] {detail}")
        verdict, moves, held, other = leaf_drift(
            img162, state["img170"], rva, dst, end, state["ext170"][dst]
        )
        print(f"  drift: {verdict}")
        for kind, mn, base, a, b in other:
            print(f"     {kind:6s} {mn} [{base}+{a:#x}] -> [{base}+{b:#x}] (not a field)")
        for mn, base, a, b in moves:
            print(f"     MOVED  {mn} [{base}+{a:#x}] -> [{base}+{b:#x}]  ({b - a:+#x})")
        for base, disp in sorted(set(held)):
            print(f"     held   [{base}+{disp:#x}]")
    else:
        print(f"  UNPAIRED: {state['refused'].get(rva, '(not a leaf)')}")
    return 0


# --------------------------------------------------------------------------- #
# selftest
# --------------------------------------------------------------------------- #


def selftest(args):
    failures = []

    def check(name, condition, detail=""):
        print(f"  {'ok  ' if condition else 'FAIL'} {name}{('  ' + detail) if detail else ''}")
        if not condition:
            failures.append(name)

    img162, img170 = Image(IMG_1162), Image(IMG_1170)
    s162, s170 = SCADUTREE_1162 - BASE, SCADUTREE_1170 - BASE

    print("A. the control is invisible to every .pdata-derived pass")
    check("GetScadutreeBlessing has no 1.16.2 .pdata record", s162 not in img162.starts)
    check("...and none in 1.17 either", s170 not in img170.starts)
    fmap = load_function_map()
    check("...and functions.tsv does not contain it", s162 not in fmap)

    print("B. extents come from the shared, Ghidra-validated rule")
    e162, e170 = extent_of(img162, s162), extent_of(img170, s170)
    check("1.16.2 extent is 0x19", e162 == s162 + SCADUTREE_EXTENT,
          f"got {None if not e162 else hex(e162 - s162)}")
    check("1.17 extent is 0x19", e170 == s170 + SCADUTREE_EXTENT,
          f"got {None if not e170 else hex(e170 - s170)}")

    print("C. padding is poison: a raw compare over the same length fails")
    raw_equal = img162.data[s162 : s162 + SCADUTREE_EXTENT] == img170.data[s170 : s170 + SCADUTREE_EXTENT]
    check("unmasked bodies differ (the field move is IN the bytes)", not raw_equal)
    naive162 = min((s for s in img162.starts if s > s162), default=s162 + 0x400)
    check("the retired 'next .pdata start' guess overruns the control",
          min(naive162, s162 + 0x400) - s162 > SCADUTREE_EXTENT,
          f"would have been {min(naive162, s162 + 0x400) - s162:#x}")

    print("D. rung 3 pairs the control by shape")
    k162 = signature_key(img162, s162, e162) if e162 else None
    k170 = signature_key(img170, s170, e170) if e170 else None
    check("masked signatures are equal", k162 is not None and k162 == k170)

    print("E. rung 4 refuses position-only pairings")
    # One candidate sits alone inside the bracket, so position says take it -- and its
    # shape says it is a different function. The rung must decline.
    shape_a, shape_b = (b"\xaa", b"\x01"), (b"\xbb", b"\x01")
    pick, why = bracket_pick(
        0x500, [0x900], shape_a, {0x900: shape_b}, [0x400, 0x600], {0x400: 0x800, 0x600: 0xA00}
    )
    check("a bracketed candidate with a different shape is refused",
          pick is None, why if pick is None else f"took {pick:#x}")
    pick, why = bracket_pick(
        0x500, [0x900], shape_a, {0x900: shape_a}, [0x400, 0x600], {0x400: 0x800, 0x600: 0xA00}
    )
    check("a bracketed candidate with the SAME shape is taken", pick == 0x900, why)
    pick, why = bracket_pick(
        0x500, [0x900, 0x950], shape_a, {0x900: shape_a, 0x950: shape_a},
        [0x400, 0x600], {0x400: 0x800, 0x600: 0xA00},
    )
    check("two candidates in one bracket are refused", pick is None, why)

    print("F. vote resolution refuses split votes and double claims")
    # No `mutated(...) or ...` escape here. A check that excuses itself under the very
    # mutation it exists to detect is not a gate, and both of these read that way until
    # `--mutate unanimity` and `--mutate injective` were observed passing.
    votes = {10: Counter({20: 3, 21: 1})}
    resolved, refused = resolve_votes(votes, {10: {1, 2}})
    check("split vote refused", 10 not in resolved and 10 in refused,
          f"resolved={sorted(resolved)}")
    votes = {10: Counter({20: 2}), 11: Counter({20: 2})}
    resolved, refused = resolve_votes(votes, {10: {1}, 11: {2}})
    check("two leaves claiming one 1.17 target are both refused",
          not resolved and len(refused) == 2, f"resolved={sorted(resolved)}")

    print("G. the drift reader finds the known field move in the control")
    if e162 and e170:
        verdict, moves, held, _other = leaf_drift(img162, img170, s162, s170, e162, e170)
        found = {a: b for _mn, base, a, b in moves if base == "rcx"}
        check("verdict is MOVED", verdict == "MOVED", verdict)
        check("[rcx+0xab5] -> [rcx+0xabd]", found.get(0xAB5) == 0xABD, str(found))
        check("[rcx+0xab4] -> [rcx+0xabc]", found.get(0xAB4) == 0xABC, str(found))
        check("[rcx+0xfc] is reported HELD, not moved", ("rcx", 0xFC) in held)
    else:
        check("extents available for drift", False)

    print("H. a stack slot is not a field")
    check("[rsp+0x30] is classified STACK", classify_displacement("rsp", 0x30) == "STACK",
          classify_displacement("rsp", 0x30))
    check("[rbp+0x30] is classified STACK", classify_displacement("rbp", 0x30) == "STACK",
          classify_displacement("rbp", 0x30))
    check("[rbx+0x3030aa0] is classified GLOBAL",
          classify_displacement("rbx", 0x3030AA0) == "GLOBAL",
          classify_displacement("rbx", 0x3030AA0))
    check("[rcx+0xab5] is classified FIELD", classify_displacement("rcx", 0xAB5) == "FIELD",
          classify_displacement("rcx", 0xAB5))

    print("I. a branch into the middle of a function is not a leaf")
    kinds = region_kinds(img162, [s162, 0x334F, 0x2561])
    check("the control is in a .pdata HOLE", kinds[s162] == "HOLE", kinds[s162])
    check("0x14000334f is MID-EXTENT", kinds[0x334F] == "MID-EXTENT", kinds[0x334F])
    check("0x140002561 is MID-EXTENT", kinds[0x2561] == "MID-EXTENT", kinds[0x2561])

    print("J. end-to-end (needs --scan cache)")
    if MUTATED:
        # Every mutation is caught by a unit section above, in seconds. Running the whole
        # pass under a mutation would take minutes and prove nothing the unit check did not.
        print("  skip  mutated run: the unit sections above are the gate")
    elif os.path.exists(cache_path("calls-1162.pickle")) and os.path.exists(
        cache_path("calls-1170.pickle")
    ):
        state = build_everything(verbose=False)
        paired = state["result"].get(s162)
        check("the full pass pairs the control", paired is not None and paired[0] == s170,
              "" if paired is None else f"-> {BASE + paired[0]:#x} [{paired[1]}]")
    else:
        print("  skip  no phase-A cache; run --scan")

    print()
    if failures:
        print(f"selftest FAILED: {len(failures)} check(s): {', '.join(failures)}")
        return 1
    print("selftest passed")
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--scan", action="store_true", help="phase A: decode both images (slow)")
    parser.add_argument("--force", action="store_true", help="rebuild the phase-A cache")
    parser.add_argument("--pair", action="store_true", help="enumerate and pair leaves")
    parser.add_argument("--audit-votes", action="store_true", help="rung-1 positive control")
    parser.add_argument("--drift", action="store_true", help="field drift over paired leaves")
    parser.add_argument("--recheck-inventory", metavar="TRIAGE_TSV",
                        help="re-ask the drift sweep's question over the leaf population")
    parser.add_argument("--leaf-drift", help="path to leaf-drift.tsv (default: the cache)")
    parser.add_argument("--show", type=int, default=25, help="rows to print per bucket")
    parser.add_argument("--crossvalidate", action="store_true",
                        help="measure rungs 2-4 against the caller-vote answer key")
    parser.add_argument("--explain", metavar="VA", help="everything known about one leaf")
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--mutate", action="append", default=[], choices=MUTATIONS,
                        help="disable a rule; --selftest must then fail")
    parser.add_argument("--out", help="pairing TSV path")
    parser.add_argument("--quiet", action="store_true")
    args = parser.parse_args()
    MUTATED.update(args.mutate)
    if MUTATED:
        print(f"[MUTATED: {', '.join(sorted(MUTATED))}]")
    if args.scan:
        return do_scan(args.force)
    if args.selftest:
        return selftest(args)
    if args.audit_votes:
        return cmd_audit_votes(args)
    if args.pair:
        return cmd_pair(args)
    if args.recheck_inventory:
        return cmd_recheck_inventory(args)
    if args.crossvalidate:
        return cmd_crossvalidate(args)
    if args.drift:
        return cmd_drift(args)
    if args.explain:
        return cmd_explain(args)
    parser.print_help()
    return 2


if __name__ == "__main__":
    try:
        import capstone  # noqa: F401
    except ImportError:
        if os.environ.get("_LEAF_UNDER_UV"):
            raise SystemExit("capstone still missing under `uv run --with capstone`")
        os.environ["_LEAF_UNDER_UV"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])
    sys.exit(main())
