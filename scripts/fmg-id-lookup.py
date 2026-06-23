#!/usr/bin/env python3
"""Look up Elden Ring FMG text by message ID across an extracted WitchyBND XML tree.

The extracted tree (one `*.fmg.xml` per FMG, `<text id="N">string</text>` entries)
is the offline, decompressed mirror of `msg/<lang>/menu.msgbnd.dcx` etc. This avoids
the Oodle/BHD unpack path entirely.

Usage:
  fmg-id-lookup.py --root <dir> id [id ...]          # find these ids, print fmg+text
  fmg-id-lookup.py --root <dir> --max-ids            # print per-file max id (range probe)
  fmg-id-lookup.py --root <dir> --near <id> [--win N] # ids within +/- N of <id>, per file

Ids may be decimal or 0x-hex. Default root is the menu tree.
"""
from __future__ import annotations

import argparse
import glob
import os
import re
import sys
import xml.etree.ElementTree as ET

DEFAULT_ROOT = "/home/banon/projects/er-msg/engus"


def parse_id(tok: str) -> int:
    tok = tok.strip()
    return int(tok, 16) if tok.lower().startswith("0x") else int(tok)


def iter_fmg_xml(root: str):
    yield from sorted(glob.glob(os.path.join(root, "**", "*.fmg.xml"), recursive=True))


def load_entries(path: str) -> dict[int, str]:
    out: dict[int, str] = {}
    try:
        tree = ET.parse(path)
    except ET.ParseError as exc:
        print(f"  (parse error {path}: {exc})", file=sys.stderr)
        return out
    for el in tree.iter("text"):
        raw = el.get("id")
        if raw is None:
            continue
        try:
            out[int(raw)] = (el.text or "")
        except ValueError:
            continue
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=DEFAULT_ROOT)
    ap.add_argument("--max-ids", action="store_true")
    ap.add_argument("--near", default=None)
    ap.add_argument("--win", type=int, default=300)
    ap.add_argument("ids", nargs="*")
    args = ap.parse_args()

    files = list(iter_fmg_xml(args.root))
    if not files:
        print(f"no *.fmg.xml under {args.root}", file=sys.stderr)
        return 2

    if args.max_ids:
        for path in files:
            entries = load_entries(path)
            if not entries:
                continue
            rel = os.path.relpath(path, args.root)
            print(f"{rel}\tmin={min(entries)}\tmax={max(entries)}\tn={len(entries)}")
        return 0

    if args.near is not None:
        target = parse_id(args.near)
        lo, hi = target - args.win, target + args.win
        for path in files:
            entries = load_entries(path)
            hits = {i: t for i, t in entries.items() if lo <= i <= hi}
            if not hits:
                continue
            rel = os.path.relpath(path, args.root)
            print(f"== {rel} ==")
            for i in sorted(hits):
                preview = " ".join(hits[i].split())[:160]
                print(f"  {i} (0x{i:x}): {preview}")
        return 0

    wanted = [parse_id(t) for t in args.ids]
    if not wanted:
        print("no ids given", file=sys.stderr)
        return 2
    remaining = set(wanted)
    for path in files:
        entries = load_entries(path)
        rel = os.path.relpath(path, args.root)
        for i in list(remaining):
            if i in entries:
                text = entries[i]
                print(f"FOUND {i} (0x{i:x}) in {rel}:")
                print("  " + text.replace("\n", "\n  "))
                print()
                remaining.discard(i)
    for i in sorted(remaining):
        print(f"NOT-FOUND {i} (0x{i:x}) in any FMG under {args.root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
