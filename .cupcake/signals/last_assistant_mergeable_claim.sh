#!/usr/bin/env bash
# Cupcake signal: last_assistant_mergeable_claim
#
# Consumed by:
#   * no_mergeable_without_green_ci (Stop): halts a turn that closed by calling the pull request
#     mergeable while CI is not passing.
#
# Why this exists
#
# 2026-09-11. A turn closed "PR #426 is **MERGEABLE** now -- the conflicts are gone." while the
# `check` job was `in_progress` and had never passed on that branch. `gh pr view` had also printed
# `mergeStateStatus = BLOCKED` in the same command's output -- GitHub saying the merge button is
# disabled -- and that was read past. The user's words: "gh cli says its mergable. That doesn't
# mean it is."
#
# `mergeable` is GitHub's three-way merge result: does the branch apply to the base without a
# textual conflict. It is computed from trees and knows nothing about builds, tests, or whether a
# required check has started.
#
# What it emits
#
#   MERGEABLECLAIM:<claimed>:<verdict>
#
# claimed is 1 when the turn's closing prose called the pull request mergeable, 0 otherwise.
# verdict is the measured CI state for the current branch -- PASS / PENDING / FAIL / NOPR / UNKNOWN
# -- computed the same way ci_state_for_branch computes it. Nothing is emitted when the transcript
# cannot be read, so an absent signal asserts nothing.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT

# The CI half first, so the python below only has to read the transcript. `gh pr checks` exits
# non-zero whenever anything is failing or pending, so the exit code is ignored and the verdict
# comes from the rows. Every error path leaves UNKNOWN, which the policy treats as not-green.
verdict="UNKNOWN"
if command -v gh >/dev/null 2>&1 && command -v git >/dev/null 2>&1; then
	branch="$(git branch --show-current 2>/dev/null || true)"
	if [ -n "$branch" ]; then
		rows="$(gh pr checks "$branch" --json name,state 2>/dev/null || true)"
		if [ -n "$rows" ]; then
			verdict="$(printf '%s' "$rows" | python3 -c '
import json, sys
try:
    rows = json.load(sys.stdin)
except Exception:
    print("UNKNOWN"); raise SystemExit(0)
states = [str(r.get("state", "")).upper() for r in rows]
live = [s for s in states if s not in ("SKIPPED", "NEUTRAL")]
if not live:
    print("UNKNOWN")
elif any(s in ("FAILURE", "ERROR", "CANCELLED", "TIMED_OUT", "ACTION_REQUIRED") for s in live):
    print("FAIL")
elif any(s in ("PENDING", "QUEUED", "IN_PROGRESS", "WAITING", "REQUESTED", "EXPECTED") for s in live):
    print("PENDING")
elif all(s == "SUCCESS" for s in live):
    print("PASS")
else:
    print("UNKNOWN")
' 2>/dev/null || true)"
		fi
	fi
fi
[ -n "$verdict" ] || verdict="UNKNOWN"
export CUPCAKE_SIGNAL_CI_VERDICT="$verdict"

python3 - <<'PY' 2>/dev/null || true
import os, re, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
except Exception:
    sys.exit(0)  # fail open: a missing helper must never wedge a session

path = scan.latest_transcript()
if not path:
    sys.exit(0)
turn = scan.last_text_turn(scan.split_turns(scan.load_events(path)))
if turn is None:
    sys.exit(0)

# A turn that kept working past its last prose did not end on that prose.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)

text = turn.text(turn.last_text_index)

# Quoted spans and code fences are how a policy's own wording, a log line, or `gh` output gets
# reproduced. Reporting what a tool printed is not the same as adopting it as the verdict.
scrubbed = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
scrubbed = re.sub(r"`[^`]*`", " ", scrubbed)
scrubbed = re.sub(r'"[^"]*"', " ", scrubbed)

# The word itself, however it is spelled or emphasised, plus the paraphrase that carries the same
# claim without it. "not mergeable" / "no longer mergeable" are the honest report and are excluded.
CLAIM_RE = re.compile(
    r"(?<!not\s)(?<!isn't\s)(?<!is\snot\s)\bmerge-?able\b"
    r"|\bready\s+to\s+merge\b"
    r"|\bsafe\s+to\s+merge\b"
    r"|\bthe\s+conflicts?\s+(?:are|is)\s+gone\b",
    re.IGNORECASE,
)
NEGATED_RE = re.compile(
    r"\b(?:not|never|isn't|is\s+not|no\s+longer|un)\s*merge-?able\b"
    r"|\bmerge-?able\s+(?:is|was)\s+(?:false|no)\b",
    re.IGNORECASE,
)

claimed = 0
if CLAIM_RE.search(scrubbed) and not NEGATED_RE.search(scrubbed):
    claimed = 1

print("MERGEABLECLAIM:%d:%s" % (claimed, os.environ.get("CUPCAKE_SIGNAL_CI_VERDICT", "UNKNOWN")))
PY
