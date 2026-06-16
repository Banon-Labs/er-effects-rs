#!/usr/bin/env python3
"""Report the PE section + its Characteristics flags for one or more absolute VAs.

Read-only. Answers: is a given VA in writable .data (patchable) vs read-only
.rdata. Usage: python3 .auto/recon_section.py <hex_va> [<hex_va> ...]
"""
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

# IMAGE_SCN_* characteristics bits of interest.
SCN = [
    (0x00000020, "CODE"),
    (0x00000040, "INITIALIZED_DATA"),
    (0x00000080, "UNINITIALIZED_DATA"),
    (0x20000000, "EXECUTE"),
    (0x40000000, "READ"),
    (0x80000000, "WRITE"),
]


def main(vas):
    data = sre.DEFAULT_EXE.read_bytes()
    image_base, _ = sre.parse_pe(data)
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    section_count = struct.unpack_from("<H", data, pe + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe + 20)[0]
    section_offset = pe + 24 + optional_size
    secs = []
    for i in range(section_count):
        off = section_offset + i * 40
        name = data[off:off + 8].split(b"\0", 1)[0].decode("ascii", "replace")
        vsize, vaddr, rsize, rptr = struct.unpack_from("<IIII", data, off + 8)
        chars = struct.unpack_from("<I", data, off + 36)[0]
        secs.append((name, vaddr, max(vsize, rsize), chars))
    for va in vas:
        rva = va - image_base
        hit = None
        for name, vaddr, vsize, chars in secs:
            if vaddr <= rva < vaddr + vsize:
                hit = (name, vaddr, vsize, chars)
                break
        if not hit:
            print(f"{hex(va)}: NOT IN ANY SECTION")
            continue
        name, vaddr, vsize, chars = hit
        flags = [label for bit, label in SCN if chars & bit]
        writable = "WRITABLE" if (chars & 0x80000000) else "READ-ONLY"
        print(f"{hex(va)}: section={name} rva=[{hex(vaddr)}..{hex(vaddr+vsize)}) "
              f"chars={hex(chars)} [{','.join(flags)}] -> {writable}")


if __name__ == "__main__":
    main([int(a, 16) for a in sys.argv[1:]])
