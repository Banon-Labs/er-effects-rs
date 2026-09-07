#!/usr/bin/env python3
"""Scan an IDA-style AOB signature across the .text of both deobfuscated ER images.

Why: several DLLs locate a function by scanning the live .text rather than hard-coding an RVA.
An address produced that way is already an address for the running build, and putting it through
the 1.16.2 -> 1.17 translator moves a correct address to a wrong one. Running the same signature
over both images is what tells the two cases apart: if the signature lands on X in 1.16.2 and on
Y in 1.17, then a DLL that logged Y was already right.

Usage
    python3 scripts/aob-scan-both-images.py "40 53 48 83 EC 40 48 8B 41 18"
"""
import argparse
import os
import re
import struct

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
IMAGES = {
    "1162": os.environ.get("ER_DEOBF_1162", os.path.join(ROOT, "eldenring-deobf.bin")),
    "1170": os.environ.get("ER_DEOBF_1170", os.path.join(ROOT, "eldenring-deobf-1.17.bin")),
}


def text_range(image):
    """(rva, size) of the .text section, from the section table."""
    e_lfanew = struct.unpack_from("<I", image, 0x3C)[0]
    sections = struct.unpack_from("<H", image, e_lfanew + 6)[0]
    opt_size = struct.unpack_from("<H", image, e_lfanew + 20)[0]
    table = e_lfanew + 24 + opt_size
    for i in range(sections):
        entry = table + i * 40
        name = image[entry : entry + 8].rstrip(b"\0").decode("ascii", "replace")
        virtual_size, virtual_address = struct.unpack_from("<II", image, entry + 8)
        if name == ".text":
            return virtual_address, virtual_size
    raise SystemExit("no .text section")


def parse_sig(sig):
    tokens = sig.split()
    pattern = b""
    for token in tokens:
        if token in ("??", "?"):
            pattern += b"."
        else:
            pattern += re.escape(bytes([int(token, 16)]))
    return re.compile(pattern, re.DOTALL), len(tokens)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("signature")
    parser.add_argument("--builds", default="1162,1170")
    args = parser.parse_args()
    rx, length = parse_sig(args.signature)
    print(f"signature: {length} bytes")
    for build in args.builds.split(","):
        image = open(IMAGES[build], "rb").read()
        rva, size = text_range(image)
        hits = [BASE + rva + m.start() for m in rx.finditer(image[rva : rva + size])]
        pretty = ", ".join(hex(h) for h in hits[:8]) or "(none)"
        print(f"[{build}] .text {BASE + rva:#x}..{BASE + rva + size:#x}  hits={len(hits)}  {pretty}")


if __name__ == "__main__":
    main()
