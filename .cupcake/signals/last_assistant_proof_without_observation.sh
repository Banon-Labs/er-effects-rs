#!/usr/bin/env bash
# Cupcake signal: last_assistant_proof_without_observation
#
# Scans the most recently completed assistant turn and emits one facts line when that turn closed by
# calling something proven while citing nothing that watched it happen in the game.
#
#   PROOFFACTS|claim=<clause>|observed=<0|1>
#
# Emitted only when the word was used as a claim; a clean turn emits empty (fail-open).
#
# The failure this exists to refuse
# ---------------------------------
# User directive 2026-09-09, recorded as bd `proven-means-observed-in-the-real-game-not-a-boot-log-
# line-2026-09-09`. The turn opened "Both addresses now translate instead of refusing" off a single
# boot line reading `ADDRESS TRANSLATED (EQUIP_PARAM_GOODS_GET_ENTRY_RVA): 0x140d39df0 -> 0x140d3b5b0`.
# An address resolving says a constant maps to a function. It does not say one frame of the feature
# ran, and the two things the run was launched to show -- the short animation and the skipped popup
# -- had produced no line at all.
#
# The distinction the word has to carry, from that memory:
#   observed in the real game -- someone watched the animation, no popup appeared, the session state
#                                moved. This is the only thing "proven" may name.
#   measured in-process       -- an oracle read a value, a hook fired, a counter moved. That is
#                                "measured", and it names the oracle.
#   the harness ran           -- it compiled, it launched, an address resolved, a detour installed,
#                                a gate went green, a test passed. Say exactly that. A DLL that
#                                loads has shown that it loads and nothing else.
#
# So the signal is a conjunction of two facts, and the policy joins them:
#   claim    -- the closing prose used the word "proven" as a claim, outside a quotation.
#   observed -- the same turn cited a real-game observation: a `br-YYYYMMDD-HHMMSS-xxxx` run id, an
#               `er-me3-runs` artifact path, a named `oracle_` field, a screenshot or image
#               artifact, a pixel diff, or prose putting an observation verb next to the game or
#               the screen.
#
# Biased hard toward not firing, the same way its neighbours are
#   * one word only -- "proven". The measurement behind that choice is beside PROOF_RE below;
#   * only the closing prose run is scanned -- the word mid-turn, before more tool calls, is a
#     working note, not a claim delivered to anyone;
#   * fenced code, backtick spans and double-quoted spans are stripped first, so quoting the user,
#     quoting this file, or quoting a banner cannot trip it;
#   * a slug or path token is a name, not a claim: `proven-means-observed-in-the-real-game-...` and
#     `docs/proven.md` are skipped, while `runtime-proven` is not;
#   * a markdown table cell is skipped, since a cell most often holds a label;
#   * an attributive use is skipped -- "a proven exception" is an adjective meaning reliable;
#   * a negated, modal or reported occurrence is skipped outright -- "not proven", "unproven",
#     "can be proven", "stop me calling that proven", "what is proven and what isn't". Saying a
#     thing is unproven is the behaviour being asked for and must never be punished;
#   * the evidence half is searched over the whole turn's prose with nothing stripped, because a run
#     id most often arrives inside a fenced block, and a generous evidence search fails toward
#     silence.
#
# The shared half (transcript discovery, turn bucketing, prose runs) comes from
# `scripts/cupcake_turn_scan.py`, the same module the neighbouring signals use, so the guards cannot
# drift into disagreeing about the same turn. Fail-open (empty output) on any error.
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
turn = scan.last_text_turn(scan.split_turns(scan.load_events(path)))
if turn is None:
    sys.exit(0)

# A turn that kept going past its last prose did not end on that prose.
if turn.last_text_index < 0 or turn.tool_after(turn.last_text_index):
    sys.exit(0)


def scrub(text):
    text = re.sub(r"```.*?```", " ", text, flags=re.DOTALL)
    text = re.sub(r"`[^`]*`", " ", text)
    return re.sub(r'"[^"]*"', " ", text)


# --- (1) the closing prose called something proven ---------------------------------------------
# One word, chosen by measurement rather than by symmetry. Running this signal over the 3,707 real
# turns in ~/.claude/projects/-home-banon-projects-er-mods-rs/*.jsonl:
#
#   proven                          122 halts   3.3% of turns
#   proven + proved                 174 halts   4.7%
#   proven + proved + proves        233 halts   6.3%
#   the whole family, incl. `proof` 349 halts   9.4%
#
# The extra 227 are not the failure this guard is for. In this repo's register the noun and the
# active verb are ordinary reasoning vocabulary -- "an excused gate proves nothing", "the proof is
# in this session", "yesterday's RE proved that class is 960 bytes", "producing proof performs
# diligence". None of them call a runtime feature proven, and halting one turn in ten is how a guard
# becomes noise that gets routed around. The directive and bd `proven-means-observed-in-the-real-
# game-not-a-boot-log-line-2026-09-09` are both about the claim word, which is this one.
PROOF_RE = re.compile(r"\bproven\b", re.IGNORECASE)

# Written after the match, within the same sentence, and turning it into an admission of absence.
FORWARD_NEG = re.compile(r"\b(?:isn'?t|aren'?t|nothing|not\s+yet)\b", re.IGNORECASE)

# A determiner in front and a noun behind makes the word an adjective meaning "reliable", not a
# claim about a run: "a proven deserialize", "the proven part", "a proven exception", "a *proven*
# subset". The predicative forms the directive is about keep a verb in front instead -- "is proven",
# "was proven", "done and proven" -- so they are untouched by this.
DETERMINERS = {"a", "an", "the", "this", "that", "these", "those", "some", "any", "its", "our",
               "their", "your", "my", "more", "most", "less", "another", "each", "every"}

