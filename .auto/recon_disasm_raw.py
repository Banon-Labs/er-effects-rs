#!/usr/bin/env python3
"""Raw disassembler: explicit begin VA + size, bypassing .pdata lookup."""
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import static_re_export as sre  # noqa: E402

EXE = sre.DEFAULT_EXE


def disasm(va: int, size: int) -> None:
    data = EXE.read_bytes()
    image_base, sections = sre.parse_pe(data)
    blob = sre.read_bytes(data, image_base, sections, va, size)
    with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as fh:
        fh.write(blob)
        binpath = fh.name
    out = subprocess.check_output(
        [
            "objdump", "-D", "-b", "binary", "-m", "i386:x86-64",
            "-M", "intel", f"--adjust-vma={hex(va)}", binpath,
        ],
        text=True,
    )
    for line in out.splitlines():
        if line.startswith(" "):
            print(line)


if __name__ == "__main__":
    target = int(sys.argv[1], 16)
    size = int(sys.argv[2], 16)
    disasm(target, size)
