#!/usr/bin/env bash
# shellcheck disable=SC2317
# Bounded attach-only smoke for er_better_refills.dll.
#
# This is an agent-owned runtime probe: it launches the approved direct/offline Elden Ring path
# through me3 with only the standalone better-refills DLL loaded, waits for the DLL's own log line
# proving the SetItemReplenishState hook installed, then tears down only the PIDs this run spawned.
# It does not use screenshots or visual state as an oracle.
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game}"
# shellcheck source=scripts/me3-launch-lib.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/me3-launch-lib.sh"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/better-refills-smoke-$(date +%Y%m%d-%H%M%S)}"
BUILT_DLL="${BUILT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_better_refills.dll}"
ME3_PROFILE="${ME3_PROFILE:-$ARTIFACT_DIR/better-refills.me3}"
PID_FILE="${PID_FILE:-$ARTIFACT_DIR/me3-launch.pid}"
GAME_LOG="${GAME_LOG:-$GAME_DIR/er-better-refills.log}"
ARTIFACT_LOG="${ARTIFACT_LOG:-$ARTIFACT_DIR/er-better-refills.log}"
MAX_WAIT_SECONDS="${MAX_WAIT_SECONDS:-90}"

fatal() {
  echo "better-refills-smoke: $*" >&2
  exit 1
}

runtime_pids() {
  local proc pid comm cmdline
  for proc in /proc/[0-9]*; do
    pid=${proc##*/}
    [[ -r "$proc/comm" ]] || continue
    comm=$(<"$proc/comm")
    if [[ "$comm" == "eldenring.exe" ]]; then
      printf '%s\n' "$pid"
      continue
    fi
    [[ -r "$proc/cmdline" ]] || continue
    cmdline=$(tr '\0' ' ' < "$proc/cmdline" 2>/dev/null || true)
    if [[ "$cmdline" == *"$GAME_DIR/eldenring.exe"* ]]; then
      printf '%s\n' "$pid"
      continue
    fi
    if [[ "$cmdline" == *"ELDEN RING\\Game\\eldenring.exe"* ]]; then
      printf '%s\n' "$pid"
      continue
    fi
    if [[ "$cmdline" == *"windows-bin/me3-launcher.exe"* || "$cmdline" == *"windows-bin\\me3-launcher.exe"* ]]; then
      printf '%s\n' "$pid"
    fi
  done
}

win_pids_for() {
  command -v tasklist.exe >/dev/null 2>&1 || return 0
  tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
    python3 -c "import csv,sys; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}

PRE_ER_PIDS=" "
PRE_ME3_PIDS=" "

# shellcheck disable=SC2317 # invoked indirectly by cleanup trap.
terminate_runtime_pids() {
  local pid
  local -a pids=()
  mapfile -t pids < <(runtime_pids)
  for pid in "${pids[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  for pid in "${pids[@]}"; do
    [[ -n "$pid" ]] || continue
    timeout 6 tail --pid="$pid" -f /dev/null >/dev/null 2>&1 || true
  done
  mapfile -t pids < <(runtime_pids)
  for pid in "${pids[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
  for pid in $(win_pids_for eldenring.exe); do
    [[ "$PRE_ER_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
  done
  for pid in $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe); do
    [[ "$PRE_ME3_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
  done
}

# shellcheck disable=SC2317 # invoked by trap EXIT/INT/TERM/HUP.
cleanup() {
  local pid
  if [[ -s "$PID_FILE" ]]; then
    IFS= read -r pid < "$PID_FILE" || pid=""
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
  terminate_runtime_pids
  if [[ -f "$GAME_LOG" ]]; then
    cp -f "$GAME_LOG" "$ARTIFACT_LOG" || true
  fi
}
trap cleanup EXIT INT TERM HUP

wait_for_hook_active() {
  local deadline=$((SECONDS + MAX_WAIT_SECONDS))
  while ((SECONDS < deadline)); do
    if timeout 30 grep --line-buffered -m1 "SetItemReplenishState hook ACTIVE" < <(tail -n +1 -F -- "$GAME_LOG" 2>/dev/null) >"$ARTIFACT_DIR/hook-active.log"; then
      return 0
    fi
  done
  return 1
}

preflight() {
  [[ "$MAX_WAIT_SECONDS" =~ ^[0-9]+$ ]] || fatal "MAX_WAIT_SECONDS must be an integer"
  (( MAX_WAIT_SECONDS > 0 && MAX_WAIT_SECONDS <= 300 )) || fatal "MAX_WAIT_SECONDS must be 1..300"
  [[ -f "$BUILT_DLL" ]] || fatal "built DLL not found: $BUILT_DLL"
  [[ -d "$GAME_DIR" ]] || fatal "GAME_DIR not found: $GAME_DIR"
  bash "$REPO_ROOT/scripts/steam-running.sh" >/dev/null || fatal "Steam is not running according to scripts/steam-running.sh"
  me3_preflight
  me3_require_no_lazyloader "$GAME_DIR"
  if [[ -n "$(runtime_pids)" ]]; then
    fatal "Elden Ring/me3 runtime process already exists; refusing to take ownership"
  fi
}

preflight
PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "
mkdir -p "$ARTIFACT_DIR"
cp -f "$BUILT_DLL" "$ARTIFACT_DIR/er_better_refills.dll"
rm -f "$GAME_LOG" "$ARTIFACT_LOG"
me3_write_profile "$ME3_PROFILE" "$ARTIFACT_DIR/er_better_refills.dll"

echo "better-refills-smoke: ARTIFACT_DIR=$ARTIFACT_DIR"
echo "better-refills-smoke: ME3_PROFILE=$ME3_PROFILE"
echo "better-refills-smoke: DLL=$ARTIFACT_DIR/er_better_refills.dll"
echo "better-refills-smoke: waiting up to ${MAX_WAIT_SECONDS}s for hook ACTIVE log"

(me3_launch "$ME3_PROFILE" >"$ARTIFACT_DIR/me3-launch.stdout.log" 2>"$ARTIFACT_DIR/me3-launch.stderr.log") &
echo $! > "$PID_FILE"

if wait_for_hook_active; then
  cp -f "$GAME_LOG" "$ARTIFACT_LOG"
  echo "better-refills-smoke: PASS hook installed"
  echo "better-refills-smoke: log=$ARTIFACT_LOG"
  exit 0
fi

cp -f "$GAME_LOG" "$ARTIFACT_LOG" 2>/dev/null || true
echo "better-refills-smoke: FAIL hook-active log not observed within ${MAX_WAIT_SECONDS}s" >&2
echo "better-refills-smoke: artifacts=$ARTIFACT_DIR" >&2
exit 1
