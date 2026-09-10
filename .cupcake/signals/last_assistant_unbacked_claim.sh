#!/usr/bin/env bash
# Cupcake signal: last_assistant_unbacked_claim
#
# Consumed by:
#   * no_unbacked_claim (Stop): halts turn-end when the closing prose claims an artifact was
#     built/changed and nothing in the turn wrote a file.
#
# Why this exists (user, 2026-08-23). The turn opened "Build a conformance gate, because 'read the
# reference first' is a note and notes are advisory", described a `reference-implementations.toml`
# and a check in check.sh -- and shipped a `bd remember` call and nothing else. No gate. No file.
# The user's reply: "You didn't. You recorded a beads memory. That absolutely is not even remotely
# close to a conformance gate." A memory costs one tool call and feels like delivery, which is
# exactly why it substitutes for delivery.
#
# Sibling, not duplicate, of last_assistant_unexecuted_promise. That one catches the future tense
# ("I'll build it") ending in nothing. This one catches the PAST/PERFECT tense ("I built it",
# "I've wired it in", "I added the check") when nothing was built. Opposite failure, same hole.
#
# The violation is a conjunction of three facts. Any one missing and the signal stays silent:
#   1. the final prose block contains a first-person completion claim -- "I built/added/created/
#      wrote/wired/landed/shipped/implemented/patched/updated/removed <x>";
#   2. the claim's object is a REPO ARTIFACT: a path-like token (scripts/..., crates/..., *.py,
#      *.rego, *.rs, *.toml, *.sh) or one of gate/check/hook/policy/test/script/guard/selftest;
#   3. Nothing in the turn wrote a file -- no Edit/Write/NotebookEdit tool_use, and no Bash call
#      carrying a write construct (redirect, tee, sed -i, heredoc-to-python, cp/mv/install, patch,
#      git apply). A `bd remember` on its own is explicitly not a write: it is the substitution
#      this guard exists to catch.
#
# Biased hard toward not firing, like its sibling -- a guard that cries wolf gets ignored.
#   * only the final prose block is scanned;
#   * quoted, backticked and fenced spans are stripped first, so quoting this file, the policy, or
#     a test fixture cannot trip it;
#   * negations and disclaimers are honoured: "I have not built it", "no gate exists", "I did not
#     write", "nothing was created" suppress the hit outright -- an honest confession of absence
#     is the behaviour being asked for, and must never be punished;
#   * a claim about the game, a run, or an external thing ("the DLL loaded", "the import granted
#     129 items") is not a repo-artifact claim and is not matched;
#   * the pronoun must open its clause. "the gate I wrote", "the function I hooked", "an importer I
#     built" are relative clauses that presuppose the artifact and then say something else about it,
#     which is ordinary reporting prose rather than a claim of creation. Measured over 2,370 real
#     turns the first time this signal could fire at all: ten hits, nine of them that shape. See
#     `_opens_a_clause` in scripts/cupcake_unbacked_claim.py.
#
# What it deliberately does not catch, stated so nobody mistakes its silence for proof: an
# imperative recommendation that reads as delivered ("Build a conformance gate.") has no first-
# person verb and will pass. Matching bare imperatives would fire on every legitimate
# recommendation, which would make the guard noise. That gap is real and is not closed here.
#
# Emitted as  UNBACKED:<the offending clause>  ; empty when the turn is clean. Fail-open on error.
#
# Silently inert from the day it landed until 2026-09-09. The call below read `turn.events`, and a
# `scripts/cupcake_turn_scan.Turn` has no such attribute -- it carries `blocks`, an ordered stream of
# ("text", str) and ("tool", block) pairs. Every invocation raised AttributeError, the `2>/dev/null
# || true` on the interpreter swallowed it, the signal printed nothing, and the policy read that as a
# clean turn. Its six opa tests were green throughout, because they feed the policy a signal string
# directly and never run this file. Same class as the `sprintf` defect in bd
# cupcake-wasm-has-no-sprintf-so-the-rule-silently-never-fires-2026-09-09: a guard can be tested at
# every layer except the one that decides whether it fires at all. The end-to-end proof that it now
# halts is `scripts/test-cupcake-stop-guards.py`, which drives the real hook over a fixture.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, re, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
    import cupcake_unbacked_claim as claim
except Exception:
    sys.exit(0)  # fail open: a missing helper must never wedge a session

path = scan.latest_transcript()
if not path:
    sys.exit(0)
turn = scan.last_text_turn(scan.split_turns(scan.load_events(path)))
if turn is None:
    sys.exit(0)
# The closing prose is the last contiguous text run -- consecutive text blocks with no tool call
# between them -- which is what "only the final prose block is scanned" means for a turn whose prose
# arrives in several blocks. The classifier's other argument wants raw transcript events, because
# `turn_wrote_a_file` walks message.content looking for tool_use blocks; a Turn keeps those blocks
# without their carrier events, so hand it one synthetic carrier holding the turn's whole tool
# stream. Reconstructing the shape here keeps `Turn` free of an `events` field that the four other
# signals importing cupcake_turn_scan would have to carry for no reason.
runs = turn.text_runs
if not runs:
    sys.exit(0)
events = [{"message": {"content": [block for kind, block in turn.blocks if kind == "tool"]}}]
hit = claim.offending_claim(runs[-1], events)
if hit:
    print("UNBACKED:" + hit)
PY
