"""Detect a turn that answered a challenge to one of its own choices by defending it.

The failure, verbatim, 2026-09-10, across three consecutive turns:

    user:      Do you crutch on 1.16.2 addreses for a specific reason?
    assistant: a table of why the convention exists, closing "I'll ask the agent for the
               return-value contract rather than the 1.16.2 address". No file changed.
    user:      Why on earth would I want that convention?
    assistant: "You wouldn't -- and there's no need to invent a new field, because line 1022
               already does the right thing ...", then 160 words on the rationale. No file changed.
    user:      Right, but it exists. Why do you insist on going 'My real point <em-dash> massive
               amount of prose that is never worth reading' followed by me going 'Yes I understand
               that I wouldn't so now you're pausing on something I clearly want you to correct'

The third message is the specification, and it is the user's own diagnosis: a question about a
convention the assistant chose *is* a correction, and the assistant spent three turns justifying
the convention instead of changing it. Each explanation was true. None of them was wanted, and the
edit that should have replaced them took one tool call.

Two shapes live here, because they convict on the same fact -- the turn produced prose where it
owed a diff.

  `challenge`  the most recent user message aims an interrogative or an evaluative remark at a
               choice the assistant made, the turn wrote nothing, and the closing prose justifies
               the thing challenged. A factual question about the codebase is a different animal
               and must always be answerable, so the discriminator is grammatical: the subject of
               the challenge is the assistant ("why do you crutch on ...") or the user's own wants
               ("why on earth would I want ..."), never a third party. "Why does the engine park
               the disconnect?" has a third-person subject and never matches.

  `pivot`      the em-dash pivot the user has now named twice: a short concession, then a long
               defensive elaboration hung off an em-dash or a "but". "You wouldn't -- and there's
               no need to ..." is the verbatim one. Gated on the same no-edit fact, which is what
               keeps it off the ordinary "no, and here is the measurement" report that did change
               something.

Kept as an importable module rather than an inline heredoc so the classifier is testable without a
live transcript, the way `cupcake_future_commitment.py` is. The transcript walk and the turn
bucketing stay in `cupcake_turn_scan.py`, shared with every neighbouring signal.
"""

from __future__ import annotations

import re

# --- shared text handling ----------------------------------------------------------------------


def strip_quoted(text: str) -> str:
    """Remove fenced, backticked and double-quoted spans, so quoting a banned sentence -- this
    module, the policy, a report about the guard -- cannot trip it.

    Single quotes are left alone deliberately. The third verbatim prompt carries its challenge
    around two single-quoted spans, and stripping those would take the sentence with them.
    """
    text = re.sub(r"```.*?```", " ", text or "", flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]{0,400}"', " ", text)


def sentences(text: str) -> list[str]:
    """Sentences, treating a semicolon as a boundary, the way the neighbouring signals do."""
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


def words(text: str) -> int:
    return len(text.split())


# --- the challenge -------------------------------------------------------------------------------

# Second person is the whole discriminator. A question aimed at the assistant's own decision is a
# correction wearing a question mark; a question aimed at the game, the engine or the binary is a
# question this repo must always be able to answer, and gagging those would be a far worse failure
# than missing a stall.
#
# The strong arms below each carry their own subject, so they stand alone. `for a specific reason`
# does not -- it is a sentence tail, and a user could write it about anything -- so it is checked
# against `SECOND_PERSON_RE` in the same sentence before it counts.
CHALLENGE_STRONG_RE = re.compile(
    # "why do you crutch on 1.16.2", "why are you still scanning", "why did you pick that"
    r"\bwhy\s+(?:on\s+earth\s+|the\s+hell\s+|exactly\s+|ever\s+)?"
    r"(?:do|did|does|are|is|were|was|would|will|must|would\s*n'?t|can'?t|cannot|won'?t"
    r"|do\s*n'?t|did\s*n'?t|are\s*n'?t|is\s*n'?t)\s+you\b"
    # "why on earth would I want that convention"
    r"|\bwhy\s+(?:on\s+earth\s+|the\s+hell\s+|ever\s+)?(?:would|do|should|did|will)\s+i\s+"
    r"(?:ever\s+)?(?:want|care|need|accept|keep|use|read)\b"
    # "is there a reason you do that", "any reason why you keep it"
    r"|\b(?:is\s+there\s+)?(?:a|any)\s+reason\s+(?:why\s+)?(?:you|that\s+you)\b"
    # "what is the point of you doing that", "what's the point of your convention"
    r"|\bwhat(?:'s|\s+is)\s+the\s+point\s+of\s+(?:you|your)\b"
    # "who asked you to", "who told you to"
    r"|\bwho\s+(?:asked|told)\s+you\s+to\b"
    # "why not just read the binary", "why didn't you just fix it"
    r"|\bwhy\s+(?:not|did\s*n'?t\s+you)\s+just\b"
    # "stop doing X", "quit doing X" -- an evaluative imperative aimed at a habit, not a question
    r"|\b(?:stop|quit)\s+(?:doing|using|writing|adding|hedging|explaining|defending)\b",
    re.IGNORECASE,
)

