#!/usr/bin/env bash
# Cupcake signal: ci_state_for_branch
#
# Consumed by:
#   * no_false_ci_green (UserPromptSubmit): puts the measured CI state of the current branch's PR
#     into context before the next answer is composed, so "green" / "passing" / "CI is clean" can
#     only be written when it is true.
#
# Why this exists
#
# 2026-09-03: an answer described PR #384 as "pushed, green, and now carries the retraction" while
# `gh pr checks 384` reported `check  pending`. The word came from a local `scripts/check.sh` run
# that had exited 0 minutes earlier, and it silently became a claim about CI. The user reads "green"
# as "safe to merge", which is exactly the decision it is not safe to make from a local gate: local
# check.sh and GitHub CI are different job sets, and a push can be seconds old with nothing having
# run yet. Their words: "saying it is 'green' while CI is not passing on the latest push is
# objectively false."
#
# What it emits
#
#   CISTATE:<branch>:<pr number>:<verdict>:<summary>
#
# where verdict is one of pass / pending / fail / NOPR / unknown, and summary is a compact
# `name=state` list. Nothing is emitted when there is no repo, no gh, or no PR -- an absent signal
# must never be read as an assertion about CI.
#
# Why it is safe to run every PROMPT: one `gh pr checks` against the current branch, bounded to a
# few seconds, and it fails open on every error path. It does not push, edit, merge or comment.
set -uo pipefail

command -v gh >/dev/null 2>&1 || exit 0
command -v git >/dev/null 2>&1 || exit 0

branch="$(git branch --show-current 2>/dev/null)" || exit 0
[ -n "$branch" ] || exit 0

# --json keeps this parseable; `gh pr checks` exits non-zero when anything is failing or pending,
# so the exit code is deliberately ignored and the verdict comes from the rows themselves.
rows="$(gh pr checks "$branch" --json name,state 2>/dev/null)" || rows=""
if [ -z "$rows" ]; then
  number="$(gh pr view "$branch" --json number -q .number 2>/dev/null)" || number=""
  if [ -z "$number" ]; then
    printf 'CISTATE:%s:0:NOPR:no pull request for this branch' "$branch"
  else
    printf 'CISTATE:%s:%s:UNKNOWN:gh returned no check rows' "$branch" "$number"
  fi
  exit 0
fi

number="$(gh pr view "$branch" --json number -q .number 2>/dev/null)" || number="0"

printf 'CISTATE:%s:%s:' "$branch" "${number:-0}"
printf '%s' "$rows" | python3 -c '
import json, sys

try:
    rows = json.load(sys.stdin)
except Exception:
    print("UNKNOWN:check rows did not parse")
    raise SystemExit(0)

if not rows:
    print("UNKNOWN:no check rows")
    raise SystemExit(0)

# Skipping is not a failure and not a pass -- it is a job that had nothing to do. It is reported in
# the summary but never decides the verdict, or a PR whose only relevant job skipped would read as
# pass on the strength of a job that never ran.
def norm(state):
    return (state or "").upper()

states = [norm(r.get("state")) for r in rows]
failing = [s for s in states if s in ("FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED")]
pending = [s for s in states if s in ("PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED", "EXPECTED")]
passing = [s for s in states if s in ("SUCCESS", "PASS", "NEUTRAL")]

if failing:
    verdict = "FAIL"
elif pending:
    verdict = "PENDING"
elif passing:
    verdict = "PASS"
else:
    verdict = "UNKNOWN"

summary = ",".join("%s=%s" % (r.get("name", "?"), norm(r.get("state")).lower()) for r in rows)
print("%s:%s" % (verdict, summary[:400]))
'
