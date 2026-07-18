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
    r"(?<![A-Za-z0-9_])"  # pi-lens-ignore: python-thread-global-write — false positive; regex literal, no threading
    r"(?:"
    r"0[xX][0-9A-Fa-f](?:_?[0-9A-Fa-f])*"
    r"|"
    r"\d(?:_?\d)*(?:\.\d(?:_?\d)*)?"
    r")"
    r"(?:[A-Za-z_][A-Za-z0-9_]*)?"
    r"(?![A-Za-z0-9_])"
)
CONSTANT_DECLARATION_RE = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const|static)\s+[A-Z_][A-Z0-9_]*\b"  # pi-lens-ignore: python-thread-global-write — false positive; regex literal, no threading
)
FILE_ALLOW_MARKER = "check-no-magic-numbers: allow-file"
FILE_ALLOW_HEADER_LINES = 20


def file_allows_unnamed_numbers(path: Path) -> bool:
    """Return true for explicitly marked binary-format helper files.

    This is a narrow escape hatch for offline asset-conversion scripts whose
    numeric values are external file-format fields, byte widths, and manifest
    literals rather than runtime IDs or product behavior. The marker must live
    in the file header so bypasses remain intentional and reviewable.
    """
    header = "\n".join(
        path.read_text(encoding="utf-8").splitlines()[
            :FILE_ALLOW_HEADER_LINES
        ]  # pi-lens-ignore: python-thread-global-write — false positive; file read, no threading
    )
    return FILE_ALLOW_MARKER in header


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
                    index += 1  # pi-lens-ignore: python-thread-global-write — false positive; local parser index, no threading
                    break
                index += 1  # pi-lens-ignore: python-thread-global-write — false positive; local parser index, no threading
            continue

        if char == "'":
            output.append("''")
            index += 1  # pi-lens-ignore: python-thread-global-write — false positive; local parser index, no threading
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

    for (
        line_number,
        original_line,
    ) in enumerate(  # pi-lens-ignore: python-thread-global-write — false positive; sequential file scan, no threading
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        stripped_line, in_block_comment = strip_strings_and_comments(
            original_line, in_block_comment
        )
        if CONSTANT_DECLARATION_RE.search(stripped_line):
            continue  # pi-lens-ignore: python-thread-global-write — false positive; loop control, no threading

        for match in NUMERIC_LITERAL_RE.finditer(stripped_line):
            findings.append(
                (line_number, match.group(), original_line.strip())
            )  # pi-lens-ignore: python-thread-global-write — false positive; local result list, no threading

    return findings


def rust_source_files() -> list[Path]:
    paths: list[Path] = []
    for path in REPO_ROOT.rglob("*.rs"):
        if any(
            part in IGNORED_DIRECTORIES for part in path.relative_to(REPO_ROOT).parts
        ):
            continue
        paths.append(path)
    return sorted(paths)


def main() -> int:
    failures: list[str] = []

    for path in rust_source_files():
        if file_allows_unnamed_numbers(path):
            continue
        for line_number, literal, line in numeric_literals_in(path):
            relative_path = path.relative_to(REPO_ROOT)
            failures.append(
                f"{relative_path}:{line_number}: unnamed numeric literal {literal!r}: {line}"
            )

    if failures:
        sys.stderr.write("Unnamed numeric literals are banned in Rust source.\n")
        sys.stderr.write(
            "Move the value to a named const/static and use the name instead.\n\n"
        )
        sys.stderr.write("\n".join(failures))
        sys.stderr.write("\n")
        return 1

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
