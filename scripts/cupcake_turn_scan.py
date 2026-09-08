#!/usr/bin/env python3
"""One definition of "what did the last assistant turn actually do", shared by the Stop-hook signals.

Every `.cupcake/signals/last_assistant_*.sh` that has to answer "did that turn do real work, or did it
just talk?" used to answer it with its own private copy of the same code. Two copies are two rules:
they drift, and then two guards disagree about the same turn. This module is the single owner --
`.cupcake/signals/last_assistant_idle_hold.sh` and `.cupcake/signals/last_assistant_unexecuted_promise.sh`
both import it, so a fix to the peek-command list or the blocked-on-user phrasing lands in both at once.

What lives here (and nowhere else):
  * transcript discovery         -- `latest_transcript`
  * turn bucketing               -- `split_turns`; a turn is bounded by real user prompts, and a
                                    tool-result carrier "user" event does not split one
  * substantive work vs peeking  -- `bash_is_status_peek`, `Turn.work`
  * blocked-on-the-user prose    -- `USER_WAIT_RE`, `blocked_on_user`
  * live background work         -- `live_background_work`: a backgrounded Bash or an async Agent
                                    launch that has not reported back yet, and whether the current
                                    turn is the one that started it
  * where a paragraph starts     -- `prose_paragraphs`, and `Turn.text_runs`, the contiguous prose
                                    runs a length rule must measure instead of the whole turn

Signal-specific prose classification (which phrases are banned, how long is too long) stays in the
individual signal: that is the part each guard genuinely owns.

Import contract: every consumer must fail open (emit nothing) if this module cannot be imported, so a
missing/renamed file can never wedge a session.
"""
from __future__ import annotations

import glob
import json
import os
import re
from dataclasses import dataclass, field


# --- transcript discovery ---------------------------------------------------------------------

def latest_transcript(project_dir: str | None = None) -> str | None:
    """Newest session transcript for the project, or None. Mirrors Claude Code's own key scheme:
    ~/.claude/projects/<cwd-with-slashes-replaced-by-dashes>/*.jsonl."""
    cwd = project_dir or os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    tdir = os.path.join(os.path.expanduser("~/.claude/projects"), cwd.replace("/", "-"))
    files = sorted(
        glob.glob(os.path.join(tdir, "*.jsonl")),
        key=lambda p: os.path.getmtime(p),
        reverse=True,
    )
    return files[0] if files else None


def load_events(path: str) -> list[dict]:
    """Parse a transcript into events, skipping unparseable lines. Returns [] on any read error."""
    events: list[dict] = []
    try:
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line:
                    continue
                try:
                    ev = json.loads(line)
                except ValueError:
                    continue
                if isinstance(ev, dict):
                    events.append(ev)
    except OSError:
        return []
    return events


# --- turn bucketing ---------------------------------------------------------------------------

def is_real_user_prompt(ev: dict) -> bool:
    """A genuine user prompt starts a new turn. Tool-result 'user' events do not (they are the harness
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


def assistant_text(ev: dict) -> str:
    """All text blocks of one assistant event, joined."""
    out = []
    for block in ev.get("message", {}).get("content", []) or []:
        if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
            out.append(block["text"])
    return "\n".join(out)


# Command names that make a Bash call a mere "status peek" (looking at a log/output), not real work.
PEEK_CMDS = {"tail", "cat", "head", "wc", "grep", "ls", "echo", "less", "more"}


def bash_is_status_peek(cmd) -> bool:
    """True when every command in the (possibly piped/chained) Bash invocation is a peek command.
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


# Tool names that change something on disk / spawn real work, as opposed to reading state.
SUBSTANTIVE_TOOLS = ("Edit", "Write", "Agent", "Task")


