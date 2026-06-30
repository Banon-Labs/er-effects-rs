#!/usr/bin/env python3
"""Objective look-at / pose-tracking verifier -- the mechanical guard against claiming a rendered
behavioral feature works from indirect signals (bone-hook counters, bucket labels, eyeballing).

A bone write is not a rendered pixel: the per-frame profile draw_step is a ClearRTV, not a rasterize,
so a driven look-at only "tracks" if the model is actually re-rasterized per pose. This compares the
captured offscreen RT at the three cursor positions and FAILS unless the pixels show a real, monotonic,
cursor-correlated head turn:

  PASS requires ALL of:
    1. opposite extremes differ MOST: diff(L,R) > diff(L,C)  AND  diff(L,R) > diff(C,R)
    2. that extreme diff clears a noise floor: diff(L,R) >= --min-extreme-diff (default 8.0 /px)
    3. the foreground (head) centroid X shifts monotonically L->C->R (either direction)

If the opposite extremes are ~identical (the 2026-06-30 failure: cursor LEFT vs RIGHT were 95% identical
pixels yet "tracking" was claimed), this reports FAIL -- the feature is broken, not proven.

Reads the ERPX dumps the DLL writes (see crates/erpx-rs for the canonical format: b"ERPX" + u32 LE width
+ u32 LE height + R8G8B8A8). Writes <artifact_dir>/lookat-tracking-verdict.json and exits non-zero on FAIL.

Usage: verify-lookat-tracking.py [ARTIFACT_DIR] [--min-extreme-diff F]
       (ARTIFACT_DIR defaults to target/runtime-probe/postcontinue-lookat-smoke)
"""
from __future__ import annotations

import argparse
import json
import os
import struct
import sys
from pathlib import Path

# slot numbers the cursor-sweep proof dumps each held cursor position to (see startup_hooks.rs).
SLOTS = {"L": 200, "C": 201, "R": 202}


def load_erpx(path: Path) -> tuple[int, int, bytes]:
    b = path.read_bytes()
    if b[:4] != b"ERPX":
        raise ValueError(f"{path}: bad ERPX magic {b[:4]!r}")
    w, h = struct.unpack_from("<II", b, 4)
    px = b[12 : 12 + w * h * 4]
    if len(px) < w * h * 4:
        raise ValueError(f"{path}: truncated ({len(px)} < {w * h * 4})")
    return w, h, px


def mean_abs_rgb_diff(a: bytes, b: bytes, stride_px: int = 7) -> float:
    """Mean absolute RGB difference per sampled pixel (sparse stride for speed)."""
    n = min(len(a), len(b))
    step = 4 * stride_px
    total = 0
    cnt = 0
    for i in range(0, n - 3, step):
        total += abs(a[i] - b[i]) + abs(a[i + 1] - b[i + 1]) + abs(a[i + 2] - b[i + 2])
        cnt += 1
    return total / max(cnt, 1)


def foreground_centroid_x(w: int, h: int, px: bytes, bg_lum: int = 24) -> float:
    """Luminance-weighted X centroid of non-background pixels, normalized to [0,1] across width.
    The portrait background is near-black, so pixels above `bg_lum` are the head/body."""
    sx = 0.0
    sw = 0.0
    step = 4 * 5  # sparse
    i = 0
    idx = 0
    npx = w * h
    while idx < npx:
        r = px[i]
        g = px[i + 1]
        b = px[i + 2]
        lum = (r * 54 + g * 183 + b * 19) >> 8
        if lum > bg_lum:
            x = idx % w
            sx += x * lum
            sw += lum
        i += step
        idx += 5
    if sw == 0:
        return float("nan")
    return (sx / sw) / max(w - 1, 1)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "artifact_dir",
        nargs="?",
        default="target/runtime-probe/postcontinue-lookat-smoke",
    )
    ap.add_argument("--min-extreme-diff", type=float, default=8.0)
    args = ap.parse_args()

    d = Path(args.artifact_dir)
    imgs = {}
    mtimes = {}
    for k, slot in SLOTS.items():
        p = d / f"portrait-capture-slot{slot}.bin"
        if not p.exists():
            print(f"FAIL: missing {p} (cursor {k} not captured this run)", file=sys.stderr)
            verdict = {"pass": False, "reason": f"missing slot{slot} ({k})"}
            (d / "lookat-tracking-verdict.json").write_text(json.dumps(verdict, indent=2))
            return 1
        imgs[k] = load_erpx(p)
        mtimes[k] = p.stat().st_mtime

    same_run = (max(mtimes.values()) - min(mtimes.values())) < 30.0
    L, C, R = imgs["L"][2], imgs["C"][2], imgs["R"][2]
    d_lr = mean_abs_rgb_diff(L, R)
    d_lc = mean_abs_rgb_diff(L, C)
    d_cr = mean_abs_rgb_diff(C, R)
    cx = {k: foreground_centroid_x(*imgs[k]) for k in SLOTS}
    monotonic = (cx["L"] < cx["C"] < cx["R"]) or (cx["L"] > cx["C"] > cx["R"])

    extremes_differ_most = d_lr > d_lc and d_lr > d_cr
    clears_floor = d_lr >= args.min_extreme_diff
    ok = extremes_differ_most and clears_floor and monotonic

    verdict = {
        "pass": bool(ok),
        "same_run": bool(same_run),
        "diff_left_right": round(d_lr, 2),
        "diff_left_center": round(d_lc, 2),
        "diff_center_right": round(d_cr, 2),
        "extremes_differ_most": bool(extremes_differ_most),
        "clears_noise_floor": bool(clears_floor),
        "min_extreme_diff": args.min_extreme_diff,
        "centroid_x": {k: (None if cx[k] != cx[k] else round(cx[k], 4)) for k in SLOTS},
        "centroid_monotonic": bool(monotonic),
    }
    (d / "lookat-tracking-verdict.json").write_text(json.dumps(verdict, indent=2))
    print(json.dumps(verdict, indent=2))
    if not same_run:
        print(
            "WARN: dumps are not from the same run (mtimes >30s apart) -- not apples-to-apples; "
            "re-run so L/C/R come from one process.",
            file=sys.stderr,
        )
    if not ok:
        msg = []
        if not extremes_differ_most:
            msg.append(
                f"opposite extremes do NOT differ most (LR={d_lr:.1f} vs LC={d_lc:.1f}, CR={d_cr:.1f}) "
                "-> head is not turning with the cursor"
            )
        if not clears_floor:
            msg.append(f"extreme diff {d_lr:.1f} below noise floor {args.min_extreme_diff}")
        if not monotonic:
            msg.append(f"head centroid not monotonic in cursor X: {verdict['centroid_x']}")
        print("FAIL: " + "; ".join(msg), file=sys.stderr)
        return 1
    print("PASS: rendered head turns monotonically with cursor; opposite extremes differ most.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
