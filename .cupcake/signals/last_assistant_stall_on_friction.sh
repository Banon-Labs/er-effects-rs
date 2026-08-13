#!/usr/bin/env bash
# Cupcake signal: last_assistant_stall_on_friction
#
# Scans the most recently COMPLETED assistant turn of the current session transcript, plus the USER
# PROMPT that opened it, and emits ONE facts line describing what the turn did when it met friction.
# Consumed by the no_stall_on_friction policy (Stop), which owns the RULE; this script owns only the
# OBSERVATION. Keeping the conjunction in rego is deliberate: it makes the rule unit-testable against
# the verbatim corpus instead of hiding it in shell regexes.
#
#   STALLFACTS|friction=<phrase>|admission=<phrase>|handback=<phrase>|blame=<phrase>|acted=<0|1>|blocked=<0|1>|question=<0|1>|owned=<0|1>
#
# Emitted only when friction OR blame was detected; a clean turn emits empty (fail-open).
#
# WHAT EACH FACT MEANS
#   friction   -- the OPENING USER PROMPT of this turn carried frustration / conflict / correction
#                 ("shut up", "that's a shit thing", "I could have told you", sarcasm like "*you*").
#   admission  -- the assistant turn admitted / retracted / apologised, OR conceded-and-closed
#                 ("Retracting:", "were all invented", "I've read none of", "That's the whole delta").
#                 The concession-closure class is here because the observed stall #3 conceded and
#                 stopped WITHOUT contrition words, and that is the same defect.
#   handback   -- the turn ended by making the USER decide ("your call", "let me know how you'd like
#                 to proceed", "say the word", "want me to X?", "two ways forward").
#   blame      -- the turn attributed a consequence to a tool / guard / sentinel / subagent /
#                 environment ("the sentinel tore down the run", "cupcake blocked it").
#   acted      -- the turn made a substantive tool call (Edit/Write/Bash/Agent/Workflow/...). ANY such
#                 call counts, deliberately unlike idle_hold's stricter "status peeks are not work":
#                 here the defect is a turn that changed NOTHING, and reading the log the user pointed
#                 at is already the corrective direction. Over-blocking is the bigger risk.
#   blocked    -- the turn stated a real dependency on a user action and committed to acting on its
#                 result ("invade now and I'll read the log"). A genuine wait, not a stall.
#   question   -- the friction-carrying prompt asked a question ("?" or an interrogative opener). A
#                 turn that merely ANSWERS what was asked must never be caught by this guard.
#   owned      -- the turn named its OWN triggering action ("my edit", "because I", "I tripped it").
#                 Blame is only a defect when the agent's own hand in the outcome goes unmentioned.
#
# WHY A WHOLE-TURN SCAN: mirrors last_assistant_authority_agreement / last_assistant_idle_hold -- a
# slip in an EARLY message of a multi-message turn must not be masked by a later clean block. "Last
# completed turn" = the last non-empty run of assistant text bounded by real user prompts; tool-result
# carrier "user" events do NOT split a turn.
#
# Fenced code blocks, backtick spans and double-quoted spans are stripped from the assistant text
# before prose matching, so QUOTING these phrases (this file, the policy's own remedy text, a
# meta-discussion) does not false-trip; a real unquoted stall still matches. The USER's prompt is
# matched raw -- how they quote things is their business. Fail-open (empty output) on any error so a
# transcript hiccup cannot wedge the session.
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


def event_text(ev):
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content
    out = []
    for block in content or []:
        if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
            out.append(block["text"])
    return "\n".join(out)


# A substantive action = the turn CHANGED something or drove real work. Read-only inspection tools
# (Read, Grep, Glob, WebFetch) are intentionally excluded: reading is not rectifying.
ACTION_TOOLS = {
    "Edit", "Write", "NotebookEdit", "Bash", "Agent", "Task", "Workflow", "SendMessage",
}


def assistant_has_action(ev):
    for block in ev.get("message", {}).get("content", []) or []:
        if not isinstance(block, dict) or block.get("type") != "tool_use":
            continue
        if block.get("name") in ACTION_TOOLS:
            return True
    return False


