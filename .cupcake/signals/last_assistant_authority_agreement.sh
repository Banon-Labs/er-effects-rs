#!/usr/bin/env bash
# Cupcake signal: last_assistant_authority_agreement
#
# Outputs the banned authority-coded-agreement phrase found in the MOST RECENT assistant text
# message of the current session transcript, or empty if none. Consumed by the Stop-event policy
# `no_authority_agreement`, which halts turn-end (forcing a correction + bd memory) when the phrase
# slipped through. Fail-open (empty output) on any error so a transcript hiccup cannot wedge the
# session -- the every-turn add_context reminder is the always-on reinforcement.
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

# Last assistant message that actually contains text (skip tool-only turns).
last_text = ""
try:
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if ev.get("type") != "assistant":
                continue
            for block in ev.get("message", {}).get("content", []) or []:
                if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
                    last_text = block["text"]
except OSError:
    sys.exit(0)

# Targeted to real authority-coded AGREEMENT, not incidental words ("the correct offset").
pat = re.compile(
    r"\b(you'?re\s+right|you\s+are\s+right|that'?s\s+right|you'?re\s+correct|you\s+are\s+correct)\b"
    r"|(?:^|[.!?]\s+|\n)\s*(correct|exactly|absolutely|precisely)[,.! ]",
    re.IGNORECASE | re.MULTILINE,
)
m = pat.search(last_text or "")
if m:
    sys.stdout.write(m.group(0).strip())
PY
