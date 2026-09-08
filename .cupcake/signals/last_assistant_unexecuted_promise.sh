#!/usr/bin/env bash
# Cupcake signal: last_assistant_unexecuted_promise
#
# Consumed by:
#   * no_unexecuted_promise (Stop): halts turn-end when the turn ended on a first-person promise to do
#     something, and nothing in the world is going to do it.
#
# Why this exists (user directive 2026-08-22, in their words):
#   "How can I prevent you from ever saying 'I'll <statement of future action>' and then landing on no
#    shells running, no monitors, and no directive to me to explain why a user is required to
#    re-initiate the task."
# The instance: a turn ended "I'll re-record the directive with the shell metacharacters escaped
# rather than leave it unsaved" -- and then stopped. No tool call, no background task, no statement
# that the user had to do anything. The work evaporated and the user had to notice and re-ask. Prose
# promises are unenforceable by prose; only a Stop hook that refuses the stop closes this.
#
# The violation is the conjunction of four facts. All four must hold, and any one missing is fine:
#   1. the turn's final prose block contains a first-person commitment to a concrete action
#      ("I'll re-run the gate", "I'm going to patch it", "let me check the offsets", and -- the shape
#      that walked through the guard on 2026-09-01 -- the bare present continuous "I'm closing it");
#   2. no tool_use follows that prose in the turn -- nothing executed it;
#   3. nothing is carrying it: no watcher (detached shell / Monitor / SendMessage) in the turn, and
#      no live background job that the promise actually waits on ("once it lands", "whatever it
#      finds"). A live job the promise never mentions is not cover -- a game session the user is
#      inspecting can be up for an hour, and the promise is deferred behind it, not carried by it;
#   4. the message does not hand the obligation to the user -- no question, no blocker statement, no
#      "once you've X", no "I'll need you to Y", no "next session".
# Emitted as  PROMISE:<the offending clause>  ; empty when the turn is clean.
#
# Biased hard toward not firing, on purpose. A guard that cries wolf gets ignored, which is worse than
# no guard. Three deliberate narrowings:
#   * only the final prose block is scanned. A mid-turn "I'll check the disassembly" that is followed
#     by the tool call doing exactly that is the normal, correct shape and must never be touched.
#   * the committed verb must be on a concrete-action allowlist. Stance and mental verbs ("I'll keep
#     that in mind", "I'll treat it as unproven"), verbs the same message already fulfils ("I'll
#     summarise", "I'll explain"), hedges ("I'll try to", "I'll probably") and negations ("I'll never")
#     are not promises of work and are all excluded.
#   * quoted spans, backticked spans and fenced code are stripped before matching, so quoting the ban
#     -- this file, the policy text, a test fixture -- cannot trip it.
#
# The shared half of the classification (turn bucketing, substantive-work vs status-peek, the
# blocked-on-user phrasing, live background work) is not reimplemented here: it comes from
# scripts/cupcake_turn_scan.py, the same module last_assistant_idle_hold.sh uses, so the two guards
# cannot drift into disagreeing about the same turn. Fail-open (empty output) on any error.
set -uo pipefail
CUPCAKE_SIGNAL_REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]:-$0}")/../.." && pwd)"
export CUPCAKE_SIGNAL_REPO_ROOT
python3 - <<'PY' 2>/dev/null || true
import os, re, sys

sys.path.insert(0, os.path.join(os.environ.get("CUPCAKE_SIGNAL_REPO_ROOT", "."), "scripts"))
try:
    import cupcake_turn_scan as scan
except Exception:
    sys.exit(0)  # fail open: a missing helper must never wedge a session

path = scan.latest_transcript()
if not path:
    sys.exit(0)
events = scan.load_events(path)
turn = scan.last_text_turn(scan.split_turns(events))
if turn is None:
    sys.exit(0)

# (2) Anything after the closing prose executed it. At Stop the last block is prose by construction,
# but an interrupted turn can end on a tool call, and that turn did act.
idx = turn.last_text_index
if idx < 0 or turn.tool_after(idx):
    sys.exit(0)
