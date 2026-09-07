#!/usr/bin/env python3
"""Align every function in the 1.16.2 image with its 1.17 counterpart.

Why a whole-image map
---------------------
`scripts/map-rvas-1162-to-1170.py` answers one address at a time by searching a
window around it, and on a trial of five it resolved two.  That is fine for a
handful of hooks and useless for the 159 raw `transmute(base + *_RVA)` game
calls this migration still has to carry forward -- each of which, unmapped,
is a crash of the kind measured on 2026-08-29: `er_invasion_warp_core::warp::
native::current_block_id` called 1.16.2's `0x1405eefb0`, which on 1.17 is the
second byte of a five-byte `call`, and the `9a` it landed on is a far call --
invalid in long mode, so #UD, so a dead game 491ms after load.

Both images carry a complete function table already: `.pdata` is an array of
RUNTIME_FUNCTION records, one per function with unwind data, and that is very
nearly every function in the binary.  So instead of searching per address, walk
both tables once and match functions by content.

How the matching works
----------------------
A function's bytes cannot be compared literally across versions, because every
rip-relative displacement and every call target moved.  So each function gets a
SIGNATURE: its opening instructions with those operand bytes zeroed out, which
leaves opcodes, registers and immediates -- the parts that only change when the
code itself changes.

A signature that is unique on both sides identifies one function on each, so the
pair is unambiguous.  Signatures shared by several functions (thunks, tiny
forwarders, `jmp` stubs) are left unpaired rather than guessed at; this tool's
value is entirely in not being wrong.

What makes the result trustworthy
---------------------------------
Not the method -- the calibration.  123 pairs are known independently: the 96
RVAs the sibling's own `binary-mapper` resolved on both versions (62 of them
from RTTI class names, which involve no pattern matching at all) and the 27
addresses this project hooks successfully.  `--selftest` requires that every one
of those the map covers agrees, and that its own byte check passes.  A map that
disagreed with a single RTTI-derived pair would be reporting a bug in itself.
"""

from __future__ import annotations

import argparse
import json
import re
import struct
import sys
from pathlib import Path

BASE = 0x140000000
IMAGE_1162 = "eldenring-deobf.bin"
IMAGE_1170 = "eldenring-deobf-1.17.bin"
SIGNATURE_BYTES = 64


def _ensure_capstone():
    """Re-exec under uv when capstone is absent; there is no system pip here.

    The re-exec must name `python3`, not `sys.executable`: an absolute path to
    the system interpreter ignores the ephemeral environment uv just built, so
    the import fails again and the process re-execs forever.  uv stops that at
    100 rounds with a message about recursion that says nothing about capstone.
    """
    try:
        import capstone  # noqa: F401
    except ImportError:
        import os

        if os.environ.get("_MAP_UNDER_UV"):
            raise SystemExit("capstone is still missing under `uv run --with capstone`")
        os.environ["_MAP_UNDER_UV"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])


class Image:
    """A flat (virtual-layout) PE image where file offset == RVA."""

    def __init__(self, path: Path):
        self.path = path
        self.data = path.read_bytes()
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        if self.data[pe : pe + 4] != b"PE\0\0":
            raise SystemExit(f"{path}: not a PE image")
        nsec = struct.unpack_from("<H", self.data, pe + 6)[0]
        optsz = struct.unpack_from("<H", self.data, pe + 20)[0]
        off = pe + 24 + optsz
        self.sections = {}
        for i in range(nsec):
            e = self.data[off + i * 40 : off + (i + 1) * 40]
            name = e[:8].rstrip(b"\0").decode("latin1")
            vsz, va, rsz, _rp = struct.unpack_from("<IIII", e, 8)
            # Two sections share the name ".text"; the first is the real one.
            self.sections.setdefault(name, (va, max(vsz, rsz)))

    def function_starts(self) -> list[int]:
        """Every RUNTIME_FUNCTION start, ascending -- the order both images agree on."""
        return sorted({start for start, _end in self.functions()})

    def functions(self) -> list[tuple[int, int]]:
        """(start_rva, end_rva) for every RUNTIME_FUNCTION in .pdata."""
        va, size = self.sections[".pdata"]
        out = []
        for off in range(va, va + size, 12):
            begin, end, _unwind = struct.unpack_from("<III", self.data, off)
            if begin == 0 and end == 0:
                continue
            if end <= begin or end - begin > 0x20000:
                continue
            out.append((begin, end))
        return out


