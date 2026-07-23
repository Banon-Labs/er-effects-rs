#!/usr/bin/env python3
"""Pixel-diff oracle for the armament-icons badge (bd er-effects-rs-pe98).

Two subcommands:

  locate  --vanilla V.png --locator L.png [--out box.json] [--viz viz.png]
      Diff a LOCATOR capture (badge DLL forced to a guaranteed-visible icon) against a
      VANILLA capture (badge DLL omitted). The connected changed-pixel regions are the
      drawn badges. Emits the bounding boxes (JSON) and a visualization with boxes drawn,
      so the crop rect for the highlighted tile can be confirmed.

  verdict --baseline V.png --candidate C.png --stage-box "x,y,w,h" [--threshold T]
      RESOLUTION-INDEPENDENT: the crop is given in GFX STAGE coordinates (the ER menu stage is
      fixed 1920x1080). Both images are mapped stage->pixels from their OWN actual dimensions
      (uniform scale for 16:9; letterbox-fit + center offset otherwise), so the same stage-box
      works at any capture resolution. Crop both to that box and compare. Prints one line:
        SUCCESS  -- candidate differs from the vanilla baseline beyond threshold (glyph present)
        FAILURE  -- candidate ~matches vanilla (no glyph drawn)
      and exits 0 (SUCCESS) / 1 (FAILURE). TIMEOUT is decided by the caller (menu never reached).

The metric is the fraction of pixels whose max per-channel abs difference exceeds 24 (a
per-pixel change), robust to minor compression/AA noise. Threshold default 0.02 (2% of the
cropped area changed = a real glyph).
"""
from __future__ import annotations

import argparse
import json
import sys

import numpy as np
from PIL import Image, ImageDraw

CHANNEL_DELTA = 24          # per-pixel per-channel change to count as "changed"
DEFAULT_THRESHOLD = 0.02    # fraction of cropped pixels changed => glyph present
STAGE_W = 1920.0            # ER menu GFX stage width  (fixed; 02_010_equiptop movie rect)
STAGE_H = 1080.0            # ER menu GFX stage height


def stage_to_pixels(img_w: int, img_h: int, sx: float, sy: float, sw: float, sh: float):
    """Map a stage-space rect to pixel coords for an image of arbitrary size.
    Uniform fit preserving the 16:9 stage aspect, centered (letterbox) if the image
    is not 16:9 -- so the crop is resolution-independent."""
    scale = min(img_w / STAGE_W, img_h / STAGE_H)
    off_x = (img_w - STAGE_W * scale) / 2.0
    off_y = (img_h - STAGE_H * scale) / 2.0
    x = int(round(off_x + sx * scale))
    y = int(round(off_y + sy * scale))
    w = int(round(sw * scale))
    h = int(round(sh * scale))
    return x, y, w, h


def load_rgb(path: str) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.int16)


