"""Detect a turn that ends by narrating the action it is taking instead of by having taken it.

The failure, verbatim from one session on 2026-09-10, all five turn-final and all with the action
either already dispatched or dispatchable in the same turn:

    Re-running it now without the cap.
    Rebuilding and relaunching now.
    Bringing it up to read the pointer chain live rather than guessing another offset:
    Making the empty read diagnosable - logging each of the three hops so the next invasion says
      which one returned zero instead of just that one did:
    Dispatching a subagent to enumerate the menu builder's rows properly, and unblocking you now by
      narrowing the guard:

The user's words when they finally called it out:

    "'Re-running it now without the cap' me: *points to the rego policy you `MUST` update to
     disallow this prose without a stop hook*"

They had already named the general form twice earlier in the same session -- "Announcing your own
next action instead of taking it", which is also how `AGENTS.md` spells it in the stop-too-early
list, and "Why do you insist on going 'My real point <em-dash> massive amount of prose that is
never worth reading'".

This is the present-progressive sibling of `cupcake_future_commitment.py`, which only reaches the
first person future ("next run I'll ...", "I'm going to ..."). A bare participial clause commits
nobody and names no future, so its opener pattern never sees one.

Two things keep it off ordinary work, and both are structural rather than a judgement about intent:

  * the signal that drives this module exits when a tool call follows the turn's last prose, so a
    mid-turn one-line preamble between two tool calls is never read. `wall_of_text.rego` says the
    same thing in its own correction text: mid-turn narration between tool calls is fine and is not
    measured; this is about the message you close on.
  * the narration has to be the last sentence of that closing prose. A narration with a clause hung
    off it -- "Rebuilding and relaunching now; the one thing I'll need from you afterwards is a
    single use of the item." -- belongs to `ER-EFFECTS-NO-ZERO-INFORMATION-STOP`, which was written
    for that exact sentence, and charging it twice would be two rules quoting one clause.

Kept as an importable module rather than an inline heredoc so the classifier is testable without a
live transcript, the way `cupcake_future_commitment.py` and `cupcake_challenged_convention.py` are.
The transcript walk and the turn bucketing stay in `cupcake_turn_scan.py`, shared with every
neighbouring signal.
"""

from __future__ import annotations

import re

# --- shared text handling ------------------------------------------------------------------------

# A fenced block is folded to one private-use character rather than to whitespace. Whitespace would
# let a closing "Checking the three offsets:" swallow the code block that answers it and read as the
# last sentence of the message; a single non-space character keeps the block a sentence of its own,
# so a turn that showed its output does not look like a turn that only announced it.
FENCE_TOKEN = "\ue000"


def fold_fences(text: str) -> str:
    """Fenced code, closed or left open by an interrupted turn, folded to one placeholder each."""
    folded = re.sub(r"```.*?```", FENCE_TOKEN, text or "", flags=re.DOTALL)
    return re.sub(r"```.*\Z", FENCE_TOKEN, folded, flags=re.DOTALL)


def strip_inline(text: str) -> str:
    """Backtick spans and double-quoted spans removed, so quoting a banned sentence -- this module,
    the policy, a report about the guard -- cannot trip it.

    Single quotes are left alone: the user's own report of this defect carries its examples in
    single quotes, and stripping those would take the sentence with them.
    """
    text = re.sub(r"`[^`]*`", " ", text or "")
    return re.sub(r'"[^"]{0,400}"', " ", text)


def sentences(text: str) -> list[str]:
    """Sentences, treating a semicolon as a boundary, the way the neighbouring signals do.

    The semicolon is what separates this rule from `ER-EFFECTS-NO-ZERO-INFORMATION-STOP`: its
    verbatim instance hangs a request off the narration with one, so splitting there is what stops
    this rule from charging that sentence a second time.
    """
    out = []
    for chunk in re.split(r"(?<=[.!?;])\s+|\n+", text or ""):
        collapsed = " ".join(chunk.split())
        if collapsed:
            out.append(collapsed)
    return out


def quote(sentence: str, limit: int = 170) -> str:
    """One sentence, safe to carry through a pipe-delimited facts line."""
    clipped = sentence[:limit].replace("|", "/")
    if len(sentence) > limit:
        clipped += " ..."
    return clipped


# --- the narrated action -------------------------------------------------------------------------

