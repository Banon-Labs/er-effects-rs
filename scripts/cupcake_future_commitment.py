"""Detect a closing message that ends on future work the turn could have done, or on a delegate's
deliverables that do not exist yet.

Two shapes, one module, because both convict a turn on the same fact: the sentence describes work
as banked when nothing in the transcript did it.

Shape one, the future-tense commitment. Verbatim, 2026-09-09:

    Next run I'll build this in and re-attach Frida to confirm zero `DISCARDING` lines across a
    full invade-reject-reinvade cycle.

The build command and the launch script were both available in that turn. The promise cost the user
a round trip carrying no information, and the reply to it was "'Next run I'll' SOUND FAMILIAR?" --
a repeat offence.

Shape two, delegation as completion. Verbatim, from the turn that dispatched the agent which wrote
this file:

    It has the verbatim sentence, the exemptions that must not trip (a real blocker, the user owning
    the observation, work already started in-turn), and the same evidence bar the others got:
    measured false-positive rate over real transcripts, all four repo gates, and a `cupcake eval`
    returning `decision:block` on that exact line rather than a passing unit test.

The `Agent` tool's own result says the caller knows nothing about a delegate's results until the
completion notification arrives. Listing what a just-dispatched agent contains converts a dispatch
into a claim of delivery -- the same defect with the subject swapped from the agent to its delegate.

Kept as an importable module rather than an inline heredoc so the classifier is testable without a
live transcript, the way `cupcake_unbacked_claim.py` is. The transcript walk, the turn bucketing and
the background-work helper stay in `cupcake_turn_scan.py`, shared with every neighbouring signal.
"""

from __future__ import annotations

import re

# --- shared text handling ----------------------------------------------------------------------


def strip_quoted(text: str) -> str:
    """Remove fenced, backticked and double-quoted spans, so quoting a banned sentence -- this
    module, the policy, a report about the guard -- cannot trip it. Single quotes are left alone
    because the openers themselves carry apostrophes."""
    text = re.sub(r"```.*?```", " ", text or "", flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]{0,400}"', " ", text)


