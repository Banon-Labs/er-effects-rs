#!/usr/bin/env python3
"""Unified diff of one file in an outside checkout against a commit in this repo.

Sibling of `compare-tree-to-commit.py`, for when the answer needed is not "did it change" but
"what changed" -- and, like it, it exists because a worktree-isolated agent cannot point git at
another checkout.

    python3 scripts/diff-tree-to-commit.py <commit> <other-checkout-root> <relative-path> [--headers]

`--headers` prints only the hunk headers, which is enough to judge whether two branches will
collide on rebase without reading somebody else's in-flight work line by line.
"""

import difflib
import subprocess
import sys


def main(argv):
    if len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    commit, root, rel = argv[1], argv[2], argv[3]
    headers_only = "--headers" in argv[4:]
    done = subprocess.run(
        ["git", "show", f"{commit}:{rel}"],
        capture_output=True,
        check=False,
        # The workspace cap for any agent-run subprocess; one blob is milliseconds.
        timeout=30,
    )
    if done.returncode != 0:
        print(f"{rel} does not exist at {commit}", file=sys.stderr)
        return 1
    base = done.stdout.decode("utf-8", errors="replace").splitlines(keepends=True)
    with open(f"{root.rstrip('/')}/{rel}", encoding="utf-8", errors="replace") as handle:
        other = handle.readlines()
    for line in difflib.unified_diff(base, other, fromfile=f"{commit}:{rel}", tofile=rel, n=1):
        if headers_only and not line.startswith(("@@", "---", "+++")):
            continue
        sys.stdout.write(line if line.endswith("\n") else line + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
