#!/usr/bin/env python3
"""Analyze a loading-screen-portrait screenshot for a lean visual false-positive oracle.

The current false-positive panel is mostly dark gray / translucent UI, not the intended
portrait-on-black composition. Until a better native/pixel portrait semaphore exists,
record the fraction of truly black/dark pixels so measure.sh can fail closed on screenshots
that look like the known bad LOAD GAME/ProfileSelect panel.

Usage: analyze-loading-screen-portrait-screenshot.py <screenshot.jpg> <out.json>
"""
from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

BLACK32_MIN_RATIO = 0.55
DARK64_MAX_RATIO = 0.95
SUBPROCESS_TIMEOUT_SECONDS = 30


def ppm_from_image(path: Path) -> tuple[int, int, bytes]:
    magick = shutil.which("magick") or shutil.which("convert")
    if magick is None:
        raise RuntimeError("missing ImageMagick magick/convert")
    proc = subprocess.run(
        [magick, str(path), "-depth", "8", "ppm:-"],
        capture_output=True,
        timeout=SUBPROCESS_TIMEOUT_SECONDS,
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.decode("utf-8", "replace")[:500] or f"{magick} failed")
    data = proc.stdout
    tokens: list[bytes] = []
    idx = 0
    while len(tokens) < 4:
        end = data.find(b"\n", idx)
        if end < 0:
            raise RuntimeError("invalid PPM header")
        line = data[idx:end].strip()
        idx = end + 1
        if not line or line.startswith(b"#"):
            continue
        tokens.extend(line.split())
    if tokens[0] != b"P6":
        raise RuntimeError(f"unexpected PPM magic {tokens[0]!r}")
    width = int(tokens[1])
    height = int(tokens[2])
    maxval = int(tokens[3])
    if maxval != 255:
        raise RuntimeError(f"unexpected PPM maxval {maxval}")
    raster = data[idx : idx + width * height * 3]
    if len(raster) != width * height * 3:
        raise RuntimeError("truncated PPM raster")
    return width, height, raster


def analyze(path: Path) -> dict[str, object]:
    width, height, raster = ppm_from_image(path)
    pixels = width * height
    black32 = dark48 = dark64 = 0
    bright = 0
    for off in range(0, len(raster), 3):
        r, g, b = raster[off], raster[off + 1], raster[off + 2]
        if r < 32 and g < 32 and b < 32:
            black32 += 1
        if r < 48 and g < 48 and b < 48:
            dark48 += 1
        if r < 64 and g < 64 and b < 64:
            dark64 += 1
        if r > 96 or g > 96 or b > 96:
            bright += 1
    black32_ratio = black32 / pixels
    dark48_ratio = dark48 / pixels
    dark64_ratio = dark64 / pixels
    bright_ratio = bright / pixels
    black_ratio_sane_for_portrait_stage = black32_ratio >= BLACK32_MIN_RATIO and dark64_ratio <= DARK64_MAX_RATIO
    return {
        "screenshot": str(path),
        "width": width,
        "height": height,
        "black32_ratio": black32_ratio,
        "dark48_ratio": dark48_ratio,
        "dark64_ratio": dark64_ratio,
        "bright96_ratio": bright_ratio,
        "black32_min_ratio": BLACK32_MIN_RATIO,
        "dark64_max_ratio": DARK64_MAX_RATIO,
        "black_ratio_sane_for_portrait_stage": black_ratio_sane_for_portrait_stage,
        "known_false_positive_signature": not black_ratio_sane_for_portrait_stage,
    }


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    screenshot = Path(sys.argv[1])
    out = Path(sys.argv[2])
    result: dict[str, object]
    try:
        result = analyze(screenshot)
    except Exception as exc:
        result = {
            "screenshot": str(screenshot),
            "error": str(exc),
            "black_ratio_sane_for_portrait_stage": False,
            "known_false_positive_signature": True,
        }
    out.write_text(json.dumps(result, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
