#!/usr/bin/env python3
"""Reject a test scratch path under a shared temp root that carries no per-process component.

What went wrong, three times in one day, in six crates
------------------------------------------------------
A test builds its scratch directory from a fixed name under `std::env::temp_dir()` and wipes it
on entry. Under this repo's wine runner `%TEMP%` is the host `/tmp` -- one directory shared by
every process on the machine -- so a second copy of the same test binary (two agents running
`scripts/check.sh` at once, the host `cargo test` racing the wine `cargo xwin test`, or a second
checkout) deletes the files between one process's `fs::write` and its own probe.

The failure then accuses correct product code, which is what makes it expensive. Measured
before/after at 8-way concurrency on 2026-08-31:

    er-quit-menu-core     5 of 8 red (one run red on both tests)   ->  8/8, then 16/16
    er-save-redirect      7 of 8 red                               ->  0/80
    er-save-picker-core  10 of 80 red                              ->  0/80
    er-soulsformats       2 of 8 red                               ->  0/80
    er-invasion-warp      1 of 80 red                              ->  0/80

Every single before-failure blamed working code: `WrongSize { len: 0 }` for a container a
sibling had truncated, `MissingOrNotFile` for one it had deleted, `BridgeWriteFailed` for a
directory it had removed, and an identity probe answering `Unknown` because both of its inputs
were `Absent`. An agent spent an hour deciding whether the save-destination logic was broken.
It was not.

Why a gate and not a shared helper crate
----------------------------------------
The fix already existed in this repo and was invisible: `save_dest_test_dir` in
`save_dest_commit_runtime.rs`'s test module was pid-keyed and carried a comment describing this
exact failure -- but it was private to that module, which is how the tests one file over came to
be written without it. Two of the six fixes hoisted their helper to crate scope for exactly that
reason.

A workspace-wide test-support crate was considered and rejected. It would reach the nine crates
that need it only through nine `[dev-dependencies]` edges plus a new workspace member, and the
crates involved (`soulsformats`, `er-save-picker-core`, `er-hotkey-config`, `er-objectkit`) share
no existing dependency that could host it -- `er-game-base` covers five of nine. The helper's
whole body is one expression. Six copies of one expression was never the problem; nothing
telling you about it was. A gate tells you, in the crate you are editing, with the fix in the
message -- which is reachable from every crate in the workspace without a dependency edge at all.

What it decides on
------------------
The expression, not the name. A constant string joined onto a shared temp root is the defect;
any of these is the fix, and all four forms occur in this tree:

    std::env::temp_dir().join(format!("er-foo-{}", std::process::id()))
    std::env::temp_dir().join(format!("er-foo-{name}-p{pid}", pid = std::process::id()))
    let unique = format!("er-foo-{}-{}", std::process::id(), nanos);  // one let away
    std::env::temp_dir().join(unique)
    tempfile::TempDir::new()                                          // accepted, unused today

A site is skipped without needing an exemption when the path provably cannot reach a filesystem
call: its enclosing test function mentions no filesystem or subprocess token and its signature
mentions no `Path` type, so the value is inert and local. That is what keeps the four pure
expected-value paths in this tree (`temp_dir().join("run-42")` compared against a function's
return, `Path::new("/tmp/staged/ER0000.sl2")` fed to a pure normalizer) out of the exemption
list, where they would have been noise.

Both directions
---------------
`EXEMPT_SITES` and the detector each fail if the other is wrong: an exemption that no longer
matches a would-be finding is reported as stale and fails the gate, so a site that gets fixed or
deleted cannot leave a dead licence behind. `FROZEN_PROCESS_SCOPED` is the frozen negative -- real,
legitimate, pid-keyed sites that must keep classifying clean, with per-file floors -- so a
detector that stops matching (a lobotomised regex, an empty read, a renamed root) goes red
instead of reporting a confident zero.

Wiring (for whoever lands the current `scripts/check.sh` -- this file must not touch it):

    python3 "$repo_root/scripts/check-test-temp-isolation.py" --selftest
    python3 "$repo_root/scripts/check-test-temp-isolation.py"

Usage:
    python3 scripts/check-test-temp-isolation.py             # scan the tree
    python3 scripts/check-test-temp-isolation.py --selftest  # prove the detector, both ways
    python3 scripts/check-test-temp-isolation.py --list      # every classified site
"""

