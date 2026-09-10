#!/usr/bin/env bash
# Cupcake signal: last_assistant_diagnosis_without_fix
#
# Scans the most recently completed assistant turn and emits one facts line when that turn ended by
# explaining a defect it did not fix.
#
#   DIAGFACTS|diagnosis=..|fixed=..|asked=..|blocked=..|promise=..|edited=..|handback=..
#            |handbackkind=..|userneed=..|didwork=..|extblocked=..|carried=..|unread=..
#            |consulted=..|future=..
#
# Emitted when either shape was found; a clean turn emits empty (fail-open).
#
# Two shapes, one fact
# --------------------
# `diagnosis` is a defect named in the closing prose. `promise` is the promissory closer, added
# 2026-09-09 after this one walked past every Stop guard in the repo, verbatim:
#
#     Fixing both: bypass the union so the naked capture is the detour entry (`MhHook::new` exists
#     for exactly that), and pass the adopted menu object to `invade` instead of the synthesized box.
#
# It reads as a fix already underway and it is a plan. The turn made no edit. Neither sibling could
# see it: `last_assistant_unexecuted_promise` needs a first-person opener ("I'll", "I'm going to",
# "let me") and this sentence commits nobody; `last_assistant_described_next_step` needs a
# forward-looking prescription ("the next step is") and the present participle points at now, not
# next. The gerund with no subject is the comfortable way to announce work without doing it or
# promising it, and the fact that settles it is the one this file already computes: did a file change.
#
# The two shapes share the transcript walk, the scrubber and the exemptions, which is why they live
# in one signal rather than a fourth near-copy of it. They report different facts because they need
# different ones: `fixed` is an edit after the diagnosis sentence in the same turn, and a closer is
# the last thing in the turn, so nothing can come after it. The promissory arm therefore reads
# `edited` -- a write anywhere in the turn -- and a turn that made the edit and then said so in the
# present participle is a truthful report that must pass.
#
# The failure this exists to refuse
# ---------------------------------
# Reported by the user 2026-09-09, mid-session, after five consecutive turns of it: "I don't care
# what is broken or why, I only know what the fix looks like, and that you're still not there at
# every turn you pause and dump prose". Each of those turns correctly identified a real defect -- a
# banner phrase, a self-contradicting test, a hot loop -- named it, explained it, and then stopped.
# The explanations were true and unwanted; the edit that should have followed took one tool call
# and was not made.
#
# Why the existing Stop rules do not catch it
#   * `ER-EFFECTS-NO-DESCRIBED-NEXT-STEP` requires a forward-looking prescription ("the next step
#     is..."). Its patterns deliberately exclude a bare "the fix is X", because that is the ordinary
#     way to report a fix already made. A turn that names a defect in the past tense and stops slips
#     straight through.
#   * `ER-EFFECTS-NO-IDLE-HOLD` wants an announcement of waiting. Such a turn announces nothing; it
#     just ends.
#   * `ER-EFFECTS-NO-STALL-ON-FRICTION` needs the opening prompt to carry frustration, and its
#     `acted` fact counts any tool call -- so a turn that read six files to build its explanation
#     scores as having acted.
#
# The distinguishing fact is narrower than "did anything happen": did the turn change a file.
# Reading, grepping and disassembling are how a diagnosis is built; they are not the fix. `fixed`
# is therefore true only for an Edit / Write / MultiEdit / NotebookEdit `tool_use` later in the same
# turn than the sentence that made the diagnosis.
#
# Two exemptions, each a case where explaining is itself the deliverable
#   asked    -- the user's own prompt asked a question ("why is it...", any "?"). Answering must
#               never be gagged. Broad on purpose.
#   blocked  -- the turn stated a real dependency (sudo, a live game, an approval) or is waiting on
#               the user. A diagnosis that cannot yet be acted on is not a stall.
#
# Fenced code, backtick spans and double-quoted spans are stripped before matching, so quoting these
# phrases -- this file, the policy, a report about the guard -- cannot trip it. The shared half
# (transcript discovery, turn bucketing, blocked-on-user phrasing) comes from
# `scripts/cupcake_turn_scan.py`, the same module the neighbouring signals use, so the guards cannot
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
turns = scan.split_turns(events)
turn = scan.last_text_turn(turns)
if turn is None:
    sys.exit(0)

# A turn that kept going past its last prose did not end on that prose.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)


def scrub(text):
    text = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]*"', " ", text)


