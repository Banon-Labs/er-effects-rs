#!/usr/bin/env python3
"""Keep the in-memory CS::ProfileSummary ABI under one deep owner.

The canonical typed layout lives in `crates/er-game-base/src/profile_summary.rs`. This gate rejects
reintroduced numeric offset declarations, lookalike layout structs, legacy base/stride formulas, and
direct field-literal reads elsewhere in Rust source. The similarly named `USER_DATA010` layout in
`er-save-loader` is an on-disk format with a different stride and is intentionally outside this rule.

Usage:
    python3 scripts/check-profile-summary-layout.py
    python3 scripts/check-profile-summary-layout.py --selftest
"""

from __future__ import annotations

import argparse
import re
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CANONICAL = Path("crates/er-game-base/src/profile_summary.rs")

LAYOUT_VALUES = {
    0x08,
    0x18,
    0x22,
    0x24,
    0x28,
    0x2C,
    0x30,
    0x34,
    0x38,
    0x1A8,
    0x290,
    0x291,
    0x292,
    0x293,
    0x294,
    0x2A0,
    0x1A58,
}
LAYOUT_NAME = re.compile(
    r"^(?:PROFILE_SUMMARY_(?:"
    r"ACTIVE_FLAGS_OFFSET|SLOT_DATA_OFFSET|RECORD_BASE|SLOT_STRIDE|RECORD_STRIDE|TOTAL_BYTES|"
    r"NAME_BYTES|LEVEL_OFFSET|PLAYTIME_OFFSET|PLAY_TIME_OFFSET|RUNE_MEMORY_OFFSET|MAP_OFFSET|"
    r"PLACE_NAME_OFFSET|FACE_DATA_OFFSET|CHR_ASM_OFFSET|GENDER_OFFSET|ARCHETYPE_OFFSET|"
    r"STARTING_GIFT_OFFSET|FIELD_C4_OFFSET|FIELD_294_OFFSET"
    r")|PROFILE_RECORD_(?:BASE|STRIDE|LEVEL_OFFSET|MAP_OFFSET))$"
)
CONST_DECL = re.compile(
    r"\b(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Z][A-Z0-9_]*)\s*:\s*[^=;]+="
    r"\s*(0x[0-9a-fA-F_]+)\b"
)
LAYOUT_STRUCT = re.compile(
    r"\bstruct\s+(?:GameDataMan)?ProfileSummary(?:Record|Layout)\b"
)
LEGACY_RECORD_FORMULA = re.compile(
    r"PROFILE_SUMMARY_RECORD_BASE\s*\+.{0,160}?PROFILE_SUMMARY_RECORD_STRIDE|"
    r"PROFILE_RECORD_BASE\s*\+.{0,160}?PROFILE_RECORD_STRIDE",
    re.S,
)
DIRECT_SUMMARY_BASE = re.compile(r"\b(?:profile_summary|summary)\s*\+\s*0x18\b")
DIRECT_RECORD_FIELD = re.compile(
    r"\b(?:record|rec|slot_data)\s*(?:\+\s*|\.wrapping_add\(\s*)"
    r"0x(?:24|28|2c|30|34|38|1a8|290|291|292|293|294)\b"
)


@dataclass(frozen=True)
class Violation:
    path: str
    line: int
    reason: str


def source_without_comments_or_strings(source: str) -> str:
    """Blank comments and strings while preserving newlines for line attribution."""
    token = re.compile(
        r"//[^\n]*|/\*.*?\*/|r#*\".*?\"#*|\"(?:\\.|[^\"\\])*\"",
        re.S,
    )

    def blank(match: re.Match[str]) -> str:
        text = match.group(0)
        return "".join("\n" if char == "\n" else " " for char in text)

    return token.sub(blank, source)


def line_at(source: str, offset: int) -> int:
    return source.count("\n", 0, offset) + 1