from __future__ import annotations

import argparse
import re
import sys
from dataclasses import dataclass
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from repo_source_scan import REPO_ROOT, rust_source_files  # noqa: E402

# --------------------------------------------------------------------------------------------
# What counts as a shared temp root, what counts as making a path this process's own, and what
# counts as evidence that a path can reach the filesystem.
# --------------------------------------------------------------------------------------------

# Rooted at the machine-wide temp directory. Under wine `%TEMP%` resolves to the host `/tmp`, so
# a windows-target test binary and a host one land in the same directory -- which is why the host
# `cargo test` and the wine `cargo xwin test` of one crate collide with each other.
TEMP_ROOT_CALL = re.compile(r"\b(?:std\s*::\s*)?env\s*::\s*temp_dir\s*\(\s*\)")
TEMP_ROOT_WINAPI = re.compile(r"\bGetTempPath[AW]?\s*\(")
TEMP_ROOT_ENV_VAR = re.compile(r"\b(?:std\s*::\s*)?env\s*::\s*var(?:_os)?\s*\(\s*(?:&\s*)?[br]*\"(?:TEMP|TMP|TMPDIR)\"")
# A hard-coded literal naming the same shared directory by absolute path.
TEMP_ROOT_LITERALS = ("/tmp/", "/var/tmp/", "%TEMP%", "%TMP%", "C:\\Temp", "C:/Temp")
TEMP_ROOT_LITERAL_EXACT = ("/tmp", "/var/tmp")

# Anything here makes the path this process's own, so two concurrent runs cannot name one file.
# `process::id()` is the repo's idiom; `tempfile` is accepted (nothing uses it today) because a
# `TempDir` guard is a legitimate answer to the same question and a gate that refused it would
# push people back to hand-rolled names.
PROCESS_UNIQUE_MARKERS = (
    re.compile(r"\b(?:std\s*::\s*)?process\s*::\s*id\s*\(\s*\)"),
    re.compile(r"\btempfile\s*::"),
    re.compile(r"\bTempDir\s*::\s*new\b"),
    re.compile(r"\btempdir\s*\("),
)

# Helpers in this tree that are themselves gated (their definitions are sites, and each is
# pid-keyed and frozen below). A call to one of these is as good as an inline pid.
SANCTIONED_HELPERS = (
    "save_dest_test_dir",
    "picker_scratch_dir",
    "scratch_dir",
)

# Evidence that a path in this function can reach the filesystem or a subprocess. Deliberately
# generous: a false positive here only means the site must carry a pid, which costs nothing,
# while a false negative means a defect ships. `.save(`/`.load(` are here because
# er-invasion-warp-core's collision happened through a product API, not through `std::fs`.
FILESYSTEM_TOKENS = (
    "fs::",
    "File::",
    "OpenOptions",
    "create_dir",
    "remove_dir",
    "remove_file",
    "read_to_string",
    "read_dir",
    "write_all",
    "set_permissions",
    "symlink",
    "hard_link",
    "canonicalize",
    ".exists()",
    ".metadata(",
    ".is_file()",
    ".is_dir()",
    ".save(",
    ".load(",
    ".persist(",
    "Command::new",
)

# A signature that mentions a path type returns or accepts one, so the value escapes the
# function and the body's own token set says nothing about where it ends up.
PATH_TYPE_IN_SIGNATURE = re.compile(r"\bPath(?:Buf)?\b")


# --------------------------------------------------------------------------------------------
# Sites that are genuinely fixed by design. Keyed by (path, snippet that must appear in the
# offending statement) so the entry survives line drift but dies the moment the site changes.
# --------------------------------------------------------------------------------------------
EXEMPT_SITES: dict[tuple[str, str], str] = {
    (
        "crates/er-build-export/tests/reference_decoder.rs",
        "DEFAULT_REFERENCE_JS",
    ): (
        "READ-ONLY INPUT, never a scratch directory. It names an extracted copy of the planner's "
        "third-party LZ-UTF8 bundle that the test only ever `require`s from node; the test writes "
        "nothing under it and removes nothing. Being fixed is the point -- it is the fallback for "
        "when ER_LZUTF8_REFERENCE_JS says nothing, and a pid in it could never match a file that "
        "some earlier session extracted. A concurrent run reading the same bundle is correct."
    ),
}

