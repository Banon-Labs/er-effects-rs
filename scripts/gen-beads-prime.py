#!/usr/bin/env python3
"""Generate a bounded `.beads/PRIME.md` for `bd prime`.

`bd prime` inlines every persistent memory body by default. With a large memory
store (1000+ memories) that output balloons to multiple MB (~650k tokens), which
overflows compaction (PreCompact hook) and bloats every session transcript.

This generator produces a bounded replacement: the base bd workflow context
followed by a *titles-only* memory index. Agents recall bodies on demand with
`bd recall <key>`. Because `.beads/PRIME.md` overrides `bd prime` output for
every caller, this bounds Claude, Pi, and Codex simultaneously.

Per-memory detail is tunable via BEADS_PRIME_MEM_CHARS:
  0 (default) -> titles only (smallest, ~18k tokens for ~1400 memories)
  N > 0       -> title + first N chars of the body (one line each)

The base context is taken from `bd prime --export`, which emits the default
content while *ignoring* any existing PRIME.md override (so regeneration never
feeds on its own bounded output).
"""
import json
import os
import subprocess
import sys

BD = os.environ.get("BD_REAL_BIN", "/home/choza/.local/bin/bd")
MEM_CHARS = int(os.environ.get("BEADS_PRIME_MEM_CHARS", "0") or "0")
MEMORY_MARKERS = ("## Persistent Memories", "## Memories")


def run(args):
    return subprocess.run(
        [BD, *args], capture_output=True, text=True, timeout=25
    ).stdout


def base_context():
    """bd's default prime text with the inlined-memory section stripped."""
    text = run(["prime", "--export"])
    for marker in MEMORY_MARKERS:
        i = text.find(marker)
        if i != -1:
            return text[:i].rstrip()
    return text.rstrip()


def memory_index():
    raw = run(["memories", "--json"])
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return 0, ["_(memory index unavailable)_"]
    if not isinstance(data, dict):
        return 0, ["_(memory index unavailable)_"]
    lines = []
    for key, body in data.items():
        if MEM_CHARS <= 0:
            lines.append(f"- {key}")
            continue
        s = body if isinstance(body, str) else json.dumps(body)
        s = s.split("\n", 1)[0][:MEM_CHARS]
        lines.append(f"- **{key}**: {s}")
    return len(data), lines


def main():
    base = base_context()
    count, index = memory_index()
    detail = "titles only" if MEM_CHARS <= 0 else f"title + first {MEM_CHARS} chars"
    out = [
        base,
        "",
        f"## Memory Index ({count}) — {detail}; run `bd recall <key>` for the full body",
        "",
        *index,
        "",
    ]
    sys.stdout.write("\n".join(out))


if __name__ == "__main__":
    main()