# --- (1) the turn named a defect --------------------------------------------------------------
# Present and past tense both: the point is that a fault was identified, not that one is planned.
# "the fix is" belongs here, unlike in the described-next-step signal, because `fixed` below already
# tells the two apart -- a fix that was made carries an edit in the same turn.
DIAGNOSIS_RE = re.compile(
    r"\b(?:the|a|one|another|its)\s+"
    r"(?:real\s+|actual\s+|underlying\s+|genuine\s+|core\s+|root\s+)?"
    r"(?:defect|bug|fault|flaw|problem|cause|culprit|breakage|regression|mistake)\b"
    r"|\bthe\s+(?:real\s+|right\s+|correct\s+|proper\s+)?fix\s+is\b"
    r"|\bwhat(?:'s|\s+is)\s+(?:actually\s+)?(?:wrong|broken)\b"
    r"|\bis\s+(?:a|the)\s+(?:real\s+)?(?:defect|bug|regression)\b"
    r"|\bthat\s+is\s+(?:a|the)\s+(?:defect|bug|problem|cause)\b"
    r"|\bwhy\s+it\s+(?:fails|failed|breaks|broke|is\s+broken)\b"
    r"|\bworth\s+(?:chasing|fixing)\b"
    r"|\bneeds?\s+(?:a\s+)?fix(?:ing)?\b",
    re.IGNORECASE,
)


def sentence_span(text, pos):
    start = 0
    for m in re.finditer(r"[.!?\n]", text[:pos]):
        start = m.end()
    end = len(text)
    for m in re.finditer(r"[.!?\n]", text[pos:]):
        end = pos + m.start() + 1
        break
    return start, end


def clause(text, m):
    start, end = sentence_span(text, m.start())
    collapsed = " ".join(text[start:end].split())
    if len(collapsed) <= 120:
        return collapsed.replace("|", "/")
    window_start = max(start, m.start() - 40)
    clipped = " ".join(text[window_start:window_start + 130].split())
    prefix = "..." if window_start > start else ""
    return (prefix + clipped + " ...").replace("|", "/")


hit = None
hit_index = -1
for index, (kind, value) in enumerate(turn.blocks):
    if kind != "text":
        continue
    scrubbed = scrub(value)
    for m in DIAGNOSIS_RE.finditer(scrubbed):
        hit, hit_index = clause(scrubbed, m), index

# --- (2) did the turn change a file after saying it? -------------------------------------------
# Reads are how a diagnosis is built. Only a write is the fix.
#
# A Bash command that writes a file counts, and leaving it out was a false positive that halted a
# turn which had edited three files, rebuilt the DLL and relaunched the game (2026-09-09). In
# bypass-permissions mode the session prompt tells the model to prefer Bash for file changes --
# heredocs, `sed -i`, redirects -- so the dedicated edit tools are exactly the ones it will not be
# using, and a guard blind to the mechanism in use fires on correct turns. A guard that punishes
# the sanctioned workflow teaches evasion, not the behaviour it wants.
#
# Deliberately narrow: a redirect into a file, an in-place stream edit, `tee`, `patch`, or a
# heredoc. Reading, grepping, building and launching are not writes, so a turn that only inspects
# still scores zero.
EDIT_TOOLS = {"edit", "write", "multiedit", "notebookedit"}

BASH_WRITE_RE = re.compile(
    r"<<\s*'?[A-Za-z_]+'?\b"          # heredoc, the shape `python3 - <<'PY'` uses
    r"|\bsed\s+(?:-[^\s]*\s+)*-i\b"  # in-place stream edit, flags in any order
    r"|\bsed\s+-i\b"
    r"|\btee\s"
    r"|\bpatch\s"
    # A redirect into a path, but never into /tmp, /dev or a scratchpad: piping a build log
    # somewhere is how a diagnosis gets read, not how one gets fixed.
    r"|>>?\s*(?!/tmp/|/dev/|/var/tmp/)[^\s|&;<>]*/[^\s|&;<>]+"
)


def is_edit(block):
    name = ""
    if isinstance(block, dict):
        name = str(block.get("name") or block.get("tool_name") or "")
    normalised = name.strip().lower().replace("_", "")
    if normalised in EDIT_TOOLS:
        return True
    if normalised != "bash":
        return False
    command = ""
    if isinstance(block, dict):
        raw = block.get("input") or {}
        if isinstance(raw, dict):
            command = str(raw.get("command") or "")
    return bool(BASH_WRITE_RE.search(command))


fixed = bool(hit) and any(
    kind == "tool" and is_edit(block) for kind, block in turn.blocks[hit_index + 1:]
)

# A closer is the last thing in the turn, so no edit can follow it and `fixed` is always 0 for one.
# The question that separates a truthful report from an announcement is whether the turn wrote
# anything at all, so the promissory arm reads this instead.
edited = any(kind == "tool" and is_edit(block) for kind, block in turn.blocks)

message = scrub(turn.text)

