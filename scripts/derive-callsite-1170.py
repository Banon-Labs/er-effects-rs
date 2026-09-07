#!/usr/bin/env python3
"""Derive the 1.17 answer for a call site -- a mid-function return address.

Why a separate tool
===================
`scripts/map-rvas-1162-to-1170.py` and `scripts/verify-rva-map-1170.py` both work on function
starts, because that is what `.pdata` records and what a masked signature can identify. A call
site is not a function start: it is a byte in the middle of one, and neither tool can see it.

But a call site has an identity of its own that survives the move: it is the return address of
the Nth `call` in a named function, and the offset of that call within its function is stable
whenever the function body is unchanged. So the derivation is:

    call site  =  (containing function, offset within it)

and the containing function is exactly the thing the address map already carries.

This prints the evidence for that claim, per site:

  * the `.pdata` record that contains the 1.16.2 address, so "mid-function" is not an assumption;
  * the whole-image map's pair for that function;
  * the `E8` at the claimed offset in both images, with the callee each one reaches -- if the
    call site really is the same call, both callees are the same function under the map;
  * whether the offset is identical in both, which is the load-bearing claim.

Usage
    python3 scripts/derive-callsite-1170.py 0x744e02 0x958a20 0x958b37 0x7ad530
    python3 scripts/derive-callsite-1170.py --selftest

The images are found beside the repo root, or through ER_DEOBF_1162 / ER_DEOBF_1170.
"""

from __future__ import annotations

import argparse
import bisect
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}
FUNCTION_MAP = os.environ.get(
    "ER_FUNCTION_MAP",
    os.path.join(ROOT, "docs", "recon", "rva-map-1162-to-1170.functions.tsv"),
)
# How far past a function's entry to look for the call. Nothing in this workspace names a call
# site further in than this, and an unbounded scan would run off the end of the image.
SCAN_SPAN = 0x1000

_IMAGE_CACHE: dict[str, bytes] = {}
_PDATA_CACHE: dict[str, list[tuple[int, int]]] = {}


def image(build: str) -> bytes:
    if build not in _IMAGE_CACHE:
        with open(IMAGES[build], "rb") as handle:
            _IMAGE_CACHE[build] = handle.read()
    return _IMAGE_CACHE[build]


def pdata(build: str) -> list[tuple[int, int]]:
    """Sorted (begin, end) RVA pairs from the image's own exception directory."""
    if build in _PDATA_CACHE:
        return _PDATA_CACHE[build]
    data = image(build)
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    magic = struct.unpack_from("<H", data, e_lfanew + 24)[0]
    directories = e_lfanew + 24 + (112 if magic == 0x20B else 96)
    table_rva, table_size = struct.unpack_from("<II", data, directories + 3 * 8)
    records = []
    for offset in range(table_rva, table_rva + table_size, 12):
        begin, end, _unwind = struct.unpack_from("<III", data, offset)
        if begin or end:
            records.append((begin, end))
    records.sort()
    _PDATA_CACHE[build] = records
    return records


def containing_function(build: str, rva: int) -> tuple[int, int] | None:
    """The `.pdata` record covering `rva`, or None if it falls in a gap."""
    records = pdata(build)
    begins = [begin for begin, _ in records]
    index = bisect.bisect_right(begins, rva) - 1
    if index < 0:
        return None
    begin, end = records[index]
    return (begin, end) if rva < end else None


def function_map() -> dict[int, int]:
    """1.16.2 -> 1.17 function starts, from the whole-image masked-signature pairing."""
    pairs: dict[int, int] = {}
    if not os.path.exists(FUNCTION_MAP):
        return pairs
    with open(FUNCTION_MAP, encoding="utf-8") as handle:
        for line in handle:
            if line.startswith("#") or not line.strip():
                continue
            fields = line.split("\t")
            if len(fields) < 2:
                continue
            try:
                pairs[int(fields[0], 16)] = int(fields[1], 16)
            except ValueError:
                continue
    return pairs


def call_at(build: str, rva: int) -> int | None:
    """Callee of the `E8` whose return address is `rva`, or None if that is not a call return."""
    data = image(build)
    site = rva - 5
    if site < 0 or site + 5 > len(data) or data[site] != 0xE8:
        return None
    return rva + struct.unpack_from("<i", data, site + 1)[0]


