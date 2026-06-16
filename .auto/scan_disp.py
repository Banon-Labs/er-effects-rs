#!/usr/bin/env python3
"""Scan .text for instructions that reference a given struct displacement (disp32).

Finds writes/reads to [reg+disp]. Read-only. Catches the 4-byte little-endian
disp32 immediately following a ModRM that uses mod=10 (disp32). We brute-scan
for the disp32 byte pattern and report the surrounding bytes + nearest function
range so register-based [reg+disp] stores can be located (recon_refs only does
rip-relative).

Usage: python3 .auto/scan_disp.py <hex_disp> [hex_disp2 ...]
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def main() -> None:
    disps = [int(a, 16) for a in sys.argv[1:]]
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    texts = [s for s in sections if s["name"] == ".text"]
    for disp in disps:
        patt = disp.to_bytes(4, "little")
        print(f"# disp 0x{disp:x} pattern {patt.hex()}")
        hits = 0
        for text in texts:
            base_off = text["raw_ptr"]
            base_va = image_base + text["virtual_address"]
            size = text["raw_size"]
            blob = data[base_off:base_off + size]
            idx = 0
            while True:
                i = blob.find(patt, idx)
                if i < 0:
                    break
                idx = i + 1
                start = max(0, i - 10)
                win = blob[start:i + 4]
                va = base_va + i
                print(f"  disp_at_va=0x{va:x} window={win.hex()}")
                hits += 1
                if hits > 400:
                    print("  ... truncated")
                    break
        print(f"  total={hits}")


if __name__ == "__main__":
    main()
