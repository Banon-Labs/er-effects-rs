#!/usr/bin/env bash
# Install the repo's version-controlled git hooks. Two things get installed, and the second one is
# the reason this script is not a one-liner any more:
#
#   1. core.hooksPath -> scripts/hooks. The normal path: git runs scripts/hooks/<name> directly.
#      Relative on purpose, so it survives a rename of the checkout and is correct inside every
#      linked worktree (git resolves it against that worktree's top-level).
#
#   2. scripts/hooks-fallback-shim -> $GIT_COMMON_DIR/hooks/<name>, for each hook in scripts/hooks/.
#      This is the directory git uses when core.hooksPath is UNSET, and it is not version
#      controlled. It held a 537-byte block-main-only pre-push from 2026-07-27 which ran neither
#      scripts/check-committed-compiles.sh nor scripts/check.sh; on 2026-08-31 a push
#      reached origin through it while core.hooksPath was briefly gone. core.hooksPath is not a
#      key this repo owns alone -- beads writes it too (`bd hooks install|uninstall`, `bd doctor`;
#      see the header of scripts/hooks-fallback-shim for the binary's own strings) -- so the
#      fallback has to be as strong as the gate rather than a weaker copy of it.
#
# Idempotent; safe to re-run, and re-running is the documented repair. bd
# static-guards-run-in-build-format-cycle-precommit-hook-2026-07-19.
set -euo pipefail

# Not `git rev-parse --show-toplevel`: this script is the repair tool for a broken git config, and
# on 2026-08-31 it could not run at all in the state it was meant to repair -- the checkout had
# core.bare=true, --show-toplevel died with "fatal: this operation must be run in a work tree",
# and `set -e` killed the script on its first line. The script's own location is not a git
# question and cannot fail that way.
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

chmod 0755 scripts/hooks/* scripts/hooks-fallback-shim 2>/dev/null || true
git config core.hooksPath scripts/hooks

# The FALLBACK directory. Hooks live in the common dir, so one install covers every linked
# worktree. Fall back to <repo>/.git/hooks only if git will not answer.
fallback_dir="$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
fallback_dir="${fallback_dir:-$repo_root/.git}/hooks"

installed=()
if mkdir -p "$fallback_dir" 2>/dev/null; then
	for hook in scripts/hooks/*; do
		[[ -f "$hook" ]] || continue
		name="$(basename -- "$hook")"
		cp -f scripts/hooks-fallback-shim "$fallback_dir/$name"
		chmod 0755 "$fallback_dir/$name"
		installed+=("$name")
	done
else
	echo "WARNING: could not create $fallback_dir -- the .git/hooks fallback is NOT installed," >&2
	echo "         so an unset core.hooksPath would mean no hook runs at all." >&2
fi

echo "installed: core.hooksPath -> scripts/hooks (pre-commit fast static guards; pre-push blocks main and runs local CI)"
echo "installed: fallback shim  -> $fallback_dir/{$(IFS=,; echo "${installed[*]:-none}")}"
echo "verify:    bash scripts/check-git-hooks-installed.sh"
echo "bypass:    git commit --no-verify / git push --no-verify   (emergency only; agents must not use this)"