# Words that turn an occurrence into a disclaimer. Checked over the five tokens before the match,
# with punctuation and hyphens flattened to spaces so "has not been runtime-proven" is caught.
#
# Every entry has to be a word that cannot also be ordinary description within five tokens of the
# claim. "short" was in this set for one draft and silently swallowed "The short animation is
# proven", which is the exact sentence the guard exists to catch; "far" (as in "far from proven")
# carries the same collision and is left out for the same reason.
NEGATORS = {
    "not", "never", "no", "none", "nothing", "cannot", "cant", "isnt", "arent", "wasnt", "werent",
    "hasnt", "havent", "hadnt", "doesnt", "dont", "didnt", "wont", "without", "yet", "hardly",
    "barely", "nowhere", "lacks", "lacking", "absent", "neither", "nor",
    # Modal and reported forms. "can be proven", "would be proven", "stop me calling that proven",
    # "rather than proven from the binary" all discuss the word instead of claiming it. "be" is the
    # cheap discriminator and "been" deliberately is not one, so "has been proven" still counts.
    "be", "can", "could", "would", "should", "might", "may", "whether",
    "rather", "instead", "calling", "call", "deemed", "considered", "stop", "stops",
}

TOKEN_CHARS = set(
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_./-"
)


def whole_token(text, start, end):
    """The path/slug token the match sits inside, e.g. `proven` inside `docs/proven.md`.

    Trailing and leading separators are trimmed, because sentence punctuation is made of the same
    characters a path is. Without the trim, `proven.` at the end of a sentence reads as a path token
    and every closing claim in the corpus goes silently unjudged -- measured while writing this.
    """
    i = start
    while i > 0 and text[i - 1] in TOKEN_CHARS:
        i -= 1
    j = end
    while j < len(text) and text[j] in TOKEN_CHARS:
        j += 1
    return text[i:j].strip("./-_")


def line_of(text, pos):
    start = text.rfind("\n", 0, pos) + 1
    end = text.find("\n", pos)
    return text[start:] if end < 0 else text[start:end]


def countable(text, m):
    token = whole_token(text, m.start(), m.end())
    if any(c in token for c in "/._"):
        return False  # a path or an identifier is a name, not a claim
    if token.count("-") >= 2:
        return False  # a slug such as a bd memory key, or proof-of-concept
    if line_of(text, m.start()).lstrip().startswith("|"):
        return False  # a markdown table cell, most often the bare column label
    if text[m.end():m.end() + 12].lower().lstrip().startswith("of concept"):
        return False
    if FORWARD_NEG.search(text[m.end():m.end() + 40]):
        return False  # "what is proven and what isn't" is an honest split, not a claim
    window = re.sub(r"[^A-Za-z']+", " ", text[max(0, m.start() - 70):m.start()])
    tokens = [t.replace("'", "").lower() for t in window.split()][-5:]
    if tokens and tokens[-1] in DETERMINERS and re.match(r"[\s*_]+[A-Za-z]", text[m.end():m.end() + 4]):
        return False  # attributive: an adjective describing a thing, not a claim about a run
    return not any(t in NEGATORS for t in tokens)


def sentence_span(text, pos):
    start = 0
    for m in re.finditer(r"[.!?\n]", text[:pos]):
        start = m.end()
    end = len(text)
    for m in re.finditer(r"[.!?\n]", text[pos:]):
        end = pos + m.start() + 1
        break
    return start, end


def clause(text, m):
    start, end = sentence_span(text, m.start())
    collapsed = " ".join(text[start:end].split())
    if len(collapsed) <= 120:
        return collapsed.replace("|", "/")
    window_start = max(start, m.start() - 40)
    clipped = " ".join(text[window_start:window_start + 130].split())
    prefix = "..." if window_start > start else ""
    return (prefix + clipped + " ...").replace("|", "/")


runs = turn.text_runs
closing = scrub(runs[-1]) if runs else ""

hit = None
for m in PROOF_RE.finditer(closing):
    if countable(closing, m):
        hit = clause(closing, m)
if not hit:
    sys.exit(0)

# --- (2) did the turn cite something that watched it happen? -----------------------------------
# Searched over the whole turn's prose with nothing stripped: a run id usually arrives inside a
# fenced block, and this half is meant to fail toward silence.
OBSERVED_RE = re.compile(
    r"\bbr-\d{8}-\d{6}-[0-9a-f]{4}\b"                                    # a launcher run id
    r"|er-me3-runs"                                                      # the run artifact tree
    r"|\boracle_[a-z0-9_]+"                                              # a named in-process oracle
    r"|\bscreenshot\b"
    r"|\bpixel[-\s]?diff"
    r"|\.(?:jpe?g|png)\b"
    r"|\b(?:the\s+user|you)\s+(?:saw|watched|confirmed)\b"
    r"|\b(?:observed|watched|saw|seen|visible|appeared|rendered|displayed)\b[^.\n]{0,60}"
    r"\b(?:in[-\s]?game|on[-\s]screen|in\s+the\s+game|live\s+run)\b"
    r"|\b(?:in[-\s]?game|on[-\s]screen|in\s+the\s+game)\b[^.\n]{0,60}"
    r"\b(?:observed|watched|saw|seen|visible|appeared|rendered|displayed)\b",
    re.IGNORECASE,
)

observed = bool(OBSERVED_RE.search(turn.text))

print("PROOFFACTS|claim={}|observed={}".format(hit, int(observed)))
PY
