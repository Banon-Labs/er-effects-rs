#!/usr/bin/env python3
"""One definition of "which `.rs` files under this root are this repository's Rust source".

Six gates used to carry six hand-maintained copies of this directory list and six copies of the
walk that consumes it. They had already drifted: on 2026-08-31 `check-no-lossy-utf8.py` was the
only one whose list omitted `.claude`, so it read 14,284 `.rs` copies out of other agents'
worktrees -- 96.2% of the 14,855 files it looked at -- and a stray `String::from_utf8_lossy` in
any sibling agent's sandbox failed this repo's gate. Nobody noticed, because the only symptoms
were a slow gate and an occasional red from a directory nobody thinks of as part of the repo.
One shared definition is the fix for the drift; the per-gate `also_ignore` argument is how a
gate says "and this one too, for THIS reason" without forking the whole list again.

The walk PRUNES, it does not post-filter. `dirs[:] = ...` during `os.walk` yields exactly the
paths a post-filtered `root.rglob("*.rs")` kept -- a path under an ignored directory carries
that directory in its relative `.parts`, so the post-filter had already dropped it -- while
never reading the ignored subtree. Measured 2026-08-31: `rglob` traversed all 1,118,634 entries
under this repo root (`.worktrees` 564,630 + `.claude` 387,965 + `target` 159,386 = 99.4% of
them) to arrive at 571 real source files, and under load that walk took 28-69s.

Not `git ls-files`. Measured and rejected for this family: the tracked set adds 20 tracked
`third_party/hudhook/*.rs` these gates deliberately exclude (they would go red on vendored
upstream code) and drops the untracked `crates/**/*.rs` that sibling agents have in flight (the
gates would stop seeing uncommitted work). The filesystem walk is the correct input here.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Iterable, Iterator

REPO_ROOT = Path(__file__).resolve().parents[1]

# Directories under this repo root that are not this repository's Rust source. Every entry here
# is load-bearing: each one actually holds `.rs` files a gate would otherwise read. Counts are
# `.rs` files measured under this root on 2026-08-31.
NOT_REPO_SOURCE: frozenset[str] = frozenset(
    {
        # VCS internals.
        ".git",
        # Gitignored local worktrees/sandboxes (AGENTS.md "Local Hidden Worktrees") -- deliberate
        # local work, explicitly not repo dirt, and not the committed tree a gate speaks for.
        ".worktrees",
        # Other agents' worktree copies of this same tree, each a full checkout: 14,284 `.rs`.
        # Scanning them double-counts every real file and lets a sibling's sandbox fail this
        # repo's gate. The bare segment also covers `.claude/skills` etc., none of which is
        # product source.
        ".claude",
        # Build output, including crate sources cargo unpacks under it: 159,386 entries.
        "target",
        # Vendored upstream (hudhook): 256 `.rs` this repo neither authors nor lints.
        "third_party",
    }
)


def iter_rust_sources(
    root: Path = REPO_ROOT, also_ignore: Iterable[str] = ()
) -> Iterator[Path]:
    """Yield every `.rs` under `root`, pruning `NOT_REPO_SOURCE | also_ignore` during the descent."""
    ignored = NOT_REPO_SOURCE | frozenset(also_ignore)
    for directory, subdirectories, filenames in os.walk(root):
        subdirectories[:] = [name for name in subdirectories if name not in ignored]
        base = Path(directory)
        for name in filenames:
            if name.endswith(".rs"):
                yield base / name


def rust_source_files(
    root: Path = REPO_ROOT, also_ignore: Iterable[str] = ()
) -> list[Path]:
    """Sorted `iter_rust_sources`. The stable order gates report and compare against."""
    return sorted(iter_rust_sources(root, also_ignore))


def _selftest() -> int:
    """Prove the pruning walk equals the post-filtered enumeration it replaced.

    Not run from `check.sh` (that file's commit is held behind other work); run it directly:
    `python3 scripts/repo_source_scan.py --selftest`.
    """
    import tempfile

    failures: list[str] = []

    with tempfile.TemporaryDirectory() as raw:
        tree = Path(raw)
        planted = {
            "crates/a/src/lib.rs",
            "tools/b/src/main.rs",
            "keep.rs",
            "docs/note.rs",
        }
        hidden = {
            ".claude/worktrees/copy/crates/a/src/lib.rs",
            ".worktrees/sandbox/crates/a/src/lib.rs",
            "target/debug/build/x/out/generated.rs",
            "third_party/hudhook/src/lib.rs",
            "crates/a/target/debug/nested.rs",
        }
        for relative in planted | hidden:
            path = tree / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("// planted\n", encoding="utf-8")
        (tree / "crates/a/src/not_rust.txt").write_text("x", encoding="utf-8")

        pruned = {p.relative_to(tree).as_posix() for p in rust_source_files(tree)}
        post_filtered = {
            p.relative_to(tree).as_posix()
            for p in tree.rglob("*.rs")
            if not any(part in NOT_REPO_SOURCE for part in p.relative_to(tree).parts)
        }
        if pruned != post_filtered:
            failures.append(
                f"pruning walk != post-filtered rglob: "
                f"only-pruned={sorted(pruned - post_filtered)} "
                f"only-post={sorted(post_filtered - pruned)}"
            )
        if pruned != planted:
            failures.append(f"expected {sorted(planted)}, got {sorted(pruned)}")

        with_docs = {p.relative_to(tree).as_posix() for p in rust_source_files(tree, {"docs"})}
        if with_docs != planted - {"docs/note.rs"}:
            failures.append(f"also_ignore did not drop docs/: {sorted(with_docs)}")

    if failures:
        for line in failures:
            print(f"FAIL {line}")
        return 1
    print("repo_source_scan selftest: OK")
    return 0


if __name__ == "__main__":
    import sys

    if "--selftest" in sys.argv:
        raise SystemExit(_selftest())
    for path in rust_source_files():
        print(path.relative_to(REPO_ROOT))
