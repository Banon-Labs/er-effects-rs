#!/usr/bin/env python3
"""Harvest MSVC RTTI vtable -> class name from both de-Arxan'd ER images and join them by class.

Why this exists
---------------
A struct field offset can only be cleared per object. Every other join -- "some function reads
that number", "a hooked function brackets it" -- joins on a number, and `0x50`/`0x88`/`0x90` are
field offsets in dozens of unrelated structures, so such a join manufactures confidence out of a
coincidence. RTTI is the one identity that is established independently in each image: the class
name is FromSoft's own embedded type descriptor, so finding `.?AVMoveMapStep@CS@@` in 1.16.2 and
again in 1.17 pairs two vtables without ever consulting a content-matched function map.

That matters twice over:
  * the pairing survives a function whose body changed, and
  * vtable slot N in both images is the same virtual method of the same class, so it pairs leaf
    functions -- which `.pdata` omits entirely and the content map therefore cannot contain.

Output (default `docs/../scratchpad`, see --out-dir):
  rtti-1162.tsv        0x<vtable_va>\t<mangled>
  rtti-1170.tsv        same, for 1.17
  rtti-joined.tsv      <class>\t0x<vt_1162>\t0x<vt_1170>   -- only classes with exactly one
                       vtable in each image, so the pairing is unambiguous by construction.

A class with several vtables (multiple inheritance emits one per base) is written to
`rtti-ambiguous.tsv` instead of being guessed at.
"""

from __future__ import annotations

import argparse
import os
import sys
from collections import defaultdict
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parent.parent
BASE = 0x140000000
IMAGES = {
    "1162": ROOT / "eldenring-deobf.bin",
    "1170": ROOT / "eldenring-deobf-1.17.bin",
}
DEFAULT_OUT = Path(
    os.environ.get(
        "ER_STRUCT_DRIFT_OUT",
        "/tmp/claude-1000/-home-banon-projects-er-mods-rs/"
        "f1b1f237-c4a5-4649-9833-a40666da21bb/scratchpad/struct-drift",
    )
)


