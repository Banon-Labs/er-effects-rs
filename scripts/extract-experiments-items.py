#!/usr/bin/env python3
"""Untangle: move a NAMED SET of top-level items out of src/experiments/mod.rs
into a sibling submodule, preserving behavior (pure code motion).

    extract-experiments-items.py <module> <name1> [<name2> ...]
    extract-experiments-items.py <module> --suffix _enabled --suffix _disabled
    extract-experiments-items.py <module> --names-file <path>

Unlike the line-range carver, this pulls items by NAME wherever they sit in the
interleaved core. For each requested top-level item (fn / unsafe fn / static /
const / struct / enum / impl-with-name), it takes the item plus its immediately
preceding contiguous doc-comment / attribute block, removes them from mod.rs,
and writes them (in original file order) to src/experiments/<module>.rs with a
copy of mod.rs's import preamble + `use super::*;`. A
`mod <module>; pub(crate) use <module>::*;` decl is inserted into mod.rs after
the preamble.

Item end detection: brace-matched for fn/struct/enum/impl/trait/mod; first
`;` at bracket-depth 0 for static/const/type. Names not found are reported and
abort the run (so a typo never silently drops an item).
"""
from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
MOD = REPO_ROOT / "src" / "experiments" / "mod.rs"

ITEM_RE = re.compile(
    r"^(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?"
    r"(fn|static|const|struct|enum|impl|trait|type|mod)\s+"
    r"(?:mut\s+)?"
    r"([A-Za-z_][A-Za-z0-9_]*)"
)
BRACE_ITEMS = {"fn", "struct", "enum", "impl", "trait", "mod"}
SEMI_ITEMS = {"static", "const", "type"}
DECL_RE = re.compile(r"\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;\s*$")
USE_RE = re.compile(r"\s*pub(?:\([^)]*\))?\s+use\s+\w+::\*\s*;\s*$")


def detect_preamble_end(lines: list[str]) -> int:
    for i, line in enumerate(lines):
        s = line.strip()
        if s.startswith((
            "pub(crate) const", "pub(crate) static", "pub(crate) fn",
            "pub(crate) unsafe fn", "static ", "const ", "fn ",
            "unsafe fn ", "struct ", "enum ", "impl ",
        )):
            return i
    raise SystemExit("could not locate end of preamble")


def _significant_chars(line: str):
    """Yield code characters of a line, skipping // line-comments, /* */ blocks
    (single-line only, which is all that occurs here), and string/char literals.
    Bracket/semicolon counting must ignore brackets inside comments and strings."""
    i = 0
    n = len(line)
    while i < n:
        c = line[i]
        if c == "/" and i + 1 < n and line[i + 1] == "/":
            return  # rest of line is a comment
        if c == "/" and i + 1 < n and line[i + 1] == "*":
            end = line.find("*/", i + 2)
            if end == -1:
                return
            i = end + 2
            continue
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
            # char literal or lifetime; only treat as literal if it closes soon
            j = i + 1
            if j < n and line[j] == "\\":
                j += 2
            else:
                j += 1
            if j < n and line[j] == "'":
                i = j + 1
                continue
            # lifetime tick (e.g. 'static) -- not a literal, emit nothing special
            i += 1
            continue
        yield c
        i += 1


def find_item_end(lines: list[str], start: int, kind: str) -> int:
    """Return the index (inclusive) of the item's last line (comment/string aware)."""
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
    # brace-matched
    depth = 0
    seen_brace = False
    for i in range(start, len(lines)):
        for ch in _significant_chars(lines[i]):
            if ch == "{":
                depth += 1
                seen_brace = True
            elif ch == "}":
                depth -= 1
                if seen_brace and depth == 0:
                    return i
    raise SystemExit(f"unterminated item at line {start + 1}")


def doc_attr_start(lines: list[str], sig: int) -> int:
    """Walk upward over the contiguous doc-comment / attribute / comment block
    immediately above the item signature (stops at a blank line or other code)."""
    i = sig
    while i > 0:
        s = lines[i - 1].lstrip()
        if s.startswith(("///", "//!", "//", "#[", "#![")):
            i -= 1
            continue
        break
    return i