# --- (2b) the promissory closer ----------------------------------------------------------------
# The shape, verbatim from the turn that prompted it: "Fixing both: bypass the union so the naked
# capture is the detour entry, and pass the adopted menu object to `invade` instead of the
# synthesized box." A present participle with no subject, announcing work as if it were in flight,
# closing a turn that changed nothing. Related spellings: "Now fixing ...", "Porting the offsets.",
# "Implementing the latch next.", "Next I bypass the union".
#
# Only the last two sentences of the final prose run are read, which is what keeps ordinary work
# untouched: the one-line gerund preamble before a tool call ("Reading the policy.") is mid-turn and
# is never seen, and a turn whose last block is followed by a tool call has already exited above.
#
# The hard part is that an identical-looking gerund is the ordinary subject of an ordinary sentence:
# "Adding the field to the struct fixed it", "Fixing the rule requires the live counter". Those
# report and explain; they do not announce. Five accepting shapes separate them, each one a
# construction a subject-gerund cannot take:
#   `CLOSER_COLON`         -- a colon after the head, the announce-then-plan shape ("Fixing both:").
#   `CLOSER_TRAILING_NEXT` -- the sentence ends on "next" ("Implementing the latch next.").
#   `CLOSER_ADVERB_LED`    -- an adverb of time comes first ("Now fixing the union").
#   `FRAGMENT_MAX_WORDS`   -- a short sentence with no finite verb anywhere in it, so the gerund
#                             heads the whole clause ("Porting the offsets."). The finite-verb list
#                             below is a proxy, not a parser, and it is why the two report sentences
#                             above are passed over.
#   `CLOSER_NEXT_I`        -- "Next I bypass the union": first person, bare present, no "I'll" for
#                             `last_assistant_unexecuted_promise` to match on.
#
# "correcting" was measured out of the verb list rather than reasoned out: it fired on "Correcting
# myself on one thing I told you earlier:", which retracts a statement and announces no work. The
# SELF_REF suppressor keeps the rest of the list from doing the same.
CLOSER_VERBS = (
    r"fixing|implementing|porting|wiring|adding|patching|bypassing|switching|rewriting"
    r"|replacing|hooking|restoring|removing|deleting|applying|plumbing|threading|swapping"
    r"|bumping|extending|landing|refactoring|reverting|renaming|dropping|splitting|merging"
    r"|folding|gating|moving|widening|narrowing|tightening"
)

CLOSER_BARE_VERBS = (
    r"fix|implement|port|wire|add|patch|bypass|switch|rewrite|replace|hook|restore|remove|delete"
    r"|apply|plumb|thread|swap|bump|extend|land|refactor|revert|rename|drop|split|merge|fold|gate"
    r"|move|widen|narrow|tighten"
)

LEAD = r"(?:(?:and|so|then|now|next|first|also)\s*,?\s+)*"
CLOSER_HEAD = re.compile(r"^" + LEAD + r"(?:" + CLOSER_VERBS + r")\b", re.IGNORECASE)
CLOSER_COLON = re.compile(
    r"^" + LEAD + r"(?:" + CLOSER_VERBS + r")\b[^.!?\n:]{0,80}:", re.IGNORECASE
)
CLOSER_TRAILING_NEXT = re.compile(
    r"^" + LEAD + r"(?:" + CLOSER_VERBS + r")\b[^.!?\n]{0,120}\bnext\b[.!?]?\s*$", re.IGNORECASE
)
CLOSER_ADVERB_LED = re.compile(
    r"^(?:now|next|then)\s*,?\s+(?:" + CLOSER_VERBS + r")\b", re.IGNORECASE
)
CLOSER_NEXT_I = re.compile(
    r"^next\s*,?\s+i\s+(?:" + CLOSER_BARE_VERBS + r")\b", re.IGNORECASE
)
# A retraction of something said, not an announcement of work.
SELF_REF = re.compile(
    r"^" + LEAD + r"(?:" + CLOSER_VERBS + r")\s+(?:myself|the\s+record|my\s+(?:earlier|previous|own))\b",
    re.IGNORECASE,
)
# Enough of a finite-verb proxy to tell a headed clause from a fragment. Deliberately short: every
# word added here makes the fragment arm quieter, never louder.
FINITE_VERB = re.compile(
    r"\b(?:is|are|was|were|be|been|being|am|has|have|had|do|does|did|will|would|can|could"
    r"|should|shall|may|might|must|make|makes|made|mean|means|meant|take|takes|took|require"
    r"|requires|need|needs|give|gives|gave|leave|leaves|left|turn|turns|turned|read|reads|show"
    r"|shows|showed|prove|proves|proved|fix|fixes|fixed|work|works|worked|cost|costs|land|lands"
    r"|landed|get|gets|got|go|goes|went|come|comes|came|stay|stays|keep|keeps|kept|remain|remains"
    r"|let|lets|put|puts|say|says|said|tell|tells|told|become|becomes|became|exist|exists|sit|sits"
    r"|sat|appear|appears|happen|happens|matter|matters|help|helps|break|breaks|broke|fail|fails"
    r"|failed|pass|passes|passed|count|counts|want|wants)\b",
    re.IGNORECASE,
)