def signature(image: Image, start: int, end: int, md) -> bytes | None:
    """Opening instructions with rip-relative and branch operands zeroed."""
    length = min(end - start, SIGNATURE_BYTES)
    if length < 8:
        return None
    raw = bytearray(image.data[start : start + length])
    consumed = 0
    for insn in md.disasm(bytes(raw), BASE + start):
        size = insn.size
        pos = insn.address - BASE - start
        d_off, d_size = insn.disp_offset, insn.disp_size
        i_off, i_size = insn.imm_offset, insn.imm_size
        # A 4-byte displacement or immediate is where an address hides; smaller
        # ones are genuine structure offsets and constants, which are part of
        # what the function is and must stay in the signature.
        if d_size == 4:
            raw[pos + d_off : pos + d_off + 4] = b"\0\0\0\0"
        if i_size == 4 and insn.mnemonic in ("call", "jmp", "je", "jne", "jz", "jnz"):
            raw[pos + i_off : pos + i_off + 4] = b"\0\0\0\0"
        consumed = pos + size
    if consumed < 8:
        return None
    return bytes(raw[:consumed])


def build(md, image: Image) -> dict[bytes, list[int]]:
    table: dict[bytes, list[int]] = {}
    for start, end in image.functions():
        sig = signature(image, start, end, md)
        if sig is None:
            continue
        table.setdefault(sig, []).append(start)
    return table


# How many same-signature functions may be paired by position before the group is abandoned.
MAX_ORDERED_GROUP = 8
# The widest hole between two confirmed anchors that may be filled by position alone.
MAX_INTERPOLATED_GAP = 24


def pair(old_table, new_table) -> dict[int, int]:
    """Pair functions by signature.

    A signature unique on both sides identifies one function on each, so the pair is
    unambiguous and needs no further argument.

    A signature shared by exactly the same small number of functions on both sides is
    paired by address order. The justification is narrow and worth stating: these are
    near-identical siblings -- generated accessors, one-line forwarders, `jmp` thunks --
    and the compiler emits them in a stable relative order, so the k-th on one side is
    the k-th on the other. It is a weaker claim than uniqueness, so it is bounded to
    small groups, requires the counts to match exactly, and lives or dies by the
    calibration below: 41 pairs are known independently, and if ordered pairing invented
    a wrong one the selftest says so.

    Groups whose counts differ are dropped entirely. A differing count means functions
    were added or removed, which is precisely when position stops meaning anything.
    """
    mapping = {}
    for sig, olds in old_table.items():
        news = new_table.get(sig)
        if news is None:
            continue
        if len(olds) == 1 and len(news) == 1:
            mapping[olds[0]] = news[0]
        elif len(olds) == len(news) <= MAX_ORDERED_GROUP:
            for a, b in zip(sorted(olds), sorted(news)):
                mapping[a] = b
    return mapping


def interpolate(mapping, old_starts, new_starts):
    """Fill gaps between two confirmed anchors when both sides have the same count.

    Signature pairing leaves holes: a function whose opening bytes are shared with several
    siblings, or which changed between the builds, has nothing unique to match on. 86 of the
    136 addresses the running game asked for and was refused fall in those holes, and they are
    not incidental -- they are the ones the boot oracles hook, which is why boot progress reads
    as zero with the game alive behind the cover.

    A hole bounded by two confirmed pairs is still constrained. Functions appear in `.pdata` in
    address order on both sides, so if the gap holds N functions on the 1.16.2 side and exactly
    N on the 1.17 side, the k-th in one is the k-th in the other. That is the same
    order-preserving argument the small same-signature groups already use, applied between
    anchors instead of within a signature.

    A gap whose counts differ is skipped entirely, and so is one wider than
    MAX_INTERPOLATED_GAP: a differing count means functions were added or removed, which is
    exactly when position stops carrying information, and a long run of unanchored guesses is
    how a plausible wrong address gets manufactured.
    """
    anchors = sorted(mapping.items())
    old_index = {rva: i for i, rva in enumerate(old_starts)}
    new_index = {rva: i for i, rva in enumerate(new_starts)}
    added = 0
    for (old_a, new_a), (old_b, new_b) in zip(anchors, anchors[1:]):
        oi, ni = old_index.get(old_a), new_index.get(new_a)
        oj, nj = old_index.get(old_b), new_index.get(new_b)
        if None in (oi, ni, oj, nj):
            continue
        span_old, span_new = oj - oi - 1, nj - ni - 1
        if span_old != span_new or not 0 < span_old <= MAX_INTERPOLATED_GAP:
            continue
        for k in range(1, span_old + 1):
            src = old_starts[oi + k]
            if src not in mapping:
                mapping[src] = new_starts[ni + k]
                added += 1
    return added


