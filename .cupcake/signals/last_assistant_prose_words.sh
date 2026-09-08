#!/usr/bin/env bash
# Cupcake signal: last_assistant_prose_words
#
# Consumed by:
#   * wall_of_text (UserPromptSubmit): injects an invisible, measured correction when the previous
#     turn's closing prose ran long in words, whatever its paragraph count.
#
# Why this exists, and why `last_assistant_wall_of_text` beside it was not enough.
#
# That signal counts paragraphs, and a paragraph count of 1 passes it unconditionally. On
# 2026-09-03 a single 281-word paragraph carried four separate subjects -- an empty-slot fallback,
# an 1800-tick threshold, a slot-validation fix, and an unrelated crash -- and ended with a pronoun.
# The user answered plainly with "it", there was no unique antecedent, and the agent could not
# resolve its own sentence. Their words:
#
#   "your message contained so much prose that when a user is asked to respond plainly, no response
#    will ever be good enough as long as it doesn't match the verbosity of your message. If a user
#    says it, and you have more than one subject it could apply to, that is a problem with your
#    prose being overloaded."
#
# So the cost is not only unread text. A long turn imposes a matching length on any reply that wants
# to be unambiguous, which bills the user for the agent's overload. Paragraph count cannot see that;
# word count can, because subjects arrive at a roughly fixed rate per word and one paragraph is a
# container that holds as many as the writer cares to stuff into it.
#
# Threshold, measured rather than guessed. Over 113 prose runs in the session that prompted this,
# `median = 20` words and `mean = 61`: ordinary mid-turn narration is short by nature and is never
# at risk. Every run the user objected to sat in a distinct band, 228-323 words.
#
# Set to 120 first, and 120 was too loose -- corrected the same day. It was chosen as "between the
# two populations", which is the right method for catching the 281-word case and the wrong one for
# the actual complaint. With 120 in force the next several answers landed at roughly 110-130 words:
# comfortably under the ceiling, still "normal human reading length", and the user objected again in
# exactly the same terms. A budget set just below the worst offenders does not shorten anything, it
# just relocates the median to sit under the bar.
#
# 60 is the measured mean of this session's own prose runs, so it is not a guess either -- it is the
# length the agent already writes when it is not padding. It leaves room for one answer and its
# proof and nothing else, which is the point: past that, content has to move into a table or wait
# for its own turn.
#
# What is MEASURED: the same longest contiguous prose run `last_assistant_wall_of_text` measures,
# via the same `scripts/cupcake_turn_scan` helpers, so the two signals can never disagree about what
# prose is. Structure -- fenced code, tables, list items, headings -- is excluded there and so is
# excluded here: the objection is to reading, and structure is scanned.
#
# Emits  PROSEWORDS:<word count>:<first 60 chars of the offending run>  ; empty when within budget.
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

BUDGET = int(os.environ.get("CUPCAKE_PROSE_WORD_BUDGET", "60"))

path = scan.latest_transcript()
if not path:
    sys.exit(0)

# last_text_turn, not turns[-1]: at UserPromptSubmit the new prompt may already be on disk, which
# leaves an empty trailing turn. The turn being judged is the last one that actually said something.
turn = scan.last_text_turn(scan.split_turns(scan.load_events(path)))
if turn is None:
    sys.exit(0)

worst, words = "", 0
for run in turn.text_runs:
    paragraphs = scan.prose_paragraphs(run)
    if not paragraphs:
        continue
    run_words = sum(len(p.split()) for p in paragraphs)
    if run_words > words:
        worst, words = paragraphs[0], run_words

if words > BUDGET:
    print("PROSEWORDS:%d:%s" % (words, " ".join(worst.split())[:60]))
PY
