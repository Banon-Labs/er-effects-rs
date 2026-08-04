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
# WHAT COUNTS AS "TRACKED"
# ------------------------
# `git ls-files --error-unmatch`. Untracked scratch files, build output and logs do NOT make a
# run stale -- only committed source does. That keeps the sentinel from killing a run because a
# log line was written, while still catching every real source edit.
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
GAME_COMMS=("eldenring.exe" "me3" "start_protected_game.exe")

list_live() {
  python3 - "${GAME_COMMS[@]}" <<'PY'
import os, sys, glob
names = set(sys.argv[1:])
for d in glob.glob('/proc/[0-9]*'):
    try:
        comm = open(os.path.join(d, 'comm')).read().strip()
    except OSError:
        continue
    if comm in names:
        print(os.path.basename(d), comm)
PY
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
    read -t 1 </dev/null 2>/dev/null || true
  done
  pids="$(list_live | awk '{print $1}')"
  if [[ -n "$pids" ]]; then
    # shellcheck disable=SC2086
    kill -KILL $pids 2>/dev/null || true
    for _ in 1 2 3 4 5 6; do
      [[ -z "$(list_live)" ]] && break
      read -t 1 </dev/null 2>/dev/null || true
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
  git -C "$REPO_ROOT" ls-files --error-unmatch -- "$abs" >/dev/null 2>&1
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
    echo "$live" | sed 's/^/[er-sentinel]   /' >&2
  else
    echo "[er-sentinel] FAILED to tear down the stale run after '$rel' was edited:" >&2
    list_live | sed 's/^/[er-sentinel]   /' >&2
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

selftest() {
  local fails=0
  # A tracked file is recognised.
  if is_tracked "$REPO_ROOT/AGENTS.md"; then echo "  ok   tracked file recognised"; else
    echo "  FAIL tracked file not recognised"; fails=$((fails + 1)); fi
  # An untracked path is not.
  local tmp="$REPO_ROOT/.er-sentinel-selftest-untracked"
  : >"$tmp"
  if is_tracked "$tmp"; then echo "  FAIL untracked file treated as tracked"; fails=$((fails + 1));
  else echo "  ok   untracked file ignored"; fi
  rm -f "$tmp"
  # A path outside the repo is not.
  if is_tracked "/etc/hostname"; then echo "  FAIL outside-repo path treated as tracked"; fails=$((fails + 1));
  else echo "  ok   outside-repo path ignored"; fi
  # An empty path is not.
  if is_tracked ""; then echo "  FAIL empty path treated as tracked"; fails=$((fails + 1));
  else echo "  ok   empty path ignored"; fi
  # Hook mode tolerates junk without erroring.
  if printf 'not json' | cmd_hook >/dev/null 2>&1; then echo "  ok   hook tolerates non-JSON stdin";
  else echo "  FAIL hook errored on non-JSON stdin"; fails=$((fails + 1)); fi
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
      echo "[er-sentinel] LIVE:"; echo "$live" | sed 's/^/[er-sentinel]   /'; exit 1; fi ;;
  --selftest) selftest ;;
  hook) cmd_hook ;;
  *) echo "usage: $0 {check <path>|teardown|status|--selftest|hook}" >&2; exit 2 ;;
esac