# --------------------------------------------------------------------------------------------
# The frozen negative. Real sites in this tree that are legitimately process-scoped and must keep
# classifying clean. Per-file floors, not exact text, so a renamed tag does not go red -- but a
# detector that stops seeing the tree does, which is the whole point. er-save-redirect's floor of
# 3 covers both shapes at once: one inline `format!(... process::id())` and two that reach the pid
# through a `let unique = ...` bound one statement earlier -- so the let-chase is frozen too, not
# only the easy inline case.
FROZEN_PROCESS_SCOPED: dict[str, int] = {
    "crates/er-quit-menu-core/src/save_dest_identity.rs": 1,
    "crates/er-save-picker-core/src/lib.rs": 1,
    "crates/er-save-redirect/src/lib.rs": 3,
    "crates/er-invasion-warp/src/lib.rs": 1,
    "crates/er-invasion-warp-core/src/local_invasion_config.rs": 4,
    "crates/soulsformats/src/lib.rs": 1,
    "crates/er-game-base/src/log.rs": 2,
    "crates/er-save-loader/src/lib_parts/load_methods.rs": 4,
    "crates/er-hotkey-config/src/reload.rs": 1,
    "crates/er-objectkit/src/capture.rs": 1,
}

# Repo-wide floor on classified sites. A scan that finds fewer than this has stopped reading the
# tree -- an empty file read, a neutered regex, a moved root -- and its zero findings mean nothing.
MIN_CLASSIFIED_SITES = 18


# --------------------------------------------------------------------------------------------
# A small Rust lexer: enough to know code from comments from string literals.
# --------------------------------------------------------------------------------------------
@dataclass(frozen=True)
class StringSpan:
    start: int
    end: int
    value: str


def lex(text: str) -> tuple[str, list[StringSpan]]:
    """Return (code, strings).

    `code` is `text` with every comment and every string/char literal blanked to spaces, same
    length and same line breaks, so offsets and line numbers stay valid and a brace inside a
    comment or a `"{"` literal cannot confuse the structure scan. `strings` carries the literal
    spans back, because a hard-coded `"/tmp/..."` is only visible there.
    """
    out = list(text)
    strings: list[StringSpan] = []
    i, n = 0, len(text)

    def blank(a: int, b: int) -> None:
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth += 1
                    j += 2
                elif text.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
            continue
        # Raw string: r"..." / r#"..."# / br##"..."## -- and not the raw identifier `r#foo`.
        m = re.match(r"(?:b?r)(#*)\"", text[i:])
        if m and (c == "r" or (c == "b" and text[i : i + 2] == "br")):
            hashes = m.group(1)
            close = '"' + hashes
            j = text.find(close, i + m.end())
            j = n if j < 0 else j + len(close)
            strings.append(StringSpan(i, j, text[i + m.end() : max(i + m.end(), j - len(close))]))
            blank(i, j)
            i = j
            continue
        if c == '"' or (c == "b" and text[i : i + 2] == 'b"'):
            start = i
            j = i + (2 if c == "b" else 1)
            buf: list[str] = []
            while j < n:
                if text[j] == "\\":
                    buf.append(text[j : j + 2])
                    j += 2
                    continue
                if text[j] == '"':
                    j += 1
                    break
                buf.append(text[j])
                j += 1
            strings.append(StringSpan(start, j, "".join(buf)))
            blank(start, j)
            i = j
            continue
        if c == "'":
            # Char literal or lifetime. `'a` is a lifetime; `'a'` and `'\n'` are literals.
            if text.startswith("'\\", i):
                j = text.find("'", i + 2)
                j = i + 2 if j < 0 else j + 1
                blank(i, j)
                i = j
                continue
            if i + 2 < n and text[i + 2] == "'":
                blank(i, i + 3)
                i += 3
                continue
            i += 1
            continue
        i += 1

    return "".join(out), strings


def match_brace(code: str, open_index: int) -> int:
    """Index just past the `}` matching the `{` at `open_index` (or len(code))."""
    depth, i, n = 0, open_index, len(code)
    while i < n:
        if code[i] == "{":
            depth += 1
        elif code[i] == "}":
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return n


def balanced_bracket(code: str, open_index: int, opener: str, closer: str) -> int:
    depth, i, n = 0, open_index, len(code)
    while i < n:
        if code[i] == opener:
            depth += 1
        elif code[i] == closer:
            depth -= 1
            if depth == 0:
                return i + 1
        i += 1
    return n


