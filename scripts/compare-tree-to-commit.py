#!/usr/bin/env python3
"""Report which files in an outside checkout differ from a commit in this repo, by content.

The companion to `compare-worktree-trees.py`, and it exists for the same reason: a
worktree-isolated agent cannot run `git -C` against a sibling checkout, but it can read that
checkout's bytes and ask its own repository what the shared base commit holds.

    python3 scripts/compare-tree-to-commit.py <commit> <other-checkout-root> <subpath> [subpath ...]

Prints one line per path whose content differs from the commit's blob, then a count.
"""

import os
import subprocess
import sys


def blob(commit, rel):
    done = subprocess.run(
        ["git", "show", f"{commit}:{rel}"],
        capture_output=True,
        check=False,
        # The workspace cap for any agent-run subprocess. `git show` on one blob is milliseconds;
        # thirty seconds means the object store is wedged, which is worth failing over.
        timeout=30,
    )
    return done.stdout if done.returncode == 0 else None


def main(argv):
    if len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    commit, root = argv[1], argv[2]
    changed = 0
    total = 0
    for sub in argv[3:]:
        for dirpath, dirnames, filenames in os.walk(os.path.join(root, sub)):
            dirnames[:] = [d for d in dirnames if d not in {".git", "target", "node_modules"}]
            for name in sorted(filenames):
                full = os.path.join(dirpath, name)
                if os.path.islink(full):
                    continue
                rel = os.path.relpath(full, root)
                total += 1
                with open(full, "rb") as handle:
                    here = handle.read()
                there = blob(commit, rel)
                if there is None:
                    print(f"untracked-at-{commit}  {rel}")
                    changed += 1
                elif here != there:
                    print(f"differs                {rel}")
                    changed += 1
    print(f"# {changed} changed path(s) of {total} under {argv[3:]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
