#!/usr/bin/env bash
# Matrix test for scripts/pr-refactor-scope.sh -- the trigger half of the DLL
# byte-identity gate. A trigger nobody tests is a gate nobody actually has.
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
scope="$root/scripts/pr-refactor-scope.sh"
failures=0

expect() { # expect <want-run> <event> <branch> <title> <label>
	local want="$1" event="$2" branch="$3" title="$4" label="$5" got
	got=$(EVENT_NAME="$event" HEAD_BRANCH="$branch" PR_TITLE="$title" GITHUB_OUTPUT='' \
		bash "$scope" | sed -n 's/^run=//p')
	if [ "$got" != "$want" ]; then
		echo "  FAIL: $label -- want run=$want got run=$got" >&2
		failures=$((failures + 1))
	fi
}

# --- in scope --------------------------------------------------------------
expect true pull_request refactor/save-picker "tidy up"          "branch contains refactor"
expect true pull_request REFACTOR-loading "tidy up"              "branch match is case-insensitive"
expect true pull_request feature/x "Refactor the quit menu"      "title contains refactor"
expect true pull_request feature/x "Move the cover above CONTINUE" "title contains move"
expect true pull_request feature/x "MOVE it"                     "title match is case-insensitive"
expect true workflow_dispatch "" ""                              "manual dispatch always runs"

# --- out of scope ----------------------------------------------------------
expect false pull_request feature/x "Add the invasion warp"      "unrelated PR"
expect false pull_request feature/x ""                           "empty title"
expect false pull_request "" ""                                  "empty branch and title"
# Substring matching is deliberate and it over-triggers; pin the known cases so
# the behaviour is a decision on record rather than an accident.
expect true pull_request feature/x "Remove the dead menu route"  "'remove' contains 'move' (known over-trigger)"
expect true pull_request feature/x "Movement injection probe"    "'movement' contains 'move' (known over-trigger)"

# --- injection safety ------------------------------------------------------
# A PR title is attacker-controlled. It must stay data, never become code.
canary="$(mktemp -u)"
EVENT_NAME=pull_request HEAD_BRANCH=feature/x GITHUB_OUTPUT='' \
	PR_TITLE="move\$(touch $canary)\`touch $canary\`" bash "$scope" >/dev/null
if [ -e "$canary" ]; then
	echo "  FAIL: PR title was evaluated by the shell" >&2
	rm -f "$canary"
	failures=$((failures + 1))
fi

if [ "$failures" -ne 0 ]; then
	echo "[test-pr-refactor-scope] $failures failure(s)" >&2
	exit 1
fi
echo "[test-pr-refactor-scope] ok"