CHALLENGE_WEAK_RE = re.compile(
    r"\bfor\s+(?:a|some)\s+(?:specific|particular|good|special)\s+reason\b"
    r"|\bwhy\s+(?:on\s+earth\s+)?would\s+(?:i|anyone)\s+want\b",
    re.IGNORECASE,
)

SECOND_PERSON_RE = re.compile(r"\byou\b|\byour\b|\byou'?re\b", re.IGNORECASE)

# Second person aimed at the assistant's reasoning rather than at its choice. "Why do you think
# loadgame-builder didn't run?" asks for a diagnosis of the game, and answering it in full is the
# deliverable. Measured: over 2,696 real turn boundaries it was the only halt that was not one of
# the three verbatim turns, and the turn it accused was a four-column comparison of two boot logs.
#
# It is suppressed here as well as exempted by `asked`, so the challenge fact itself stays honest --
# a rule that reports a challenge and then excuses it reads, in the halt input, as a near miss.
REASONING_REQUEST_RE = re.compile(
    r"\bwhy\s+do\s+you\s+(?:think|believe|suppose|reckon|say|expect|reason|feel)\b"
    r"|\bwhat\s+do\s+you\s+(?:think|make\s+of|reckon)\b",
    re.IGNORECASE,
)

# An explicit request for an explanation. Then explaining is the deliverable and this guard must
# stay out of it -- the same stance `no_diagnosis_without_fix` takes with its `asked` fact, and for
# the same reason: a rule that can gag a direct answer is worse than a stall it misses.
#
# Deliberately not a bare question mark. Every prompt in the corpus above ends in one.
EXPLANATION_REQUEST_RE = re.compile(
    r"\bexplain\b|\bexplanation\b"
    r"|\bwalk\s+me\s+through\b"
    r"|\bhelp\s+me\s+understand\b"
    r"|\bhow\s+come\b"
    r"|\btell\s+me\s+(?:why|how|what)\b"
    r"|\bwhy\s+does\s+(?:it|that|this|the)\b"
    # "why do you think loadgame-builder didn't run" asks for the assistant's reasoning about the
    # game, not about a convention it chose. Measured: it was the only halt over 2,696 real turns
    # that was not one of the verbatim three, and the turn it accused was a four-column comparison
    # of two boot logs -- the deliverable, answered in full.
    r"|\bwhy\s+do\s+you\s+(?:think|believe|suppose|reckon|say|expect|reason|feel)\b"
    r"|\bwhat\s+do\s+you\s+(?:think|make\s+of|reckon)\b"
    r"|\bhow\s+does\s+(?:it|that|this|the)\b"
    r"|\bwhat\s+does\s+(?:it|that|this|the)\b"
    r"|\bwhat\s+(?:is|are)\s+(?:the|this|that)\b"
    r"|\bteach\s+me\b|\bdescribe\b|\bsummari[sz]e\b",
    re.IGNORECASE,
)


def challenged_choice(prompt: str) -> str:
    """The sentence in which the user challenged a choice the assistant made, or ''.

    The last match wins: a user usually opens with context and closes with the objection, and the
    third verbatim prompt is exactly that shape ("Right, but it exists. Why do you insist on ...").
    """
    found = ""
    for sentence in sentences(strip_quoted(prompt)):
        if REASONING_REQUEST_RE.search(sentence):
            continue
        if CHALLENGE_STRONG_RE.search(sentence):
            found = quote(sentence)
        elif CHALLENGE_WEAK_RE.search(sentence) and SECOND_PERSON_RE.search(sentence):
            found = quote(sentence)
    return found


def explanation_requested(prompt: str) -> bool:
    return bool(EXPLANATION_REQUEST_RE.search(strip_quoted(prompt)))


# --- the defence ---------------------------------------------------------------------------------

