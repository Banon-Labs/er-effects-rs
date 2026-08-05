#!/usr/bin/env bash
# Tear down a live Elden Ring run the moment a TRACKED source file in this repo is edited.
#
# WHY
# ---
# A live run has DLLs loaded from a particular state of this tree. The instant a tracked file
# changes, that run is STALE: its loaded code no longer matches the source, so anything observed
# in it is evidence about a build that no longer exists. Two concrete ways that bit us:
#
#   * a newly built DLL "validated" inside a process that had loaded the previous one;
#   * two live processes appending to the SAME log files next to the game exe, so one run's
#     lines land in the other's evidence -- and a per-process counter reading "#2" gets
#     misread as one process doing something twice.
#
# Advisory rules did not prevent either. AGENTS.md said to tear down; bd memories said to tear
# down; it kept happening, because an advisory rule only fires if it is recalled at the right
# moment. This sentinel does not rely on anyone remembering: it runs from the PostToolUse hook
# that already fires on Edit/MultiEdit/Write, and kills the stale run automatically.
#
# WHAT COUNTS AS SOURCE
# ---------------------
# A path inside the repo that git does NOT ignore. Build output, logs and scratch files are
# gitignored and so do not make a run stale; that is what stops the sentinel killing a run because
# a log line was written.
#
# It deliberately does NOT use `git ls-files --error-unmatch` alone. That only matches files
# already committed, so CREATING a new source file -- which invalidates a loaded DLL exactly as
# much as editing an existing one -- slipped straight through: a new `.rs` is untracked until it
# is `git add`ed, the check returned false, and a live run was left running against a tree that
# had already changed. Observed 2026-08-04, on the very first edit after the sentinel was written.
# `git check-ignore` answers the question that was actually meant: is this a file the repo cares
# about, tracked yet or not.
#
# USAGE
#   scripts/er-stale-run-sentinel.sh check <path>     # tear down if <path> is tracked
#   scripts/er-stale-run-sentinel.sh teardown         # unconditional teardown + verify
#   scripts/er-stale-run-sentinel.sh status
#   scripts/er-stale-run-sentinel.sh --selftest
#
# As a hook it reads the tool payload as JSON on stdin and extracts the edited path itself.
set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# Matched EXACTLY against /proc/<pid>/comm -- never a substring match on the full command line,
# which is how `rsi`-matches-`version` style false positives happen.
#
# The kernel caps comm at TASK_COMM_LEN-1 = 15 characters, so a longer executable name NEVER
# appears in full. `start_protected_game.exe` is 24 characters: comparing it verbatim could not
# match any process that has ever existed, so that entry silently protected nothing. Each name is
# therefore compared against its 15-character truncation as well.
GAME_COMMS=("eldenring.exe" "me3" "start_protected_game.exe")

list_live() {
  python3 - "${GAME_COMMS[@]}" <<'PY'
import os, sys, glob

# TASK_COMM_LEN - 1. A name longer than this is stored truncated, so match both forms.
COMM_MAX = 15
names = set()
for raw in sys.argv[1:]:
    names.add(raw)
    names.add(raw[:COMM_MAX])
for d in glob.glob('/proc/[0-9]*'):
    try:
        comm = open(os.path.join(d, 'comm')).read().strip()
    except OSError:
        continue
    if comm in names:
        print(os.path.basename(d), comm)
PY
}

# Prefix each line of stdin with the sentinel tag. A shell loop rather than `sed` so the script
# stays free of shellcheck findings -- it is itself gated by scripts/check.sh, and a gate that
# tolerates its own noise stops being read.
prefix_lines() {
  local line
  while IFS= read -r line; do
    printf '[er-sentinel]   %s\n' "$line"
  done
}

