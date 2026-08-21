#!/usr/bin/env bash
# Decide whether a PR is in scope for the DLL byte-identity gate
# (.github/workflows/refactor-byte-identical.yml).
#
# In scope when the branch name contains "refactor", or the PR title contains
# "move" or "refactor" -- matched case-insensitively.
#
# Lives here rather than inline in the workflow for two reasons: the matching is
# then unit-testable (scripts/test-pr-refactor-scope.sh), and the PR title --
# attacker-controlled text on a `pull_request` event -- is read from the
# environment instead of being interpolated into a script body by Actions.
#
# Input  (environment): EVENT_NAME, PR_TITLE, HEAD_BRANCH
# Output (stdout, and appended to $GITHUB_OUTPUT when set):
#     run=true|false
#     reason=<human-readable>
set -euo pipefail

event="${EVENT_NAME:-}"
title=$(printf '%s' "${PR_TITLE:-}" | tr '[:upper:]' '[:lower:]')
branch=$(printf '%s' "${HEAD_BRANCH:-}" | tr '[:upper:]' '[:lower:]')

run=false
reason="not a refactor/move PR"

if [ "$event" = "workflow_dispatch" ]; then
	run=true
	reason="manual dispatch"
else
	case "$branch" in
	*refactor*)
		run=true
		reason="branch name contains 'refactor'"
		;;
	esac
	if [ "$run" = false ]; then
		case "$title" in
		*refactor*)
			run=true
			reason="PR title contains 'refactor'"
			;;
		*move*)
			run=true
			reason="PR title contains 'move'"
			;;
		esac
	fi
fi

printf 'run=%s\n' "$run"
printf 'reason=%s\n' "$reason"
if [ -n "${GITHUB_OUTPUT:-}" ]; then
	printf 'run=%s\n' "$run" >>"$GITHUB_OUTPUT"
	printf 'reason=%s\n' "$reason" >>"$GITHUB_OUTPUT"
fi
