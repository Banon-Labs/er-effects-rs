#!/usr/bin/env python3
"""Resolve a 1.16.2 `.data` RVA into its 1.17 address by instruction-shape uniqueness.

Why this exists
---------------
`scripts/map-data-rvas-1162-to-1170.py` carries a global by VOTING: it finds every
rip-relative reference in 1.16.2 `.text`, maps each enclosing function onto 1.17 through
the function map, re-reads the same instruction there, and lets the references agree.
That needs the enclosing function to be mappable. A global whose only reference lives in
the de-Arxan'd image's trampoline rubble -- `jmp`/`int3` soup with no `.pdata` entry --
has nothing to vote with, and the mapper correctly reports `no usable reference` rather
than guessing. Two such globals cost real features:

    0x3d6c5e8  RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA   339,684 refusals in one session
    0x3d69920  SAVE_SERIALIZE_BYTES_RVA                     er-save-disable refused outright

This tool is the fallback for exactly that class, and it is deliberately narrow.

The argument
------------
A global has no content of its own, but the instruction that references it does. Blank the
displacement -- the one field a rebuild is guaranteed to change -- and what is left is the
opcode, the registers and any trailing immediate: a byte shape that a patch does not
rewrite. Then:

    Source   in 1.16.2, exactly N sites of that masked shape resolve to the address.
    Target   in 1.17,   exactly N sites of that same masked shape resolve to the candidate.

The shape alone does not select an address -- `mov byte ptr [rip+d], 1` reaches hundreds of
distinct addresses image-wide, and with N == 1 most of them have a count of 1 too. So the
shape confirms and a window selects. The window comes from the map's own already-carried
neighbours: the nearest independently-mapped anchors on each side of the address, and how
far each of them moved. A candidate has to be a shape match, inside the bracket, and the
only address in that bracket whose site count matches. Anything else prints unresolved with
the reason, because a missing address costs a feature and a confident wrong one cost a boot.

Both flat images are de-Arxan'd, so file offset == RVA and VA == 0x140000000 + offset.

    uv run --with capstone --with numpy python3 \
        scripts/resolve-data-addr-by-shape-uniqueness.py --selftest
    uv run --with capstone --with numpy python3 \
        scripts/resolve-data-addr-by-shape-uniqueness.py 0x3d6c5e8
"""

from __future__ import annotations

import argparse
import bisect
import json
import os
import struct
import sys
from pathlib import Path

BASE = 0x140000000
CHUNK = 1 << 22
DATA_MAP = "docs/recon/rva-map-1162-to-1170.data.tsv"

# Bytes that can sit between a rip-relative displacement and the end of its instruction. The
# displacement is relative to the end of the instruction, so a trailing immediate shifts the
# arithmetic by its own width. Scanning only tail 4 finds every plain read of a global and misses
# every `mov [x], imm` / `cmp [x], imm` -- which is the entire vocabulary of a single-byte flag,
# and both addresses this tool was written for are exactly that.
IMMEDIATE_TAILS = (4, 5, 6, 8)

# The two pairs the method was derived on. `--selftest` fails if either stops reproducing.
SELFTEST = {
    0x3D6C5E8: (0x3D70658, "RETURN_TITLE_FINAL_FUNCTOR_GLOBAL_FLAG_RVA"),
    0x3D69920: (0x3D6D990, "SAVE_SERIALIZE_BYTES_RVA"),
}


def _ensure(module: str) -> None:
    """Re-exec under uv when a decoder is missing. There is no system pip on this machine."""
    try:
        __import__(module)
    except ImportError:
        if os.environ.get("_SHAPEUNIQ_UNDER_UV"):
            raise SystemExit(f"{module} is still missing under uv")
        os.environ["_SHAPEUNIQ_UNDER_UV"] = "1"
        os.execvp(
            "uv",
            ["uv", "run", "--with", "capstone", "--with", "numpy", "python3", *sys.argv],
        )


