#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/product-continue-direct-$(date +%Y%m%d-%H%M%S)}"
PID_FILE="${PID_FILE:-$ARTIFACT_DIR/proton-run.pid}"
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/er-effects-telemetry.json}"
BOOTSTRAP_PATH="${BOOTSTRAP_PATH:-$ARTIFACT_DIR/bootstrap.jsonl}"
BOOTSTRAP_STATE_PATH="${BOOTSTRAP_STATE_PATH:-$ARTIFACT_DIR/bootstrap-state.json}"
AUTOLOAD_PATH="${AUTOLOAD_PATH:-$GAME_DIR/er-effects-autoload.txt}"
AUTOLOAD_REQUEST="${AUTOLOAD_REQUEST:-}"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-60}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
DRY_RUN=0

usage() {
  cat <<EOF
Usage: $0 [--dry-run] [--autoload-request PATH]

Launches the approved direct/offline eldenring.exe runtime path and runs
.auto/runtime_probe.sh as the bounded readiness watcher. This intentionally has
no Steam/AppID launch path and no protected launcher path.

Required for real execution:
  ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --autoload-request) AUTOLOAD_REQUEST="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

fatal() { echo "run-product-continue-direct-probe: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }
require_executable() { [[ -x "$1" ]] || fatal "missing executable: $1"; }

runtime_pids() {
  local proc pid comm cmdline
  for proc in /proc/[0-9]*; do
    pid=${proc##*/}
    [[ -r "$proc/comm" ]] || continue
    comm=$(<"$proc/comm")
    # The exact process name is specific to the game (not a broad pattern like "wine"), and
    # "eldenring.exe" (13 chars) is not /proc/comm-truncated. Match it directly so a
    # wine-reparented process whose cmdline no longer contains the install path is still found
    # and torn down -- the earlier cmdline-substring-only match leaked such processes.
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
    fi
  done
}

preflight() {
  [[ "$RUNTIME_TIMEOUT_SECONDS" =~ ^[0-9]+$ ]] || fatal "RUNTIME_TIMEOUT_SECONDS must be an integer"
  (( RUNTIME_TIMEOUT_SECONDS > 0 && RUNTIME_TIMEOUT_SECONDS <= 60 )) || fatal "RUNTIME_TIMEOUT_SECONDS must be 1..60"
  require_executable "$PROTON"
  require_file "$GAME_DIR/eldenring.exe"
  require_file "$REPO_ROOT/.auto/runtime_probe.sh"
  [[ -d "$STEAM_COMPAT_DATA_PATH" ]] || fatal "missing compatdata path: $STEAM_COMPAT_DATA_PATH"
  if [[ -n "$(runtime_pids)" ]]; then
    fatal "eldenring.exe is already running; refusing to mix probe ownership"
  fi
}

write_autoload_request() {
  if [[ -n "$AUTOLOAD_REQUEST" ]]; then
    require_file "$AUTOLOAD_REQUEST"
    cp -f "$AUTOLOAD_REQUEST" "$AUTOLOAD_PATH"
    cp -f "$AUTOLOAD_REQUEST" "$ARTIFACT_DIR/autoload-request.txt"
  fi
}

terminate_runtime_pids() {
  local pid
  local -a pids=()
  mapfile -t pids < <(runtime_pids)
  for pid in "${pids[@]}"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  # Deterministic, bounded wait for graceful exit (no sleep): block on each pid's exit with
  # `tail --pid`, hard-capped by a <=30s timeout. tail returns the instant the pid is gone.
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
}

cleanup() {
  local pid
  if [[ -s "$PID_FILE" ]]; then
    IFS= read -r pid < "$PID_FILE" || pid=""
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  fi
  terminate_runtime_pids
}
trap cleanup EXIT

preflight
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
PID_FILE=$(realpath -m "$PID_FILE")
TELEMETRY_PATH=$(realpath -m "$TELEMETRY_PATH")
BOOTSTRAP_PATH=$(realpath -m "$BOOTSTRAP_PATH")
BOOTSTRAP_STATE_PATH=$(realpath -m "$BOOTSTRAP_STATE_PATH")
mkdir -p "$ARTIFACT_DIR"

if (( DRY_RUN )); then
  write_autoload_request
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","launch":"direct-proton-eldenring-exe","watcher":".auto/runtime_probe.sh","timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"runtime_expected_mode":"$RUNTIME_EXPECTED_MODE"}
EOF
  echo "dry-run ok: would start .auto/runtime_probe.sh, launch direct eldenring.exe through Proton, wait <=${RUNTIME_TIMEOUT_SECONDS}s, then tear down owned launcher pid and exact eldenring.exe runtime pids"
  exit 0
fi

[[ "${ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME:-0}" == "1" ]] || fatal "set ER_EFFECTS_AUTHORIZED_DIRECT_RUNTIME=1 for the exact runtime invocation"
[[ "${AUTO_ALLOW_MANUAL_RUNTIME_PROBE:-0}" == "1" ]] || fatal "set AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 for .auto/runtime_probe.sh"
write_autoload_request

(
  cd "$REPO_ROOT"
  ARTIFACT_DIR="$ARTIFACT_DIR" \
  PID_FILE="$PID_FILE" \
  TELEMETRY_PATH="$TELEMETRY_PATH" \
  BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  RUNTIME_TIMEOUT_SECONDS="$RUNTIME_TIMEOUT_SECONDS" \
  RUNTIME_EXPECTED_MODE="$RUNTIME_EXPECTED_MODE" \
  AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 \
  ./.auto/runtime_probe.sh
) > "$ARTIFACT_DIR/runtime-probe.out" 2> "$ARTIFACT_DIR/runtime-probe.err" &
watcher_pid=$!

(
  cd "$GAME_DIR"
  STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
  STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
  ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" \
  ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1 & echo $! > "$PID_FILE"
)

wait "$watcher_pid"
