#!/usr/bin/env bash
# Cupcake signal: last_assistant_deferred_evidence_read
#
# Consumed by:
#   * no_deferred_evidence_read (Stop): halts turn-end when the closing message names a concrete
#     piece of evidence that already exists as holding the answer, and the turn never opened it.
#
# The shape, verbatim from the turn that prompted it (2026-09-09):
#
#     ... and the next measurement is whether the DLL is using the menu object it now captures
#     (`r14`) for the re-invade gate or still falling back to the scan -- which its own log answers,
#     so I am reading that next.
#
# The answer was in a file on disk. Naming the read instead of doing it costs the user a round trip
# worth zero information: they learn only that the agent knows where to look.
#
# Why this is not one of the neighbours. `last_assistant_unexecuted_promise` keys on a first-person
# intention verb ("I'll", "I'm going to", "let me"); `last_assistant_described_next_step` keys on a
# forward-looking prescription whose noun is in a fixed list that does not contain "measurement";
# `last_assistant_diagnosis_without_fix` keys on a defect named without an edit. This sentence is
# phrased as an observation about a file, so all three read past it.
#
# Why this is not a copy of its closest neighbour either. Between the assignment and this file,
# `last_assistant_diagnosis_without_fix` grew an `unread` fact covering the same defect, reported
# under `ER-EFFECTS-NO-PROMISSORY-CLOSER`. That arm is the one that convicts the verbatim sentence
# above, and it stays the one: this signal exists for the deferrals its narrower artifact model
# declines, and the policy that reads it yields whenever that arm recognised the sentence at all.
# Four measured declines, each replayed through `cupcake eval` before this file was written:
#
#   1. `er-quickload-autoload-debug.log will say which` -- a modal between the artifact and the verb
#      of telling, which its pattern (noun immediately followed by `says|answers|tells|holds|has`)
#      cannot span.
#   2. `What remains is to read the newest er-invasion-warp log` -- its weak-artifact pattern needs
#      the determiner adjacent to the noun, and an adjective in between hides the artifact.
#   3. `the run artifact under er-me3-runs will tell us which one ran` -- it spells this shape as the
#      literal `that will tell us`, so a named artifact in the subject position misses.
#   4. `reading that log now` -- it spells this as `reading that (next|now)`, so a noun between the
#      pronoun and the adverb misses.
#
# The violation is a conjunction of six facts. All must hold, and any one missing keeps it quiet:
#   1. the closing text run contains a deferral phrase and, in the same sentence, names an artifact:
#      a path, a filename, an address, a symbol, a run id, or an artifact noun under a determiner;
#   2. no tool call in the turn touched it. Keyed on strong tokens only -- a path, a filename, an
#      address, a hyphenated repo name -- because almost every command in this repo mentions some
#      log, and letting the bare word clear the check would clear the instance above;
#   3. the evidence exists. Deferring to a log a run has yet to write is a plan, not a skipped read,
#      and this repo defers to future evidence constantly;
#   4. the read does not need the user -- no question, no observation only they can make;
#   5. no genuine blocker was stated;
#   6. no live background work is carrying it.
#
# Emitted as one facts line, so the observation lives here and the verdict lives in the policy:
#   DEFERFACTS|deferral=<clause>|consulted=0|1|future=0|1|userneed=0|1|blocked=0|1|carried=0|1
# Empty output when the closing message deferred to no existing evidence.
#
# Fenced code, backticked spans and double-quoted spans are blanked before phrase matching, so
# quoting the ban -- this file, the policy, a fixture, a report about the guard -- cannot trip it.
# They are blanked to spaces rather than removed, which keeps every offset intact so the artifact
# scan can run over the raw sentence: an artifact name is usually written in backticks, and stripping
# the span would delete the very thing the rule asks about.
#
# The shared half of the classification (transcript discovery, turn bucketing, blocked-on-user
# phrasing, live background work) comes from `scripts/cupcake_turn_scan.py`, the module the
# idle-hold, unexecuted-promise and described-next-step signals already read, so the guards cannot
# drift into disagreeing about the same turn. Fail-open (empty output) on any error.
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

