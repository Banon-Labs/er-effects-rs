#!/usr/bin/env python3
"""Reject sleeps and unbounded/over-30-second timeout control flow."""
from __future__ import annotations

import argparse
import ast
import json
import os
import re
import shlex
import subprocess
import sys
from collections.abc import Iterator
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
    # Host-side VM GUI-automation tools -- Not runtime probes. Keystroke pacing and
    # display-wake waits are inherent to driving a Windows guest via `virsh send-key`
    # / `virsh screenshot`; there is no readiness primitive for "the guest input queue
    # drained" or "the display woke". The no-sleep rule targets runtime probes.
    Path("scripts/vm-sendkeys.py"),
    Path("scripts/vanilla-control-probe.py"),
    # Per-DLL runtime sweep. Its `time.sleep` is the watch window -- the measurement itself --
    # not synchronization: the question it answers is "is this DLL's thread-group leader still
    # alive N seconds after launch", and N is the datum. Its stop conditions are real semaphores
    # (/proc leader state `Z`, or the process disappearing), never a timer; the window only bounds
    # how long a surviving game is watched. There is no readiness primitive for "nothing has gone
    # wrong yet", which is precisely what a boot verdict asserts.
    Path("scripts/sweep-dll-1170-runtime.py"),
    # Sampling profiler / flight recorder, not a runtime probe that waits on a readiness
    # signal. Its `time.sleep` is the sample period -- the measurement instrument itself --
    # not synchronization: the tool exists to record a thread's state at a fixed rate right
    # up to the instant that thread dies, and its stop condition is that observed death (a
    # real semaphore: /proc state Z or the task disappearing), never a timer. There is no
    # readiness primitive that can replace a sampling rate, and the event-driven alternative
    # (ptrace stops) is exactly what this tier avoids so it cannot perturb the game.
    Path("scripts/wine-thread-death-watch.py"),
    # Steam lobby probes. Their `time.sleep` is a sample period, for the same reason as the
    # profiler above: `RequestLobbyList` results arrive through a callback that lives inside
    # ersc.dll's Themida-virtualised region, so there is nothing to hook for "results are ready"
    # and no readiness primitive exists to replace re-reading at a fixed rate. Both stop on a real
    # observation -- a member arriving, or the oracle document differing from its previous
    # contents -- and their deadlines are backstops, never the synchronisation.
    Path("scripts/frida-lobby-watch-members.py"),
    Path("scripts/frida-hunt-drive-query.py"),
    # Stack sampler for a wedged game, same class as wine-thread-death-watch.py above: its
    # `time.sleep` is the CPU sample period, and its stop condition is an observed state (the
    # process flatlining, or its leader going Z), never a timer. There is no readiness primitive
    # for "the game has stopped doing work" -- measuring whether it has is the tool.
    Path("scripts/er-wedge-stacks.py"),
    # Frida agent, running inside the game as JavaScript. Its `setTimeout` re-reads
    # `session+0x150` waiting for Seamless's session to fall back to `0x01` idle, which is what
    # re-arms the hunt after a match this mod did not end.
    #
    # There is nothing to hook for that transition. The two ersc.dll entry points this repo has
    # reversed write the other two states -- `+0x25850` invade writes `0x0e`, `+0x258d0` cancel
    # writes `0x23` -- and both are already attached to in this same file, which covers every idle
    # this agent or the player causes. The case the watchdog exists for is the one neither does:
    # Seamless ending its own attempt ("Failed to invade. Could not invade host of session",
    # measured 2026-09-08) and returning the session to idle from code with no identified writer,
    # the same wall the two frida lobby probes above ran into. Reading the word is the only
    # detector, and inside Frida's JS runtime `setTimeout` is the only non-blocking way to read it
    # again -- `Thread.sleep` would block the agent thread, which is strictly worse.
    #
    # Its sibling `scripts/frida/drive-cancel-now.js` is deliberately not listed: the state it
    # waited for is `0x0e`, whose sole writer is the invade action, so that one became an
    # `Interceptor` and needs no exemption.
    Path("scripts/frida/ersc-session.js"),
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
# Bounded safety cap for the `git ls-files` enumeration below (literal-constant, <= the hard cap so this
# checker satisfies its own subprocess-timeout rule).
_GIT_LS_FILES_TIMEOUT_SECONDS = 10.0
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


