#!/usr/bin/env python3
"""Scan the whole image for an 8-byte little-endian pointer to a VA (recon only).

Finds vtable / function-pointer-table / data references to a function that has
no rip-relative code refs (runtime-dispatched). Read-only.

Usage: python3 .auto/recon_ptr_scan.py <hex_target_va>
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def main() -> None:
    target = int(sys.argv[1], 16)
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    needle = target.to_bytes(8, "little")
    hits = []
    # search each section's raw data, map file offset -> va
    for s in sections:
        raw_off = s["raw_ptr"]
        raw_size = s["raw_size"]
        va_base = image_base + s["virtual_address"]
        blob = data[raw_off:raw_off + raw_size]
        start = 0
        while True:
            i = blob.find(needle, start)
            if i < 0:
                break
            if i % 8 == 0 or True:
                hits.append((va_base + i, s["name"]))
            start = i + 1
    print(f"# 8-byte LE pointer hits to {hex(target)}: {len(hits)}")
    for va, name in hits:
        print(f"  {hex(va)}  section={name}")


if __name__ == "__main__":
    main()
