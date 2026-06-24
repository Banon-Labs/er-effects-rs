#!/usr/bin/env python3
"""Delete named top-level items (fn/static/const/struct/enum) from a Rust file,
including each item's immediately-preceding doc-comment / attribute block. Shares
the comment/string-aware item-boundary logic with extract-experiments-items.py.

    delete-rust-items.py <file.rs> <name1> [<name2> ...]

Aborts if any requested name is not found (so a typo never silently no-ops).
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

ITEM_RE = re.compile(
    r"^(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?"
    r"(fn|static|const|struct|enum|impl|trait|type|mod)\s+"
    r"(?:mut\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)"
)
SEMI_ITEMS = {"static", "const", "type"}
DECL_RE = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;\s*$")
USE_RE = re.compile(r"\s*pub(?:\([^)]*\))?\s+use\s+\w+::\*\s*;\s*$")


def _significant_chars(line: str):
    i, n = 0, len(line)
    while i < n:
        c = line[i]
        if c == "/" and i + 1 < n and line[i + 1] == "/":
            return
        if c == '"':
            i += 1
            while i < n:
                if line[i] == "\\":
                    i += 2
                    continue
                if line[i] == '"':
                    i += 1
                    break
                i += 1
            continue
        if c == "'":
            j = i + 1
            j += 2 if (j < n and line[j] == "\\") else 1
            if j < n and line[j] == "'":
                i = j + 1
                continue
            i += 1
            continue
        yield c
        i += 1


def find_item_end(lines, start, kind):
    if kind in SEMI_ITEMS:
        depth = 0
        for i in range(start, len(lines)):
            for ch in _significant_chars(lines[i]):
                if ch in "([{":
                    depth += 1
                elif ch in ")]}":
                    depth -= 1
                elif ch == ";" and depth == 0:
                    return i
        raise SystemExit(f"unterminated item at line {start + 1}")
    depth, seen = 0, False
    for i in range(start, len(lines)):
        for ch in _significant_chars(lines[i]):
            if ch == "{":
                depth += 1
                seen = True
            elif ch == "}":
                depth -= 1
                if seen and depth == 0:
                    return i
    raise SystemExit(f"unterminated item at line {start + 1}")


def doc_attr_start(lines, sig):
    i = sig
    while i > 0 and lines[i - 1].lstrip().startswith(("///", "//!", "//", "#[", "#![")):
        i -= 1
    return i


def main() -> int:
    if len(sys.argv) < 3:
        raise SystemExit(__doc__)
    path = Path(sys.argv[1])
    wanted = set(sys.argv[2:])
    lines = path.read_text(encoding="utf-8").splitlines(keepends=True)
    spans, found = [], set()
    i, n = 0, len(lines)
    while i < n:
        if DECL_RE.match(lines[i]) or USE_RE.match(lines[i]):
            i += 1
            continue
        m = ITEM_RE.match(lines[i])
        if m and lines[i][0] not in " \t":
            kind, name = m.group(1), m.group(2)
            end = find_item_end(lines, i, kind)
            if name in wanted:
                spans.append((doc_attr_start(lines, i), end))
                found.add(name)
            i = end + 1
        else:
            i += 1
    missing = wanted - found
    if missing:
        raise SystemExit(f"not found (typo? already gone?): {sorted(missing)}")
    drop = set()
    for s, e in spans:
        drop.update(range(s, e + 1))
    kept = [l for idx, l in enumerate(lines) if idx not in drop]
    path.write_text("".join(kept), encoding="utf-8")
    print(f"deleted {len(spans)} items ({len(drop)} lines) from {path}; now {len(kept)} lines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
