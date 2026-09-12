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
# Both signals now read `scripts/er-runtime-evidence.py`, which is the point: this file used to
# carry its own scan, and the two could disagree about which log the verdict came from. On
# 2026-09-11 a refusal named `er-quickload-autoload-debug.log` as "built from a DIRTY tree" -- the
# newest file by mtime, written by a module the sentence never mentioned -- and the agent reading
# it went hunting the wrong log. The sentence now names the module as well as the file, and comes
# from the same scan that produced the verdict.
#
# What decides the answer is the sha a DLL log names on its own `build git=` line, never a
# timestamp. The first version compared mtimes and answered `OK` on a log written by a build two
# commits old that happened to still be running -- newer file, older code -- which is the exact
# failure being guarded, made inside the guard.
#
# Safe to run on every Bash call: three git reads and a bounded directory scan, no network, no
# writes.
set -uo pipefail

# The regression tests drive the policy through this, the same way the branch guards use
# CUPCAKE_CURRENT_BRANCH_OVERRIDE.
if [ -n "${CUPCAKE_RUNTIME_EVIDENCE_NOTE_OVERRIDE:-}" ]; then
  printf '%s' "$CUPCAKE_RUNTIME_EVIDENCE_NOTE_OVERRIDE"
  exit 0
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." 2>/dev/null && pwd)" || exit 0

command -v git >/dev/null 2>&1 || exit 0
git rev-parse --git-dir >/dev/null 2>&1 || exit 0

head_sha="$(git rev-parse --short HEAD 2>/dev/null)" || exit 0

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

# One `note` line always comes back, whatever the verdict: it is the sentence, and the caller
# already has the verdict from the sibling signal.
printf '%s\n' "$changed" |
  python3 "$repo_root/scripts/er-runtime-evidence.py" --head "$head_sha" 2>/dev/null |
  sed -n 's/^note //p' |
  head -1 |
  tr -d '\n'