# Bucket the transcript into turns delimited by real user prompts, keeping each turn's opening prompt
# alongside its assistant text and action flag.
turns = [{"prompt": "", "text": [], "acted": False}]
try:
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if is_real_user_prompt(ev):
                turns.append({"prompt": event_text(ev), "text": [], "acted": False})
            elif ev.get("type") == "assistant":
                t = event_text(ev)
                if t:
                    turns[-1]["text"].append(t)
                if assistant_has_action(ev):
                    turns[-1]["acted"] = True
except OSError:
    sys.exit(0)

prompt = ""
turn = ""
acted = False
for bucket in reversed(turns):
    if bucket["text"]:
        prompt = bucket["prompt"]
        turn = "\n".join(bucket["text"])
        acted = bucket["acted"]
        break

if not turn:
    sys.exit(0)

# Strip fenced code blocks, inline backtick spans and double-quoted spans from the ASSISTANT text so
# quoting/naming a banned shape is not using it. Single quotes are left alone: the phrases themselves
# contain apostrophes (I'm / that's / you're).
scrubbed = re.sub(r"```.*?```", " ", turn, flags=re.DOTALL)
scrubbed = re.sub(r"`[^`]*`", " ", scrubbed)
scrubbed = re.sub(r'"[^"]*"', " ", scrubbed)

# --- (a) FRICTION in the opening user prompt ------------------------------------------------------
FRICTION_RES = [
    re.compile(r"\bshut\s+up\b", re.IGNORECASE),
    re.compile(r"\bno\s+idea\s+what\s+you'?re\s+talking\s+about\b", re.IGNORECASE),
    re.compile(r"\byou\s+(?:have|had|got)\s+no\s+idea\b", re.IGNORECASE),
    re.compile(r"\b(?:i|we)\s+could(?:'ve|\s+have)\s+told\s+you\b", re.IGNORECASE),
    re.compile(r"\b(?:i|we)\s+(?:already\s+)?told\s+you\b", re.IGNORECASE),
    re.compile(r"\b(?:you'?re|you\s+are|that'?s|this\s+is)\s+wrong\b", re.IGNORECASE),
    re.compile(r"\byou\s+(?:didn'?t|did\s+not|failed\s+to|keep|always|never)\b", re.IGNORECASE),
    re.compile(
        r"\b(?:shit|shitty|fuck|fucking|fucked|bullshit|crap|garbage|useless|stupid|dumb"
        r"|idiotic|moronic|pathetic|sloppy|lazy|nonsense)\b",
        re.IGNORECASE,
    ),
    re.compile(r"\bstop\s+(?:doing|asking|guessing|making|apologi|telling\s+me)", re.IGNORECASE),
    re.compile(r"\b(?:i\s+)?(?:didn'?t|never)\s+ask(?:ed)?\s+(?:you\s+)?(?:for|to)\b", re.IGNORECASE),
    re.compile(r"\bthat'?s\s+not\s+what\s+i\s+(?:asked|said|meant|wanted)\b", re.IGNORECASE),
    re.compile(r"\byou\s+sound\b", re.IGNORECASE),
    re.compile(r"\b(?:most\s+)?harmful\b|\bunacceptable\b|\bdisappointing\b", re.IGNORECASE),
    re.compile(r"\bwhy\s+(?:the\s+hell|would\s+you|did\s+you|do\s+you\s+keep)\b", re.IGNORECASE),
    re.compile(r"\bread\s+the\s+(?:instructions?|directives?|rules?|agents)\b", re.IGNORECASE),
    # Sarcasm carried by emphasis markers around a pronoun/praise word: "I'm happy for *you*".
    re.compile(r"[*_]\s*(?:you|your|great|nice|wonderful|lovely|fantastic|brilliant)\s*[*_]",
               re.IGNORECASE),
]

