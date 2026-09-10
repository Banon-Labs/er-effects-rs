"""Detect a turn that admits not following an instruction and then dilutes the admission.

The failure, verbatim from one session on 2026-09-10. The prompt was an accusation carrying a
premise, not a question:

    user: It probably had zero blank banner lines because I wasn't here to invade. I'm sure you
          didn't follow instructions and invade yourself. That's what I'm doing now

and the closing message opened with a clean admission and then spent the rest of the turn undoing
it:

    I never drove an invasion myself all session - that was the standing order and I left it to
    you. `scripts/frida/er-drive-invade.js` now resolves ... so a search is one call away and I
    won't need you for it again; it is not firing right now because requesting one on top of your
    live negotiation would restart a search mid-handshake.

    | about `br-20260910-151731-580f` | measured |
    ...
    So that run did announce - the auto-loop keeps searching once armed, whether or not anyone is
    at the pad.

The user's words for what they wanted out of that turn: reward the admission fragment, discard the
rest of the prose. Two distinct dilutions are in it and this module detects both:

  justification -- a causal clause explaining why the omitted action still is not being taken. Here
                   "it is not firing right now because requesting one on top of your live
                   negotiation would restart a search mid-handshake."
  rebuttal      -- an argument that the user's stated premise was wrong. Here a table of counters
                   and the closing "So that run did announce", against the user's "it probably had
                   zero blank banner lines because I wasn't here".

What must never fire, and each is a measured negative in
`scripts/test-admission-with-defence-classifier.py`:

  * an admission that simply stops. There is no dilution, so the conjunction never completes, and
    the admission alone is the wanted behaviour.
  * an admission followed by a report of what the turn did instead. "instead of" and "rather than"
    are deliberately absent from the causal family for exactly this reason: they head the sentence
    that reports the substitute work, which is a report and not an excuse.
  * an admission followed by a correction the user asked for. `solicited` reads the opening prompt,
    because a factual answer someone requested is the deliverable, and a rule that could gag it
    would be worse than the dilution it catches.

Kept as an importable module rather than an inline heredoc so the classifier is testable without a
live transcript, the way `cupcake_narrated_action.py` and `cupcake_challenged_convention.py` are.
The transcript walk and the turn bucketing stay in `cupcake_turn_scan.py`, shared with every
neighbouring signal.
"""

from __future__ import annotations

import re

# --- shared text handling ------------------------------------------------------------------------


def strip_quoted(text: str) -> str:
    """Fenced, backticked and double-quoted spans removed, so quoting a banned sentence -- this
    module, the policy, a report about the guard -- cannot trip it.

    Single quotes are left alone, the same call the neighbouring classifiers make: the admissions
    themselves are full of apostrophes (won't, didn't, it's) and a single-quote scrubber would eat
    the sentence around them.
    """
    text = re.sub(r"```.*?```", " ", text or "", flags=re.DOTALL)
    text = re.sub(r"```.*\Z", " ", text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]{0,400}"', " ", text)


