#!/usr/bin/env python3
"""Resolve PE import thunk names for given IAT VAs in eldenring.exe.

Read-only. Walks the import directory and prints DLL!Name for each IAT slot.
Usage: python3 .auto/recon_imports.py [hex_va ...]
If no VAs given, dumps all user32/kernel32 imports.
"""
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def rva_to_off(sections, rva):
    for s in sections:
        va = int(s["virtual_address"], 16) if isinstance(s["virtual_address"], str) else s["virtual_address"]
        # static_re_export sections store keys differently; normalize below
    return None


def main(argv):
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)

    # Build a simple list of (rva, vsize, raw_off, raw_size) from sections
    secs = []
    for s in sections:
        secs.append(s)

    def va_off(va):
        return sre.va_to_file_offset(image_base, secs, va)

    # Locate data directory: import table = index 1
    pe_off = struct.unpack_from("<I", data, 0x3C)[0]
    opt_off = pe_off + 0x18
    magic = struct.unpack_from("<H", data, opt_off)[0]
    # PE32+ optional header; data dir starts at opt_off+0x70
    dd_off = opt_off + 0x70
    import_rva = struct.unpack_from("<I", data, dd_off + 1 * 8)[0]
    import_size = struct.unpack_from("<I", data, dd_off + 1 * 8 + 4)[0]
    import_va = image_base + import_rva
    imp_off = va_off(import_va)

    # Map IAT slot VA -> name
    slot_to_name = {}
    desc = imp_off
    while True:
        oft, tds, fwd, name_rva, first_thunk = struct.unpack_from("<IIIII", data, desc)
        if oft == 0 and name_rva == 0 and first_thunk == 0:
            break
        dll_off = va_off(image_base + name_rva)
        dll_end = data.index(b"\x00", dll_off)
        dll = data[dll_off:dll_end].decode("ascii", "replace")
        thunk_array_rva = oft if oft else first_thunk
        ta_off = va_off(image_base + thunk_array_rva)
        iat_off = va_off(image_base + first_thunk)
        i = 0
        while True:
            ent = struct.unpack_from("<Q", data, ta_off + i * 8)[0]
            if ent == 0:
                break
            iat_slot_va = image_base + first_thunk + i * 8
            if ent & (1 << 63):
                fname = f"#{ent & 0xFFFF}"
            else:
                hint_off = va_off(image_base + (ent & 0x7FFFFFFF))
                nstart = hint_off + 2
                nend = data.index(b"\x00", nstart)
                fname = data[nstart:nend].decode("ascii", "replace")
            slot_to_name[iat_slot_va] = f"{dll}!{fname}"
            i += 1
        desc += 20

    if len(argv) > 1:
        for a in argv[1:]:
            va = int(a, 16)
            print(f"{hex(va)} -> {slot_to_name.get(va, '??? (not an import IAT slot)')}")
    else:
        for va, n in sorted(slot_to_name.items()):
            if "user32" in n.lower() or "kernel32" in n.lower():
                print(f"{hex(va)} -> {n}")


if __name__ == "__main__":
    main(sys.argv)