# --------------------------------------------------------------------------------------------
# Scopes: which byte ranges of a file are test code, and which function encloses an offset.
# --------------------------------------------------------------------------------------------
CFG_TEST = re.compile(r"\bcfg\s*\(")
NOT_TEST = re.compile(r"\bnot\s*\(\s*test\b")
ITEM_AFTER_ATTR = re.compile(
    r"\A\s*(?:(?:#\[[^\n]*\]|///[^\n]*|//![^\n]*|//[^\n]*)\s*)*"
    r"(?:pub\s*(?:\([^)]*\)\s*)?)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?"
    r"(?:extern\s+\"[^\"]*\"\s+)?(mod|fn)\b"
)
FN_HEAD = re.compile(r"\bfn\s+\w+")


def attribute_spans(code: str) -> list[tuple[int, int, str]]:
    """Every `#[...]` in `code` as (start, end, inner text)."""
    spans: list[tuple[int, int, str]] = []
    for m in re.finditer(r"#!?\[", code):
        end = balanced_bracket(code, m.end() - 1, "[", "]")
        spans.append((m.start(), end, code[m.end() : end - 1]))
    return spans


def test_scopes(path: Path, code: str) -> list[tuple[int, int]]:
    """Byte ranges of `code` that are test code.

    Three ways a range becomes test code, all of which occur in this tree:
      * a whole file under `tests/` (integration tests),
      * an item carrying `#[cfg(test)]` / `#[cfg(all(test, windows))]` -- which is a `mod` for
        most crates but a bare `fn` for er-quit-menu-core's shared helper,
      * a `#[test]` function.
    """
    parts = path.parts
    if "tests" in parts:
        return [(0, len(code))]

    scopes: list[tuple[int, int]] = []
    for start, end, inner in attribute_spans(code):
        is_cfg_test = bool(CFG_TEST.search(inner)) and re.search(r"\btest\b", inner) and not NOT_TEST.search(inner)
        is_test_fn = re.fullmatch(r"\s*(?:\w+\s*::\s*)*test\s*", inner) is not None
        if not (is_cfg_test or is_test_fn):
            continue
        item = ITEM_AFTER_ATTR.match(code[end:])
        if not item:
            continue
        brace = code.find("{", end + item.end())
        if brace < 0:
            continue
        scopes.append((start, match_brace(code, brace)))
    return scopes


def function_spans(code: str) -> list[tuple[int, int, str]]:
    """Every `fn` as (signature start, body end, signature text)."""
    spans: list[tuple[int, int, str]] = []
    for m in FN_HEAD.finditer(code):
        i, n = m.end(), len(code)
        paren = angle = 0
        brace = -1
        while i < n:
            ch = code[i]
            if ch == "(":
                paren += 1
            elif ch == ")":
                paren -= 1
            elif ch == "<":
                angle += 1
            elif ch == ">":
                angle = max(0, angle - 1)
            elif ch == ";" and paren == 0:
                break  # trait method declaration, no body
            elif ch == "{" and paren == 0:
                brace = i
                break
            i += 1
        if brace < 0:
            continue
        spans.append((m.start(), match_brace(code, brace), code[m.start() : brace]))
    return spans


def enclosing(spans: list[tuple[int, int, str]], offset: int) -> tuple[int, int, str] | None:
    """Innermost span containing `offset`."""
    best = None
    for start, end, text in spans:
        if start <= offset < end and (best is None or start > best[0]):
            best = (start, end, text)
    return best


# --------------------------------------------------------------------------------------------
# Statements, and chasing a name back to where it was bound.
# --------------------------------------------------------------------------------------------
def statement_around(code: str, offset: int) -> tuple[int, int]:
    """The statement containing `offset`: back to the previous `;`/`{`/`}`, forward to its `;`."""
    depth = 0
    i = offset - 1
    while i >= 0:
        ch = code[i]
        if ch in ")]}":
            depth += 1
        elif ch in "([{":
            if depth == 0:
                break
            depth -= 1
        elif ch == ";" and depth == 0:
            break
        i -= 1
    start = i + 1

    depth = 0
    j = offset
    n = len(code)
    while j < n:
        ch = code[j]
        if ch in "([{":
            depth += 1
        elif ch in ")]}":
            if depth == 0:
                break
            depth -= 1
        elif ch == ";" and depth == 0:
            j += 1
            break
        j += 1
    return start, min(j, n)


