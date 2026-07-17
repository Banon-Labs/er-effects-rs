#!/usr/bin/env bash
# Cupcake signal: last_assistant_authority_agreement
#
# Outputs the banned authority-coded-agreement phrase found ANYWHERE in the most recently COMPLETED
# assistant turn of the current session transcript, or empty if none. Consumed by TWO policies:
#   * no_authority_agreement (Stop): halts turn-end so the agent must correct.
#   * no_authority_agreement_reminder (UserPromptSubmit): injects a mandatory correction directive.
#
# WHY A WHOLE-TURN SCAN, NOT JUST THE LAST MESSAGE (2026-07-17 fix): the old signal kept only the LAST
# assistant text block, so (a) a slip in an EARLIER message of a multi-message turn was overwritten by a
# later clean block and escaped, and (b) when the user INTERRUPTS a turn, the Stop event never fires at
# all -- the halt could not catch it. Scanning the whole last-completed turn fixes (a); routing the SAME
# signal into the UserPromptSubmit reminder (which ALWAYS runs on the next prompt, even after an
# interrupt) fixes (b). "Last completed turn" = the last non-empty run of assistant text bounded by real
# user prompts; on UserPromptSubmit the just-submitted prompt opens a new empty run, so the prior turn is
# still the last NON-EMPTY one -- the same value both events need.
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


# Bucket assistant text into turns delimited by real user prompts; keep the last NON-EMPTY bucket.
turns = [[]]
try:
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if is_real_user_prompt(ev):
                turns.append([])
            elif ev.get("type") == "assistant":
                t = assistant_text(ev)
                if t:
                    turns[-1].append(t)
except OSError:
    sys.exit(0)

last_turn = ""
for bucket in reversed(turns):
    if bucket:
        last_turn = "\n".join(bucket)
        break

# Strip DOUBLE-quoted spans so quoting the ban does not count as using it (single quotes are left alone
# because the phrases themselves contain apostrophes, e.g. you're).
scrubbed = re.sub(r'"[^"]*"', " ", last_turn)

# Targeted to real authority-coded AGREEMENT, not incidental words ("the correct offset").
pat = re.compile(
    r"\b(you'?re\s+right|you\s+are\s+right|that'?s\s+right|you'?re\s+correct|you\s+are\s+correct)\b"
    r"|(?:^|[.!?]\s+|\n)\s*(correct|exactly|absolutely|precisely)[,.! ]",
    re.IGNORECASE | re.MULTILINE,
)
m = pat.search(scrubbed)
if m:
    sys.stdout.write(m.group(0).strip())
PY
