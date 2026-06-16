#!/usr/bin/env python3
"""Dump N qwords starting at a VA (read-only). Usage: recon_qwords.py <hex_va> [count]"""
import sys
from pathlib import Path
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def main():
    va = int(sys.argv[1], 16)
    count = int(sys.argv[2]) if len(sys.argv) > 2 else 8
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    for i in range(count):
        v = sre.read_qword_value(data, image_base, sections, va + i * 8)
        off = i * 8
        if v is None:
            print(f"+0x{off:02x} ({hex(va + off)}): <unmapped>")
        else:
            print(f"+0x{off:02x} ({hex(va + off)}): {hex(v)}")


if __name__ == "__main__":
    main()