BINDING = "let|const|static"


def binding_statement(code: str, name: str, before: int, scope_start: int) -> tuple[int, int] | None:
    """Where `name` was bound, searching backwards from `before` within `scope_start`."""
    pattern = re.compile(rf"\b(?:{BINDING})\s+(?:mut\s+)?{re.escape(name)}\b")
    found = None
    for m in pattern.finditer(code, scope_start, max(scope_start, before)):
        found = m
    if found is None:
        # Module-scope const/static declared after the use, which is legal in Rust.
        for m in pattern.finditer(code):
            found = m
            break
    if found is None:
        return None
    return statement_around(code, found.start() + 1)


IDENTIFIER = re.compile(r"\b[A-Za-z_][A-Za-z0-9_]*\b")


def is_process_unique(code: str, statement: tuple[int, int], scope_start: int, depth: int = 2) -> bool:
    """Does the statement -- or, one or two `let`s back, what it names -- carry a per-process key?"""
    start, end = statement
    text = code[start:end]
    if any(marker.search(text) for marker in PROCESS_UNIQUE_MARKERS):
        return True
    if any(re.search(rf"\b{re.escape(h)}\s*\(", text) for h in SANCTIONED_HELPERS):
        return True
    if depth <= 0:
        return False
    for name in {m.group(0) for m in IDENTIFIER.finditer(text)}:
        nested = binding_statement(code, name, start, scope_start)
        if nested is None or nested == statement:
            continue
        if is_process_unique(code, nested, scope_start, depth - 1):
            return True
    return False


# --------------------------------------------------------------------------------------------
# Classification
# --------------------------------------------------------------------------------------------
@dataclass(frozen=True)
class Site:
    relative: str
    line: int
    statement: str
    verdict: str  # "process-scoped" | "shared" | "inert"
    reason: str


def temp_root_offsets(text: str, code: str, strings: list[StringSpan]) -> list[int]:
    offsets: list[int] = []
    for pattern in (TEMP_ROOT_CALL, TEMP_ROOT_WINAPI):
        offsets += [m.start() for m in pattern.finditer(code)]
    # The env-var forms name their key inside a literal, which `code` has blanked; match the
    # original text and keep only the hits whose call token survives in `code` (i.e. not in a
    # comment).
    for m in TEMP_ROOT_ENV_VAR.finditer(text):
        if code[m.start() : m.start() + 3].strip():
            offsets.append(m.start())
    for span in strings:
        value = span.value
        if value.startswith(TEMP_ROOT_LITERALS) or value in TEMP_ROOT_LITERAL_EXACT:
            offsets.append(span.start)
    return sorted(set(offsets))


def _snippet(text: str, statement: tuple[int, int]) -> str:
    """One-line rendering of the offending statement, without the doc comment above it.

    A statement's start is the previous `;`/`{`/`}`, so any comment between that and the code
    comes along; reporting `crates/x.rs:844: // PROCESS-SCOPED, like every other temp path ...`
    hides the expression the reader needs to see.
    """
    lines = [line for line in text[statement[0] : statement[1]].splitlines()]
    while lines and (not lines[0].strip() or lines[0].lstrip().startswith(("//", "/*", "*"))):
        lines.pop(0)
    return " ".join(" ".join(lines).split())[:200]


# Cheap pre-filter. `lex` is a per-character Python loop and the binding chase re-scans the file
# per identifier; running either over all 571 sources took 16s, which is too slow for the
# pre-commit hook this gate is wired into. Every root form below leaves one of these substrings in
# the raw bytes, so a file without any of them cannot hold a site and is never lexed. Measured:
# 24 of 571 files survive this, and the gate drops from 16s to well under one.
PREFILTER = ("temp_dir", "GetTempPath", "/tmp", "/var/tmp", "%TEMP%", "%TMP%", "C:\\Temp", "C:/Temp", '"TEMP"', '"TMP"', '"TMPDIR"')