# The verbs a turn narrates instead of performing, grouped by the kind of work each names. The group
# is reported so the halt can say what was owed, exactly as the future-commitment guard does.
#
# The code-change gerunds are deliberately absent -- see `PROMISSORY_CLOSER_VERBS` below.
ACTION_GROUPS: dict[str, tuple[str, ...]] = {
    "build": (
        "building", "rebuilding", "compiling", "recompiling", "linking", "relinking",
        "cross-compiling",
    ),
    "launch": (
        "running", "rerunning", "re-running", "launching", "relaunching", "restarting",
        "booting", "rebooting", "replaying", "reproducing", "deploying", "installing",
        "reinstalling", "bringing", "kicking", "starting", "spinning",
    ),
    "attach": (
        "attaching", "reattaching", "re-attaching", "injecting", "instrumenting", "tracing",
        "detouring",
    ),
    "measure": (
        "measuring", "confirming", "verifying", "validating", "checking", "rechecking", "testing",
        "retesting", "proving", "capturing", "screenshotting", "benchmarking", "profiling",
        "probing", "sampling",
    ),
    "read": (
        "reading", "rereading", "re-reading", "inspecting", "grepping", "searching", "examining",
        "diffing", "decompiling", "disassembling", "dumping", "scanning", "auditing", "digging",
    ),
    "vcs": (
        "committing", "pushing", "rebasing", "tagging", "publishing", "cherry-picking", "staging",
    ),
    "delegate": ("dispatching", "spawning", "delegating", "briefing", "forking"),
    "edit": ("logging", "making", "unblocking", "annotating", "recording", "diagnosing"),
}

ACTION_CLASS = {verb: group for group, verbs in ACTION_GROUPS.items() for verb in verbs}

# The gerunds `ER-EFFECTS-NO-PROMISSORY-CLOSER` already owns, copied from `CLOSER_VERBS` in
# `.cupcake/signals/last_assistant_diagnosis_without_fix.sh`. They must never appear in
# `ACTION_GROUPS`, or one closing sentence would be charged by two rules with two different
# corrections. `scripts/test-narrated-action-classifier.py` re-reads that signal file and fails if
# the two sets ever intersect, so the boundary is enforced rather than remembered.
PROMISSORY_CLOSER_VERBS = frozenset(
    """fixing implementing porting wiring adding patching bypassing switching rewriting replacing
    hooking restoring removing deleting applying plumbing threading swapping bumping extending
    landing refactoring reverting renaming dropping splitting merging folding gating moving widening
    narrowing tightening""".split()
)

VERB_ALTERNATION = "|".join(sorted(ACTION_CLASS, key=len, reverse=True))

# Adverbs and conjunctions that can sit in front of the participle without changing what is being
# narrated. "Now rebuilding it" and "So, re-running it" are the same announcement.
LEAD = r"(?:(?:and|so|then|now|next|first|also|meanwhile|separately|quickly)\s*,?\s+)*"

HEAD_RE = re.compile(r"^" + LEAD + r"(" + VERB_ALTERNATION + r")\b", re.IGNORECASE)

# The first-person progressive, the same announcement with a subject attached.
#
# Anchored at the start of the sentence, which is not cosmetic: an unanchored search read two
# ordinary reports as narrations over 763 real turns -- "Both are now repinned to measured values
# (2800 and 1336 ...), er-npc-possess compiles again, and I'm restarting the full 26-shell relink"
# and a two-clause summary ending "and the next measurement I'm reading is ...". A trailing clause
# inside a report is a report; the shape the directive names heads its own sentence.
FIRST_PERSON_RE = re.compile(
    r"^" + LEAD + r"i(?:'|’)?m\s+(?:now\s+|just\s+|also\s+|currently\s+|already\s+|still\s+)?"
    r"(" + VERB_ALTERNATION + r")\b"
    r"|^" + LEAD + r"i\s+am\s+(?:now\s+|just\s+|also\s+|currently\s+|already\s+|still\s+)?"
    r"(" + VERB_ALTERNATION + r")\b",
    re.IGNORECASE,
)

# A colon closing the sentence is the announce-then-do shape: the tool call was meant to follow it,
# and at turn-end nothing did. Three of the five verbatim instances end this way, and the user's
# report calls the colon out by name.
COLON_END_RE = re.compile(r":\s*$")

# The other end-anchored spelling: the sentence finishes on "now".
ENDS_NOW_RE = re.compile(r"\bnow\b\s*[.!?;:,–—-]*\s*$", re.IGNORECASE)