def main() -> int:
    args = sys.argv[1:]
    if len(args) < 2:
        raise SystemExit(__doc__)
    module = args[0]
    rest = args[1:]
    names: set[str] = set()
    suffixes: list[str] = []
    i = 0
    while i < len(rest):
        a = rest[i]
        if a == "--suffix":
            suffixes.append(rest[i + 1]); i += 2
        elif a == "--names-file":
            names.update(
                ln.strip() for ln in Path(rest[i + 1]).read_text().splitlines()
                if ln.strip() and not ln.strip().startswith("#")
            )
            i += 2
        else:
            names.add(a); i += 1

    text = MOD.read_text(encoding="utf-8")
    lines = text.splitlines(keepends=True)
    preamble_end = detect_preamble_end(lines)

    # Index every top-level item: name -> (block_start, end_inclusive)
    items: dict[str, tuple[int, int]] = {}
    order: list[str] = []
    i = 0
    n = len(lines)
    while i < n:
        # `mod foo;` / `pub(crate) use foo::*;` submodule decls are not extractable
        # items; skip them so the scanner never tries to brace-match a `mod foo;`.
        if DECL_RE.match(lines[i]) or USE_RE.match(lines[i]):
            i += 1
            continue
        m = ITEM_RE.match(lines[i])
        if m and (lines[i][0] not in " \t"):
            kind, name = m.group(1), m.group(2)
            end = find_item_end(lines, i, kind)
            block_start = doc_attr_start(lines, i)
            key = name if name not in items else f"{name}@{i}"
            items[key] = (block_start, end)
            order.append(key)
            i = end + 1
        else:
            i += 1

    # Resolve requested names (exact + suffix match), keep file order.
    selected: list[tuple[int, int, str]] = []
    matched_names: set[str] = set()
    for key in order:
        base = key.split("@")[0]
        if base in names or any(base.endswith(s) for s in suffixes):
            selected.append((*items[key], base))
            matched_names.add(base)

    missing = names - matched_names
    if missing:
        raise SystemExit(f"items not found (typo? already moved?): {sorted(missing)}")
    if not selected:
        raise SystemExit("no items matched")

    selected.sort()
    # Guard: nothing in the preamble.
    if selected[0][0] < preamble_end:
        raise SystemExit("a selected item starts inside the preamble")

    moved_idx: set[int] = set()
    for start, end, _ in selected:
        moved_idx.update(range(start, end + 1))

    preamble_lines = [
        ln for ln in lines[:preamble_end]
        if not DECL_RE.match(ln) and not USE_RE.match(ln)
    ]
    moved = "".join("".join(lines[start:end + 1]) for start, end, _ in selected)

    out = REPO_ROOT / "src" / "experiments" / f"{module}.rs"
    header = "".join(preamble_lines) + "\nuse super::*;\n\n"
    if out.exists():
        body = out.read_text(encoding="utf-8")
        out.write_text(body + "\n" + moved, encoding="utf-8")
        appended = True
    else:
        out.write_text(header + moved, encoding="utf-8")
        appended = False

    decl = f"\nmod {module};\npub(crate) use {module}::*;\n"
    # Reassemble mod.rs minus moved lines (insert decl only on first creation).
    rebuilt: list[str] = []
    rebuilt.extend(lines[:preamble_end])
    if not appended:
        rebuilt.append(decl)
    for idx in range(preamble_end, n):
        if idx not in moved_idx:
            rebuilt.append(lines[idx])
    MOD.write_text("".join(rebuilt), encoding="utf-8")

    print(f"moved {len(selected)} items ({sum(e - s + 1 for s, e, _ in selected)} lines) "
          f"-> {out.relative_to(REPO_ROOT)}{' (appended)' if appended else ''}")
    print(f"mod.rs now {len(MOD.read_text().splitlines())} lines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