final = turn.blocks[idx][1]

# Strip fenced code, inline-backticked spans and double-quoted spans before prose matching, so
# quoting the ban does not count as committing it. Single quotes are left alone: the phrases
# themselves contain apostrophes (I'll / I'm).
scrubbed = re.sub(r"```.*?```", " ", final, flags=re.DOTALL)
scrubbed = re.sub(r"`[^`]*`", " ", scrubbed)
scrubbed = re.sub(r'"[^"]*"', " ", scrubbed)

# --- (1) first-person future commitment --------------------------------------------------------
# The openers an assistant actually uses to commit itself. "let's" is deliberately absent: it
# proposes joint action rather than committing the agent.
#
# The bare present continuous ("I'm closing it") is last, and it is not cosmetic. The guard shipped
# without it and a turn walked straight through the hole: "I'm closing it rather than pushing an
# empty merge commit to make an empty PR green" -- no `gh pr close` ran, the PR stayed open, and the
# user had to notice and ask. Because the opener never matched, the other three facts were never even
# evaluated. Present continuous is the more seductive way to make an unkept promise than "I'll",
# precisely because it reads to a human as already underway.
#
# Order matters. Python alternation takes the first alternative that matches at a position, not the
# longest, so the bare form must come after "i'm going to" / "i'm about to" -- ahead of them it would
# swallow their prefix and hand `committed_verb` the word "going", which is deliberately not an
# action. Everything that keeps this narrow lives in `committed_verb`: the next non-filler word must
# be the -ING form of a verb already on the concrete-action allowlist, so the stative uses this
# repo's prose is full of ("I'm treating that as unproven", "I'm reporting it rather than fixing it",
# "I'm not sure", "I'm quoting the failure below") resolve to no action verb and stay silent.
OPENER_RE = re.compile(
    r"\b(?:i['’]?ll"
    r"|i\s+will"
    r"|i['’]?m\s+going\s+to|i\s+am\s+going\s+to"
    r"|i['’]?m\s+about\s+to|i\s+am\s+about\s+to"
    r"|let\s+me"
    r"|i['’]?m|i\s+am)\b",
    re.IGNORECASE,
)

# Words between the opener and the verb that carry no commitment either way.
FILLER = {
    "then", "now", "also", "just", "first", "next", "go", "immediately", "still", "quickly",
    "actually", "already", "instead", "finally", "again", "simply", "promptly", "right", "away",
    "ahead", "and", "straight", "also", "briefly", "properly", "fully", "quietly", "one", "more",
}

# A hedge is not a commitment, and a negation is the opposite of one. Either kills the match.
HEDGE = {
    "probably", "likely", "maybe", "perhaps", "possibly", "hopefully", "try", "attempt", "consider",
    "think", "might", "may", "plan", "intend", "want", "hope", "aim", "prefer", "need", "have",
    "should", "could", "would", "must", "can", "expect", "guess", "suppose",
}
NEGATION = {"not", "never", "no", "nothing", "avoid", "stop", "refrain", "skip", "leave", "hold"}