# --- (b) ADMISSION / CONCESSION-CLOSURE in the assistant turn -------------------------------------
ADMISSION_RES = [
    re.compile(r"\b(?:retracting|i\s+retract|withdrawing\s+(?:that|the)\s+claim)\b", re.IGNORECASE),
    re.compile(r"\bi\s+(?:was|got\s+(?:that|this|it))\s+wrong\b", re.IGNORECASE),
    re.compile(r"\bmy\s+(?:mistake|error|bad)\b", re.IGNORECASE),
    re.compile(r"\b(?:i\s+apolog|apologies|sorry)\b", re.IGNORECASE),
    re.compile(
        r"\bi\s+(?:invented|fabricated|conflated|overstated|overclaimed|misread|guessed"
        r"|made\s+(?:that|this|it)\s+up)\b",
        re.IGNORECASE,
    ),
    re.compile(r"\b(?:was|were|are)\s+(?:all\s+)?(?:invented|fabricated|unfounded|made\s+up)\b",
               re.IGNORECASE),
    re.compile(r"\bi'?(?:ve)?\s*(?:have\s+)?read\s+(?:none|no)\b", re.IGNORECASE),
    re.compile(r"\bi\s+(?:haven'?t|have\s+not)\s+read\b", re.IGNORECASE),
    re.compile(r"\bi\s+(?:don'?t|do\s+not)\s+(?:actually\s+)?know\b", re.IGNORECASE),
    re.compile(r"\b(?:no|zero)\s+evidence\b|\bwithout\s+evidence\b", re.IGNORECASE),
    re.compile(r"\bi\s+should(?:n'?t|\s+not)\s+have\b", re.IGNORECASE),
    # Concession-closure: agreeing and stopping, with no contrition word anywhere. Observed stall #3.
    re.compile(r"\bthat'?s\s+the\s+whole\s+\w+", re.IGNORECASE),
    re.compile(r"\bthat'?s\s+(?:all|it)\b(?!\s+\w+ing)", re.IGNORECASE),
    re.compile(r"\bnothing\s+(?:else|more|further|new)\b", re.IGNORECASE),
    re.compile(r"\bno\s+(?:new|further)\s+(?:information|findings?|delta)\b", re.IGNORECASE),
    re.compile(r"\byou\s+(?:already\s+)?knew\s+(?:this|that)\b", re.IGNORECASE),
    re.compile(r"\bthe\s+(?:whole\s+)?delta\s+is\b", re.IGNORECASE),
]

# --- (c) DECISION HAND-BACK in the assistant turn --------------------------------------------------
# Kept to DECISION-solicitation forms only. A bare "let me know if ..." is excluded on purpose: asking
# the user to report an observation they alone can make is a legitimate handoff, not a hand-back.
HANDBACK_RES = [
    re.compile(r"\byour\s+call\b", re.IGNORECASE),
    re.compile(r"\bup\s+to\s+you\b", re.IGNORECASE),
    re.compile(r"\bsay\s+the\s+word\b", re.IGNORECASE),
    re.compile(r"\blet\s+me\s+know\s+(?:how|which|what)\b", re.IGNORECASE),
    re.compile(r"\bhow\s+you'?(?:d|ll)?\s*(?:would\s+)?like\s+(?:me\s+)?to\s+proceed\b", re.IGNORECASE),
    re.compile(r"\bhow\s+you\s+want\s+(?:me\s+)?to\s+proceed\b", re.IGNORECASE),
    re.compile(r"\b(?:want|do\s+you\s+want|would\s+you\s+like)\s+me\s+to\b", re.IGNORECASE),
    re.compile(r"\bshall\s+i\b", re.IGNORECASE),
    re.compile(r"\b(?:two|three|both)\s+(?:ways|options|paths|routes)\s+forward\b", re.IGNORECASE),
    re.compile(r"\bwhich(?:ever)?\s+(?:would\s+you|do\s+you)\s+(?:prefer|want)\b", re.IGNORECASE),
    re.compile(r"\btell\s+me\s+which\b", re.IGNORECASE),
    re.compile(r"\bpick\s+one\b", re.IGNORECASE),
]