# A finite-verb proxy, borrowed in spirit from the promissory closer's and widened where this verb
# list needs it. It is what tells an announcement from a report whose subject happens to be a gerund:
# "Reading the binary first is a rule this repo already has" and "Making it worse: the second hop
# also returns zero" both carry one and are passed over.
FINITE_VERB_RE = re.compile(
    r"\b(?:is|are|was|were|be|been|being|am|has|have|had|do|does|did|will|would|can|could"
    r"|should|shall|may|might|must|make|makes|made|mean|means|meant|take|takes|took|require"
    r"|requires|need|needs|give|gives|gave|leave|leaves|left|turn|turns|turned|read|reads|show"
    r"|shows|showed|prove|proves|proved|fix|fixes|fixed|work|works|worked|cost|costs|land|lands"
    r"|landed|get|gets|got|go|goes|went|come|comes|came|stay|stays|keep|keeps|kept|remain|remains"
    r"|let|lets|put|puts|say|says|said|tell|tells|told|become|becomes|became|exist|exists|sit|sits"
    r"|sat|appear|appears|happen|happens|matter|matters|help|helps|break|breaks|broke|fail|fails"
    r"|failed|pass|passes|passed|count|counts|want|wants|return|returns|returned|report|reports"
    r"|reported|look|looks|looked|seem|seems|contain|contains|carry|carries|hold|holds|ran|runs"
    r"|built|builds|point|points|list|lists|name|names|print|prints|emit|emits|wrote|writes"
    r"|found|finds|knows|know|hits|hit|adds|answer|answers|answered|explain|explains)\b",
    re.IGNORECASE,
)

# A short sentence with no finite verb anywhere in it: the participle heads the whole clause, which
# is what "Re-running it now without the cap." and "Rebuilding and relaunching now." are.
FRAGMENT_MAX_WORDS = 9

# A hedge or a report about the narration rather than the narration itself. "Worth re-running" and
# "no point rebuilding" announce nothing.
HEDGE_LEAD_RE = re.compile(
    r"^(?:worth|no\s+point|not|never|instead\s+of|rather\s+than|without|after|before|by|while"
    r"|when|since|because|although|though|despite|besides|apart\s+from)\b",
    re.IGNORECASE,
)


def narrated_action(closing_text: str) -> tuple[str, str, str] | None:
    """The closing narration as (clause, action class, accepting shape), or None.

    Only the final sentence of the closing prose is read. That is the whole difference between this
    guard and a general gerund detector: the objection is to a turn that *ends* on the narration,
    and every other position for the same sentence is either ordinary work or a neighbour's rule.
    """
    folded = fold_fences(closing_text)
    tail = sentences(folded)
    if not tail:
        return None
    sentence = strip_inline(tail[-1]).strip()
    if not sentence or HEDGE_LEAD_RE.match(sentence):
        return None

    head = HEAD_RE.match(sentence)
    if head:
        verb = head.group(1).lower()
        shape = accepting_shape(sentence)
        if shape:
            return quote(tail[-1]), ACTION_CLASS[verb], shape

    first = FIRST_PERSON_RE.search(sentence)
    if first:
        verb = (first.group(1) or first.group(2)).lower()
        return quote(tail[-1]), ACTION_CLASS[verb], "firstperson"
    return None


def accepting_shape(sentence: str) -> str:
    """Which construction makes a participial head an announcement rather than a subject, or ''."""
    if COLON_END_RE.search(sentence):
        return "colon"
    if ENDS_NOW_RE.search(sentence):
        return "now"
    if len(sentence.split()) <= FRAGMENT_MAX_WORDS and not FINITE_VERB_RE.search(sentence):
        return "fragment"
    return ""


# --- the exemptions -------------------------------------------------------------------------------

# A measured outcome sitting in the narration sentence or after it: a number with a unit, an exit
# code, a hash, an address, a path, a file name, or the fenced block a command's output lands in.
# Then the sentence reports something that happened rather than announcing something that has not.
MEASURED_RE = re.compile(
    r"\bexit(?:ed|s)?\s+(?:code\s+|status\s+)?\d+\b"
    r"|\bexit\s+code\b"
    r"|\breturn(?:ed|s)?\s+\d+\b"
    r"|\b\d+(?:[.,]\d+)?\s*(?:%|ms|s|sec|secs|seconds|m|min|mins|minutes|b|kb|mb|gb|hz)\b"
    # Two digits or more in front of the counted noun, not one. "26 rows" is a result; "the 3 call
    # sites" and "the 3 hops" are a count of work about to be done, and a single digit cannot tell
    # them apart. Under-reporting here only ever lets a halt through, which is the safe direction.
    r"|\b\d{2,}\s+(?:lines?|rows?|hits?|calls?|turns?|cases?|files?|tests?|halts?|matches?|bytes?"
    r"|errors?|warnings?|failures?|frames?|entries|functions?|offsets?|addresses)\b"
    r"|\b0x[0-9a-f]{3,}\b"
    r"|\b[0-9a-f]{7,40}\b"
    # A bare number of two digits or more. Measured on the transcripts: a closing report reads
    # "repinned to measured values (2800 and 1336, read out of type errors rather than guessed)",
    # and no unit follows either figure. A single digit is left out because it is usually a count
    # of things about to be done ("the three hops", "both addresses") rather than a result.
    r"|\b\d{2,}\b"
    r"|(?:^|\s|\()(?:/|\./|~/)[\w.\-/]*[\w\-]"
    r"|\b[\w.\-]+\.(?:rs|py|sh|rego|json|jsonl|toml|yml|yaml|log|md|dll|bin|txt|exe|ini)\b"
    r"|" + re.escape(FENCE_TOKEN),
    re.IGNORECASE,
)