# Verbs that name work only a tool call can do. Everything outside this list is left alone on
# purpose: stance verbs ("keep", "treat", "use", "follow", "remember"), verbs a message fulfils by
# itself ("summarise", "explain", "describe", "list", "note", "mention", "show", "tell"), and vague
# ones ("do", "get", "take", "handle", "continue") are not evidence of unexecuted work.
ACTIONS = {
    "run", "rerun", "launch", "relaunch", "build", "rebuild", "compile", "check", "recheck",
    "reread", "recapture", "reinstall", "rerecord", "redo", "reopen",
    "verify", "validate", "test", "retest", "fix", "patch", "repair", "correct", "add", "remove",
    "delete", "drop", "write", "rewrite", "record", "save", "update", "edit", "create", "commit",
    "push", "pull", "rebase", "merge", "open", "file", "land", "implement", "apply", "revert",
    "restore", "capture", "read", "inspect", "investigate", "search", "look", "dig",
    "trace", "disassemble", "decompile", "hook", "probe", "measure", "diff", "compare", "clean",
    "stage", "package", "install", "generate", "regenerate", "wire", "refactor", "rename", "move",
    "copy", "kill", "restart", "retry", "send", "post", "reply", "document", "extend", "factor",
    "migrate", "port", "bump", "pin", "split", "tag", "publish", "deploy", "sync", "fetch",
    "download", "upload", "convert", "parse", "dump", "scan", "grep", "find", "count", "render",
    "screenshot", "audit", "lint", "format", "benchmark", "profile", "instrument", "emit",
    "register", "enable", "disable", "toggle", "configure", "tune", "adjust", "tweak", "resolve",
    "ship", "harden", "delegate", "dispatch", "spawn", "teardown", "follow",
    # Terminal actions. `open` and `reopen` were here from the start and `close` was not, which is an
    # oversight rather than a decision -- and it is half of why "I'm closing it rather than pushing an
    # empty merge commit to make an empty PR green" walked through the guard on 2026-09-01: even with
    # the opener fixed there was no verb on the list for it to commit to. `gh pr close` is a tool call
    # like any other. The siblings are added with it because they name the same terminal act on a
    # branch/PR/script, though only "close" is evidenced in the transcript corpus (4 uses after a
    # first-person opener; abandon/archive/withdraw/retire: zero).
    "close", "archive", "retire", "withdraw", "abandon",
    # In-game input is agent work in this repo (standing order: the agent drives every input), so a
    # promise to press/drive/navigate/inject is a promise of a tool call like any other.
    "press", "drive", "navigate", "inject",
}

WORD_RE = re.compile(r"[A-Za-z][A-Za-z’'\-]*")


def base_of_gerund(word):
    """The concrete-action verb an -ING form inflects, or None.

    Deliberately NOT a stemmer and deliberately NOT a second vocabulary: it proposes the three
    regular English spellings and accepts one only if it is already on ACTIONS. That is what keeps
    the tense change from widening the guard -- "reporting" proposes "report"/"reporte", neither is
    an action, so it is silent; "hoping" proposes "hop"/"hope" and is silent for the same reason,
    without needing to know that "hope" is a hedge. A free-running stemmer would instead have to be
    told, in a second list, every word it must not invent a promise out of.
    """
    if not word.endswith("ing") or len(word) < 6:
        return None
    stem = word[:-3]
    candidates = [stem, stem + "e"]
    if len(stem) > 2 and stem[-1] == stem[-2]:
        candidates.append(stem[:-1])  # commit -> committing, run -> running
    for cand in candidates:
        if cand in ACTIONS:
            return cand
    return None


def committed_verb(tail):
    """The concrete-action verb this opener commits to, or None."""
    for raw in WORD_RE.findall(tail)[:6]:
        word = raw.lower().replace("’", "'").strip("-'")
        if word in FILLER:
            continue
        if word in NEGATION or word in HEDGE:
            return None
        if word in ACTIONS:
            return word
        # A HYPHENATED re- form is the same verb ("re-record" -> "record"). A merged one is not
        # derivable: stripping a bare "re" turns "report" into "port" and invents a promise that was
        # never made (caught by scripts/audit-unexecuted-promise-false-positives.py against real
        # transcripts). Merged forms are enumerated in actions instead.
        if word.startswith("re-") and word[3:] in ACTIONS:
            return word
        # Present continuous: "I'm closing it", "I'm reverting it", "I'm landing it". Same verb, same
        # allowlist, one tense later -- and the same hyphenated-re- rule ("re-recording").
        base = base_of_gerund(word)
        if base:
            return base
        if word.startswith("re-") and base_of_gerund(word[3:]):
            return word
        return None
    return None


def clause(text, start):
    """The offending sentence, collapsed and clipped, for quoting back at the agent."""
    end = len(text)
    for m in re.finditer(r"[.!?\n]", text[start:]):
        end = start + m.start() + 1
        break
    return " ".join(text[start:end].split())[:110]


