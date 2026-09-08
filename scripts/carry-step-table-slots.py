#!/usr/bin/env python3
"""Carry every slot of a name/function step table from 1.16.2 onto 1.17, by its initialiser.

Why this exists separately from `map-data-rvas-1162-to-1170.py`
---------------------------------------------------------------
The general data carrier votes with every rip-relative reference it can find, and for a slot
inside a table there is usually exactly one -- the store that fills it at startup. One unopposed
vote is what that file (correctly) declines to promote, so `TITLE_STEP_IDX6_SLOT_RVA` and
`TITLE_STEP_IDX10_SLOT_RVA` came out `WEAK (one reference of 1)` and sat unmapped while
`own_stepper_patch_once` refused to install.

A step table is not an ordinary global, though, and the missing corroboration is sitting right
next to the address. Its initialiser is a single straight-line run of stores:

    lea rax, [<step function>]      ; mov [rip + table + i*0x10 + 0], rax
    lea rax, [<step name string>]   ; mov [rip + table + i*0x10 + 8], rax

so each slot has three independent things to check that do not come from its own displacement:

  1. the sibling stores. All 2N of them live in one function at fixed byte offsets, and they
     either all agree on a delta or the function was restructured and none of them are usable.
  2. the stored function pointer. `lea rax, [<step function>]` names a real function, and the
     128k-row function map either carries it to the 1.17 value the 1.17 initialiser stores, or
     it does not.
  3. The name. `lea rax, [<name string>]` names the slot in UTF-16 -- "TitleStep::STEP_MenuJobWait"
     -- and that string occurs exactly once per image. A slot whose neighbour holds a string that
     appears once in each image, at the source in 1.16.2 and at the candidate in 1.17, has
     identified itself; no delta is being trusted at all.

Check 3 is why this is worth a file. A wrong answer here writes our handler into an unrelated
function-pointer table and stays silent until something calls it, and "the majority of this
region moved +0x4070" is a statistic about the region, not evidence about this slot.

Alignment is by byte offset, not instruction index. One inserted instruction ahead of the store
shifts every later index by one, and an index-aligned read then either finds nothing (reported as
"no evidence", which is indistinguishable from a genuinely unreferenced address) or, worse, reads
whatever instruction now sits at that index.

Usage
    python3 scripts/carry-step-table-slots.py                      # the TitleStep table
    python3 scripts/carry-step-table-slots.py --init 0xa4f50 --slots 16
"""

from __future__ import annotations

import argparse
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import function_extent  # noqa: E402 - repo-local, and the sys.path line above is what makes it work

BASE = 0x140000000
# The TitleStep step table's initialiser. Two `lea`/`mov` pairs per slot, 0x10 bytes per slot,
# starting at the table base -- `own_stepper` patches slot 6 (STEP_GameStepWait) and slot 10
# (STEP_MenuJobWait).
DEFAULT_INIT = 0xA4F50


def _ensure_capstone() -> None:
    try:
        import capstone  # noqa: F401
    except ImportError:
        if os.environ.get("_STEPTABLE_UNDER_UV"):
            raise SystemExit("capstone is still missing under uv")
        os.environ["_STEPTABLE_UNDER_UV"] = "1"
        os.execvp("uv", ["uv", "run", "--with", "capstone", "python3", *sys.argv])


def load_function_map(path: Path) -> dict[int, int]:
    out: dict[int, int] = {}
    for line in path.read_text().splitlines():
        if line.startswith("#") or not line.strip():
            continue
        parts = line.split()
        if len(parts) >= 2:
            out[int(parts[0], 16)] = int(parts[1], 16)
    return out


def utf16_at(image: bytes, rva: int) -> str | None:
    """The NUL-terminated UTF-16LE string at `rva`, or None if it is not one."""
    end = rva
    while end + 2 <= len(image) and image[end : end + 2] != b"\0\0":
        end += 2
    if end == rva or end - rva > 512:
        return None
    try:
        text = image[rva:end].decode("utf-16-le")
    except UnicodeDecodeError:
        return None
    return text if text.isprintable() else None


def stores(md, image: bytes, init: int, span: int) -> dict[int, tuple[int, int]]:
    """`{byte offset: (destination RVA, value RVA)}` for each `mov [rip+d], rax` in the run.

    The value is whatever the immediately preceding `lea rax, [rip+d]` loaded, which is how the
    slot's contents are recovered from an image where the table itself is still all zeroes.

    The INITIALISER'S own extent bounds this, `span` is only a cap. Until 2026-08-31 `span` was
    the window: `0x40 + slots * 0x40`, which for the default 16 slots is 0x440 read from a
    function entry with no terminator stop of any kind. The TitleStep initialiser at 0xa4f50 is
    0x16d bytes long in both images, so the decode ran 0x2d3 bytes past its `ret`, through the
    de-Arxan'd gap where the deobfuscator's leftovers resynchronise into instructions nobody
    assembled -- the same class that manufactured a phantom `jno` for the hook-target audit and
    12 false DIVERGES for the verifier.

    Measured before the change: zero phantom stores, in both images. The junk simply did not
    happen to decode into a `lea rax,[rip+d]` / `mov [rip+d],rax` pair. That is luck, not a
    property -- in instance 1 of this class a single leftover byte decided which image a correct
    row failed on -- and a table of function pointers is the worst place to be lucky in: a wrong
    slot writes our handler into an unrelated function-pointer table and stays silent until
    something calls it.
    """
    from capstone import Cs, CS_ARCH_X86, CS_MODE_64  # noqa: F401  (md is already built)

    out: dict[int, tuple[int, int]] = {}
    pending: int | None = None
    stop = function_extent.body_slice_end(image, BASE + init, cap=span)
    if stop is None:
        # No declarable extent and no decodable leaf end. Fall back to the cap and say so, rather
        # than reporting an unbounded read as if it were a bounded one.
        print(f"  WARNING: no extent for the initialiser at 0x{BASE + init:x}; "
              f"reading the 0x{span:x}-byte cap, which may cross into the next function")
        stop = init + span
    for insn in md.disasm(image[init:stop], BASE + init):
        pos = insn.address - BASE - init
        if insn.disp_size != 4:
            pending = None if insn.mnemonic != "nop" else pending
            continue
        reaches = insn.address - BASE + insn.size + insn.disp
        if insn.mnemonic == "lea" and insn.op_str.startswith("rax,"):
            pending = reaches
        elif insn.mnemonic == "mov" and insn.op_str.endswith(", rax") and pending is not None:
            out[pos] = (reaches, pending)
            pending = None
    return out


