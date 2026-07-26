#!/usr/bin/env python3
"""Live, ANSI-colored tail of a Claude Code dynamic-workflow run.

The /workflows TUI shows only a condensed tree. This renders the actual per-agent
activity of a workflow run -- text, tool calls, tool results, and the final
structured results -- interleaved and colorized, following live as the run writes.

A workflow run dir lives at:
  ~/.claude/projects/<proj>/<session>/subagents/workflows/wf_<id>/
containing journal.jsonl (workflow-level started/result events) and one
agent-<id>.jsonl (standard CC transcript) per sub-agent.

Usage:
  watch-workflow.py [RUNID|latest|/abs/run/dir] [--follow/--no-follow] [--full]
    RUNID    a wf_... id (resolved under the current project's sessions)
    latest   newest wf_* run dir under the project (default)
    --follow tail live (default); --no-follow render current content and exit
    --full   include assistant reasoning text (default: tool calls + results + results only-ish)
"""
import glob
import json
import os
import shutil
import sys
import textwrap
import time
import threading

# Default caps on how much of a command result / reasoning block to show. These are
# DISPLAY-only: the sub-agent always read the full result from its transcript; this only
# shortens the terminal view. Override with --result-lines N (0 = uncapped).
DEFAULT_RESULT_LINES = 24
MAX_TEXT_LINES = 10

PROJECTS_ROOT = os.path.expanduser("~/.claude/projects")

# --- ANSI ---
RESET = "\033[0m"
DIM = "\033[2m"
BOLD = "\033[1m"
RED = "\033[31m"
GREEN = "\033[32m"
YELLOW = "\033[33m"
CYAN = "\033[36m"
GREY = "\033[90m"
AGENT_PALETTE = [
    "\033[38;5;39m", "\033[38;5;208m", "\033[38;5;170m", "\033[38;5;76m",
    "\033[38;5;214m", "\033[38;5;45m", "\033[38;5;204m", "\033[38;5;149m",
]



def bounded_poll_wait(seconds: float) -> None:
    """Bounded loop pacing; loop predicates still own readiness/stop decisions."""
    threading.Event().wait(min(max(float(seconds), 0.0), 30.0))

def agent_color(agent_id):
    return AGENT_PALETTE[hash(agent_id) % len(AGENT_PALETTE)]


def term_width():
    return max(60, shutil.get_terminal_size((120, 40)).columns)


def clip(s, n):
    s = " ".join(str(s).split())
    return s if len(s) <= n else s[: n - 1] + "…"


def wrap_block(text, width, max_lines, pad, bar_color, body_color):
    """Wrap a multiline blob into readable lines behind a thin colored gutter bar.

    The BODY is drawn in `body_color` (empty string = terminal default fg, i.e. full
    contrast on any theme); only the `│` gutter is colored, so a multi-line block is
    visually grouped without dimming the text the user is trying to read. max_lines<=0
    = uncapped.
    """
    gutter = f"{pad}{bar_color}│{RESET} "
    avail = max(20, width - len(pad) - 2)
    raw = str(text).replace("\t", "  ").rstrip().splitlines() or [""]
    lines = []
    for ln in raw:
        ln = ln.rstrip()
        if not ln:
            lines.append("")
            continue
        lines.extend(textwrap.wrap(ln, avail, replace_whitespace=False, drop_whitespace=False) or [""])
    hidden = (len(lines) - max_lines) if max_lines and max_lines > 0 else 0
    if hidden > 0:
        lines = lines[:max_lines]
    out = [gutter + (f"{body_color}{ln}{RESET}" if body_color else ln) for ln in lines]
    if hidden > 0:
        out.append(f"{gutter}{DIM}… (+{hidden} more lines — rerun with --result-lines 0 for full){RESET}")
    return out


def find_run_dir(spec):
    """Resolve a run spec to a run dir path."""
    if spec and os.path.isdir(spec):
        return spec
    proj = None
    cwd_key = "-" + os.getcwd().strip("/").replace("/", "-")
    cand = os.path.join(PROJECTS_ROOT, cwd_key)
    if os.path.isdir(cand):
        proj = cand
    roots = [proj] if proj else glob.glob(os.path.join(PROJECTS_ROOT, "*"))
    runs = []
    for r in roots:
        runs += glob.glob(os.path.join(r, "*", "subagents", "workflows", "wf_*"))
    runs = [d for d in runs if os.path.isdir(d)]
    if spec and spec != "latest":
        hit = [d for d in runs if os.path.basename(d) == spec or spec in os.path.basename(d)]
        if hit:
            return max(hit, key=os.path.getmtime)
        print(f"no run matching {spec!r}; falling back to latest", file=sys.stderr)
    if not runs:
        return None
    return max(runs, key=os.path.getmtime)


def task_hint(path):
    """First readable line of an agent's opening prompt, to label the stream."""
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            for line in f:
                o = json.loads(line)
                if o.get("type") != "user":
                    continue
                c = (o.get("message") or {}).get("content")
                if isinstance(c, str):
                    txt = c
                elif isinstance(c, list):
                    txt = " ".join(b.get("text", "") for b in c if isinstance(b, dict) and b.get("type") == "text")
                else:
                    continue
                txt = txt.strip()
                if txt:
                    for ln in txt.splitlines():
                        if ln.strip():
                            return clip(ln.strip(), 90)
    except Exception:
        pass
    return ""


def brief_tool(inp):
    if not isinstance(inp, dict):
        return clip(inp, 100)
    for k in ("command", "file_path", "pattern", "description", "prompt", "query", "url", "skill"):
        if k in inp and inp[k]:
            return f"{k}={clip(inp[k], 110)}"
    return clip(json.dumps(inp), 100)


