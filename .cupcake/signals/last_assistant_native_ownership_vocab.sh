#!/usr/bin/env bash
# Cupcake signal: last_assistant_native_ownership_vocab
#
# Scans the most recently COMPLETED assistant turn of the current session transcript and returns a
# tagged list of risky implementation-vocabulary hits, or empty if the turn is clean. Consumed by
# native_ownership_vocab_reminder.rego, which injects an ADVISORY/non-blocking reminder on the next
# prompt. The reminder is intentionally not a halt: the user asked for caution text sent back to the
# LLM, not a stop/address/pause requirement.
#
# This detects vocabulary that has repeatedly correlated with address-level steering instead of native
# job/queue ownership in this repo: pulse/pump/poke/manual per-frame writes/repeated direct field
# adjustment/broad ad-hoc state windows/address-level steering. It scans assistant prose and tool_use
# inputs from the previous turn so Edit/Write/Bash-introduced comments or code paths are caught too.
# Fail-open (empty output) on errors so a transcript hiccup cannot wedge the session.
set -uo pipefail
python3 - <<'PY' 2>/dev/null || true
import glob, json, os, re, sys

cwd = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
key = cwd.replace("/", "-")
tdir = os.path.join(os.path.expanduser("~/.claude/projects"), key)
files = sorted(glob.glob(os.path.join(tdir, "*.jsonl")), key=lambda p: os.path.getmtime(p), reverse=True)
if not files:
    sys.exit(0)


def is_real_user_prompt(ev):
    if ev.get("type") != "user":
        return False
    content = ev.get("message", {}).get("content")
    if isinstance(content, str):
        return content.strip() != ""
    if isinstance(content, list):
        for block in content:
            if isinstance(block, dict) and block.get("type") == "tool_result":
                return False
        return True
    return False


def assistant_turn_text(ev):
    out = []
    for block in ev.get("message", {}).get("content", []) or []:
        if not isinstance(block, dict):
            continue
        if block.get("type") == "text" and block.get("text"):
            # Strip double-quoted prose spans so quoting the rule itself does not count as using it.
            out.append(re.sub(r'"[^"\\]*(?:\\.[^"\\]*)*"', " ", block["text"]))
        elif block.get("type") == "tool_use":
            # Include tool input JSON so risky vocabulary inserted via Edit/Write/Bash is visible.
            # Do not scrub quotes here: code/comment strings in JSON are exactly what we need to scan.
            try:
                out.append(json.dumps(block.get("input") or {}, sort_keys=True, ensure_ascii=False))
            except Exception:
                pass
    return "\n".join(out)

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
                t = assistant_turn_text(ev)
                if t:
                    turns[-1].append(t)
except OSError:
    sys.exit(0)

last_turn = ""
for bucket in reversed(turns):
    if bucket:
        last_turn = "\n".join(bucket)
        break
if not last_turn:
    sys.exit(0)

scrubbed = last_turn

PATTERNS = [
    ("pulse", re.compile(r"\b(pulse|pulses|pulsed|pulsing)\b", re.IGNORECASE)),
    ("pump", re.compile(r"\b(pump|pumps|pumped|pumping)\b", re.IGNORECASE)),
    ("poke", re.compile(r"\b(poke|pokes|poked|poking)\b", re.IGNORECASE)),
    ("manual-per-frame", re.compile(r"\bmanual\s+(?:per[- ]frame|per\s+frame)\b|\bper[- ]frame\s+(?:manual\s+)?(?:write|writes|adjust|adjustment|steer|steering)\b", re.IGNORECASE)),
    ("direct-field-adjustment", re.compile(r"\b(?:direct|manual|repeated)\s+(?:field|address|memory)\s+(?:write|writes|adjustment|adjustments|steering|patch|patches)\b|\brepeated\s+direct\s+field\s+adjustment\b", re.IGNORECASE)),
    ("ad-hoc-state-window", re.compile(r"\b(?:broad|broaden(?:ed|ing)?|ad[- ]hoc)\s+(?:state\s+)?window(?:s)?\b|\bad[- ]hoc\s+state\b", re.IGNORECASE)),
    ("address-level-steering", re.compile(r"\baddress[- ]level\s+steering\b|\bstate\s+steering\b", re.IGNORECASE)),
]

hits = []
for label, pattern in PATTERNS:
    if pattern.search(scrubbed):
        hits.append(label)

if hits:
    sys.stdout.write("NATIVEVOCAB:" + ",".join(dict.fromkeys(hits)))
PY
