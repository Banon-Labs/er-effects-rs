#!/usr/bin/env python3
"""Reconnaissance-only disassembler for the local eldenring.exe.

Reuses static_re_export.py PE parsing to size a function via .pdata and
disassembles it with objdump (Intel syntax). Read-only: never launches the
game, never writes the binary. Used to decode the SimpleTitleStep selection
pump / SelectBot stream for the autoresearch autoload lane.

Usage: python3 .auto/recon_disasm.py <hex_va> [hex_fallback_size]
"""
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def disasm(va: int, fallback_size: int = 0x400) -> None:
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    rng = sre.find_pdata_range_for_pc(data, image_base, sections, va)
    begin = int(rng["begin_va"], 16) if rng["begin_va"] else va
    end = int(rng["end_va"], 16) if rng["end_va"] else va + fallback_size
    size = end - begin
    if size <= 0 or size > 0x4000:
        begin = va
        size = fallback_size
    blob = sre.read_bytes(data, image_base, sections, begin, size)
    print(f"# func {hex(begin)}..{hex(begin + size)} size={size} (requested {hex(va)})")
    with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as fh:
        fh.write(blob)
        binpath = fh.name
    out = subprocess.check_output(
        [
            "objdump", "-D", "-b", "binary", "-m", "i386:x86-64",
            "-M", "intel", f"--adjust-vma={hex(begin)}", binpath,
        ],
        text=True,
    )
    for line in out.splitlines():
        if line.startswith(" "):
            print(line)


if __name__ == "__main__":
    target = int(sys.argv[1], 16)
    fb = int(sys.argv[2], 16) if len(sys.argv) > 2 else 0x400
    disasm(target, fb)
