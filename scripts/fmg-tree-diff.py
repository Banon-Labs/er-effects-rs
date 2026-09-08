#!/usr/bin/env python3
"""Diff two WitchyBND-extracted FMG XML trees id-by-id (e.g. 1.16.2 vs 1.17 msg/engus).

Companion to `fmg-id-lookup.py`, which reads one tree. This one answers the
version-drift question: which text ids were added, removed, or re-worded between
two extractions of `msg/<lang>/*.msgbnd.dcx`.

Both trees are produced by WitchyBND (`--passive --recursive --unpack --location <dir>`)
and hold one `*.fmg.xml` per FMG with `<text id="N">string</text>` entries.
Extracted game assets live outside this repo; pass their paths in.

Usage:
  fmg-tree-diff.py <old_root> <new_root> [--only SUBSTR] [--ids-only] [--quiet-text]
"""
from __future__ import annotations

import argparse
import glob
import os
import sys
import xml.etree.ElementTree as ET


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
            out[int(raw)] = el.text or ""
        except ValueError:
            continue
    return out


def index_tree(root: str) -> dict[str, str]:
    found: dict[str, str] = {}
    for path in sorted(glob.glob(os.path.join(root, "**", "*.fmg.xml"), recursive=True)):
        found[os.path.relpath(path, root)] = path
    return found


def flat(text: str, limit: int = 110) -> str:
    return " ".join((text or "").split())[:limit]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("old_root")
    ap.add_argument("new_root")
    ap.add_argument("--only", default=None, help="only files whose relpath contains this substring")
    ap.add_argument("--ids-only", action="store_true", help="report id set changes, ignore re-wordings")
    ap.add_argument("--quiet-text", action="store_true", help="counts per file, no per-id lines")
    args = ap.parse_args()

    old, new = index_tree(args.old_root), index_tree(args.new_root)
    if not old:
        print(f"no *.fmg.xml under {args.old_root}", file=sys.stderr)
        return 2
    if not new:
        print(f"no *.fmg.xml under {args.new_root}", file=sys.stderr)
        return 2

    t_add = t_rm = t_chg = 0
    for key in sorted(set(old) | set(new)):
        if args.only and args.only not in key:
            continue
        if key not in old:
            print(f"### FILE-ADDED   {key}")
            continue
        if key not in new:
            print(f"### FILE-REMOVED {key}")
            continue
        a, b = load_entries(old[key]), load_entries(new[key])
        added = sorted(set(b) - set(a))
        removed = sorted(set(a) - set(b))
        changed = [] if args.ids_only else sorted(i for i in set(a) & set(b) if a[i] != b[i])
        if not (added or removed or changed):
            continue
        t_add += len(added)
        t_rm += len(removed)
        t_chg += len(changed)
        print(f"== {key}  n_old={len(a)} n_new={len(b)}  +{len(added)} -{len(removed)} ~{len(changed)}")
        if args.quiet_text:
            continue
        for i in added:
            print(f"   + {i}: {flat(b[i])}")
        for i in removed:
            print(f"   - {i}: {flat(a[i])}")
        for i in changed:
            print(f"   ~ {i}:")
            print(f"       old: {flat(a[i])}")
            print(f"       new: {flat(b[i])}")

    print(f"\nTOTALS added={t_add} removed={t_rm} changed={t_chg}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
