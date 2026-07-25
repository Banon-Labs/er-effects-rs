#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
PRODUCT_LAUNCHER="${PRODUCT_LAUNCHER:-$HOME/Elden/launch.sh}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/windows-proof-render-smoke-$(date +%Y%m%d-%H%M%S)}"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-}"
RUNTIME_WATCH_TARGET="${RUNTIME_WATCH_TARGET:-}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
DRY_RUN=0
REQUIRE_HANDOFF=0

usage() {
  cat <<EOF
Usage: scripts/run-windows-proof-render-smoke.sh [--dry-run] [--require-handoff]

Launches the normal user/product ME3 launcher ($PRODUCT_LAUNCHER) and fails unless runtime telemetry proves:
  oracle_windows_proof_mode == 1
  oracle_forbidden_render_backend_hits == 0
  oracle_native_overlay_frames > 0

With --require-handoff, also requires:
  oracle_native_overlay_handoff_ready_hits > 0
  oracle_scaleform_memoryfile_custom_asset_hits > 0

Default target is game-man so this is a short renderer-safety smoke. --require-handoff switches the default target to native-overlay-handoff and uses the canonical runtime cap unless overridden.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --require-handoff) REQUIRE_HANDOFF=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$RUNTIME_WATCH_TARGET" ]]; then
  if (( REQUIRE_HANDOFF )); then
    RUNTIME_WATCH_TARGET="native-overlay-handoff"
  else
    RUNTIME_WATCH_TARGET="game-man"
  fi
fi
if [[ -z "$RUNTIME_TIMEOUT_SECONDS" ]]; then
  if (( REQUIRE_HANDOFF )); then
    RUNTIME_TIMEOUT_SECONDS="$(python3 "$REPO_ROOT/scripts/runtime_timeout_cap.py")"
  else
    RUNTIME_TIMEOUT_SECONDS=20
  fi
fi

fatal() { echo "run-windows-proof-render-smoke: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }

runtime_pids() {
  local proc pid comm
  for proc in /proc/[0-9]*; do
    pid=${proc##*/}
    [[ -r "$proc/comm" ]] || continue
    comm=$(<"$proc/comm")
    if [[ "$comm" == "eldenring.exe" ]]; then
      printf '%s\n' "$pid"
    fi
  done
}

cleanup_runtime_pids() {
  local pid
  local -a pids=()
  mapfile -t pids < <(runtime_pids)
  for pid in "${pids[@]}"; do
    [[ -n "$pid" ]] || continue
    kill "$pid" 2>/dev/null || true
  done
  for pid in "${pids[@]}"; do
    [[ -n "$pid" ]] || continue
    timeout 6 tail --pid="$pid" -f /dev/null >/dev/null 2>&1 || true
    if [[ -e "/proc/$pid" ]]; then
      kill -9 "$pid" 2>/dev/null || true
    fi
  done
}

preflight() {
  require_file "$PRODUCT_LAUNCHER"
  require_file "$REPO_ROOT/.auto/runtime_probe.sh"
  require_file "$REPO_ROOT/.auto/runtime_timeout_cap_seconds"
  if (( DRY_RUN )); then
    return 0
  fi
  if ! pgrep -x steam >/dev/null; then
    fatal "Steam is not running; approved Elden Ring probes require Steam already running"
  fi
  if [[ -n "$(runtime_pids)" ]]; then
    fatal "eldenring.exe is already running; refusing to validate a new DLL in an existing process"
  fi
}

preflight
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
mkdir -p "$ARTIFACT_DIR/tmp"
export TMPDIR="$ARTIFACT_DIR/tmp"

PID_FILE="$ARTIFACT_DIR/product-launch.pid"
TELEMETRY_PATH="$ARTIFACT_DIR/er-effects-telemetry.json"
BOOTSTRAP_PATH="$ARTIFACT_DIR/bootstrap.jsonl"
BOOTSTRAP_STATE_PATH="$ARTIFACT_DIR/bootstrap-state.json"
CRASH_LOG_PATH="$ARTIFACT_DIR/er-effects-crash-log.txt"
AUTOLOAD_DEBUG_PATH="$ARTIFACT_DIR/er-effects-autoload-debug.log"
VERDICT_PATH="$ARTIFACT_DIR/windows-proof-render-smoke-verdict.json"

if (( DRY_RUN )); then
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","launcher":"$PRODUCT_LAUNCHER","watch_target":"$RUNTIME_WATCH_TARGET","timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"require_handoff":$REQUIRE_HANDOFF,"criteria":["oracle_windows_proof_mode == 1","oracle_forbidden_render_backend_hits == 0","oracle_native_overlay_frames > 0","oracle_native_overlay_handoff_ready_hits > 0 if --require-handoff","oracle_scaleform_memoryfile_custom_asset_hits > 0 if --require-handoff"]}
EOF
  echo "dry-run ok: would launch $PRODUCT_LAUNCHER, watch target '$RUNTIME_WATCH_TARGET', require Windows-proof renderer telemetry, then cleanup exact eldenring.exe pids"
  echo "artifact_dir=$ARTIFACT_DIR"
  exit 0
fi

rm -f "$PID_FILE" "$TELEMETRY_PATH" "$BOOTSTRAP_PATH" "$BOOTSTRAP_STATE_PATH" "$CRASH_LOG_PATH" "$AUTOLOAD_DEBUG_PATH" "$VERDICT_PATH"
trap cleanup_runtime_pids EXIT

LAUNCH_EPOCH="$(date +%s.%N)"
printf '%s\n' "$LAUNCH_EPOCH" > "$ARTIFACT_DIR/launch-epoch.txt"
(
  TMPDIR="$TMPDIR" \
  ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" \
  ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
  ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
  ER_EFFECTS_CRASH_LOG_PATH="$CRASH_LOG_PATH" \
  ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" \
  "$PRODUCT_LAUNCHER" > "$ARTIFACT_DIR/product-launch.out" 2> "$ARTIFACT_DIR/product-launch.err" & echo $! > "$PID_FILE"
)

watcher_status=0
ARTIFACT_DIR="$ARTIFACT_DIR" \
PID_FILE="$PID_FILE" \
TELEMETRY_PATH="$TELEMETRY_PATH" \
BOOTSTRAP_PATH="$BOOTSTRAP_PATH" \
BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" \
RUNTIME_TIMEOUT_SECONDS="$RUNTIME_TIMEOUT_SECONDS" \
RUNTIME_EXPECTED_MODE="$RUNTIME_EXPECTED_MODE" \
RUNTIME_WATCH_TARGET="$RUNTIME_WATCH_TARGET" \
ER_PROBE_LAUNCH_EPOCH="$LAUNCH_EPOCH" \
RUNTIME_SKIP_VISUAL_CAPTURE=1 \
RUNTIME_EXTRA_WATCH_ARGS="${RUNTIME_EXTRA_WATCH_ARGS:---no-phase-watchdog --no-world-load-deadline}" \
"$REPO_ROOT/.auto/runtime_probe.sh" > "$ARTIFACT_DIR/runtime-probe.out" 2> "$ARTIFACT_DIR/runtime-probe.err" || watcher_status=$?

python3 - "$ARTIFACT_DIR" "$TELEMETRY_PATH" "$VERDICT_PATH" "$watcher_status" "$REQUIRE_HANDOFF" <<'PY'
import json
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
telemetry_path = Path(sys.argv[2])
verdict_path = Path(sys.argv[3])
watcher_status = int(sys.argv[4])
require_handoff = sys.argv[5] == "1"

def as_int(value, default=0):
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, int):
        return value
    if isinstance(value, str):
        try:
            return int(value, 0)
        except ValueError:
            return default
    return default

