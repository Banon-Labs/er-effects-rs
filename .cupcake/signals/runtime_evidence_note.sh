#!/usr/bin/env bash
# Cupcake signal: runtime_evidence_note
#
# Consumed by:
#   * git_require_runtime_evidence (PreToolUse/Bash): refuses `git push` when the commits being
#     pushed change code that runs inside ELDEN RING and no run has produced evidence since.
#
# Why this exists
#
# 2026-09-09: two commits were authored and a push attempted for both -- an F3 key that had never
# been pressed in the game, and a stall-watchdog fix whose code had never executed, because the DLL
# in the running process had been built before either. The user stopped the push and asked for this
# guard by name. AGENTS.md already says to commit only after a runtime validation run completes;
# that rule is prose, and prose did not stop it. This does.
#
# What it emits, one line:
#
#   one line of prose naming what ran, for the denial message. Never branched on.
#
# The verdict lives in the sibling signal `runtime_evidence_for_head`, which emits one of
# `OK`, `MISSING`, `NOTRUNTIME` or `UNKNOWN`. Splitting them keeps every comparison in the policy a
# string equality against a word, and leaves the sentence a human reads free to change without
# touching a rule.
#
# What decides the answer is the sha a DLL log names on its own `build git=` line, never a
# timestamp. The first version compared mtimes and answered `OK` on a log written by a build two
# commits old that happened to still be running -- newer file, older code -- which is the exact
# failure being guarded, made inside the guard.
#
# Safe to run on every Bash call: three git reads and one directory stat, no network, no writes.
set -uo pipefail

# The regression tests drive the policy through this, the same way the branch guards use
# CUPCAKE_CURRENT_BRANCH_OVERRIDE.
if [ -n "${CUPCAKE_RUNTIME_EVIDENCE_NOTE_OVERRIDE:-}" ]; then
  printf '%s' "$CUPCAKE_RUNTIME_EVIDENCE_NOTE_OVERRIDE"
  exit 0
fi

command -v git >/dev/null 2>&1 || exit 0
git rev-parse --git-dir >/dev/null 2>&1 || exit 0

head_sha="$(git rev-parse --short HEAD 2>/dev/null)" || exit 0
head_epoch="$(git log -1 --format=%ct HEAD 2>/dev/null)" || exit 0
[ -n "$head_epoch" ] || exit 0

# Which commits are about to go out. `origin/main` is the merge base for every branch in this repo;
# when it is unknown, fall back to the tip alone rather than guessing a range.
if git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null 2>&1; then
  changed="$(git diff --name-only refs/remotes/origin/main...HEAD 2>/dev/null)"
else
  changed="$(git show --name-only --format= HEAD 2>/dev/null)"
fi

# Only code that ends up inside the game can be proven by a run. A push that moves docs, scripts or
# policies has nothing to demonstrate and must not be blocked -- a guard that fires on everything is
# one the next agent learns to override by reflex.
if ! printf '%s\n' "$changed" | grep -q '^crates/'; then
  printf 'no crate under crates/ changed since origin/main'
  exit 0
fi

run_root="${ER_ME3_RUN_ROOT:-$HOME/.cache/er-me3-runs}"
if [ ! -d "$run_root" ]; then
  printf 'no run root at %s' "$run_root"
  exit 0
fi

# A DLL log names the commit it was built from on its first line:
#
#   build git=b6b459560dfa module=er_invasion_warp.dll base=0x... pe=0x... (2026-09-09T02:05:18Z)
#
# That sha, not the file's mtime, is what ties a run to code. The first version of this signal
# compared mtimes and immediately answered OK for head 0e084240 on a log whose own first line read
# `build git=b6b459560dfa` -- a DLL two commits older, still running and still writing, so its log
# was newer than the commit it could not possibly have executed. Reading a clock and calling it
# provenance is the same mistake this guard exists to stop, made inside the guard.
#
# `+dirty` disqualifies the run as well. It means the tree carried uncommitted changes when that DLL
# was built, so the binary is not the commit even when the sha matches.
python3 - "$run_root" "$head_sha" "$head_epoch" <<'PY'
import pathlib
import re
import sys

run_root = pathlib.Path(sys.argv[1])
head_sha = sys.argv[2]
head_epoch = int(sys.argv[3])

BUILD_LINE = re.compile(r"^build git=([0-9a-f]+)(\+dirty)?\b")

newest_epoch = 0
newest_run = "-"
note = "no DLL log under any run directory"
matched = None

for run in sorted(run_root.iterdir()):
    if not run.is_dir():
        continue
    for artifact in run.glob("er-*.log"):
        try:
            mtime = int(artifact.stat().st_mtime)
            with artifact.open(encoding="utf-8", errors="replace") as handle:
                first = handle.readline()
        except OSError:
            continue
        found = BUILD_LINE.match(first)
        if not found:
            continue
        built, dirty = found.group(1), bool(found.group(2))
        if mtime > newest_epoch:
            newest_epoch, newest_run = mtime, run.name
            if dirty:
                note = f"{artifact.name} was built from a DIRTY tree at {built[:8]}"
            elif not (built.startswith(head_sha) or head_sha.startswith(built)):
                note = f"{artifact.name} was built from {built[:8]}, not {head_sha}"
            else:
                note = f"{artifact.name} was built from {head_sha} and ran"
        if not dirty and (built.startswith(head_sha) or head_sha.startswith(built)):
            matched = (run.name, mtime, artifact.name)

if matched:
    run_name, mtime, name = matched
    print(
        f"{matched[2]} was built from {head_sha} and ran", end="",
    )
else:
    print(
        note, end="",
    )
PY
