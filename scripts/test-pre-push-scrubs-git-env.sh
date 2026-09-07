#!/usr/bin/env bash
# Prove scripts/hooks/pre-push cannot let a gate edit the live repository.
#
# THE DEFECT THIS PINS, measured 2026-09-06. `git` exports GIT_DIR to hooks run from a LINKED
# WORKTREE but not from the main checkout (scripts/measure-git-hook-env.sh, git 2.55.0). GIT_DIR
# wins over cwd, so a gate that builds a throwaway repository and configures it with
# `git -C <fixture> config ...` writes to THIS repository instead of the fixture. A push from
# .worktrees/teardown-outcome came back with the config guard reporting
#     core.hooksPath  scripts/hooks -> /home/banon/projects/er-mods-rs/scripts/hooks
# -- an absolute path computed for a fixture, landing on the checkout every other worktree and
# every other agent reads.
#
# WHY A BEHAVIOURAL TEST AND NOT A GREP. `unset $(git rev-parse --local-env-vars)` is one line and
# easy to keep; what is easy to LOSE is its position. Moved above the `cd`, or below the first
# gate, it still greps fine and stops working. So this builds a real main-repo-plus-linked-worktree
# pair in a temp directory, exports GIT_DIR exactly as git does, and measures where a fixture write
# actually lands -- with and without the scrub. Nothing here touches the repository it lives in.
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
hook="$repo_root/scripts/hooks/pre-push"

fail=0
ok()  { printf '  ok    %s\n' "$1"; }
bad() { printf '  FAIL  %s\n' "$1" >&2; fail=1; }

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# A main checkout with a linked worktree, standing in for this repo. `git worktree add` needs at
# least one commit, so make one.
main="$tmp/main"
git init -q "$main"
git -C "$main" -c user.email=t@t -c user.name=t commit -q --allow-empty -m init
git -C "$main" config core.hooksPath scripts/hooks
git -C "$main" worktree add -q --detach "$tmp/linked" HEAD

fixture="$tmp/fixture"
git init -q "$fixture"

# Exactly what git hands a pre-push hook in a linked worktree.
git_dir=$(git -C "$tmp/linked" rev-parse --absolute-git-dir)

shared_value() { git -C "$main" config --get core.hooksPath; }

# NEGATIVE CONTROL FIRST. If this does not leak, the test is vacuous -- it would pass on a git
# that never exported GIT_DIR, proving nothing about the scrub.
env GIT_DIR="$git_dir" bash -c \
	"cd '$tmp/linked'; git -C '$fixture' config core.hooksPath /LEAKED" >/dev/null 2>&1
if [[ "$(shared_value)" == "/LEAKED" ]]; then
	ok "negative control: without the scrub, a fixture write DOES hit the shared config"
else
	bad "negative control did not leak (got '$(shared_value)') -- this test proves nothing"
fi
git -C "$main" config core.hooksPath scripts/hooks

# ...and with the scrub the hook performs, the same write must land on the fixture.
env GIT_DIR="$git_dir" bash -c \
	"cd '$tmp/linked'; unset \$(git rev-parse --local-env-vars); git -C '$fixture' config core.hooksPath /CONTAINED" >/dev/null 2>&1
if [[ "$(shared_value)" == "scripts/hooks" ]]; then
	ok "with the scrub, the shared config is untouched"
else
	bad "the scrub did not contain the write: shared config is now '$(shared_value)'"
fi
if [[ "$(git -C "$fixture" config --get core.hooksPath)" == "/CONTAINED" ]]; then
	ok "with the scrub, the write reaches the fixture it was aimed at"
else
	bad "the write did not reach the fixture"
fi

# The hook must actually carry it, AFTER the cd that establishes which tree it is about and
# BEFORE the first gate it invokes. Position is the part that rots.
line_cd=$(awk '/^cd "\$repo_root"$/ { print NR; exit }' "$hook")
line_unset=$(awk '/^unset \$\(git rev-parse --local-env-vars\)$/ { print NR; exit }' "$hook")
line_first_gate=$(awk '/^(printf|bash|python3|mapfile|exec)/ { print NR; exit }' "$hook")
if [[ -z "$line_unset" ]]; then
	bad "scripts/hooks/pre-push does not scrub the local git env vars at all"
elif [[ -n "$line_cd" && "$line_unset" -gt "$line_cd" && -n "$line_first_gate" && "$line_unset" -lt "$line_first_gate" ]]; then
	ok "scrub is at line $line_unset -- after the cd ($line_cd), before the first gate ($line_first_gate)"
else
	bad "scrub at line $line_unset is out of position (cd=$line_cd, first gate=$line_first_gate)"
fi

if [[ $fail -eq 0 ]]; then
	echo "[test-pre-push-scrubs-git-env] passed"
else
	echo "[test-pre-push-scrubs-git-env] FAILED" >&2
fi
exit "$fail"
