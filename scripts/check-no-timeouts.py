#!/usr/bin/env python3
"""Reject timeout/sleep-based control flow and point to deterministic fixes."""
from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
IGNORED_DIRECTORIES = {
    ".git",
    ".beads",
    ".auto",
    "target",
    "docs",
}
IGNORED_FILES = {
    Path("scripts/check-no-timeouts.py"),
    Path("scripts/test-no-timeouts.py"),
}
SOURCE_SUFFIXES = {
    ".rs",
    ".sh",
    ".bash",
    ".py",
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".mjs",
    ".cjs",
    ".yml",
    ".yaml",
}


@dataclass(frozen=True)
class Rule:
    code: str
    pattern: re.Pattern[str]
    applies_to: set[str]
    guidance: str


@dataclass(frozen=True)
class Finding:
    path: Path
    line_number: int
    rule: Rule
    line: str

    def to_json(self) -> dict[str, object]:
        return {
            "path": str(self.path),
            "line": self.line_number,
            "rule": self.rule.code,
            "source": self.line.strip(),
            "guidance": self.rule.guidance,
        }


RULES = [
    Rule(
        "shell-sleep-command",
        re.compile(r"(^|[;&|()\s])sleep(\s|$)"),
        {".sh", ".bash"},
        "Replace sleep with an observable readiness signal such as process exit, driver acknowledgement, file/event notification, or game/task-frame state.",
    ),
    Rule(
        "shell-timeout-command",
        re.compile(r"(^|[;&|()\s])timeout(\s|$)"),
        {".sh", ".bash"},
        "Remove the timeout wrapper; make the invoked helper terminate on a deterministic completion or structured failure condition.",
    ),
    Rule(
        "shell-read-timeout",
        re.compile(r"(^|[;&|()\s])read\s[^#\n;|&]*\s-t(\s|$)"),
        {".sh", ".bash"},
        "Use a deterministic input/event source instead of read -t.",
    ),
    Rule(
        "rust-thread-sleep",
        re.compile(r"\b(?:std::)?thread::sleep\s*\("),
        {".rs"},
        "Replace thread sleep with a readiness/event handshake, task-frame callback, channel receive, or explicit driver acknowledgement.",
    ),
    Rule(
        "rust-async-sleep-or-timeout",
        re.compile(r"\btokio::time::(?:sleep|timeout)\s*\("),
        {".rs"},
        "Use deterministic async completion, cancellation from an observed state, or a channel/event instead of tokio time gates.",
    ),
    Rule(
        "rust-elapsed-deadline",
        re.compile(r"\.elapsed\s*\(\s*\)\s*[<>]=?\s*"),
        {".rs"},
        "Replace elapsed wall-clock gates with explicit state transitions, frame counters, or structured failure states.",
    ),
    Rule(
        "rust-duration-max",
        re.compile(r"\b(?:std::time::)?Duration::MAX\b"),
        {".rs"},
        "Do not pass an infinite timeout sentinel; expose or call a no-timeout/no-deadline readiness API instead.",
    ),
    Rule(
        "rust-timeout-wait-api",
        re.compile(r"\b(?:wait_for_system_init|wait_for_instance)\s*\("),
        {".rs"},
        "Avoid timeout-shaped wait APIs; use a deterministic readiness helper that has no timeout parameter.",
    ),
    Rule(
        "python-sleep-or-wait-for",
        re.compile(r"\b(?:time\.sleep|asyncio\.sleep|asyncio\.wait_for)\s*\("),
        {".py"},
        "Use deterministic process/event/file readiness primitives instead of Python sleep or wait_for.",
    ),
    Rule(
        "python-timeout-argument",
        re.compile(r"\btimeout\s*="),
        {".py"},
        "Avoid timeout keyword arguments; drive the operation from an observable completion or failure condition.",
    ),
    Rule(
        "js-timer-api",
        re.compile(r"\b(?:setTimeout|setInterval|AbortSignal\.timeout)\s*\("),
        {".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"},
        "Replace timer APIs with explicit events, promises resolved by real readiness, or deterministic test hooks.",
    ),
    Rule(
        "yaml-timeout-minutes",
        re.compile(r"(^|\s)timeout-minutes\s*:"),
        {".yml", ".yaml"},
        "Do not encode CI timeouts; make the invoked job/check exit on deterministic completion or structured failure.",
    ),
]


def strip_line_for_suffix(line: str, suffix: str) -> str:
    stripped = line.lstrip()
    if suffix in {".sh", ".bash", ".py", ".yml", ".yaml"} and stripped.startswith("#"):
        return ""
    if suffix == ".rs" and stripped.startswith("//"):
        return ""
    if suffix in {".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"} and stripped.startswith("//"):
        return ""
    return line


def source_files() -> list[Path]:
    paths: list[Path] = []
    for path in REPO_ROOT.rglob("*"):
        if not path.is_file():
            continue
        relative = path.relative_to(REPO_ROOT)
        if relative in IGNORED_FILES:
            continue
        if any(part in IGNORED_DIRECTORIES for part in relative.parts):
            continue
        if path.suffix in SOURCE_SUFFIXES:
            paths.append(path)
    return sorted(paths)


def scan_file(path: Path) -> list[Finding]:
    findings: list[Finding] = []
    relative = path.relative_to(REPO_ROOT)
    suffix = path.suffix
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise SystemExit(f"failed to decode {relative}: {error}") from error

    for line_number, line in enumerate(lines, start=1):
        searchable = strip_line_for_suffix(line, suffix)
        if not searchable:
            continue
        for rule in RULES:
            if suffix not in rule.applies_to:
                continue
            if rule.pattern.search(searchable):
                findings.append(Finding(relative, line_number, rule, line))
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="Emit machine-readable findings.")
    args = parser.parse_args()

    findings = [finding for path in source_files() for finding in scan_file(path)]
    if args.json:
        json.dump([finding.to_json() for finding in findings], sys.stdout, indent=2)
        sys.stdout.write("\n")
    elif findings:
        print("Timeout/sleep-based control flow is banned.", file=sys.stderr)
        print(
            "Use deterministic readiness instead: event files, process exit, inotify/file changes, explicit driver acknowledgements, game/task-frame state, channels, or structured failure states.\n",
            file=sys.stderr,
        )
        for finding in findings:
            print(
                f"{finding.path}:{finding.line_number}: {finding.rule.code}: {finding.line.strip()}",
                file=sys.stderr,
            )
            print(f"  fix: {finding.rule.guidance}", file=sys.stderr)
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
