#!/usr/bin/env python3
"""Second-opinion the 1.16.2 -> 1.17 data map: RTTI identity, accessors, literals, code pointers.

Wiring -- Already done, and deliberately not by adding a line. `scripts/check.sh` line 818 already
runs `verify-data-rvas-by-rtti.py --selftest`, and the anchor audit below runs inside that
selftest: the tracked map must have zero disagree, every verified row must survive two mutations,
the frozen negative must stay unverified, and the unanchored set must match `UNANCHORED` exactly.
So the enforcement landed without touching `check.sh`, which another agent has in flight.

`--anchors` is the same audit in a human-readable form and is not needed by CI. `--occupancy` and
`--population` are reporting modes.

What was UNAUDITED, and why it mattered
---------------------------------------
Two separate audits of the 1.17 migration closed the code side -- 485 of 490 declared game
addresses callable, 92 detour sites all detour-safe -- and both ended on the same open sentence:
Data addresses are checked by nothing. `getFunctionByAddress` has no opinion about a global, and
byte equality is worse than no opinion at all, because `.data` at rest is zeros in both builds and
zeros match. That is how `FIRST_SECTION_RVA` (0x1000, a PE section-boundary sanity bound, not an
address of anything) earned an `IDENTICAL-WHOLE` verdict: the bytes there really are identical.

The cost of the hole is measured, not hypothetical. Four singleton globals read garbage on 1.17 on
2026-08-31; `GameDataMan`'s stale address returned `0x6e614d6e6f697463`, little-endian ASCII
`"ctionMan"`, because 1.17 parks the RTTI name `.?AVNWSteamConnectionManager@DLNW3@@` there. Their
deltas were +0x4060, +0x4070, +0x4070 and +0x4080 -- and across the whole tracked map there are
fourteen distinct deltas (seven in `.data` from +0x4010 to +0x4110, seven in `.rdata` from +0x3000
to +0x3160). A single constant is wrong on 46 of the 103 addresses even if you pick the modal one
per section, so nothing carries a data address forward by arithmetic.

The five anchors, and what each one is worth
---------------------------------------------
Every anchor below is content or evidence about the destination. None of them is "the bytes at the
two addresses are equal", and none of them reads the data map for its answer -- the map is only
what the answer is compared against.

  RTTI     A vtable's `[base-8]` CompleteObjectLocator names its class. A mangled name occurring
           exactly once per image identifies its vtable outright (proven); a repeated name falls
           back to the ordinal (ORDINAL). Compiler metadata, no pairing, no pattern matching.

  ACCESSOR The code that reads the global. Every rip-relative reference in 1.16.2 `.text` gets a
           short window with its displacement blanked; a window that is unique in both images
           identifies the same instruction on 1.17, and its displacement says where the global
           went. A neighbourhood that was edited simply stops matching and casts no vote, so an
           edit costs evidence instead of producing a wrong answer. Two agreeing accessors are a
           fact; One is a guess, and is reported as ACCESSOR-weak rather than counted.

           This is not the data map's own method. That one carries the referencing function across
           with the function map and re-reads the same instruction, so every row it produces
           depends on the function map being right about that function. This depends on the
           function map not at all.

  Literal  For a string, the string -- the actual bytes at the target, NUL-terminated, ASCII or
           UTF-16. Unique per image is proven; repeated falls back to the ordinal. `TextFadeOut`,
           `PressStart`, `TosTitle/Text` and `m60_42_34_00` occur exactly once in each build.

  FNPTR    For a table of code pointers, the pointers. Each slot's 1.16.2 target is looked up in
           the function ledger and the paired 1.17 function must be the qword at the same slot of
           the destination. Two agreeing slots required, for the same reason as accessors.

  String-  literal, one dereference out. For a datum whose qwords are `wchar_t*`, the strings
  PTR      those pointers reach -- required to be the same literal, occurring exactly once in each
           image, at the source's pointee and the candidate's, with no slot disagreeing and the
           identical test failing at every +-0x8/+-0x10 neighbour. Added 2026-08-31, and it moved
           two of the four thinnest rows in the ledger out of UNANCHORED:
           `STEAM_ID_ACCESSOR_CALL_SLOT` (9 slots, "Resolution-WindowScreenWidth" and eight more,
           though see `string_ptr_identity` on whose slots those are) and
           `PROFILE_OFFSCREEN_SIZE_TABLE` (3, "SYSTEX_Menu_Profile01"). Both had been carried by a
           bracket plus a single agreeing reference; neither is a vtable, neither has
           unique-enough accessors, and their pointers are data rather than code, so all four
           earlier anchors passed over them in silence.

What is deliberately *not* an anchor
-------------------------------------
Byte equality, a constant delta, and "the address is readable". All three pass on `FIRST_SECTION_RVA`
and on every zeroed `.data` slot in the image. The selftest plants `0x1000 -> 0x1000` as a frozen
negative and requires this tool to answer no-ANCHOR: an over-broad matcher that verifies whatever it
is handed goes red there even though the row is, in the byte sense, perfectly "verified".

Why this second opinion is not a repeat
---------------------------------------
`map-data-rvas-1162-to-1170.py` carries a datum by the code that references it: it maps the
referencing function onto 1.17 and re-reads the displacement. Every row it produces therefore
depends on the function map being right about that one function. RTTI depends on none of that.
A vtable's `[base-8]` qword points at its CompleteObjectLocator, whose TypeDescriptor holds the
class's mangled name -- metadata the compiler emitted, present in both images, and read here
with no pairing, no signature matching and no shared input with the voting method.

So when both agree, two methods with disjoint failure modes agree. When they disagree, the
reference vote is wrong or the row is not a vtable, and either way it is worth a stop.

What counts as proof, and what only counts as CORROBORATION
-----------------------------------------------------------
A mangled name that occurs exactly once per image identifies its vtable outright: Proven.
ELDEN RING has plenty of names that occur several times (base subobject vtables, and 5,616 of
the 10,202 are `std::_Func_impl` / lambda functors whose names repeat). For those the name alone
is not an identity, so the check falls back to the ORDINAL: if the source is the k-th vtable
carrying that name in 1.16.2, the destination must be the k-th carrying it in 1.17. Both images
hold the same 10,202 vtables in the same relative order, so the ordinal is meaningful -- but it
is an ordering argument, not a unique key, so it is reported as ORDINAL rather than proven.

A row whose source is not a vtable at all (a plain global, a table of pointers) is N/A here.
That is most of the map, and it is why this tool does not replace the voting one.

Usage
  python3 scripts/verify-data-rvas-by-rtti.py                 # check the tracked data map
  python3 scripts/verify-data-rvas-by-rtti.py --anchors       # all rows, all five anchors (gate)
  python3 scripts/verify-data-rvas-by-rtti.py --occupancy     # what sits at each 1.17 address
  python3 scripts/verify-data-rvas-by-rtti.py --population    # declared data addresses with no row
  python3 scripts/verify-data-rvas-by-rtti.py --deltas        # + whole-.rdata delta census
  python3 scripts/verify-data-rvas-by-rtti.py --selftest
"""

import argparse
import collections
import os
import re
import struct
import subprocess
import sys

# MSVC stamps each translation unit's anonymous NAMESPACE with a per-build hash, so
# `?A0x7c8d539b` in 1.16.2 is `?A0x8fca6706` in 1.17 for the same namespace. Comparing the
# raw name therefore fails on a class that is otherwise byte-identical, and it fails silently
# as a "no counterpart" rather than as a mismatch. Measured on the tracked map:
# `MenuJobLoadContextVtable` (0x2ac71e0; renamed 2026-08-30 from `SELECTOR_STEP_VTABLE_RVA` --
# this RTTI class name was correct all along, it was just filed under the old symbol) is
# `MenuJobWithContext<LoadJobContext@?A0x7c8d539b,
# lambda_1af212c9...>` in 1.16.2 and the identical name with `?A0x8fca6706` in 1.17 -- the
# lambda hash is stable across builds, only the namespace tag moves. Nothing else about the
# name is touched; two genuinely different classes still differ.
ANON_NAMESPACE = re.compile(r"\?A0x[0-9a-f]{8}")


def canonical(name):
    return ANON_NAMESPACE.sub("?A0x@ANON@", name)

BASE = 0x140000000
REPO = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
OLD_IMAGE = os.path.join(REPO, "eldenring-deobf.bin")
NEW_IMAGE = os.path.join(REPO, "eldenring-deobf-1.17.bin")
DATA_MAP = os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.data.tsv")
FUNCTION_MAP = os.path.join(REPO, "docs", "recon", "rva-map-1162-to-1170.functions.tsv")
SCAN = os.path.join(REPO, "scripts", "rtti-scan-all.py")
# The repo-wide cap for any non-game subprocess (scripts/check-no-timeouts.py MAX_TIMEOUT_SECONDS).
SCAN_TIMEOUT_SECONDS = 30