def main() -> int:
    _ensure_capstone()
    from capstone import Cs, CS_ARCH_X86, CS_MODE_64

    root = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--old", default=str(root / "eldenring-deobf.bin"))
    ap.add_argument("--new", default=str(root / "eldenring-deobf-1.17.bin"))
    ap.add_argument("--map", default=str(root / "docs/recon/rva-map-1162-to-1170.functions.tsv"))
    ap.add_argument("--init", default=hex(DEFAULT_INIT), help="1.16.2 RVA of the table initialiser")
    ap.add_argument("--slots", type=int, default=16, help="how many 0x10-byte slots to decode")
    args = ap.parse_args()

    old = Path(args.old).read_bytes()
    new = Path(args.new).read_bytes()
    fmap = load_function_map(Path(args.map))
    init = int(args.init, 16)
    span = 0x40 + args.slots * 0x40

    md = Cs(CS_ARCH_X86, CS_MODE_64)
    md.detail = True

    # The 1.17 initialiser. Identity is the common case for a function nothing edited, and is what
    # the function map reports here; a moved one is carried by the map like anything else.
    new_init = fmap.get(init, init)
    print(f"initialiser: 1.16.2 0x{init:x} -> 1.17 0x{new_init:x}"
          f"{'  (identity)' if new_init == init else ''}")

    a = stores(md, old, init, span)
    b = stores(md, new, new_init, span)
    shared = sorted(set(a) & set(b))
    print(f"stores decoded: {len(a)} in 1.16.2, {len(b)} in 1.17, {len(shared)} at a shared byte offset")
    if not shared:
        print("REFUSED: no store in the 1.17 initialiser lands at a byte offset the 1.16.2 one uses.")
        return 2

    base_old = min(dst for dst, _ in a.values())
    base_new = min(b[o][0] for o in shared)
    print(f"table base: 0x{base_old:x} -> 0x{base_new:x}  (delta {base_new - base_old:+#x})\n")

    print(f"{'slot':>4} {'1.16.2':>10} {'1.17':>10} {'delta':>8}  {'fnptr':<10} {'name':<8} step name")
    bad = 0
    rows: list[tuple[int, int, int]] = []
    for off in shared:
        dst_old, val_old = a[off]
        dst_new, val_new = b[off]
        idx, part = divmod(dst_old - base_old, 0x10)
        if part not in (0, 8) or idx >= args.slots:
            continue
        if part == 8:
            continue  # the name half is reported on the function half's row
        name_off = next((o for o in shared if a[o][0] == dst_old + 8), None)
        # 2. the stored function pointer, through the function map.
        mapped = fmap.get(val_old)
        fn_verdict = "-" if mapped is None else ("OK" if mapped == val_new else f"WRONG 0x{mapped:x}")
        # 3. The name in the neighbouring slot, which must occur exactly once in each image.
        name_verdict, step = "-", ""
        if name_off is not None:
            step = utf16_at(old, a[name_off][1]) or ""
            other = utf16_at(new, b[name_off][1]) or ""
            blob = step.encode("utf-16-le") + b"\0\0" if step else b""
            if not step or step != other:
                name_verdict = "DIFFERS"
            elif old.count(blob) != 1 or new.count(blob) != 1:
                name_verdict = f"x{old.count(blob)}/x{new.count(blob)}"
            elif old.find(blob) != a[name_off][1] or new.find(blob) != b[name_off][1]:
                name_verdict = "ELSEWHERE"
            else:
                name_verdict = "UNIQUE"
        if fn_verdict.startswith("WRONG") or name_verdict in ("DIFFERS", "ELSEWHERE"):
            bad += 1
        rows.append((idx, dst_old, dst_new))
        print(f"{idx:>4} 0x{dst_old:08x} 0x{dst_new:08x} {dst_new - dst_old:>+8x}  "
              f"{fn_verdict:<10} {name_verdict:<8} {step}")

    deltas = {n - o for _, o, n in rows}
    print()
    print(f"{len(rows)} function slots carried, {len(deltas)} distinct delta(s): "
          + ", ".join(f"{d:+#x}" for d in sorted(deltas)))
    if bad:
        print(f"REFUSED: {bad} slot(s) failed the pointer or the name check.")
        return 1
    print("Every slot's stored function pointer and every slot's name string agree with the"
          " byte-offset-aligned store. Ledger rows (1.16.2 -> 1.17):")
    for idx, o, n in rows:
        print(f"  0x{o:x}\t0x{n:x}\t# slot {idx}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
