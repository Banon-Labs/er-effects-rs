#!/usr/bin/env bash
# Cupcake signal: last_assistant_wall_of_text
#
# Consumed by:
#   * wall_of_text (UserPromptSubmit): injects an invisible, measured one-paragraph correction into
#     the next turn's context when the previous turn's answer ran past one paragraph.
#
# Why this exists (user directive 2026-08-21, stated absolutely):
#   "I'll never. Repeat: Never read more than a single paragraph of response text from you.
#    Everything else is a skim."
# Every word past the first paragraph is not merely unwanted -- it is UNREAD. Prose the user skims is
# worse than no prose, because the agent believes it has communicated when it has not, and any
# caveat, blocker or correction buried past paragraph one has effectively been withheld.
#
# Why it no longer feeds a Stop HALT (2026-08-22, user: "That stop hook is not working if 1) I see it
# and 2) it doesn't prevent you from spewing information."). Both halves were correct and both are
# structural, not tuning problems:
#   1. Claude Code renders every Stop-hook verdict into the user's transcript. A blocking `reason`
#      becomes `stop_hook_summary.hookErrors` and prints as "Stop hook error: <reason>";
#      `hookSpecificOutput.additionalContext` becomes `hookAdditionalContext` and prints as "Stop hook
#      feedback: <text>". There is no agent-only channel at Stop, so the scolding was unavoidable.
#   2. Stop fires after the answer has been streamed to the terminal. Blocking cannot unsend it; it
#      only adds a rewrite, so the user read the long version and the scolding and the short version
#      -- three times the reading, from a guard whose whole purpose is less reading.
# UserPromptSubmit `additionalContext` has neither problem: it lands in the hidden-attachment set
# (`hook_additional_context`) that the REPL filters out of the rendered message list, and it arrives
# before the next answer is written. So this signal now measures the turn that just ended in order to
# constrain the turn about to start.
#
# What counts as a PARAGRAPH: `scripts/cupcake_turn_scan.prose_paragraphs` owns that, so the signal,
# its regression test and the false-positive audit cannot drift into three different answers. Fenced
# code (closed or left open), tables, list items and their continuations, headings, rules, and a
# caption line introducing structure are all structure, not prose: the objection is to reading, and
# those are scanned.
#
# What is MEASURED: the longest contiguous run of prose -- text blocks with no tool call between them
# -- not the turn's prose summed together. Summing was the guard's worst false positive: a tool-heavy
# turn emits a one-line preamble before each call, and eleven of those scored identically to an
# eleven-paragraph essay. Measured over six real transcripts (179 turns with prose),
# `scripts/audit-wall-of-text-false-positives.py` puts 15 turns in exactly that class.
#
# Emits  WALLOFTEXT:<paragraph count>:<first 60 chars of the offending run>  ; empty when clean.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
except Exception:
    sys.exit(0)  # fail open: a missing helper must never wedge a session

path = scan.latest_transcript()
if not path:
    sys.exit(0)

# last_text_turn, not turns[-1]: at UserPromptSubmit the new prompt may already be on disk, which
# leaves an empty trailing turn. The turn being judged is the last one that actually said something.
turn = scan.last_text_turn(scan.split_turns(scan.load_events(path)))
if turn is None:
    sys.exit(0)

worst, count = "", 0
for run in turn.text_runs:
    paragraphs = scan.prose_paragraphs(run)
    if len(paragraphs) > count:
        worst, count = paragraphs[0], len(paragraphs)

if count > 1:
    print("WALLOFTEXT:%d:%s" % (count, " ".join(worst.split())[:60]))
PY
