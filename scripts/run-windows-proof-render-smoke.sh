#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
PRODUCT_LAUNCHER="${PRODUCT_LAUNCHER:-$HOME/Elden/launch.sh}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/windows-proof-render-smoke-$(date +%Y%m%d-%H%M%S)}"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-}"
RUNTIME_WATCH_TARGET="${RUNTIME_WATCH_TARGET:-}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
DRY_RUN=0
REQUIRE_WORLD_READY=0

usage() {
  cat <<EOF
Usage: scripts/run-windows-proof-render-smoke.sh [--dry-run] [--require-world-ready|--require-handoff]

Launches the normal user/product ME3 launcher ($PRODUCT_LAUNCHER) and fails unless runtime telemetry proves:
  oracle_windows_proof_mode == 1
  oracle_forbidden_render_backend_hits == 0
  oracle_native_overlay_child_window == 1
  oracle_native_overlay_parent_hwnd > 0
  oracle_native_overlay_child_parent_match == 1
  oracle_native_overlay_child_cover_match == 1        # exact resize/history is diagnostic; current cover is product proof
  oracle_native_overlay_frames > 0
  oracle_native_overlay_zorder_lift_hits > 0
  oracle_native_overlay_present_ok_hits > 0
  oracle_native_overlay_present_fail_hits == 0
  oracle_native_overlay_child_is_window == 1
  oracle_native_overlay_child_is_visible == 1
  oracle_native_overlay_bar_pixel_frames > 0
  oracle_native_overlay_bar_pixel_missing_frames == 0
  oracle_native_overlay_pixel_probe_matches > 0

With --require-world-ready, also requires:
  oracle_native_overlay_covering_loading_hits > 0   # bridge was visible during loading
  oracle_native_overlay_content_frames > 0          # bridge rendered real full-frame/content proof
  oracle_native_overlay_show == 0

Default target is game-man so this is a short renderer-safety smoke. --require-world-ready switches the default target to world-stable and uses the canonical runtime cap unless overridden. --require-handoff is a compatibility alias for --require-world-ready; GFx handoff is not the product cover.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1; shift ;;
    --require-world-ready|--require-handoff) REQUIRE_WORLD_READY=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [[ -z "$RUNTIME_WATCH_TARGET" ]]; then
  if (( REQUIRE_WORLD_READY )); then
    RUNTIME_WATCH_TARGET="world-stable"
  else
    RUNTIME_WATCH_TARGET="game-man"
  fi
fi
if [[ -z "$RUNTIME_TIMEOUT_SECONDS" ]]; then
  if (( REQUIRE_WORLD_READY )); then
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
GAME_DIR="${ER_GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
STANDALONE_TELEMETRY_JSONL="$GAME_DIR/er-telemetry-timeseries.jsonl"
STANDALONE_TELEMETRY_JSON="$GAME_DIR/er-telemetry-standalone.json"

if (( DRY_RUN )); then
  cat > "$ARTIFACT_DIR/dry-run-summary.json" <<EOF
{"artifact_dir":"$ARTIFACT_DIR","launcher":"$PRODUCT_LAUNCHER","watch_target":"$RUNTIME_WATCH_TARGET","timeout_seconds":$RUNTIME_TIMEOUT_SECONDS,"require_world_ready":$REQUIRE_WORLD_READY,"criteria":["oracle_windows_proof_mode == 1","oracle_forbidden_render_backend_hits == 0","oracle_native_overlay_child_window == 1","oracle_native_overlay_parent_hwnd > 0","oracle_native_overlay_child_parent_match == 1","oracle_native_overlay_child_cover_match == 1","oracle_native_overlay_frames > 0","oracle_native_overlay_zorder_lift_hits > 0","oracle_native_overlay_present_ok_hits > 0","oracle_native_overlay_present_fail_hits == 0","oracle_native_overlay_child_is_window == 1","oracle_native_overlay_child_is_visible == 1","oracle_native_overlay_bar_pixel_frames > 0","oracle_native_overlay_bar_pixel_missing_frames == 0","oracle_native_overlay_pixel_probe_matches > 0","oracle_native_overlay_covering_loading_hits > 0 if --require-world-ready","oracle_native_overlay_content_frames > 0 if --require-world-ready","oracle_native_overlay_show == 0 if --require-world-ready"]}
EOF
  echo "dry-run ok: would launch $PRODUCT_LAUNCHER, watch target '$RUNTIME_WATCH_TARGET', require Windows-proof renderer telemetry, then cleanup exact eldenring.exe pids"
  echo "artifact_dir=$ARTIFACT_DIR"
  exit 0
fi

rm -f "$PID_FILE" "$TELEMETRY_PATH" "$BOOTSTRAP_PATH" "$BOOTSTRAP_STATE_PATH" "$CRASH_LOG_PATH" "$AUTOLOAD_DEBUG_PATH" "$VERDICT_PATH"
rm -f "$STANDALONE_TELEMETRY_JSONL" "$STANDALONE_TELEMETRY_JSON"
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

if [[ -f "$STANDALONE_TELEMETRY_JSONL" ]]; then
  cp -f "$STANDALONE_TELEMETRY_JSONL" "$ARTIFACT_DIR/er-telemetry-timeseries.jsonl"
fi
if [[ -f "$STANDALONE_TELEMETRY_JSON" ]]; then
  cp -f "$STANDALONE_TELEMETRY_JSON" "$ARTIFACT_DIR/er-telemetry-standalone.json"
fi

verdict_args=(
  --artifact-dir "$ARTIFACT_DIR"
  --telemetry "$TELEMETRY_PATH"
  --verdict "$VERDICT_PATH"
  --watcher-status "$watcher_status"
)
if (( REQUIRE_WORLD_READY )); then
  verdict_args+=(--require-world-ready)
fi
python3 "$REPO_ROOT/scripts/windows-proof-render-smoke-verdict.py" "${verdict_args[@]}"
