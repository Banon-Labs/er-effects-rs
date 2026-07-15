#!/usr/bin/env python3
"""Reject sleeps and unbounded/over-30-second timeout control flow."""
from __future__ import annotations

import argparse
import ast
import json
import re
import shlex
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
    # Gitignored local worktrees/sandboxes (AGENTS.md "Local Hidden Worktrees"): they are not part
    # of the committed tree, so their sleep/timeout state must not gate a commit of tracked files.
    ".worktrees",
}
IGNORED_FILES = {
    Path("scripts/check-no-timeouts.py"),
    Path("scripts/test-no-timeouts.py"),
    # Host-side VM GUI-automation tools -- NOT runtime probes. Keystroke pacing and
    # display-wake waits are inherent to driving a Windows guest via `virsh send-key`
    # / `virsh screenshot`; there is no readiness primitive for "the guest input queue
    # drained" or "the display woke". The no-sleep rule targets runtime probes.
    Path("scripts/vm-sendkeys.py"),
    Path("scripts/vanilla-control-probe.py"),
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
MAX_TIMEOUT_SECONDS = 30.0
SHELL_DURATION_UNITS = {
    "": 1.0,
    "s": 1.0,
    "m": 60.0,
    "h": 3600.0,
    "d": 86400.0,
}
# subprocess.Popen is intentionally excluded: it spawns a long-lived process handle and accepts no
# `timeout=` kwarg (timeouts belong on the subsequent `.communicate()`/`.wait()` call). Requiring a
# timeout on the Popen() call itself is impossible to satisfy, so flagging it is a false positive.
PYTHON_SUBPROCESS_FUNCTIONS = {
    "run",
    "check_call",
    "check_output",
    "call",
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
        "rust-thread-sleep",
        re.compile(r"\b(?:std::)?thread::sleep\s*\("),
        {".rs"},
        "Replace thread sleep with a readiness/event handshake, task-frame callback, channel receive, or explicit driver acknowledgement.",
    ),
    Rule(
        "rust-async-sleep",
        re.compile(r"\btokio::time::sleep\s*\("),
        {".rs"},
        "Use deterministic async completion, cancellation from an observed state, or a channel/event instead of sleeps.",
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
        "js-timer-api",
        re.compile(r"\b(?:setTimeout|setInterval|AbortSignal\.timeout)\s*\("),
        {".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs"},
        "Replace timer APIs with explicit events, promises resolved by real readiness, or deterministic test hooks.",
    ),
]

SHELL_TIMEOUT_RULE = Rule(
    "shell-timeout-unbounded-or-over-30",
    re.compile(r"$^"),
    {".sh", ".bash"},
    "Use timeout/read -t only with an explicit numeric duration greater than 0 and no more than 30 seconds.",
)
PYTHON_SUBPROCESS_MISSING_TIMEOUT_RULE = Rule(
    "python-subprocess-missing-timeout",
    re.compile(r"$^"),
    {".py"},
    "Every subprocess call must include an explicit timeout no greater than 30 seconds.",
)
PYTHON_SUBPROCESS_UNBOUNDED_TIMEOUT_RULE = Rule(
    "python-subprocess-unbounded-or-over-30-timeout",
    re.compile(r"$^"),
    {".py"},
    "Use a literal or module constant timeout greater than 0 and no more than 30 seconds.",
)
YAML_TIMEOUT_MINUTES_RULE = Rule(
    "yaml-timeout-minutes-over-30-seconds",
    re.compile(r"$^"),
    {".yml", ".yaml"},
    "timeout-minutes cannot express the 30-second hard cap; use a repo helper with a <=30 second timeout instead.",
)


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


def line_text(lines: list[str], line_number: int) -> str:
    if 1 <= line_number <= len(lines):
        return lines[line_number - 1]
    return ""


def parse_shell_duration_seconds(token: str) -> float | None:
    value = token.strip().strip("'\"")
    match = re.fullmatch(r"([0-9]+(?:\.[0-9]+)?)([smhd]?)", value)
    if match is None:
        return None
    magnitude = float(match.group(1))
    unit = match.group(2)
    return magnitude * SHELL_DURATION_UNITS[unit]


def bounded_seconds(value: float | None) -> bool:
    return value is not None and 0 < value <= MAX_TIMEOUT_SECONDS


def shell_timeout_duration(tokens: list[str], timeout_index: int) -> float | None:
    index = timeout_index + 1
    while index < len(tokens) and tokens[index].startswith("-"):
        option = tokens[index]
        index += 1
        if option in {"-k", "--kill-after"} and index < len(tokens):
            index += 1
    if index >= len(tokens):
        return None
    return parse_shell_duration_seconds(tokens[index])


def shell_read_timeout(tokens: list[str], read_index: int) -> tuple[bool, float | None]:
    """Detect a `read -t` timeout flag by TOKEN, not substring.

    Returns (has_timeout_flag, duration_seconds_or_None). A bare `read -r f` has no `-t` flag, so it
    must not be flagged even when the surrounding line contains a `-t` substring (e.g. `find -type f`).
    Handles `-t N`, `-tN`, and single-dash clusters ending in `t` (`-rt N`) or carrying an inline value
    (`-rtN`).
    """
    index = read_index + 1
    while index < len(tokens):
        token = tokens[index]
        if token == "-t" or re.fullmatch(r"-[a-z]*t", token):
            nxt = tokens[index + 1] if index + 1 < len(tokens) else None
            return True, (parse_shell_duration_seconds(nxt) if nxt is not None else None)
        match = re.fullmatch(r"-[a-z]*t([0-9.]+)", token)
        if match:
            return True, parse_shell_duration_seconds(match.group(1))
        index += 1
    return False, None


