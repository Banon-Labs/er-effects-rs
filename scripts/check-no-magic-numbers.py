#!/usr/bin/env python3
"""Reject unnamed numeric literals in Rust source files.

Numeric values are allowed only on constant/static declaration lines. This keeps
runtime IDs and UI/layout values named before they are used in implementation
code.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
IGNORED_DIRECTORIES = {".git", "target"}
NUMERIC_LITERAL_RE = re.compile(
    r"(?<![A-Za-z0-9_.])"
    r"(?:"
    r"0[xX][0-9A-Fa-f](?:_?[0-9A-Fa-f])*"
    r"|"
    r"\d(?:_?\d)*(?:\.\d(?:_?\d)*)?"
    r")"
    r"(?:[A-Za-z_][A-Za-z0-9_]*)?"
    r"(?![A-Za-z0-9_])"
)
CONSTANT_DECLARATION_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+[A-Z_][A-Z0-9_]*\b"
)


def strip_strings_and_comments(line: str, in_block_comment: bool) -> tuple[str, bool]:
    output: list[str] = []
    index = 0

    while index < len(line):
        char = line[index]
        nxt = line[index + 1] if index + 1 < len(line) else ""

        if in_block_comment:
            if char == "*" and nxt == "/":
                in_block_comment = False
                index += 2
            else:
                index += 1
            continue

        if char == "/" and nxt == "/":
            break

        if char == "/" and nxt == "*":
            in_block_comment = True
            index += 2
            continue

        if char == '"':
            output.append('""')
            index += 1
            while index < len(line):
                if line[index] == "\\":
                    index += 2
                    continue
                if line[index] == '"':
                    index += 1
                    break
                index += 1
            continue

        if char == "'":
            output.append("''")
            index += 1
            while index < len(line):
                if line[index] == "\\":
                    index += 2
                    continue
                if line[index] == "'":
                    index += 1
                    break
                index += 1
            continue

        output.append(char)
        index += 1

    return "".join(output), in_block_comment


def numeric_literals_in(path: Path) -> list[tuple[int, str, str]]:
    findings: list[tuple[int, str, str]] = []
    in_block_comment = False

    for line_number, original_line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        stripped_line, in_block_comment = strip_strings_and_comments(original_line, in_block_comment)
        if CONSTANT_DECLARATION_RE.search(stripped_line):
            continue

        for match in NUMERIC_LITERAL_RE.finditer(stripped_line):
            findings.append((line_number, match.group(), original_line.strip()))

    return findings


def rust_source_files() -> list[Path]:
    paths: list[Path] = []
    for path in REPO_ROOT.rglob("*.rs"):
        if any(part in IGNORED_DIRECTORIES for part in path.relative_to(REPO_ROOT).parts):
            continue
        paths.append(path)
    return sorted(paths)


def main() -> int:
    failures: list[str] = []

    for path in rust_source_files():
        for line_number, literal, line in numeric_literals_in(path):
            relative_path = path.relative_to(REPO_ROOT)
            failures.append(f"{relative_path}:{line_number}: unnamed numeric literal {literal!r}: {line}")

    if failures:
        print("Unnamed numeric literals are banned in Rust source.", file=sys.stderr)
        print("Move the value to a named const/static and use the name instead.\n", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