try:
    telemetry = json.loads(telemetry_path.read_text(encoding="utf-8", errors="replace"))
except Exception:
    telemetry = None

mode = isinstance(telemetry, dict) and as_int(telemetry.get("oracle_windows_proof_mode"), 0) == 1
hits = as_int(telemetry.get("oracle_forbidden_render_backend_hits") if isinstance(telemetry, dict) else None, -1)
native_overlay_frames = as_int(telemetry.get("oracle_native_overlay_frames") if isinstance(telemetry, dict) else None, -1)
native_overlay_stage = as_int(telemetry.get("oracle_native_overlay_stage") if isinstance(telemetry, dict) else None, -1)
native_overlay_failure = as_int(telemetry.get("oracle_native_overlay_failure") if isinstance(telemetry, dict) else None, -1)
native_overlay_handoff_ready_hits = as_int(telemetry.get("oracle_native_overlay_handoff_ready_hits") if isinstance(telemetry, dict) else None, -1)
scaleform_memoryfile_custom_asset_hits = as_int(telemetry.get("oracle_scaleform_memoryfile_custom_asset_hits") if isinstance(telemetry, dict) else None, -1)
native_overlay_proven = native_overlay_frames > 0
native_overlay_handoff_proven = native_overlay_handoff_ready_hits > 0
scaleform_memoryfile_custom_asset_proven = scaleform_memoryfile_custom_asset_hits > 0
verdict = {
    "artifact_dir": str(artifact),
    "watcher_status": watcher_status,
    "watcher_pass": watcher_status == 0,
    "telemetry_written": telemetry_path.is_file(),
    "oracle_windows_proof_mode": 1 if mode else 0,
    "oracle_forbidden_render_backend_hits": hits,
    "oracle_native_overlay_frames": native_overlay_frames,
    "oracle_native_overlay_stage": native_overlay_stage,
    "oracle_native_overlay_failure": native_overlay_failure,
    "oracle_native_overlay_handoff_ready_hits": native_overlay_handoff_ready_hits,
    "oracle_scaleform_memoryfile_custom_asset_hits": scaleform_memoryfile_custom_asset_hits,
    "native_overlay_proven": native_overlay_proven,
    "native_overlay_handoff_proven": native_overlay_handoff_proven,
    "scaleform_memoryfile_custom_asset_proven": scaleform_memoryfile_custom_asset_proven,
    "require_handoff": require_handoff,
    "windows_proof_render_runtime": mode and hits == 0 and native_overlay_proven and (not require_handoff or (native_overlay_handoff_proven and scaleform_memoryfile_custom_asset_proven)),
}
verdict_path.write_text(json.dumps(verdict, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print("windows-proof-render-smoke:", json.dumps(verdict, sort_keys=True))
sys.exit(0 if verdict["watcher_pass"] and verdict["windows_proof_render_runtime"] else 3)
PY