def sentences(text: str) -> list[str]:
    """Sentences, treating a semicolon as a boundary.

    The semicolon matters for the same reason it does in the neighbouring signal: a closer often
    hangs a second clause off the promise ("...; the one thing I need from you is X"), and without
    the split the promise cannot be located at the end of the message.
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


# --- shape one: the future-tense commitment ------------------------------------------------------

# The first-person future openers. "let me" is deliberately absent: it belongs to
# `last_assistant_unexecuted_promise`, which owns the present-tense request-to-act, and duplicating
# it here would double-halt one sentence. "i plan to" and "i intend to" are present because the
# directive names them; the sibling reads them as hedges, which is what let this shape through.
OPENER_RE = re.compile(
    r"\bi(?:'|’)?ll\b"
    r"|\bi\s+will\b"
    r"|\bi(?:'|’)?m\s+going\s+to\b|\bi\s+am\s+going\s+to\b"
    r"|\bi(?:'|’)?m\s+about\s+to\b|\bi\s+am\s+about\s+to\b"
    r"|\bi\s+plan\s+to\b|\bi\s+intend\s+to\b",
    re.IGNORECASE,
)

# A time marker in front of the opener is what makes the deferral explicit, and it is also what
# disarmed the sibling: `last_assistant_unexecuted_promise` treats "next session/turn/time" as the
# ball being handed to the user, so a promise wearing a deadline was read as a handoff. Recorded as
# a fact rather than a requirement -- a bare "I'll rebuild it" closer is the same defect without the
# deadline, and the directive names both.
TIME_MARKER_RE = re.compile(
    r"\bnext\s+(?:run|time|session|turn|round|pass|launch|build|attempt|boot|probe|cycle)\b"
    r"|\bafterwards?\b"
    r"|\bafter\s+(?:that|this|the\s+\w+)\b"
    r"|\bonce\s+\w+"
    r"|\blater\b|\btomorrow\b|\bthen\b|\bin\s+the\s+next\b",
    re.IGNORECASE,
)

# Adverbs and particles that sit between the opener and the verb without changing what is promised.
# A word here keeps the reader in a verb position: "I'll now go and rebuild it" is still a promise to
# rebuild.
FILLER = {
    "now", "also", "just", "first", "go", "immediately", "still", "quickly", "actually", "already",
    "instead", "finally", "again", "simply", "promptly", "right", "away", "ahead", "straight",
    "briefly", "properly", "fully", "quietly", "be", "have", "get", "the", "a", "one", "more",
}

# Words that reopen a verb position after one has been spent. The scan needs them because the
# directive names the trailing purpose clause as catchable: in "I'll build this in and re-attach
# Frida to confirm ...", the verb worth quoting can sit a dozen words past the opener.
COORDINATORS = {"and", "then", "or", "to", "before", "after", "plus", "next", "so", "while"}

# A hedge is not a commitment and a negation is its opposite. Either kills the match. "plan" and
# "intend" are absent on purpose: they are openers here, not hedges.
HEDGE = {
    "probably", "likely", "maybe", "perhaps", "possibly", "hopefully", "try", "attempt", "consider",
    "think", "might", "may", "want", "hope", "aim", "prefer", "should", "could", "would", "expect",
    "guess", "suppose", "see", "know", "treat", "keep", "leave", "let", "remember", "note",
}
NEGATION = {"not", "never", "no", "nothing", "avoid", "stop", "refrain", "skip", "hold"}

# Concrete actions, grouped by the kind of tool call that would have performed one. The group is
# what makes the exemption exact: an `Edit` does not keep a promise to rebuild, and a build does not
# keep a promise to relaunch. Everything outside these groups is left alone -- stance verbs, verbs a
# message fulfils by itself ("explain", "summarise"), and vague ones ("do", "handle", "continue").
ACTION_GROUPS: dict[str, tuple[str, ...]] = {
    "edit": (
        "add", "wire", "patch", "fix", "implement", "write", "rewrite", "edit", "create", "update",
        "remove", "delete", "land", "refactor", "rename", "revert", "restore", "apply", "record",
        "document", "extend", "split", "generate", "regenerate", "stage", "package", "plumb",
        "thread", "swap", "bump", "pin", "harden", "gate", "tighten", "port", "migrate", "fold",
        "encode", "codify", "hardcode", "correct", "repair", "drop",
    ),
    "build": ("build", "rebuild", "compile", "recompile", "cross-compile", "link", "relink"),
    "launch": (
        "launch", "relaunch", "run", "rerun", "restart", "boot", "reboot", "reproduce", "replay",
        "smoke", "deploy", "install", "reinstall", "kick",
    ),
    "attach": (
        "attach", "reattach", "hook", "inject", "instrument", "trace", "breakpoint", "detour",
    ),
    "measure": (
        "measure", "confirm", "verify", "validate", "check", "recheck", "test", "retest", "prove",
        "capture", "screenshot", "benchmark", "profile", "watch", "observe", "probe", "sample",
        "time",
    ),
    "read": (
        "read", "reread", "inspect", "grep", "search", "look", "examine", "diff", "compare",
        "decompile", "disassemble", "dump", "scan", "audit", "review", "count", "investigate",
        "dig", "trace-through", "parse", "map", "walk",
    ),
    "vcs": (
        "commit", "push", "rebase", "tag", "publish", "sync", "merge", "cherry-pick", "file",
        "open",
    ),
}

ACTION_CLASS = {verb: group for group, verbs in ACTION_GROUPS.items() for verb in verbs}

WORD_RE = re.compile(r"[A-Za-z][A-Za-z’'\-]*")

# How far past the opener a concrete verb may sit. Wider than the sibling's six words because the
# directive names the trailing purpose clause ("... and re-attach Frida to confirm ...") as a
# catchable shape, and that verb can be a dozen words in.
VERB_WINDOW_WORDS = 14


def committed_action(tail: str) -> str | None:
    """The concrete-action verb this opener commits to, or None.

    An action word only counts in a verb position -- straight after the opener, or after a
    coordinator that opens a new clause. Scanning every word instead was measured wrong on a real
    turn: "I'll colourise at display time" has no allowlisted verb, but `time` sits in the measuring
    group, and reading it as a promise to time something turned a report into a violation. The same
    trap waits in "I'll get the events as they land" (`land`) and any sentence whose object nouns
    happen to be verbs elsewhere.

    A hedge or a negation in a verb position ends the scan: those sentences commit to nothing.
    """
    expect_verb = True
    for raw in WORD_RE.findall(tail)[:VERB_WINDOW_WORDS]:
        word = raw.lower().replace("’", "'").strip("-'")
        if word in FILLER:
            continue
        if expect_verb:
            if word in NEGATION or word in HEDGE:
                return None
            if word in ACTION_CLASS:
                return word
            # A hyphenated `re-` form is the same verb: "re-attach" is "attach". Stripping a bare
            # "re" would turn "report" into "port" and invent a promise nobody made, so only the
            # hyphenated spelling is derived; merged forms are enumerated in the groups above.
            if word.startswith("re-") and word[3:] in ACTION_CLASS:
                return word[3:]
            expect_verb = False
            continue
        if word in COORDINATORS:
            expect_verb = True
    return None


def promised_action(
    closing_text: str, tail_sentences: int = 2
) -> tuple[str, str, bool, bool] | None:
    """The closing promise as (clause, action class, carries a time marker, is conditional), or None.

    Only the last `tail_sentences` of the closing prose are read. That is the whole difference
    between this guard and a general promise detector: the objection is to a turn that *ends* on
    deferred work, and a mid-turn "I'll check the offsets" followed by the check is the correct
    shape that must never be touched.
    """
    scrubbed = strip_quoted(closing_text)
    for sentence in reversed(sentences(scrubbed)[-tail_sentences:]):
        for match in OPENER_RE.finditer(sentence):
            verb = committed_action(sentence[match.end():])
            if verb:
                return (
                    quote(sentence),
                    ACTION_CLASS[verb],
                    bool(TIME_MARKER_RE.search(sentence)),
                    bool(CONDITIONAL_RE.search(sentence)),
                )
    return None


# --- did the turn perform that class of action? --------------------------------------------------

WRITE_TOOLS = {"edit", "write", "multiedit", "notebookedit"}

# Bash constructs that put bytes on disk. The session prompt tells the model to prefer Bash for file
# changes in bypass-permissions mode, so a guard blind to heredocs and redirects would fire on the
# sanctioned workflow.
BASH_WRITE_RE = re.compile(
    r"<<\s*'?[A-Za-z_]+\b"
    r"|\bsed\s+(?:-[^\s]*\s+)*-i\b"
    r"|\btee\b|\bpatch\b"
    r"|>>?\s*(?!/dev/)[^\s|&;<>]*/[^\s|&;<>]+"
)

CLASS_COMMAND_RE = {
    "build": re.compile(r"\bcargo\b|xwin|er-build-dlls|check-rust-build|build-[\w-]*\.sh|\bmake\b"),
    "launch": re.compile(
        r"er-run-branch|launch\.sh|\bme3\b|run-[\w-]*\.sh|eldenring|\bprobe\b|smoke|\bwine\b"
        r"|readiness-watch"
    ),
    "attach": re.compile(r"frida|\battach\b|winedbg|\bgdb\b|inject|\btrace\b"),
    "vcs": re.compile(r"\bgit\b|\bgh\b|\bbd\s+dolt\b"),
}


def _bash_command(block: dict) -> str:
    raw = block.get("input") if isinstance(block, dict) else None
    if isinstance(raw, dict):
        value = raw.get("command")
        if isinstance(value, str):
            return value
    return ""


def command_head(cmd: str) -> str:
    """The part of a Bash command that names programs, with payload text removed.

    A heredoc body and a quoted argument are data, not an invocation, and matching a class pattern
    against them is how a memory note that happens to contain the word "build" was read as a build.
    Measured on the turn this guard exists to refuse: `bd remember --key ... "...rebuild..."` scored
    as having rebuilt, and the halt did not fire.
    """
    head = re.split(r"<<-?\s*'?[A-Za-z_]", cmd or "", maxsplit=1)[0]
    head = re.sub(r'"[^"]*"', " ", head)
    return re.sub(r"'[^']*'", " ", head)


def _tool_name(block: dict) -> str:
    if not isinstance(block, dict):
        return ""
    return str(block.get("name") or block.get("tool_name") or "").strip().lower().replace("_", "")


def action_taken(action_class: str, tool_blocks: list[dict]) -> bool:
    """True when the turn already did work of the promised class.

    Deliberately generous in every branch: a false "it acted" is a quiet non-event, while a false
    "it did nothing" accuses a turn that kept its own promise.
    """
    names = [_tool_name(b) for b in tool_blocks]
    raw_commands = [_bash_command(b) for b in tool_blocks if _tool_name(b) == "bash"]
    commands = [command_head(cmd) for cmd in raw_commands]
    backgrounded = any(
        (b.get("input") or {}).get("run_in_background")
        for b in tool_blocks
        if isinstance(b, dict) and isinstance(b.get("input"), dict)
    )
    spawned = any(name in ("agent", "task", "monitor", "sendmessage") for name in names)

    if action_class == "edit":
        # The write test reads the raw command: its evidence is the heredoc marker and the redirect,
        # which `command_head` cuts away precisely because they introduce payload.
        return any(name in WRITE_TOOLS for name in names) or any(
            BASH_WRITE_RE.search(cmd) for cmd in raw_commands
        )
    if action_class == "read":
        # Any tool call at all reads something. A promise to go and look is kept by looking.
        return bool(tool_blocks)
    if action_class == "measure":
        # Measuring in this repo means producing an observation: a launch, an attach, a background
        # job, or a delegate. An edit proves nothing and must not exempt one.
        return (
            backgrounded
            or spawned
            or any(
                CLASS_COMMAND_RE[cls].search(cmd)
                for cmd in commands
                for cls in ("launch", "attach", "build")
            )
        )
    pattern = CLASS_COMMAND_RE.get(action_class)
    if pattern is None:
        return bool(tool_blocks)
    return any(pattern.search(cmd) for cmd in commands) or (
        action_class == "launch" and backgrounded
    )


# --- the exemptions ------------------------------------------------------------------------------

# A dependency the agent genuinely cannot dissolve: something only the user can observe or supply,
# a credential, a purchase, a decision that is theirs, or an explicit instruction to stop.
EXTERNAL_BLOCKER_RE = re.compile(
    r"\bblocked\b|\bblocker\b|\bcannot\s+proceed\b"
    r"|\bcan(?:no|')?t\s+(?:proceed|run|test|verify|build|launch|start|do\s+that)\b"
    r"|\brequires?\s+(?:sudo|root|approval|credentials|a\s+login|a\s+purchase|your|you\s+to)\b"
    r"|\bneeds?\s+(?:sudo|root|approval|credentials|a\s+login|a\s+purchase|your|you\s+to)\b"
    r"|\btell\s+me\s+(?:what|whether|if|when|how)\b"
    r"|\bwhat\s+(?:did|do)\s+you\s+(?:see|observe|notice|get)\b"
    r"|\bdid\s+you\s+(?:see|notice|observe|hear)\b"
    r"|\byour\s+(?:call|preference|choice|judgement|judgment)\b"
    r"|\bonly\s+you\s+can\b|\bup\s+to\s+you\b"
    r"|\byou\s+(?:said|asked|told\s+me)\s+to\s+(?:stop|wait|hold|pause)\b"
    r"|\blog\s+in(?:to)?\b|\bsign\s+in\b|\bpurchase\b"
    r"|\bwaiting\s+on\s+(?:approval|ci|the\s+merge|the\s+gate)\b",
    re.IGNORECASE,
)

# The promise is conditioned on something that has not happened: a decision the user has not made
# ("say the word and I'll build it", "say which shape and I'll implement it"), or an event that may
# never occur ("if either trips over the other, I'll relaunch them that way"). Neither is work the
# turn was withholding -- the first belongs to `ER-EFFECTS-NO-ZERO-INFORMATION-STOP`, which convicts an
# offer to do authorised work as a handback, and the second is a contingency that is not yet due.
# Measured: five of the eleven first-draft hits over 2,622 real turns were one of these two shapes,
# and quoting them back as broken promises would have been the wrong charge for the right sentence.
CONDITIONAL_RE = re.compile(
    r"\bsay\s+the\s+word\b|\bsay\s+which\b|\btell\s+me\s+which\b"
    r"|\btell\s+me\s+and\s+i(?:'|’)?ll\b"
    r"|\bif\s+you(?:'d|\s+would)?\s+(?:want|like|prefer|rather)\b"
    r"|\bwant\s+me\s+to\b|\bwould\s+you\s+like\s+me\s+to\b"
    r"|\blet\s+me\s+know\s+(?:if|whether|and)\b"
    r"|\bon\s+your\s+say-?so\b"
    r"|\bunless\s+\w+\b|\bif\s+(?:you|it|they|that|either|any|the\s+\w+)\b"
    r"|\bshould\s+(?:i|it|they)\s+\w+"
    r"|\bthe\s+ball\s+is\s+yours\b"
    r"|\bin\s+case\s+\w+",
    re.IGNORECASE,
)

# The promise waits on work that does not exist yet. Paired with `started_something` below: naming a
# dependency is not enough, the turn has to have started the thing it waits on.
WAITS_ON_RESULT_RE = re.compile(
    r"\b(?:when|once|after|as\s+soon\s+as|the\s+moment|while|until)\b[^.\n]{0,70}?"
    r"\b(?:it|that|they|its|their"
    # The thing being waited on, named directly or with a qualifier in front of it: "the worktree
    # subagent", "the smoke run". Deliberately not a bare "the <anything>" -- this branch only ever
    # exempts, and an open noun would let any subordinate clause buy silence.
    r"|the(?:\s+\w+)?\s+(?:run|build|probe|launch|agent|subagent|job|task|worker|gate|check)"
    r"|finishes|lands|exits|completes|reports|comes\s+back)\b"
    r"|\bas\s+(?:they|it|these|those)\s+(?:land|arrive|come\s+in|appear|report)\b"
    r"|\bwhatever\s+(?:it|they)\s+\w+"
    r"|\b(?:its|their)\s+(?:output|log|logs|result|results|findings?|verdict|report)\b",
    re.IGNORECASE,
)

DETACHED_RE = re.compile(
    r"\bnohup\b|\bsetsid\b|\bdisown\b|\bsystemd-run\b"
    r"|\bscreen\s+-d\b|\btmux\s+new(?:-session)?\s+-d\b"
    r"|&\s*(?:$|\n)"
)


def started_something(tool_blocks: list[dict]) -> bool:
    """True when this turn started work that outlives it: a backgrounded command, a subagent, a
    watcher, or a shell it detached itself."""
    for block in tool_blocks:
        name = _tool_name(block)
        if name in ("agent", "task", "monitor", "sendmessage"):
            return True
        if isinstance(block, dict) and isinstance(block.get("input"), dict):
            if block["input"].get("run_in_background"):
                return True
        if name == "bash" and DETACHED_RE.search(command_head(_bash_command(block))):
            return True
    return False


def started_watcher(tool_blocks: list[dict]) -> bool:
    """True when this turn started something whose entire purpose is watching work that outlives the
    turn: a `Monitor`, a `SendMessage` to a running agent, or a shell it detached.

    `cupcake_turn_scan.live_background_work` draws the same distinction, and for the same reason: a
    watcher covers a promise on its own, because it is the mechanism that brings the agent back. A
    real turn tripped this guard without it -- a `Monitor` was tailing the log and the closer said
    "I'll get the events as they land", which is a description of the watcher, not a deferral.
    """
    return any(
        _tool_name(block) in ("monitor", "sendmessage")
        or (_tool_name(block) == "bash" and DETACHED_RE.search(command_head(_bash_command(block))))
        for block in tool_blocks
    )


def deferred_to_started_work(
    promise_sentence: str, tool_blocks: list[dict], live_background: bool = False
) -> bool:
    """The exemption for work that genuinely cannot happen yet.

    Three ways in, and the first two are the ones the neighbouring guards get wrong in opposite
    directions:
      * this turn started a watcher -- a Monitor, a SendMessage, a shell it detached -- whose whole
        purpose is work continuing past the turn. That brings the agent back on its own.
      * the promise waits on a result ("once the worktree subagent makes those gates green"), and
        something is actually running: started in this turn, or still live from an earlier one.
      * neither, in which case nothing is carrying the work and the promise is the defect.

    The waits-on test reads the promise sentence, not the whole message, and that is what keeps the
    live-work half from swallowing the shape this guard exists to refuse. The instance's message
    contains "while lobby/protocol advanced" seventy words above its promise, and a whole-message
    read would have paired that with the running game and exempted it -- which is precisely how
    `last_assistant_unexecuted_promise` was disarmed on the real turn.
    """
    if started_watcher(tool_blocks):
        return True
    if not WAITS_ON_RESULT_RE.search(strip_quoted(promise_sentence)):
        return False
    return started_something(tool_blocks) or live_background


# The user asked for the plan. Then stating one is the answer, and the guard must stay out of it.
# Narrow on purpose: a broad "the prompt contained a question mark" exemption -- which the
# diagnosis guard can afford, because explaining is its deliverable -- would gut this rule, since
# most prompts here carry a question.
PLAN_REQUEST_RE = re.compile(
    r"\bwhat(?:'s|’s| is|\s+are)?\s+(?:your\s+|the\s+)?(?:plan|next\s+steps?|approach|strategy)\b"
    r"|\bwhat\s+(?:will|would|do)\s+you\s+(?:do|going\s+to\s+do|plan|intend|propose)\b"
    r"|\bhow\s+(?:will|would)\s+you\s+(?:approach|do|handle|tackle)\b"
    r"|\bwhat(?:'s|’s| is)\s+next\b|\bwhat\s+comes\s+next\b"
    r"|\boutline\s+(?:the|your)\s+plan\b|\bwalk\s+me\s+through\s+(?:the|your)\s+plan\b"
    r"|\bplan\s*\?",
    re.IGNORECASE,
)


def plan_requested(prompt: str) -> bool:
    return bool(PLAN_REQUEST_RE.search(prompt or ""))


# --- shape two: delegation as completion ----------------------------------------------------------

# The subject has to be the delegate. "it" is included and is the reason the enumeration test below
# exists: "it has" is an extremely common English opening, and only a list of deliverables makes it
# a claim about an agent's output.
DELEGATE_SUBJECT = (
    r"(?:it|they|the\s+(?:agent|subagent|sub-agent|delegate|fork|worker|run)|that\s+agent"
    r"|the\s+(?:one|brief)\s+i\s+(?:just\s+)?(?:sent|dispatched|spawned|launched))"
)

DELEGATION_PRESENT_RE = re.compile(
    r"^(?:and\s+|but\s+|so\s+)?" + DELEGATE_SUBJECT + r"\s+(?:also\s+|already\s+|now\s+)?"
    r"(?:has|have|covers?|includes?|contains?|carries|carry|handles?|takes?\s+in|gets?"
    r"|is\s+(?:building|writing|producing|running|measuring|adding|handling|covering)"
    r"|are\s+(?:building|writing|producing|running|measuring|adding|handling|covering))\b",
    re.IGNORECASE,
)

DELEGATION_FUTURE_RE = re.compile(
    r"^(?:and\s+|but\s+|so\s+)?" + DELEGATE_SUBJECT + r"\s+(?:will|'ll|’ll|should|is\s+going\s+to)\s+"
    r"(?:also\s+|then\s+)?"
    r"(?:produce|report|return|come\s+back|deliver|land|include|cover|have|add|write|measure"
    r"|give|hand\s+back|bring\s+back)\b",
    re.IGNORECASE,
)

# What a delegate's output is made of in this repo. A claim about an agent that names none of these
# is a status remark, not a delivery claim.
DELIVERABLE_RE = re.compile(
    r"\b(?:tests?|cases?|rates?|gates?|evidence|proof|polic(?:y|ies)|rules?|reports?|numbers?"
    r"|measurements?|coverage|fixtures?|benchmarks?|outputs?|results?|findings?|deliverables?"
    r"|patch(?:es)?|diffs?|sentences?|exemptions?|eval|signals?|scores?|metrics?|summar(?:y|ies)"
    r"|analys[ei]s|answers?|files?|regexes?|assertions?|verdicts?)\b",
    re.IGNORECASE,
)

# A statement about the instruction rather than the output: a fact about the caller's own tool call,
# which the caller does know. Kept as a suppressor on the offending sentence itself, so a message
# that says both ("I asked it to measure the rate. It has the measurement.") still convicts on the
# second sentence.
INSTRUCTION_FRAMED_RE = re.compile(
    r"\bi\s+(?:asked|told|gave|instructed|briefed|handed|sent|dispatched|spawned|launched)\b"
    r"|\basked\s+it\s+to\b|\btold\s+it\s+to\b"
    r"|\bthe\s+(?:brief|prompt|instructions?|task|spec|remit|ask)\s+"
    r"(?:says|carries|names|includes|asks|requires|is|covers)\b"
    r"|\bit\s+was\s+(?:asked|told|given|briefed)\b"
    r"|\bits\s+(?:brief|instructions?|prompt|remit)\b"
    r"|\b(?:it|the\s+agent|the\s+subagent)(?:'|’)?s?\s+instructed\b"
    r"|\bpre-?decide\b|\bpre-?decided\b",
    re.IGNORECASE,
)


def _enumerates(sentence: str) -> bool:
    """True when the sentence lists deliverables rather than mentioning one in passing.

    Two deliverable nouns and a list shape -- two commas, or a colon with a comma after it. A single
    noun ("a subagent is working on the policy") is a dispatch statement, which the directive
    exempts, and one noun inside a long sentence is how "it has four specific things to prove" read
    as a delivery claim over the real transcripts.
    """
    if len(DELIVERABLE_RE.findall(sentence)) < 2:
        return False
    if sentence.count(",") >= 2:
        return True
    return ":" in sentence and "," in sentence.split(":", 1)[1]


def delegation_claim(closing_text: str, tail_sentences: int = 3) -> str:
    """The closing sentence that asserts a delegate's deliverables, or '' when there is none."""
    scrubbed = strip_quoted(closing_text)
    for sentence in reversed(sentences(scrubbed)[-tail_sentences:]):
        if INSTRUCTION_FRAMED_RE.search(sentence):
            continue
        if not (DELEGATION_PRESENT_RE.search(sentence) or DELEGATION_FUTURE_RE.search(sentence)):
            continue
        if _enumerates(sentence):
            return quote(sentence)
    return ""