def known_pairs(repo: Path, sibling: Path) -> dict[int, int]:
    """The 123 pairs established without this tool."""
    pairs: dict[int, int] = {}
    field = re.compile(r"^\s*(\w+):\s*(0x[0-9a-fA-F]+),", re.M)
    old_rs = sibling / "crates/eldenring/src/rva/rva_ww.rs"
    new_rs = sibling / "crates/eldenring/src/rva/rva_ww_270.rs"
    if old_rs.is_file() and new_rs.is_file():
        old = dict(field.findall(old_rs.read_text(encoding="utf-8")))
        new = dict(field.findall(new_rs.read_text(encoding="utf-8")))
        for name, value in old.items():
            if name in new:
                pairs[int(value, 16)] = int(new[name], 16)
    verified = repo / "docs/recon/rva-map-1162-to-1170.verified.tsv"
    if verified.is_file():
        for line in verified.read_text(encoding="utf-8").splitlines():
            if line.startswith("#") or not line.strip():
                continue
            cols = line.split("\t")
            if len(cols) < 2:
                continue
            try:
                a, b = int(cols[0], 16), int(cols[1], 16)
            except ValueError:
                continue
            pairs[a - BASE if a >= BASE else a] = b - BASE if b >= BASE else b
    return pairs


def main() -> int:
    _ensure_capstone()
    from capstone import CS_ARCH_X86, CS_MODE_64, Cs

    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    repo = Path(__file__).resolve().parent.parent
    ap.add_argument("--old", type=Path, default=repo / IMAGE_1162)
    ap.add_argument("--new", type=Path, default=repo / IMAGE_1170)
    ap.add_argument("--sibling", type=Path, default=repo.parent / "fromsoftware-rs")
    ap.add_argument("--out", type=Path, help="write the map as TSV here")
    ap.add_argument(
        "--interpolate",
        action="store_true",
        help="also fill gaps between anchors by position (OFF by default; see `interpolate`)",
    )
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()

    for path in (args.old, args.new):
        if not path.is_file():
            print(f"SKIP: no image at {path}")
            return 0

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True
    old_img, new_img = Image(args.old), Image(args.new)
    old_table, new_table = build(md, old_img), build(md, new_img)
    mapping = pair(old_table, new_table)
    signature_pairs = len(mapping)
    # Off by default, and the default was chosen by a game rather than by taste.
    #
    # Interpolation adds 62,863 pairs and keeps the calibration clean -- 45 known pairs, zero
    # disagreements -- which is exactly as reassuring as it sounds and no more: the calibration
    # set does not reach into the holes being filled. Enabling it alongside a second widening
    # took the translation table from 304 rows to 329 and killed the game at +145ms, during DLL
    # init, where 304 rows had survived past twenty seconds.
    #
    # A position-guessed pair is right whenever nothing was added or removed in the gap, and
    # silently wrong when something was. Signature pairs do not have that failure mode. So this
    # stays behind a flag until a row promoted this way can be checked individually.
    filled = interpolate(mapping, sorted(old_img.function_starts()), sorted(new_img.function_starts())) if args.interpolate else 0
    print(f"signature pairs {signature_pairs}, plus {filled} filled between anchors")
    print(
        f"functions: {sum(len(v) for v in old_table.values())} (1.16.2) "
        f"{sum(len(v) for v in new_table.values())} (1.17); paired {len(mapping)}"
    )

    known = known_pairs(repo, args.sibling)
    covered = {k: v for k, v in known.items() if k in mapping}
    wrong = {k: (mapping[k], v) for k, v in covered.items() if mapping[k] != v}
    print(f"calibration: {len(known)} known pairs, {len(covered)} covered by the map, {len(wrong)} disagree")
    for src, (got, want) in sorted(wrong.items())[:20]:
        print(f"  DISAGREE 0x{src:x}: map says 0x{got:x}, known-good is 0x{want:x}")

    if args.out:
        lines = ["# 1.16.2 RVA\t1.17 RVA\t(paired by masked-signature identity across .pdata)"]
        lines += [f"0x{k:x}\t0x{v:x}" for k, v in sorted(mapping.items())]
        args.out.write_text("\n".join(lines) + "\n", encoding="utf-8")
        print(f"wrote {args.out} ({len(mapping)} rows)")

    if args.selftest:
        failures = []
        if not covered:
            failures.append("no known pair is covered by the map -- calibration proved nothing")
        if wrong:
            failures.append(f"{len(wrong)} known pair(s) disagree with the map")
        # A mapped function must actually start where the map says it does: the
        # 1.17 side has to be a .pdata function start, not an address inside one.
        new_starts = {s for s, _e in new_img.functions()}
        stray = [k for k, v in list(mapping.items())[:5000] if v not in new_starts]
        if stray:
            failures.append(f"{len(stray)} mapping(s) do not land on a 1.17 function start")
        for line in failures:
            print(f"SELFTEST FAIL: {line}")
        print(f"selftest: {len(failures)} failure(s)")
        return 1 if failures else 0
    return 0


if __name__ == "__main__":
    sys.exit(main())
