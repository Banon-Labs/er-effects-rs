#!/usr/bin/env python3
"""Windows thread-suspension primitives must not be reachable from a default code path.

Why this gate exists, measured
------------------------------
Three boots of the 21-DLL profile wedged (bd `er-effects-rs-1742`), each with the last log
line being er-hook's `HOOK TRANSLATED` -- the statement immediately before `MH_CreateHook`.
Diagnosed 2026-09-08: `crates/er-telemetry-core/src/lib.rs` spawned an ungated
`er-cpu-sampler` thread out of `profiler::note_frame` that called `SuspendThread` plus
`GetThreadContext` on the game thread roughly once per millisecond for as long as frames
were slow -- and boot is the longest run of slow frames there is. Meanwhile each of the 21
shells installs its detours through its own statically linked MinHook, whose `Freeze()`
suspends every other thread and rewrites their `RIP` through `Get`/`SetThreadContext`. Two
mutually unaware suspenders, one of them firing continuously, is the deadlock.

Why a static gate is the only enforcement available
---------------------------------------------------
`er-hook`'s `freeze_guard` serialises MinHook's four entry points and nothing else, so it
cannot help a suspender that is not MinHook. Its 5s timeout can never fire either: a thread
waiting in `hold()` that another module's `Freeze` suspends cannot return from
`WaitForSingleObject`, so the timeout is measured while the waiter is frozen. There is no
runtime backstop left. This gate is it.

The invariant
-------------
Every call site of a primitive listed in `PRIMITIVES` must be one of:

  (a) reachable only behind an explicit opt-in -- an env var or marker-file predicate such as
      `er_boot_profiler::profiler_rip_enabled()`. The predicate may sit in the calling
      function or up to `MAX_CALLER_DEPTH` callers above it, and every caller on every path
      must be gated, not merely one of them;
  (b) holding the cross-DLL freeze lock (`freeze_guard::hold`, the named mutex
      `Local\\er-mods-rs-minhook-freeze-<pid>` in `crates/er-hook/src/lib.rs`);
  (c) listed in `scripts/thread-suspension.baseline.json` with a one-line reason, so the
      exemption is in the diff and reviewable. A baselined site must also carry the
      `Thread suspension:` justification in a comment on or above its function, so the next
      reader sees the reason at the code rather than only in a json file.

The approved predicates for (a) and (b) are the `gates` table of that same baseline file:
a new opt-in switch is a reviewed addition there, not something a call site can assert
about itself.

What this gate reads
--------------------
Rust source only, with comments, string literal bodies, `use` statements and `#[cfg(test)]`
modules removed first, so a primitive named in prose, in a log message, in an import list or
in a `extern` declaration block is not a call site. Function nesting comes from brace
counting over that stripped text.

Usage:
    python3 scripts/check-no-thread-suspension.py
    python3 scripts/check-no-thread-suspension.py --selftest
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from repo_source_scan import REPO_ROOT, rust_source_files  # noqa: E402

PRIMITIVES = (
    "SuspendThread",
    "ResumeThread",
    "SetThreadContext",
    "GetThreadContext",
    "CreateToolhelp32Snapshot",
    "Thread32First",
    "Thread32Next",
)

BASELINE_PATH = REPO_ROOT / "scripts" / "thread-suspension.baseline.json"

JUSTIFICATION_MARKER = "Thread suspension:"

# How far above a call site a gate predicate may sit. Three is what the tree needs: the
# deepest real chain is `arm_c30_watchpoint` -> `maybe_arm_c30_watch` -> the game task tick
# that tests `c30_watch_enabled()`. Raising it would let a gate sit arbitrarily far from the
# suspension it is supposed to govern, which is how the telemetry sampler stayed invisible.
MAX_CALLER_DEPTH = 3

PRIMITIVE_RE = re.compile(r"\b(" + "|".join(PRIMITIVES) + r")\b")
DECLARATION_RE = re.compile(r"\bfn\s+(?:" + "|".join(PRIMITIVES) + r")\s*[(<]")
FN_RE = re.compile(r"\bfn\s+([A-Za-z_][A-Za-z0-9_]*)")
CFG_TEST_RE = re.compile(r"#\[cfg\((?:test\)|all\(test\b)")
IDENT_CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_"


def split_code_and_comments(text: str) -> tuple[str, str]:
    """Return (code, comments), both the same length as `text` and newline-aligned with it.

    In `code`, comment bodies and string/char literal bodies are blanked. Blanking the
    literals is what makes brace counting survive `format!("{name}")`, and it is also why a
    primitive spelled inside a log message is never read as a call.
    """
    code = list(text)
    comments = ["\n" if character == "\n" else " " for character in text]
    length = len(text)
    index = 0

    def blank_code(start: int, stop: int, into_comment: bool) -> None:
        for position in range(start, stop):
            if text[position] == "\n":
                continue
            if into_comment:
                comments[position] = code[position]
            code[position] = " "

    while index < length:
        character = text[index]
        if character == '"' or (
            character == "r" and _raw_string_quote(text, index) is not None
        ):
            index = _skip_string(text, index, blank_code)
            continue
        if character == "'" and _is_char_literal(text, index):
            stop = index + (4 if text[index + 1] == "\\" else 3)
            blank_code(index + 1, min(stop - 1, length), False)
            index = min(stop, length)
            continue
        if text.startswith("//", index):
            stop = text.find("\n", index)
            stop = length if stop < 0 else stop
            blank_code(index, stop, True)
            index = stop
            continue
        if text.startswith("/*", index):
            depth = 1
            cursor = index + 2
            while cursor < length and depth:
                if text.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif text.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            blank_code(index, cursor, True)
            index = cursor
            continue
        index += 1

    return "".join(code), "".join(comments)


def _raw_string_quote(text: str, index: int) -> int | None:
    """Offset of the opening quote of a raw string starting at `index`, or None."""
    cursor = index + 1
    while cursor < len(text) and text[cursor] == "#":
        cursor += 1
    if cursor < len(text) and text[cursor] == '"':
        return cursor
    return None


def _is_char_literal(text: str, index: int) -> bool:
    """Distinguish `'x'` and `'\\n'` from a lifetime such as `'a`."""
    if index + 2 >= len(text):
        return False
    if text[index + 1] == "\\":
        return True
    return text[index + 2] == "'"


def _skip_string(text: str, index: int, blank_code) -> int:
    length = len(text)
    if text[index] == "r":
        quote = _raw_string_quote(text, index)
        hashes = quote - index - 1
        closer = '"' + "#" * hashes
        stop = text.find(closer, quote + 1)
        stop = length if stop < 0 else stop + len(closer)
        blank_code(quote + 1, max(quote + 1, stop - len(closer)), False)
        return stop
    cursor = index + 1
    while cursor < length:
        if text[cursor] == "\\":
            cursor += 2
            continue
        if text[cursor] == '"':
            cursor += 1
            break
        cursor += 1
    blank_code(index + 1, max(index + 1, cursor - 1), False)
    return cursor


class FileIndex:
    """One Rust file, parsed into the few facts this gate asks about."""

    def __init__(self, path: Path, root: Path) -> None:
        self.path = path
        self.rel = path.relative_to(root).as_posix()
        text = path.read_text(encoding="utf-8", errors="replace")
        code, comments = split_code_and_comments(text)
        self.code_lines = code.split("\n")
        self.comment_lines = comments.split("\n")
        self.raw_lines = text.split("\n")
        count = len(self.code_lines)
        self.fn_of_line: list[str | None] = [None] * count
        self.in_test = [False] * count
        self.in_use = [False] * count
        self.fn_def_line: dict[str, int] = {}
        self._parse()

    def _parse(self) -> None:
        depth = 0
        stack: list[tuple[str | None, int]] = []
        pending_fn: str | None = None
        pending_cfg_test = False
        test_depth: int | None = None
        use_open = False

        for index, line in enumerate(self.code_lines):
            self.fn_of_line[index] = next(
                (name for name, _ in reversed(stack) if name), None
            )
            self.in_test[index] = test_depth is not None
            starts_use = line.lstrip().startswith("use ")
            self.in_use[index] = use_open or starts_use
            if starts_use and ";" not in line:
                use_open = True
            elif use_open and ";" in line:
                use_open = False

            if CFG_TEST_RE.search(line):
                pending_cfg_test = True

            events: list[tuple[int, str, str | None]] = [
                (match.start(), "fn", match.group(1)) for match in FN_RE.finditer(line)
            ]
            events += [
                (position, character, None)
                for position, character in enumerate(line)
                if character in "{};"
            ]
            events.sort(key=lambda event: event[0])

            for _, kind, name in events:
                if kind == "fn":
                    pending_fn = name
                    self.fn_def_line.setdefault(name, index)
                elif kind == "{":
                    if pending_cfg_test and test_depth is None:
                        test_depth = depth
                        pending_cfg_test = False
                    stack.append((pending_fn, depth))
                    pending_fn = None
                    depth += 1
                elif kind == "}":
                    depth -= 1
                    if stack and stack[-1][1] == depth:
                        stack.pop()
                    if test_depth is not None and depth <= test_depth:
                        test_depth = None
                elif kind == ";":
                    pending_fn = None

    def body_text(self, function: str) -> str:
        return "\n".join(
            line
            for index, line in enumerate(self.code_lines)
            if self.fn_of_line[index] == function
        )

    def justification_text(self, function: str) -> str:
        """Comments inside the function, plus the comment block directly above its `fn`."""
        parts = [
            self.comment_lines[index]
            for index, line in enumerate(self.code_lines)
            if self.fn_of_line[index] == function
        ]
        definition = self.fn_def_line.get(function)
        if definition is not None:
            cursor = definition - 1
            while cursor >= 0:
                stripped = self.raw_lines[cursor].strip()
                if stripped.startswith(("//", "#[", "*", "/*")) or not stripped:
                    parts.append(self.comment_lines[cursor])
                    cursor -= 1
                    continue
                break
        return "\n".join(parts)

    def references(self, name: str) -> list[tuple[str, int]]:
        """Lines that mention `name` as code, excluding its own definition, imports, tests.

        Returns (enclosing function or empty string for module scope, line number).
        """
        pattern = re.compile(r"\b" + re.escape(name) + r"\b")
        definition = self.fn_def_line.get(name)
        found: list[tuple[str, int]] = []
        for index, line in enumerate(self.code_lines):
            if self.in_use[index] or self.in_test[index]:
                continue
            if index == definition:
                continue
            if not pattern.search(line):
                continue
            found.append((self.fn_of_line[index] or "", index + 1))
        return found


class Tree:
    """Every Rust source under `root`, parsed on demand.

    Parsing is char-by-char and the tree is ~570 files, so it is done only for a file whose
    raw text already contains the token being asked about. That prefilter is a plain
    substring test on purpose: it can only admit files, never reject a real finding, so it
    cannot become a second matcher that quietly decides the verdict.
    """

    def __init__(self, root: Path) -> None:
        self.root = root
        self.paths = rust_source_files(root)
        self.raw = {
            path.relative_to(root).as_posix(): path.read_text(
                encoding="utf-8", errors="replace"
            )
            for path in self.paths
        }
        self._parsed: dict[str, FileIndex] = {}

    def index_for(self, rel: str) -> FileIndex:
        cached = self._parsed.get(rel)
        if cached is None:
            cached = FileIndex(self.root / rel, self.root)
            self._parsed[rel] = cached
        return cached

    def files_mentioning(self, token: str) -> list[str]:
        return [rel for rel, text in self.raw.items() if token in text]

    def callers_of(self, name: str) -> list[tuple[str, str]] | None:
        """(file, function) contexts that mention `name`, or None if any is module scope."""
        contexts: list[tuple[str, str]] = []
        for rel in self.files_mentioning(name):
            for function, _ in self.index_for(rel).references(name):
                if not function:
                    return None
                contexts.append((rel, function))
        return contexts


def has_gate(tree: Tree, rel: str, function: str, gates: dict[str, str]) -> bool:
    body = tree.index_for(rel).body_text(function)
    return any(re.search(re.escape(token), body) for token in gates)


def reaches_gate(
    tree: Tree,
    rel: str,
    function: str,
    gates: dict[str, str],
    depth: int = 0,
    path: frozenset[tuple[str, str]] = frozenset(),
) -> bool:
    """Is every path that can reach `function` behind one of the approved `gates`?"""
    if (rel, function) in path:
        return False
    if has_gate(tree, rel, function, gates):
        return True
    if depth >= MAX_CALLER_DEPTH:
        return False
    callers = tree.callers_of(function)
    if not callers:
        return False
    deeper = path | {(rel, function)}
    return all(
        reaches_gate(tree, caller_rel, caller_fn, gates, depth + 1, deeper)
        for caller_rel, caller_fn in callers
    )


def load_baseline(path: Path) -> tuple[dict[str, str], dict[str, str]]:
    if not path.exists():
        return {}, {}
    data = json.loads(path.read_text(encoding="utf-8"))
    return data.get("gates", {}), data.get("sites", {})


def scan(root: Path, gates: dict[str, str], sites: dict[str, str]) -> list[str]:
    tree = Tree(root)
    failures: list[str] = []
    matched_sites: set[str] = set()

    candidates = sorted(
        {rel for token in PRIMITIVES for rel in tree.files_mentioning(token)}
    )
    for rel in candidates:
        index = tree.index_for(rel)
        for line_number, line in enumerate(index.code_lines, start=1):
            if index.in_use[line_number - 1] or index.in_test[line_number - 1]:
                continue
            if DECLARATION_RE.search(line):
                continue
            match = PRIMITIVE_RE.search(line)
            if not match:
                continue
            function = index.fn_of_line[line_number - 1]
            where = f"{index.rel}:{line_number}"
            if function is None:
                failures.append(
                    f"{where}: {match.group(1)} outside any function, so nothing can gate it"
                )
                continue
            key = f"{index.rel}::{function}"
            if reaches_gate(tree, index.rel, function, gates):
                continue
            if key in sites:
                matched_sites.add(key)
                if JUSTIFICATION_MARKER not in index.justification_text(function):
                    failures.append(
                        f"{where}: {key} is in the baseline but carries no "
                        f"'{JUSTIFICATION_MARKER}' comment on or above `fn {function}`"
                    )
                continue
            failures.append(
                f"{where}: {match.group(1)} in `fn {function}` is reachable by default -- "
                f"no approved gate within {MAX_CALLER_DEPTH} callers and no baseline entry"
            )

    for key in sorted(set(sites) - matched_sites):
        failures.append(
            f"{BASELINE_PATH.name}: stale entry {key} -- it matches no ungated call site; delete it"
        )

    return failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    parser.add_argument("--baseline", type=Path, default=BASELINE_PATH)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)

    if args.selftest:
        return selftest()

    gates, sites = load_baseline(args.baseline)
    if not gates:
        print(
            f"{args.baseline}: no approved gate predicates; every call site would fail",
            file=sys.stderr,
        )
        return 1

    failures = scan(args.root, gates, sites)
    if failures:
        print(
            "Thread suspension must never be reachable from a default code path.",
            file=sys.stderr,
        )
        print(
            "Gate the site behind an approved opt-in from "
            f"{args.baseline.name}, hold the cross-DLL freeze lock, or add the site to that "
            "file with a one-line reason and a "
            f"'{JUSTIFICATION_MARKER}' comment at the code.\n",
            file=sys.stderr,
        )
        print("\n".join(failures), file=sys.stderr)
        return 1
    return 0


# --------------------------------------------------------------------------
# selftest
# --------------------------------------------------------------------------
def _write(root: Path, rel: str, body: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def _case(name: str, files: dict[str, str], sites: dict[str, str], want_hits: int) -> bool:
    gates = {
        "profiler_rip_enabled": "boot profiler RIP sampling opt-in",
        "freeze_guard::hold": "cross-DLL MinHook freeze lock",
    }
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for rel, body in files.items():
            _write(root, rel, body)
        failures = scan(root, gates, sites)
    ok = len(failures) == want_hits
    print(f"  {'pass' if ok else 'FAIL'}  {name}  (findings={len(failures)}, want={want_hits})")
    if not ok:
        for line in failures:
            print(f"        | {line}")
    return ok


UNGATED = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
fn freeze_everything(thread: isize) {
    unsafe { SuspendThread(thread) };
}
"""