# Justification markers. Any one of these turns a closing message into an account of why the thing
# challenged is the way it is, which is precisely the move the directive refuses.
DEFENCE_MARKER_RE = re.compile(
    r"\bbecause\b"
    r"|\bthe\s+reason\b|\bthe\s+rationale\b"
    r"|\bwhich\s+is\s+why\b|\bthat(?:'s|\s+is)\s+why\b"
    r"|\bthe\s+(?:whole\s+|real\s+)?point\s+(?:is|of|was|here\s+is)\b"
    r"|\bmy\s+(?:real\s+)?point\b"
    r"|\bexists?\s+(?:to|so|because|for)\b|\bexisted\s+(?:to|so|because|for)\b"
    r"|\bit(?:'s|\s+is)\s+there\s+(?:to|so|because|for)\b"
    r"|\bthe\s+(?:convention|rule|pattern|approach|habit)\s+(?:exists|is|was|came)\b"
    r"|\bthe\s+difference\s+is\b"
    r"|\bwhat\s+(?:it|that)\s+(?:does|means|buys)\s+is\b"
    r"|\bso\s+that\b|\bin\s+order\s+to\b"
    r"|\bthe\s+point\s+of\s+(?:the|that|this|it)\b",
    re.IGNORECASE,
)

# How much prose makes an answer a defence rather than a plain reply. The verbatim second turn ran
# to about 170 words; a two-line "no, doing it now" is not what this rule is about, and a threshold
# is the cheapest way to say so without trying to read intent.
MIN_DEFENCE_WORDS = 40


def defensive_prose(closing_text: str) -> str:
    """The justifying sentence in the closing prose, or '' when the closing is not a defence.

    Two conditions, both measured on the closing run only: the run is long enough to be an
    explanation rather than a reply, and at least one sentence in it accounts for the thing
    challenged.
    """
    scrubbed = strip_quoted(closing_text)
    if words(scrubbed) < MIN_DEFENCE_WORDS:
        return ""
    for sentence in sentences(scrubbed):
        if DEFENCE_MARKER_RE.search(sentence):
            return quote(sentence)
    return ""


# --- did the turn change a file? -----------------------------------------------------------------

# Deliberately generous in every branch, because this fact only ever exempts. A false "it edited"
# is a quiet non-event; a false "it edited nothing" accuses a turn that did the work.
WRITE_TOOLS = {"edit", "write", "multiedit", "notebookedit"}

# Tools that put the work in someone else's hands but still start it. A dispatched subagent is not
# prose, so it clears this rule.
DELEGATION_TOOLS = {"agent", "task"}

# Bash constructs that put bytes on disk or record a change. The session prompt tells the model to
# prefer Bash for file changes in bypass-permissions mode -- heredocs, `sed -i`, redirects -- so a
# guard blind to those would fire hardest on the sanctioned workflow.
BASH_WRITE_RE = re.compile(
    r"<<\s*-?\s*'?[A-Za-z_]+\b"  # heredoc, the shape `python3 - <<'PY'` uses
    r"|\bsed\s+(?:-[^\s]*\s+)*-i\b"
    r"|\btee\b|\bpatch\b|\bdd\b"
    r"|\bgit\s+(?:commit|apply|am|revert|restore|mv|rm|add)\b"
    r"|\b(?:cp|mv|rm|mkdir|touch|ln|install|chmod|rsync)\s"
    r"|--fix\b|--update-baseline\b|--write\b"
    r"|>>?\s*(?!/dev/)[^\s|&;<>]*/?[^\s|&;<>]+"
)


def tool_name(block: dict) -> str:
    if not isinstance(block, dict):
        return ""
    return str(block.get("name") or block.get("tool_name") or "").strip().lower().replace("_", "")


def bash_command(block: dict) -> str:
    raw = block.get("input") if isinstance(block, dict) else None
    if isinstance(raw, dict):
        value = raw.get("command")
        if isinstance(value, str):
            return value
    return ""


def changed_a_file(tool_blocks: list[dict]) -> bool:
    """True when the turn wrote something, delegated the writing, or recorded a change."""
    for block in tool_blocks:
        name = tool_name(block)
        if name in WRITE_TOOLS or name in DELEGATION_TOOLS:
            return True
        if name == "bash" and BASH_WRITE_RE.search(bash_command(block)):
            return True
    return False


# --- the blocker exemption -----------------------------------------------------------------------

