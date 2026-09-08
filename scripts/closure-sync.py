#!/usr/bin/env python3
"""Copy a candidate closure's paths from the main checkout into a pinned worktree.

Reads newline-separated repo-relative paths on stdin (blank lines and `#` comments ignored),
resets the worktree to its pinned head, then copies each path in. A path that is deleted in the
main checkout is deleted in the worktree too. Prints a sha256 for each copied file so the caller
can re-verify the same bytes at commit time.

The source checkout is the repo this script lives in (`$ER_MODS_ROOT` overrides), not a
hard-coded home directory -- a literal `/home/<someone>` here silently resolves to nothing
under a different user and the copy loop then reports every path deleted, which reads as
"the closure is empty" rather than "you looked in the wrong checkout".

The target must be a linked `git worktree`, and that is checked rather than trusted: this
script runs `git checkout -- .` and `git clean -fdq` on it, so pointing it at a main
checkout by mistake would destroy exactly the uncommitted pile it exists to help land. A
linked worktree's `.git` is a file containing a `gitdir:` pointer; a main checkout's is a
directory. That is the whole discriminator, and it is cheap.
"""
import hashlib
import os
import shutil
import subprocess
import sys

SRC = os.environ.get("ER_MODS_ROOT") or os.path.dirname(
    os.path.dirname(os.path.abspath(__file__))
)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: closure-sync.py <worktree-dir>", file=sys.stderr)
        return 2
    wt = os.path.abspath(sys.argv[1])
    dot_git = os.path.join(wt, ".git")
    if not os.path.isfile(dot_git):
        print(
            f"refusing: {wt} is not a linked git worktree (its .git is not a file).\n"
            "This script runs `git checkout -- .` and `git clean -fdq` on the target;\n"
            "against a main checkout that would destroy the uncommitted work being landed.\n"
            "Create one with: git worktree add --detach <dir> <commit>",
            file=sys.stderr,
        )
        return 2
    if not os.path.isdir(os.path.join(SRC, "scripts")):
        print(f"refusing: source checkout {SRC} has no scripts/ directory", file=sys.stderr)
        return 2
    paths = [
        line.strip()
        for line in sys.stdin
        if line.strip() and not line.lstrip().startswith("#")
    ]
    # Bounded because `scripts/check-no-timeouts.py` refuses an unbounded subprocess, and it is
    # right to: a `git` that hangs here leaves the worktree half-reset, which is the one state
    # that makes a later "the closure compiles" claim meaningless. Both calls are local index
    # operations on a worktree of a few thousand files; 30 s is the repo-wide hard cap, not a
    # tuned expectation.
    subprocess.run(["git", "-C", wt, "checkout", "--", "."], check=True, timeout=30)
    subprocess.run(
        ["git", "-C", wt, "clean", "-fdq", "crates", "scripts", "docs"], check=True, timeout=30
    )
    for rel in paths:
        src = os.path.join(SRC, rel)
        dst = os.path.join(wt, rel)
        if not os.path.exists(src):
            if os.path.exists(dst):
                os.remove(dst)
            print(f"DELETED\t{rel}")
            continue
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copy(src, dst)
        # Deliberately not copy2: preserving the source mtime lets cargo call a freshly
        # swapped-in file "fresh" and skip recompiling it, which reads exactly like a green
        # check of code that was never compiled.
        os.utime(dst, None)
        digest = hashlib.sha256(open(src, "rb").read()).hexdigest()[:16]
        print(f"{digest}\t{rel}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