def classmap(image, cache):
    """{rva: mangled_name} for every RTTI vtable in `image`, via rtti-scan-all.py."""
    if not os.path.exists(cache) or os.path.getmtime(cache) < os.path.getmtime(image):
        # Bounded at the repo-wide non-game cap. The scan only runs when the cache is missing or
        # older than the image, so the steady-state cost is zero; if a cold scan ever exceeds the
        # cap, that is a signal to pre-build the cache out of band, not to raise the ceiling.
        subprocess.run(
            [sys.executable, SCAN, cache, "--image", image],
            check=True,
            stdout=subprocess.DEVNULL,
            timeout=SCAN_TIMEOUT_SECONDS,
        )
    out = {}
    with open(cache, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#"):
                continue
            va, _, name = line.rstrip("\n").partition("\t")
            if name:
                out[int(va, 16) - BASE] = canonical(name)
    return out


def ordinals(cm):
    """{rva: (name, k)} -- k is the 0-based index of this vtable among same-named ones."""
    seen = collections.Counter()
    out = {}
    for rva in sorted(cm):
        name = cm[rva]
        out[rva] = (name, seen[name])
        seen[name] += 1
    return out


def by_key(cm):
    """{(name, k): rva}."""
    return {key: rva for rva, key in ordinals(cm).items()}


def read_rows(path):
    rows = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            fields = line.rstrip("\n").split("\t")
            if len(fields) < 3:
                continue
            rows.append(
                (int(fields[0], 16), int(fields[1], 16), fields[2],
                 fields[3] if len(fields) > 3 else "")
            )
    return rows


def judge(src, dst, old_cm, new_cm, old_ord, new_key, old_names, new_names):
    """One row's verdict: (code, detail)."""
    if src not in old_cm:
        return "N/A", "source is not an RTTI vtable"
    name, k = old_ord[src]
    if old_names[name] == 1 and new_names.get(name, 0) == 1:
        expected = new_key.get((name, 0))
        if expected is None:
            return "MISSING", f"{name} has no 1.17 vtable"
        if expected == dst:
            return "PROVEN", name
        return "DISAGREE", f"{name} is at 0x{expected:x} in 1.17, map says 0x{dst:x}"
    expected = new_key.get((name, k))
    if expected is None:
        return "MISSING", f"{name}#{k} has no 1.17 counterpart"
    if expected == dst:
        return "ORDINAL", f"{name}#{k} (name occurs {old_names[name]}x/{new_names.get(name, 0)}x)"
    return "DISAGREE", f"{name}#{k} is at 0x{expected:x} in 1.17, map says 0x{dst:x}"


def run(rows, old_cm, new_cm, quiet=False):
    old_ord = ordinals(old_cm)
    new_key = by_key(new_cm)
    old_names = collections.Counter(old_cm.values())
    new_names = collections.Counter(new_cm.values())
    tally = collections.Counter()
    results = []
    for src, dst, const, note in rows:
        code, detail = judge(src, dst, old_cm, new_cm, old_ord, new_key, old_names, new_names)
        tally[code] += 1
        results.append((code, src, dst, const, note, detail))
        if not quiet and code != "N/A":
            print(f"  {code:9s} 0x{src:x} -> 0x{dst:x}  {const}  [{note}]  {detail}")
    return tally, results


def delta_census(old_cm, new_cm):
    """Delta for every uniquely-named vtable -- an RTTI-only view of how .rdata moved."""
    old_names = collections.Counter(old_cm.values())
    new_names = collections.Counter(new_cm.values())
    new_by_name = {name: rva for rva, name in new_cm.items()}
    census = collections.Counter()
    per_region = collections.defaultdict(collections.Counter)
    for rva, name in old_cm.items():
        if old_names[name] != 1 or new_names.get(name, 0) != 1:
            continue
        delta = new_by_name[name] - rva
        census[delta] += 1
        per_region[rva >> 20][delta] += 1
    return census, per_region


def bracket(rows, old_cm, new_cm, window=0x20000):
    """Bracket every non-vtable row with the nearest RTTI vtables on each side.

    The data map's own `bracket` corroboration uses the data map's own anchors, so it cannot be a
    second opinion about them. RTTI anchors are a different population entirely -- 10,202 vtables
    the compiler labelled, paired here by mangled name with no reference to the map -- and they
    are dense enough through `.rdata` to put a real fence around a string or a pointer table that
    only one instruction reaches.

    A row agrees when its delta equals a delta observed on both sides of it. It is inside when
    the delta merely falls within the neighbours' range (`.rdata` moves as a fine staircase, so
    landing between two different neighbour deltas is common and is not proof). It disagrees when
    the delta is outside the bracket entirely -- that is the shape of a row that jumped a
    discontinuity.
    """
    old_names = collections.Counter(old_cm.values())
    new_names = collections.Counter(new_cm.values())
    new_by_name = {name: rva for rva, name in new_cm.items()}
    anchors = sorted(
        (rva, new_by_name[name] - rva)
        for rva, name in old_cm.items()
        if old_names[name] == 1 and new_names.get(name, 0) == 1
    )
    positions = [rva for rva, _ in anchors]
    verdicts = []
    for src, dst, const, note in rows:
        if src in old_cm:
            continue
        delta = dst - src
        index = 0
        lo = hi = None
        import bisect

        index = bisect.bisect_left(positions, src)
        below = [anchors[i] for i in range(max(0, index - 8), index) if src - anchors[i][0] <= window]
        above = [anchors[i] for i in range(index, min(len(anchors), index + 8)) if anchors[i][0] - src <= window]
        if not below or not above:
            verdicts.append(("NO-ANCHOR", src, dst, const, note, "no RTTI vtable within window"))
            continue
        below_deltas = {d for _, d in below}
        above_deltas = {d for _, d in above}
        span_lo, span_hi = min(below_deltas | above_deltas), max(below_deltas | above_deltas)
        if delta in below_deltas and delta in above_deltas:
            code, detail = "AGREE", f"{delta:+#x} seen on both sides"
        elif span_lo <= delta <= span_hi:
            code, detail = "INSIDE", f"{delta:+#x} within [{span_lo:+#x}, {span_hi:+#x}]"
        else:
            code, detail = "DISAGREE", f"{delta:+#x} outside [{span_lo:+#x}, {span_hi:+#x}]"
        verdicts.append((code, src, dst, const, note, detail))
    return verdicts


# =============================================================================================
# ANCHOR: the code that reads the global
# =============================================================================================

# The first `.text` is the real code. `eldenring-deobf-1.17.bin` declares two sections called
# `.text` -- the second, at RVA 0x4c13000, is 18 MB of tail that no reference in this workspace
# points into, and scanning it instead of the first returns a clean-looking zero rather than an
# error. So the section table is ENUMERATED and the first match taken, never assumed.
def first_text(image):
    """`(rva, size)` of the first section named `.text`, read from the image's own headers."""
    if len(image) < 0x400:
        raise ValueError(
            f"the image is {len(image)} bytes -- there is no PE header to read a section table "
            "from, so nothing below can be answered. A gate that cannot read its input must say "
            "so rather than report a clean scan of nothing."
        )
    pe = struct.unpack_from("<I", image, 0x3C)[0]
    count = struct.unpack_from("<H", image, pe + 6)[0]
    optional = struct.unpack_from("<H", image, pe + 20)[0]
    table = pe + 24 + optional
    for index in range(count):
        entry = image[table + index * 40 : table + (index + 1) * 40]
        if entry[:8].rstrip(b"\0") != b".text":
            continue
        virtual_size, rva, raw_size, _ = struct.unpack_from("<IIII", entry, 8)
        return rva, max(virtual_size, raw_size)
    raise ValueError("no .text section in the image header")


# Rip-relative memory operands, grouped so that one regex covers a whole family. A rip-relative
# operand is `mod=00, rm=101` in the modrm byte, which is exactly the eight values below, and the
# only thing that varies between opcodes is how many immediate bytes follow the displacement.
#
# The optional REX prefix is inside the pattern rather than enumerated, so `mov eax,[rip+d]` and
# `mov r12,[rip+d]` are the same family and the displacement position is read from the match
# length instead of being tabulated per encoding. Getting that by hand is how the first cut of
# this scan reported `refs=0` for all 111 rows: every displacement offset was off by one, and an
# off-by-one produces a confident empty answer rather than an error.
#
# Tail is not cosmetic. The displacement is relative to the end of the instruction, so a trailing
# immediate shifts the arithmetic by its own width -- and an immediate is the normal encoding for
# a single-byte flag global (`mov byte [rip+d],1` / `cmp byte [rip+d],0`). Scanning only the
# no-immediate group finds every read of a pointer global and misses every write to a flag.
_MODRM = b"[\x05\x0d\x15\x1d\x25\x2d\x35\x3d]"
_REX = b"[\x40-\x4f]"
OPERAND_GROUPS = (
    # mov / lea / cmp / test / arithmetic, register operand, no immediate.
    (
        "rm",
        re.compile(
            _REX
            + b"?[\x88\x89\x8a\x8b\x8d\x63\x39\x3b\x85\x84\x01\x03\x29\x2b\x31\x33\x21\x23"
            b"\x09\x0b\xff\x38\x3a\x87]" + _MODRM,
            re.S,
        ),
        0,
    ),
    # group-1 / group-3 with an imm8: `mov byte [rip],1`, `cmp byte [rip],0`, `test byte [rip],x`.
    ("imm8", re.compile(_REX + b"?[\xc6\x80\x83\xf6]" + _MODRM, re.S), 1),
    # ...with an imm32.
    ("imm32", re.compile(_REX + b"?[\xc7\x81\xf7]" + _MODRM, re.S), 4),
    # two-byte opcodes: movzx/movsx (how a byte flag is read) and the SSE loads/stores.
    (
        "0f",
        re.compile(
            b"[\x66\xf2\xf3]?" + _REX + b"?\x0f[\xb6\xb7\xbe\xbf\x10\x11\x28\x29\x6f\x7f\x2e\x2f\xd6]"
            + _MODRM,
            re.S,
        ),
        0,
    ),
)

# Bytes of context kept around each reference. Measured: at 24 the shape is unique often enough to
# carry 87 of the 111 rows and never produced a split vote; dropping to 16 bought one extra row and
# introduced sixteen split votes, several of them a single stray site against a 700-vote majority.
# A shorter window is not a weaker claim that still works, it is a different claim, so it is out.
ACCESSOR_WINDOW = 24
# Agreeing accessors required before the anchor counts. One unopposed reference inside a function
# that happens to have been edited is exactly how a confident wrong address is produced.
MIN_ACCESSORS = 2


def reference_index(image):
    """`(targets, shape_count, shape_site)` for every rip-relative operand in the first `.text`.

    `targets[rva]` lists the sites addressing `rva`; `shape_count[key]` says how many sites in this
    image share a displacement-blanked window, and `shape_site[key]` is one of them. Counting is
    what makes the anchor safe: a shape occurring twice identifies nothing, and is dropped rather
    than allowed to vote.

    Restricted to sites of the same family on purpose, and that loses nothing: a window starts at
    the instruction's first byte, so any occurrence of it anywhere in the image begins with that
    family's opcode bytes and is therefore a site the family's own scan already found.
    """
    base, size = first_text(image)
    segment = image[base : base + size]
    targets = collections.defaultdict(list)
    shape_count = collections.Counter()
    shape_site = {}
    for name, pattern, tail in OPERAND_GROUPS:
        for match in pattern.finditer(segment):
            at = base + match.start()
            displacement_at = match.end() - match.start()
            length = displacement_at + 4 + tail
            if at + ACCESSOR_WINDOW > len(image):
                continue
            (displacement,) = struct.unpack_from("<i", image, at + displacement_at)
            targets[at + length + displacement].append((name, at, displacement_at, length))
            window = image[at : at + ACCESSOR_WINDOW]
            key = (
                name,
                displacement_at,
                window[:displacement_at] + b"\0" * 4 + window[displacement_at + 4 :],
            )
            shape_count[key] += 1
            shape_site.setdefault(key, at)
    return targets, shape_count, shape_site


def accessor_votes(old, new, sources):
    """`{src: (target|None, agreeing, total_sites)}` -- where the accessors say each global went.

    Cached under `/tmp` keyed by both images' size and mtime, because the two scans cost about ten
    seconds and the selftest below runs the whole audit four times over. The cache holds only the
    derivation, which depends on the two images and the source address and on nothing else -- in
    particular not on the map being checked, so a mutated map still gets an honest answer.
    """
    sources = sorted(set(sources))
    stamp = "-".join(
        f"{os.path.getsize(path):x}.{int(os.path.getmtime(path)):x}"
        for path in (OLD_IMAGE, NEW_IMAGE)
    )
    cache = f"/tmp/er-data-accessors-{stamp}.tsv"
    cached = {}
    if os.path.exists(cache):
        with open(cache, encoding="utf-8") as handle:
            for line in handle:
                fields = line.split()
                if len(fields) == 4:
                    target = None if fields[1] == "-" else int(fields[1], 16)
                    cached[int(fields[0], 16)] = (target, int(fields[2]), int(fields[3]))
    if all(src in cached for src in sources):
        return {src: cached[src] for src in sources}

    old_targets, old_count, _ = reference_index(old)
    new_targets, new_count, new_site = reference_index(new)
    del new_targets
    out = {}
    for src in sources:
        sites = old_targets.get(src, [])
        votes = collections.Counter()
        for name, at, displacement_at, length in sites:
            window = old[at : at + ACCESSOR_WINDOW]
            key = (
                name,
                displacement_at,
                window[:displacement_at] + b"\0" * 4 + window[displacement_at + 4 :],
            )
            if old_count.get(key, 0) != 1 or new_count.get(key, 0) != 1:
                continue
            landing = new_site[key]
            (displacement,) = struct.unpack_from("<i", new, landing + displacement_at)
            votes[landing + length + displacement] += 1
        if len(votes) == 1:
            (target, agreeing), = votes.items()
            out[src] = (target, agreeing, len(sites))
        else:
            # No vote at all, or a split. A split is not resolved by majority here: the window is
            # long enough that splits do not occur on this map, so one appearing is a fact worth
            # surfacing rather than averaging away.
            out[src] = (None, 0, len(sites))
    try:
        # Merged, not replaced. Writing only the sources of this call would evict every other
        # source from a cache keyed by the images alone -- so a caller asking about one address
        # would silently make the next full run pay the ten seconds again, and the cache would
        # look present while never being usable.
        merged = dict(cached)
        merged.update(out)
        with open(cache, "w", encoding="utf-8") as handle:
            for src, (target, agreeing, total) in sorted(merged.items()):
                shown = "-" if target is None else f"{target:x}"
                handle.write(f"{src:x}\t{shown}\t{agreeing}\t{total}\n")
    except OSError:
        pass  # a cache that cannot be written is a slow run, not a wrong one
    return out


# =============================================================================================
# ANCHOR: the string itself
# =============================================================================================

# Shortest literal worth treating as identity. Below this a "string" is a few printable bytes of
# some other structure, which is the byte-equality trap wearing a different hat.
MIN_LITERAL = 4


def literal_at(image, rva):
    """`(kind, bytes)` of the NUL-terminated literal at `rva`, or `(None, None)`.

    UTF-16 is tried only when the byte reading fails, and both include their terminator, so a
    literal can be counted in the image without matching a longer string that ends with it.
    """
    end = image.find(b"\0", rva, rva + 512)
    if end > rva and all(0x20 <= byte < 0x7F for byte in image[rva:end]):
        if end - rva >= MIN_LITERAL:
            return "ascii", image[rva : end + 1]
    at = rva
    body = bytearray()
    while at + 1 < len(image) and at - rva < 512:
        pair = image[at : at + 2]
        if pair == b"\0\0":
            break
        if not (0x20 <= pair[0] < 0x7F and pair[1] == 0):
            return None, None
        body += pair
        at += 2
    if len(body) // 2 >= MIN_LITERAL:
        return "utf16", bytes(body) + b"\0\0"
    return None, None


def literal_identity(old, new, src, dst):
    """Verdict from the string content at the two addresses, or `None` when neither is a string."""
    kind, text = literal_at(old, src)
    if text is None:
        return None
    other_kind, other = literal_at(new, dst)
    if other != text:
        return "DISAGREE", f"1.16.2 holds {text[:40]!r}, 1.17 0x{dst:x} holds {other!r}"
    shown = text[:-1].decode("ascii") if kind == "ascii" else text[:-2].decode("utf-16le")
    here, there = old.count(text), new.count(text)
    if here == 1 and there == 1:
        return "LITERAL", f"{kind} {shown!r} occurs exactly once in each image"
    # Repeated literal: fall back to the ordinal, the same argument the RTTI half makes for a
    # repeated class name. Reported as its own verdict so the weaker claim stays visible.
    mine, theirs = _ordinal_of(old, text, src), _ordinal_of(new, text, dst)
    if mine is not None and mine == theirs:
        return "LITERAL-ORDINAL", f"{kind} {shown!r} occurs {here}x/{there}x, both at ordinal {mine}"
    return "DISAGREE", f"{shown!r} is occurrence {mine} in 1.16.2 and {theirs} in 1.17"


def _ordinal_of(image, needle, rva):
    """Which occurrence of `needle` sits at `rva`, or `None` if none does."""
    index = 0
    at = image.find(needle)
    while at != -1:
        if at == rva:
            return index
        index += 1
        at = image.find(needle, at + 1)
    return None


# =============================================================================================
# ANCHOR: the strings a table points at
# =============================================================================================

# Slots read out of a candidate table. Same window as the code-pointer anchor below, and for the
# same reason: past a dozen qwords you are no longer reading the table, you are reading whatever
# follows it.
STRING_PTR_SLOTS = 12


def string_ptr_identity(old, new, src, dst):
    """Verdict from the strings the table's pointers reach, or `None` when it holds none.

    Why a hop. `literal_identity` above asks what is at the address, which answers for the four
    rows that are strings and for nothing else. A table of `wchar_t*` is one dereference away from
    the same evidence and was invisible to every anchor here: it is not a vtable (no RTTI), its
    accessors are not unique enough to vote, and its pointers are data rather than code so the
    function map cannot pair them.

    That was not a small gap. `STEAM_ID_ACCESSOR_CALL_SLOT_RVA` and
    `PROFILE_OFFSCREEN_SIZE_TABLE_RVA` are two of the four thinnest addresses in the ledger --
    carried by a bracket alone, with a single agreeing reference each -- and both sit in front of
    `wchar_t*` slots reaching strings that occur
    exactly once per image: "Resolution-WindowScreenWidth" and "Resolution-WindowScreenHeight" for
    the first, "SYSTEX_Menu_Profile01" for the second, each at the source's pointee in 1.16.2 and
    at the candidate's pointee in 1.17. A name that occurs once per image is the same class of
    evidence the RTTI anchor rests on, and it owes nothing to a delta, to a neighbour, or to the
    reference that carried the row into the map.

    The bar is deliberately strict, because one slot is enough to promote a row here and a wrong
    table would be promoted just as confidently as a right one:
      * the slot's pointee must be a literal in both images and the same literal;
      * that literal must occur exactly once in each image -- an ordinal fallback is not offered,
        because a repeated string reached through a pointer says nothing about which table holds
        the pointer;
      * no slot may disagree -- one mismatched slot returns disagree, which is what catches a
        destination nudged by eight bytes: the slots shift by one and the strings stop lining up;
      * and the identical test must fail at every +-0x8/+-0x10 neighbour, so the verdict identifies
        an address rather than a neighbourhood.

    The window is twelve QWORDS and the object may be shorter, which is worth saying plainly, and
    on this row it is not a hypothetical: `STEAM_ID_ACCESSOR_CALL_SLOT_RVA` (renamed 2026-08-31
    from `STEAM_INTERFACE_GUARD_RVA`) is one qword, an indirect-call slot with exactly one
    reference in the image. The nine slots that carry it here are a graphics-settings key table --
    "Resolution-WindowScreenWidth" through "EffectsQuality" -- that begins at +0x10, after a
    `0x8000000a00000000` filler at +0x8, and belongs to a different object entirely. That does not
    weaken the claim, because the claim being made is about the position -- the 96 bytes at the
    candidate hold pointers to the same nine unique strings, in the same order, as at the source --
    and the neighbour test above is what stops that from degenerating into "somewhere around here".
    """
    agreed, mismatch, shown = _string_ptr_scan(old, new, src, dst)
    if mismatch:
        return "DISAGREE", mismatch
    if not agreed:
        return None
    # Does it select, or does it merely accept? The same question `fnptr_table_confirms` asks in
    # the map generator, and the same answer: a test that also passes one slot either way has not
    # identified an address, it has identified a neighbourhood. Measured on all four rows this
    # anchor has an opinion about, every offset from -0x40 to +0x40 comes back disagree or
    # withheld and only the candidate itself passes.
    for step in (-0x10, -0x8, 0x8, 0x10):
        near = dst + step
        if near < 0 or near + STRING_PTR_SLOTS * 8 > len(new):
            continue
        near_agreed, near_mismatch, _ = _string_ptr_scan(old, new, src, near)
        if near_agreed and not near_mismatch:
            return (
                "DISAGREE",
                f"the identical test also passes at 0x{near:x}, so it does not select an address",
            )
    return (
        "STRING-PTR",
        f"{agreed} pointer slot(s) reach a literal occurring exactly once in each image, "
        f"at the source's pointee and the candidate's ({shown!r}); the same test fails at every "
        "+-0x8/+-0x10 neighbour",
    )


def _string_ptr_scan(old, new, src, dst):
    """`(agreeing slots, first mismatch text or None, one agreeing literal)` over the window.

    Split out so the selectivity check above can re-run the identical scan at a neighbour without
    recursing through the rules that consume its result.
    """
    agreed, shown = 0, None
    for slot in range(STRING_PTR_SLOTS):
        offset = slot * 8
        if src + offset + 8 > len(old) or dst + offset + 8 > len(new):
            break
        here = struct.unpack_from("<Q", old, src + offset)[0]
        there = struct.unpack_from("<Q", new, dst + offset)[0]
        if not (BASE <= here < BASE + len(old) and BASE <= there < BASE + len(new)):
            continue
        kind, text = literal_at(old, here - BASE)
        if text is None:
            continue
        _other_kind, other = literal_at(new, there - BASE)
        if other != text:
            return 0, (
                f"slot +0x{offset:x} reaches {text[:40]!r} in 1.16.2 and {other!r} in 1.17"
            ), None
        if old.count(text) != 1 or new.count(text) != 1:
            continue
        agreed += 1
        if shown is None:
            shown = text[:-1].decode("ascii") if kind == "ascii" else text[:-2].decode("utf-16le")
    return agreed, None, shown


# =============================================================================================
# ANCHOR: the code pointers a table holds
# =============================================================================================

# Slots read from a candidate pointer table, and agreeing pairs required. Two is the same bar the
# accessor anchor uses and for the same reason: one slot is a coincidence away from a wrong table.
FNPTR_SLOTS = 12
MIN_FNPTR_PAIRS = 2


_FUNCTION_PAIRS = {}


def function_pairs():
    """`{1.16.2 rva: 1.17 rva}` from the function ledger -- a different artifact from the data map."""
    if _FUNCTION_PAIRS:
        return _FUNCTION_PAIRS
    pairs = _FUNCTION_PAIRS
    if not os.path.isfile(FUNCTION_MAP):
        return pairs
    with open(FUNCTION_MAP, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#"):
                continue
            fields = line.split("\t")
            if len(fields) < 2:
                continue
            try:
                pairs[int(fields[0], 16)] = int(fields[1], 16)
            except ValueError:
                continue
    return pairs


def fnptr_identity(old, new, src, dst, pairs, text_span):
    """Verdict from pairing each code pointer in the table, or `None` when it is not one."""
    if not pairs:
        return None
    low, high = text_span
    agreed = disagreed = 0
    for slot in range(FNPTR_SLOTS):
        offset = slot * 8
        if src + offset + 8 > len(old) or dst + offset + 8 > len(new):
            break
        here = struct.unpack_from("<Q", old, src + offset)[0]
        if not (BASE + low <= here < BASE + high):
            continue
        expected = pairs.get(here - BASE)
        if expected is None:
            continue
        there = struct.unpack_from("<Q", new, dst + offset)[0]
        if there == BASE + expected:
            agreed += 1
        else:
            disagreed += 1
    if disagreed:
        return "DISAGREE", f"{disagreed} of {agreed + disagreed} code-pointer slots pair elsewhere"
    if agreed >= MIN_FNPTR_PAIRS:
        return "FNPTR", f"{agreed} code-pointer slots pair through the function map"
    return None


# =============================================================================================
# ANCHOR: the sites that dispatch through a table's code-pointer slots
# =============================================================================================

# Slots read out of a candidate table, and agreeing dispatch sites required. The window is the
# same twelve qwords the two anchors above read, for the same reason. Two is the same bar the
# accessor and code-pointer anchors use: one site is a coincidence away from a wrong table.
CALLSITE_SLOTS = 12
MIN_CALLSITE_SITES = 2


def callsite_identity(old, new, src, dst, votes, old_span, new_span):
    """Verdict from the call sites that dispatch through the table's slots, or `None`.

    Why `.pdata` absence is not function absence, and why that left a hole here. `fnptr_identity`
    above pairs a table's code pointers through the function ledger, which is built from `.pdata`.
    `.pdata` declares only functions with unwind data: it is blind to leaves, and measurably so --
    it covers 235,848 entries against the 367,183 functions Ghidra's own analysis finds in 1.16.2
    (366,673 in 1.17). `MENU_PUMP_KICK_PTR_RVA` falls straight into that gap. Its two slots hold
    `0x1409b3ff0` and `0x1409b3fe0`, each a five-byte `E9 rel32` thunk into Arxan rubble, neither
    in either image's `.pdata` and therefore neither in the 128,602-row ledger -- so FNPTR looked
    at the one row it was most needed for and returned `None`.

    Ghidra sees both. `getFunctionByAddress` answers `thunk_FUN_1458f4ac8` at `0x1409b3ff0` and
    `thunk_FUN_1405b2d8e` at `0x1409b3fe0` in 1.16.2, and a five-byte thunk at `0x1409b5240` and
    `0x1409b5230` in 1.17; each has exactly one caller, and the callers are `FUN_1409b24e0` and
    `FUN_1409b3730` -- a pair the function ledger already carries, both 2043 bytes, 28 callees,
    4 callers. The two dispatch sites sit at the same body-relative offsets in each, `+0xdb` and
    `+0x780`.

    But the thunk itself cannot be the evidence, and that is worth saying rather than glossing:
    `getXrefsTo` returns exactly two references to each of the four, the table slot and the
    computed call through that slot. A thunk reachable only from the table has no identity apart
    from the table, so pairing it by its own content or its own callers would be reading the
    answer out of the question. Following its `jmp` does not help either -- the Arxan gadget chain
    behind it is regenerated per build (1.16.2 spills `r14` where 1.17 spills `rcx`, and ends on
    `xchg` where 1.17 ends on `push/pop`), so there is no byte or mnemonic comparison to make.

    So this anchor reads the sites instead, one per slot, with the accessor machinery:
      * a slot counts only when both images hold a first-`.text` address in it, which is what
        makes it a code-pointer table rather than a neighbouring global that moved the same way.
        Bracketing is not an anchor in this file and this must not become one by accident, and the
        gate is what stops it becoming one: Without it the same two-site rule fires on 54 of the
        116 rows, with it on 8. The 46 it drops are brackets wearing this anchor's clothes --
        `DLUID_SINGLETON_RVA` would have been "corroborated" by the site reading its `+0x8`, which
        is `FD4_PAD_MANAGER_RVA`, a different global that happens to have moved the same distance;
      * that slot's rip-relative reference must have a displacement-blanked 24-byte window
        occurring exactly once in each image -- the accessor test, unchanged -- and its 1.17
        displacement must land exactly on `dst + offset`;
      * two such slots are required, and one disagreeing slot returns disagree;
      * and the whole test must fail at every +-0x8/+-0x10 neighbour.

    What that buys over `ACCESSOR-WEAK` is the second instruction. `MENU_PUMP_KICK_PTR_RVA` has a
    single reference to its base, which is why the accessor anchor calls it a guess; the reference
    to `+0x8` is a different instruction, 0x6a5 bytes away in the same function, identified by its
    own unique window, and it lands eight bytes further along. Two independently identified
    instructions dispatching through `dst+0` and `dst+8` is a positional claim about the table
    that no single reference can make.

    Of the 8 rows it has an opinion about, 7 are vtables the RTTI anchor reaches first, and on all
    7 it agrees -- so the one row it actually promotes arrives with the rule already exercised
    against seven independently-decided answers. Measured against the controls the string-PTR
    precedent set: `dst+-0x8` returns disagree, `dst+-0x10` and `+-0x18` return `None`, dst reverted
    to the 1.16.2 address returns `None`, another row's address returns `None`, and the frozen
    negative returns `None`.
    """
    agreed, mismatch = _callsite_scan(old, new, src, dst, votes, old_span, new_span)
    if mismatch:
        return "DISAGREE", mismatch
    if len(agreed) < MIN_CALLSITE_SITES:
        return None
    for step in (-0x10, -0x8, 0x8, 0x10):
        near = dst + step
        if near < 0 or near + CALLSITE_SLOTS * 8 > len(new):
            continue
        near_agreed, near_mismatch = _callsite_scan(old, new, src, near, votes, old_span, new_span)
        if len(near_agreed) >= MIN_CALLSITE_SITES and not near_mismatch:
            return (
                "DISAGREE",
                f"the same test also passes at 0x{near:x}, so it does not select an address",
            )
    where = ", ".join(f"+0x{offset:x}" for offset, _, _ in agreed)
    unledgered = sum(1 for _, here, _ in agreed if here not in function_pairs())
    return (
        "CALLSITE",
        f"{len(agreed)} dispatch site(s) reach the code-pointer slots at {where}, each identified "
        f"by a window unique in both images ({unledgered} of the slots hold a pointee absent from "
        "the function ledger, which is why FNPTR cannot); the same test fails at every "
        "+-0x8/+-0x10 neighbour",
    )


def _callsite_scan(old, new, src, dst, votes, old_span, new_span):
    """`(agreeing [(offset, 1.16.2 pointee, 1.17 pointee)], first mismatch text or None)`.

    Split out so the selectivity check above can re-run the identical scan at a neighbour without
    recursing through the rules that consume its result -- the same shape `_string_ptr_scan` has.
    """
    old_low, old_size = old_span
    new_low, new_size = new_span
    agreed = []
    for slot in range(CALLSITE_SLOTS):
        offset = slot * 8
        if src + offset + 8 > len(old) or dst + offset + 8 > len(new):
            break
        here = struct.unpack_from("<Q", old, src + offset)[0] - BASE
        there = struct.unpack_from("<Q", new, dst + offset)[0] - BASE
        if not (old_low <= here < old_low + old_size and new_low <= there < new_low + new_size):
            continue
        target, agreeing, _total = votes.get(src + offset, (None, 0, 0))
        if target is None or agreeing < 1:
            continue
        if target != dst + offset:
            return agreed, (
                f"the site reading +0x{offset:x} lands on 0x{target:x}, not 0x{dst + offset:x}"
            )
        agreed.append((offset, here, there))
    return agreed, None

# =============================================================================================
# The audit
# =============================================================================================

VERIFIED_CODES = (
    "PROVEN", "ORDINAL", "ACCESSORS", "LITERAL", "LITERAL-ORDINAL", "FNPTR", "STRING-PTR",
    "CALLSITE",
)


def vote_sources(rows):
    """Every address the anchors below need an accessor vote for.

    A row's own address, plus the twelve qwords `callsite_identity` reads out of it. Named rather
    than inlined because `anchor_selftest` PRIMES the vote cache in one pass and the two lists must
    be the same list: `accessor_votes` only reuses its cache when every requested source is already
    in it, so a priming call that asks for less than the audit does pays the ten-second whole-image
    scan twice and looks, from the outside, exactly like a cache that is working.
    """
    wanted = set()
    for src, _dst, _const, _note in rows:
        wanted.update(src + slot * 8 for slot in range(CALLSITE_SLOTS))
    return sorted(wanted)


def audit(rows, old, new, old_cm, new_cm):
    """One verdict per row: the strongest anchor that applies, or no-anchor.

    Order is deliberate. RTTI first because it is compiler metadata and involves no matching at
    all; accessors next because two agreeing sites is the strongest evidence available for a
    global that has no content; then the two content anchors, which apply to the few rows that
    are strings or pointer tables.
    """
    old_ord = ordinals(old_cm)
    new_key = by_key(new_cm)
    old_names = collections.Counter(old_cm.values())
    new_names = collections.Counter(new_cm.values())
    votes = accessor_votes(old, new, vote_sources(rows))
    pairs = function_pairs()
    span = first_text(old)
    new_span = first_text(new)
    results = []
    for src, dst, const, note in rows:
        code, detail = judge(src, dst, old_cm, new_cm, old_ord, new_key, old_names, new_names)
        if code != "N/A":
            results.append((code, src, dst, const, note, detail))
            continue
        target, agreeing, total = votes.get(src, (None, 0, 0))
        if target is not None and target != dst:
            results.append((
                "DISAGREE", src, dst, const, note,
                f"{agreeing} of {total} accessors read 0x{target:x}, the map says 0x{dst:x}",
            ))
            continue
        if target == dst and agreeing >= MIN_ACCESSORS:
            results.append((
                "ACCESSORS", src, dst, const, note,
                f"{agreeing} of {total} independent accessors agree",
            ))
            continue
        verdict = (
            literal_identity(old, new, src, dst)
            or fnptr_identity(old, new, src, dst, pairs, span)
            or string_ptr_identity(old, new, src, dst)
            or callsite_identity(old, new, src, dst, votes, span, new_span)
        )
        if verdict is not None:
            results.append((verdict[0], src, dst, const, note, verdict[1]))
            continue
        if target == dst and agreeing:
            results.append((
                "ACCESSOR-WEAK", src, dst, const, note,
                f"one accessor of {total} agrees -- a guess, not a fact",
            ))
            continue
        results.append(("NO-ANCHOR", src, dst, const, note, f"{total} reference site(s), no vote"))
    return results


# The 19 rows this file's anchors do not reach, pinned by name so the set can only shrink.
#
# It is a list rather than a count so a row leaving it (someone found evidence) and a row entering
# it (someone lost evidence) are different diffs. The data map still carries every one of them on
# its own method; what is recorded here is that the independent anchors above do not corroborate
# them, which is a different and much weaker statement than "verified".
#
# Two shapes, and the distinction is the reason the set is not one bucket:
#
#   No-anchor (12) -- zeroed `.data` at rest in both images, reached only from code 1.17 edited
#   around, so no window is unique on both sides. There is nothing to read and nothing to vote
#   with. `FIRST_SECTION_RVA` is what pretending otherwise looks like.
#
#   ACCESSOR-weak (7) -- exactly one reference site survives the uniqueness test, and it agrees
#   with the map. That is corroboration, not proof: one reference inside a function that happens to
#   have been edited is how a confident wrong address is produced, so it is counted with the
#   unverified.
#
# `NAV_COST_TABLE_RVA` is the one to read before trying to shrink this set again, because it is
# where every anchor above genuinely runs out rather than merely not applying. It is 0x400 bytes of
# zeros in both images with no non-zero qword within +-0x200, so there is no content, no pointer
# and no neighbour to read. Its three reference sites survive uniqueness in 1.16.2 and match
# nothing in 1.17, and the reason is visible in the bytes: at 0x2ec3a2 the 24-byte window differs
# from its 1.17 counterpart at 0x2ec3b2 in exactly two places, both rip-relative displacements of
# other instructions in the window. Masking every operand in the window rather than only the one
# being read -- the treatment `map-rvas-1162-to-1170.py` gives a function signature -- was measured
# on it: it rescues one of the three sites and leaves the other two at nine-plus matches and zero
# matches respectively, so the row still falls short of two and the change would have loosened the
# anchor that carries 58 rows for nothing. So NAV_COST is unanchorable here by every method tried,
# not merely by the one; it is carried by the data map on a bracket and that is all it has.
#
# An entry is a constant name, or an address where there is no name. Four rows in the data map are
# addresses the workspace writes as bare HEX LITERALS handed to `game_rva`, with no constant
# anywhere, so the map keys them `<file>:<line>` -- a string that changes every time somebody edits
# a line above them. Pinning one of those by "name" would pin a line number into a gate and fail on
# the next unrelated edit to the menu tracer. `pin_of` therefore uses the constant when the row has
# one and the source address when it does not; an address is a strictly more stable pin, and the
# set still only shrinks.
UNANCHORED = {
    # `crates/er-quickload/src/experiments/trace/menu_trace_hooks.rs:1248` -- the lazy
    # `CSEblFileManager` slot. ACCESSOR-WEAK: exactly one surviving reference and it agrees. The
    # data map carries it on a bracket (0x3d5b078 and 0x3d5b0f4, both +0x4060, 105 anchors in
    # +-0x400) plus a shape count of 10 sites each side -- neither of which is an anchor this file
    # computes, which is the whole point of listing it here.
    "0x3d5b088",
    # `BUFFER_RVA_1162` (added 2026-09-05, `er-npc-possess`'s packet15_receive layout). No-ANCHOR:
    # zeroed `.data` qword in both images, all 12 rip-relative reference sites unique in 1.16.2
    # `.text` but none of their 24-byte windows recur anywhere in 1.17 -- the referencing code was
    # edited enough that no accessor casts a vote, and the destination holds no string/pointer for
    # the content anchors to read either. Measured directly against `eldenring-deobf.bin` /
    # `eldenring-deobf-1.17.bin` via this file's own `reference_index`, not carried from a comment.
    "BUFFER_RVA_1162",
    "DLUID_SINGLETON_RVA",
    "FAKE_LOADING_SCREEN_SINGLETON_RVA",
    "FD4_IO_POOL_RVA",
    # `GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA` (renamed from the `_RVA` spelling
    # 2026-09-05; same address throughout). No-anchor for the same reason as `BUFFER_RVA_1162`: a
    # zeroed `.data` byte in both images, both 1.16.2 reference-window shapes absent from 1.17.
    "GAME_DEBUG_ENABLE_CONTROL_ON_DISACTIVE_WINDOW_DATA_RVA",
    "INNER_TITLE_STATE_TABLE_RVA",
    "IO_DEVICE_SINGLETON_RVA",
    "MOVIE_SKIP_FLAG_RVA",
    "NAV_COST_TABLE_RVA",
    "PROFILE_MODEL_REND_TABLE_RVA",
    "RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA",
    "SAVE_SERIALIZE_BYTES_RVA",
    "SL_IODEV_GLOBAL_RVA",
    "STREAMING_DRIVER_SINGLETON_RVA",
    "TITLE_CUSTOM_COVER_PROFILE_RENDERER_TABLE_RVA",
    "TITLE_GLOBAL_ACCEPT_BYTE_RVA",
    "TITLE_STEP_IDX10_SLOT_RVA",
    "TITLE_STEP_IDX6_SLOT_RVA",
}


def pin_of(src, const):
    """How a row is named in `UNANCHORED`: its constant, or its address when it has no constant.

    `LEDGER_CONSTANT`'s test, deliberately -- `select-needed-1170-rows.py` decides the same way
    which rows in these files are attributable to a name at all.
    """
    return const if re.match(r"[A-Z][A-Z0-9_]*\b", const or "") else f"0x{src:x}"


def run_anchors(rows, old, new, old_cm, new_cm, quiet=False):
    """Print the per-row verdicts and return `(tally, results)`."""
    results = audit(rows, old, new, old_cm, new_cm)
    tally = collections.Counter()
    for code, src, dst, const, note, detail in results:
        tally[code] += 1
        if not quiet:
            print(f"  {code:14s} 0x{src:x} -> 0x{dst:x}  {const}  [{note}]  {detail}")
    return tally, results


def occupancy(old, new, rows):
    """What sits at each 1.17 destination, and what a stale 1.16.2 read would have returned.

    The point of the second column is the `"ctionMan"` finding: the failure was only recognised
    because someone DECODED the bytes at the stale address instead of checking they were readable.
    A zero there is not reassurance either -- three of the four broken oracles read zero, and a
    zero pointer is indistinguishable from a global the game has not created yet.
    """
    lines = []
    for src, dst, const, _ in rows:
        moved = struct.unpack_from("<Q", new, dst)[0] if dst + 8 <= len(new) else 0
        stale = struct.unpack_from("<Q", new, src)[0] if src + 8 <= len(new) else 0
        kind, text = literal_at(new, dst)
        _, stale_text = literal_at(new, src)
        lines.append(
            f"  {const}\n"
            f"    1.17 0x{dst:x}      qword 0x{moved:016x}  {text!r}\n"
            f"    stale 0x{src:x} on 1.17 qword 0x{stale:016x}  {stale_text!r}"
        )
    return lines


# =============================================================================================
# Population: which declared data addresses have no row at all
# =============================================================================================

# Sections a game DATUM can live in. `.text` is excluded because a `.text` address is a function
# and the function map covers it; `.pdata`/`.rsrc`/`.reloc` are excluded because nothing in this
# workspace addresses them and a constant landing there is arithmetic, not an address.
DATA_SECTIONS = (".rdata", ".data")


def sections(image):
    """`[(name, rva, size)]` from the image's own section table."""
    pe = struct.unpack_from("<I", image, 0x3C)[0]
    count = struct.unpack_from("<H", image, pe + 6)[0]
    optional = struct.unpack_from("<H", image, pe + 20)[0]
    table = pe + 24 + optional
    out = []
    for index in range(count):
        entry = image[table + index * 40 : table + (index + 1) * 40]
        virtual_size, rva, raw_size, _ = struct.unpack_from("<IIII", entry, 8)
        out.append((entry[:8].rstrip(b"\0").decode("latin1"), rva, max(virtual_size, raw_size)))
    return out


def test_scopes(text):
    """Line ranges under `#[cfg(test)]` / `#[test]`, from brace depth in comment-blanked source.

    Needed because half of what lands in `.rdata` by value is a unit test -- a deliberately wrong
    address asserted to be rejected, a `PinListGeometry` fixture, a catalogue of item ids that
    happen to fall in the section's range. Reporting those beside a live defect buries it, and
    filtering them by file name would miss the `#[cfg(test)] mod tests` that sits in the middle of
    `save_picker_menu.rs` with production code after it.
    """
    lines = text.split("\n")
    scopes = []
    for number, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped not in ("#[cfg(test)]", "#[test]"):
            continue
        depth, started, end = 0, False, None
        for later in range(number, len(lines) + 1):
            body = lines[later - 1]
            depth += body.count("{") - body.count("}")
            started = started or "{" in body
            if started and depth <= 0:
                end = later
                break
        scopes.append((number, end or len(lines)))
    return scopes


def population(old, new, rows):
    """Declared addresses that land in `.rdata`/`.data` and hold no row in the data map.

    Resolved through `rva_symbols`, which evaluates values rather than matching spellings. That is
    the whole point: the data map's own refresh harvests `const *RVA*: usize = 0x..;` and nothing
    else, so an address written as a bare literal at its use site is invisible to it -- and four of
    them are, all read through `game_rva` in the menu tracer, all with no row.

    Each hit is annotated with what the image says about it, because that is the only thing that
    separates a missing address from a number:

      * whether any code in the game image references it rip-relatively, and where those references
        say it moved to on 1.17. A bound like `MODULE_SPAN_FALLBACK` (0x3000000) has zero
        references -- nothing in `eldenring.exe` addresses it, which is the proof it is arithmetic
        and not an address, arrived at without looking at its name;
      * whether every site that mentions it is inside a `#[cfg(test)]` scope.
    """
    sys.path.insert(0, os.path.join(REPO, "scripts"))
    import rva_symbols

    spans = [(rva, size) for name, rva, size in sections(old) if name in DATA_SECTIONS]

    def in_data(value):
        value = value - BASE if value >= BASE else value
        return value if any(rva <= value < rva + size for rva, size in spans) else None

    mapped = {src for src, _, _, _ in rows}
    index = rva_symbols.Index.build()
    scopes = {path: test_scopes(text) for path, text in index.text.items()}

    def under_test(path, line):
        return any(lo <= line <= hi for lo, hi in scopes.get(path, ()))

    found = collections.defaultdict(list)
    for declaration in index.decls:
        for value in declaration.value or ():
            rva = in_data(value)
            if rva is not None and rva not in mapped:
                found[rva].append(
                    (declaration.symbol, declaration.where(REPO),
                     under_test(declaration.path, declaration.line))
                )
    for literal in index.literals:
        rva = in_data(literal.value)
        if rva is not None and rva not in mapped:
            found[rva].append(
                ("(bare literal)", literal.where(REPO), under_test(literal.path, literal.line))
            )
    votes = accessor_votes(old, new, list(found))
    return {rva: (sorted(set(sites)), votes.get(rva, (None, 0, 0))) for rva, sites in found.items()}


def selftest(old_cm, new_cm):
    """Positive control and a regressed control, so a green here cannot be vacuous."""
    rows = read_rows(DATA_MAP)
    tally, results = run(rows, old_cm, new_cm, quiet=True)
    checked = tally["PROVEN"] + tally["ORDINAL"]
    print(f"selftest: {len(rows)} rows, {checked} carry RTTI identity, {tally['N/A']} are not vtables")
    if checked == 0:
        print("FAIL: nothing was checked -- the filter matched no rows, so any verdict is vacuous")
        return 1
    if tally["DISAGREE"] or tally["MISSING"]:
        print(f"FAIL: {tally['DISAGREE']} DISAGREE, {tally['MISSING']} MISSING on the tracked map")
        return 1
    print(f"  positive control: {checked} real rows verified clean")

    # Negative control. Regress every checked row's destination onto the next vtable in 1.17
    # and require the check to catch all of them. A matcher that has quietly stopped matching
    # -- an empty name table, a botched ordinal -- passes the green above and fails here.
    new_sorted = sorted(new_cm)
    following = {rva: new_sorted[i + 1] for i, rva in enumerate(new_sorted[:-1])}
    broken = []
    for code, src, dst, const, note, _ in results:
        if code in ("PROVEN", "ORDINAL") and dst in following:
            broken.append((src, following[dst], const, note))
    if not broken:
        print("FAIL: could not build a negative control -- nothing to regress")
        return 1
    bad_tally, _ = run(broken, old_cm, new_cm, quiet=True)
    if bad_tally["DISAGREE"] != len(broken):
        print(
            f"FAIL: negative control leaked -- {len(broken)} wrong destinations, "
            f"only {bad_tally['DISAGREE']} caught"
        )
        return 1
    print(f"  negative control: {len(broken)}/{len(broken)} shifted destinations rejected")
    return anchor_selftest(old_cm, new_cm)


# The address `er-hook/src/detour_site.rs` compares a PE section-header field against. It is not
# the address of anything, it is 0x1000 in both builds because every PE puts its first section
# there, and it has already been through this migration's machinery once: it entered
# `rva-map-1162-to-1170.needed.tsv` as `0x1000 -> 0x1000`, scored `IDENTICAL-WHOLE` on the byte
# comparison -- correctly, the bytes are identical -- and reached the detour-safe table.
#
# So it is the frozen negative here: a row that byte equality "verifies" and that this file must
# answer no-anchor about, because no accessor reads it, it holds no string and it is not a table.
# A matcher that has widened until it approves whatever it is handed goes red on this row while
# staying green on all 111 real ones.
FROZEN_NEGATIVE = 0x1000
FROZEN_NEGATIVE_NAME = "FIRST_SECTION_RVA (frozen negative -- a PE bound, not an address)"


def anchor_selftest(old_cm, new_cm):
    """Prove the four anchors can go red, in three independent directions.

    A green audit means nothing on its own: the rows it passes are the rows it was written from.
    So the tracked map is mutated three ways and the audit is required to catch each. Every mutant
    is applied to the rows in memory -- nothing is written -- and each is a shape a real regression
    takes:

      Reverted  someone puts a 1.16.2 address back in the destination column, which is exactly what
                a bad merge or a half-finished repoint leaves behind;
      NUDGED    the destination is off by eight bytes -- the neighbouring slot, the next vtable
                entry, one qword into the string. This is the mutant that separates a real anchor
                from "the address is readable": every wrong-but-plausible address is readable.
      Planted   the frozen negative, which must not come back verified.
    """
    old = open(OLD_IMAGE, "rb").read()
    new = open(NEW_IMAGE, "rb").read()
    rows = read_rows(DATA_MAP)
    # Primed in one pass, including the frozen negative. The two whole-image scans behind
    # `accessor_votes` cost ten seconds and are only paid when a requested source is missing from
    # the cache -- so asking for the frozen negative later, on its own, paid for them a second
    # time and took this selftest from 13s to 25s, which is the vacuity auditor's whole budget.
    accessor_votes(
        old,
        new,
        vote_sources(rows + [(FROZEN_NEGATIVE, FROZEN_NEGATIVE, FROZEN_NEGATIVE_NAME, "")]),
    )
    tally, results = run_anchors(rows, old, new, old_cm, new_cm, quiet=True)
    verified = [row for row in results if row[0] in VERIFIED_CODES]
    print(
        f"anchor selftest: {len(rows)} rows, {len(verified)} carry a non-byte-equality anchor "
        f"({dict(tally)})"
    )
    if not verified:
        print("FAIL: no row carries an anchor -- every verdict below would be vacuous")
        return 1
    if tally["DISAGREE"] or tally["MISSING"]:
        print(f"FAIL: {tally['DISAGREE']} DISAGREE, {tally['MISSING']} MISSING on the tracked map")
        return 1

    pinned = {
        pin_of(src, const)
        for code, src, _, const, _, _ in results
        if code in ("NO-ANCHOR", "ACCESSOR-WEAK")
    }
    print(f"  positive control: {len(verified)} rows verified, {len(pinned)} unanchored")

    # The mutants run before the pin check, deliberately. Both catch a widened matcher, but only
    # the mutants catch it for the right reason: the pin fires because a name moved between two
    # lists, which is bookkeeping, while a surviving mutant is the matcher failing to notice a
    # wrong address. Running the weaker check first would let it return early and leave the strong
    # one unexercised.
    for label, mutate in (
        ("REVERTED (destination put back to the 1.16.2 address)", lambda src, dst: src),
        ("NUDGED (destination moved eight bytes)", lambda src, dst: dst + 8),
    ):
        mutants = [(src, mutate(src, dst), const, note) for _, src, dst, const, note, _ in verified]
        bad_tally, bad = run_anchors(mutants, old, new, old_cm, new_cm, quiet=True)
        missed = [row for row in bad if row[0] in VERIFIED_CODES]
        if missed:
            for code, src, dst, const, _, detail in missed[:5]:
                print(f"FAIL: {label} -- 0x{src:x} -> 0x{dst:x} {const} still reads {code}: {detail}")
            print(f"FAIL: {len(missed)} of {len(mutants)} mutants survived {label}")
            return 1
        print(f"  {label}: {len(mutants)}/{len(mutants)} caught ({dict(bad_tally)})")

    if pinned != UNANCHORED:
        for const in sorted(pinned - UNANCHORED):
            print(f"FAIL: {const} lost its anchor and is not in UNANCHORED")
        for const in sorted(UNANCHORED - pinned):
            print(f"FAIL: {const} is anchored now -- remove it from UNANCHORED")
        return 1

    planted = [(FROZEN_NEGATIVE, FROZEN_NEGATIVE, FROZEN_NEGATIVE_NAME, "")]
    _, verdicts = run_anchors(planted, old, new, old_cm, new_cm, quiet=True)
    code = verdicts[0][0]
    if code in VERIFIED_CODES:
        print(
            f"FAIL: the frozen negative 0x{FROZEN_NEGATIVE:x} came back {code}. The bytes there ARE "
            "identical across the builds; a matcher that calls that verification approves anything."
        )
        return 1
    print(f"  frozen negative: 0x{FROZEN_NEGATIVE:x} answered {code}, as it must")

    # A constant delta is not an anchor, measured on the map rather than argued. Per-section modal
    # deltas are the best a constant can do, and the count below is how many addresses it still
    # gets wrong -- which is why the anchors above read evidence instead of doing arithmetic.
    per_section = collections.defaultdict(collections.Counter)
    unique = {}
    for src, dst, _, _ in rows:
        unique.setdefault(src, dst)
    for src, dst in unique.items():
        per_section[src < 0x3B11000][dst - src] += 1
    wrong = sum(
        count
        for section, spread in per_section.items()
        for delta, count in spread.items()
        if delta != spread.most_common(1)[0][0]
    )
    if wrong == 0:
        print("FAIL: every address moved by its section's modal delta, so this control proves nothing")
        return 1
    spreads = ", ".join(f"{len(spread)} deltas" for spread in per_section.values())
    print(f"  constant-delta control: {spreads}; the modal one is wrong on {wrong}/{len(unique)}")
    print("selftest OK")
    return 0


def prove_selftest_catches_regression(old_cm, new_cm):
    """Break the matcher on purpose and require --selftest to fail.

    A green selftest only means something if a broken instrument would go red. This
    monkey-patches `judge` into a matcher that waves every vtable row through, re-runs the
    whole selftest, and fails if that still passes. Eight instruments in this repo were caught
    reporting false greens on 2026-08-30, several because a filter matched nothing and an
    assertion then passed over the empty set; this is the check that would have caught them.
    """
    global judge
    real = judge

    def always_agree(src, dst, old_cm_, new_cm_, old_ord, new_key, old_names, new_names):
        return ("PROVEN", "REGRESSED-MATCHER") if src in old_cm_ else ("N/A", "not a vtable")

    judge = always_agree
    try:
        code = selftest(old_cm, new_cm)
    finally:
        judge = real
    if code == 0:
        print("FAIL: the selftest passed with a matcher that agrees with everything -- it is vacuous")
        return 1
    print("regression proof OK: a matcher that always agrees fails the selftest")

    # Second blind, for the anchor layer. The one above breaks `judge`, which is the RTTI half, and
    # a green there says nothing about the accessor / literal / code-pointer anchors -- those decide
    # 60 of the 92 verified rows and are exactly the part that had no audit before. So the whole
    # anchor layer is replaced by one that approves every row it is handed, and the selftest must
    # still go red: its mutants would survive and the frozen negative would come back verified.
    global audit
    real_audit = audit

    def approve_everything(rows, old, new, old_cm_, new_cm_):
        return [("ACCESSORS", src, dst, const, note, "BLINDED") for src, dst, const, note in rows]

    audit = approve_everything
    try:
        code = selftest(old_cm, new_cm)
    finally:
        audit = real_audit
    if code == 0:
        print("FAIL: the selftest passed with an anchor layer that approves every row -- vacuous")
        return 1
    print("regression proof OK: an anchor layer that approves everything fails the selftest")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--map", default=DATA_MAP)
    ap.add_argument("--old", default=OLD_IMAGE)
    ap.add_argument("--new", default=NEW_IMAGE)
    ap.add_argument("--deltas", action="store_true", help="whole-.rdata RTTI delta census")
    ap.add_argument(
        "--bracket",
        action="store_true",
        help="fence the NON-vtable rows with RTTI vtable anchors (independent of the data map)",
    )
    ap.add_argument(
        "--anchors",
        action="store_true",
        help="every row, every anchor: RTTI, accessors, literals, code pointers, string tables",
    )
    ap.add_argument(
        "--occupancy",
        action="store_true",
        help="what sits at each 1.17 address, and what a stale 1.16.2 read would return",
    )
    ap.add_argument(
        "--population",
        action="store_true",
        help="declared .rdata/.data addresses that hold no row in the map",
    )
    ap.add_argument("--selftest", action="store_true")
    ap.add_argument(
        "--prove-selftest-catches-regression",
        action="store_true",
        help="break the matcher on purpose; the selftest must go red",
    )
    args = ap.parse_args()

    # The two de-Arxan'd images are gitignored (copyrighted), so a fresh checkout and CI simply do
    # not have them. Skip at exit 0 rather than crash: a gate that cannot run must not be
    # indistinguishable from a gate that ran and failed.
    for path in (args.old, args.new, args.map):
        if not os.path.isfile(path):
            print(f"SKIP: missing {path}")
            return 0

    old_cm = classmap(args.old, "/tmp/er-rtti-1162.tsv")
    new_cm = classmap(args.new, "/tmp/er-rtti-1170.tsv")
    print(f"1.16.2: {len(old_cm)} vtables    1.17: {len(new_cm)} vtables")

    if args.prove_selftest_catches_regression:
        return prove_selftest_catches_regression(old_cm, new_cm)

    if args.selftest:
        return selftest(old_cm, new_cm)

    rows = read_rows(args.map)

    if args.anchors or args.occupancy or args.population:
        old = open(args.old, "rb").read()
        new = open(args.new, "rb").read()
        status = 0
        if args.anchors:
            print("\nanchored verdicts over EVERY row:")
            tally, results = run_anchors(rows, old, new, old_cm, new_cm)
            verified = sum(tally[code] for code in VERIFIED_CODES)
            print(f"\n  {dict(tally)}")
            print(f"  {verified}/{len(rows)} rows verified by an anchor that is not byte equality")
            unanchored = {
                pin_of(src, const)
                for code, src, _, const, _, _ in results
                if code in ("NO-ANCHOR", "ACCESSOR-WEAK")
            }
            for const in sorted(unanchored - UNANCHORED):
                print(f"  NEW UNANCHORED ROW: {const} -- it had evidence before and does not now")
                status = 1
            for const in sorted(UNANCHORED - unanchored):
                print(f"  note: {const} is anchored now -- shrink UNANCHORED (--selftest enforces)")
            if tally["DISAGREE"] or tally["MISSING"]:
                status = 1
        if args.occupancy:
            print("\nwhat sits at each address:")
            print("\n".join(occupancy(old, new, rows)))
        if args.population:
            print("\ndeclared .rdata/.data addresses with no row in the map:")
            live = 0
            # An address that is some row's 1.17 destination is not a missing row, it is a
            # deliberate 1.17 spelling -- `er-quickload/build.rs` and `er-save-suppress/build.rs`
            # both name `GameMan` at 0x143d6d988 because the constants they generate are compared
            # against the bytes of the running game. Saying "nothing references it" about those
            # would be true of the 1.16.2 image and useless.
            destinations = {dst: const for _, dst, const, _ in rows}
            for rva, (sites, (target, agreeing, total)) in sorted(population(old, new, rows).items()):
                shipped = [site for site in sites if not site[2]]
                if rva in destinations:
                    verdict = f"already the 1.17 destination of {destinations[rva]}"
                elif target is None and total == 0:
                    verdict = "nothing in the 1.16.2 image addresses it"
                elif target is None:
                    verdict = f"{total} reference site(s), none identifiable on both builds"
                elif not shipped:
                    verdict = f"reached only from #[cfg(test)]; would be 0x{target:x}"
                else:
                    verdict = f"LIVE: {agreeing} accessors say 1.17 0x{target:x} -- no row exists"
                    live += 1
                print(f"  0x{rva:x}  {verdict}")
                for symbol, where, in_test in sites:
                    print(f"      {symbol} {where}{'  [test]' if in_test else ''}")
            print(f"\n  {live} address(es) reached by shipped code with no row in the map")
        return status

    tally, _ = run(rows, old_cm, new_cm)
    print(f"\n{dict(tally)}")

    if args.bracket:
        print("\nRTTI-anchor bracket over non-vtable rows:")
        counts = collections.Counter()
        for code, src, dst, const, note, detail in bracket(rows, old_cm, new_cm):
            counts[code] += 1
            if code != "NO-ANCHOR":
                print(f"  {code:9s} 0x{src:x} -> 0x{dst:x}  {const}  [{note}]  {detail}")
        print(f"  {dict(counts)}")

    if args.deltas:
        census, per_region = delta_census(old_cm, new_cm)
        print("\nRTTI-only delta census over uniquely-named vtables:")
        for delta, count in sorted(census.items()):
            print(f"  {delta:+#x}  {count}")
        print("\nper 1 MiB region of 1.16.2 .rdata:")
        for region in sorted(per_region):
            spread = per_region[region]
            shown = ", ".join(f"{d:+#x}x{c}" for d, c in sorted(spread.items()))
            print(f"  0x{region << 20:x}: {shown}")

    return 1 if (tally["DISAGREE"] or tally["MISSING"]) else 0


if __name__ == "__main__":
    sys.exit(main())