# Escalating teardown, then VERIFY. "I sent a signal" is not "it is gone", and every
# contamination so far came from assuming it was.
teardown() {
  local pids
  pids="$(list_live | awk '{print $1}')"
  if [[ -z "$pids" ]]; then
    return 0
  fi
  # shellcheck disable=SC2086
  kill -TERM $pids 2>/dev/null || true
  for _ in 1 2 3 4 5 6; do
    [[ -z "$(list_live)" ]] && break
    read -r -t 1 </dev/null 2>/dev/null || true
  done
  pids="$(list_live | awk '{print $1}')"
  if [[ -n "$pids" ]]; then
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
    for _ in 1 2 3 4 5 6; do
      [[ -z "$(list_live)" ]] && break
      read -r -t 1 </dev/null 2>/dev/null || true
    done
  fi
  [[ -z "$(list_live)" ]]
}

is_tracked() {
  local path="$1"
  [[ -n "$path" ]] || return 1
  # Resolve to a repo-relative path; anything outside the repo is not our source.
  local abs
  abs="$(cd -- "$(dirname -- "$path")" 2>/dev/null && pwd)/$(basename -- "$path")" || return 1
  case "$abs" in
    "$REPO_ROOT"/*) ;;
    *) return 1 ;;
  esac
  # Already committed -> unambiguously source.
  if git -C "$REPO_ROOT" ls-files --error-unmatch -- "$abs" >/dev/null 2>&1; then
    return 0
  fi
  # Not committed yet. A BRAND NEW source file is still source -- it invalidates the loaded DLLs
  # just as much as an edit to a committed one. The discriminator is whether git ignores it:
  # `check-ignore` exits 0 when the path IS ignored (target/, logs, scratch), which is exactly the
  # set that must NOT trigger a teardown.
  if git -C "$REPO_ROOT" check-ignore -q -- "$abs" 2>/dev/null; then
    return 1
  fi
  return 0
}

cmd_check() {
  local path="${1:-}"
  if ! is_tracked "$path"; then
    exit 0
  fi
  local live
  live="$(list_live)"
  if [[ -z "$live" ]]; then
    exit 0
  fi
  local rel="${path#"$REPO_ROOT"/}"
  if teardown; then
    echo "[er-sentinel] TORE DOWN the live Elden Ring run: '$rel' was edited, so the loaded" >&2
    echo "[er-sentinel] DLLs no longer match this tree and anything observed in that run would" >&2
    echo "[er-sentinel] be evidence about a build that no longer exists. Rebuild and relaunch." >&2
    echo "[er-sentinel] Killed:" >&2
    echo "$live" | prefix_lines >&2
  else
    echo "[er-sentinel] FAILED to tear down the stale run after '$rel' was edited:" >&2
    list_live | prefix_lines >&2
    exit 1
  fi
  exit 0
}

# Hook mode: Claude Code passes the tool payload as JSON on stdin.
cmd_hook() {
  local payload path
  payload="$(cat 2>/dev/null || true)"
  [[ -n "$payload" ]] || exit 0
  path="$(printf '%s' "$payload" | python3 -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    sys.exit(0)
ti = d.get("tool_input") or {}
for key in ("file_path", "notebook_path", "path"):
    v = ti.get(key)
    if isinstance(v, str) and v:
        print(v)
        break
' 2>/dev/null || true)"
  [[ -n "$path" ]] || exit 0
  cmd_check "$path"
}

# SC2317 ("command appears unreachable") fires for everything after the `cmd_hook` probe below.
# The analysis reasons that `cmd_hook` always `exit`s -- which it does -- and concludes the rest
# of this function is dead. It is not: `cmd_hook` is invoked on the right-hand side of a PIPELINE,
# so it runs in a subshell and its exit terminates only that subshell. The checker does not model
# that, so the finding is a false positive on every line here.
# shellcheck disable=SC2317
selftest() {
  local fails=0
  # A tracked file is recognised.
  if is_tracked "$REPO_ROOT/AGENTS.md"; then echo "  ok   tracked file recognised"; else
    echo "  FAIL tracked file not recognised"; fails=$((fails + 1)); fi
  # A BRAND NEW source file counts, even though git has never heard of it. This is the case that
  # actually escaped on 2026-08-04: the first file created after the sentinel was written was a
  # new `.rs`, `ls-files --error-unmatch` said no, and a live run survived an edit it should not
  # have. Creating source invalidates a loaded DLL exactly as much as editing source.
  local newsrc="$REPO_ROOT/crates/.er-sentinel-selftest-new-source.rs"
  : >"$newsrc"
  if is_tracked "$newsrc"; then echo "  ok   brand-new untracked source file counts as source";
  else echo "  FAIL new source file not recognised (the 2026-08-04 escape)"; fails=$((fails + 1)); fi
  rm -f "$newsrc"
  # A gitignored path does NOT count -- this is what keeps a log line or a build artefact from
  # killing a run. `target/` is ignored in this repo.
  local ignored="$REPO_ROOT/target/.er-sentinel-selftest-ignored"
  mkdir -p "$REPO_ROOT/target" 2>/dev/null || true
  : >"$ignored"
  if is_tracked "$ignored"; then echo "  FAIL gitignored path treated as source"; fails=$((fails + 1));
  else echo "  ok   gitignored path ignored"; fi
  rm -f "$ignored"
  # A path outside the repo is not.
  if is_tracked "/etc/hostname"; then echo "  FAIL outside-repo path treated as tracked"; fails=$((fails + 1));
  else echo "  ok   outside-repo path ignored"; fi
  # An empty path is not.
  if is_tracked ""; then echo "  FAIL empty path treated as tracked"; fails=$((fails + 1));
  else echo "  ok   empty path ignored"; fi
  # Hook mode tolerates junk without erroring.
  if printf 'not json' | cmd_hook >/dev/null 2>&1; then echo "  ok   hook tolerates non-JSON stdin";
  else echo "  FAIL hook errored on non-JSON stdin"; fails=$((fails + 1)); fi
  # A name longer than the kernel's 15-char comm cap is still matched. Proven end to end against a
  # REAL process rather than by re-deriving the truncation in the test: a copy of /bin/sleep named
  # `start_protected_game.exe` gets comm `start_protected`, which the verbatim comparison this
  # replaced could never have matched. Detection only -- the process is killed by pid, never by
  # calling teardown, so this case cannot touch a live game.
  local longdir longbin longpid
  longdir="$(mktemp -d)"
  longbin="$longdir/start_protected_game.exe"
  if cp -f /bin/sleep "$longbin" 2>/dev/null; then
    "$longbin" 5 &
    longpid=$!
    read -r -t 1 </dev/null 2>/dev/null || true
    local seen=0 live_pid
    while read -r live_pid _; do
      [[ "$live_pid" == "$longpid" ]] && seen=1
    done < <(list_live)
    if [[ $seen -eq 1 ]]; then
      echo "  ok   over-length executable name matched despite comm truncation"
    else
      echo "  FAIL over-length executable name not matched (comm truncation regression)"
      fails=$((fails + 1))
    fi
    kill -KILL "$longpid" 2>/dev/null || true
    wait "$longpid" 2>/dev/null || true
  else
    echo "  note comm-truncation case skipped (could not stage a test binary)"
  fi
  rm -rf "$longdir"

  # Teardown with nothing running is a no-op success.
  if [[ -z "$(list_live)" ]] && teardown; then echo "  ok   teardown no-ops when nothing is live";
  else echo "  note teardown selftest skipped (a run is live)"; fi
  if [[ $fails -eq 0 ]]; then echo "selftest ok"; return 0; fi
  echo "selftest FAILED ($fails)"; return 1
}

case "${1:-hook}" in
  check) shift; cmd_check "${1:-}" ;;
  teardown) if teardown; then echo "[er-sentinel] teardown: verified clean"; else
      echo "[er-sentinel] teardown FAILED" >&2; exit 1; fi ;;
  status) live="$(list_live)"; if [[ -z "$live" ]]; then echo "[er-sentinel] no live run"; else
      echo "[er-sentinel] LIVE:"; echo "$live" | prefix_lines; exit 1; fi ;;
  --selftest) selftest ;;
  hook) cmd_hook ;;
  *) echo "usage: $0 {check <path>|teardown|status|--selftest|hook}" >&2; exit 2 ;;
esac