FRAGMENT_MAX_WORDS = 9


def closing_sentences(text):
    """Sentences, treating a semicolon as a boundary.

    The verbatim zero-information stop is "Rebuilding and relaunching now; the one thing I'll need
    from you afterwards is a single use of the item." Without the semicolon the announcement and the
    request are one sentence, the announcement cannot be end-anchored, and anchoring it loosely is
    what made the first draft fire on reports like "Reading it now -- the file is on disk".
    """
    out = []
    for chunk in re.split(r"(?<=[.!?;])\s+|\n+", text):
        collapsed = " ".join(chunk.split())
        if collapsed:
            out.append(collapsed)
    return out


def announces_work(sentence):
    if CLOSER_NEXT_I.match(sentence):
        return True
    if not CLOSER_HEAD.match(sentence):
        return False
    if SELF_REF.match(sentence):
        return False
    if CLOSER_COLON.match(sentence) or CLOSER_TRAILING_NEXT.match(sentence):
        return True
    if CLOSER_ADVERB_LED.match(sentence):
        return True
    words = sentence.split()
    return len(words) <= FRAGMENT_MAX_WORDS and not FINITE_VERB.search(sentence)


def quote(sentence):
    """One sentence, safe to carry through a pipe-delimited facts line."""
    clipped = sentence[:160].replace("|", "/")
    if len(sentence) > 160:
        clipped += " ..."
    return clipped


promise = ""
runs = turn.text_runs
if runs:
    for sentence in closing_sentences(scrub(runs[-1]))[-2:]:
        if announces_work(sentence):
            promise = quote(sentence)
            break


# --- (2c) the zero-information handback ---------------------------------------------------------
# User directive 2026-09-09, verbatim: "If nothing is the ideal response from me, in every case,
# this means we are lacking a rego policy that adds a stop hook. Every. Single. Time. This.
# Happens." A turn whose ideal next user reply is nothing has spent a round trip to say so.
#
# The instance. Asked "What's my ideal response for you at this moment?", the turn closed:
#
#     Nothing -- the ball is in my court. Rebuilding and relaunching now; the one thing I'll need
#     from you afterwards is a single use of the item.
#
# The build and the relaunch were the agent's own next actions, already known and already
# unblocked, and the turn stopped instead of taking them. Every existing guard was disarmed by the
# last clause: `last_assistant_unexecuted_promise` and `last_assistant_described_next_step` both
# exempt a turn that hands the obligation to the user, and "I'll need from you ... a single use of
# the item" reads as exactly that -- except that in this repo the agent drives every in-game input
# itself (AGENTS.md standing order 2026-07-22), so it is not a handoff at all.
#
# Three shapes, reported as `handbackkind` so the policy can weigh them differently:
#   a -- the user is told they need do nothing ("Nothing -- ...", "the ball is in my court", "no
#        action needed from you", "up to me"). On its own this is often the honest end of a
#        finished task, so the policy fires on it only when the turn did no work at all.
#   b -- the agent announces its own next action instead of taking it. Two spellings: the present
#        participle that ends on "now" ("Rebuilding and relaunching now"), and the build/launch/run
#        family in the first person ("I'll build and launch", "next I'll rerun it", "let me now
#        relaunch"). Fires on its own.
#   c -- an offer to do work the agent is already authorised to do ("say the word and I'll ...",
#        "want me to ...", "let me know if you want me to ..."). Fires on its own.
#
# What was measured out, and why the shapes above are this narrow. The first draft fired on 255 of
# 3,749 real turn boundaries -- 6.8%, one turn in fifteen, which is a guard nobody would keep. All
# three over-reaches came from the same mistake, taking a grammar for an intent:
#   * a fourth shape fired on any request for in-game input ("use the Lynchpin now", "sit at a
#     grace"). It is deleted. The directive names it as something that cannot exempt a handback, not
#     as a handback itself, and by the rule's own test a turn asking for an item use does not have
#     "nothing" as its ideal reply -- the reply is the run. `USER_INPUT_REQUEST_RE` survives for that
#     one job: barring an input request from being read as the observation exemption below.
#   * the participle spelling matched "now" anywhere in the next 120 characters, which caught two
#     reports outright -- "Reading it now -- the file is on disk and the three frames are exact",
#     "Reading the binary first is a rule this repo already has; I have now paid the tuition". It is
#     end-anchored instead, and `closing_sentences` now breaks on a semicolon, which is what keeps
#     the verbatim instance's "Rebuilding and relaunching now;" a sentence of its own.
#   * the first-person spelling covered every own-action verb, so "I'll read the log on demand
#     instead of streaming it" was a violation. It is cut to the build/launch/run family, the one
#     the instance is about; `last_assistant_unexecuted_promise` owns the rest and the gap where a
#     handoff clause disarms it is declared, not closed.
HANDBACK_NOTHING_RE = re.compile(
    r"^nothing\s*(?:[-–—,.:;!]|$)"
    r"|^nothing\s+(?:from\s+you|you\s+need|on\s+your|is\s+needed|to\s+do|for\s+you)\b"
    r"|\bthe\s+ball\s+is\s+in\s+my\s+court\b"
    r"|\bno\s+action\s+(?:is\s+)?(?:needed|required)\b"
    r"|\bnothing\s+(?:is\s+)?(?:needed|required)\s+from\s+you\b"
    r"|\bnothing\s+(?:for\s+you|from\s+you)\s+to\s+do\b"
    r"|\byou\s+(?:don'?t|do\s+not)\s+need\s+to\s+do\s+anything\b"
    r"|\bnothing\s+on\s+your\s+(?:side|end)\b"
    r"|\b(?:it'?s|its|that'?s)\s+up\s+to\s+me\b"
    r"|\byour\s+ideal\s+(?:response|reply)\s+is\s+nothing\b",
    re.IGNORECASE,
)

