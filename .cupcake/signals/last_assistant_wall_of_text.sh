#!/usr/bin/env bash
# Cupcake signal: last_assistant_wall_of_text
#
# Consumed by:
#   * wall_of_text (Stop): halts turn-end when the just-emitted assistant turn wrote MORE THAN ONE
#     paragraph of user-facing prose.
#
# WHY THIS EXISTS (user directive 2026-08-21, stated absolutely):
#   "I'll never. Repeat: NEVER read more than a single paragraph of response text from you.
#    Everything else is a skim."
# Every word past the first paragraph is not merely unwanted -- it is UNREAD. Prose the user skims
# is worse than no prose, because the agent believes it has communicated when it has not, and any
# caveat, blocker or correction buried past paragraph one has effectively been withheld.
#
# WHAT COUNTS AS A PARAGRAPH: blocks of prose separated by a blank line. Fenced code blocks, tables
# and list blocks are NOT counted -- they are scannable structure, not prose to read, and the user's
# objection is to reading. A single paragraph plus a code block or a table is fine; two prose
# paragraphs are not.
set -uo pipefail
python3 - <<'PY'
import glob, json, os, re, sys

cwd = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
tdir = os.path.join(os.path.expanduser("~/.claude/projects"), cwd.replace("/", "-"))
files = sorted(glob.glob(os.path.join(tdir, "*.jsonl")),
               key=lambda p: os.path.getmtime(p), reverse=True)
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


def assistant_text(ev):
    out = []
    for block in ev.get("message", {}).get("content", []) or []:
        if isinstance(block, dict) and block.get("type") == "text" and block.get("text"):
            out.append(block["text"])
    return "\n".join(out)


turns = [[]]
try:
    with open(files[0], encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
            except Exception:
                continue
            if is_real_user_prompt(ev):
                turns.append([])
            elif ev.get("type") == "assistant":
                t = assistant_text(ev)
                if t.strip():
                    turns[-1].append(t)
except OSError:
    sys.exit(0)

text = "\n\n".join(t for t in turns[-1] if t.strip())
if not text.strip():
    sys.exit(0)

# Strip fenced code, then classify each blank-line-delimited block. Tables and lists are structure.
text = re.sub(r"```.*?```", "", text, flags=re.DOTALL)
paragraphs = []
for block in re.split(r"\n\s*\n", text):
    block = block.strip()
    if not block:
        continue
    lines = [l.strip() for l in block.splitlines() if l.strip()]
    if not lines:
        continue
    if all(l.startswith("|") for l in lines):          # table
        continue
    if all(re.match(r"^([-*+]|\d+[.)])\s", l) for l in lines):  # list
        continue
    paragraphs.append(block)

if len(paragraphs) > 1:
    first = " ".join(paragraphs[0].split())[:60]
    print(f"WALLOFTEXT:{len(paragraphs)}:{first}")
PY
