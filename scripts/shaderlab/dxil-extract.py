#!/usr/bin/env python3
"""Carve the LLVM bitcode out of a DXContainer ('DXBC'-magic) shader member.

Parses the container header + part table, finds the DXIL part, walks the
DxilProgramHeader/DxilBitcodeHeader, and writes the inner LLVM bitcode (starts
with 'BC\\xC0\\xDE') to <out>. Use for ER .vpo/.ppo/.fpo/.cpo members.

Usage: dxil-extract.py <member.vpo> <out.bc>
"""
import struct, sys


def carve(data: bytes) -> bytes:
    if data[:4] != b"DXBC":
        raise SystemExit(f"not a DXContainer (magic={data[:4]!r})")
    # header: magic[4] hash[16] major(u16) minor(u16) fileSize(u32) partCount(u32)
    part_count = struct.unpack_from("<I", data, 28)[0]
    offsets = struct.unpack_from(f"<{part_count}I", data, 32)
    parts = {}
    for off in offsets:
        name = data[off:off + 4].decode("latin1")
        size = struct.unpack_from("<I", data, off + 4)[0]
        parts[name] = (off + 8, size)
    print(f"parts: {', '.join(parts)}", file=sys.stderr)
    if "DXIL" not in parts:
        raise SystemExit("no DXIL part (SM5 DXBC? look for SHEX/SHDR)")
    poff, psize = parts["DXIL"]
    # DxilProgramHeader: ProgramVersion(u32) SizeInUint32(u32) then DxilBitcodeHeader:
    #   DxilMagic[4]='DXIL' DxilVersion(u32) BitcodeOffset(u32) BitcodeSize(u32)
    prog_ver = struct.unpack_from("<I", data, poff)[0]
    bc_hdr = poff + 8
    magic = data[bc_hdr:bc_hdr + 4]
    dxil_ver = struct.unpack_from("<I", data, bc_hdr + 4)[0]
    bc_off = struct.unpack_from("<I", data, bc_hdr + 8)[0]
    bc_size = struct.unpack_from("<I", data, bc_hdr + 12)[0]
    print(f"DXIL part: progVersion=0x{prog_ver:08x} innerMagic={magic!r} "
          f"dxilVersion=0x{dxil_ver:08x} bcOffset={bc_off} bcSize={bc_size}", file=sys.stderr)
    bc_start = bc_hdr + bc_off
    bc = data[bc_start:bc_start + bc_size]
    if bc[:2] != b"BC":
        raise SystemExit(f"bitcode does not start with 'BC' (got {bc[:4]!r})")
    return bc


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    out = carve(open(sys.argv[1], "rb").read())
    open(sys.argv[2], "wb").write(out)
    print(f"wrote {len(out)} bytes of LLVM bitcode -> {sys.argv[2]}", file=sys.stderr)