# The last committing sentence, not the first. Which one fires is unchanged either way -- any one of
# them is the violation -- but the halt quotes this clause back at the agent, so it should be the
# promise nearest where the turn actually stopped. Measured when the bare present continuous was
# added (2026-09-01): on a real halted turn the first match moved from the closing sentence "I'll
# revert the marker spawn to the wrapper..." back to an incidental table cell earlier in the same
# message ("why I'm not guessing | I'm re-running it with..."), which is a worse instruction to hand
# back. Broadening the opener set makes an early incidental match likelier; taking the last one
# cancels that out.
promise = None
for m in OPENER_RE.finditer(scrubbed):
    if committed_verb(scrubbed[m.end():m.end() + 120]):
        promise = clause(scrubbed, m.start())
if not promise:
    sys.exit(0)

# --- (4) the obligation was handed to the user -------------------------------------------------
# A question, a stated blocker, a dependency on something the user must do, or an explicit
# "this needs re-initiating" all mean the user knows the ball is theirs. Checked over the whole
# closing message, not just the promising sentence.
HANDOFF_RE = re.compile(
    r"\?"
    r"|\bonce\s+(?:you|your|the\s+user)\b"
    r"|\bwhen\s+(?:you|your|the\s+user)\b"
    r"|\bafter\s+(?:you|your|the\s+user)\b"
    r"|\bif\s+(?:you|your|the\s+user)\b"
    r"|\bas\s+soon\s+as\s+you\b"
    r"|\b(?:while|until)\s+you\b"
    r"|\bunless\s+you\b"
    r"|\byou'?(?:ll|d)\s+(?:need|have)\s+to\b|\byou\s+will\s+need\s+to\b"
    r"|\byou\s+(?:need|have)\s+to\b"
    r"|\bneed\s+you\s+to\b|\bneed\s+the\s+user\s+to\b"
    r"|\blet\s+me\s+know\b|\btell\s+me\b|\bping\s+me\b|\bcome\s+back\s+to\s+me\b"
    r"|\byour\s+call\b|\bsay\s+the\s+word\b|\bup\s+to\s+you\b"
    r"|\bre-?initiate\b|\bre-?ask\b|\bask\s+me\s+again\b|\bask\s+again\b"
    r"|\bnext\s+(?:session|turn|time)\b|\bfollow-?up\s+turn\b"
    r"|\bblocked\s+on\b|\bcan'?t\s+proceed\b|\bcannot\s+proceed\b"
    r"|\brequires?\s+(?:you|your|a\s+user|the\s+user)\b"
    r"|\bneeds?\s+(?:you|your)\b",
    re.IGNORECASE,
)
if HANDOFF_RE.search(scrubbed) or scan.blocked_on_user(scrubbed):
    sys.exit(0)

# --- (3) something is already carrying it ------------------------------------------------------
# "Something is running" is not by itself cover. An Elden Ring session the user is inspecting can be
# up for an hour, and a promise to go fix an unrelated file is not being carried by it -- it is being
# deferred behind it, which is the disappearance this guard exists to stop (measured: the reported
# instance was silenced by an unrelated game launch two turns earlier). A live job covers a promise
# in exactly two cases: This turn started it (the harness wakes the agent when it exits, so the agent
# genuinely comes back), or the promise explicitly waits on its result.
WAITS_ON_RESULT_RE = re.compile(
    r"\b(?:when|once|after|as\s+soon\s+as|the\s+moment|while|until)\b[^.\n]{0,60}?"
    r"\b(?:it|that|they|its|their|the\s+\w+)\b"
    r"|\b(?:its|their)\s+(?:output|log|logs|result|results|findings?|verdict)\b"
    r"|\bwhatever\s+(?:it|they)\s+\w+"
    r"|\bif\s+(?:it|that|they)\s+\w+",
    re.IGNORECASE,
)
live = scan.live_background_work(events)
if live and (live.watcher or WAITS_ON_RESULT_RE.search(scrubbed)):
    sys.exit(0)

sys.stdout.write("PROMISE:" + promise)
PY
