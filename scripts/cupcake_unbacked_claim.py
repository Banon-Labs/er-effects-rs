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
        if _CLAIM.search(sentence) and _ARTIFACT.search(sentence):
            hits.append(" ".join(sentence.split())[:220])
    return hits


def offending_claim(final_text: str, events: list[dict]) -> str:
    """The first unbacked completion claim in the closing prose, or '' when the turn is clean."""
    if turn_wrote_a_file(events):
        return ""
    hits = claim_sentences(final_text)
    return hits[0] if hits else ""
