#!/usr/bin/env python3
"""A log describes exactly ONE process run. This makes that executable.

Standing rule (2026-08-04): no product DLL, shell or harness in this repo may append to a log
across runs. Each log file is truncated by the first write of the process that owns it; keeping
an older run means copying the file aside yourself, not letting it accumulate.

The failure that set the rule: `crates/er-invasion-warp-dll/src/lib.rs` opened its log with a
plain `OpenOptions::new().append(true)` on a fixed name next to the game executable. Twelve
separate launches piled into one 565 KB file, so a count taken over it ("37 confirms") read as
ONE run doing something 37 times when it was really twelve runs, and per-run state could only be
recovered by hand-splitting on the module-base banner. Worse, lines from builds that no longer
exist sat indistinguishably next to lines from the build under test.

A comment saying "don't append" is not enforcement -- this repo has been bitten by exactly that
(see `check-me3-shell-coverage.py`, which exists because an array was kept correct by a comment).
So the shape is pinned instead:

  * `er_game_base::log::begin_fresh_run(path)` is the one-shot. The FIRST call for a path in a
    process rotates the previous run's file aside (`<name>.prev`, exactly one generation) and
    truncates. Every later call is a no-op, so a run never loses its own earlier lines.
  * `er_game_base::log::open_fresh_run_append(path)` runs that one-shot and hands back an
    appending handle. It is the ONLY sanctioned appending opener.
  * Therefore: an `OpenOptions`-style `.append(...)` may appear ONLY in the helper module, or in
    a file listed in EXEMPT with a stated reason.

`Vec::append` / `String::append` take `&mut ...`, so an argument starting with `&` is not a file
opener and is not flagged. `File::create` / `truncate(true)` are already fresh and are ignored.

Usage:
    python3 scripts/check-fresh-run-logs.py
    python3 scripts/check-fresh-run-logs.py --selftest

Exit status is 1 on any failure, so this can gate.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Directories that hold no product/harness Rust source: build output, agent worktree COPIES of
# this same repo (scanning them double-counts and reports paths that do not exist upstream), and
# vendored trees.
SKIP_DIRS = {".git", ".claude", ".worktrees", "target", "third_party", "save-files", "docs"}

# `.append(` whose argument does NOT start with `&`. That one character separates an OpenOptions
# builder (`.append(true)`, `.append(flag)`) from `Vec::append(&mut other)`.
APPEND_CALL = re.compile(r"\.append\s*\(\s*(?!&)([^)]*)\)")

# The sanctioned entry points. A file that opens for append must be the module defining these.
HELPER_FUNCTIONS = ("begin_fresh_run", "open_fresh_run_append")

# Files allowed to open a log for append. Each needs a reason: an unexplained exemption reads as
# considered and silently drops a real violation.
EXEMPT: dict[str, str] = {
    "crates/er-game-base/src/log.rs": (
        "the one-shot truncation helper itself -- `begin_fresh_run` rotates + truncates on the "
        "first write of the process and `open_fresh_run_append` is the appending opener every "
        "other crate routes through"
    ),
}


def rust_files(root: Path) -> list[Path]:
    """Every Rust source under `root`, minus build output and repo copies."""
    files: list[Path] = []
    for path in root.rglob("*.rs"):
        if any(part in SKIP_DIRS for part in path.relative_to(root).parts):
            continue
        files.append(path)
    return sorted(files)


def append_openers(text: str) -> list[tuple[int, str]]:
    """`(line number, argument)` for every file-append opener in `text`."""
    found: list[tuple[int, str]] = []
    for match in APPEND_CALL.finditer(text):
        line = text[: match.start()].count("\n") + 1
        found.append((line, match.group(1).strip()))
    return found


def defines_helper(text: str) -> bool:
    """Whether `text` is the module that DEFINES the sanctioned helpers."""
    return all(re.search(rf"fn\s+{name}\b", text) for name in HELPER_FUNCTIONS)


def check(root: Path) -> list[str]:
    """Return a list of failures; empty means every log in the tree is fresh per run."""
    failures: list[str] = []
    exempt_hits: dict[str, int] = {name: 0 for name in EXEMPT}

    for reason in EXEMPT.values():
        if not reason.strip():
            failures.append(
                "EXEMPT carries an entry with an empty reason -- an unexplained exemption "
                "cannot be told apart from an oversight"
            )

    for path in rust_files(root):
        relative = path.relative_to(root).as_posix()
        text = path.read_text(encoding="utf-8", errors="replace")
        openers = append_openers(text)
        if not openers:
            continue
        if relative in EXEMPT:
            exempt_hits[relative] += len(openers)
            continue
        for line, argument in openers:
            failures.append(
                f"{relative}:{line}: opens a log for append (`.append({argument})`) without the "
                f"one-shot truncation. Logs must describe ONE run: use "
                f"`er_game_base::log::open_fresh_run_append(path)` (or `append_line` / "
                f"`begin_fresh_run`), or add this file to EXEMPT with a reason."
            )

    # A stale exemption is worse than none: it reads as considered while covering nothing, and the
    # day that file grows a real appender the gate stays green.
    for name in sorted(EXEMPT):
        if not (root / name).exists():
            failures.append(f"{name}: EXEMPT here but the file does not exist -- stale exemption")
        elif exempt_hits[name] == 0:
            failures.append(
                f"{name}: EXEMPT here but it opens nothing for append -- stale exemption, drop it "
                f"so the next real appender in this file is caught"
            )

    # The helpers must actually exist, or every caller is routing through nothing.
    helper = root / "crates" / "er-game-base" / "src" / "log.rs"
    if not helper.exists() or not defines_helper(
        helper.read_text(encoding="utf-8", errors="replace")
    ):
        failures.append(
            "crates/er-game-base/src/log.rs no longer defines "
            f"{' + '.join(HELPER_FUNCTIONS)} -- the rule has nothing to route through"
        )
    return failures


def selftest() -> int:
    """Prove the checks fire, on synthetic inputs, in BOTH directions."""
    import tempfile

    failures = 0

    def case(name: str, condition: bool) -> None:
        nonlocal failures
        if not condition:
            print(f"selftest FAIL: {name}", file=sys.stderr)
            failures += 1

    # The real helper module: defines both one-shot entry points AND holds the repo's one
    # appending opener, which is exactly what its exemption covers.
    helper_source = (
        "pub fn begin_fresh_run(path: &Path) {}\n"
        "pub fn open_fresh_run_append(path: &Path) -> Option<File> {\n"
        "    begin_fresh_run(path);\n"
        "    OpenOptions::new().create(true).append(true).open(path).ok()\n"
        "}\n"
    )
    # Same helpers, no appending opener -- the shape that makes the exemption stale.
    helper_without_opener = (
        "pub fn begin_fresh_run(path: &Path) {}\n"
        "pub fn open_fresh_run_append(path: &Path) -> Option<File> { None }\n"
    )

    def tree(tmp: str, sources: dict[str, str]) -> Path:
        root = Path(tmp)
        (root / "crates" / "er-game-base" / "src").mkdir(parents=True, exist_ok=True)
        (root / "crates" / "er-game-base" / "src" / "log.rs").write_text(helper_source)
        for name, body in sources.items():
            target = root / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(body)
        return root

    # NEGATIVE DIRECTION: a violating snippet must fail.
    with tempfile.TemporaryDirectory() as tmp:
        root = tree(
            tmp,
            {
                "crates/bad-dll/src/lib.rs": (
                    "fn log(args: Arguments<'_>) {\n"
                    "    if let Ok(mut f) = OpenOptions::new()\n"
                    "        .create(true)\n"
                    "        .append(true)\n"
                    "        .open(LOG_PATH)\n"
                    "    {\n"
                    "        let _ = writeln!(f, \"{args}\");\n"
                    "    }\n"
                    "}\n"
                )
            },
        )
        problems = check(root)
        case(
            "a plain append(true) opener fails",
            any("crates/bad-dll/src/lib.rs:4" in p and ".append(true)" in p for p in problems),
        )

    # The variable-argument bypass: `.append(flag)` is the same hole spelled differently.
    with tempfile.TemporaryDirectory() as tmp:
        root = tree(
            tmp,
            {"crates/sneaky-dll/src/lib.rs": "let f = File::options().append(keep).open(p);\n"},
        )
        problems = check(root)
        case(
            "append(<variable>) fails too",
            any("crates/sneaky-dll/src/lib.rs:1" in p for p in problems),
        )

    # POSITIVE DIRECTION: compliant sources must pass.
    with tempfile.TemporaryDirectory() as tmp:
        root = tree(
            tmp,
            {
                "crates/good-dll/src/lib.rs": (
                    "fn log(args: Arguments<'_>) {\n"
                    "    if let Some(mut f) = er_game_base::log::open_fresh_run_append(&path()) {\n"
                    "        let _ = writeln!(f, \"{args}\");\n"
                    "    }\n"
                    "}\n"
                ),
                # Vec::append must not be mistaken for a file opener.
                "crates/good-dll/src/other.rs": "keep.append(&mut local);\n",
                # Truncation is already fresh and is not the target of this rule.
                "crates/good-dll/src/third.rs": (
                    "let f = OpenOptions::new().write(true).truncate(true).open(p);\n"
                ),
            },
        )
        problems = check(root)
        case("a compliant tree passes", problems == [])

    # The helper module itself is allowed to hold the one appending opener.
    with tempfile.TemporaryDirectory() as tmp:
        root = tree(tmp, {})
        problems = check(root)
        case("the exempt helper module passes", problems == [])

    # A stale exemption must be reported rather than quietly covering nothing.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "crates" / "er-game-base" / "src").mkdir(parents=True)
        (root / "crates" / "er-game-base" / "src" / "log.rs").write_text(helper_without_opener)
        problems = check(root)
        case(
            "an exemption covering no appender fails",
            any("stale exemption" in p for p in problems),
        )

    # Losing the helper must fail loudly: callers would be routing through nothing.
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "crates" / "er-game-base" / "src").mkdir(parents=True)
        (root / "crates" / "er-game-base" / "src" / "log.rs").write_text("// gutted\n")
        problems = check(root)
        case(
            "a gutted helper module fails",
            any("nothing to route through" in p for p in problems),
        )

    if failures:
        print(f"selftest: {failures} case(s) failed", file=sys.stderr)
        return 1
    print("[check-fresh-run-logs] selftest ok (7 cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    root = args.root.resolve()
    failures = check(root)
    if failures:
        print("[check-fresh-run-logs] FAIL:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    print(
        f"[check-fresh-run-logs] ok -- {len(rust_files(root))} Rust files scanned, every log "
        f"opener routes through the one-shot truncation, {len(EXEMPT)} file(s) exempt with reasons"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