# --- (d) BLAME DEFLECTION in the assistant turn ----------------------------------------------------
# A consequence attributed to a mechanism. Only a defect when the turn never names its OWN hand in it
# (see `owned` below) -- reporting a real blocker is required behaviour, disowning one is not.
MECHANISM = (
    r"(?:cupcake|opa|rego|guard(?:rail)?|polic(?:y|ies)|hook|sentinel|watcher|watchdog|harness"
    r"|environment|sandbox|tool(?:ing|chain)?|wrapper|linter|sub-?agent|daemon|timeout|rtk|shell)"
)
BLAME_RES = [
    re.compile(
        r"\b(?:the|a|that)\s+" + MECHANISM + r"\s+"
        r"(?:just\s+|then\s+|apparently\s+|silently\s+|helpfully\s+)?"
        r"(?:blocked|denied|refused|rejected|killed|tore\s+down|stopped|halted|interrupted"
        r"|aborted|broke|prevented|wiped|reverted|clobbered|nuked|ate|swallowed|mangled|corrupted)\b",
        re.IGNORECASE,
    ),
    re.compile(
        r"\b(?:was|were|got|been)\s+(?:\w+\s+){0,2}by\s+(?:the|a|that)\s+" + MECHANISM + r"\b",
        re.IGNORECASE,
    ),
]

# The turn names its own triggering action, so the attribution is a full account, not a deflection.
OWNED_RES = [
    re.compile(r"\bmy\s+(?:own\s+)?(?:edit|write|change|command|call|run|script|patch|test"
               r"|choice|decision|action|fault)\b", re.IGNORECASE),
    re.compile(r"\bbecause\s+i\b", re.IGNORECASE),
    re.compile(r"\bi\s+(?:triggered|caused|tripped|broke|edited|wrote|ran|launched|introduced|chose)\b",
               re.IGNORECASE),
    re.compile(r"\bthat\s+was\s+(?:me|mine|my\s+\w+)\b", re.IGNORECASE),
]

# --- exemptions -----------------------------------------------------------------------------------
# A stated dependency on a user action PLUS a commitment to act on its result: "invade now and I'll
# read the log". That is a real wait. It exempts the confess-and-stop arm only; the policy decides.
BLOCKED_RES = [
    re.compile(r"\b(?:and|then|once|after|when)\b[^.\n]{0,60}\bi'?ll\b", re.IGNORECASE),
    re.compile(r"\bping\s+me\b", re.IGNORECASE),
    re.compile(r"\bblocked\s+on\s+(?:you|the\s+user)\b", re.IGNORECASE),
    re.compile(r"\bonly\s+you\s+can\b", re.IGNORECASE),
]

# The prompt asked something. A turn that merely ANSWERS it is not a stall.
QUESTION_OPENER_RE = re.compile(
    r"^\s*(?:what|why|how|where|when|which|who|whose|can|could|should|would|will|is|are|was|were"
    r"|does|do|did|has|have|had|am|any)\b",
    re.IGNORECASE,
)


def first_match(text, regexes):
    for rx in regexes:
        m = rx.search(text)
        if m:
            return m.group(0)
    return ""


def sanitize(phrase):
    """Field values travel in a |-delimited, =-keyed line; keep them from breaking it."""
    return re.sub(r"[|=\r\n\t]+", " ", phrase).strip()[:60]


friction = first_match(prompt, FRICTION_RES)
blame = first_match(scrubbed, BLAME_RES)

if not friction and not blame:
    sys.exit(0)

admission = first_match(scrubbed, ADMISSION_RES)
handback = first_match(scrubbed, HANDBACK_RES)
blocked = bool(first_match(scrubbed, BLOCKED_RES))
owned = bool(first_match(scrubbed, OWNED_RES))
question = bool("?" in prompt or QUESTION_OPENER_RE.search(prompt or ""))

sys.stdout.write(
    "STALLFACTS"
    "|friction=" + sanitize(friction) +
    "|admission=" + sanitize(admission) +
    "|handback=" + sanitize(handback) +
    "|blame=" + sanitize(blame) +
    "|acted=" + ("1" if acted else "0") +
    "|blocked=" + ("1" if blocked else "0") +
    "|question=" + ("1" if question else "0") +
    "|owned=" + ("1" if owned else "0")
)
PY
