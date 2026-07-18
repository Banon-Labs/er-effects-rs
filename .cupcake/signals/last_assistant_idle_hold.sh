#!/usr/bin/env bash
# Cupcake signal: last_assistant_idle_hold
#
# Scans the most recently COMPLETED assistant turn of the current session transcript and returns a
# TAGGED idle-hold marker, or empty if the turn is clean. Consumed by TWO policies:
#   * idle_hold (Stop): halts turn-end so the agent does non-overlapping work or justifies the wait.
#   * idle_hold_reminder (UserPromptSubmit): standing reminder + interlock backstop (catches an
#     interrupted turn the Stop halt could not see).
#
# BANNED CLASS (persistent user directive 2026-07-17, recurring anti-pattern):
#   The agent announces it is IDLING / HOLDING / STANDING BY while a background task runs, WITHOUT
#   justification. Phrases like "I'm holding", "holding for", "holding off", "standing by",
#   "I'll wait for", "waiting for X before", "waiting on X rather than", "nothing to do but wait",
#   "I'll pause here", "let it run and wait". Emitted as  IDLEHOLD:<phrase>.
#
# TWO EXEMPTIONS suppress the flag (the turn is NOT idle -- it is either productive or justified):
#   (a) JUSTIFICATION PROSE -- the same turn contains "I would normally have <...> but <...>" /
#       "normally I'd <...> however <...>": the agent acknowledges non-overlapping work exists and
#       states a reason it could not be done. The user can validate that, so it is allowed.
#   (b) SUBSTANTIVE WORK -- the same turn contains a substantive tool_use: an Edit/Write/Agent
#       tool_use, or a Bash command that is NOT a pure status/log peek. A turn whose only Bash calls
#       are tail/cat/head/wc/grep/echo/ls of a log/output file is a "status peek" and does NOT count
#       as substantive (so "holding" + only peeking still flags).
#
# One further carve-out inside the phrase match: a wait that is legitimately BLOCKED ON THE USER
# ("waiting for user confirmation", "holding for the user to drive", "I'll wait for you") is NOT
# idling -- the agent genuinely cannot proceed -- so those are excluded from the phrase hit.
#
# WHY A WHOLE-TURN SCAN + BOTH EVENTS: mirrors last_assistant_authority_agreement -- an early-message
# slip must not be masked by a later clean block (whole-turn scan), and an INTERRUPTED turn fires no
# Stop event, so the same signal is routed into the UserPromptSubmit interlock which always runs.
# "Last completed turn" = the last non-empty run of assistant text bounded by real user prompts;
# tool-result carrier "user" events do NOT split a turn.
#
# Double-quoted spans are stripped before prose matching so quoting the ban (this file, the reminder
# text, or a meta-discussion like the phrase "I'm holding") does not false-trip; a real unquoted
# announcement still matches. Fail-open (empty output) on any error so a transcript hiccup cannot
# wedge the session.
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
        return True
    return False


def assistant_text(ev):
    out = []
    for block in ev.get("message", {}).get("content", []) or []:
        if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
            out.append(block["text"])
    return "\n".join(out)


# Command names that make a Bash call a mere "status peek" (looking at a log/output), NOT real work.
PEEK_CMDS = {"tail", "cat", "head", "wc", "grep", "ls", "echo", "less", "more"}


def bash_is_status_peek(cmd):
    """True when EVERY command in the (possibly piped/chained) Bash invocation is a peek command.
    Any non-peek command (cargo, python, bd remember, a build, ...) makes the call substantive."""
    if not isinstance(cmd, str) or not cmd.strip():
        return False  # empty command -> not a peek (but also handled as non-substantive by caller)
    # Strip quoted spans so a quoted pipe/pattern (grep "a|b") does not desync the segment split.
    stripped = re.sub(r'"[^"]*"', " ", cmd)
    stripped = re.sub(r"'[^']*'", " ", stripped)
    segments = re.split(r"\|\||&&|[|;\n]", stripped)
    names = []
    for seg in segments:
        toks = seg.strip().split()
        i = 0
        while i < len(toks) and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", toks[i]):
            i += 1  # skip leading FOO=bar env assignments
        if i < len(toks):
            names.append(toks[i].split("/")[-1])  # basename of the command
    if not names:
        return False
    return all(n in PEEK_CMDS for n in names)