def render_agent_line(o, tag, color, w, full, result_lines):
    t = o.get("type")
    m = o.get("message") or {}
    out = []
    pre = f"{color}{tag}{RESET} "
    indent = " " * (len(tag) + 1)
    if t == "assistant" and isinstance(m.get("content"), list):
        for b in m["content"]:
            bt = b.get("type")
            if bt == "text" and full:
                txt = b.get("text", "").strip()
                if txt:
                    out.append(pre + f"{DIM}•{RESET}")
                    out += wrap_block(txt, w, MAX_TEXT_LINES, indent, color, "")
            elif bt == "tool_use":
                out.append(pre + f"{YELLOW}⚙ {b.get('name','?')}{RESET} {DIM}{clip(brief_tool(b.get('input')), w-len(tag)-16)}{RESET}")
    elif t == "user" and isinstance(m.get("content"), list):
        for b in m["content"]:
            if b.get("type") == "tool_result":
                cont = b.get("content")
                if isinstance(cont, list):
                    cont = "\n".join(x.get("text", "") for x in cont if isinstance(x, dict))
                err = bool(b.get("is_error"))
                bar = RED if err else color
                body = RED if err else ""  # "" = terminal default fg (full contrast)
                out.append(pre + f"{bar}└─ result{RESET}")
                out += wrap_block(cont, w, result_lines, indent, bar, body)
    return out


def main():
    argv = sys.argv[1:]
    follow = "--no-follow" not in argv
    full = "--full" in argv
    result_lines = DEFAULT_RESULT_LINES
    if "--result-lines" in argv:
        try:
            result_lines = int(argv[argv.index("--result-lines") + 1])
        except (ValueError, IndexError):
            pass
    positional = [a for a in argv if not a.startswith("--")]
    # Drop the value that follows --result-lines from positionals.
    if "--result-lines" in argv:
        val = argv[argv.index("--result-lines") + 1] if argv.index("--result-lines") + 1 < len(argv) else None
        positional = [p for p in positional if p != val]
    spec = positional[0] if positional else "latest"
    run = find_run_dir(spec)
    if not run:
        print("no workflow run dir found", file=sys.stderr)
        return 1
    w = term_width()
    # Self-title the tab/window (OSC 2) so it names the run even on the fallback paths.
    sys.stdout.write(f"\033]2;wf {os.path.basename(run)}\007")
    print(f"{BOLD}{CYAN}▶ watching workflow {os.path.basename(run)}{RESET}")
    print(f"{DIM}{run}{RESET}")
    cap_desc = "uncapped" if result_lines <= 0 else f"{result_lines} lines/result"
    print(f"{DIM}{'follow (Ctrl-C to exit)' if follow else 'snapshot'} · {'full' if full else 'tools+results'} · {cap_desc}{RESET}\n")

    offsets = {}
    labeled = set()
    started, resulted = set(), set()
    tags = {}

    def process():
        files = sorted(glob.glob(os.path.join(run, "agent-*.jsonl"))) + [os.path.join(run, "journal.jsonl")]
        for fp in files:
            if not os.path.exists(fp):
                continue
            is_journal = fp.endswith("journal.jsonl")
            aid = None if is_journal else os.path.basename(fp)[len("agent-"):-len(".jsonl")]
            if aid and aid not in tags:
                tags[aid] = f"[{aid[:6]}]"
            if aid and aid not in labeled:
                hint = task_hint(fp)
                labeled.add(aid)
                print(f"{agent_color(aid)}{BOLD}▂ {tags[aid]} {RESET}{agent_color(aid)}{hint}{RESET}")
            size = os.path.getsize(fp)
            off = offsets.get(fp, 0)
            if off > size:
                off = 0
            with open(fp, encoding="utf-8", errors="replace") as f:
                f.seek(off)
                chunk = f.read()
                offsets[fp] = f.tell()
            lines = chunk.split("\n")
            if not chunk.endswith("\n") and lines:
                # keep the partial last line for next poll
                offsets[fp] -= len(lines[-1].encode("utf-8", "replace"))
                lines = lines[:-1]
            for line in lines:
                line = line.strip()
                if not line:
                    continue
                try:
                    o = json.loads(line)
                except Exception:
                    continue
                if is_journal:
                    jt = o.get("type")
                    ja = o.get("agentId", "")
                    jtag = tags.get(ja, f"[{ja[:6]}]")
                    if jt == "started":
                        started.add(ja)
                        print(f"{GREEN}▶ {jtag} started{RESET}")
                    elif jt == "result":
                        resulted.add(ja)
                        res = o.get("result")
                        summ = ""
                        if isinstance(res, dict):
                            summ = res.get("summary") or json.dumps(res)
                        elif isinstance(res, str):
                            summ = res
                        print(f"{GREEN}{BOLD}✔ {jtag} RESULT{RESET} {GREEN}{clip(summ, w-16)}{RESET}")
                else:
                    for out in render_agent_line(o, tags[aid], agent_color(aid), w, full, result_lines):
                        print(out)
        if started and resulted >= started and follow:
            # all started agents have results -> run complete
            return True
        return False

    try:
        done = process()
        while follow and not done:
            bounded_poll_wait(0.6)
            done = process()
        if done:
            print(f"\n{BOLD}{GREEN}═══ RUN COMPLETE ({len(resulted)} agents) ═══{RESET}")
    except KeyboardInterrupt:
        print(f"\n{DIM}(detached; run keeps going){RESET}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