def classify_file(path: Path, text: str) -> list[Site]:
    if not any(needle in text for needle in PREFILTER):
        return []
    code, strings = lex(text)
    offsets = temp_root_offsets(text, code, strings)
    if not offsets:
        return []

    try:
        relative = path.relative_to(REPO_ROOT).as_posix()
    except ValueError:
        relative = path.as_posix()

    scopes = test_scopes(path, code)
    if not scopes:
        return []
    functions = function_spans(code)
    sites: list[Site] = []

    for offset in offsets:
        if not any(a <= offset < b for a, b in scopes):
            continue  # not test code
        statement = statement_around(code, offset)
        fn = enclosing(functions, offset)
        scope_start = min(a for a, b in scopes if a <= offset < b)
        line = text.count("\n", 0, offset) + 1
        snippet = _snippet(text, statement)

        if is_process_unique(code, statement, fn[0] if fn else scope_start):
            sites.append(Site(relative, line, snippet, "process-scoped", ""))
            continue

        if fn is not None:
            body = code[fn[0] : fn[1]]
            signature = fn[2]
            touches_fs = any(token in body for token in FILESYSTEM_TOKENS)
            escapes = bool(PATH_TYPE_IN_SIGNATURE.search(signature))
            if not touches_fs and not escapes:
                sites.append(
                    Site(
                        relative,
                        line,
                        snippet,
                        "inert",
                        "pure value: no filesystem or subprocess token in the function, no path type in its signature",
                    )
                )
                continue

        sites.append(Site(relative, line, snippet, "shared", "no per-process component"))

    return sites


def scan(files: list[Path] | None = None) -> list[Site]:
    sites: list[Site] = []
    for path in files if files is not None else rust_source_files():
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        sites += classify_file(path, text)
    return sites


# --------------------------------------------------------------------------------------------
# Gate
# --------------------------------------------------------------------------------------------
FIX = (
    "Key the path by PROCESS, e.g.\n"
    '    let dir = std::env::temp_dir().join(format!("er-<crate>-{tag}-p{pid}", pid = std::process::id()));\n'
    "or hoist one such helper to crate scope and call it from every test module in the crate\n"
    "(that is what `picker_scratch_dir` in er-save-picker-core and `save_dest_test_dir` in\n"
    "er-quit-menu-core are). A `tempfile::TempDir` guard is accepted too."
)


def evaluate(sites: list[Site], *, enforce_floors: bool = True) -> list[str]:
    failures: list[str] = []
    shared = [s for s in sites if s.verdict == "shared"]
    used_exemptions: set[tuple[str, str]] = set()

    for site in shared:
        licence = None
        for key in EXEMPT_SITES:
            if key[0] == site.relative and key[1] in site.statement:
                licence = key
                break
        if licence is not None:
            used_exemptions.add(licence)
            continue
        failures.append(
            f"{site.relative}:{site.line}: shared temp path with no per-process component: {site.statement}"
        )

    for key, reason in EXEMPT_SITES.items():
        if key not in used_exemptions:
            failures.append(
                f"STALE EXEMPTION {key[0]} :: {key[1]!r} matched nothing the detector flagged. "
                f"Either the site was fixed or removed (delete the entry) or the detector stopped "
                f"seeing it (fix the detector). Recorded reason: {reason}"
            )

    if not enforce_floors:
        return failures

    clean = [s for s in sites if s.verdict == "process-scoped"]
    if len(sites) < MIN_CLASSIFIED_SITES:
        failures.append(
            f"FLOOR: classified {len(sites)} temp-rooted test sites, expected at least "
            f"{MIN_CLASSIFIED_SITES}. A scan this small is not reading the tree, so its zero "
            f"findings prove nothing."
        )
    per_file: dict[str, int] = {}
    for site in clean:
        per_file[site.relative] = per_file.get(site.relative, 0) + 1
    for relative, floor in FROZEN_PROCESS_SCOPED.items():
        actual = per_file.get(relative, 0)
        if actual < floor:
            failures.append(
                f"FROZEN NEGATIVE {relative}: {actual} process-scoped site(s), expected at least "
                f"{floor}. These are known-good pid-keyed paths; if one was legitimately removed, "
                f"lower the floor in FROZEN_PROCESS_SCOPED in the same commit."
            )
    return failures


# --------------------------------------------------------------------------------------------
# Selftest
# --------------------------------------------------------------------------------------------
DEFECT = """
#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn wipes_a_fixed_directory() {
        let dir = std::env::temp_dir().join("er-widget-scratch");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a"), b"x").unwrap();
        assert!(dir.join("a").exists());
    }
}
"""