def block_is_substantive(block: dict) -> bool:
    """A single tool_use block that constitutes real work: an Edit/Write/Agent, or a Bash call that is
    not a pure status/log peek."""
    if not isinstance(block, dict) or block.get("type") != "tool_use":
        return False
    name = block.get("name")
    if name in SUBSTANTIVE_TOOLS:
        return True
    if name == "Bash":
        cmd = (block.get("input") or {}).get("command", "")
        return isinstance(cmd, str) and bool(cmd.strip()) and not bash_is_status_peek(cmd)
    return False


def assistant_has_substantive_tool(ev: dict) -> bool:
    """A turn is doing real work if it has an Edit/Write/Agent tool_use, or a Bash call that is not a
    pure status/log peek."""
    for block in ev.get("message", {}).get("content", []) or []:
        if block_is_substantive(block):
            return True
    return False


@dataclass
class Turn:
    """One assistant turn: its text blocks and tool_use blocks, in order.

    `blocks` is the ordered stream, each entry ("text", str) or ("tool", block-dict). Order is what
    lets a caller ask the question that separates a kept promise from a broken one: was there a tool
    call after the sentence that promised one?
    """

    blocks: list[tuple] = field(default_factory=list)

    @property
    def texts(self) -> list[str]:
        return [b[1] for b in self.blocks if b[0] == "text"]

    @property
    def text(self) -> str:
        """The turn's prose. Identical to joining each event's `assistant_text` with a newline."""
        return "\n".join(self.texts)

    @property
    def work(self) -> bool:
        return any(b[0] == "tool" and block_is_substantive(b[1]) for b in self.blocks)

    @property
    def text_runs(self) -> list[str]:
        """The turn's prose split into contiguous runs -- consecutive text blocks with no tool call
        between them.

        This is the unit a length rule has to measure, and measuring the whole turn instead is what
        made the old wall-of-text guard fire on work it should never have touched. A tool-heavy turn
        emits a one-line preamble before each call ("Now the disassembly.", "Reading the policy.");
        eleven of those are eleven runs of one line, not an eleven-paragraph wall, and the user reads
        them one at a time interleaved with tool activity. Summing them scored that turn identical to
        an eleven-paragraph essay.
        """
        runs: list[str] = []
        current: list[str] = []
        for kind, value in self.blocks:
            if kind == "text":
                current.append(value)
            elif current:
                runs.append("\n\n".join(current))
                current = []
        if current:
            runs.append("\n\n".join(current))
        return runs

    @property
    def last_text_index(self) -> int:
        for i in range(len(self.blocks) - 1, -1, -1):
            if self.blocks[i][0] == "text":
                return i
        return -1

    def tool_after(self, index: int) -> bool:
        """Any tool_use block after `index` -- i.e. the turn kept going after that prose."""
        return any(b[0] == "tool" for b in self.blocks[index + 1:])

    def tools(self, name: str | None = None) -> list[dict]:
        out = [b[1] for b in self.blocks if b[0] == "tool"]
        if name is not None:
            out = [b for b in out if b.get("name") == name]
        return out


def split_turns(events: list[dict]) -> list[Turn]:
    """Bucket events into turns delimited by real user prompts."""
    turns = [Turn()]
    for ev in events:
        if is_real_user_prompt(ev):
            turns.append(Turn())
            continue
        if ev.get("type") != "assistant":
            continue
        for block in ev.get("message", {}).get("content", []) or []:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "text" and block.get("text"):
                turns[-1].blocks.append(("text", block["text"]))
            elif block.get("type") == "tool_use":
                turns[-1].blocks.append(("tool", block))
    return turns


def last_text_turn(turns: list[Turn]) -> Turn | None:
    """The last turn that actually said something -- the turn whose end the Stop hook is judging."""
    for turn in reversed(turns):
        if turn.texts:
            return turn
    return None


# --- prose segmentation -----------------------------------------------------------------------

# Why this lives here and not in the signal. The module docstring says prose classification stays in
# the individual signal, and it does: "how many paragraphs is too many" is still the wall_of_text
# signal's own call. What lives here is the mechanical part -- where a paragraph starts and stops --
# because three consumers need the identical answer (the signal, its regression test, and the
# false-positive audit) and the previous arrangement had all three carrying private copies. They
# drifted: scripts/test-wall-of-text-classifier.py asserted a classifier that no longer had to match
# the one the signal ran, so the test could pass while production counted differently.