def scan_shell_bounded_timeouts(relative: Path, lines: list[str], suffix: str) -> list[Finding]:
    if suffix not in {".sh", ".bash"}:
        return []
    findings: list[Finding] = []
    for line_number, line in enumerate(lines, start=1):
        searchable = strip_line_for_suffix(line, suffix)
        if not searchable:
            continue
        try:
            tokens = shlex.split(searchable, comments=True, posix=True)
        except ValueError:
            tokens = searchable.split()
        for index, token in enumerate(tokens):
            if token == "timeout" and not bounded_seconds(shell_timeout_duration(tokens, index)):
                findings.append(Finding(relative, line_number, SHELL_TIMEOUT_RULE, line))
            if token == "read":
                has_timeout, duration = shell_read_timeout(tokens, index)
                if has_timeout and not bounded_seconds(duration):
                    findings.append(Finding(relative, line_number, SHELL_TIMEOUT_RULE, line))
    return findings


def numeric_constant_value(node: ast.AST) -> float | None:
    if isinstance(node, ast.Constant) and isinstance(node.value, int | float):
        return float(node.value)
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
        operand = numeric_constant_value(node.operand)
        return -operand if operand is not None else None
    return None


def module_numeric_constants(tree: ast.AST) -> dict[str, float]:
    constants: dict[str, float] = {}
    for node in getattr(tree, "body", []):
        if isinstance(node, ast.Assign):
            value = numeric_constant_value(node.value)
            if value is None:
                continue
            for target in node.targets:
                if isinstance(target, ast.Name) and target.id.isupper():
                    constants[target.id] = value
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            value = numeric_constant_value(node.value) if node.value is not None else None
            if value is not None and node.target.id.isupper():
                constants[node.target.id] = value
    return constants


def subprocess_attr_name(node: ast.AST) -> str | None:
    if isinstance(node, ast.Attribute) and isinstance(node.value, ast.Name):
        if node.value.id == "subprocess" and node.attr in PYTHON_SUBPROCESS_FUNCTIONS:
            return node.attr
    return None


def timeout_keyword_value(node: ast.Call) -> ast.AST | None:
    for keyword in node.keywords:
        if keyword.arg == "timeout":
            return keyword.value
    return None


def bounded_python_timeout(value_node: ast.AST, constants: dict[str, float]) -> bool:
    value = numeric_constant_value(value_node)
    if value is None and isinstance(value_node, ast.Name):
        value = constants.get(value_node.id)
    return bounded_seconds(value)


def scan_python_subprocess_timeouts(relative: Path, text: str, lines: list[str]) -> list[Finding]:
    if relative.suffix != ".py":
        return []
    try:
        tree = ast.parse(text, filename=str(relative))
    except SyntaxError:
        return []
    constants = module_numeric_constants(tree)
    findings: list[Finding] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        if subprocess_attr_name(node.func) is None:
            continue
        timeout_value = timeout_keyword_value(node)
        line = line_text(lines, node.lineno)
        if timeout_value is None:
            findings.append(Finding(relative, node.lineno, PYTHON_SUBPROCESS_MISSING_TIMEOUT_RULE, line))
        elif not bounded_python_timeout(timeout_value, constants):
            findings.append(Finding(relative, node.lineno, PYTHON_SUBPROCESS_UNBOUNDED_TIMEOUT_RULE, line))
    return findings


def scan_yaml_timeouts(relative: Path, lines: list[str], suffix: str) -> list[Finding]:
    if suffix not in {".yml", ".yaml"}:
        return []
    findings: list[Finding] = []
    for line_number, line in enumerate(lines, start=1):
        searchable = strip_line_for_suffix(line, suffix)
        if re.search(r"(^|\s)timeout-minutes\s*:", searchable):
            findings.append(Finding(relative, line_number, YAML_TIMEOUT_MINUTES_RULE, line))
    return findings


def scan_file(path: Path) -> list[Finding]:
    findings: list[Finding] = []
    relative = path.relative_to(REPO_ROOT)
    suffix = path.suffix
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError as error:
        raise SystemExit(f"failed to decode {relative}: {error}") from error
    lines = text.splitlines()

    for line_number, line in enumerate(lines, start=1):
        searchable = strip_line_for_suffix(line, suffix)
        if not searchable:
            continue
        for rule in RULES:
            if suffix not in rule.applies_to:
                continue
            if rule.pattern.search(searchable):
                findings.append(Finding(relative, line_number, rule, line))
    findings.extend(scan_shell_bounded_timeouts(relative, lines, suffix))
    findings.extend(scan_python_subprocess_timeouts(relative, text, lines))
    findings.extend(scan_yaml_timeouts(relative, lines, suffix))
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
        print("Sleep or unbounded/over-30-second timeout control flow is banned.", file=sys.stderr)
        print(
            "Use explicit timeouts of 30 seconds or less as hard safety caps, and prefer deterministic readiness inside that cap: event files, process exit, inotify/file changes, explicit driver acknowledgements, game/task-frame state, channels, or structured failure states.\n",
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