# The build/launch/run family only. These are the actions a turn most often narrates instead of
# performing, and they are not code-change verbs, which is what keeps this shape distinct from the
# promissory closer above.
LAUNCH_ACTION = r"build|rebuild|launch|relaunch|run|rerun|re-run|restart|rebase|compile"
LAUNCH_ACTION_ING = (
    r"building|rebuilding|launching|relaunching|running|rerunning|restarting|rebasing|compiling"
)
HANDBACK_OWN_ACTION_RE = re.compile(
    # "Rebuilding and relaunching now", as its own sentence or semicolon clause.
    r"^(?:(?:and|so|then|now|next|first|also)\s*,?\s+)*(?:" + LAUNCH_ACTION_ING + r")\b"
    r"[^.!?\n;]{0,60}\bnow\b\s*[.!?;:,-]?\s*$"
    r"|\bnext\s*,?\s+i'?ll\b"
    r"|\blet\s+me\s+now\s+(?:" + LAUNCH_ACTION + r")\b"
    r"|\b(?:i'?ll|i\s+will|i'?m\s+going\s+to|i\s+am\s+going\s+to)\s+(?:now\s+|then\s+|just\s+)?"
    r"(?:" + LAUNCH_ACTION + r")\b",
    re.IGNORECASE,
)

HANDBACK_OFFER_RE = re.compile(
    r"\bsay\s+the\s+word\s+and\s+i'?(?:ll|d)\b"
    r"|\bwant\s+me\s+to\b"
    r"|\bwould\s+you\s+like\s+me\s+to\b"
    r"|\blet\s+me\s+know\s+if\s+you\s+(?:want|would\s+like)\s+me\s+to\b"
    r"|\bif\s+you\s+want,?\s+i\s+(?:can|could|will|'?ll)\b"
    r"|\bshould\s+i\s+(?:go\s+ahead|proceed|do\s+that|start)\b"
    r"|\btell\s+me\s+(?:to\s+go|when\s+to\s+go)\b",
    re.IGNORECASE,
)

# An offer the agent is right to make, because taking the action unasked would be worse than the
# round trip. Two families, both measured out of the 255: an action that is destructive, irreversible
# or visible on the user's own desktop (tearing down their live game, a push, a pull request, a
# merge), and a genuine fork between named alternatives, which only they can settle.
HANDBACK_OFFER_EXEMPT_RE = re.compile(
    r"\btear\s+(?:it|this|that|them)?\s*down\b|\bteardown\b"
    r"|\bkill\b|\bterminate\b|\bshut\s+(?:it\s+)?down\b"
    r"|\bdelete\b|\bwipe\b|\boverwrite\b|\buninstall\b|\breboot\b"
    r"|\bforce[- ]push\b|\bpush\b|\bpull\s+request\b|\bas\s+a\s+pr\b|\bmerge\b|\breset\b|\brevert\b"
    r"|\btearing\s+(?:it|this|that|them|\w+)\s+down\b|\breap\b"
    r"|\bsay\s+which\b|\bwhich\s+(?:shape|option|approach|one|of\s+the\s+two|way)\b"
    r"|\beither\s+of\s+(?:those|these)\b"
    # An alternation inside the offer: "Want me to build A, or run B first?" is a fork.
    r"|\bor\b[^?]{0,120}\?",
    re.IGNORECASE,
)