GATED_SAME_FN = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
fn freeze_everything(thread: isize) {
    if !profiler_rip_enabled() {
        return;
    }
    unsafe { SuspendThread(thread) };
}
"""

GATED_IN_CALLER = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
fn freeze_everything(thread: isize) {
    unsafe { SuspendThread(thread) };
}
fn tick(thread: isize) {
    if profiler_rip_enabled() {
        freeze_everything(thread);
    }
}
"""

GATED_ONE_CALLER_ONLY = GATED_IN_CALLER + """
fn other_tick(thread: isize) {
    freeze_everything(thread);
}
"""

FREEZE_LOCK = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
fn install_detour(thread: isize) {
    let _guard = freeze_guard::hold("install_detour");
    unsafe { SuspendThread(thread) };
}
"""

PROSE_AND_STRINGS = """
// SuspendThread is what MinHook's Freeze() calls on every other thread.
/// Between `SuspendThread` and `ResumeThread` nothing may allocate.
fn describe() -> &'static str {
    "SuspendThread and GetThreadContext"
}
"""

TEST_ONLY = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
#[cfg(test)]
mod tests {
    #[test]
    fn suspends_in_a_host_test() {
        unsafe { super::SuspendThread(0) };
    }
}
"""

BASELINED_WITHOUT_MARKER = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
/// Snapshot one thread.
fn sample_thread(thread: isize) {
    unsafe { SuspendThread(thread) };
}
"""

BASELINED_WITH_MARKER = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
/// Snapshot one thread.
///
/// Thread suspension: fires only after a stall has already been detected.
fn sample_thread(thread: isize) {
    unsafe { SuspendThread(thread) };
}
"""