def scan_cols(data: bytes) -> dict[int, tuple[str, int]]:
    """`{col_va: (mangled_name, chd_rva)}` -- pass 1, shared by the vtable and hierarchy scans."""
    n = len(data)
    words = np.frombuffer(data[: (n // 4) * 4], dtype="<u4")
    idx = np.arange(words.size, dtype=np.int64) * 4
    lim = words.size - 6
    cand = np.nonzero(words[5 : lim + 5] == idx[:lim])[0]
    out: dict[int, tuple[str, int]] = {}
    for i in cand:
        off = int(i) * 4
        if int(words[i]) != 1:
            continue
        td_rva = int(words[i + 3])
        if not (0 < td_rva < n - 0x10):
            continue
        name_off = td_rva + 0x10
        end = data.find(b"\x00", name_off, name_off + 512)
        if end < 0:
            continue
        name = data[name_off:end].decode("latin1", "replace")
        if name.startswith(".?A"):
            out[BASE + off] = (name, int(words[i + 4]))
    return out


def base_classes(data: bytes, cols: dict[int, tuple[str, int]]) -> dict[str, set[str]]:
    """`{class: set(its base classes)}` from the RTTI ClassHierarchyDescriptors.

    Why this is needed for a clearance, not just for tidiness. In a virtual method of class `C`,
    `this` points at a `C` *or at anything derived from it*. So for a leaf class the object is
    unambiguous and a field read off `this` is that class's field -- but for a shared base like
    `DLUT::DLReferenceCountObject` or `FD4::FD4Time` the same evidence describes whichever
    derived object the caller happened to pass, which is the offset-coincidence trap wearing an
    object's name. A class that other classes derive from therefore cannot lend a clearance to a
    field above its own size, and this table is what says which classes those are.

    CHD: +0x08 numBaseClasses, +0x0c pBaseClassArray (RVA of an array of RVAs to
    BaseClassDescriptors, whose +0x00 is the TypeDescriptor RVA). Entry 0 is the class itself.
    """
    n = len(data)
    out: dict[str, set[str]] = {}
    for _col, (name, chd_rva) in cols.items():
        if not (0 < chd_rva < n - 0x10):
            continue
        count = int.from_bytes(data[chd_rva + 8 : chd_rva + 0xC], "little")
        arr = int.from_bytes(data[chd_rva + 0xC : chd_rva + 0x10], "little")
        if count == 0 or count > 64 or not (0 < arr < n - 4 * count):
            continue
        bases: set[str] = set()
        for k in range(count):
            bcd = int.from_bytes(data[arr + 4 * k : arr + 4 * k + 4], "little")
            if not (0 < bcd < n - 4):
                continue
            td = int.from_bytes(data[bcd : bcd + 4], "little")
            if not (0 < td < n - 0x10):
                continue
            end = data.find(b"\x00", td + 0x10, td + 0x210)
            if end < 0:
                continue
            bn = data[td + 0x10 : end].decode("latin1", "replace")
            if bn.startswith(".?A"):
                bases.add(demangle(bn))
        me = demangle(name)
        bases.discard(me)
        out.setdefault(me, set()).update(bases)
    return out


def scan_image(data: bytes) -> dict[int, str]:
    """`{vtable_va: mangled_class_name}` for one flat image (file offset == RVA)."""
    n = len(data)
    # Pass 1 -- CompleteObjectLocators. The x64 identifier is `u32[COL+0x14] == COL_rva`
    # (`pSelf`), which no other structure satisfies by accident at 4-byte alignment.
    col_class = {va: nm for va, (nm, _chd) in scan_cols(data).items()}

    # Pass 2 -- any qword equal to a COL VA is a vtable's [-8] slot.
    quads = np.frombuffer(data[: (n // 8) * 8], dtype="<u8")
    col_vas = np.fromiter(col_class.keys(), dtype="<u8", count=len(col_class))
    col_vas.sort()
    hit = np.nonzero(np.isin(quads, col_vas))[0]
    out: dict[int, str] = {}
    for i in hit:
        out[BASE + int(i) * 8 + 8] = col_class[int(quads[i])]
    return out


def demangle(mangled: str) -> str:
    """`.?AVMoveMapStep@CS@@` -> `CS::MoveMapStep`. Structural only; no MSVC decorations."""
    body = mangled[4:] if mangled[:4] in (".?AV", ".?AU") else mangled
    body = body.rstrip("@")
    parts = [p for p in body.split("@") if p]
    return "::".join(reversed(parts))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest()

    args.out_dir.mkdir(parents=True, exist_ok=True)
    maps: dict[str, dict[int, str]] = {}
    for tag, path in IMAGES.items():
        if not path.is_file():
            raise SystemExit(f"missing image {path}")
        maps[tag] = scan_image(path.read_bytes())
        dest = args.out_dir / f"rtti-{tag}.tsv"
        with dest.open("w", encoding="utf-8") as fh:
            fh.write(f"# {len(maps[tag])} vtables in {path.name}\n")
            for va in sorted(maps[tag]):
                fh.write(f"{va:#x}\t{maps[tag][va]}\n")
        print(f"{tag}: {len(maps[tag])} vtables -> {dest}")

    # Which classes other classes derive from -- see `base_classes` for why a clearance depends
    # on it. Taken from the 1.16.2 image; the hierarchy is a source-level fact, not an address.
    cols = scan_cols(IMAGES["1162"].read_bytes())
    hierarchy = base_classes(IMAGES["1162"].read_bytes(), cols)
    derived_count: dict[str, int] = defaultdict(int)
    for child, bases in hierarchy.items():
        for b in bases:
            derived_count[b] += 1
    bp = args.out_dir / "rtti-bases.tsv"
    bp.write_text(
        "# class\tn_classes_that_derive_from_it   (0 = leaf; a leaf's `this` is unambiguous)\n"
        + "".join(f"{k}\t{v}\n" for k, v in sorted(derived_count.items(), key=lambda kv: -kv[1])),
        encoding="utf-8",
    )
    print(f"classes used as a base by someone: {len(derived_count)} -> {bp}")

    by_class: dict[str, dict[str, list[int]]] = defaultdict(lambda: {"1162": [], "1170": []})
    for tag, table in maps.items():
        for va, name in table.items():
            by_class[demangle(name)][tag].append(va)

    joined, ambiguous = [], []
    for name, sides in sorted(by_class.items()):
        a, b = sorted(sides["1162"]), sorted(sides["1170"])
        if len(a) == 1 and len(b) == 1:
            joined.append((name, a[0], b[0]))
        elif a and b:
            ambiguous.append((name, len(a), len(b)))
    jp = args.out_dir / "rtti-joined.tsv"
    jp.write_text(
        "# class\tvtable_1162\tvtable_1170  (exactly one vtable per image; unambiguous)\n"
        + "".join(f"{n}\t{a:#x}\t{b:#x}\n" for n, a, b in joined),
        encoding="utf-8",
    )
    ap_ = args.out_dir / "rtti-ambiguous.tsv"
    ap_.write_text(
        "# class\tn_vtables_1162\tn_vtables_1170  (multiple inheritance; NOT paired)\n"
        + "".join(f"{n}\t{a}\t{b}\n" for n, a, b in ambiguous),
        encoding="utf-8",
    )
    print(f"joined unambiguously: {len(joined)} classes -> {jp}")
    print(f"ambiguous (>1 vtable a side): {len(ambiguous)} -> {ap_}")
    return 0


def selftest() -> int:
    """Positive controls: a hand-built COL+vtable must be found; breaking pSelf must lose it."""
    ok = True

    def build(name: bytes, *, good_pself: bool = True) -> bytes:
        # layout: [0x0000 pad][0x1000 TypeDescriptor][0x2000 COL][0x3000 vtable-8]
        buf = bytearray(0x4000)
        td = 0x1000
        buf[td + 0x10 : td + 0x10 + len(name)] = name
        col = 0x2000
        buf[col + 0x00 : col + 0x04] = (1).to_bytes(4, "little")          # signature
        buf[col + 0x0C : col + 0x10] = td.to_bytes(4, "little")           # pTypeDescriptor
        buf[col + 0x14 : col + 0x18] = (col if good_pself else col + 4).to_bytes(4, "little")
        vt = 0x3000
        buf[vt : vt + 8] = (BASE + col).to_bytes(8, "little")             # vtable[-1] = COL VA
        return bytes(buf)

    found = scan_image(build(b".?AVMoveMapStep@CS@@"))
    want = BASE + 0x3008
    if found.get(want) != ".?AVMoveMapStep@CS@@":
        print(f"FAIL: control vtable not found ({found})")
        ok = False
    else:
        print("ok: control COL+vtable found at the constructed address")

    # MUTATION: corrupt pSelf, which is the whole identifier. It must vanish.
    broken = scan_image(build(b".?AVMoveMapStep@CS@@", good_pself=False))
    if broken:
        print(f"FAIL: mutation (bad pSelf) still matched {broken}")
        ok = False
    else:
        print("ok: mutation (pSelf != COL) is rejected, so the rule is load-bearing")

    # MUTATION: a name that is not an RTTI descriptor must be rejected.
    if scan_image(build(b"not_a_type_descriptor")):
        print("FAIL: mutation (non `.?A` name) still matched")
        ok = False
    else:
        print("ok: mutation (name lacking `.?A`) is rejected")

    # base_classes must find a base, and must not report a class as its own base.
    def build_hier() -> bytes:
        buf = bytearray(0x6000)
        td_c, td_b = 0x1000, 0x1100
        buf[td_c + 0x10 : td_c + 0x10 + 20] = b".?AVChild@CS@@\x00"
        buf[td_b + 0x10 : td_b + 0x10 + 20] = b".?AVBase@CS@@\x00"
        bcd_c, bcd_b = 0x1200, 0x1240
        buf[bcd_c : bcd_c + 4] = td_c.to_bytes(4, "little")
        buf[bcd_b : bcd_b + 4] = td_b.to_bytes(4, "little")
        arr = 0x1300
        buf[arr : arr + 4] = bcd_c.to_bytes(4, "little")
        buf[arr + 4 : arr + 8] = bcd_b.to_bytes(4, "little")
        chd = 0x1400
        buf[chd + 8 : chd + 0xC] = (2).to_bytes(4, "little")
        buf[chd + 0xC : chd + 0x10] = arr.to_bytes(4, "little")
        col = 0x2000
        buf[col : col + 4] = (1).to_bytes(4, "little")
        buf[col + 0x0C : col + 0x10] = td_c.to_bytes(4, "little")
        buf[col + 0x10 : col + 0x14] = chd.to_bytes(4, "little")
        buf[col + 0x14 : col + 0x18] = col.to_bytes(4, "little")
        return bytes(buf)

    img = build_hier()
    hier = base_classes(img, scan_cols(img))
    if hier.get("CS::Child") != {"CS::Base"}:
        print(f"FAIL: hierarchy control -- CS::Child bases = {hier.get('CS::Child')}")
        ok = False
    else:
        print("ok: RTTI hierarchy names CS::Base as a base of CS::Child, and not itself")

    cases = {
        ".?AVMoveMapStep@CS@@": "CS::MoveMapStep",
        ".?AVTitleFlowCoordinator@CS@@": "CS::TitleFlowCoordinator",
        ".?AVRendMan@CS@@": "CS::RendMan",
        ".?AVFD4FileCap@@": "FD4FileCap",
    }
    for mangled, expect in cases.items():
        got = demangle(mangled)
        if got != expect:
            print(f"FAIL: demangle({mangled}) = {got}, want {expect}")
            ok = False
    if ok:
        print(f"ok: demangle on {len(cases)} controls")
    print("SELFTEST", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