# An observation or a decision only the user can supply. Every one of these makes the next user
# reply carry information, which is the whole test.
USER_OBSERVATION_RE = re.compile(
    r"\btell\s+me\s+(?:what|whether|if|when|how)\b"
    r"|\bwhat\s+(?:did|do)\s+you\s+(?:see|observe|notice|get)\b"
    r"|\bdid\s+you\s+(?:see|notice|observe|get|hear)\b"
    r"|\bwhat\s+you\s+(?:saw|see|observed)\b"
    r"|\blet\s+me\s+know\s+(?:what|whether|if|when|how)\s+(?!you\s+want\s+me\b)"
    r"|\b(?:take|have)\s+a\s+look\b"
    r"|\bwatch\s+(?:for|the)\b"
    r"|\breport\s+(?:back|what)\b"
    r"|\bconfirm\s+(?:whether|that|if)\b"
    r"|\b(?:does|did|do)\s+(?:it|that|they)\s+look\b"
    r"|\bhow\s+(?:does|did)\s+(?:it|that)\s+look\b"
    r"|\byour\s+(?:call|preference|choice|judgement|judgment)\b"
    r"|\bwhich\s+(?:do|would)\s+you\s+(?:prefer|rather)\b"
    r"|\bonly\s+you\s+can\b"
    r"|\bup\s+to\s+you\b"
    r"|\bneeds?\s+(?:your|a\s+human)\s+(?:login|password|credentials|purchase|decision|approval)\b"
    r"|\blog\s+in(?:to)?\b|\bsign\s+in\b|\bpurchase\b|\bbuy\b",
    re.IGNORECASE,
)

# A request aimed at the user to drive an in-game input. Not a firing shape -- see the measurement
# note above -- but it has two jobs: it must never be mistaken for the observation exemption, because
# an input request is work the agent drives itself rather than information only the user has; and it
# marks the evidence in the unread-evidence shape as future, because a log cannot answer before the
# input that produces it.
INPUT_VERB = r"press|click|hit|use|sit|equip|activate|navigate|load|quit|move|drive|tap|hold|invade"
USER_INPUT_REQUEST_RE = re.compile(
    r"^(?:please\s+|now\s+|then\s+|go\s+|just\s+)*(?:" + INPUT_VERB + r")\b"
    r"|\b(?:go|then|now|and)\s+(?:" + INPUT_VERB + r")\s+(?:the|at|a|it|your)\b"
    r"|\byou\s+(?:need\s+to|have\s+to|should|must|can|could)\s+(?:" + INPUT_VERB + r")\b"
    r"|\b(?:a|one|single)\s+(?:single\s+)?use\s+of\s+the\s+item\b"
    r"|\buse\s+the\s+(?:item|lynchpin|pot|flask)\b",
    re.IGNORECASE,
)

# A dependency outside the agent's reach, and deliberately not `blocked_on_user`, which the other
# two arms fold in: "I need you to press X" is a dependency on the user in the letter and a handback
# in fact, because the agent drives every in-game input itself. Reading the broad fact here would
# have exempted the exact shape this rule exists to refuse.
EXTERNAL_BLOCKER_RE = re.compile(
    r"\bblocked\b|\bblocker\b|\bcannot\s+proceed\b"
    r"|\bcan(?:no|')?t\s+(?:proceed|run|test|verify|build|launch|start)\b"
    r"|\brequires?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game|physical|credentials|network|a\s+login|a\s+purchase)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game\s+running|credentials|a\s+login|a\s+purchase)\b"
    r"|\bwaiting\s+on\s+(?:the\s+build|the\s+gate|ci|the\s+merge)\b"
    r"|\buntil\s+(?:they|it|the\s+(?:agents?|subagents?|run|build|probe))\s+(?:report|reports|finish|finishes|lands?|come\s+back)\b",
    re.IGNORECASE,
)


def handback_shape(sentence):
    # The offer is decided first: it is the narrower reading and it carries the exemptions, so a
    # bare "I'll build it" inside "say the word and I'll build it" must not bypass them.
    if HANDBACK_OFFER_RE.search(sentence):
        if HANDBACK_OFFER_EXEMPT_RE.search(sentence):
            return ""
        return "c"
    if HANDBACK_OWN_ACTION_RE.search(sentence):
        return "b"
    if HANDBACK_NOTHING_RE.search(sentence):
        return "a"
    return ""


handback = ""
handback_kind = ""
userneed = False
extblocked = False
didwork = any(kind == "tool" for kind, _ in turn.blocks)
if runs:
    closing = scrub(runs[-1])
    # An observation ask wins over a request for input when a message carries both. That is the
    # documented gap: an input request smuggled in beside a genuine observation ask is not caught,
    # because the user's reply still has to carry the observation and the round trip is not empty.
    userneed = bool(USER_OBSERVATION_RE.search(closing))
    extblocked = bool(EXTERNAL_BLOCKER_RE.search(closing))
    # `b` and `c` fire unaided, so a message carrying one of them is reported under it rather than
    # under the `a` sentence that may open the same message. The verbatim instance opens "Nothing --
    # the ball is in my court." and goes on to announce the build; the announcement is the clause
    # worth quoting back.
    for wanted in ("c", "b", "a"):
        for sentence in closing_sentences(closing):
            if handback_shape(sentence) == wanted:
                handback = quote(sentence)
                handback_kind = wanted
                break
        if handback:
            break