FIXED = """
#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn wipes_its_own_directory() {
        let dir = std::env::temp_dir().join(format!("er-widget-scratch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a"), b"x").unwrap();
        assert!(dir.join("a").exists());
    }
}
"""

INDIRECT = """
#[cfg(test)]
mod tests {
    use std::fs;

    #[test]
    fn reaches_the_pid_through_one_let() {
        let unique = format!("er-widget-{}-{}", std::process::id(), 7);
        let root = std::env::temp_dir().join(unique);
        fs::create_dir_all(&root).unwrap();
    }
}
"""

INERT = """
#[cfg(test)]
mod tests {
    #[test]
    fn compares_a_path_it_never_touches() {
        let wanted = std::env::temp_dir().join("run-42").join("artifact.log");
        assert_eq!(redirect("artifact.log"), wanted);
    }
}
"""

HELPER_HOLE = """
#[cfg(test)]
mod tests {
    // No filesystem token in this function at all -- but it hands the path OUT, so the body's
    // token set says nothing. The `-> PathBuf` is what keeps it flagged.
    fn scratch(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(tag)
    }
}
"""

WINDOWS_ONLY_FN = """
#[cfg(all(test, windows))]
pub(crate) fn helper_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("er-thing-{name}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
"""

LITERAL_TMP = """
#[cfg(test)]
mod tests {
    #[test]
    fn writes_under_a_hardcoded_tmp_path() {
        let path = std::path::Path::new("/tmp/er-widget-fixture/save.bin");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"x").unwrap();
    }
}
"""

NON_TEST = """
pub fn install_dxc() {
    let tarball = std::env::temp_dir().join("er-shaderlab-dxc.tar.gz");
    std::fs::write(&tarball, b"x").unwrap();
}
"""

COMMENTED_OUT = """
#[cfg(test)]
mod tests {
    #[test]
    fn only_mentions_it_in_prose() {
        // let dir = std::env::temp_dir().join("er-widget-scratch");
        let name = "not /tmp/anything, just a string in a comparison";
        assert_eq!(name, name);
    }
}
"""


def _verdicts(source: str, name: str = "probe.rs") -> list[Site]:
    return classify_file(Path(REPO_ROOT / "crates" / "probe" / "src" / name), source)