_FENCED_RE = re.compile(r"```.*?```", re.DOTALL)
# A fence opened and never closed (a truncated/interrupted turn). Without this the whole remaining
# message counts as prose, and an interrupted code dump reads as a dozen paragraphs.
_OPEN_FENCE_RE = re.compile(r"```.*\Z", re.DOTALL)

_TABLE_ROW_RE = re.compile(r"^\|")
_LIST_ITEM_RE = re.compile(r"^([-*+]|\d+[.)])\s")
_HEADING_RE = re.compile(r"^#{1,6}\s")
_RULE_RE = re.compile(r"^([-*_])\1{2,}\s*$")
# A caption introduces the structure under it ("Findings:", "**Root cause:**") -- a label, not a
# paragraph to read. Only counted as a caption when structure actually follows it in the same block.
_CAPTION_RE = re.compile(r":\**\s*$")


def _is_structure_line(line: str) -> bool:
    """True for a line that is scannable structure rather than prose to read."""
    if line != line.lstrip() and len(line) - len(line.lstrip()) >= 2:
        return True  # indented: list continuation or an indented code block
    stripped = line.strip()
    if not stripped:
        return True
    return bool(
        _TABLE_ROW_RE.match(stripped)
        or _LIST_ITEM_RE.match(stripped)
        or _HEADING_RE.match(stripped)
        or _RULE_RE.match(stripped)
    )


def prose_paragraphs(text: str) -> list[str]:
    """The blank-line-delimited blocks of the text that are prose the user has to read.

    Structure is not prose and never counts: fenced code (closed or left open), tables, list items
    and their indented continuations, headings, horizontal rules, and a caption line that introduces
    structure in the same block. The objection this serves is to reading, and those are scanned.

    Classification is per line, not per block, which is the fix for the shape that used to inflate
    the count most: the old rule exempted a block only when every line was a table row (or every line
    a list item), so the ordinary "Findings:\n| a | b |" -- a caption with its table -- was scored as
    a prose paragraph, and three tables with three captions read as three paragraphs of prose.
    """
    text = _FENCED_RE.sub("\n", text)
    text = _OPEN_FENCE_RE.sub("\n", text)
    out: list[str] = []
    for block in re.split(r"\n\s*\n", text):
        if not block.strip():
            continue
        lines = [l for l in block.splitlines() if l.strip()]
        if not lines:
            continue
        prose = [l for l in lines if not _is_structure_line(l)]
        if not prose:
            continue
        if len(prose) == 1 and len(lines) > 1 and prose[0] == lines[0] and _CAPTION_RE.search(prose[0]):
            continue  # a caption introducing the structure beneath it
        out.append(block.strip())
    return out


# --- blocked on the user ----------------------------------------------------------------------

# Phrases that mean the turn is legitimately blocked on the user (awaiting their answer/drive). Kept
# specific so an incidental "you"/"your" in ordinary prose does not over-exempt.
USER_WAIT_RE = re.compile(
    r"\bwait(?:ing)?\s+for\s+(?:the\s+)?(?:user|you)\b"
    r"|\bblocked\s+on\s+(?:the\s+)?(?:user|you)\b"
    r"|\b(?:awaiting|await)\s+(?:the\s+)?(?:user'?s?|your)\b"
    r"|\bneed\s+(?:the\s+)?(?:user|you)\s+to\b"
    r"|\b(?:for|until)\s+you\s+to\b"
    r"|\bi'?ll\s+wait\s+for\s+you\b"
    r"|\bover\s+to\s+you\b"
    r"|\bhand(?:ing)?\s+(?:it\s+|this\s+)?(?:back\s+|off\s+)?to\s+you\b",
    re.IGNORECASE,
)


def blocked_on_user(text: str) -> bool:
    return bool(USER_WAIT_RE.search(text))