# --- (2d) the unread-evidence closer -----------------------------------------------------------
# The same defect as the promissory closer, in a grammar that reads as an observation rather than an
# intention, so a pattern keyed on "I'll" or a work gerund misses it. Verbatim, 2026-09-09:
#
#     ... and the next measurement is whether the DLL is using the menu object it now captures
#     (`r14`) for the re-invade gate or still falling back to the scan -- which its own log answers,
#     so I am reading that next.
#
# The answer was in a file on disk. Naming the read instead of performing it buys a round trip worth
# nothing, and it is reported under the same rule id as the promissory closer because it is the same
# failure: work identified, named, and left for after the turn.
#
# The discriminator is checkable without judging intent: the closing prose names a concrete artifact
# as holding the answer, and the turn did not consult it. `consulted` is deliberately keyed on strong
# artifact tokens only -- a path, a filename, an address, a symbol that appears in the closing prose
# and again in a tool call. A weak noun ("its own log") cannot clear it, because this repo's turns
# mention some log in almost every command, and letting the word exempt the shape would exempt the
# instance above, whose turn had grepped a different run's directory.
#
# `future` is the exemption that keeps a legitimate deferral legitimate: evidence that does not exist
# yet has to be produced before it can be read, so "the next run's log will say" is a plan waiting on
# a run rather than an unread file. The observation and blocker exemptions apply as they do elsewhere.
#
# Measured out, and worth stating because it was on the list of shapes to catch: the future tense
# "the log will say" is gone from the deferral set. Over 3,790 real turn boundaries it produced six
# of eight halts, and every one of the six named evidence a future run or a future press would
# produce -- "Invade once on this build and the log will say whether an owner exists", "Next time
# you're hit while possessed, the log will say whether that buffer ever changed". What survives is
# the tense that claims the answer already exists ("answers", "already says", "holds the answer")
# and the explicit deferral of the read ("so I am reading that next"), which is what the instance
# above is made of.
UNREAD_DEFER_RE = re.compile(
    r"\bthe\s+next\s+(?:measurement|step|question|thing|check|read|move)\s+is\b"
    r"|\bwhat\s+(?:remains|is\s+left)\s+is\s+to\b"
    r"|\bthe\s+remaining\s+question\s+is\b"
    r"|\bwhich\s+(?:its\s+own\s+|the\s+|that\s+)?(?:log|dump|trace|output|disassembly)\s+answers\b"
    r"|\b(?:the|its|that|this)\s+(?:own\s+)?(?:log|dump|trace|output|disassembly|binary)\s+"
    r"(?:already\s+(?:says|tells)|answers|holds\s+the\s+answer)\b"
    r"|\bthat\s+will\s+tell\s+(?:us|me|you)\b"
    r"|\bso\s+i\s+am\s+reading\s+that\s+next\b"
    r"|\bso\s+that\s+is\s+what\s+i\s+(?:check|read|measure|look\s+at)\s+next\b"
    r"|\breading\s+that\s+(?:next|now)\b"
    r"|\bthe\s+answer\s+is\s+(?:already\s+)?(?:in|on\s+disk)\b",
    re.IGNORECASE,
)

# A path, a filename, an address or a reverse-engineering symbol: something a tool call can be shown
# to have opened.
ARTIFACT_STRONG_RE = re.compile(
    r"\b[\w.~-]*/[\w./~-]+\b"
    r"|\b[\w-]+\.(?:log|rs|py|rego|toml|sh|json|txt|bin|md|jsonl|dll|exe)\b"
    r"|\b0x[0-9a-fA-F]{4,}\b"
    r"|\bFUN_[0-9a-fA-F]+\b"
)

# The artifact named by kind rather than by path. Enough to say evidence exists; never enough to say
# it was consulted.
ARTIFACT_WEAK_RE = re.compile(
    r"\b(?:its\s+own\s+|the\s+|that\s+|this\s+|our\s+|[\w-]+'s\s+)"
    r"(?:log|logs|dump|trace|transcript|disassembly|binary|output|artifact|record|telemetry)\b",
    re.IGNORECASE,
)

