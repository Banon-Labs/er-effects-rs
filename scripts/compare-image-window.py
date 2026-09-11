#!/usr/bin/env python3
"""Print the same byte window from several deobf images, so a pin can be read rather than argued.

`find-deobf-bytes.py` answers "where does this pattern occur"; this answers the other half of the
same question -- "are these images the same bytes at this address" -- which is what a ledger row
carrying one address across two 1.17 builds actually claims. Both images are flat, so
`VA = 0x140000000 + file_offset` with no section mapping.

Usage: `python3 scripts/compare-image-window.py 0x1407c6ac0 [--bytes 24] [image ...]`

With no image arguments it reads the three this repo keeps, resolving each the way
`map-rvas-1162-to-1170.py` does: this checkout first, then the main worktree, because the images
are gitignored and never copied into a linked worktree.
"""

import argparse
import os
import subprocess
import sys

BASE = 0x140000000
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_IMAGES = (
    "eldenring-deobf.bin",
    "eldenring-deobf-1.17.bin",
    "eldenring-deobf-1.17.1.bin",
)


def resolve(filename: str) -> str:
    """This checkout, then the main worktree. Same fallback the mapper takes, same reason."""
    local = os.path.join(ROOT, filename)
    if os.path.exists(local):
        return local
    try:
        common = subprocess.run(
            ["git", "-C", ROOT, "rev-parse", "--git-common-dir"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
        if common.returncode == 0:
            main_root = os.path.dirname(os.path.abspath(os.path.join(ROOT, common.stdout.strip())))
            candidate = os.path.join(main_root, filename)
            if os.path.exists(candidate):
                return candidate
    except Exception:
        pass
    return local


def window(path: str, va: int, count: int) -> bytes | None:
    offset = va - BASE
    if offset < 0:
        return None
    try:
        with open(path, "rb") as handle:
            handle.seek(offset)
            return handle.read(count)
    except OSError:
        return None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("va", help="virtual address (hex)")
    parser.add_argument("--bytes", type=int, default=24, dest="count")
    parser.add_argument("images", nargs="*", default=[])
    args = parser.parse_args()

    va = int(args.va, 0)
    images = args.images or list(DEFAULT_IMAGES)
    seen: dict[bytes, list[str]] = {}
    for name in images:
        path = resolve(name)
        data = window(path, va, args.count)
        if data is None or len(data) < args.count:
            print(f"{name:<28} unreadable at {va:#x} ({path})")
            continue
        print(f"{name:<28} {data.hex(' ')}")
        seen.setdefault(data, []).append(name)
    if len(seen) == 1 and len(next(iter(seen.values()))) == len(images):
        print(f"all {len(images)} images identical over {args.count} bytes at {va:#x}")
    else:
        print(f"{len(seen)} distinct windows at {va:#x}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