# --- live background work ---------------------------------------------------------------------

# A foreground Bash call can still leave a process running after it returns. These are the forms that
# deliberately detach one.
DETACHED_RE = re.compile(
    r"\bnohup\b|\bsetsid\b|\bdisown\b|\bsystemd-run\b"
    r"|\bscreen\s+-d\b|\btmux\s+new(?:-session)?\s+-d\b"
    r"|&\s*(?:$|\n)",
)

# How many turns back a background launch can be and still count as live. See live_background_work.
# Ten is deliberately generous: a genuinely long build or subagent stays covered across ten turns of
# conversation, while the failure this bound exists to stop -- a launch whose completion never reached
# the transcript -- was measured contaminating 46 consecutive turns before the guard was silenced on
# the very turn it exists to catch.
RECENT_TURNS = 10

# The harness's own words when a tool call was launched rather than awaited. A backgrounded Bash and
# an async subagent both answer immediately with one of these and report for real later, through a
# `<task-notification>`. Treating that immediate acknowledgement as the result is what makes a guard
# think a running job has finished -- measured on a real transcript, where "Command running in
# background with ID: ..." was read as completion and the still-running job stopped covering the turn.
BACKGROUND_LAUNCH_RE = re.compile(
    r"async agent launched|agentId:"
    r"|command running in background|running in background with id"
    r"|you will be notified when it completes",
    re.IGNORECASE,
)

# The harness injects these when a background task stops. `<status>` distinguishes a finished task
# from a progress ping.
TASK_NOTIFICATION_RE = re.compile(r"<task-notification>", re.IGNORECASE)
TASK_TOOL_USE_ID_RE = re.compile(r"<tool-use-id>\s*([^<\s]+)\s*</tool-use-id>", re.IGNORECASE)
TASK_STATUS_RE = re.compile(r"<status>\s*([^<\s]+)\s*</status>", re.IGNORECASE)
FINISHED_STATUSES = {"completed", "complete", "failed", "error", "killed", "cancelled", "canceled"}


def _result_text(block: dict) -> str:
    content = block.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        parts = []
        for sub in content:
            if isinstance(sub, dict) and sub.get("type") == "text":
                parts.append(sub.get("text") or "")
            elif isinstance(sub, str):
                parts.append(sub)
        return "\n".join(parts)
    return ""


def _user_content_string(ev: dict) -> str:
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content
    return ""


@dataclass
class BackgroundWork:
    """Background work still running at turn-end. Falsy when there is none.

    `watcher` marks the kinds whose entire purpose is work continuing past this turn -- a Monitor, a
    SendMessage to a running agent, a shell the turn deliberately detached. Those cover a promise on
    their own. A plain background job does not: see live_background_work.
    """

    description: str = ""
    watcher: bool = False

    def __bool__(self) -> bool:
        return bool(self.description)


