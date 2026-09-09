#!/usr/bin/env bash
# Cupcake signal: last_assistant_described_next_step
#
# Consumed by:
#   * no_described_next_step (Stop): halts turn-end when the closing message names a concrete next
#     action the agent could have begun in that turn, and the turn begins none of it.
#
# Why this exists (user directive 2026-09-08, in their words):
#   "I feel like I probably should have a rego policy that catches this prose and encourages you to
#    do the thing you talked about."
# The instance: after seven runs proved that the writable data of `ersc.dll` does not hold the live
# Seamless session pointer, a turn closed with "The next approach follows from the disassembly rather
# than from hope: ... the address-space walk I deleted an hour ago is exactly the right tool pointed
# at the right object this time. I'd rather say that plainly than dress up a seventh variation on
# searching the one place we now know it isn't." -- and built none of it. `AGENTS.md` had banned this
# in prose since July ("Before asking the user to proceed, ask yourself 'do I already know how to
# proceed?' If yes, do not respond -- just proceed"), and prose did not stop it. Advisory text is
# advisory; only a Stop hook that refuses the stop is binding.
#
# The sibling guard `last_assistant_unexecuted_promise.sh` does not reach this shape, and that is the
# gap being closed rather than a duplicate. Its `OPENER_RE` requires a first-person commitment
# ("I'll", "I'm going to", "let me"). The failing sentence commits to no one: "the next approach
# follows", "the address-space walk is the right tool". Naming the step impersonally is the more
# comfortable way to leave it undone, since nobody was ever said to be doing it.
#
# The violation is a conjunction of five facts. All five must hold, and any one missing is fine:
#   1. the turn names a concrete next action in a forward-looking prescription ("the next step is
#      reading X", "the right approach would be to walk the address space", "what comes next is Y").
#      The named thing must carry a work word, so a prescription with no work in it stays silent;
#   2. no substantive tool call follows the sentence that named it -- the turn described the step and
#      then did not begin it. A turn that says it and then runs it is the correct shape and is never
#      touched;
#   3. the turn does not hand the ball to the user -- no question, no either/or fork, no "want me
#      to", no "once you have X";
#   4. the turn does not state a genuine blocker -- nothing about needing sudo, a live game run,
#      approval for a destructive step, a guard refusal, or a tool that is absent;
#   5. no live background work is carrying it.
# Emitted as one facts line, so the observation lives here and the rule lives in the policy:
#   NEXTSTEPFACTS|nextstep=<clause>|acted=0|1|blocked=0|1|handoff=0|1|carried=0|1
# Empty output when the turn named no next step at all.
#
# Biased hard toward staying quiet, and the bias was measured rather than asserted. A false positive
# blocks a legitimate turn -- a genuine fork, a real blocker, a destructive step awaiting approval --
# which costs more here than a miss. Replayed over 2,142 real turns from 70 session transcripts by
# `scripts/audit-described-next-step-false-positives.py`, it fires on 9 (0.4%), and the sentence it
# quotes back is a described-and-abandoned next step in each. Four narrowings earned their place in
# that replay, each removing a class of hit that was not this defect:
#   * a prescription used as an object or inside a subordinate clause is not a prescription. "on the
#     next run:", "the part that decides the next step:", "a suspect that the next run will clear" --
#     the word ahead of the phrase demotes it, and `PRECEDING_DEMOTERS` drops all three;
#   * a copula is required after the phrase and a bare colon is refused, which is what separates "the
#     next step is reading X" from "one delta to watch on the next run: ...";
#   * "what is left is" and "is the right move" were dropped outright. Both read as definition or as
#     justification for a move being made now, and both fired on turns that were doing the work;
#   * a step contingent on something landing or merging is not startable, so it is exempt.
# Fenced code, backticked spans and double-quoted spans are stripped before matching, so quoting the
# ban -- this file, the policy, a test fixture, a report about the guard -- cannot trip it.
#
# The shared half of the classification (transcript discovery, turn bucketing, substantive work
# against a status peek, blocked-on-user phrasing, live background work) comes from
# `scripts/cupcake_turn_scan.py`, the same module the idle-hold and unexecuted-promise signals use, so
# the guards cannot drift into disagreeing about the same turn. Fail-open (empty output) on any error.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
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
events = scan.load_events(path)
turn = scan.last_text_turn(scan.split_turns(events))
if turn is None:
    sys.exit(0)

# A turn that kept going past its last prose did not end on that prose. At Stop the last block is
# text by construction; an interrupted turn can end on a tool call, and that turn acted.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)


def scrub(text):
    """Drop fenced code, backticked spans and double-quoted spans before prose matching.

    Quoting the ban is not committing it. Single quotes are left alone: the phrases carry
    apostrophes. This is also what lets a report about this guard describe its own triggers without
    halting the turn that describes them, provided the triggers are written as quotations.
    """
    text = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]*"', " ", text)