# Enough to say a sentence is about a delegate at all. Weaker than the subject-plus-verb patterns
# above on purpose: this one only ever exempts, and it exempts only alongside instruction framing.
DELEGATE_MENTION_RE = re.compile(
    r"\b(?:it|they|them|the\s+(?:agent|subagent|sub-agent|delegate|fork|worker)"
    r"|a\s+(?:agent|subagent|sub-agent))\b",
    re.IGNORECASE,
)


def delegation_instruction_framed(closing_text: str, tail_sentences: int = 3) -> bool:
    """True when a closing sentence enumerates what was asked of a delegate rather than what it
    produced -- "I asked it to measure the rate, run the gates, and report the number".

    That is a fact about the caller's own tool call, which the caller does know, so it must not be
    punished. Reported as its own fact so the exemption is visible in the halt input and testable in
    Rego rather than hiding as a silent `continue` in this file.

    The signal never emits this beside a surviving claim: `delegation_claim` already skips
    instruction-framed sentences, so a message carrying both ("I asked it to measure the rate. It
    has the measurement.") still convicts on the second sentence and reports `instructed=0`. That is
    the stricter reading, chosen deliberately -- an exemption that could be armed by adding one
    honest sentence would be worth nothing.
    """
    scrubbed = strip_quoted(closing_text)
    for sentence in reversed(sentences(scrubbed)[-tail_sentences:]):
        if not INSTRUCTION_FRAMED_RE.search(sentence):
            continue
        if _enumerates(sentence) and DELEGATE_MENTION_RE.search(sentence):
            return True
    return False


