#!/usr/bin/env bash
# Cupcake signal: runtime_evidence_for_head
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
# What it emits, one line, one word:
#
#   `OK`         a run executed this code: a DLL log names the tip on its own `build git=` line,
#                or names a commit the tip adds no cargo work on top of.
#   `MISSING`    no run did. This is the case that denies.
#   `NOTRUNTIME` the commits about to be pushed touch no crate that ships in a DLL, so there is
#                nothing for a run to prove. Never denies.
#   `UNKNOWN`    the signal could not measure (no git, no log directory, unreadable). Never denies:
#                a guard that cannot see must not invent a verdict, and the pre-push hook plus
#                CI still stand behind it.
#
# One word with no fields, because the parsing is easier to selftest in bash than in rego, and the
# prose belongs to the sibling signal `runtime_evidence_note`, which the policy interpolates but
# never inspects. The header used to justify this differently -- that every parsing form tried in
# rego was inert -- and that was wrong: the policy was dead because of `sprintf`, which cupcake's
# WASM runtime does not implement, not because of anything to do with parsing or `input.signals`.
#
# Which logs count, and what a `+dirty` line means, is decided in one place for all three readers
# of this evidence: `scripts/er-runtime-evidence.py`. It reads the run root and the game directory
# both -- a run launched through `~/Elden/launch.sh` writes only into the second, and reading only
# the first refused a push on 2026-09-11 that a live run had proven. Its header carries the rest.
#
# What decides the answer is the sha in the log, never a timestamp. The first version compared
# mtimes and answered `OK` on a log written by a build two commits old that happened to still be
# running -- newer file, older code -- which is the exact failure being guarded, made inside the
# guard.
#
# Safe to run on every Bash call, and measured rather than asserted: ~0.1s warm, ~1.2s the first
# time a new tip is seen, no network. It writes nothing but its own memo under `XDG_RUNTIME_DIR`.
# The version that read the whole run root and walked the reverse-dependency graph once per log
# line took over 45 seconds, which one gate then paid 176 times.
set -uo pipefail

# The regression tests drive the policy through this, the same way the branch guards use
# CUPCAKE_CURRENT_BRANCH_OVERRIDE.
if [ -n "${CUPCAKE_RUNTIME_EVIDENCE_OVERRIDE:-}" ]; then
  printf '%s' "$CUPCAKE_RUNTIME_EVIDENCE_OVERRIDE"
  exit 0
fi

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." 2>/dev/null && pwd)" || exit 0

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
  printf 'NOTRUNTIME'
  exit 0
fi

# The changed paths go in on stdin because the scan needs them: a `+dirty` log is only accepted
# when the shell that wrote it compiles every crate this push changes.
evidence="$(printf '%s\n' "$changed" |
  python3 "$repo_root/scripts/er-runtime-evidence.py" --head "$head_sha" 2>/dev/null)"
status=$?

case "$status" in
0)
  printf 'OK'
  exit 0
  ;;
2)
  printf 'UNKNOWN'
  exit 0
  ;;
1) ;;
*)
  # The scanner itself failed. That is not a verdict.
  printf 'UNKNOWN'
  exit 0
  ;;
esac

# No log names this commit, but a run may still have executed the same code. The tip proves out
# when it adds nothing cargo would compile on top of a sha that ran -- otherwise this guard demands
# a rebuild and a relaunch to publish a shell script, and a guard that fires on everything is one
# the next agent overrides by reflex.
#
# scripts/er-change-scope.py answers it, the same reverse-dependency walk the compile gate uses:
# `--rust-touched` exits 3 for "provably no cargo work required" and 0 otherwise, and it fails open
# on a git failure, an unresolvable base, or any build input outside a single crate directory. So it
# can only forgive a diff that provably cannot change a DLL. It agrees with
# scripts/check-runtime-evidence.sh by construction: two enforcement points that answer differently
# about one push teach the next agent to ignore whichever is louder.
#
# Deduplicated and capped by the scanner, which the pre-push copy does not need to be. This signal
# runs on every single Bash tool call, and every attempt is a whole reverse-dependency walk: the
# first version tried one per matching log line, and the run root here holds hundreds of them across
# a dozen builds. It took over 45 seconds to answer, on a signal whose header promises three git
# reads and a stat. The newest builds are the only ones a live branch can carry forward from anyway
# -- an older sha reaches the tip across strictly more commits, so if the newest cannot forgive the
# diff, an older one cannot either.
# Memoised, because one gate charges this signal 176 times. `scripts/test-cupcake-policies.py`
# drives that many `cupcake eval` spawns and every one of them runs every signal, so a walk that
# costs half a second lands as minutes on the suite. The answer is a pure function of the two
# commit shas -- `--rev` reads the commit, not the working tree -- so it is safe to keep, and it is
# keyed by both shas so it cannot be served for a different pair.
# The candidate lines go in as an argument rather than on stdin: the heredoc below is itself the
# program, so a pipe into it would be swallowed (shellcheck SC2259) and every carry-forward would
# quietly find no candidates at all.
python3 - "$head_sha" "$repo_root" "$evidence" <<'PY'
import os
import pathlib
import subprocess
import sys

head_sha = sys.argv[1]
repo_root = pathlib.Path(sys.argv[2])

candidates = []
for line in sys.argv[3].splitlines():
    parts = line.split()
    if len(parts) >= 2 and parts[0] == "candidate" and parts[1] not in candidates:
        candidates.append(parts[1])

cache_dir = pathlib.Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "er-mods-rs-evidence"
try:
    cache_dir.mkdir(parents=True, exist_ok=True)
except OSError:
    cache_dir = None


# The roots a diff may be confined to and still carry forward, kept identical to
# scripts/check-runtime-evidence.sh. `er-change-scope.py` treats `.github/` as a build input and
# widens to everything, which is right for "what must CI re-run" and wrong for "could this change
# the DLL" -- a workflow file cannot end up inside one. All nine build scripts in the workspace
# were read for the paths they open and the processes they spawn, and none reaches any of these
# three; the only `Command::new` in any of them is `git`. `docs/` is deliberately absent, because
# `crates/er-game-base/build.rs` reads `docs/recon/*.tsv`.
SAFE_ROOTS = (".github/", ".cupcake/", "scripts/")


def confined_to_safe_roots(built):
    diff = subprocess.run(
        ["git", "diff", "--name-only", f"{built}..{head_sha}"],
        capture_output=True,
        text=True,
        check=False,
    )
    if diff.returncode != 0:
        return False
    paths = [line for line in diff.stdout.splitlines() if line]
    return bool(paths) and all(p.startswith(SAFE_ROOTS) for p in paths)


def adds_no_cargo_work(built):
    cached = cache_dir / f"{head_sha}-{built}" if cache_dir else None
    if cached is not None:
        try:
            return cached.read_text() == "3"
        except OSError:
            pass
    probe = subprocess.run(
        [
            "python3",
            str(repo_root / "scripts" / "er-change-scope.py"),
            "--rust-touched",
            "--base",
            built,
            "--rev",
            head_sha,
        ],
        capture_output=True,
        check=False,
    )
    verdict = probe.returncode == 3 or confined_to_safe_roots(built)
    if cached is not None:
        try:
            cached.write_text("3" if verdict else "0")
        except OSError:
            pass
    return verdict


for built in candidates:
    if adds_no_cargo_work(built):
        print("OK", end="")
        raise SystemExit(0)

print("MISSING", end="")
PY