def assistant_has_substantive_tool(ev):
    """A turn is doing real work if it has an Edit/Write/Agent tool_use, or a Bash call that is not a
    pure status/log peek."""
    for block in ev.get("message", {}).get("content", []) or []:
        if not isinstance(block, dict) or block.get("type") != "tool_use":
            continue
        name = block.get("name")
        if name in ("Edit", "Write", "Agent"):
            return True
        if name == "Bash":
            cmd = (block.get("input") or {}).get("command", "")
            if isinstance(cmd, str) and cmd.strip() and not bash_is_status_peek(cmd):
                return True
    return False


# Bucket assistant text AND substantive-tool flags into turns delimited by real user prompts; keep
# the last bucket that has any text (its own substantive flag decides exemption (b)).
turns = [{"text": [], "work": False}]
try:
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if is_real_user_prompt(ev):
                turns.append({"text": [], "work": False})
            elif ev.get("type") == "assistant":
                t = assistant_text(ev)
                if t:
                    turns[-1]["text"].append(t)
                if assistant_has_substantive_tool(ev):
                    turns[-1]["work"] = True
except OSError:
    sys.exit(0)

last_turn = ""
turn_has_work = False
for bucket in reversed(turns):
    if bucket["text"]:
        last_turn = "\n".join(bucket["text"])
        turn_has_work = bucket["work"]
        break

# Strip DOUBLE-quoted spans so quoting the ban does not count as using it (single quotes are left
# alone because the phrases themselves contain apostrophes, e.g. I'm / I'll).
scrubbed = re.sub(r'"[^"]*"', " ", last_turn)

# Idle-announcement phrases. Word-boundaried / anchored to avoid incidental prose.
IDLE_RE = re.compile(
    r"\b(i'?m\s+holding|i\s+am\s+holding|holding\s+for|holding\s+off"
    r"|standing\s+by"
    r"|i'?ll\s+wait\s+for|i\s+will\s+wait\s+for"
    r"|waiting\s+(?:for|on)\b[^.\n]*?\b(?:before|rather\s+than)\b"
    r"|nothing\s+to\s+do\s+(?:but|while)\s+wait"
    r"|i'?ll\s+pause\s+here"
    r"|let\s+it\s+(?:run|complete)\s+and\s+wait)\b",
    re.IGNORECASE,
)

# A wait BLOCKED ON THE USER is legitimate, not idling. If the phrase's immediate context names the
# user as the blocker, do not count it as an idle hit.
USER_BLOCK_RE = re.compile(r"\b(?:the\s+)?(?:user|users|you|your)\b", re.IGNORECASE)

# Exemption (a): explicit justification prose acknowledging non-overlapping work + a reason.
JUSTIFY_RE = re.compile(
    r"i\s+would\s+normally\b[^.\n]*\b(?:but|however)\b"
    r"|normally\s+i'?d\b[^.\n]*\b(?:but|however)\b"
    r"|normally\s+i\s+would\b[^.\n]*\b(?:but|however)\b",
    re.IGNORECASE,
)


def find_idle_phrase(text):
    for m in IDLE_RE.finditer(text):
        window = text[m.start():m.end() + 48]
        if USER_BLOCK_RE.search(window):
            continue  # blocked on the user -> legitimate, not idling
        return m.group(1).strip()
    return None


phrase = find_idle_phrase(scrubbed)
if phrase and not turn_has_work and not JUSTIFY_RE.search(scrubbed):
    sys.stdout.write("IDLEHOLD:" + phrase)
PY
