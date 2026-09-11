#!/usr/bin/env python3
"""Append rows to the curated 1.16.2 -> 1.17 ledger without retyping the file.

`verify-rva-map-1170.py --tsv` TRUNCATES, so it cannot be used to add a hand-derived pair to a
ledger that already holds 100+ rows -- and a row's `how` column is a paragraph of derivation that
has no business being pasted through a shell. This reads the new rows from a TSV on disk and
inserts them immediately above the `CARRIED FORWARD` banner, which is where the writer's own
preserved rows begin.

Refuses to add a 1.16.2 address the ledger already carries: a duplicate source is two verdicts
about one address, and `build.rs` would silently take whichever it read first.

Usage: `python3 scripts/append-verified-rva-rows.py <rows.tsv> [--ledger PATH]`
"""

import argparse
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_LEDGER = os.path.join(ROOT, "docs/recon/rva-map-1162-to-1170.verified.tsv")
BANNER = "# CARRIED FORWARD"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("rows", help="TSV holding the rows to add, one per line")
    parser.add_argument("--ledger", default=DEFAULT_LEDGER)
    args = parser.parse_args()

    ledger = open(args.ledger, encoding="utf-8").read().splitlines()
    incoming = [
        line
        for line in open(args.rows, encoding="utf-8").read().splitlines()
        if line.strip() and not line.startswith("#")
    ]
    have = {line.split("\t")[0].strip().lower() for line in ledger if line.startswith("0x")}
    for line in incoming:
        fields = line.split("\t")
        if len(fields) < 7:
            sys.exit(f"a row needs at least 7 tab-separated columns, this one has {len(fields)}")
        source = fields[0].strip().lower()
        if source in have:
            sys.exit(f"{source} is already in {args.ledger}; two verdicts for one address")

    where = next((i for i, line in enumerate(ledger) if line.startswith(BANNER)), len(ledger))
    merged = ledger[:where] + incoming + ledger[where:]
    with open(args.ledger, "w", encoding="utf-8") as handle:
        handle.write("\n".join(merged) + "\n")
    print(f"added {len(incoming)} row(s) to {args.ledger} above line {where + 1}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