BRACES_IN_A_FORMAT_STRING = """
unsafe extern "system" {
    fn SuspendThread(thread: isize) -> u32;
}
fn log_it(count: u32) -> String {
    format!("{count} threads {{escaped}}")
}
fn freeze_everything(thread: isize) {
    if !profiler_rip_enabled() {
        return;
    }
    unsafe { SuspendThread(thread) };
}
"""


def selftest() -> int:
    print("check-no-thread-suspension selftest")
    results = [
        _case("ungated call is caught", {"a.rs": UNGATED}, {}, 1),
        _case("gate in the same function", {"a.rs": GATED_SAME_FN}, {}, 0),
        _case("gate one caller above", {"a.rs": GATED_IN_CALLER}, {}, 0),
        _case(
            "one gated caller is not enough",
            {"a.rs": GATED_ONE_CALLER_ONLY},
            {},
            1,
        ),
        _case("freeze lock held", {"a.rs": FREEZE_LOCK}, {}, 0),
        _case("comments and strings are not call sites", {"a.rs": PROSE_AND_STRINGS}, {}, 0),
        _case("cfg(test) module is not a runtime path", {"a.rs": TEST_ONLY}, {}, 0),
        _case(
            "baseline entry without the justification comment",
            {"a.rs": BASELINED_WITHOUT_MARKER},
            {"a.rs::sample_thread": "diagnostic watchdog"},
            1,
        ),
        _case(
            "baseline entry with the justification comment",
            {"a.rs": BASELINED_WITH_MARKER},
            {"a.rs::sample_thread": "diagnostic watchdog"},
            0,
        ),
        _case(
            "stale baseline entry",
            {"a.rs": GATED_SAME_FN},
            {"a.rs::gone": "no longer exists"},
            1,
        ),
        _case(
            "braces inside a format string do not break nesting",
            {"a.rs": BRACES_IN_A_FORMAT_STRING},
            {},
            0,
        ),
    ]
    failed = results.count(False)
    print(f"{len(results) - failed}/{len(results)} selftest cases passed")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