# --- (1) a forward-looking prescription of a next action ---------------------------------------
# Each alternative is future-facing on its own, so no separate tense test is needed. The bare
# prescription "the fix is to add a null check" is absent on purpose: it is the ordinary way to
# report a fix that was just made, and including it turned completed work into a violation.
PATTERNS = {
    # "the next step is reading X", "the honest next test is the user path", "the next probe goes at
    # the binding layer". An adjective or two may sit before "next".
    "next_noun": r"\b(?:the|a|an|my|our)\s+(?:\w+\s+){0,2}next\s+"
                 r"(?:approach|step|steps|move|thing|test|experiment|run|probe|attempt|action|check"
                 r"|pass|iteration|lead|angle|task|fix)\s+"
                 r"\b(?:is|are|would|will|should|follows|comes|goes|has\s+to|needs\s+to)\b",
    # "the right approach would be", "a better test will be".
    "would_be": r"\b(?:the|a|an|my|our)\s+(?:\w+\s+){0,2}"
                r"(?:fix|answer|solution|approach|move|play|tool|way|path|route|test|check|proof"
                r"|experiment|probe|lead|plan)\s+(?:here\s+|now\s+|then\s+)?(?:would|will|should)\s+be\b",
    "what_comes_next": r"\bwhat\s+(?:comes\s+next|needs\s+to\s+happen|has\s+to\s+happen)\s+is\b",
    "way_forward": r"\bthe\s+(?:way|path|route)\s+forward\s+is\b",
    # "would be to walk the address space".
    "would_be_to": r"\bwould\s+be\s+to\s+(?:go\s+|then\s+|just\s+|actually\s+|first\s+)?[a-z]+\b",
}
COMPILED = {name: re.compile(rx, re.IGNORECASE) for name, rx in PATTERNS.items()}

# The word directly ahead of the phrase can make it an object or a subordinate clause instead of a
# prescription, and then it prescribes nothing. Measured cases: "on the next run:", "the part that
# decides the next step:", "a live suspect that the next run will confirm or clear".
PRECEDING_DEMOTERS = set("""
on in for at by during before after with to from of about within against under over
that which whether while unless because since though although if when
decides decided decide names named name mentions mentioned describes described
""".split())

# Work words. A prescription with none of these names no action a tool call could begin, so it is
# left alone: "the next step is obvious", "the way forward is clear".
ACTION_WORDS = set("""
run rerun rerunning running launch relaunch build rebuild compile check recheck reread recapture
verify validate test retest fix patch repair correct add remove delete write rewrite record save
update edit create commit push pull rebase merge open file implement apply revert restore capture
read inspect investigate search hunt trace disassemble disassembly decompile hook probe measure
measurement diff compare stage package install generate regenerate wire refactor rename move copy
kill restart retry send post reply document extend migrate port bump pin split tag publish deploy
sync fetch download upload convert parse dump scan sweep grep find count render screenshot audit
lint format benchmark profile instrument emit register enable disable toggle configure tune adjust
resolve ship harden delegate dispatch spawn walk crawl breakpoint xref xrefs disasm harness script
gate query lookup snapshot experiment repro reproduce bisect isolate narrow enumerate map mapping
press drive navigate inject searching walking tracing probing reading building running hooking
""".split())

WORD_RE = re.compile(r"[A-Za-z][A-Za-z'\-]*")


def sentence_span(text, pos):
    """The (start, end) offsets of the sentence containing `pos`."""
    start = 0
    for m in re.finditer(r"[.!?\n]", text[:pos]):
        start = m.end()
    end = len(text)
    for m in re.finditer(r"[.!?\n]", text[pos:]):
        end = pos + m.start() + 1
        break
    return start, end


def sentence_around(text, pos):
    start, end = sentence_span(text, pos)
    return text[start:end]


def demoted(text, m):
    words = WORD_RE.findall(text[max(0, m.start() - 40):m.start()])
    return bool(words) and words[-1].lower() in PRECEDING_DEMOTERS


def concrete(text, m):
    window = sentence_around(text, m.start()) + " " + text[m.end():m.end() + 140]
    return any(w.lower().strip("-'") in ACTION_WORDS for w in WORD_RE.findall(window))


def clause(text, m):
    """The offending sentence, collapsed and clipped, for quoting back at the agent.

    Clipped around the match rather than from the start of the sentence. A long markdown line, or a
    sentence whose start was misplaced by an abbreviation such as `AGENTS.md`, otherwise produces a
    120-character quote that stops before the phrase it is quoting, and the correction then names a
    step the agent cannot find in it. Pipes are replaced, because the facts line is pipe-delimited and
    a sentence lifted out of a markdown table carries them.
    """
    start, end = sentence_span(text, m.start())
    collapsed = " ".join(text[start:end].split())
    if len(collapsed) <= 120:
        return collapsed.replace("|", "/")
    # Keep a little of the run-up so the sentence reads, then the phrase and what follows it.
    window_start = max(start, m.start() - 40)
    clipped = " ".join(text[window_start:window_start + 130].split())
    prefix = "..." if window_start > start else ""
    return (prefix + clipped + " ...").replace("|", "/")