LAUNCH_ACK_RE = re.compile(
    r"async agent launched|agentId:|command running in background|running in background with id"
    r"|you will be notified when it completes",
    re.IGNORECASE,
)

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


def unreported_dispatch(turn_tool_blocks: list[dict], events: list[dict]) -> bool:
    """True when this turn dispatched an agent whose results have not come back.

    An agent whose completion notification is in the transcript can be quoted freely -- that is
    relaying, not predicting. The distinction is the whole rule, so it is read out of the transcript
    rather than inferred from prose.
    """
    dispatched = {
        block.get("id")
        for block in turn_tool_blocks
        if _tool_name(block) in ("agent", "task") and isinstance(block, dict)
    }
    dispatched.discard(None)
    if not dispatched:
        return False

    finished: set[str] = set()
    for ev in events or []:
        message = ev.get("message", {}) if isinstance(ev, dict) else {}
        content = message.get("content")
        if isinstance(content, str):
            if TASK_NOTIFICATION_RE.search(content):
                ident = TASK_TOOL_USE_ID_RE.search(content)
                status = TASK_STATUS_RE.search(content)
                if ident and status and status.group(1).lower() in FINISHED_STATUSES:
                    finished.add(ident.group(1))
            continue
        for block in content or []:
            if not isinstance(block, dict) or block.get("type") != "tool_result":
                continue
            ident = block.get("tool_use_id")
            if ident in dispatched and not LAUNCH_ACK_RE.search(_result_text(block)):
                # A real result rather than the launch acknowledgement: the agent answered inline.
                finished.add(ident)
    return bool(dispatched - finished)
