#!/usr/bin/env bash
# Small-step Elden Ring window-placement smoke.
# Usage:
#   scripts/smoke-er-window-placement.sh preflight
#   scripts/smoke-er-window-placement.sh launch [env-file]
#   scripts/smoke-er-window-placement.sh observe [artifact-dir]
#   scripts/smoke-er-window-placement.sh teardown [artifact-dir]
#   scripts/smoke-er-window-placement.sh summarize [artifact-dir]
set -euo pipefail

REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
DEFAULT_ENV_FILE="$REPO_ROOT/.envs/title-resource-embedded-golden-onscreen.env"
LATEST_LINK="$REPO_ROOT/target/runtime-probe/no-move-window-smoke-latest"
WINDOW_CLASS=steam_app_1245620

usage() {
  sed -n '1,12p' "$0" | sed 's/^# \{0,1\}//'
}

artifact_dir() {
  if [[ -n "${1:-}" ]]; then
    printf '%s\n' "$1"
  elif [[ -e "$LATEST_LINK" ]]; then
    readlink -f "$LATEST_LINK"
  else
    echo "missing artifact dir and no latest link" >&2
    exit 2
  fi
}

preflight() {
  cd "$REPO_ROOT"
  pgrep -x steam >/dev/null || { echo steam=missing; exit 1; }
  echo steam=running
  if pgrep -x eldenring.exe >/dev/null; then
    echo eldenring.exe=already-running
    exit 1
  fi
  echo eldenring.exe=clear
  local cfg="$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/GraphicsConfig.xml"
  [[ -f "$cfg" ]] || { echo "appdata_graphics_config=missing:$cfg"; exit 1; }
  echo "appdata_graphics_config=$cfg"
}

launch() {
  cd "$REPO_ROOT"
  local env_file="${1:-$DEFAULT_ENV_FILE}"
  local art="$REPO_ROOT/target/runtime-probe/no-move-window-smoke-$(date +%Y%m%d-%H%M%S)"
  mkdir -p "$(dirname "$LATEST_LINK")"
  rm -f "$LATEST_LINK"
  ln -s "$art" "$LATEST_LINK"
  echo "artifact=$art"
  echo "latest=$LATEST_LINK"
  echo "teardown_deadline=$(date -d '+90 seconds' '+%Y-%m-%d %H:%M:%S %Z')"
  ARTIFACT_DIR="$art" bash scripts/record-title-gfx-proof-wf.sh 20 4 "$env_file"
}

observe() {
  local art
  art=$(artifact_dir "${1:-}")
  local pid_file="$art/capture-and-teardown.pid"
  local out="$art/target-window-observer.jsonl"
  [[ -f "$pid_file" ]] || { echo "missing capture pid: $pid_file" >&2; exit 2; }
  local pid
  pid=$(<"$pid_file")
  echo "observing=$art"
  echo "capture_pid=$pid"
  while kill -0 "$pid" 2>/dev/null; do
    hyprctl clients -j | jq -c --arg cls "$WINDOW_CLASS" '{t:now, windows:[.[] | select(.class==$cls) | {address,class,at,size,mapped,hidden,focusHistoryID,fullscreen,monitor,floating,pid,workspace:{id:.workspace.id,name:.workspace.name}}]}' >> "$out"
    python3 -c 'import select; select.select([], [], [], 0.25)'
  done
  echo "observe_done=$out"
}

teardown() {
  local art
  art=$(artifact_dir "${1:-}")
  echo "teardown_artifact=$art"
  mapfile -t pids < <(pgrep -x eldenring.exe || true)
  printf 'teardown_pids'; printf ' %s' "${pids[@]}"; printf '\n'
  ((${#pids[@]} == 0)) && { echo teardown_done; return 0; }
  kill "${pids[@]}" || true
  for pid in "${pids[@]}"; do timeout 5 tail --pid="$pid" -f /dev/null || true; done
  if pgrep -x eldenring.exe >/dev/null; then
    pkill -9 -x eldenring.exe || true
    echo teardown_forced
  else
    echo teardown_done
  fi
}

summarize() {
  cd "$REPO_ROOT"
  scripts/summarize-er-window-placement-smoke.py "$(artifact_dir "${1:-}")"
}

case "${1:-}" in
  preflight) shift; preflight "$@" ;;
  launch) shift; launch "$@" ;;
  observe) shift; observe "$@" ;;
  teardown) shift; teardown "$@" ;;
  summarize) shift; summarize "$@" ;;
  artifact) artifact_dir "${2:-}" ;;
  -h|--help|help|"") usage ;;
  *) usage >&2; exit 2 ;;
esac