def selftest() -> int:
    failures: list[str] = []

    def expect(label: str, source: str, wanted: list[str], filename: str = "probe.rs") -> None:
        got = [s.verdict for s in _verdicts(source, filename)]
        if got != wanted:
            failures.append(f"{label}: expected {wanted}, got {got}")

    # --- positive controls: the defect, in each of the shapes the six real ones took ---------
    expect("fixed name under temp_dir", DEFECT, ["shared"])
    expect("hard-coded /tmp literal", LITERAL_TMP, ["shared"])
    expect("path escapes via -> PathBuf", HELPER_HOLE, ["shared"])
    expect("cfg(all(test, windows)) fn", WINDOWS_ONLY_FN, ["shared"])

    # --- negative controls: legitimate forms that must not trip ------------------------------
    expect("pid in the format!", FIXED, ["process-scoped"])
    expect("pid one `let` away", INDIRECT, ["process-scoped"])
    expect("pure expected value", INERT, ["inert"])
    expect("non-test code", NON_TEST, [])
    expect("comment and prose only", COMMENTED_OUT, [])

    # --- an integration test file: the whole file is test code -------------------------------
    integration = classify_file(REPO_ROOT / "crates" / "probe" / "tests" / "it.rs", DEFECT.replace("#[cfg(test)]\nmod tests {", "mod tests {")[:-2])
    if [s.verdict for s in integration] != ["shared"]:
        failures.append(f"tests/ file: expected ['shared'], got {[s.verdict for s in integration]}")

    # --- the exemption mechanism, both directions --------------------------------------------
    detected = _verdicts(DEFECT)
    if not detected:
        # Reached when the detector has been neutered (a blinded regex, an empty read). Say so
        # and stop rather than dying on an index, so the reason is legible in the output.
        print("FAIL exemption: the detector found nothing in the planted defect", file=sys.stderr)
        for line in failures:
            print(f"FAIL {line}", file=sys.stderr)
        return 1
    shared_site = detected[0]
    live = evaluate([shared_site], enforce_floors=False)
    if not any("er-widget-scratch" in line for line in live):
        failures.append("exemption: an unexempted shared site was not reported")
    stale = [line for line in live if "STALE EXEMPTION" in line]
    if len(stale) != len(EXEMPT_SITES):
        failures.append(
            f"exemption: expected every one of the {len(EXEMPT_SITES)} real exemptions to read STALE "
            f"against a synthetic finding set, got {len(stale)}"
        )

    saved = dict(EXEMPT_SITES)
    try:
        EXEMPT_SITES.clear()
        EXEMPT_SITES[(shared_site.relative, "er-widget-scratch")] = "synthetic"
        licensed = evaluate([shared_site], enforce_floors=False)
        if licensed:
            failures.append(f"exemption: a licensed site should be silent, got {licensed}")
        EXEMPT_SITES.clear()
        EXEMPT_SITES[(shared_site.relative, "er-nonexistent")] = "synthetic"
        orphan = evaluate([shared_site], enforce_floors=False)
        if not any("STALE EXEMPTION" in line for line in orphan):
            failures.append("exemption: an exemption matching nothing must be reported STALE")
    finally:
        EXEMPT_SITES.clear()
        EXEMPT_SITES.update(saved)

    # --- the real tree, mutated: every clean site must go red when its pid is removed --------
    # The floors below prove the detector still sees the tree. This proves it still judges it:
    # each frozen file is re-read with every `process::id()` call deleted -- the exact edit that
    # produced the six defects -- and every site that was clean must come back as a finding. A
    # detector that has drifted into matching only one spelling passes the floors and fails here.
    for relative, floor in FROZEN_PROCESS_SCOPED.items():
        path = REPO_ROOT / relative
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            failures.append(f"mutation sweep: cannot read {relative}")
            continue
        clean = sum(1 for s in classify_file(path, text) if s.verdict == "process-scoped")
        stripped = re.sub(r"\b(?:std\s*::\s*)?process\s*::\s*id\s*\(\s*\)", "", text)
        shared = sum(1 for s in classify_file(path, stripped) if s.verdict == "shared")
        if clean and shared < clean:
            failures.append(
                f"mutation sweep {relative}: {clean} clean site(s), but only {shared} were "
                f"reported when every process::id() was removed -- the detector cannot see this "
                f"file's shape."
            )

    # --- the real tree: the frozen negatives, and the floor ----------------------------------
    # This is what makes the selftest non-vacuous. Blind the regexes, or make every file read
    # return empty, and the site count collapses to zero -- the floors below then fail, so a
    # silent zero can never be mistaken for a clean tree.
    real = scan()
    for line in evaluate(real):
        if line.startswith(("FLOOR", "FROZEN NEGATIVE", "STALE EXEMPTION")):
            failures.append(line)

    if failures:
        for line in failures:
            print(f"FAIL {line}", file=sys.stderr)
        return 1
    print(
        f"check-test-temp-isolation selftest: OK "
        f"({len(real)} sites classified, {len(FROZEN_PROCESS_SCOPED)} frozen negatives, "
        f"{len(EXEMPT_SITES)} exemption(s))"
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--selftest", action="store_true", help="prove the detector, both directions")
    parser.add_argument("--list", action="store_true", help="print every classified site and its verdict")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    sites = scan()
    if args.list:
        for site in sorted(sites, key=lambda s: (s.relative, s.line)):
            print(f"{site.verdict:>15}  {site.relative}:{site.line}  {site.statement}")

    failures = evaluate(sites)
    if failures:
        print(
            "A test scratch path under a SHARED temp root must carry a per-process component.\n"
            "`std::env::temp_dir()` is ONE directory for every process on the machine, and under\n"
            "this repo's wine runner `%TEMP%` resolves to the host `/tmp` -- so the host\n"
            "`cargo test` and the wine `cargo xwin test` of the same crate, two agents running\n"
            "check.sh at once, or a second checkout all name the SAME directory and delete each\n"
            "other's fixtures mid-test. The failure then accuses correct product code.\n\n" + FIX + "\n",
            file=sys.stderr,
        )
        print("\n".join(failures), file=sys.stderr)
        return 1

    print(f"check-test-temp-isolation: OK ({len(sites)} temp-rooted test sites classified, 0 shared)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
