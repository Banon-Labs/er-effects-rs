#!/usr/bin/env python3
"""Dump bytes / UTF-16 / ASCII / qwords around a VA in eldenring.exe (recon).

Read-only. Used to decode the SelectBot stream source descriptor and other
.rdata constants the selection pump consumes.

Usage: python3 .auto/recon_strings.py <hex_va> [hex_size]
"""
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def main() -> None:
    va = int(sys.argv[1], 16)
    size = int(sys.argv[2], 16) if len(sys.argv) > 2 else 0x80
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    blob = sre.read_bytes(data, image_base, sections, va, size)
    print(f"# bytes at {hex(va)} ({size} bytes)")
    print(blob.hex())
    # UTF-8 Lossy: recon-only diagnostic dump of binary .rdata, not program data.
    print("utf16:", blob.decode("utf-16le", errors="replace"))
    ascii_repr = "".join(chr(b) if 32 <= b < 127 else "." for b in blob)
    print("ascii:", ascii_repr)
    print("qwords:", sre.read_qwords(data, image_base, sections, va, 8))


if __name__ == "__main__":
    main()
