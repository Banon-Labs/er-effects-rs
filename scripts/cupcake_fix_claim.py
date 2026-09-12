#!/usr/bin/env python3
"""Detect a closing message that calls a change a fix when no run showed the change working.

User, 2026-09-11: "If someone ever says real fix to me and doesn't put it in airquotes, I look at
them like this" -- and then, when nothing stopped it: "We are supposed to have a rego policy that
stops you from saying 'fix' without runtime evidence". The turn that prompted it had edited a crate
that ships inside the game, never run the game, and closed on "the real fix". It failed live on the
next launch.

Kept as an importable module rather than an inline heredoc so it is unit-testable without a live
transcript, the way `cupcake_narrated_action.py` and `cupcake_unbacked_claim.py` are.
`.cupcake/signals/last_assistant_fix_claim_without_runtime_evidence.sh` imports it, and so does
`scripts/test-fix-claim-classifier.py`, so the test cannot pass against a classifier production
does not run.

The vocabulary this module owns, and the boundary it must not cross: the present participle
"fixing" belongs to `ER-EFFECTS-NO-PROMISSORY-CLOSER`, which already halts on "Fixing both: ...".
Charging one sentence twice with two different corrections is how a guard layer becomes noise, so
no pattern here matches it.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

# --- (1) the closing prose called a change a fix ------------------------------------------------
# An explicit alternation rather than the bare stem. A turn that edits a runtime crate and mentions
# the word at all is not the failure: "this needs a fix in the loader too" names remaining work and
# "the fix for the other bug is unrelated" is ordinary reference. What the directive is about is a
# change being asserted to be the fix, so every branch below carries that assertion in its grammar.
#
# `real fix` stands alone because it is the phrase the user quoted, twice, and it is the one that
# cannot be said honestly about an unrun change.
_FIX_CLAIM = re.compile(
    r"\breal\s+fix\b"
    r"|\b(?:this|that|it|here)(?:'s|\s+is|\s+was)\s+(?:the|a|my|our)\s+"
    r"(?:real\s+|actual\s+|genuine\s+|proper\s+|right\s+|correct\s+|only\s+|true\s+|whole\s+)?fix\b"
    r"|\b(?:the|a|my|our)\s+"
    r"(?:real\s+|actual\s+|genuine\s+|proper\s+|right\s+|correct\s+|only\s+|true\s+|whole\s+)?"
    r"fix\s+(?:works|worked|holds|holds\s+up|landed|is\s+in\b|is\s+live\b|is\s+done\b)"
    r"|\b(?:this|that|it)\s+fixes\b"
    r"|\bfixes\s+(?:it|that|this|the)\b"
    r"|\bI(?:'ve|\s+have)?\s+(?:just\s+|now\s+|already\s+)?fixed\b"
    r"|\b(?:is|are|was|were|now)\s+fixed\b"
    r"|^\s*fixed\b",
    re.IGNORECASE,
)

# The stems that are names rather than claims. `\b` already excludes `prefix`, `suffix`, `bugfix`
# and `hotfix`, because a word character sits in front of the stem in each; these are the ones it
# does not exclude on its own. A branch (`fix/harness-repl-reach`), a slug (`fix-the-loader`), a
# file (`fix.md`) and `fixture` all read as "fix" to a word-boundary match and none of them is a
# claim about anything.
_NAME_STEM = re.compile(r"\bfix(?:ture|/|-[A-Za-z0-9]|\.[A-Za-z0-9])")

# An honest hedge suppresses the hit outright, and that is the whole point: saying a change is
# unverified is the behaviour being asked for and must never be punished. Scanned over the entire
# closing prose run rather than the matched sentence, so a claim in one paragraph and its hedge in
# the next still passes -- a guard that demanded the two sit in one sentence would teach people to
# stop hedging rather than to hedge better.
_HEDGE = re.compile(
    r"\bunverified\b|\bunproven\b|\buntested\b|\bunconfirmed\b"
    r"|\bnot\s+(?:yet\s+)?(?:proven|verified|confirmed|tested|run|launched|validated|observed)\b"
    r"|\bno\s+(?:run|runtime\s+evidence|evidence|proof|oracle|telemetry)\b"
    r"|\bneeds?\s+(?:a\s+)?(?:run|launch|runtime\s+proof|runtime\s+evidence|validation|testing)\b"
    r"|\bhas\s+not\s+(?:run|been\s+run|been\s+launched)\b"
    r"|\bI\s+(?:have\s+not|haven't|did\s+not|didn't)\s+(?:run|launch|test|verify|observe)\b"
    r"|\byet\s+to\s+(?:be\s+)?(?:run|prove|proven|verify|verified)\b"
    r"|\bawaiting\s+(?:a\s+)?run\b|\buntil\s+(?:a\s+|the\s+)?(?:run|launch)\b"
    r"|\battempt(?:s|ed)?\b|\bcandidate\b|\bnot\s+a\s+fix\b|\bmay\s+not\b"
    r"|\bshould\s+fix\b|\bmight\s+fix\b|\bmay\s+fix\b|\bwould\s+fix\b|\bif\s+it\s+works\b"
    # Measured, not guessed: this spelling closed the one 2026-08 turn in the corpus whose closing
    # line already said the change had not run, in the repo's own idiom rather than in the
    # dictionary's -- "the table now exists once ... (DLL 798414f3, ready for the next run)".
    r"|\b(?:ready\s+)?for\s+the\s+next\s+run\b|\bon\s+the\s+next\s+run\b",
    re.IGNORECASE,
)

# A dependency the agent cannot dissolve by working harder. Same family the neighbouring guards
# exempt, and for the same reason: a claim that cannot be tested yet is not a claim made carelessly.
_BLOCKED = re.compile(
    r"\bsudo\b|\bcredential|\bpassword\b|\blog\s*in\b|\blogin\b|\bpurchase\b"
    r"|\bsteam\s+is\s+(?:not|down)\b|\bno\s+(?:game|session)\s+is\s+(?:up|running)\b"
    r"|\bneeds?\s+(?:the\s+)?game\s+running\b|\bwith\s+the\s+game\s+down\b"
    r"|\bwait(?:ing)?\s+for\s+(?:the\s+)?(?:user|you)\b"
    r"|\bblocked\s+on\s+(?:the\s+)?(?:user|you)\b"
    r"|\btell\s+me\s+what\s+you\s+(?:saw|see)\b"
    r"|\bonce\s+you\b|\bover\s+to\s+you\b"
    # A decision handed to the user is a dependency like any other, and the corpus turn that forced
    # this branch names a real fix it deliberately did not make: "... which I've left for you to
    # call since it changes behaviour". Charging that sentence would punish the handback the rest of
    # this guard layer asks for.
    r"|\bleft\s+(?:it\s+|that\s+|them\s+|this\s+)?(?:for|to)\s+you\b"
    r"|\bfor\s+you\s+to\s+call\b|\byour\s+call\b",
    re.IGNORECASE,
)

# --- the claim is about host work, which a run cannot show either way ---------------------------
# Two of the five real turns this classifier convicted on a first pass had fixed a GATE: "Two gates
# broke on the way and I fixed them rather than shaving around them", and "The integration gate came
# back red and I've fixed all four failures". Both are honest reports of host work whose proof is the
# gate going green, and demanding a game launch for a clippy lint is how a guard earns a reputation
# for being wrong.
#
# The exemption needs both halves. A host noun on its own would swallow "the fix works and the tests
# pass", which is the exact substitution the directive is about -- a passing test is not a run. So it
# applies only when the sentence names host machinery and names nothing that lives inside the game.
_HOST_OBJECT = re.compile(
    r"\b(?:gate|gates|check|checks|selftest|selftests|test|tests|suite|suites|lint|lints"
    r"|clippy|rustfmt|fmt|compile|compiles|compiler|warning|warnings|workflow|ci|typo|docstring)\b",
    re.IGNORECASE,
)

# Deliberately stem-matched rather than word-matched, so "hooked", "loading" and "rendered" count.
_RUNTIME_NOUN = re.compile(
    r"\b(?:crash|hang|softlock|game|load|menu|row|button|hook|detour|dll|player|save|boot"
    r"|launch|frame|render|animation|popup|invasion|warp|world|character|runtime|live"
    r"|oracle|telemetry)",
    re.IGNORECASE,
)

# --- (2) what a run leaves behind ---------------------------------------------------------------
# Deliberately an allowlist of artifact shapes rather than a loose notion of "ran something". The
# distinction the directive turns on is that a build is not a run and a launch is not a run: only
# something a run wrote, read back afterwards, says the change behaved.
#
# What is in it:
#   er-<name>.log        the per-module log a loaded DLL writes, the `er-*.log` family the
#                        runtime-evidence push guard already reads: er-quit-rows-debug.log,
#                        er-quickload-autoload-debug.log, er-invasion-warp.log, and the rest;
#   er-<name>telemetry<name>.json  the oracle dump, er-quickload-telemetry.json and its siblings;
#   er-<name>.jsonl      the streamed records: phases, timeseries, profile, bootstrap, input trace;
#   er-me3-runs          the run root a launch writes its artifact directory under;
#   br-<date>-<time>-<id>  a run id, the same shape the proof-without-observation guard reads;
#   oracle_<field>       a named in-process memory-read semaphore;
#   er-live-fields.py    a read of the live process through /proc/<pid>/mem;
#   er-teardown.py --status   the live-session state read, which needs both tokens together;
#   er-readiness-watch.py     the watcher that collects the semaphores during a run;
#   er-frida-watch.py         an attached agent reading the running game.
#
# What is deliberately absent, because each of them was offered as proof in the turn that prompted
# this rule: `cargo build`, `cargo xwin build`, `cargo test`, `cargo check`, `scripts/er-build-dlls.sh`,
# `scripts/check-rust-build.sh`, a `sha256sum` of the artifact, `scripts/er-run-branch.py`, and
# `~/Elden/launch.sh`. Building proves it compiles. Launching proves the game starts.
_RUNTIME_ARTIFACT = re.compile(
    r"\ber-[a-z0-9-]+\.log\b"
    r"|\ber-[a-z0-9-]*telemetry[a-z0-9-]*\.json\b"
    r"|\ber-[a-z0-9-]+\.jsonl\b"
    r"|er-me3-runs|ER_ME3_RUN_ROOT"
    r"|\bbr-\d{8}-\d{6}-[0-9a-z]+"
    r"|\boracle_[a-z][a-z0-9_]*"
    r"|er-live-fields\.py"
    r"|er-readiness-watch\.py"
    r"|er-frida-watch\.py",
    re.IGNORECASE,
)

# The one artifact read that needs two tokens in one command: the script alone can also tear a
# session down, and a teardown is not a measurement.
_TEARDOWN_STATUS = re.compile(r"er-teardown\.py[^\n]*--status")

_WRITE_TOOLS = {"edit", "write", "multiedit", "notebookedit"}

# A Bash write whose target is a crate path, matched as one span so the target is what decides.
# Narrower than the list the diagnosis signal carries, and narrowed on purpose: there a heredoc
# counting as an edit exonerates a turn, here it would convict one, and `python3 - <<'PY'` reading
# three files under `crates/` is how most of the reading in this repo is done. A bare heredoc is
# therefore not a write here -- only a redirect, `tee`, `patch` or an in-place `sed` that names a
# crate path.
_BASH_WRITE_TARGET = re.compile(
    r">>?\s*[^\s|&;<>]*crates/[A-Za-z0-9_-]+/[^\s|&;<>]*"
    r"|\b(?:tee|patch)\b[^|;]*?crates/[A-Za-z0-9_-]+/[^\s|&;<>\'\"]*"
    r"|\bsed\s+(?:-[^\s]*\s+)*-i\b[^|;]*?crates/[A-Za-z0-9_-]+/[^\s|&;<>\'\"]*"
)

# A path inside one workspace crate. The capture is the crate directory name, which is what decides
# whether the edit can reach the game at all.
_CRATE_PATH = re.compile(r"(?:^|[\s\"'=/])crates/([A-Za-z0-9_-]+)/([^\s\"']*)")

# Crate subtrees that are host work even in a crate that ships in a DLL.
_HOST_SUBTREE = ("tests/", "benches/", "examples/")


def scrub(text: str) -> str:
    """Drop fenced, backticked and double-quoted spans, as every neighbouring signal does.

    Quoting the word is the airquotes the directive asks for, so a scare-quoted "fix" and a
    backticked one both stop being claims here rather than needing a rule of their own.
    """
    text = re.sub(r"```.*?```", " ", text or "", flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]{0,400}"', " ", text)


def fix_claim(closing_text: str) -> str:
    """The first sentence of the closing prose that calls a change a fix, or '' when there is none."""
    for raw in re.split(r"(?<=[.!?;])\s+|\n", scrub(closing_text)):
        sentence = raw.strip()
        if not sentence or sentence.startswith("|"):
            continue  # a table cell most often holds a label, not a claim
        if _FIX_CLAIM.search(_NAME_STEM.sub(" ", sentence)):
            return " ".join(sentence.split())[:220].replace("|", "/")
    return ""


def hedged(closing_text: str) -> bool:
    """True when the closing prose admits somewhere that the change has not been shown to work."""
    return bool(_HEDGE.search(scrub(closing_text)))


def externally_blocked(closing_text: str) -> bool:
    """True when the closing prose names a dependency the agent cannot dissolve by working harder."""
    return bool(_BLOCKED.search(scrub(closing_text)))


def host_object(claim_sentence: str) -> bool:
    """True when the sentence says host machinery was fixed and names nothing inside the game."""
    if not claim_sentence:
        return False
    return bool(_HOST_OBJECT.search(claim_sentence)) and not _RUNTIME_NOUN.search(claim_sentence)


def runtime_crates(repo_root: str | Path) -> set[str]:
    """Crate directory names whose source can end up inside a DLL the game loads.

    Measured from the workspace manifests rather than listed by hand, because a hand-written list
    rots the first time a crate is added and rots silently. A crate qualifies when it declares a
    `cdylib` artifact, or when a crate that does reaches it through a path dependency. On this tree
    that is 63 of the 65 crate directories; `er-objectkit` and `soulsformats` are host libraries and
    a change confined to one of them cannot be proven or disproven by a run.

    Returns an empty set on any read failure, which makes the whole guard silent. That is the safe
    direction for a rule whose false positive gags an honest report, and the classifier regression
    asserts the walk still finds the crates it is supposed to find.
    """
    root = Path(repo_root) / "crates"
    manifests: dict[str, tuple[bool, set[str]]] = {}
    try:
        for toml in sorted(root.glob("*/Cargo.toml")):
            text = toml.read_text(encoding="utf-8", errors="replace")
            deps = set(re.findall(r'path\s*=\s*"\.\./([A-Za-z0-9_-]+)"', text))
            manifests[toml.parent.name] = ("cdylib" in text, deps)
    except OSError:
        return set()
    reachable = {name for name, (cdylib, _deps) in manifests.items() if cdylib}
    growing = True
    while growing:
        growing = False
        for name in list(reachable):
            for dep in manifests.get(name, (False, set()))[1]:
                if dep in manifests and dep not in reachable:
                    reachable.add(dep)
                    growing = True
    return reachable


def _written_paths(block: dict) -> list[str]:
    """Paths a single tool_use block put bytes into. Empty for a read, a build or a launch."""
    if not isinstance(block, dict):
        return []
    name = str(block.get("name") or block.get("tool_name") or "").strip().lower().replace("_", "")
    raw = block.get("input") or {}
    if not isinstance(raw, dict):
        return []
    if name in _WRITE_TOOLS:
        return [str(raw.get("file_path") or raw.get("notebook_path") or "")]
    if name != "bash":
        return []
    return _BASH_WRITE_TARGET.findall(str(raw.get("command") or ""))


def wrote_runtime_source(block: dict, crates: set[str]) -> bool:
    """True when this tool_use changed a file that can end up inside a loaded DLL."""
    for candidate in _written_paths(block):
        for crate, tail in _CRATE_PATH.findall(candidate):
            if crate not in crates:
                continue
            if tail.startswith(_HOST_SUBTREE):
                continue
            if tail.endswith((".rs", ".toml", ".json", ".tsv")) or tail == "":
                return True
    return False


def reads_run_artifact(block: dict) -> bool:
    """True when this tool_use opened something a run of the game produced."""
    if not isinstance(block, dict):
        return False
    try:
        payload = json.dumps(block.get("input") or {})
    except (TypeError, ValueError):
        return False
    return bool(_RUNTIME_ARTIFACT.search(payload) or _TEARDOWN_STATUS.search(payload))


def cites_run_artifact(text: str) -> bool:
    """True when the prose itself names a run artifact, an oracle field or a run id.

    Searched unscrubbed and over the whole turn's prose, the way the proof-without-observation
    guard searches its evidence half: a run id most often arrives inside a fenced block, and a
    generous evidence search fails toward silence.
    """
    return bool(_RUNTIME_ARTIFACT.search(text or "") or _TEARDOWN_STATUS.search(text or ""))


def runtime_evidence(blocks: list[tuple], crates: set[str]) -> tuple[bool, bool]:
    """(changed, evidence) for one turn's ordered block stream.

    `changed` is a write to a crate that ships in a DLL. `evidence` is a run artifact opened at or
    after that write -- ordering is the whole point, because a log read before the edit describes
    the code that was there before it. Measured from the first such write rather than the last, so
    a turn that reads the log and then makes one more small edit still counts as having looked.
    """
    first_write = None
    for index, (kind, block) in enumerate(blocks):
        if kind == "tool" and wrote_runtime_source(block, crates):
            first_write = index
            break
    if first_write is None:
        return False, False
    evidence = any(
        kind == "tool" and reads_run_artifact(block)
        for kind, block in blocks[first_write:]
    )
    return True, evidence
