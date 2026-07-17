#!/usr/bin/env bash
# Cupcake signal: last_assistant_authority_agreement
#
# Scans the most recently COMPLETED assistant turn of the current session transcript and returns a
# TAGGED banned-phrase marker, or empty if the turn is clean. Consumed by TWO policies:
#   * no_authority_agreement (Stop): halts turn-end so the agent must correct.
#   * no_authority_agreement_reminder (UserPromptSubmit): injects a mandatory correction directive.
#
# TWO BANNED CLASSES (2026-07-17 directives):
#   Category A -- AUTHORITY-CODED AGREEMENT ("You're right", "That's right", "Correct,", "Exactly,",
#     "Absolutely,", "Precisely,"). Banned OUTRIGHT. Emitted as  AUTH:<phrase>.
#   Category B -- FEEDBACK-ACKNOWLEDGEMENT / receipt-announcement prose ("Point taken", "Got it",
#     "Understood", "Noted", "Fair point", "Makes sense", ...). This ANNOUNCES that the agent received
#     feedback; it is only acceptable when the agent actually internalized it by recording a beads
#     memory in the SAME turn (a Bash tool_use running `bd remember`). So it is banned ONLY when the
#     turn contains NO bd-memory recording. Emitted as  ACKUNBACKED:<phrase>  (and suppressed entirely
#     when the turn has a bd-memory write).
# Category A always wins over Category B (checked first, regardless of any bd-memory write).
# A clean turn emits empty. Consumers also treat any NON-EMPTY UNTAGGED value as a Category-A hit
# (backward compat with crafted/bare signal values).
#
# WHY A WHOLE-TURN SCAN, NOT JUST THE LAST MESSAGE (2026-07-17 fix): the old signal kept only the LAST
# assistant text block, so (a) a slip in an EARLIER message of a multi-message turn was overwritten by a
# later clean block and escaped, and (b) when the user INTERRUPTS a turn, the Stop event never fires at
# all -- the halt could not catch it. Scanning the whole last-completed turn fixes (a); routing the SAME
# signal into the UserPromptSubmit reminder (which ALWAYS runs on the next prompt, even after an
# interrupt) fixes (b). "Last completed turn" = the last non-empty run of assistant text bounded by real
# user prompts; on UserPromptSubmit the just-submitted prompt opens a new empty run, so the prior turn is
# still the last NON-EMPTY one -- the same value both events need. The bd-memory flag is bucketed into
# the SAME turns so the Category-B exception is evaluated against the turn that produced the ack.
#
# Double-quoted spans are stripped before matching so QUOTING the ban (this file, the reminder text, or a
# meta-discussion like `the phrase "You're right"`) does not false-trip; a real unquoted slip
# (`You're right, ...`) still matches. Fail-open (empty output) on any error so a transcript hiccup
# cannot wedge the session.
set -uo pipefail
python3 - <<'PY' 2>/dev/null || true
import glob, json, os, re, sys

cwd = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
key = cwd.replace("/", "-")
tdir = os.path.join(os.path.expanduser("~/.claude/projects"), key)
files = sorted(glob.glob(os.path.join(tdir, "*.jsonl")),
               key=lambda p: os.path.getmtime(p), reverse=True)
if not files:
    sys.exit(0)


def is_real_user_prompt(ev):
    """A genuine user prompt starts a new turn. Tool-result 'user' events do NOT (they are the harness
    handing tool output back mid-turn), so they must not split the assistant turn."""
    if ev.get("type") != "user":
        return False
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content.strip() != ""
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "tool_result":
                return False  # tool-result carrier, not a prompt
        # any non-tool_result content (text block, or plain) counts as a prompt
        return True
    return False


def assistant_text(ev):
    out = []
    for block in ev.get("message", {}).get("content", []) or []:
        if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
            out.append(block["text"])
    return "\n".join(out)


# A bd-memory recording = a Bash tool_use whose command runs `bd remember` (accept both the bare `bd`
# and the local-bin path form). DOTALL so a multiline command still matches.
BD_MEMORY_RE = re.compile(r"(?:\bbd\b|/\.local/bin/bd\b).*\bremember\b", re.IGNORECASE | re.DOTALL)


def assistant_has_bd_memory(ev):
    for block in ev.get("message", {}).get("content", []) or []:
        if not isinstance(block, dict):
            continue
        if block.get("type") == "tool_use" and block.get("name") == "Bash":
            cmd = (block.get("input") or {}).get("command", "")
            if isinstance(cmd, str) and BD_MEMORY_RE.search(cmd):
                return True
    return False


# Bucket assistant text AND bd-memory writes into turns delimited by real user prompts; keep the last
# bucket that has any text (its own bd flag decides the Category-B exception).
turns = [{"text": [], "bd": False}]
try:
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if is_real_user_prompt(ev):
                turns.append({"text": [], "bd": False})
            elif ev.get("type") == "assistant":
                t = assistant_text(ev)
                if t:
                    turns[-1]["text"].append(t)
                if assistant_has_bd_memory(ev):
                    turns[-1]["bd"] = True
except OSError:
    sys.exit(0)

last_turn = ""
turn_has_bd_memory = False
for bucket in reversed(turns):
    if bucket["text"]:
        last_turn = "\n".join(bucket["text"])
        turn_has_bd_memory = bucket["bd"]
        break

# Strip DOUBLE-quoted spans so quoting the ban does not count as using it (single quotes are left alone
# because the phrases themselves contain apostrophes, e.g. you're).
scrubbed = re.sub(r'"[^"]*"', " ", last_turn)


def phrase(m):
    for g in m.groups():
        if g:
            return g.strip()
    return m.group(0).strip()


# Category A: authority-coded AGREEMENT, not incidental words ("the correct offset").
AUTH_RE = re.compile(
    r"\b(you'?re\s+right|you\s+are\s+right|that'?s\s+right|you'?re\s+correct|you\s+are\s+correct)\b"
    r"|(?:^|[.!?]\s+|\n)\s*(correct|exactly|absolutely|precisely)[,.! ]",
    re.IGNORECASE | re.MULTILINE,
)

# Category B: feedback-acknowledgement / receipt-announcement prose.
# Distinctive multi-word receipts occur anywhere (word-boundaried); they do not appear incidentally in
# technical prose.
ACK_ANYWHERE_RE = re.compile(
    r"\b(point taken|good point|fair point|fair enough|duly noted|message received"
    r"|lesson learned|i hear you|will do|take that on board|i'?ll internalize"
    r"|(?:that|this|it)\s+makes sense)\b",
    re.IGNORECASE,
)
# Ambiguous words/short phrases: only when they OPEN a sentence as a standalone receipt, so
# "as noted above" / "I understood the code" / "get everyone on board" do not false-trip.
ACK_SENTENCE_INITIAL_RE = re.compile(
    r"(?:^|[.!?]\s+|\n)\s*"
    r"(got it|understood|noted|makes sense|on board)"
    r"[.!,: ]",
    re.IGNORECASE | re.MULTILINE,
)

mA = AUTH_RE.search(scrubbed)
if mA:
    sys.stdout.write("AUTH:" + phrase(mA))
    sys.exit(0)

if not turn_has_bd_memory:
    mB = ACK_ANYWHERE_RE.search(scrubbed) or ACK_SENTENCE_INITIAL_RE.search(scrubbed)
    if mB:
        sys.stdout.write("ACKUNBACKED:" + phrase(mB))
PY
