#!/usr/bin/env python3
"""Fail when a Rust crate reintroduces an FNV-1a implementation outside er-game-base.

The shared owner is `crates/er-game-base/src/fnv1a.rs`. Callers may use its byte, incremental,
and integer-mix entry points; they must not redeclare the offset basis/prime or rebuild the round.

Usage:
    python3 scripts/check-fnv1a-owner.py
    python3 scripts/check-fnv1a-owner.py --selftest
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
OWNER = Path("crates/er-game-base/src/fnv1a.rs")
OFFSET_BASIS = 0xCBF29CE484222325
PRIME = 0x100000001B3

NUMBER = re.compile(r"(?<![A-Za-z0-9_])(?:0x[0-9A-Fa-f_]+|[0-9][0-9_]*)(?:u(?:8|16|32|64|128|size)|i(?:8|16|32|64|128|size))?")
FNV_FUNCTION = re.compile(r"\b(?:const\s+)?fn\s+[A-Za-z0-9_]*fnv[A-Za-z0-9_]*\s*\(", re.IGNORECASE)
LOCAL_ROUND = re.compile(
    r"\.wrapping_mul\s*\(\s*(?:er_game_base\s*::\s*fnv1a\s*::\s*)?FNV1A64_PRIME\s*\)"
)
BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.DOTALL)
RAW_QUOTED = re.compile(r'(?s)(?:br|r)(?P<hashes>#{0,16})".*?"(?P=hashes)')
QUOTED = re.compile(r'(?s)(?:b)?"(?:\\.|[^"\\])*"')


@dataclass(frozen=True)
class Finding:
    path: str
    line: int
    reason: str


def code_only(text: str) -> str:
    """Remove comments and ordinary quoted strings while preserving line coordinates."""

    def blank(match: re.Match[str]) -> str:
        return "".join("\n" if char == "\n" else " " for char in match.group(0))

    text = BLOCK_COMMENT.sub(blank, text)
    text = RAW_QUOTED.sub(blank, text)
    text = QUOTED.sub(blank, text)
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())


def number_value(token: str) -> int:
    number = re.match(r"(?:0x[0-9A-Fa-f_]+|[0-9][0-9_]*)", token)
    assert number is not None
    raw = number.group(0).replace("_", "")
    return int(raw, 16 if raw.lower().startswith("0x") else 10)


def scan(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    crates = root / "crates"
    if not crates.is_dir():
        return findings
    for path in sorted(crates.rglob("*.rs")):
        rel = path.relative_to(root)
        if rel == OWNER or "target" in rel.parts:
            continue
        code = code_only(path.read_text(encoding="utf-8", errors="replace"))
        rel_text = rel.as_posix()
        for line_number, line in enumerate(code.splitlines(), 1):
            for match in NUMBER.finditer(line):
                value = number_value(match.group(0))
                if value == OFFSET_BASIS:
                    findings.append(Finding(rel_text, line_number, "local FNV-1a offset basis"))
                elif value == PRIME:
                    findings.append(Finding(rel_text, line_number, "local FNV-1a prime"))
        for pattern, reason in (
            (FNV_FUNCTION, "local fnv-named function"),
            (LOCAL_ROUND, "local FNV-1a multiply round"),
        ):
            for match in pattern.finditer(code):
                line_number = code.count("\n", 0, match.start()) + 1
                findings.append(Finding(rel_text, line_number, reason))
    return findings


def selftest() -> int:
    cases = [
        (
            "green shared calls",
            False,
            {"crates/a/src/lib.rs": "use er_game_base::fnv1a::{fnv1a64, fnv1a64_mix};\nfn digest(b: &[u8]) -> u64 { fnv1a64(b) }\n"},
        ),
        (
            "red copied hex implementation",
            True,
            {"crates/a/src/lib.rs": "fn hash(bytes: &[u8]) -> u64 { let mut h = 0xcbf2_9ce4_8422_2325; for b in bytes { h ^= *b as u64; h = h.wrapping_mul(0x0000_0100_0000_01b3); } h }\n"},
        ),
        (
            "red decimal constants",
            True,
            {"crates/a/src/lib.rs": f"const BASIS: u64 = {OFFSET_BASIS};\nconst PRIME: u64 = {PRIME};\n"},
        ),
        (
            "red renamed round using owner constant",
            True,
            {"crates/a/src/lib.rs": "fn digest(h: u64, b: u64) -> u64 { (h ^ b).wrapping_mul(\n    FNV1A64_PRIME\n) }\n"},
        ),
        (
            "green comments strings and owner",
            False,
            {
                "crates/a/src/lib.rs": 'const NOTE: &str = r#"0xcbf29ce484222325"#; // 0x100000001b3\n',
                OWNER.as_posix(): "pub const FNV1A64_OFFSET_BASIS: u64 = 0xcbf29ce484222325;\n",
            },
        ),
    ]
    failures: list[str] = []
    for label, should_find, files in cases:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            for rel, body in files.items():
                target = root / rel
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(body, encoding="utf-8")
            found = bool(scan(root))
            if found != should_find:
                failures.append(f"{label}: expected findings={should_find}, got {found}")
    if failures:
        for failure in failures:
            print(f"selftest FAILED: {failure}", file=sys.stderr)
        return 1
    print(f"[check-fnv1a-owner] selftest ok ({len(cases)} red/green cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    findings = scan(REPO_ROOT)
    if not findings:
        print(f"[check-fnv1a-owner] ok: {OWNER.as_posix()} is the sole Rust FNV-1a owner")
        return 0

    print("FNV-1a ownership violations. Use er_game_base::fnv1a instead:\n", file=sys.stderr)
    for finding in findings:
        print(f"  {finding.path}:{finding.line}: {finding.reason}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
