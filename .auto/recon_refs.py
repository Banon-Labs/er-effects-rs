#!/usr/bin/env python3
"""Find rip-relative references to a target VA across .text (recon only).

Read-only. Lists every cmp/mov/lea that addresses the given VA, with the
containing function range, so writers vs readers of a global can be told
apart. Used to locate the writer of the SimpleTitleStep PlayGame gate.

Usage: python3 .auto/recon_refs.py <hex_target_va>
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
    refs = sre.scan_rip_relative_refs_to_va(data, image_base, sections, target)
    print(f"# refs to {hex(target)}: {len(refs)}")
    for r in refs:
        print(r)


if __name__ == "__main__":
    main()
