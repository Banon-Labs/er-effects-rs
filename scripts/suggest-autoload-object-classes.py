#!/usr/bin/env python3
"""Propose the owning class for each unknown-STRUCT autoload offset, from the repo's own prose.

`detect-struct-field-drift.py --report` refuses to judge a constant whose structure it cannot
name, and it can only name one when the constant is written as `offset_of!(T, f)` or its name
carries one of ten known prefixes. That leaves 422 autoload offsets untyped -- and untyped means
untested, because a clearance is only valid per named object.

The type information is not actually missing: it is in the doc comment above each constant, where
the RE that produced the number was written down. This reads that prose and matches every token
in it against the RTTI class names harvested from the images themselves
(`scripts/rtti-classmap-both.py`), so a suggestion is only ever a class that DEMONSTRABLY exists
in both builds -- never a guess at a name.

A suggestion is a lead, not a verdict. `scripts/clear-fields-by-object.py` is what decides.
Output: `autoload-class-suggest.json` under the drift out-dir.
"""
from __future__ import annotations

import argparse
import collections
import csv
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
# Resolved by scripts/struct_drift_out.py, not spelled here: this used to be a literal
# containing an agent session UUID, which is correct for exactly one session and wrong for
# every other one. `$ER_STRUCT_DRIFT_OUT` still overrides, and so does `--out-dir`.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import struct_drift_out  # noqa: E402 -- the path is set up on the line above

DEFAULT_OUT = struct_drift_out.default_out()
# A class name shorter than this matches ordinary English in a comment.
MIN_NAME = 5
CONTEXT_LINES = 45
_TOKEN = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def simple_index(joined_path: Path) -> dict[str, list[str]]:
    index: dict[str, list[str]] = {}
    for line in joined_path.read_text(encoding="utf-8").splitlines():
        if line.startswith("#") or not line.strip():
            continue
        name = line.split("\t")[0]
        if "lambda" in name or "$" in name or name.count("::") > 1:
            continue
        leaf = name.split("::")[-1]
        if len(leaf) >= MIN_NAME and re.fullmatch(r"[A-Za-z0-9_]+", leaf):
            index.setdefault(leaf, []).append(name)
    return index


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    ap.add_argument("--selftest", action="store_true")
    args = ap.parse_args()
    if args.selftest:
        return selftest(args.out_dir)

    index = simple_index(args.out_dir / "rtti-joined.tsv")
    triage = args.out_dir / "unknown-struct-triage.tsv"
    rows = [r for r in csv.DictReader(triage.open(), delimiter="\t") if r["autoload"] == "A"]
    cache: dict[str, list[str]] = {}
    out = []
    for row in rows:
        path, line_no = row["site"].rsplit(":", 1)
        if path not in cache:
            cache[path] = (ROOT / path).read_text(encoding="utf-8", errors="replace").split("\n")
        lines = cache[path]
        n = int(line_no)
        context = "\n".join(lines[max(0, n - CONTEXT_LINES) : n + 2])
        # Tokenise the prose and look each token up, rather than running 9k regexes over it.
        hits: set[str] = set()
        for token in set(_TOKEN.findall(context)):
            for name in index.get(token, ()):
                hits.add(name)
        out.append({**row, "classes": sorted(hits)})
    dest = args.out_dir / "autoload-class-suggest.json"
    dest.write_text(json.dumps(out, indent=1), encoding="utf-8")
    named = sum(1 for r in out if r["classes"])
    print(f"{named} of {len(out)} autoload UNKNOWN-STRUCT constants have >=1 RTTI-backed "
          f"class suggestion\nwrote {dest}\n")
    counts = collections.Counter(c for r in out for c in r["classes"])
    for name, count in counts.most_common(50):
        print(f"  {count:4d}  {name}")
    return 0


def selftest(out_dir: Path) -> int:
    ok = True
    joined = out_dir / "rtti-joined.tsv"
    if not joined.is_file():
        print(f"SKIP: {joined} absent; run scripts/rtti-classmap-both.py")
        return 0
    index = simple_index(joined)
    # Positive CONTROLS: classes this migration definitely touches must be indexed by their leaf.
    for leaf, want in (("MoveMapStep", "CS::MoveMapStep"),
                       ("MenuJob", "CS::MenuJob"),
                       ("PlayerGameData", "CS::PlayerGameData")):
        if want not in index.get(leaf, []):
            print(f"FAIL: control {want} not indexed under {leaf} ({index.get(leaf)})")
            ok = False
    if ok:
        print("ok: 3 control classes indexed by leaf name")
    # MUTATION: the length floor is load-bearing -- without it, short leaves match English.
    short = [k for k in index if len(k) < MIN_NAME]
    if short:
        print(f"FAIL: mutation guard -- leaves shorter than {MIN_NAME} leaked in: {short[:5]}")
        ok = False
    else:
        print(f"ok: no leaf shorter than {MIN_NAME} chars is indexed")
    # MUTATION: lambda/template vtables must be excluded, or every comment matches noise.
    if any("lambda" in n for names in index.values() for n in names):
        print("FAIL: mutation guard -- lambda vtables leaked into the index")
        ok = False
    else:
        print("ok: lambda/template vtables excluded")
    print("SELFTEST", "PASS" if ok else "FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