closing_raw = turn.blocks[turn.last_text_index][1]
if not isinstance(closing_raw, str) or not closing_raw.strip():
    sys.exit(0)


def blank(match):
    """Replace a span with the same number of characters of whitespace, newlines preserved."""
    return re.sub(r"[^\n]", " ", match.group(0))


def scrub(text):
    """Blank fenced code, backticked spans and double-quoted spans, keeping every offset.

    Quoting the ban is not committing it, so those spans are excluded from phrase matching. They are
    blanked rather than deleted because the artifact scan reads the raw text at the same offsets, and
    an artifact name is nearly always written inside backticks.
    """
    text = re.sub(r"```.*?```", blank, text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", blank, text)
    return re.sub(r'"[^"]*"', blank, text)


# --- (1) a deferral to evidence -----------------------------------------------------------------
# Each alternative asserts that an answer is available somewhere the turn did not go. The last two
# families are the announcement of an imminent read, which is the same deferral with the artifact
# left implicit; they still need an artifact in the sentence to fire.
ARTIFACT_NOUNS = (
    r"log|logs|logfile|dump|dumps|trace|traces|transcript|disassembly|binary|output|artifact"
    r"|artifacts|telemetry|report|manifest|capture|record|records|file|listing"
)
# A short gap inside one sentence. A full stop is allowed only when no whitespace follows it, which
# is what lets `target/er-me3-runs/latest.log` sit between the verb and the adverb in "I am reading
# target/er-me3-runs/latest.log next": excluding every dot excluded exactly the filenames the rule is
# about, and the two phrasings that carried one went silent.
GAP = r"(?:[^!?;\n.]|\.(?!\s))"
# The announcement of an imminent read has to end on its adverb. Without the anchor it swallowed a
# report of a read already under way -- "Reading it now - `ersc.dll` is on disk and the three frames
# are exact" -- where the rest of the sentence carries what the file said. The sibling guard measured
# and deleted the same false positive from its own participle pattern.
ADVERB_END = r"(?=\s*(?:[.!?;,]|$))"
DEFERRAL_RE = re.compile(
    # "the next measurement is ...", "our honest next check would be ..."
    r"\b(?:the|a|an|my|our)\s+(?:\w+\s+){0,2}next\s+"
    r"(?:measurement|measure|step|question|thing|check|read|move|probe|action|task)\s+"
    r"(?:is|are|would\s+be|will\s+be|has\s+to\s+be)\b"
    # "what remains is to read ...", "what is left is to check ..."
    r"|\bwhat\s+(?:remains|is\s+left|is\s+missing)\s+is\s+to\b"
    # "the remaining question is ...", "the one open question is ..."
    r"|\bthe\s+(?:\w+\s+){0,1}(?:remaining|open|only|last|outstanding)\s+"
    r"(?:question|unknown|thing|gap)\s+(?:is|remains)\b"
    # "which its own log answers", "which the newest dump already records"
    r"|\bwhich\s+(?:its\s+own\s+|the\s+|that\s+|this\s+|our\s+)?(?:\w+\s+){0,3}?"
    r"(?:" + ARTIFACT_NOUNS + r")\s+(?:already\s+)?"
    r"(?:answers|records|says|shows|settles|holds|has|tells)\b"
    # "the log will say", "er-me3-runs will tell us", "the trace should show"
    r"|\b(?:" + ARTIFACT_NOUNS + r")\b" + GAP + r"{0,40}?\b(?:will|would|should)\s+"
    r"(?:say|tell|show|answer|record|reveal|settle|confirm|name|report|decide)\b"
    # "that will tell us", "it would settle which branch ran"
    r"|\b(?:that|this|it)\s+(?:will|would|should)\s+"
    r"(?:say|tell|show|answer|settle|confirm|decide)\b"
    # "its own log records that", "the trace at 0x140d39df0 already holds that". The object has to
    # end the clause: a pronoun with more sentence after it is a report of what the artifact said
    # ("the log even says it resolved WITHOUT hooking Seamless"), which is the wanted behaviour and
    # was the last surviving false positive in the transcript replay.
    r"|\b(?:its|their|the|that|this|our)\s+(?:own\s+)?(?:\w+\s+){0,3}?"
    r"(?:" + ARTIFACT_NOUNS + r")\b" + GAP + r"{0,30}?\b"
    r"(?:records|answers|says|holds|has|shows|tells)\s+"
    r"(?:that|it|this|which|the\s+answer)\b" + ADVERB_END +
    # "so I am reading that next", "I'm tailing the log now"
    r"|\b(?:i\s+am|i'm)\s+"
    r"(?:reading|re-?reading|checking|rechecking|opening|tailing|grepping|looking\s+at)\b"
    + GAP + r"{0,40}?\b(?:next|now)\b" + ADVERB_END +
    # "reading that log now", "checking the artifact next"
    r"|\b(?:reading|re-?reading|checking|rechecking|opening|tailing|grepping)\s+"
    r"(?:that|it|this|the|its|their|our)\b" + GAP + r"{0,40}?\b(?:next|now)\b" + ADVERB_END +
    # "so that is what I check next"
    r"|\bthat\s+is\s+what\s+i\s+(?:check|read|measure|look\s+at)\s+next\b"
    # "the answer is already in the log", "the answer lives on disk"
    r"|\bthe\s+answer\s+(?:is|lives|sits)\s+(?:already\s+)?(?:in|on)\b",
    re.IGNORECASE,
)

# A path, a filename, an address, a reverse-engineering symbol, an oracle field, a run id, or a
# hyphenated repo name: something a tool call can be shown to have opened.
STRONG_RE = re.compile(
    r"\b[\w.~-]*/[\w./~-]+\b"
    r"|\b[\w-]+\.(?:log|rs|py|rego|toml|sh|json|jsonl|txt|md|bin|dll|exe|csv|yml|yaml)\b"
    r"|\b0x[0-9a-fA-F]{4,}\b"
    r"|\bFUN_[0-9a-fA-F]+\b"
    r"|\boracle_\w+\b"
    r"|\bbr-\d{8}-\d{6}\b"
    r"|\b[a-z][a-z0-9]*(?:-[a-z0-9]+){2,}\b"
)

# The artifact named by kind rather than by name, with adjectives tolerated between the determiner
# and the noun. Enough to say evidence exists; never enough to say it was consulted.
WEAK_RE = re.compile(
    r"\b(?:its|their|the|that|this|our|[\w-]+'s)\s+(?:own\s+)?(?:\w+\s+){0,3}?"
    r"(?:" + ARTIFACT_NOUNS + r")\b",
    re.IGNORECASE,
)


def strong_tokens(text):
    """Strong artifact tokens worth searching tool inputs for.

    A token has to be specific enough that finding it in a command means that command opened this
    artifact. Bare English caught by the path alternative ("and/or") and short fragments are dropped,
    and what survives carries a separator, a digit, or two hyphens.
    """
    out = set()
    for token in STRONG_RE.findall(text):
        if len(token) <= 4:
            continue
        if not re.search(r"[/._]|\d|-.*-", token):
            continue
        out.add(token)
    return out


# Evidence that has to be produced before it can be read. Deferring to it is a plan waiting on a run,
# not a skipped read, and this repo does it constantly and legitimately.
FUTURE_RE = re.compile(
    r"\b(?:the\s+next|a\s+new|another|this\s+next|the\s+following)\s+"
    r"(?:run|runs|launch|build|boot|attempt|probe|session|invasion|capture|pass|invade)\b"
    r"|\bonce\s+(?:it|they|that|you|the\s+\w+)\b"
    r"|\bafter\s+(?:the\s+)?(?:next\s+)?(?:run|rerun|build|rebuild|launch|relaunch|boot|it|that)\b"
    r"|\bwhen\s+(?:you|it|they|the\s+\w+)\s+\w+"
    r"|\bnext\s+time\b"
    r"|\bwill\s+(?:print|write|emit|record|produce|generate|create|land|appear|exist)\b"
    r"|\bhas\s+to\s+(?:happen|run|land|finish|build)\s+first\b"
    r"|\bdoes\s*n[o']?t\s+exist\s+yet\b"
    r"|\bnot\s+(?:been\s+)?(?:written|produced|generated|captured|emitted)\s+yet\b"
    r"|\bneeds?\s+a\s+(?:run|rerun|launch|relaunch|rebuild|build|boot|capture|game)\b"
    r"|\bis\s+(?:building|compiling|running|launching|booting)\b"
    # Each of the five below was a false positive in the transcript replay, and each names evidence a
    # run has still to produce rather than a file sitting on disk.
    r"|\bthat\s+run'?s\b"
    r"|\b(?:is|are)\s+armed\b"
    r"|\b(?:on|at|during|by)\s+the\s+(?:first|next)\b"
    r"|\bthe\s+moment\s+you\b|\bwhile\s+you\b|\bas\s+you\b"
    r"|\bto\s+(?:rebuild|relaunch|re-?run|rerun|launch|build|boot|capture)\b"
    r"|\bevidence\s+(?:that\s+)?is\s+missing\b",
    re.IGNORECASE,
)

# The read needs something only the user has. A question mark anywhere exempts the turn, the way the
# described-next-step guard does it: a genuine fork is exactly what a turn is allowed to stop for,
# and no guard may gag one.
USERNEED_RE = re.compile(
    r"\?"
    r"|\btell\s+me\s+(?:what|whether|if|which|how)\b"
    r"|\bwhat\s+(?:do|did|does)\s+you\s+see\b"
    r"|\blet\s+me\s+know\b|\bping\s+me\b"
    r"|\bonly\s+you\s+can\b|\bneed\s+you\s+to\b|\bneed\s+the\s+user\s+to\b"
    r"|\byour\s+call\b|\bup\s+to\s+you\b|\bsay\s+the\s+word\b"
    r"|\bwant\s+me\s+to\b|\bshall\s+i\b|\bshould\s+i\b"
    r"|\bon\s+your\s+(?:screen|end|side|machine)\b"
    r"|\byou'?(?:ll|d)\s+(?:need|have)\s+to\b|\byou\s+(?:need|have)\s+to\b",
    re.IGNORECASE,
)

# Reporting a blocker is required behaviour, and evidence that cannot be opened is not evidence that
# was skipped. Lifted from the sibling guards so the layer agrees about the same sentence.
BLOCKER_RE = re.compile(
    r"\bblocked\b|\bblocker\b|\bcannot\s+proceed\b"
    r"|\bcan(?:no|')?t\s+(?:proceed|run|read|open|test|verify|build|launch|start)\b"
    r"|\brequires?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game|physical|credentials|network)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|credentials|a\s+login)\b"
    r"|\bnot\s+(?:available|installed|possible|permitted|allowed|reachable|readable)\b"
    r"|\b(?:guard|policy|cupcake|opa)\s+(?:denied|blocked|refused|refuses)\b"
    r"|\bpermission\s+denied\b",
    re.IGNORECASE,
)

# Live background work covers the read when the turn waits on its result, or when the live thing is a
# watcher this turn started. A game session the user is inspecting carries nothing by itself.
WAITS_ON_RESULT_RE = re.compile(
    r"\b(?:when|once|after|as\s+soon\s+as|the\s+moment|while|until)\b[^.\n]{0,60}?"
    r"\b(?:it|that|they|its|their|the\s+\w+)\b"
    r"|\b(?:its|their)\s+(?:output|log|logs|result|results|findings?|verdict)\b"
    r"|\bwhatever\s+(?:it|they)\s+\w+"
    r"|\bif\s+(?:it|that|they)\s+\w+",
    re.IGNORECASE,
)


# Sentence boundaries. Semicolons end a sentence here, matching the sibling guards: a closing line
# often chains an observation and a deferral with one, and the deferral is its own claim. A full stop
# counts only when whitespace or the end of the run follows it, which is what keeps
# `er-quickload-autoload-debug.log` one token instead of two sentences -- the failure the sibling
# guard's own comment records as a misplaced sentence start, reproduced here before it was fixed.
BOUNDARY_RE = re.compile(r"[!?;\n]|\.(?=\s|\Z)")


def sentence_span(text, pos):
    """The (start, end) offsets of the sentence containing `pos`.

    Scanned over the whole run rather than over `text[:pos]`: the end-of-string lookahead in
    `BOUNDARY_RE` fires at the artificial end of a slice, which turned the dot in
    `er-quickload-autoload-debug.log` into a sentence break and left `log will say which` as the
    sentence -- a fragment with no artifact in it, so the guard went quiet on the shape it exists for.
    """
    start = 0
    end = len(text)
    for m in BOUNDARY_RE.finditer(text):
        if m.end() <= pos:
            start = m.end()
        elif m.start() >= pos:
            end = m.start() + 1
            break
    return start, end


def quote(text, start, end, pos):
    """The offending sentence, collapsed and clipped, for quoting back at the agent."""
    collapsed = " ".join(text[start:end].split())
    if len(collapsed) <= 160:
        return collapsed.replace("|", "/")
    window_start = max(start, pos - 40)
    clipped = " ".join(text[window_start:window_start + 170].split())
    prefix = "..." if window_start > start else ""
    return (prefix + clipped + " ...").replace("|", "/")


def tool_input_text(block):
    try:
        raw = block.get("input") or {}
    except AttributeError:
        return ""
    return " ".join(v for v in (raw.values() if isinstance(raw, dict) else []) if isinstance(v, str))


NOUN_RE = re.compile(r"\b(?:" + ARTIFACT_NOUNS + r")\b", re.IGNORECASE)

scrubbed = scrub(closing_raw)
hit = None
hit_tokens = set()
hit_nouns = set()
for m in DEFERRAL_RE.finditer(scrubbed):
    start, end = sentence_span(scrubbed, m.start())
    sentence_raw = closing_raw[start:end]
    tokens = strong_tokens(sentence_raw)
    named = bool(tokens) or bool(WEAK_RE.search(scrub(sentence_raw)))
    if not named:
        continue
    hit = quote(closing_raw, start, end, m.start())
    hit_tokens = tokens
    hit_nouns = {n.lower() for n in NOUN_RE.findall(sentence_raw)}
if not hit:
    sys.exit(0)

# --- (2) a tool call in this turn opened it -----------------------------------------------------
# A strong token in a command is not enough on its own when the sentence named the artifact by kind
# as well: `er-invasion-warp` appears in a command that opened
# `crates/er-invasion-warp/src/local_invasion_filter.rs`, and that command did not open the crate's
# log. So a tool call clears the check only when one command carries the token and, if the sentence
# also named a kind, a word of that kind. A sentence naming only a path or an address keeps the plain
# token test, because there is no kind to pair.
tool_texts = [
    tool_input_text(block).lower() for kind, block in turn.blocks if kind == "tool"
]
consulted = any(
    token.lower() in text and (not hit_nouns or any(noun in text for noun in hit_nouns))
    for text in tool_texts
    for token in hit_tokens
)

message = scrub(turn.text)
future = bool(FUTURE_RE.search(message))
userneed = bool(USERNEED_RE.search(message)) or scan.blocked_on_user(message)
blocked = bool(BLOCKER_RE.search(message))
live = scan.live_background_work(events)
carried = bool(live) and (live.watcher or bool(WAITS_ON_RESULT_RE.search(message)))

sys.stdout.write(
    "DEFERFACTS|deferral=%s|consulted=%d|future=%d|userneed=%d|blocked=%d|carried=%d"
    % (hit, int(consulted), int(future), int(userneed), int(blocked), int(carried))
)
PY