# Scan every prose block, keep the last qualifying match, and remember which block held it. The block
# index is what decides fact 2: a step named in block two and run before the turn ended is covered by
# the tool call that follows it, while the same sentence in the closing block has nothing after it.
hit = None
hit_index = -1
for index, (kind, value) in enumerate(turn.blocks):
    if kind != "text":
        continue
    scrubbed = scrub(value)
    for rx in COMPILED.values():
        for m in rx.finditer(scrubbed):
            if demoted(scrubbed, m) or not concrete(scrubbed, m):
                continue
            hit, hit_index = clause(scrubbed, m), index
if not hit:
    sys.exit(0)

# --- (2) something began it ---------------------------------------------------------------------
acted = any(
    kind == "tool" and scan.block_is_substantive(block)
    for kind, block in turn.blocks[hit_index + 1:]
)

message = scrub(turn.text)

# --- (3) the ball was handed to the user --------------------------------------------------------
# A question mark anywhere in the closing message exempts the turn. That is broad on purpose: a
# genuine fork is exactly what the agent is allowed to stop for, and this rule must never gag one.
HANDOFF_RE = re.compile(
    r"\?"
    r"|\bonce\s+(?:you|your|the\s+user)\b"
    r"|\bwhen\s+(?:you|your|the\s+user)\b"
    r"|\bafter\s+(?:you|your|the\s+user)\b"
    r"|\bif\s+(?:you|your|the\s+user)\b"
    r"|\byou'?(?:ll|d)\s+(?:need|have)\s+to\b|\byou\s+(?:need|have)\s+to\b"
    r"|\bneed\s+you\s+to\b|\bneed\s+the\s+user\s+to\b"
    r"|\blet\s+me\s+know\b|\btell\s+me\b|\bping\s+me\b"
    r"|\byour\s+call\b|\bsay\s+the\s+word\b|\bup\s+to\s+you\b"
    r"|\bwant\s+me\s+to\b|\bshall\s+i\b|\bshould\s+i\b"
    r"|\bwould\s+you\s+(?:like|prefer|rather)\b"
    r"|\bmy\s+recommendation\b|\bi\s+recommend\b"
    r"|\bnext\s+(?:session|turn)\b|\bre-?initiate\b|\bre-?ask\b"
    # A stated dependency on a user action plus a commitment to act on its result: "invade now and
    # I'll read the log". Lifted verbatim from `BLOCKED_RES` in
    # `.cupcake/signals/last_assistant_stall_on_friction.sh` so the two guards agree about the same
    # sentence rather than each inventing a rule for it.
    r"|\b(?:and|then|once|after|when)\b[^.\n]{0,60}\bi'?ll\b"
    r"|\bonly\s+you\s+can\b",
    re.IGNORECASE,
)
handoff = bool(HANDOFF_RE.search(message)) or scan.blocked_on_user(message)

# --- (4) a genuine blocker was stated -----------------------------------------------------------
# Reporting a blocker is required behaviour, and a step waiting on approval or on something landing
# is not a step that could have been begun. Both exempt.
BLOCKER_RE = re.compile(
    r"\bblocked\b|\bblocker\b|\bcannot\s+proceed\b"
    r"|\bcan(?:no|')?t\s+(?:proceed|run|test|verify|build|launch|start)\b"
    r"|\brequires?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game|physical|credentials|network)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game\s+running)\b"
    r"|\bnot\s+(?:available|installed|possible|permitted|allowed|reachable)\b"
    r"|\b(?:guard|policy|cupcake|opa)\s+(?:denied|blocked|refused|refuses)\b"
    r"|\bdestructive\b|\birreversible\b"
    r"|\b(?:when|once|after)\s+(?:they|it|those|these|the\s+\w+)\s+"
    r"(?:land|lands|merge|merges|complete|completes|finish|finishes|is\s+merged|are\s+merged)\b",
    re.IGNORECASE,
)
blocked = bool(BLOCKER_RE.search(message))

# --- (5) live background work is carrying it ----------------------------------------------------
# Something running is not by itself cover, for the reason the promise guard records: a game session
# the user is inspecting can be up for an hour, and an unrelated next step is deferred behind it, not
# carried by it. It covers the step when the step waits on its result, or when the live thing is a
# watcher this turn started.
WAITS_ON_RESULT_RE = re.compile(
    r"\b(?:when|once|after|as\s+soon\s+as|the\s+moment|while|until)\b[^.\n]{0,60}?"
    r"\b(?:it|that|they|its|their|the\s+\w+)\b"
    r"|\b(?:its|their)\s+(?:output|log|logs|result|results|findings?|verdict)\b"
    r"|\bwhatever\s+(?:it|they)\s+\w+"
    r"|\bif\s+(?:it|that|they)\s+\w+",
    re.IGNORECASE,
)
live = scan.live_background_work(events)
carried = bool(live) and (live.watcher or bool(WAITS_ON_RESULT_RE.search(message)))

sys.stdout.write(
    "NEXTSTEPFACTS|nextstep=%s|acted=%d|blocked=%d|handoff=%d|carried=%d"
    % (hit, int(acted), int(blocked), int(handoff), int(carried))
)
PY