def reported_outcome(closing_text: str) -> bool:
    """True when the closing narration carries, or is followed by, something measured.

    Read on the folded text rather than the scrubbed one, because the outcome is usually exactly
    what the scrubber removes: a backticked path, a quoted log line, a fenced block of output.
    """
    folded = fold_fences(closing_text)
    tail = sentences(folded)
    if not tail:
        return False
    return bool(MEASURED_RE.search(tail[-1]))


# The loud launch or teardown banner `AGENTS.md` mandates immediately before a game launch. It is a
# required form, not a defect: the section that mandates it -- "THE BANNER IS A PROMISE, NOT A MOOD"
# -- exists so the user is alerted before their session is torn down or a run starts, and a rule
# that made the banner unspeakable would take a safety announcement away to save a round trip.
#
# Two spellings occur in this repo's transcripts: the box-drawn block, and a bold inline heading.
BANNER_RE = re.compile(
    r"[╔╗╚╝║═]"
    r"|⚠"
    r"|\bLAUNCHING\s+ELDEN\s+RING\b"
    r"|\bTEARING\s+DOWN\b"
    r"|\bRELAUNCHING\s+ELDEN\s+RING\b"
    r"|\bKILLING\s+(?:RUN|THE\s+RUN|YOUR)\b"
    r"|\bLAUNCH(?:ING)?\s+BANNER\b"
)


def launch_banner(closing_text: str) -> bool:
    """True when the closing prose carries the mandated launch or teardown banner."""
    return bool(BANNER_RE.search(closing_text or ""))


# A dependency the agent cannot dissolve by working harder: an observation only the user can make, a
# credential, sudo, a login, a purchase, a decision that is theirs, a guard that refused the write,
# or an explicit instruction to stop. Narrow on purpose, and the same family the neighbouring Stop
# guards read, so the three cannot drift into disagreeing about what a blocker is.
EXTERNAL_BLOCKER_RE = re.compile(
    r"\bblocked\b|\bblocker\b|\bcannot\s+proceed\b"
    r"|\bcan(?:no|')?t\s+(?:proceed|run|test|verify|build|launch|start|do\s+that)\b"
    r"|\bpermission\s+denied\b|\baccess\s+denied\b|\bread-?only\s+file\s*system\b"
    r"|\b(?:cupcake|the\s+guard|the\s+policy|the\s+hook|the\s+sentinel)\s+"
    r"(?:denied|blocked|refused|rejected)\b"
    r"|\brequires?\s+(?:sudo|root|approval|credentials|a\s+login|a\s+purchase|a\s+live\s+game"
    r"|the\s+game\s+running|physical|network|your|you\s+to)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|credentials|a\s+login|a\s+purchase|a\s+live\s+game"
    r"|the\s+game\s+running|your|you\s+to)\b"
    r"|\bnot\s+installed\b|\bis\s+missing\s+from\s+this\s+machine\b"
    r"|\btell\s+me\s+(?:what|whether|if|when|how)\b"
    r"|\bwhat\s+(?:did|do)\s+you\s+(?:see|observe|notice|get)\b"
    r"|\bdid\s+you\s+(?:see|notice|observe|hear)\b"
    r"|\byour\s+(?:call|preference|choice|judgement|judgment)\b"
    r"|\bonly\s+you\s+can\b|\bup\s+to\s+you\b"
    r"|\byou\s+(?:said|asked|told\s+me)\s+to\s+(?:stop|wait|hold|pause)\b"
    r"|\bwaiting\s+on\s+(?:approval|ci|the\s+merge|the\s+gate)\b",
    re.IGNORECASE,
)


def externally_blocked(closing_text: str) -> bool:
    return bool(EXTERNAL_BLOCKER_RE.search(strip_inline(fold_fences(closing_text))))