class Image:
    """A flat de-Arxan'd PE image: file offset == RVA, VA == base + RVA."""

    def __init__(self, path: Path):
        self.path = path
        self.data = path.read_bytes()
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        nsec = struct.unpack_from("<H", self.data, pe + 6)[0]
        optsz = struct.unpack_from("<H", self.data, pe + 20)[0]
        off = pe + 24 + optsz
        self.sections: dict[str, tuple[int, int]] = {}
        for i in range(nsec):
            entry = self.data[off + i * 40 : off + (i + 1) * 40]
            name = entry[:8].rstrip(b"\0").decode("latin1")
            vsz, va, rsz, _ = struct.unpack_from("<IIII", entry, 8)
            self.sections.setdefault(name, (va, max(vsz, rsz)))
        self.text = self.sections[".text"]
        self.pdata = self.sections[".pdata"]
        self._ranges: list[tuple[int, int]] | None = None

    def function_ranges(self) -> list[tuple[int, int]]:
        """`(begin, end)` per `.pdata` entry. The end is kept, not just the start: the rubble a
        trampoline reference lives in is often a few bytes past the last real function, and a
        start-only containment test silently claims it, decodes linearly from an unrelated
        prologue and gets an answer by luck rather than by structure."""
        if self._ranges is None:
            va, size = self.pdata
            out = []
            for off in range(va, va + size, 12):
                begin, end, _ = struct.unpack_from("<III", self.data, off)
                if begin and end > begin and end - begin <= 0x20000:
                    out.append((begin, end))
            out.sort()
            self._ranges = out
        return self._ranges

    def section_of(self, rva: int) -> str:
        for name, (va, size) in self.sections.items():
            if va <= rva < va + size:
                return name
        return "?"


