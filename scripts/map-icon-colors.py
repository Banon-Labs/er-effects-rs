#!/usr/bin/env python3
"""Measure the colour of every world-map icon subtexture, so "which icon is red" is a number.

The world-map pin icon id is a 1-based FRAME index into a MovieClip in
`menu:/02_120_WorldMap.gfx`; each frame places one `MENU_MAP_*` external image, and those
images are subtextures packed into the `SB_MapCursor*` atlases and described by
`01_common.sblytbnd`. So picking a red pin icon means picking the frame whose subtexture is
red -- which is a measurement, not a judgement, and this script makes it one.

Nothing here opens an image for a human (or a model) to look at. It decodes the atlas,
crops each declared rect, and reduces it to alpha-weighted channel means plus a redness
score, which is what a caller actually needs in order to choose an icon id.

Usage:
    uv run --with pillow python3 scripts/map-icon-colors.py
    uv run --with pillow python3 scripts/map-icon-colors.py --min-alpha 32 --top 25

The corpus root is env-overridable (`ER_GFX_CORPUS_ROOT`) exactly as the er-gfx tests' one
is, so a re-extraction to a different path needs no source edit. The script SKIPS with a
clear message when the corpus is absent rather than failing the caller: these are game
assets, and they are deliberately not in the repo.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
from pathlib import Path

# Default local extraction root. Overridable so this keeps working after a re-extract.
DEFAULT_CORPUS_ROOT = Path(
    os.environ.get("ER_GFX_CORPUS_ROOT", "/home/banon/er-extract")
)

# Where the unpacked menu textures and their atlas layouts live inside the corpus. Several
# candidates because the extraction layout has changed between dumps; the first that has
# both an atlas and a layout wins.
TEXTURE_DIR_CANDIDATES = (
    "LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/menu/low/01_common-tpf-dcx",
    "LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/menu/hi/_chunk_0001/01_common-tpf-dcx",
)
LAYOUT_DIR_CANDIDATES = (
    "LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/menu/low/01_common-sblytbnd-dcx",
    "LOOK_HERE_WITCHY_RECURSIVE_20260713/sharded/menu/hi/_chunk_0001/01_common-sblytbnd-dcx",
)

SUBTEXTURE_RE = re.compile(
    r'<SubTexture\s+name="(?P<name>[^"]+)"\s+x="(?P<x>-?\d+)"\s+y="(?P<y>-?\d+)"\s+'
    r'width="(?P<w>\d+)"\s+height="(?P<h>\d+)"'
)


def first_existing(root: Path, candidates: tuple[str, ...]) -> Path | None:
    for relative in candidates:
        path = root / relative
        if path.is_dir():
            return path
    return None


def parse_layout(path: Path) -> tuple[str, list[tuple[str, int, int, int, int]]]:
    """Return the atlas image name and every declared subtexture rect."""
    text = path.read_text(encoding="utf-8", errors="replace")
    image = re.search(r'imagePath="([^"]+)"', text)
    rects = [
        (
            m.group("name"),
            int(m.group("x")),
            int(m.group("y")),
            int(m.group("w")),
            int(m.group("h")),
        )
        for m in SUBTEXTURE_RE.finditer(text)
    ]
    return (image.group(1) if image else path.stem + ".png"), rects


def measure(image, rect, min_alpha: int):
    """Alpha-weighted mean colour of one subtexture, or None when it is effectively empty.

    Weighting by alpha matters more than it looks: these icons are a small opaque glyph on a
    fully transparent field, so an unweighted mean is dominated by whatever the transparent
    pixels happen to carry and every icon comes out looking the same colour.
    """
    _, x, y, w, h = rect
    if w <= 0 or h <= 0:
        return None
    crop = image.crop((x, y, x + w, y + h))
    pixels = crop.load()
    total_a = 0
    sums = [0.0, 0.0, 0.0]
    opaque = 0
    for py in range(crop.height):
        for px in range(crop.width):
            r, g, b, a = pixels[px, py]
            if a < min_alpha:
                continue
            opaque += 1
            total_a += a
            sums[0] += r * a
            sums[1] += g * a
            sums[2] += b * a
    if total_a == 0:
        return None
    mean = [channel / total_a for channel in sums]
    # Redness: how far red sits above the brighter of the other two channels, normalised.
    # Using max(g, b) rather than their mean keeps orange-ish art from scoring as "red"
    # purely because blue is low.
    redness = (mean[0] - max(mean[1], mean[2])) / 255.0
    return mean, redness, opaque


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus-root", type=Path, default=DEFAULT_CORPUS_ROOT)
    parser.add_argument(
        "--min-alpha",
        type=int,
        default=16,
        help="ignore pixels below this alpha; they are the transparent field, not the glyph",
    )
    parser.add_argument("--top", type=int, default=20, help="how many rows to print")
    parser.add_argument(
        "--name-filter",
        default="MENU_MAP",
        help="only measure subtextures whose name contains this",
    )
    args = parser.parse_args()

    try:
        from PIL import Image
    except ImportError:
        print(
            "Pillow is required. Run under uv:\n"
            "  uv run --with pillow python3 scripts/map-icon-colors.py",
            file=sys.stderr,
        )
        return 2

    texture_dir = first_existing(args.corpus_root, TEXTURE_DIR_CANDIDATES)
    layout_dir = first_existing(args.corpus_root, LAYOUT_DIR_CANDIDATES)
    if texture_dir is None or layout_dir is None:
        print(
            f"SKIP: no extracted menu texture/layout corpus under {args.corpus_root}. "
            "Set ER_GFX_CORPUS_ROOT to an extraction that has "
            "01_common-tpf-dcx and 01_common-sblytbnd-dcx.",
            file=sys.stderr,
        )
        return 0

    rows = []
    for layout_path in sorted(layout_dir.glob("*.layout")):
        image_name, rects = parse_layout(layout_path)
        wanted = [r for r in rects if args.name_filter in r[0]]
        if not wanted:
            continue
        atlas_path = texture_dir / (Path(image_name).stem + ".dds")
        if not atlas_path.exists():
            print(f"note: {layout_path.name} -> {atlas_path.name} missing", file=sys.stderr)
            continue
        atlas = Image.open(atlas_path).convert("RGBA")
        for rect in wanted:
            result = measure(atlas, rect, args.min_alpha)
            if result is None:
                continue
            mean, redness, opaque = result
            rows.append(
                {
                    "name": Path(rect[0]).stem,
                    "atlas": atlas_path.name,
                    "w": rect[3],
                    "h": rect[4],
                    "r": mean[0],
                    "g": mean[1],
                    "b": mean[2],
                    "redness": redness,
                    "opaque_px": opaque,
                }
            )

    if not rows:
        print("no subtextures matched", file=sys.stderr)
        return 1

    rows.sort(key=lambda row: row["redness"], reverse=True)
    print(f"{len(rows)} subtextures measured; most red first")
    print(f"{'name':28} {'atlas':22} {'size':>10} {'R':>6} {'G':>6} {'B':>6} {'redness':>8}")
    for row in rows[: args.top]:
        size = f"{row['w']}x{row['h']}"
        print(
            f"{row['name']:28} {row['atlas']:22} {size:>10} "
            f"{row['r']:6.1f} {row['g']:6.1f} {row['b']:6.1f} {row['redness']:8.3f}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
