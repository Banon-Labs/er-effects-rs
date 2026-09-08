#!/usr/bin/env python3
"""Apply `audit-1170-hook-targets.py`'s detour checks to an arbitrary candidate map file.

`audit-1170-hook-targets.py` only reads the tracked `docs/recon/*.tsv` inputs, so a
migration agent holding fresh candidate rows cannot ask it the one question that matters
before a row is proposed: is the 1.17 destination a real function entry, and do its first
five bytes relocate? Those are exactly MinHook's two preconditions, and a row that fails
either corrupts a live function rather than merely losing a feature.

This reuses that script's own `entry_verdict` / `patch_safe` / `pdata_entry_starts` /
`xref_targets` -- no second implementation to drift -- and applies them to a map file in
the same format `verify-rva-map-1170.py --map` accepts. It reads only; it writes nothing
into `docs/recon`.

Usage
    uv run --with capstone python3 scripts/audit-1170-detour-mapfile.py <map.tsv>
"""

import importlib.util
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = 0x140000000
OVERLAP_BYTES = 16


def load_audit():
    path = os.path.join(ROOT, "scripts", "audit-1170-hook-targets.py")
    spec = importlib.util.spec_from_file_location("audit1170", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def read_pairs(path):
    pairs = []
    for line in open(path, encoding="utf-8"):
        if line.startswith("#") or not line.strip():
            continue
        fields = line.split("\t")
        if len(fields) < 2:
            continue
        try:
            a, b = int(fields[0], 16), int(fields[1], 16)
        except ValueError:
            continue
        pairs.append((a if a >= BASE else a + BASE, b if b >= BASE else b + BASE))
    return pairs


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    audit = load_audit()
    pairs = read_pairs(sys.argv[1])
    blob = open(audit.IMAGE_1170, "rb").read()
    src_blob = open(audit.IMAGE_1162, "rb").read()
    hits = audit.xref_targets(blob, {b for _a, b in pairs})
    src_hits = audit.xref_targets(src_blob, {a for a, _b in pairs})
    starts = audit.pdata_entry_starts(blob)
    src_starts = audit.pdata_entry_starts(src_blob)
    ok_count = 0
    previous = None
    for a, b in sorted(pairs, key=lambda p: p[1]):
        entry_ok, entry_why = audit.entry_verdict(hits[b], starts, b)
        patch_ok, patch_why = audit.patch_safe(blob, b)
        # The same two questions asked of the 1.16.2 original. A row whose source also fails
        # is not evidence the translation is wrong -- it is a hook that was already like that.
        src_entry_ok, src_entry_why = audit.entry_verdict(src_hits[a], src_starts, a)
        src_patch_ok, _src_patch_why = audit.patch_safe(src_blob, a)
        overlap = previous is not None and b - previous < OVERLAP_BYTES
        previous = b
        verdict = "DETOUR-SAFE" if (entry_ok and patch_ok and not overlap) else "REJECT"
        if verdict == "DETOUR-SAFE":
            ok_count += 1
        print(f"{hex(a)} -> {hex(b)}  {verdict}")
        print(f"    1.17   entry: {entry_ok} ({entry_why});  patch: {patch_ok} ({patch_why})")
        print(f"    1.16.2 entry: {src_entry_ok} ({src_entry_why});  patch: {src_patch_ok}")
        if overlap:
            print("    overlaps the previous target")
    print(f"\n{ok_count}/{len(pairs)} detour-safe")
    return 0


if __name__ == "__main__":
    sys.exit(main())
