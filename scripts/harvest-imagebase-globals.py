#!/usr/bin/env python3
"""Harvest a 1.16.2 -> 1.17 global (.data/.rdata RVA) map from image-base-in-register reads.

Why this exists
---------------
`scripts/map-data-rvas-1162-to-1170.py` carries a global by the code that references it
rip-relatively.  A global that nothing reaches rip-relatively therefore cannot be carried at
all, and several sit in that ledger's unused list for exactly that reason.

But rip-relative is not the only way this image reaches a global.  The de-Arxan'd binary keeps
Arxan's image-base-in-register form in places:

    lea  r14, [rip - 0x1b2e983]        ; r14 = 0x140000000, the image base
    ...
    mov  ecx, [r14 + rax*4 + 0x3030aa0] ; <- 0x3030aa0 is an RVA, not a struct field

That composite displacement is a data RVA, and it is a reference the rip-relative scan never
sees.  `scripts/detect-struct-field-drift.py` had to split these out (they were 895 of its
1,129 apparent "field moves") and noted they amount to a free global map.  This is that map,
re-derived with the two things the by-product lacked: Proof the base register actually holds
the image base, and a witness list behind every pair so the vote can be audited.

How a pair is produced
----------------------
For every already-established function pair (`docs/recon/rva-map-1162-to-1170.functions.tsv`):

  1. decode both bodies (extents from `.pdata`, else the decoded-terminator sweep both other
     tools already share);
  2. in each body track which registers hold the image base -- set by `lea REG,[rip+d]` whose
     target is exactly 0x140000000, cleared when the register is written or (for a volatile
     register) when a `call` crosses it;
  3. collect, in order, every memory operand whose base is such a register;
  4. require the two bodies to expose the same number of such references and the same
     instruction shape at each, then pair them positionally.

Each pairing is one vote from one witness function.  Nothing is promoted on a single vote.

What this deliberately does not do
----------------------------------
It does not apply a delta.  The `.data` delta is neither constant nor monotonic -- +0x4060,
+0x4070, +0x4078 and +0x4080 all occur inside one region -- so "correcting" an address to agree
with its neighbours manufactures exactly the kind of confident wrong answer that took the game
down 894 ms after load.  An address with no votes is reported as having no votes.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import re
import struct
import sys
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000
# A composite displacement below this is a structure field, not an RVA.  Same threshold, and the
# same measured justification, as detect-struct-field-drift.py: across the whole scan the largest
# field-shaped displacement is 0xd18 and the smallest RVA-shaped one 0x289ce0.
GLOBAL_MIN = 0x100000
MAX_FUNCTION_BYTES = 0x4000

# Win64 volatile registers: a `call` destroys them, so an image base parked in one does not
# survive the call and must not be trusted after it.
VOLATILE = {"rax", "rcx", "rdx", "r8", "r9", "r10", "r11"}
REG64 = {
    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp",
    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15",
}
# Mnemonics that read their first operand without writing it.  Everything else that names a
# tracked register first is assumed to CLOBBER it -- the conservative direction: a missed
# invalidation would forge a global, a spurious one only loses a vote.
READ_ONLY_FIRST = {"cmp", "test", "push", "jmp", "call", "ret", "nop", "int3", "bt", "ucomiss",
                   "ucomisd", "comiss", "comisd", "prefetcht0", "prefetchnta"}

_LEA_RIP = re.compile(r"^(\w+), \[rip ([+-]) (0x[0-9a-f]+|\d+)\]$")


def load_split_memory():
    """Reuse detect-struct-field-drift.py's operand parser rather than writing a second one.

    Two implementations of this parse would drift apart, and the second one would be the one
    nobody re-checked.  That file's --selftest already exercises it against hand-written
    encodings including the awkward forms (`[rax + rdx*8 - 0x10]`, segment prefixes, absolutes).
    """
    path = ROOT / "scripts" / "detect-struct-field-drift.py"
    spec = importlib.util.spec_from_file_location("_dsfd", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class Image:
    def __init__(self, path: Path):
        self.path = path
        self.data = path.read_bytes()
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        nsec = struct.unpack_from("<H", self.data, pe + 6)[0]
        optsz = struct.unpack_from("<H", self.data, pe + 20)[0]
        off = pe + 24 + optsz
        self.sections: list[tuple[str, int, int]] = []
        for i in range(nsec):
            entry = self.data[off + i * 40 : off + (i + 1) * 40]
            name = entry[:8].rstrip(b"\0").decode("latin1")
            vsz, va, rsz, _raw = struct.unpack_from("<IIII", entry, 8)
            self.sections.append((name, va, max(vsz, rsz)))

    def section_of(self, rva: int) -> str:
        for name, va, size in self.sections:
            if va <= rva < va + size:
                return name
        return "?"

    def pdata_ends(self) -> dict[int, int]:
        for name, va, size in self.sections:
            if name == ".pdata":
                break
        else:
            return {}
        out: dict[int, int] = {}
        for off in range(va, va + size, 12):
            begin, end, _u = struct.unpack_from("<III", self.data, off)
            if begin == 0 or end <= begin or end - begin > 0x20000:
                continue
            out.setdefault(begin, end)
        return out


def image_base_refs(md, body: bytes, va: int):
    """Return `[(index, shape, rva)]` for every memory read through an image-base register.

    The register tracking is the whole point.  Without it a large struct displacement on an
    ordinary pointer is indistinguishable from an RVA, and the by-product set this replaces
    contained exactly that noise -- 'globals' at RVA 0x50300000 and inside `.text`.
    """
    dsfd = image_base_refs.dsfd
    insns = list(md.disasm_lite(body, va))
    holders: set[str] = set()
    refs: list[tuple[int, str, int]] = []
    for idx, (addr, size, mn, op) in enumerate(insns):
        if mn == "lea":
            m = _LEA_RIP.match(op)
            if m:
                reg = m.group(1)
                disp = int(m.group(3), 0)
                if m.group(2) == "-":
                    disp = -disp
                target = addr + size + disp
                if target == BASE and reg in REG64:
                    holders.add(reg)
                elif reg in holders:
                    holders.discard(reg)
                continue
        if holders:
            shape, mems = dsfd.split_memory(op)
            for base, disp in mems:
                if base in holders and disp >= GLOBAL_MIN:
                    refs.append((idx, mn + " " + shape, disp))
        if mn == "call":
            holders -= VOLATILE
            continue
        # Clobber check: the destination is the first operand for the forms that matter here.
        if holders and mn not in READ_ONLY_FIRST:
            first = op.split(",")[0].strip()
            if first in holders:
                holders.discard(first)
    return refs


def build_pairs(args, md, old: Image, new: Image, limit: int | None):
    dsfd = image_base_refs.dsfd
    leaf_extent = dsfd._sibling_leaf_extent()
    old_ends, new_ends = old.pdata_ends(), new.pdata_ends()
    old_starts, new_starts = set(old_ends), set(new_ends)

    votes: dict[int, Counter] = defaultdict(Counter)
    witnesses: dict[tuple[int, int], list[tuple[int, int]]] = defaultdict(list)
    stats = Counter()

    for line in (ROOT / args.map).read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        parts = line.split("\t")
        a_rva, b_rva = int(parts[0], 16), int(parts[1], 16)
        stats["pairs"] += 1
        if limit and stats["pairs"] > limit:
            break
        a_end = dsfd.extent_of(a_rva, old_ends, old.data, old_starts, leaf_extent)
        b_end = dsfd.extent_of(b_rva, new_ends, new.data, new_starts, leaf_extent)
        if a_end is None or b_end is None:
            stats["no-extent"] += 1
            continue
        if a_end - a_rva > MAX_FUNCTION_BYTES or b_end - b_rva > MAX_FUNCTION_BYTES:
            stats["too-big"] += 1
            continue
        a_refs = image_base_refs(md, old.data[a_rva:a_end], BASE + a_rva)
        if not a_refs:
            continue
        stats["fn-with-refs"] += 1
        b_refs = image_base_refs(md, new.data[b_rva:b_end], BASE + b_rva)
        if len(a_refs) != len(b_refs):
            stats["count-mismatch"] += 1
            continue
        ok = all(a[1] == b[1] for a, b in zip(a_refs, b_refs))
        if not ok:
            stats["shape-mismatch"] += 1
            continue
        stats["fn-paired"] += 1
        for (_ai, _sh, a_disp), (_bi, _sh2, b_disp) in zip(a_refs, b_refs):
            votes[a_disp][b_disp] += 1
            witnesses[(a_disp, b_disp)].append((a_rva, b_rva))
            stats["votes"] += 1
    return votes, witnesses, stats


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--old", default="eldenring-deobf.bin")
    ap.add_argument("--new", default="eldenring-deobf-1.17.bin")
    ap.add_argument("--map", default="docs/recon/rva-map-1162-to-1170.functions.tsv")
    ap.add_argument("--limit", type=int, default=0, help="only the first N function pairs (probe)")
    ap.add_argument("--out", default="", help="write the harvest as JSON here")
    args = ap.parse_args()

    try:
        import capstone
    except ImportError:
        import os
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])

    image_base_refs.dsfd = load_split_memory()
    md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_64)
    old = Image(ROOT / args.old)
    new = Image(ROOT / args.new)

    votes, witnesses, stats = build_pairs(args, md, old, new, args.limit or None)

    print(f"pairs read            {stats['pairs']}")
    print(f"functions with refs   {stats['fn-with-refs']}")
    print(f"  paired              {stats['fn-paired']}")
    print(f"  count mismatch      {stats['count-mismatch']}")
    print(f"  shape mismatch      {stats['shape-mismatch']}")
    print(f"total votes cast      {stats['votes']}")
    print(f"distinct 1.16.2 RVAs  {len(votes)}")

    unanimous = sum(1 for c in votes.values() if len(c) == 1)
    print(f"  unanimous           {unanimous}")
    print(f"  contested           {len(votes) - unanimous}")

    if args.out:
        payload = {
            "stats": dict(stats),
            "globals": [
                {
                    "old": old_rva,
                    "old_section": old.section_of(old_rva),
                    "candidates": [
                        {
                            "new": n,
                            "new_section": new.section_of(n),
                            "votes": v,
                            "delta": n - old_rva,
                            "witnesses": witnesses[(old_rva, n)][:12],
                        }
                        for n, v in counter.most_common()
                    ],
                }
                for old_rva, counter in sorted(votes.items())
            ],
        }
        Path(args.out).write_text(json.dumps(payload))
        print(f"wrote {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
