"""Detect a closing message that claims a repo artifact was built when the turn wrote no file.

Sibling of the unexecuted-promise scan: that one catches "I'll build it" ending in nothing, this
one catches "I built it" when nothing was built. See
.cupcake/signals/last_assistant_unbacked_claim.sh for the incident that forced it.

Kept as an importable module rather than inline heredoc so it is unit-testable without a live
transcript -- the sibling guard's logic lives in cupcake_turn_scan.py for the same reason.
"""

from __future__ import annotations

import re

# First-person completion verbs. Present-perfect and simple past only: a future promise is the
# sibling guard's job, and matching both here would double-halt one turn.
_CLAIM = re.compile(
    r"\bI(?:'ve| have)?\s+(?:just\s+|already\s+)?"
    r"(built|added|created|wrote|written|wired|landed|shipped|implemented|patched|updated|"
    r"removed|deleted|moved|made|taught|extended|hooked)\b",
    re.IGNORECASE,
)

# The claim only counts when its object is something in this repo. A claim about the game, a run,
# or an external service is not what this guard is for.
_ARTIFACT = re.compile(
    r"(?:\b(?:scripts|crates|tools|docs|\.cupcake)/[\w./-]+"
    r"|\b[\w.-]+\.(?:py|rego|rs|toml|sh|yml|yaml|json|md)\b"
    r"|\b(?:gate|check|hook|policy|guard|selftest|test|script|lint|signal|rulebook)s?\b)",
    re.IGNORECASE,
)

# An honest confession of absence must never be punished -- it is the behaviour being asked for.
_DISCLAIMED = re.compile(
    r"\b(?:no|not|nothing|never|neither)\b[^.]{0,80}?"
    r"\b(?:built|created|exists?|wrote|written|added|shipped|landed|implemented|wired)\b"
    r"|\bI\s+(?:have\s+not|haven't|did\s+not|didn't)\b"
    r"|\bstill\s+(?:need|needs|to\s+be)\b"
    r"|\bunbuilt\b|\bnot\s+(?:yet\s+)?(?:built|written|created|wired)\b",
    re.IGNORECASE,
)

# Bash constructs that actually put bytes on disk. `bd remember` is deliberately absent: recording
# a memory instead of doing the work is the exact substitution this guard exists to catch.
_BASH_WRITE = re.compile(
    r">>?\s*[\w./~$-]"          # redirect into a file
    r"|\btee\b"
    r"|\bsed\b[^|;]*\s-i\b"
    r"|<<\s*'?[A-Z]"            # heredoc (python3 - <<'PY', cat > f <<'EOF')
    r"|\b(?:cp|mv|install|patch|touch|mkdir)\b"
    r"|\bgit\s+(?:apply|checkout|revert|restore|merge|cherry-pick)\b"
    r"|\bchmod\b",
)

_WRITE_TOOLS = ("Edit", "Write", "NotebookEdit", "MultiEdit")

# Punctuation that closes whatever came before it, so the next word begins a clause. Markdown
# emphasis markers and the table pipe are in the set because this repo's prose is full of both.
_CLAUSE_BREAK = set(".,;:!?()[]{}*_|\"'—–-\n")

# Words that introduce a clause instead of being modified by one. Deliberately excludes the
# relativizers "that", "which" and "who": those are the explicit spelling of the shape below.
_CLAUSE_OPENER = {
    "and", "but", "so", "or", "yet", "nor", "then", "because", "since", "after", "before",
    "when", "while", "if", "once", "though", "although", "however", "therefore", "thus",
    "meanwhile", "instead", "plus",
}


def _opens_a_clause(before: str) -> bool:
    """True when a claim starting where `before` ends is asserting, not referring back.

    Measured over 2,370 real turns on 2026-09-09, the first time this guard could fire at all: it hit
    ten turns and nine of them read "the gate I wrote", "the staleness test I shipped", "the function
    I hooked", "an importer I built". Those are relative clauses with the relativizer dropped, and
    grammatically they do the opposite of what the guard is looking for -- they presuppose the
    artifact as already given and go on to say something else about it, which is ordinary reporting
    prose. Only "I wrote a cupcake guard" asserts the creation, and assertion is the whole violation.

    The test is positional, not semantic: the pronoun must open its clause. Start of text, any
    closing punctuation, or a conjunction before it means it does. An ordinary word before it means
    the pronoun is inside a noun phrase, so the sentence refers rather than claims.
    """
    tail = before.rstrip()
    if not tail:
        return True
    if tail[-1] in _CLAUSE_BREAK:
        return True
    return tail.split()[-1].lower().strip("".join(_CLAUSE_BREAK)) in _CLAUSE_OPENER


def _asserts_completion(sentence: str) -> bool:
    """True when the sentence carries a completion claim that opens its own clause."""
    return any(_opens_a_clause(sentence[: m.start()]) for m in _CLAIM.finditer(sentence))


def strip_quoted(text: str) -> str:
    """Remove fenced/backticked/quoted spans so quoting a claim cannot trip the guard."""
    text = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    text = re.sub(r'"[^"]{0,400}"', " ", text)
    return text


def turn_wrote_a_file(events: list[dict]) -> bool:
    """True when any tool_use in the turn actually mutated the working tree."""
    for ev in events or []:
        for block in (ev.get("message", {}) or {}).get("content", []) or []:
            if not isinstance(block, dict) or block.get("type") != "tool_use":
                continue
            if block.get("name") in _WRITE_TOOLS:
                return True
            if block.get("name") == "Bash":
                cmd = (block.get("input") or {}).get("command", "")
                if isinstance(cmd, str) and _BASH_WRITE.search(cmd):
                    return True
    return False


def claim_sentences(text: str) -> list[str]:
    """Sentences in `text` that assert a repo artifact was built, minus disclaimed ones."""
    hits = []
    for raw in re.split(r"(?<=[.!?;])\s+|\n", strip_quoted(text or "")):
        sentence = raw.strip()
        if not sentence or _DISCLAIMED.search(sentence):
            continue
        if _asserts_completion(sentence) and _ARTIFACT.search(sentence):
            hits.append(" ".join(sentence.split())[:220])
    return hits


def offending_claim(final_text: str, events: list[dict]) -> str:
    """The first unbacked completion claim in the closing prose, or '' when the turn is clean."""
    if turn_wrote_a_file(events):
        return ""
    hits = claim_sentences(final_text)
    return hits[0] if hits else ""
