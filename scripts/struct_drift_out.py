"""Where the struct-field-drift tools keep their derived caches.

Why this is not a literal in four scripts
-----------------------------------------
It was, and the literal contained a session UUID:

    /tmp/claude-1000/-home-banon-projects-er-mods-rs/f1b1f237-.../scratchpad/struct-drift

That path is correct for exactly one agent session and wrong for every other one, including every
future run by the same user -- a scratchpad directory is created per session. The next agent gets
a path that does not exist, so `detect-struct-field-drift.py` reports "no scan output; run --scan
first" and `clear-fields-by-object.py` reports "missing rtti-joined.tsv" -- both loud, but both
describing a missing cache rather than the real cause, which is that the tool is looking in
somebody else's directory.

Resolution order
  1. `$ER_STRUCT_DRIFT_OUT`, so a caller can still point the tools anywhere.
  2. The legacy session scratchpad, but only if it actually exists -- an agent that is mid-way
     through a scan keeps its cache instead of silently starting a new one somewhere else.
  3. `<repo>/target/struct-drift`, which is gitignored, current-user-owned and survives a reboot.

`--out-dir` still overrides all three on every tool that takes it.
"""
from __future__ import annotations

import os
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# The path the tools were originally written against. Kept as a fallback, not as a default, so an
# in-flight session does not lose its cache the day this module lands.
LEGACY = Path(
    "/tmp/claude-1000/-home-banon-projects-er-mods-rs/"
    "f1b1f237-c4a5-4649-9833-a40666da21bb/scratchpad/struct-drift"
)


def default_out() -> Path:
    override = os.environ.get("ER_STRUCT_DRIFT_OUT")
    if override:
        return Path(override)
    if LEGACY.is_dir():
        return LEGACY
    return REPO / "target" / "struct-drift"