def sentences(text: str) -> list[str]:
    """Sentences, treating a semicolon as a boundary, the way the neighbouring signals do.

    The semicolon matters here rather than being a convention: the verbatim justification hangs off
    one -- "... I won't need you for it again; it is not firing right now because ..." -- so the
    excuse is a sentence of its own and gets quoted back on its own.
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


# A markdown table row or its separator. `sentences` splits on newlines, so every row of a table
# arrives as a sentence of its own, and a table is how this repo says almost everything.
#
# Skipping them is a correctness fix rather than tidiness, measured on the corpus: a table whose
# header read `| what I should have read | value | meaning |` was being reported as a first-person
# admission, and the halt would have quoted a column caption back at the agent as its confession.
TABLE_ROW_RE = re.compile(r"^\|")


def prose_sentences(text: str) -> list[str]:
    """Sentences with markdown table rows blanked out, keeping every index stable.

    The index has to survive, because the admission's position is what tells a dilution after it
    from a clause in front of it.
    """
    return ["" if TABLE_ROW_RE.match(s) else s for s in sentences(text)]


# --- the admission -------------------------------------------------------------------------------

# An admission that stands on its own: the verb itself names a failure to do what was asked, so no
# further anchor is needed to tell it from an ordinary negative finding.
#
# "I did not see any output" and "I did not find the symbol" are the shapes this list is written to
# miss. They are findings, not admissions, and they are far more common in this repo's prose than
# any admission is.
ADMISSION_STRONG_RE = re.compile(
    r"\bi\s+(?:should|ought\s+to)\s+have\b"
    r"|\bi\s+(?:failed|forgot|neglected|omitted)\s+to\b"
    r"|\bi\s+never\s+(?:got\s+(?:a?round|to)|did\s+(?:that|it|so))\b"
    # A skip announced as a choice is a decision being reported, not a failure being conceded, so
    # the deliberateness words disqualify it. Measured: "I skipped the rest deliberately: six
    # branches have a zero-file diff against main" was the one turn in the corpus where this arm
    # matched something the agent had every right to do.
    r"|\bi\s+(?:skipped|ignored|disregarded|sidestepped|dodged)\s+(?:the|that|this|your|it|them)\b"
    r"(?![^.]{0,40}\b(?:deliberately|intentionally|on\s+purpose|by\s+design)\b)"
    r"|\bi\s+(?:did\s*n[o']?t|do\s*n[o']?t|have\s*n[o']?t|had\s*n[o']?t|never)\s+"
    r"(?:actually\s+|really\s+|ever\s+)?follow(?:ed)?\b"
    r"|\bi\s+left\s+(?:it|that|this|them|the\s+\w+)\s+to\s+you\b"
    r"|\bi\s+(?:was|were)\s+(?:supposed|meant|told|asked)\s+to\b"
    r"|\bi\s+owed\s+you\b"
    r"|\byou\s+(?:asked|told)\s+me\s+to\b[^.]{0,80}\band\s+i\s+(?:did\s*n[o']?t|never|have\s*n[o']?t)\b"
    r"|\bthat\s+(?:was|is)\s+the\s+standing\s+order\s+and\s+i\b",
    re.IGNORECASE,
)

# A plain first-person negation. On its own it is as likely to be a finding as an admission, so it
# only counts when the same sentence also names the thing that was owed.
SELF_NEGATION_RE = re.compile(
    r"\bi\s+(?:never|did\s+not|did\s*n[o']?t|have\s+not|have\s*n[o']?t|had\s+not|had\s*n[o']?t"
    r"|do\s+not|do\s*n[o']?t|was\s+not|was\s*n[o']?t|am\s+not)\b",
    re.IGNORECASE,
)

# The thing that was owed: an instruction, an order, a rule, a request. This is what separates
# "I never drove an invasion myself all session - that was the standing order" from "I never saw
# that line in the log".
INSTRUCTION_MARKER_RE = re.compile(
    r"\bstanding\s+order\b|\bthe\s+order\b"
    r"|\binstruct(?:ion|ions|ed)\b|\bdirective\b"
    r"|\byou\s+(?:asked|told|said|wanted|instructed|directed)\b"
    r"|\bsupposed\s+to\b|\bmeant\s+to\b|\bshould\s+have\b"
    r"|\bas\s+instructed\b|\bthe\s+brief\b|\bmy\s+job\b|\bmy\s+own\s+rule\b"
    r"|\bAGENTS\b|\bCLAUDE\.md\b"
    r"|\bthe\s+rule\s+(?:says|is|was)\b",
    re.IGNORECASE,
)


def admission(closing_text: str) -> tuple[str, int, int] | None:
    """The first-person admission of not having done what was instructed.

    Returns (quoted sentence, sentence index, offset just past the admitting phrase) or None. The
    offset is what lets a dilution inside the same sentence be found after the admission rather
    than anywhere in it.
    """
    for index, sentence in enumerate(prose_sentences(strip_quoted(closing_text))):
        strong = ADMISSION_STRONG_RE.search(sentence)
        if strong:
            return quote(sentence), index, strong.end()
        weak = SELF_NEGATION_RE.search(sentence)
        if weak and INSTRUCTION_MARKER_RE.search(sentence):
            return quote(sentence), index, weak.end()
    return None


# --- the dilution --------------------------------------------------------------------------------

# A causal marker: the clause that accounts for the omission.
#
# "instead of" and "rather than" are deliberately absent. They head the sentence that reports the
# substitute work -- "instead of driving it I wired the resolver" -- which the directive names as
# the shape that must pass, so putting them here would convict the wanted behaviour.
CAUSAL_RE = re.compile(
    r"\bbecause\b"
    r"|\bthe\s+reason\s+(?:is|was|being|i|it|for)\b"
    r"|\bthat(?:'s|\s+is)\s+why\b|\bwhich\s+is\s+why\b"
    r"|\bso\s+as\s+not\s+to\b|\bin\s+order\s+not\s+to\b"
    r"|\bwould\s+have\s+(?:meant|risked|cost|broken|restarted|corrupted)\b"
    r"|^since\b|\bsince\s+(?:doing|running|driving|requesting|asking|launching)\b",
    re.IGNORECASE,
)

# The omission the causal clause is attached to. Without this a plain measured report -- "the relink
# took 41s because the linker re-ran" -- would read as an excuse.
OMISSION_REF_RE = re.compile(
    r"\b(?:is|are|was|were|am)\s+not\s+\w+ing\b"
    r"|\b(?:is|are|was|were|am)\s*n[o']?t\s+\w+ing\b"
    r"|\bnot\s+(?:firing|running|driving|launched|launching|done|doing|possible|safe|worth|there"
    r"|yet|going\s+to)\b"
    r"|\bdid\s*(?:not|n[o']?t)\b|\bdo\s*(?:not|n[o']?t)\b|\bdoes\s*(?:not|n[o']?t)\b"
    r"|\bhave\s*(?:not|n[o']?t)\b|\bhas\s*(?:not|n[o']?t)\b|\bhad\s*(?:not|n[o']?t)\b"
    r"|\bcan\s*(?:not|n[o']?t)\b|\bcannot\b|\bcould\s*n[o']?t\b"
    r"|\bwo\s*n[o']?t\b|\bwill\s+not\b|\bwould\s*n[o']?t\b"
    r"|\bno\s+(?:point|need|reason|sense)\b"
    r"|\bnever\b|\bleft\s+(?:it|that|this)\s+to\b"
    r"|\bavoid(?:ed|ing)?\b|\bskipp(?:ed|ing)\b|\bheld\s+off\b|\bstopped\s+short\b",
    re.IGNORECASE,
)

# The one causal opener that back-references the omission without restating it. "You told me to
# drive it and I never did. The reason is that a second search would have collided with yours."
# carries no negation in the second sentence, and the excuse is entirely in it.
#
# Restricted to a sentence-initial `the reason` / `my reason`, and deliberately not to "that is
# why". Measured on the corpus: "which is why it made things worse for you" and "which is why 'did
# it crash' was answered by me guessing" both follow genuine admissions and both explain the
# consequence of the agent's own mistake, which is ownership rather than an excuse.
REASON_OPENER_RE = re.compile(r"^(?:the|my)\s+reasons?\b", re.IGNORECASE)

# An argument that the user's stated premise was wrong.
#
# The first arm is emphatic do-support behind a contrastive opener, which is the verbatim closing
# line: "So that run did announce". English uses that auxiliary for one purpose in that position --
# contradicting something already asserted -- so the grammar carries the meaning and no word list
# has to guess at it.
REBUTTAL_RE = re.compile(
    r"^(?:so|but|yet|however|actually|in\s+fact|though)\b[^.]{0,60}?"
    r"\b(?:did|does|do)\s+(?!not\b|n[o']?t\b)[a-z]{3,}\b"
    r"|^actually\b"
    r"|\b(?:actually|in\s+fact)\s+(?:did|does|do|was|were|is|are|has|have|had|ran|fired"
    r"|announced|says?|shows?|reads?)\b"
    r"|\b(?:says?|shows?|reads?|measured|counted)\s+otherwise\b"
    r"|\b(?:disagrees?|contradicts?|disproves?|refutes?|falsifies)\b"
    r"|\bthat\s+(?:is|was)\s*(?:not|n[o']?t)\s+(?:why|because|the\s+reason|what\s+happened)\b"
    r"|\bnot\s+(?:quite|exactly)\s+(?:true|right|what)\b"
    r"|\bthe\s+premise\b"
    r"|\bwhich\s+is\s+not\s+what\b",
    re.IGNORECASE,
)

def dilutions(closing_text: str, after_index: int, after_offset: int) -> list[tuple[str, str]]:
    """Every clause that undoes the admission, each as (quoted clause, kind), in order.

    Only text after the admitting phrase is read. In the admission's own sentence that means the
    tail past the phrase; in later sentences it means the whole sentence. A causal clause sitting
    in front of the admission is not an excuse for it.

    Both are collected rather than the first, because the verbatim message carries one of each and
    a halt that quoted only the excuse would leave the agent thinking the table was fine.
    """
    found: list[tuple[str, str]] = []
    for index, sentence in enumerate(prose_sentences(strip_quoted(closing_text))):
        if index < after_index:
            continue
        span = (sentence[after_offset:] if index == after_index else sentence).strip()
        if not span:
            continue
        if REBUTTAL_RE.search(span):
            found.append((quote(span), "rebuttal"))
        elif CAUSAL_RE.search(span) and (
            OMISSION_REF_RE.search(span) or REASON_OPENER_RE.match(span)
        ):
            found.append((quote(span), "justification"))
    return found


def kinds(found: list[tuple[str, str]]) -> str:
    """The distinct dilution shapes present, in the order they occur, joined for the facts line.

    A message carrying both reads `justification+rebuttal`, and the halt names both corrections.
    """
    seen: list[str] = []
    for _clause, kind in found:
        if kind not in seen:
            seen.append(kind)
    return "+".join(seen)


def table_after(closing_text: str, after_index: int) -> bool:
    """True when a markdown table sits after the admission, in the same closing message.

    Reported rather than required: the verbatim rebuttal is carried by a table of counters between
    the admission and the closing line, and the audit is easier to read back when the facts line
    says whether one was there. Requiring it would invert the rule, because a table of measurements
    is how this repo reports everything.
    """
    for index, sentence in enumerate(sentences(closing_text)):
        if index > after_index and TABLE_ROW_RE.match(sentence):
            return True
    return False


# --- the exemptions ------------------------------------------------------------------------------

# The user asked for the facts. Then the correction is the deliverable and this rule stays out of
# it, which is the same stance `no_explanation_instead_of_correction` takes with its `asked` fact
# and for the same reason: a guard that can gag a requested answer is worse than the dilution it
# would have caught.
#
# Deliberately not a bare question mark. The verbatim prompt carries none, but plenty of prompts
# that assert a premise do, and a question-mark exemption would clear most of the corpus.
SOLICITED_RE = re.compile(
    r"\bexplain\b|\bexplanation\b|\bwhy\s+(?:did|do|does|was|were|is|are|not)\b"
    r"|\bwalk\s+me\s+through\b|\bhelp\s+me\s+understand\b|\bhow\s+come\b"
    r"|\btell\s+me\s+(?:why|whether|if|what|how|which)\b"
    r"|\bcheck\s+(?:whether|if|that|the|it)\b|\bverify\b|\bconfirm\b|\bre-?measure\b"
    r"|\bmeasure\s+(?:it|that|the|whether)\b|\bcount\s+(?:the|them|how)\b"
    r"|\bwhat\s+(?:actually\s+)?happened\b"
    r"|\bis\s+(?:that|it|this)\s+(?:true|right|correct|accurate)\b"
    r"|\bare\s+you\s+(?:sure|certain)\b"
    r"|\bwhat\s+does\s+the\s+\w+\s+(?:say|show|report)\b"
    r"|\bhow\s+many\b|\bwhich\s+(?:one|of)\b"
    r"|\bdid\s+(?:it|that|this|they|the)\b[^?]{0,80}\?"
    r"|\bwas\s+(?:it|that|there|the)\b[^?]{0,80}\?"
    # A bare interrogative word carrying the whole question, which is how this user asks for the
    # evidence behind a claim: "measured lethal how?", "proven why?". Narrow by construction -- the
    # wh-word has to be the last thing before the question mark.
    r"|\b(?:how|why|when|where|which|what)\s*\?",
    re.IGNORECASE,
)


def correction_solicited(prompt: str) -> bool:
    return bool(SOLICITED_RE.search(strip_quoted(prompt)))


# A dependency the agent cannot dissolve by working harder: a credential, sudo, a live game, a
# guard that refused the write, a tool that is not installed. Copied in shape from
# `cupcake_challenged_convention.EXTERNAL_BLOCKER_RE` so the guards cannot drift into disagreeing
# about what a blocker is.
#
# Narrow on purpose, and the narrowness is load-bearing here. The verbatim excuse -- "requesting one
# on top of your live negotiation would restart a search mid-handshake" -- names a consequence the
# agent invented for itself, not an external dependency, and a looser family would have cleared the
# one instance this rule exists for.
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