# Files allowed to call `thread::yield_now()` directly, and why each one is not the hazard the
# `rust-unbounded-yield-spin` rule exists to catch.
YIELD_SPIN_ALLOWED = {
    # The helper itself: one yield per backed-off round is the fix.
    Path("crates/er-game-base/src/wait.rs"),
    # Already bounded by an `Instant` deadline a few milliseconds out, not by hope.
    Path("crates/er-loading-portrait-core/src/portrait_shared.rs"),
    Path("crates/er-quickload/src/experiments/startup_hooks/quit_menu/save_swap_profile_table.rs"),
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
        "rust-unbounded-yield-spin",
        re.compile(r"\b(?:std::)?thread::yield_now\s*\(\s*\)"),
        {".rs"},
        "Use er_game_base::wait::poll_until (bounded) or ::back_off (user-space spin between "
        "yields). A bare yield_now() per attempt is a wineserver round trip per attempt: on "
        "2026-08-29 two such loops starved the wineserver and the game managed 104 CPU ticks in "
        "three minutes -- no window, no crash, nothing in any log.",
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


def tracked_relative_paths() -> set[Path] | None:
    """Repo-relative paths git tracks (index + committed), or None when git is unavailable.

    Untracked scratch files (git status ??) are -- exactly like the gitignored `.worktrees` sandboxes
    exempted above -- not part of the committed tree, so their sleep/timeout state must not gate a commit
    of tracked files. `git ls-files` reports the index, so a staged-but-uncommitted (about-to-be-committed)
    file is still gated; only pure `??` scratch is skipped. If git is unavailable, return None so the
    caller fails open and scans everything -- the gate must never silently disable itself.
    """
    try:
        result = subprocess.run(
            ["git", "-C", str(REPO_ROOT), "ls-files", "-z"],
            capture_output=True,
            check=True,
            timeout=_GIT_LS_FILES_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    names = result.stdout.decode("utf-8", errors="strict").split("\0")
    return {Path(name) for name in names if name}


def candidate_relative_paths(tracked: set[Path] | None) -> Iterator[Path]:
    """Repo-relative paths to filter, taken from the cheapest source that is complete.

    When git answered, the tracked set is the answer: every path this gate can possibly scan is a
    tracked one (the untracked-scratch filter below used to discard the rest anyway), so walking the
    filesystem to rediscover them is pure waste. Measured 2026-08-31 at loadavg ~9 on 16 cores:
    `REPO_ROOT.rglob("*")` enumerates 1,117,583 entries and `source_files()` took 69.2s, against
    2.1s for the scan it feeds -- 97% of this gate's wall clock spent walking `.worktrees`, `.claude`
    and `target` to find 1112 files that `git ls-files` lists in 0.011s. That is the same defect
    `audit-fromsoft-candidates.py` was found to have on the same day.

    When git is unavailable, fall back to the filesystem walk so the gate fails open and still scans
    everything -- it must never silently narrow itself just because git is missing. That walk PRUNES
    the ignored directories as it descends rather than filtering their contents afterwards, which is
    what `rglob` forced. Pruning cannot change the result: an entry under an ignored directory has
    that directory in `relative.parts` and was already discarded. It is purely the difference between
    reading `target/` and `.worktrees/` and then throwing them away, and never reading them at all.
    """
    if tracked is not None:
        return iter(tracked)
    return walk_untracked_fallback()


def walk_untracked_fallback() -> Iterator[Path]:
    for directory, subdirectories, filenames in os.walk(REPO_ROOT):
        subdirectories[:] = [name for name in subdirectories if name not in IGNORED_DIRECTORIES]
        base = Path(directory)
        for name in filenames:
            yield (base / name).relative_to(REPO_ROOT)


def source_files() -> list[Path]:
    tracked = tracked_relative_paths()
    paths: list[Path] = []
    for relative in candidate_relative_paths(tracked):
        # Suffix first: it is a pure string test, so on the git-less fallback path it rejects the
        # overwhelming majority of entries before they cost a stat() syscall.
        if relative.suffix not in SOURCE_SUFFIXES:
            continue
        if relative in IGNORED_FILES:
            continue
        if any(part in IGNORED_DIRECTORIES for part in relative.parts):
            continue
        path = REPO_ROOT / relative
        # `git ls-files` reports the index, so a tracked path can be absent from the working tree
        # (deleted but not yet committed); a broken symlink reaches here from either source. Both
        # were excluded by the old walk's `is_file()` and are excluded by this one.
        if not path.is_file():
            continue
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
    """Detect a `read -t` timeout flag by token, not substring.

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
            if rule.code == "rust-unbounded-yield-spin" and relative in YIELD_SPIN_ALLOWED:
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