def changed_mask(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    h = min(a.shape[0], b.shape[0])
    w = min(a.shape[1], b.shape[1])
    diff = np.abs(a[:h, :w] - b[:h, :w]).max(axis=2)
    return diff > CHANNEL_DELTA


def boxes_from_mask(mask: np.ndarray, min_area: int = 200) -> list[dict]:
    """Simple connected-component labeling via scipy-free flood fill over a downsampled grid."""
    ys, xs = np.nonzero(mask)
    if len(xs) == 0:
        return []
    # Cluster changed pixels into boxes by a coarse grid merge (no scipy dependency):
    # bucket into 8px cells, union adjacent occupied cells.
    cell = 8
    cells = set(zip((ys // cell).tolist(), (xs // cell).tolist()))
    seen: set = set()
    boxes = []
    for c in list(cells):
        if c in seen:
            continue
        stack = [c]
        comp = []
        while stack:
            cur = stack.pop()
            if cur in seen or cur not in cells:
                continue
            seen.add(cur)
            comp.append(cur)
            cy, cx = cur
            for dy in (-1, 0, 1):
                for dx in (-1, 0, 1):
                    stack.append((cy + dy, cx + dx))
        cy = [p[0] for p in comp]
        cx = [p[1] for p in comp]
        y0, y1 = min(cy) * cell, (max(cy) + 1) * cell
        x0, x1 = min(cx) * cell, (max(cx) + 1) * cell
        area = (y1 - y0) * (x1 - x0)
        if area >= min_area:
            boxes.append({"x": int(x0), "y": int(y0), "w": int(x1 - x0), "h": int(y1 - y0),
                          "area": int(area)})
    boxes.sort(key=lambda b: -b["area"])
    return boxes


def pixels_to_stage(img_w: int, img_h: int, x: float, y: float, w: float, h: float):
    """Inverse of stage_to_pixels: pixel rect -> stage-space rect (resolution-independent)."""
    scale = min(img_w / STAGE_W, img_h / STAGE_H)
    off_x = (img_w - STAGE_W * scale) / 2.0
    off_y = (img_h - STAGE_H * scale) / 2.0
    return (round((x - off_x) / scale, 1), round((y - off_y) / scale, 1),
            round(w / scale, 1), round(h / scale, 1))


def cmd_locate(args) -> int:
    v = load_rgb(args.vanilla)
    l = load_rgb(args.locator)
    ih, iw = l.shape[0], l.shape[1]
    mask = changed_mask(v, l)
    boxes = boxes_from_mask(mask)
    print(f"locate: {len(boxes)} changed region(s); total changed px={int(mask.sum())}; "
          f"capture={iw}x{ih}")
    for i, b in enumerate(boxes[:12]):
        st = pixels_to_stage(iw, ih, b["x"], b["y"], b["w"], b["h"])
        print(f"  box[{i}] px=({b['x']},{b['y']},{b['w']},{b['h']}) "
              f"stage=({st[0]},{st[1]},{st[2]},{st[3]}) area={b['area']}")
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump({"boxes": boxes}, f, indent=2)
    if args.viz:
        img = Image.open(args.locator).convert("RGB")
        d = ImageDraw.Draw(img)
        for i, b in enumerate(boxes[:12]):
            d.rectangle([b["x"], b["y"], b["x"] + b["w"], b["y"] + b["h"]], outline=(255, 0, 0), width=3)
            d.text((b["x"], max(0, b["y"] - 14)), str(i), fill=(255, 255, 0))
        img.save(args.viz)
        print(f"locate: viz -> {args.viz}")
    return 0


def cmd_verdict(args) -> int:
    sx, sy, sw, sh = (float(v) for v in args.stage_box.split(","))
    base_img = load_rgb(args.baseline)
    cand_img = load_rgb(args.candidate)
    bx, by, bw, bh = stage_to_pixels(base_img.shape[1], base_img.shape[0], sx, sy, sw, sh)
    cx, cy, cw, ch = stage_to_pixels(cand_img.shape[1], cand_img.shape[0], sx, sy, sw, sh)
    base = base_img[by:by + bh, bx:bx + bw]
    cand = cand_img[cy:cy + ch, cx:cx + cw]
    mask = changed_mask(base, cand)
    frac = float(mask.mean()) if mask.size else 0.0
    ok = frac >= args.threshold
    print(f"verdict: {'SUCCESS' if ok else 'FAILURE'} changed_fraction={frac:.4f} "
          f"threshold={args.threshold} stage_box=({sx},{sy},{sw},{sh}) "
          f"baseline_px=({bx},{by},{bw},{bh}) candidate_px=({cx},{cy},{cw},{ch})")
    return 0 if ok else 1


def main() -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    lo = sub.add_parser("locate")
    lo.add_argument("--vanilla", required=True)
    lo.add_argument("--locator", required=True)
    lo.add_argument("--out")
    lo.add_argument("--viz")
    lo.set_defaults(fn=cmd_locate)
    ve = sub.add_parser("verdict")
    ve.add_argument("--baseline", required=True)
    ve.add_argument("--candidate", required=True)
    ve.add_argument("--stage-box", required=True, help="x,y,w,h in 1920x1080 stage units")
    ve.add_argument("--threshold", type=float, default=DEFAULT_THRESHOLD)
    ve.set_defaults(fn=cmd_verdict)
    args = ap.parse_args()
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
