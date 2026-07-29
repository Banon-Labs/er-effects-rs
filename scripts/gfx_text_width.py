#!/usr/bin/env python3
"""Measure how wide a string renders in one of the game's own Scaleform fonts.

Answers the question every menu-text change eventually asks -- "does this fit the
field?" -- from the asset instead of from a guess. Reads the `DefineFont3` advance
table out of a `font.gfx` in the local extraction corpus (never committed) and sums
the per-glyph advances for the string, scaled to a `DefineEditText`'s authored font
height.

    # Does the picker's last-saved text fit the 05_010 PlayTime field?
    #   PlayTime (char 72): bounds -40..3960 twips = 200px, font height 480 twips = 24px
    python3 scripts/gfx_text_width.py --height-px 24 --box-px 200 \
        '0:00:00' 'Last saved: 2026-07-29 08:03' '2026-07-29 08:03'

Font root defaults to the local extraction corpus and is overridable:
    ER_GFX_FONT_GFX   full path to a specific font.gfx
    ER_GFX_FONT_ROOT  directory of per-language font dirs (…/font/<lang>/font.gfx)

Exit status is 1 when any measured string overflows `--box-px`, so this can gate a
change as well as inform one.
"""
import argparse
import json
import os
import struct
import sys

DEFAULT_FONT_ROOT = "/home/banon/er-extract/LOOK_HERE_ALL_ASSETS_20260713/font"
DEFAULT_LANG = "eu_std"
# DefineFont3 coordinates are 20x finer than DefineFont2's 1024-unit em square.
EM_UNITS = 20480.0
TAG_DEFINE_FONT3 = 75


def font_path(lang):
    override = os.environ.get("ER_GFX_FONT_GFX", "").strip()
    if override:
        return override
    root = os.environ.get("ER_GFX_FONT_ROOT", "").strip() or DEFAULT_FONT_ROOT
    return os.path.join(root, lang, "font.gfx")


def iter_tags(data):
    """Yield (code, body_offset, body_len) for each top-level tag in a GFX/SWF movie."""
    if data[:3] not in (b"GFX", b"FWS"):
        raise SystemExit(f"not an uncompressed GFX/SWF movie (magic {data[:3]!r})")
    # header: magic(3) version(1) file_len(4) then RECT + frame rate + frame count
    pos = 8
    nbits = data[pos] >> 3
    rect_bits = 5 + 4 * nbits
    pos += (rect_bits + 7) // 8
    pos += 4  # frame rate (8.8) + frame count
    while pos + 2 <= len(data):
        th = struct.unpack_from("<H", data, pos)[0]
        code, ln = th >> 6, th & 0x3F
        pos += 2
        if ln == 0x3F:
            ln = struct.unpack_from("<I", data, pos)[0]
            pos += 4
        yield code, pos, ln
        pos += ln
        if code == 0:
            return


def parse_font3(data, body, length):
    """Parse a DefineFont3 body into (info, {char: advance_units})."""
    b = data[body:body + length]
    font_id = struct.unpack_from("<H", b, 0)[0]
    flags = b[2]
    name_len = b[4]
    name = b[5:5 + name_len].decode("latin1", "replace")
    pos = 5 + name_len
    n_glyphs = struct.unpack_from("<H", b, pos)[0]
    pos += 2
    wide_offsets = bool(flags & 0x08)
    has_layout = bool(flags & 0x80)
    offset_table = pos
    if wide_offsets:
        pos += 4 * n_glyphs
        code_offset = struct.unpack_from("<I", b, pos)[0]
    else:
        pos += 2 * n_glyphs
        code_offset = struct.unpack_from("<H", b, pos)[0]
    code_table = offset_table + code_offset
    codes = list(struct.unpack_from("<%dH" % n_glyphs, b, code_table))
    after_codes = code_table + 2 * n_glyphs
    info = {
        "font_id": font_id,
        "font_name": name,
        "flags": hex(flags),
        "glyphs": n_glyphs,
        "wide_offsets": wide_offsets,
        "has_layout": has_layout,
    }
    if not has_layout:
        return info, {}
    ascent, descent, leading = struct.unpack_from("<3h", b, after_codes)
    advances = struct.unpack_from("<%dh" % n_glyphs, b, after_codes + 6)
    info.update({"ascent": ascent, "descent": descent, "leading": leading})
    return info, {chr(c): a for c, a in zip(codes, advances)}


def load_font(path):
    data = open(path, "rb").read()
    for code, body, length in iter_tags(data):
        if code == TAG_DEFINE_FONT3:
            return parse_font3(data, body, length)
    raise SystemExit(f"{path}: no DefineFont3 tag")


def measure(advances, text, height_px):
    total, missing = 0, []
    for ch in text:
        adv = advances.get(ch)
        if adv is None:
            missing.append(ch)
            adv = 0
        total += adv
    return total * height_px / EM_UNITS, missing


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("text", nargs="+", help="strings to measure")
    ap.add_argument("--lang", default=DEFAULT_LANG, help="font dir (default: %(default)s)")
    ap.add_argument("--height-px", type=float, required=True,
                    help="the DefineEditText font height in px (twips / 20)")
    ap.add_argument("--box-px", type=float, default=None,
                    help="field width in px; strings wider than this report OVERFLOWS")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    path = font_path(args.lang)
    if not os.path.exists(path):
        print(f"SKIP: font movie {path} not present (set ER_GFX_FONT_GFX/ER_GFX_FONT_ROOT)",
              file=sys.stderr)
        return 0
    info, advances = load_font(path)
    if not advances:
        print(f"SKIP: {path} font has no layout/advance table", file=sys.stderr)
        return 0

    rows, overflow = [], False
    for text in args.text:
        width, missing = measure(advances, text, args.height_px)
        fits = args.box_px is None or width <= args.box_px
        overflow |= not fits
        rows.append({"text": text, "width_px": round(width, 1),
                     "fits": fits, "missing_glyphs": missing})
    if args.json:
        print(json.dumps({"font": info, "path": path, "height_px": args.height_px,
                          "box_px": args.box_px, "rows": rows}, indent=2))
    else:
        print(f"{info['font_name'] or '(unnamed)'} id={info['font_id']} "
              f"glyphs={info['glyphs']} @ {args.height_px}px  [{path}]")
        for row in rows:
            box = f" / {args.box_px:.0f}px" if args.box_px is not None else ""
            verdict = "" if args.box_px is None else ("  FITS" if row["fits"] else "  OVERFLOWS")
            miss = f"  missing={row['missing_glyphs']}" if row["missing_glyphs"] else ""
            print(f"  {row['text']!r:34} {row['width_px']:7.1f}px{box}{verdict}{miss}")
    return 1 if overflow else 0


if __name__ == "__main__":
    sys.exit(main())