# Evidence that has to be produced before it can be read. Deferring to it is a plan, not a skipped
# read.
FUTURE_EVIDENCE_RE = re.compile(
    r"\b(?:the\s+next|a\s+new|another|this\s+next)\s+(?:run|launch|build|boot|attempt|probe)\b"
    r"|\bonce\s+(?:it|they|the\s+run|the\s+build|the\s+game|the\s+probe|you)\b"
    r"|\bafter\s+the\s+(?:next\s+)?(?:run|build|launch|boot)\b"
    r"|\bwhen\s+(?:you|the\s+run|it)\s+(?:use|uses|press|presses|sit|sits|invade|invades|lands|finishes)\b"
    r"|\bnext\s+time\b"
    r"|\bwill\s+(?:print|write|emit|record)\b"
    r"|\bhas\s+to\s+(?:happen|run|land)\s+first\b",
    re.IGNORECASE,
)


def tool_input_text(block):
    try:
        raw = block.get("input") or {}
    except AttributeError:
        return ""
    parts = []
    for value in (raw.values() if isinstance(raw, dict) else []):
        if isinstance(value, str):
            parts.append(value)
    return " ".join(parts)


unread = ""
consulted = False
future = False
if runs:
    closing_raw = runs[-1]
    closing_scrubbed = scrub(closing_raw)
    if UNREAD_DEFER_RE.search(closing_scrubbed):
        strong = {t for t in ARTIFACT_STRONG_RE.findall(closing_raw) if len(t) > 3}
        named = bool(strong) or bool(ARTIFACT_WEAK_RE.search(closing_scrubbed))
        if named:
            # A log cannot answer before the input that produces it: an in-game input request in
            # the same message puts the evidence in the future, which is the legitimate deferral
            # rather than a skipped read. Measured -- "Use the item once; the log answers with one
            # of three lines:" was a halt.
            future = bool(FUTURE_EVIDENCE_RE.search(closing_scrubbed)) or bool(
                USER_INPUT_REQUEST_RE.search(closing_scrubbed)
            )
            tool_text = " ".join(
                tool_input_text(block) for kind, block in turn.blocks if kind == "tool"
            ).lower()
            consulted = any(token.lower() in tool_text for token in strong)
            for sentence in closing_sentences(closing_scrubbed):
                if UNREAD_DEFER_RE.search(sentence):
                    unread = quote(sentence)
                    break

# --- (3) the user asked ------------------------------------------------------------------------
# `split_turns` keeps only blocks, not the prompt that opened the turn, so the prompt is recovered
# here the same way the stall signal recovers it: the last real user prompt in the transcript.
def prompt_text(ev):
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content
    parts = []
    for block in content or []:
        if isinstance(block, dict) and block.get("type") == "text":
            parts.append(block.get("text") or "")
        elif isinstance(block, str):
            parts.append(block)
    return "\n".join(parts)


prompt = ""
try:
    for ev in events:
        if scan.is_real_user_prompt(ev):
            prompt = prompt_text(ev)
except Exception:
    prompt = ""
asked = bool(re.search(r"\?", prompt)) or bool(
    re.match(
        r"\s*(?:why|what|how|when|where|which|who|is|are|was|were|does|do|did|can|could|should"
        r"|would|explain|tell\s+me)\b",
        prompt,
        re.IGNORECASE,
    )
)


# --- (4) a genuine blocker ---------------------------------------------------------------------
BLOCKER_RE = re.compile(
    r"\bblocked\b|\bblocker\b|\bcannot\s+proceed\b"
    r"|\bcan(?:no|')?t\s+(?:proceed|run|test|verify|build|launch|start)\b"
    r"|\brequires?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game|physical|credentials|network)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|a\s+game|a\s+live|the\s+game\s+running)\b"
    r"|\bwaiting\s+on\s+(?:you|the\s+user|the\s+build|the\s+gate|ci)\b",
    re.IGNORECASE,
)
try:
    blocked = bool(BLOCKER_RE.search(message)) or scan.blocked_on_user(message)
except Exception:
    blocked = bool(BLOCKER_RE.search(message))

# --- (5) work already in flight -----------------------------------------------------------------
# Read by the zero-information-stop arm. A turn that says "nothing to do until they report" while a
# subagent or a backgrounded build is genuinely running has not handed anything back: the work
# exists and something is carrying it. Same helper the idle-hold and described-next-step signals
# use, so the three cannot disagree about the same turn.
try:
    carried = bool(scan.live_background_work(events))
except Exception:
    carried = False

if not hit and not promise and not handback and not unread:
    sys.exit(0)

print(
    "DIAGFACTS|diagnosis={}|fixed={}|asked={}|blocked={}|promise={}|edited={}"
    "|handback={}|handbackkind={}|userneed={}|didwork={}|extblocked={}|carried={}"
    "|unread={}|consulted={}|future={}".format(
        hit or "",
        int(fixed),
        int(asked),
        int(blocked),
        promise,
        int(edited),
        handback,
        handback_kind,
        int(userneed),
        int(didwork),
        int(extblocked),
        int(carried),
        unread,
        int(consulted),
        int(future),
    )
)
PY