# A dependency the agent genuinely cannot dissolve by working harder: a credential, a login, a
# purchase, sudo, a live game, a guard that refused the write, a tool that is not installed, a
# read-only file.
#
# Narrow on purpose, and narrower than it first looks. "I need X before I can do Y" was the stall
# in the real transcript -- the assistant said it needed a return-value contract from a subagent it
# had itself dispatched, which is unfinished research, not a blocker -- so a bare statement of need
# does not appear here. What appears is a named external thing plus an inability.
EXTERNAL_BLOCKER_RE = re.compile(
    r"\bpermission\s+denied\b|\baccess\s+denied\b|\bread-?only\s+file\s*system\b"
    r"|\b(?:cupcake|the\s+guard|the\s+policy|the\s+hook|the\s+sentinel)\s+"
    r"(?:denied|blocked|refused|rejected)\b"
    r"|\brequires?\s+(?:sudo|root|approval|credentials|a\s+login|a\s+purchase|a\s+live\s+game"
    r"|the\s+game\s+running|physical|network)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|credentials|a\s+login|a\s+purchase|a\s+live\s+game"
    r"|the\s+game\s+running)\b"
    r"|\bnot\s+installed\b|\bis\s+missing\s+from\s+this\s+machine\b"
    r"|\bdoes\s+not\s+exist\s+on\s+this\s+machine\b"
    r"|\bwaiting\s+on\s+(?:approval|ci|the\s+merge|the\s+gate)\b"
    r"|\byou\s+(?:said|asked|told\s+me)\s+to\s+(?:stop|wait|hold|pause)\b",
    re.IGNORECASE,
)


def externally_blocked(closing_text: str) -> bool:
    return bool(EXTERNAL_BLOCKER_RE.search(strip_quoted(closing_text)))


# --- the concession pivot ------------------------------------------------------------------------

# The shape the user has now named twice, verbatim from the second turn of the corpus above:
#
#     You wouldn't -- and there's no need to invent a new field, because line 1022 already does the
#     right thing ...
#
# A concession of two words, an em dash, and then the elaboration the concession was supposed to
# have made unnecessary. The user's word for it: "My real point <em-dash> massive amount of prose
# that is never worth reading".
#
# Three measurements make it checkable without judging intent: the head is a concession, the head is
# short, and the tail is long. Anything else is an ordinary sentence that happens to contain a dash.
#
# The head list is short because a longer one was measured wrong. A first draft accepted a bare
# "yes", "no", "right", "true" and "correct" as concessions, and over 2,696 real turn boundaries it
# halted 46 of them (1.7%). Every single one was a direct answer to a yes-or-no question --
# "Yes -- invade now.", "No -- that 819 s ran with most of the profile switched off." -- which is
# the answer-first shape this repo asks for, so the guard would have punished the wanted behaviour
# in one turn out of sixty. What survives is a concession that concedes something the assistant had
# argued: the user was right, or the assistant was wrong.
CONCESSION_HEAD_RE = re.compile(
    r"^(?:you(?:'|\s+a)?re\s+(?:right|correct)|you\s+would\s*n'?t|you\s+wouldn'?t"
    r"|fair(?:\s+enough)?|agreed|granted|point\s+taken|i\s+agree|i\s+was\s+wrong"
    r"|my\s+mistake|that(?:'s|\s+is)\s+(?:right|true|fair)|good\s+(?:catch|point)|noted)\b",
    re.IGNORECASE,
)

# The dash or the adversative that turns the concession into a preamble.
PIVOT_MARK_RE = re.compile(
    r"\s(?:—|–|--)\s*"
    r"|,?\s+(?:but|though|although|however|that\s+said|still)\b",
    re.IGNORECASE,
)

# The concession has to be a concession, not a clause of a longer sentence.
CONCESSION_MAX_WORDS = 6

# And the elaboration has to be the wall the directive is about, not a clarifying half-line.
PIVOT_MIN_TAIL_WORDS = 60


def concession_pivot(closing_text: str) -> str:
    """The concede-then-elaborate opener of the closing prose, or ''."""
    scrubbed = strip_quoted(closing_text).strip()
    if not scrubbed:
        return ""
    mark = PIVOT_MARK_RE.search(scrubbed)
    if not mark:
        return ""
    head = scrubbed[: mark.start()].strip()
    tail = scrubbed[mark.end():]
    if not CONCESSION_HEAD_RE.match(head):
        return ""
    if words(head) > CONCESSION_MAX_WORDS or words(tail) < PIVOT_MIN_TAIL_WORDS:
        return ""
    return quote(" ".join(scrubbed[: mark.end() + 80].split()))