def describe(rva: int, pairs: dict[int, int]) -> bool:
    """Print the evidence for one 1.16.2 call site. True when the derivation holds."""
    print(f"=== 1.16.2 call site 0x{BASE + rva:x} (rva 0x{rva:x})")
    record = containing_function("1162", rva)
    if record is None:
        print("    NOT inside any .pdata record -- no containing function to name")
        return False
    begin, end = record
    offset = rva - begin
    print(
        f"    inside 0x{BASE + begin:x} .. 0x{BASE + end:x} "
        f"(size 0x{end - begin:x}), at +0x{offset:x}"
    )
    moved = pairs.get(begin)
    if moved is None:
        print(f"    containing function 0x{begin:x} is UNMAPPED -- nothing to translate to")
        return False
    print(f"    containing function maps 0x{begin:x} -> 0x{moved:x} (delta +0x{moved - begin:x})")

    old_callee = call_at("1162", rva)
    new_callee = call_at("1170", moved + offset)
    if old_callee is None:
        print("    1.16.2: that address is NOT the return of an E8 call")
        return False
    print(f"    1.16.2: E8 at 0x{BASE + rva - 5:x} -> callee 0x{BASE + old_callee:x}")
    if new_callee is None:
        print(
            f"    1.17:   0x{BASE + moved + offset:x} is NOT the return of an E8 call "
            "-- the body changed, the offset does not carry"
        )
        return False
    print(f"    1.17:   E8 at 0x{BASE + moved + offset - 5:x} -> callee 0x{BASE + new_callee:x}")

    expected = pairs.get(old_callee)
    if expected is None:
        verdict = "callee UNMAPPED, so the callee identity cannot be checked"
        ok = False
    elif expected == new_callee:
        verdict = f"callee agrees with the map (0x{old_callee:x} -> 0x{new_callee:x})"
        ok = True
    else:
        verdict = (
            f"callee DISAGREES: map says 0x{old_callee:x} -> 0x{expected:x}, "
            f"the 1.17 site calls 0x{new_callee:x}"
        )
        ok = False
    print(f"    {verdict}")
    print(f"    => 1.17 call site 0x{BASE + moved + offset:x}  "
          f"(fn 0x{moved:x} + 0x{offset:x})")
    return ok


def selftest() -> int:
    """Exercise the pure decoders. The image readers are covered by the repo's own images."""
    failures: list[str] = []

    # A synthetic image body: `E8 rel32` at 0x100 targeting 0x200.
    body = bytearray(0x400)
    body[0x100] = 0xE8
    struct.pack_into("<i", body, 0x101, 0x200 - 0x105)
    _IMAGE_CACHE["fixture"] = bytes(body)
    IMAGES["fixture"] = "<fixture>"
    if call_at("fixture", 0x105) != 0x200:
        failures.append("call_at must decode an E8 return address to its callee")
    if call_at("fixture", 0x104) is not None:
        failures.append("call_at must reject an address that is not an E8 return")
    if call_at("fixture", 0x2) is not None:
        failures.append("call_at must reject an address too close to the start to hold a call")

    _PDATA_CACHE["fixture"] = [(0x1000, 0x1100), (0x1100, 0x1200), (0x1300, 0x1400)]
    if containing_function("fixture", 0x1050) != (0x1000, 0x1100):
        failures.append("containing_function must find the record covering an interior address")
    if containing_function("fixture", 0x1100) != (0x1100, 0x1200):
        failures.append("an exact entry belongs to its own record, not the previous one")
    if containing_function("fixture", 0x1250) is not None:
        failures.append("an address in a .pdata gap has no containing function")
    if containing_function("fixture", 0x10) is not None:
        failures.append("an address below the first record has no containing function")

    for failure in failures:
        print(f"selftest FAILED: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("derive-callsite-1170 selftest: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sites", nargs="*", help="1.16.2 call-site RVAs or VAs, e.g. 0x744e02")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()
    if not args.sites:
        parser.error("give at least one call-site address, or --selftest")
    for name, path in IMAGES.items():
        if not os.path.exists(path):
            print(f"missing the {name} image at {path}", file=sys.stderr)
            return 2
    pairs = function_map()
    if not pairs:
        print(f"missing or empty function map at {FUNCTION_MAP}", file=sys.stderr)
        return 2
    ok = True
    for site in args.sites:
        value = int(site, 16)
        ok &= describe(value - BASE if value >= BASE else value, pairs)
        print()
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