def live_background_work(events: list[dict]) -> BackgroundWork:
    """What work is still running at turn-end.

    A caller must not read "something is running" as "the promise is covered". An Elden Ring session
    the user is inspecting stays up for an hour; a promise to go fix an unrelated file is not being
    carried by it, it is being deferred behind it -- the exact disappearance the promise guard exists
    to stop, and measured on the real transcript that prompted the guard, where a game launch two
    lines earlier would otherwise have excused it. Live work covers a promise only when the promise
    waits on that work ("once it lands", "whatever it finds", "if it flags") or when the live thing is
    a `watcher`.

    Otherwise deliberately generous -- every branch here suppresses a halt, so a false "something is running" is
    a quiet non-event while a false "nothing is running" would accuse an agent that did cover itself.
    Three sources, all read straight out of the transcript:

      * a Bash tool_use with `run_in_background: true` whose tool_result has not arrived (the harness
        delivers that result when the command exits, so a missing one means it is still going);
      * an Agent tool_use whose result said "Async agent launched" and for which no
        `<task-notification>` with a finished `<status>` has arrived;
      * a foreground Bash call in the last turn that detached a process itself (nohup/setsid/`&`), or
        a Monitor/SendMessage call, which wake or watch work that outlives the turn.

    But only recently launched work counts (RECENT_TURNS). A launch whose completion never made it
    into the transcript -- the notification was dropped, the session was resumed into a new file, the
    task was killed -- would otherwise stay "pending" forever and silently exempt every turn after it
    for the rest of the session. Measured on real transcripts: One subagent launched at line 658 and
    never notified suppressed the guard across the remaining 2,800 lines, including the exact turn the
    guard exists to catch. A guard that quietly stops firing is worse than one that never shipped, so
    a launch older than RECENT_TURNS turns is treated as finished. Genuine long-running work is
    re-covered every turn it is still waited on in prose, and ten turns of conversation is far longer
    than a background job normally survives without its result landing.
    """
    pending_bg: dict[str, tuple[int, str]] = {}   # tool_use_id -> (event index, description)
    agent_uses: dict[str, int] = {}
    pending_agents: dict[str, tuple[int, str]] = {}

    # Everything before this event index is too old to still be running (see RECENT_TURNS above).
    prompt_indices = [i for i, ev in enumerate(events) if is_real_user_prompt(ev)]
    cutoff = prompt_indices[-RECENT_TURNS] if len(prompt_indices) >= RECENT_TURNS else 0

    for index, ev in enumerate(events):
        content = ev.get("message", {}).get("content")

        # Harness-injected completion notice for a background task.
        raw = _user_content_string(ev)
        if raw and TASK_NOTIFICATION_RE.search(raw):
            m = TASK_TOOL_USE_ID_RE.search(raw)
            status = TASK_STATUS_RE.search(raw)
            if m and status and status.group(1).lower() in FINISHED_STATUSES:
                # One notification shape closes both kinds of launch: a backgrounded Bash and an
                # async subagent are both reported this way.
                pending_agents.pop(m.group(1), None)
                pending_bg.pop(m.group(1), None)
            continue

        if not isinstance(content, list):
            continue
        for block in content:
            if not isinstance(block, dict):
                continue
            if block.get("type") == "tool_use":
                name = block.get("name") or ""
                inp = block.get("input") or {}
                if isinstance(inp, dict) and inp.get("run_in_background"):
                    pending_bg[block.get("id") or ""] = (index, "backgrounded %s" % (name or "tool"))
                elif name in ("Agent", "Task"):
                    agent_uses[block.get("id") or ""] = index
            elif block.get("type") == "tool_result":
                tid = block.get("tool_use_id") or ""
                text = _result_text(block)
                launched = bool(BACKGROUND_LAUNCH_RE.search(text))
                if tid in pending_bg and not launched:
                    # A real result (output, exit status) -- the job is done. A "running in
                    # background" acknowledgement is not a result and leaves it pending.
                    pending_bg.pop(tid, None)
                if tid in agent_uses:
                    if launched:
                        pending_agents[tid] = (agent_uses[tid], "async subagent")
                    agent_uses.pop(tid, None)

    candidates = [
        (at, desc)
        for at, desc in list(pending_bg.values()) + list(pending_agents.values())
        if at >= cutoff
    ]
    if candidates:
        # Report the most recent live job; with several in flight the newest is the representative one.
        _launched_at, description = max(candidates)
        return BackgroundWork(description)

    # Last-turn-only signals: a detached process, or a tool whose whole point is work that outlives
    # the turn. Scoped to the last turn so a long-dead `nohup` from an hour ago cannot exempt forever.
    turn = last_text_turn(split_turns(events))
    if turn is not None:
        for block in turn.tools():
            name = block.get("name")
            if name in ("Monitor", "SendMessage"):
                return BackgroundWork("a %s call (work continues outside this turn)" % name, True)
            if name == "Bash":
                cmd = (block.get("input") or {}).get("command", "")
                if isinstance(cmd, str) and DETACHED_RE.search(cmd):
                    return BackgroundWork("a detached shell command", True)
    return BackgroundWork()