def scan(root: Path) -> list[Violation]:
    violations: list[Violation] = []
    crates = root / "crates"
    if not crates.exists():
        return violations
    for path in sorted(crates.rglob("*.rs")):
        if "target" in path.parts:
            continue
        rel = path.relative_to(root)
        if rel == CANONICAL:
            continue
        source = source_without_comments_or_strings(
            path.read_text(encoding="utf-8", errors="replace")
        )
        rel_text = rel.as_posix()
        for match in CONST_DECL.finditer(source):
            name = match.group(1)
            value = int(match.group(2).replace("_", ""), 16)
            renamed_layout_fact = name.startswith(("PROFILE_SUMMARY_", "PROFILE_RECORD_")) and (
                value in LAYOUT_VALUES
            )
            if LAYOUT_NAME.match(name) or renamed_layout_fact:
                violations.append(
                    Violation(
                        rel_text,
                        line_at(source, match.start()),
                        f"literal layout declaration {name}={match.group(2)}",
                    )
                )
        for pattern, reason in (
            (LAYOUT_STRUCT, "duplicate ProfileSummary layout struct"),
            (LEGACY_RECORD_FORMULA, "duplicate ProfileSummary base/stride formula"),
            (DIRECT_SUMMARY_BASE, "direct ProfileSummary record-base literal"),
            (DIRECT_RECORD_FIELD, "direct ProfileSummary record-field literal"),
        ):
            for match in pattern.finditer(source):
                violations.append(Violation(rel_text, line_at(source, match.start()), reason))
    return sorted(violations, key=lambda item: (item.path, item.line, item.reason))


def selftest() -> int:
    cases = [
        (
            "red: duplicate literal declaration",
            {"crates/feature/src/lib.rs": "const PROFILE_RECORD_STRIDE: usize = 0x2a0;\n"},
            1,
        ),
        (
            "red: renamed duplicate literal declaration",
            {"crates/feature/src/lib.rs": "const PROFILE_SUMMARY_LEVEL_FIELD: usize = 0x24;\n"},
            1,
        ),
        (
            "red: duplicate typed layout",
            {"crates/feature/src/lib.rs": "pub struct ProfileSummaryLayout { bytes: [u8; 4] }\n"},
            1,
        ),
        (
            "red: legacy base/stride reader",
            {
                "crates/feature/src/lib.rs": (
                    "let rec = summary + PROFILE_SUMMARY_RECORD_BASE "
                    "+ slot * PROFILE_SUMMARY_RECORD_STRIDE;\n"
                )
            },
            1,
        ),
        (
            "red: direct field literal reader",
            {"crates/feature/src/lib.rs": "let map = safe_read(record + 0x30);\n"},
            1,
        ),
        (
            "green: canonical aliases and helper",
            {
                "crates/feature/src/lib.rs": (
                    "const SLOT_MANAGER_CONTAINER_OFFSET: usize = "
                    "GAME_DATA_MAN_PROFILE_SUMMARY_OFFSET;\n"
                    "let rec = profile_summary_record_address(summary, slot).unwrap();\n"
                )
            },
            0,
        ),
        (
            "green: distinct on-disk profile summary",
            {
                "crates/er-save-loader/src/profile_summary.rs": (
                    "const SUMMARY_RECORD_STRIDE: usize = 0x24c;\n"
                    "const REC_BLOCK_ID_OFFSET: usize = 0x30;\n"
                )
            },
            0,
        ),
        (
            "green: prose examples are not code",
            {
                "crates/feature/src/lib.rs": (
                    "// let rec = summary + 0x18; record + 0x30\n"
                    "const NOTE: &str = \"PROFILE_RECORD_BASE + PROFILE_RECORD_STRIDE\";\n"
                )
            },
            0,
        ),
    ]
    failures: list[str] = []
    for label, files, expected in cases:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for relative, body in files.items():
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(body, encoding="utf-8")
            found = scan(root)
            if len(found) != expected:
                failures.append(f"{label}: expected {expected}, found {len(found)}: {found}")
    if failures:
        for failure in failures:
            print(f"selftest FAILED: {failure}", file=sys.stderr)
        return 1
    print(f"[check-profile-summary-layout] selftest ok ({len(cases)} red/green cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    violations = scan(REPO_ROOT)
    if not violations:
        print("[check-profile-summary-layout] ok: one typed owner, no duplicate layout readers")
        return 0

    print(
        "[check-profile-summary-layout] duplicate in-memory ProfileSummary layout facts found.\n"
        f"Use {CANONICAL.as_posix()} constants/helpers instead:\n",
        file=sys.stderr,
    )
    for violation in violations:
        print(
            f"  {violation.path}:{violation.line}: {violation.reason}",
            file=sys.stderr,
        )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
