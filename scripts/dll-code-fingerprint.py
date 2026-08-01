#!/usr/bin/env python3
"""Fingerprint the CODE of a built PE, ignoring build-identity noise.

Answers one question mechanically: **did this change alter the shipped DLL, or not?**

That question decides how work is delivered. A pure rename or a constant re-derivation cannot
change codegen -- names are compile-time -- so the DLL is bit-identical and the change can ship
without a runtime proof run. Anything that moves a byte of `.text` is a different kind of
change and needs one. Guessing which side a commit falls on is exactly the mistake this repo's
rules exist to prevent, so measure it.

Whole-file `sha256` does NOT work: two builds of *identical, untouched* source differ, because
the PE carries build identity. Measured on this repo's release DLL -- of 679,936 bytes of
`.rdata`, exactly 10 differ across a no-op rebuild, all inside a 36-byte span holding the
`RSDS` CodeView PDB signature (a fresh GUID per link) and a build timestamp. Every other
section, `.text` included, is byte-identical.

So the fingerprint masks:
  * the COFF header `TimeDateStamp`,
  * the optional header `CheckSum`,
  * every IMAGE_DEBUG_DIRECTORY entry and the CodeView blob each points at.

and hashes the remaining bytes of every section. `.text` is reported separately because it is
the one that answers "did the machine code change".

Usage:
    python3 scripts/dll-code-fingerprint.py <dll>                # print fingerprint
    python3 scripts/dll-code-fingerprint.py <dll_a> <dll_b>      # compare; exit 1 if code differs
    python3 scripts/dll-code-fingerprint.py --selftest
"""

from __future__ import annotations

import hashlib
import struct
import sys
from pathlib import Path

DEBUG_DIR_INDEX = 6  # IMAGE_DIRECTORY_ENTRY_DEBUG
DEBUG_ENTRY_SIZE = 28


def _parse(blob: bytes):
    pe = struct.unpack_from("<I", blob, 0x3C)[0]
    if blob[pe:pe + 4] != b"PE\0\0":
        raise ValueError("not a PE file")
    n_sections = struct.unpack_from("<H", blob, pe + 6)[0]
    opt_size = struct.unpack_from("<H", blob, pe + 20)[0]
    opt = pe + 24
    magic = struct.unpack_from("<H", blob, opt)[0]
    # PE32+ (0x20b) puts the data directories 16 bytes further in than PE32 (0x10b).
    dd_off = opt + (112 if magic == 0x20B else 96)
    return pe, opt, opt_size, n_sections, dd_off


def _mask_ranges(blob: bytes) -> list[tuple[int, int]]:
    """Byte ranges that carry build identity rather than program content."""
    pe, opt, opt_size, _n, dd_off = _parse(blob)
    ranges = [
        (pe + 8, pe + 12),      # COFF TimeDateStamp
        (opt + 64, opt + 68),   # optional header CheckSum
    ]
    dbg_rva, dbg_size = struct.unpack_from("<II", blob, dd_off + DEBUG_DIR_INDEX * 8)
    if not dbg_rva or not dbg_size:
        return ranges

    sections = _sections(blob)
    dbg_off = _rva_to_off(sections, dbg_rva)
    if dbg_off is None:
        return ranges
    for i in range(dbg_size // DEBUG_ENTRY_SIZE):
        entry = dbg_off + i * DEBUG_ENTRY_SIZE
        if entry + DEBUG_ENTRY_SIZE > len(blob):
            break
        raw_size, _addr, ptr = struct.unpack_from("<III", blob, entry + 16)
        ranges.append((entry, entry + DEBUG_ENTRY_SIZE))
        if ptr and raw_size:
            ranges.append((ptr, min(ptr + raw_size, len(blob))))
    return ranges


def _sections(blob: bytes):
    pe, _opt, opt_size, n_sections, _dd = _parse(blob)
    table = pe + 24 + opt_size
    out = []
    for i in range(n_sections):
        off = table + 40 * i
        name = blob[off:off + 8].rstrip(b"\0").decode("ascii", "replace")
        _vsize, vaddr, rsize, rptr = struct.unpack_from("<IIII", blob, off + 8)
        out.append((name, vaddr, rptr, rsize))
    return out


def _rva_to_off(sections, rva: int):
    for _name, vaddr, rptr, rsize in sections:
        if vaddr <= rva < vaddr + rsize:
            return rptr + (rva - vaddr)
    return None


def fingerprint(path: Path) -> dict[str, str]:
    blob = bytearray(Path(path).read_bytes())
    for start, end in _mask_ranges(bytes(blob)):
        blob[start:end] = b"\0" * (end - start)
    frozen = bytes(blob)
    out: dict[str, str] = {}
    everything = hashlib.sha256()
    for name, _vaddr, rptr, rsize in _sections(frozen):
        chunk = frozen[rptr:rptr + rsize]
        out[name] = hashlib.sha256(chunk).hexdigest()[:16]
        everything.update(chunk)
    out["ALL"] = everything.hexdigest()[:16]
    return out


def selftest() -> int:
    """The masking must neutralise build identity and nothing else."""
    failures = []
    sample = Path("target/x86_64-pc-windows-msvc/release/er_effects_rs.dll")
    if not sample.exists():
        print("[dll-code-fingerprint] selftest SKIPPED (no built DLL to sample)")
        return 0
    blob = bytearray(sample.read_bytes())
    base = fingerprint(sample)

    # Flipping a masked byte (the COFF timestamp) must NOT move the fingerprint.
    pe = struct.unpack_from("<I", bytes(blob), 0x3C)[0]
    poked = bytearray(blob)
    poked[pe + 8] ^= 0xFF
    tmp = sample.with_suffix(".selftest.tmp")
    tmp.write_bytes(bytes(poked))
    try:
        if fingerprint(tmp)["ALL"] != base["ALL"]:
            failures.append("masked timestamp byte changed the fingerprint")
        # Flipping a .text byte MUST move it.
        poked2 = bytearray(blob)
        text = next(s for s in _sections(bytes(blob)) if s[0] == ".text")
        poked2[text[2]] ^= 0xFF
        tmp.write_bytes(bytes(poked2))
        if fingerprint(tmp)["ALL"] == base["ALL"]:
            failures.append(".text byte flip did NOT change the fingerprint")
        if fingerprint(tmp)[".text"] == base[".text"]:
            failures.append(".text byte flip did NOT change the .text hash")
    finally:
        tmp.unlink(missing_ok=True)

    for failure in failures:
        print(f"selftest FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("[dll-code-fingerprint] selftest ok (3 cases)")
    return 0


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if "--selftest" in sys.argv:
        return selftest()
    if not args:
        print(__doc__)
        return 2
    if len(args) == 1:
        for key, value in fingerprint(Path(args[0])).items():
            print(f"  {key:10s} {value}")
        return 0

    a, b = fingerprint(Path(args[0])), fingerprint(Path(args[1]))
    material = False
    for key in sorted(set(a) | set(b)):
        same = a.get(key) == b.get(key)
        if not same and key != "ALL":
            material = True
        print(f"  {key:10s} {'SAME' if same else 'DIFF'}  {a.get(key, '-')}  {b.get(key, '-')}")
    print()
    if material:
        print("MATERIAL: the built code differs. This needs a runtime proof run.")
        return 1
    print("NOT MATERIAL: the built code is byte-identical once build identity is masked.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
