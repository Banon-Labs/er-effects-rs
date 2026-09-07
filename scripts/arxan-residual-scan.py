#!/usr/bin/env python3
"""Is the de-Arxan'd image code everywhere a function is declared, or is there ciphertext left?

# The question

`eldenring-deobf-1.17.bin` is dearxan's output for the installed build, and everything this
workspace does on 1.17 -- every ledger row, every hook target, every gate that reads the flat
image -- assumes it is a faithful rendering of the game's code. dearxan proves the correctness of
what it decrypted (the plaintext is real code). It cannot prove COMPLETENESS: a region whose
guarding stub it never found would stay ciphertext, and every downstream verdict taken from those
bytes would be a verdict about noise, silently.

This is the completeness half, run the way it was first run on 2026-07-01 against the previous
build: walk every declared function and ask whether its bytes decode as code. A function that is
still encrypted cannot pass -- Arxan's at-rest filler differs from the plaintext in every single
byte (measured: of the 89309 bytes dearxan rewrites on 1.17, zero were already equal), so an
undecrypted function is maximally unlike code, not marginally.

# What counts as "declared"

Two sources, and the second is why this is not just the 2026-07-01 scan re-run:

  * `.pdata` -- the linker's own exception table. Authoritative, but blind to leaf FUNCTIONS: the
    x64 ABI omits unwind data for a function that allocates no stack and calls nothing, and this
    image has 146,715 holes between consecutive `.pdata` extents. A missing entry is not a missing
    function, so `.pdata` alone leaves most small getters unexamined.
  * `--functions`, a Ghidra function list (`scripts/dump-ghidra-function-list.py`). Ghidra's
    analysis finds ~366k functions against ~176k `.pdata` extents, so it reaches into those holes.

# The classifier, and how its thresholds were calibrated rather than guessed

Per function, over a prefix bounded by the function's extent (never by a byte count -- see
`scripts/function_extent.py` and the gate that enforces it):

    tail         bytes at the end of the slice capstone could not decode (an invalid opcode)
    common_frac  share of decoded instructions whose mnemonic is one a compiler emits
    distinct     distinct byte values in the prefix, over the prefix length

    Flagged  <=>  distinct >= 0.75  and  (tail >= 15  or  common_frac < 0.5)

Those numbers are not taste. 1.17 supplies a labelled dataset for free: the same 1371 spans exist
as ciphertext in the installed `eldenring.exe` and as plaintext in the deobfuscated image, so the
rule can be scored against known-encrypted and known-decrypted bytes at identical addresses. Over
the 831 spans of at least 32 bytes, measured 2026-08-31:

    ciphertext  flagged 88.2%   <- sensitivity
    plaintext   flagged  0.0%   <- zero false positives on decrypted code
    control     flagged  0.225% <- 4000 random `.pdata` functions

The `distinct` half alone separates them perfectly on those spans (1.000 against 0.000); it is the
`tail`/`common_frac` half that keeps ordinary-but-unusual code out. Sensitivity is per function, and
that is the honest way to read it: a single encrypted 64-byte prefix escapes 12% of the time, but a
missed region covers a run of functions, and three consecutive misses have probability 0.0017.
Random bytes decode into mostly-arithmetic x86 far more often than intuition suggests -- a uniform
byte stream scores common_frac 0.78 -- which is exactly why a common-mnemonic test alone was not
enough and the two-part rule is.

Flagging is not a defect count. The 2026-07-01 run flagged 13 functions out of 228,889 and every
one turned out to be Arxan control-flow obfuscation -- `jmp`/`jcc` trampolines whose entry lives in
Arxan's own `.text` -- which is a different protection layer, out of dearxan's decryption scope,
and present in a runtime dump too. So the output is a rate and a clustering, and the verdict is
about where the flags fall, not how many there are. A missed decryption is regional: it would put
a dense run of flags inside one address range, not a scatter.

    uv run --with capstone python3 scripts/arxan-residual-scan.py --selftest
    uv run --with capstone python3 scripts/arxan-residual-scan.py \\
        --image eldenring-deobf-1.17.bin --functions <funcs.tsv> --out <out.tsv>

capstone is not installed system-wide and there is no system pip; `uv run --with capstone`
provisions it ephemerally.
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import function_extent  # noqa: E402

BASE = function_extent.BASE
ROOT = Path(__file__).resolve().parents[1]

# How much of each function to judge. A prefix, not the body: a function's first bytes are its
# prologue, which is the most code-shaped part of it, so a prefix that fails is failing hard.
PREFIX_BYTES = 0x40
# Below this many bytes there is not enough signal to call anything, and the ABI's smallest leaves
# (`xor eax,eax; ret`) would be judged on three instructions.
MIN_BYTES = 8
COMMON_MIN = 0.5
DISTINCT_MIN = 0.75
# An invalid opcode truncates capstone's decode. Below the longest x86-64 instruction the shortfall
# is just the prefix cap cutting the last instruction in half, which says nothing.
TAIL_MIN = 15

# Mnemonics a compiler actually emits, by name or by family. Anything matching is "common"; the
# families cover the conditional branches, the conditional moves and the setcc block without
# spelling out sixty names.
COMMON_NAMES = frozenset(
    """mov movzx movsx movsxd lea push pop call ret leave nop int3 add sub cmp test and or xor not
    neg inc dec shl shr sar rol ror imul mul idiv div cdq cdqe cqo xchg movups movaps movdqa movdqu
    movsd movss movq movd xorps xorpd addss addsd subss subsd mulss mulsd divss divsd comiss comisd
    ucomiss ucomisd cvtsi2ss cvtsi2sd cvttss2si cvttsd2si pxor punpcklqdq vmovups vmovaps vmovdqa
    vmovdqu vxorps vzeroupper endbr64 bt bts btr sbb adc""".split()
)
COMMON_PREFIXES = ("j", "set", "cmov", "rep", "loop")


def is_common(mnemonic: str) -> bool:
    if mnemonic in COMMON_NAMES:
        return True
    return mnemonic.startswith(COMMON_PREFIXES)


def load_capstone():
    try:
        import capstone
    except ModuleNotFoundError:  # pragma: no cover - exercised only without uv
        print(
            "capstone is not importable. Run this under uv:\n"
            "  uv run --with capstone python3 scripts/arxan-residual-scan.py ...",
            file=sys.stderr,
        )
        raise SystemExit(2)
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = False
    return md


def classify(md, blob: bytes, va: int, end_off: int) -> tuple[int, float, float, int]:
    """`(tail_undecoded, common_frac, distinct_ratio, n_bytes)` for the function prefix at `va`.

    `end_off` is the slice's upper bound, resolved by the caller from the function's extent. It is
    passed in rather than computed here so the bound is one expression the reader can follow, and
    so this stays a pure scoring function the selftest can drive with synthetic bytes.
    """
    start = va - BASE
    body = blob[start:end_off]
    if len(body) < MIN_BYTES:
        return (0, 0.0, 0.0, len(body))
    decoded = 0
    total = 0
    common = 0
    for _addr, size, mnemonic, _ops in md.disasm_lite(body, va):
        decoded += size
        total += 1
        if is_common(mnemonic):
            common += 1
    common_frac = (common / total) if total else 0.0
    distinct_ratio = len(set(body)) / len(body)
    return (len(body) - decoded, common_frac, distinct_ratio, len(body))


def flagged(tail: int, common_frac: float, distinct_ratio: float) -> bool:
    """Not-code means unlike code on the byte axis and on the decode axis at once.

    Both halves earn their place: a run of `00` padding is not code but is not ciphertext either
    and fails the first, while a small hand-written thunk can fail the second and is ordinary. See
    the module docstring for the labelled scores behind the thresholds.
    """
    return distinct_ratio >= DISTINCT_MIN and (tail >= TAIL_MIN or common_frac < COMMON_MIN)


def entries_from_pdata(blob: bytes) -> list[int]:
    extents, _starts, _spans = function_extent.declared_functions(blob)
    return sorted(BASE + rva for rva in extents)


def entries_from_tsv(path: str) -> list[tuple[int, int]]:
    out = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) < 2:
                continue
            try:
                out.append((int(parts[0], 16), int(parts[1])))
            except ValueError:
                continue
    return out


def applied_regions(path: str) -> list[tuple[int, int]]:
    """Merged `(start_rva, end_rva)` spans dearxan applied, from a `dearxan-profile` region TSV."""
    spans = []
    with open(path, encoding="utf-8") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("\t")
            if len(parts) != 4 or parts[0] != "applied":
                continue
            rva = int(parts[2], 16)
            spans.append((rva, rva + int(parts[3])))
    spans.sort()
    merged: list[list[int]] = []
    for s, e in spans:
        if merged and s <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], e)
        else:
            merged.append([s, e])
    return [(s, e) for s, e in merged]


def covered(rva: int, merged: list[tuple[int, int]]) -> bool:
    lo, hi = 0, len(merged)
    while lo < hi:
        mid = (lo + hi) // 2
        if merged[mid][0] <= rva:
            lo = mid + 1
        else:
            hi = mid
    return bool(lo) and rva < merged[lo - 1][1]


def scan(image: str, functions: str | None, regions: str | None, out: str | None) -> int:
    blob = Path(image).read_bytes()
    md = load_capstone()
    merged = applied_regions(regions) if regions else []

    sizes: dict[int, int] = {}
    if functions:
        for va, size in entries_from_tsv(functions):
            if size > 0:
                sizes[va] = size
    entries = set(entries_from_pdata(blob))
    pdata_only = len(entries)
    entries.update(sizes)
    ordered = sorted(entries)

    print(f"image             = {image} ({len(blob)} bytes)")
    print(f"pdata_functions   = {pdata_only}")
    print(f"ghidra_functions  = {len(sizes)}")
    print(f"union_scanned     = {len(ordered)}")
    print(f"applied_spans     = {len(merged)}")

    rows = []
    scanned = 0
    skipped_short = 0
    skipped_extent = 0
    hits = 0
    in_applied = 0
    for va in ordered:
        # The extent, not a byte count. `body_end` answers from `.pdata`'s declared start, then an
        # enclosing extent, then a decoded leaf watermark, and returns None rather than guessing.
        # `limit` bounds only that last arm, so a leaf costs one prefix-sized decode.
        end = function_extent.body_end(blob, va, limit=PREFIX_BYTES)
        if end is None:
            size = sizes.get(va)
            if not size:
                skipped_extent += 1
                continue
            end = (va - BASE) + size
        end_off = min(end, (va - BASE) + PREFIX_BYTES)
        tail, common_frac, distinct_ratio, n = classify(md, blob, va, end_off)
        if n < MIN_BYTES:
            skipped_short += 1
            continue
        scanned += 1
        if flagged(tail, common_frac, distinct_ratio):
            hits += 1
            inside = covered(va - BASE, merged)
            in_applied += int(inside)
            rows.append((va, n, tail, common_frac, distinct_ratio, inside))

    print(f"scanned           = {scanned}")
    print(f"skipped_short     = {skipped_short}")
    print(f"skipped_no_extent = {skipped_extent}")
    rate = (hits / scanned) if scanned else 0.0
    print(f"flagged           = {hits}  ({rate * 100:.4f}%)")
    print(f"  of which inside a region dearxan DECRYPTED = {in_applied}")

    if rows:
        print("\n== FLAG CLUSTERING (1 MB buckets, VA) ==")
        buckets: dict[int, int] = {}
        for va, *_rest in rows:
            buckets[(va - BASE) >> 20] = buckets.get((va - BASE) >> 20, 0) + 1
        for b in sorted(buckets):
            print(f"  {BASE + (b << 20):#012x}..{BASE + ((b + 1) << 20):#012x}  {buckets[b]}")

    if out:
        with open(out, "w", encoding="utf-8") as fh:
            fh.write("va\tbytes\ttail\tcommon_frac\tdistinct\tinside_decrypted_region\n")
            for va, n, tail, cf, dr, inside in rows:
                fh.write(f"{va:x}\t{n}\t{tail}\t{cf:.3f}\t{dr:.3f}\t{int(inside)}\n")
        print(f"\nwrote {out}")
    return 0


def selftest() -> int:
    """Drive the classifier with bytes whose verdict is known, both ways.

    Vacuity is the failure this guards against: a scan that flags nothing is indistinguishable
    from a scan that cannot flag anything. So the negative control is real compiled code and the
    positive control is a deterministic byte stream standing in for ciphertext, and the test fails
    if either lands on the wrong side.
    """
    md = load_capstone()
    # A plausible x86-64 prologue: push/mov/sub/lea/call/add/pop/ret.
    code = bytes.fromhex(
        "48895c2408574883ec20488d0d97000000488bf9e8000000004883c4205fc3"
        "4883ec28488b0d00000000e8000000004885c07405488b00c3"
    )
    tail, common, distinct, n = classify(md, code, BASE, len(code))
    assert n == len(code), n
    assert not flagged(tail, common, distinct), f"real code flagged: {tail=} {common=} {distinct=}"
    assert common >= COMMON_MIN, common
    assert tail < TAIL_MIN, tail

    # Stand-in for ciphertext: a linear congruential stream, deterministic and byte-uniform, so
    # this test carries no game bytes and still reproduces exactly.
    state = 0x1234_5678
    noise = bytearray()
    while len(noise) < PREFIX_BYTES:
        state = (state * 1103515245 + 12345) & 0xFFFFFFFF
        noise.append((state >> 16) & 0xFF)
    ntail, ncommon, ndistinct, _n = classify(md, bytes(noise), BASE, len(noise))
    assert flagged(ntail, ncommon, ndistinct), f"noise passed: {ntail=} {ncommon=} {ndistinct=}"
    # The half that actually catches it here is the decode tail, not the mnemonic mix: a uniform
    # byte stream decodes into common arithmetic most of the time. Asserted so a future widening
    # of COMMON_NAMES cannot quietly turn this control into a pass for the wrong reason.
    assert ncommon >= COMMON_MIN, f"noise should score HIGH on common mnemonics: {ncommon}"
    assert ntail >= TAIL_MIN, ntail

    # A run of zero padding must not be flagged: it is not code, but it is not ciphertext either,
    # and the distinct-byte half of the rule is the only thing separating them.
    ztail, zcommon, zdistinct, _n = classify(md, b"\x00" * PREFIX_BYTES, BASE, PREFIX_BYTES)
    assert not flagged(ztail, zcommon, zdistinct), "zero padding flagged as ciphertext"
    assert zdistinct < DISTINCT_MIN, zdistinct

    # The region reader must merge and answer containment, or "inside a decrypted region" is noise.
    import tempfile

    with tempfile.NamedTemporaryFile("w", suffix=".tsv", delete=False, encoding="utf-8") as fh:
        fh.write("stage\tkind\trva\tsize\n")
        fh.write("applied\tTea\t1000\t16\n")
        fh.write("applied\tTea\t1010\t16\n")
        fh.write("declared\tTea\t9000\t16\n")
        tmp = fh.name
    try:
        merged = applied_regions(tmp)
        assert merged == [(0x1000, 0x1020)], merged
        assert covered(0x1005, merged) and covered(0x101F, merged)
        assert not covered(0x0FFF, merged) and not covered(0x1020, merged)
        assert not covered(0x9000, merged), "a DECLARED row must not count as decrypted"
    finally:
        os.unlink(tmp)

    print("arxan-residual-scan selftest OK")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--image", default=str(ROOT / "eldenring-deobf-1.17.bin"))
    ap.add_argument("--functions", help="Ghidra function list TSV (entry, size, name)")
    ap.add_argument("--regions", help="dearxan-profile region TSV, to mark decrypted spans")
    ap.add_argument("--out", help="write flagged functions here")
    ap.add_argument("--selftest", action="store_true")
    a = ap.parse_args()
    if a.selftest:
        return selftest()
    return scan(a.image, a.functions, a.regions, a.out)


if __name__ == "__main__":
    raise SystemExit(main())