def reference_offsets(image: Image, target: int) -> list[int]:
    """Byte offsets in `.text` holding a 4-byte displacement that could address `target`.

    A displacement `d` stored at offset `i` addresses `i + tail + d`. Every plausible tail is
    scanned in one vectorised pass rather than decoding forty-three megabytes of instructions.
    These are candidates -- bytes that merely look right are discarded by the decode that follows.
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


def _decode_one(md, image: Image, at: int):
    """The single instruction starting at `at`, or None."""
    for insn in md.disasm(image.data[at : at + 20], BASE + at):
        return insn
    return None


def decode_reference(md, image: Image, disp_at: int, target: int):
    """The instruction whose displacement sits at `disp_at` and really addresses `target`.

    Boundary-free on purpose. The addresses this tool exists for are referenced from trampoline
    rubble with no `.pdata` entry, so decoding forward from an enclosing function start finds
    nothing at all -- which is indistinguishable from the address having no reference. Instead the
    instruction start is recovered backwards: a valid start satisfies `start + disp_offset ==
    disp_at` and `start + size + disp == target`, which pins it to a handful of candidates.

    Where a `.pdata` function does enclose the site, its forward linear decode is authoritative and
    is preferred, because a linear stream cannot be desynchronised by a byte that merely looks like
    an opcode. The backwards fallback breaks its own ties by asking whether the preceding bytes
    decode to an instruction that ends exactly at the candidate start, then by length.
    """
    ranges = image.function_ranges()
    i = bisect.bisect_right(ranges, (disp_at, 1 << 62)) - 1
    if i >= 0 and ranges[i][0] <= disp_at < ranges[i][1]:
        func = ranges[i][0]
        for insn in md.disasm(image.data[func : disp_at + 16], BASE + func):
            at = insn.address - BASE
            if at > disp_at:
                break
            if insn.disp_size == 4 and at + insn.disp_offset == disp_at:
                if at + insn.size + insn.disp == target:
                    return insn, at, "pdata-linear"
                break

    best = None
    for at in range(disp_at - 1, max(disp_at - 16, 0) - 1, -1):
        insn = _decode_one(md, image, at)
        if insn is None or insn.disp_size != 4:
            continue
        if at + insn.disp_offset != disp_at:
            continue
        if at + insn.size + insn.disp != target:
            continue
        lands = 0
        for back in range(1, 16):
            prev = _decode_one(md, image, at - back)
            if prev is not None and at - back + prev.size == at:
                lands = 1
                break
        key = (lands, insn.size)
        if best is None or key > best[0]:
            best = (key, insn, at)
    if best is None:
        return None, None, "no decode covers the displacement"
    return best[1], best[2], "backwards" + ("+predecessor" if best[0][0] else "")


def shape_of(insn) -> tuple[str, str, int]:
    """`(text, masked-bytes-hex, displacement offset)` for one referencing instruction.

    The four displacement bytes are zeroed. What survives is what a rebuild does not rewrite: the
    opcode, the ModRM/SIB registers and any trailing immediate. The text keeps only the mnemonic
    and the first operand, so the printed shape never quotes the displacement it just masked.
    """
    raw = bytearray(insn.bytes)
    for i in range(insn.disp_offset, min(insn.disp_offset + 4, len(raw))):
        raw[i] = 0
    return f"{insn.mnemonic} {insn.op_str.split(',')[0]}", raw.hex(), insn.disp_offset


def shape_sites(image: Image, shapes: list[tuple[str, str, int]]) -> dict[int, list[str]]:
    """`{addressed RVA: [site, ...]}` for every occurrence of these masked shapes in `.text`.

    Deliberately boundary-free, and deliberately counted the same way in both images: a byte run
    that merely looks like the shape is noise, but it is noise of the same kind on both sides, so
    it cannot manufacture an agreement that is not there.
    """
    va, size = image.text
    found: dict[int, list[str]] = {}
    for text, hexbytes, disp_at in shapes:
        raw = bytes.fromhex(hexbytes)
        head, tail = raw[:disp_at], raw[disp_at + 4 :]
        at = image.data.find(head, va, va + size) if head else va
        while at >= 0 and at + len(raw) <= va + size:
            if image.data[at + disp_at + 4 : at + len(raw)] == tail:
                disp = int.from_bytes(
                    image.data[at + disp_at : at + disp_at + 4], "little", signed=True
                )
                found.setdefault(at + len(raw) + disp, []).append(f"{text} @0x{at:x}")
            nxt = image.data.find(head, at + 1, va + size) if head else at + 1
            if nxt < 0:
                break
            at = nxt
    return found


def load_anchors(path: Path) -> list[tuple[int, int]]:
    """`(1.16.2 RVA, 1.17 RVA)` from the tracked data map, sorted, deduplicated."""
    pairs: dict[int, int] = {}
    if not path.is_file():
        return []
    for line in path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) < 2:
            continue
        try:
            src, dst = int(parts[0], 16), int(parts[1], 16)
        except ValueError:
            continue
        pairs.setdefault(src, dst)
    return sorted(pairs.items())


def bracket_window(
    anchors: list[tuple[int, int]], rva: int, each_side: int, slack: int
) -> tuple[int, int, list[int], str]:
    """The 1.17 window `rva` must land in, from how its nearest mapped neighbours moved.

    The window is what selects an address; the shape only confirms it. Anchors are taken from the
    map's own independently-carried rows, the row for `rva` itself excluded so nothing confirms
    itself. When the neighbours disagree the window simply widens, which shows up as an ambiguous
    verdict rather than a quiet coin-flip.
    """
    others = [(s, d) for s, d in anchors if s != rva]
    if not others:
        return 0, 0, [], "no anchors in the map"
    keys = [s for s, _ in others]
    i = bisect.bisect_left(keys, rva)
    picked = others[max(0, i - each_side) : i + each_side]
    if not picked:
        return 0, 0, [], "no anchors near the address"
    deltas = sorted({d - s for s, d in picked})
    lo = rva + deltas[0] - slack
    hi = rva + deltas[-1] + slack
    note = (
        f"{len(picked)} anchor(s), delta "
        + (f"+0x{deltas[0]:x}" if len(deltas) == 1 else f"+0x{deltas[0]:x}..+0x{deltas[-1]:x}")
        + f", slack 0x{slack:x}"
    )
    return lo, hi, deltas, note


def resolve(md, old: Image, new: Image, rva: int, window, anchors, each_side, slack) -> dict:
    """One address, one verdict. `RESOLVED` only when the count matches and nothing else fits."""
    out: dict = {"rva": rva, "verdict": "UNRESOLVED", "reason": "", "moved": None}
    if not 0 < rva < len(old.data):
        # A mistyped VA (`0x1403d6c5e8` for `0x143d6c5e8`) otherwise reaches the scan, finds no
        # reference to an address that is not in the image, and is answered "no usable reference"
        # -- an honest-sounding verdict about a question nobody asked.
        out["reason"] = f"0x{rva:x} is outside the 1.16.2 image (0x1..0x{len(old.data) - 1:x})"
        return out

    offsets = reference_offsets(old, rva)
    shapes: list[tuple[str, str, int]] = []
    sites: list[str] = []
    for disp_at in offsets:
        insn, at, how = decode_reference(md, old, disp_at, rva)
        if insn is None:
            continue
        shapes.append(shape_of(insn))
        sites.append(f"{shape_of(insn)[0]} @0x{at:x} ({how})")
    out["ref_sites"] = sites
    if not shapes:
        out["reason"] = (
            f"no rip-relative instruction in 1.16.2 .text addresses it "
            f"({len(offsets)} byte-level candidate(s), none decoded)"
        )
        return out

    # Duplicates collapse: two references of the same shape search for the same bytes, and counting
    # that search twice would double every site on both sides symmetrically for no gain.
    unique_shapes = sorted({s for s in shapes})
    src = shape_sites(old, unique_shapes)
    src_n = len(src.get(rva, []))
    out["shapes"] = [s[0] for s in unique_shapes]
    out["src_sites"] = src_n
    out["src_addresses_reached"] = len(src)
    if src_n == 0:
        out["reason"] = "the shape scan cannot even find the 1.16.2 reference it was taken from"
        return out

    if window:
        lo, hi = window
        wnote = f"explicit 0x{lo:x}..0x{hi:x}"
        deltas: list[int] = []
    else:
        lo, hi, deltas, wnote = bracket_window(anchors, rva, each_side, slack)
        if not deltas:
            out["reason"] = f"no search window: {wnote}"
            return out
    out["window"] = [lo, hi]
    out["window_note"] = wnote
    out["anchor_deltas"] = [hex(d) for d in deltas]

    dst = shape_sites(new, unique_shapes)
    out["dst_addresses_reached"] = len(dst)
    out["dst_count_matches_image_wide"] = sum(1 for v in dst.values() if len(v) == src_n)
    in_window = {a: v for a, v in dst.items() if lo <= a <= hi}
    out["in_window"] = {hex(a): len(v) for a, v in sorted(in_window.items())}
    matching = [a for a, v in in_window.items() if len(v) == src_n]

    if not matching:
        out["reason"] = (
            f"no 1.17 address in the window is reached by the same shape {src_n} time(s) "
            f"(window holds {len(in_window)} shape target(s))"
        )
        return out
    if len(matching) > 1:
        out["reason"] = "AMBIGUOUS: " + ", ".join(f"0x{a:x}" for a in sorted(matching))
        return out

    moved = matching[0]
    out["verdict"] = "RESOLVED"
    out["moved"] = moved
    out["delta"] = moved - rva
    out["dst_sites"] = [s for s in dst[moved]]
    out["section"] = new.section_of(moved)
    # The delta is not a global constant and must not be treated as one. Across the tracked map
    # fourteen distinct deltas occur, clustered by region: +0x3080 dominates `.rdata`, +0x4070 the
    # low `.data`, +0x4080 the high. So a delta differing from a neighbour's is not grounds to
    # reject a resolution -- `MOVEMAPSTEP_GLOBAL_DISABLE` really does move +0x4071 with +0x4070 on
    # both sides of it. But a delta no neighbour shares is one step weaker than one they do, and
    # saying so is free.
    out["neighbour_agreement"] = out["delta"] in deltas if deltas else None
    if deltas and out["delta"] not in deltas:
        out["confidence"] = "LOWER -- delta matches no neighbouring anchor"
    return out


def report(res: dict) -> None:
    rva = res["rva"]
    if res["verdict"] == "RESOLVED":
        print(
            f"0x{rva:x}  ->  0x{res['moved']:x}   RESOLVED  delta +0x{res['delta']:x}  "
            f"[{res['section']}]"
        )
        if res.get("confidence"):
            print(
                f"    CONFIDENCE  {res['confidence']} "
                f"({', '.join(res.get('anchor_deltas', []))}) -- not a reason to reject it, but "
                "corroborate before merging"
            )
    else:
        print(f"0x{rva:x}  ->  UNRESOLVED   {res['reason']}")
    for site in res.get("ref_sites", [])[:6]:
        print(f"    1.16.2 ref  {site}")
    if "shapes" in res:
        print(
            f"    shape(s)    {', '.join(res['shapes'][:4])}"
            f"   (displacement masked; operand text is the 1.16.2 one)"
        )
        print(
            f"    1.16.2      {res['src_sites']} site(s) reach the source; "
            f"the shape reaches {res['src_addresses_reached']} address(es) image-wide"
        )
    if "window" in res:
        print(f"    window      0x{res['window'][0]:x}..0x{res['window'][1]:x}  ({res['window_note']})")
    if "dst_addresses_reached" in res:
        print(
            f"    1.17        the shape reaches {res['dst_addresses_reached']} address(es) "
            f"image-wide, {res['dst_count_matches_image_wide']} of them {res['src_sites']} time(s); "
            f"{len(res.get('in_window', {}))} inside the window"
        )
    for site in res.get("dst_sites", [])[:6]:
        print(f"    1.17 ref    {site}")


def calibrate(md, old: Image, new: Image, map_path: Path, anchors, each_side, slack, limit) -> int:
    """Ask the shape method for addresses the map already answers by independent evidence.

    The two pairs in `--selftest` are the cases the method was derived on, so passing them proves
    only that it is self-consistent. This asks it for rows carried by reference voting instead --
    a different mechanism, decided without reference to any shape -- and scores three outcomes:

        agree    the shape method returns the address the vote already agreed on.
        refuse   it returns unresolved. Costs nothing; the row is carried by the vote.
        Wrong    it returns a different address. That is the failure this whole file is built to
                 avoid, and one instance invalidates the method rather than the row.

    Rows whose own suffix says they were rescued (`shape`, `rtti`, `bracket`, ...) are skipped:
    they are not independent of the fallbacks. So are rows with a huge reference count, whose
    dozens of distinct shapes each cost a full pass over both `.text` sections.
    """
    rows = []
    for line in map_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        parts = line.rstrip("\n").split("\t")
        if len(parts) < 4:
            continue
        votes = parts[3]
        if not votes.replace("/", "").isdigit():
            continue  # a suffix means a rescue carried it, so it is not independent evidence
        best, total = (int(x) for x in votes.split("/"))
        if best < 2 or total > 60:
            continue
        rows.append((int(parts[0], 16), int(parts[1], 16), parts[2], votes))
    rows.sort()
    if limit and len(rows) > limit:
        step = len(rows) / limit
        rows = [rows[int(i * step)] for i in range(limit)]

    agree = refuse = wrong = 0
    for src, want, name, votes in rows:
        res = resolve(md, old, new, src, None, anchors, each_side, slack)
        if res["verdict"] != "RESOLVED":
            refuse += 1
            verdict = "refuse"
        elif res["moved"] == want:
            agree += 1
            verdict = "agree "
        else:
            wrong += 1
            verdict = "WRONG "
        got = f"0x{res['moved']:x}" if res["moved"] else res["reason"][:52]
        print(f"  {verdict} {name:44s} 0x{src:x} -> want 0x{want:x}  got {got}  [{votes}]")
    print(f"calibrate: {len(rows)} row(s)  {agree} agree, {refuse} refuse, {wrong} WRONG")
    return 1 if wrong else 0


def selftest(md, old: Image, new: Image, anchors, each_side, slack) -> int:
    bad = 0
    for rva, (want, name) in sorted(SELFTEST.items()):
        res = resolve(md, old, new, rva, None, anchors, each_side, slack)
        ok = res["verdict"] == "RESOLVED" and res["moved"] == want
        print(f"  {'ok   ' if ok else 'FAIL '} {name}")
        report(res)
        if not ok:
            got = f"0x{res['moved']:x}" if res["moved"] else res["reason"]
            print(f"  SELFTEST FAIL: 0x{rva:x} wanted 0x{want:x}, got {got}")
            bad += 1
    print(f"selftest: {len(SELFTEST) - bad}/{len(SELFTEST)} reproduced")
    return 1 if bad else 0


def main() -> int:
    _ensure("capstone")
    _ensure("numpy")
    import capstone

    repo = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rvas", nargs="*", help="1.16.2 data RVAs or VAs (hex)")
    ap.add_argument("--old", default=str(repo / "eldenring-deobf.bin"))
    ap.add_argument("--new", default=str(repo / "eldenring-deobf-1.17.bin"))
    ap.add_argument("--map", default=str(repo / DATA_MAP), help="anchors for the bracket window")
    ap.add_argument("--window", help="explicit 1.17 window LO:HI (hex), instead of the bracket")
    ap.add_argument("--anchors", type=int, default=6, help="mapped neighbours per side (default 6)")
    ap.add_argument("--slack", default="0x100", help="widen the bracket window (hex, default 0x100)")
    ap.add_argument("--json", help="write the verdicts to this path")
    ap.add_argument("--selftest", action="store_true", help="reproduce the two derived pairs, or fail")
    ap.add_argument(
        "--calibrate",
        nargs="?",
        type=int,
        const=24,
        help="cross-check the method against rows the map carries by reference voting (default 24)",
    )
    args = ap.parse_args()

    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    md.detail = True
    old, new = Image(Path(args.old)), Image(Path(args.new))
    anchors = load_anchors(Path(args.map))
    slack = int(args.slack, 16)

    window = None
    if args.window:
        lo_s, hi_s = args.window.split(":")
        window = (int(lo_s, 16), int(hi_s, 16))

    if args.selftest:
        return selftest(md, old, new, anchors, args.anchors, slack)
    if args.calibrate is not None:
        return calibrate(
            md, old, new, Path(args.map), anchors, args.anchors, slack, args.calibrate
        )
    if not args.rvas:
        ap.error("give at least one RVA, or --selftest")

    results = []
    worst = 0
    for text in args.rvas:
        value = int(text, 16)
        rva = value - BASE if value >= BASE else value
        res = resolve(md, old, new, rva, window, anchors, args.anchors, slack)
        report(res)
        results.append(res)
        if res["verdict"] != "RESOLVED":
            worst = 1
    if args.json:
        Path(args.json).write_text(json.dumps(results, indent=2), encoding="utf-8")
    return worst


if __name__ == "__main__":
    sys.exit(main())
