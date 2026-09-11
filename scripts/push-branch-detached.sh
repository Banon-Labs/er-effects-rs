#!/usr/bin/env bash
# Push the current branch from a shell that outlives the agent's foreground bash cap.
#
# The pre-push hook runs scripts/check.sh -- the same suite CI runs -- which takes far longer than
# the 30s an agent's foreground command gets. Run inline, the push is killed partway through its own
# gate and reports nothing useful; the branch does not reach origin and the failure looks like a
# hook problem rather than a timeout (bd branch-upload-runs-check-sh-and-the-harness-timeout-kills-it-2026-09-10).
#
# So this detaches with setsid, writes everything to a log the caller names, and exits immediately.
# The caller reads the log when it wants to know the outcome.
#
# Refuses `main` outright. The repo's rule is that agents never push there
# (AGENTS.md Session Completion), and scripts/git-pre-push-block-main.sh enforces it at the hook --
# but a guard that only fires inside a detached process reports its refusal to a log nobody is
# watching, so the check is repeated here where the caller still sees it.
#
# Usage: bash scripts/push-branch-detached.sh [<branch>] [<logfile>]
set -uo pipefail

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root" || exit 1

branch="${1:-$(git rev-parse --abbrev-ref HEAD)}"
log="${2:-$repo_root/target/push-$branch.log}"

if [[ "$branch" == "main" || "$branch" == "master" ]]; then
	printf 'push-branch-detached: REFUSED -- %s is not an agent-pushable branch.\n' "$branch" >&2
	exit 2
fi

mkdir -p "$(dirname "$log")"
: >"$log"

setsid nohup git push -u origin "$branch" >>"$log" 2>&1 </dev/null &
pid=$!

printf 'push-branch-detached: pushing %s, pid %s\n' "$branch" "$pid"
printf '  log: %s\n' "$log"
printf '  the pre-push suite runs first, so the log stays quiet for several minutes.\n'
