#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-60}"
RUNTIME_LAZYLOAD_CHAINLOAD_DLL="${RUNTIME_LAZYLOAD_CHAINLOAD_DLL:-er_effects_rs.dll}"
POLICY_PATH="$REPO_ROOT/.auto/runtime_experiment_policy.rego"

cleanup_runtime() {
  :
}

runtime_policy_input() {
  python3 - "$RUNTIME_TIMEOUT_SECONDS" <<'PY'
import json
import sys

timeout_seconds = int(sys.argv[1])
print(json.dumps({
    "readiness_watcher": "scripts/er-readiness-watch.py",
    "no_telemetry_bootstrap_failure": "window_without_bootstrap_or_task_ready",
    "host_input": "none",
    "teardown": "process_tree_and_save_restore",
    "timeout_seconds": timeout_seconds,
}, sort_keys=True))
PY
}

validate_runtime_policy() {
  python3 - "$RUNTIME_TIMEOUT_SECONDS" <<'PY'
import sys

timeout_seconds = int(sys.argv[1])
if timeout_seconds <= 0 or timeout_seconds > 60:
    raise SystemExit("RUNTIME_TIMEOUT_SECONDS must be greater than 0 and no more than 60")
PY
  if [[ "${AUTO_ALLOW_MANUAL_RUNTIME_PROBE:-0}" != "1" ]]; then
    echo "runtime_probe.sh is disabled fail-closed; set AUTO_ALLOW_MANUAL_RUNTIME_PROBE=1 for a deliberate manual run" >&2
    exit 2
  fi
  command -v opa >/dev/null 2>&1 || { echo "missing required command: opa" >&2; exit 127; }
  local decision
  decision=$(runtime_policy_input | opa eval --format raw -d "$POLICY_PATH" -I 'data.auto.runtime_experiment.allow')
  if [[ "$decision" != "true" ]]; then
    echo "runtime policy denied manual probe" >&2
    runtime_policy_input | opa eval --format pretty -d "$POLICY_PATH" -I 'data.auto.runtime_experiment.deny' >&2 || true
    exit 2
  fi
}

setup_runtime_payload() {
  mkdir -p "$GAME_DIR/dllMods"
  cp -f "$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll" "$GAME_DIR/er_effects_rs.dll"
  rm -f "$GAME_DIR/dllMods/er_effects_rs.dll"
  if [[ "$RUNTIME_LAZYLOAD_CHAINLOAD_DLL" == "er_effects_rs.dll" ]]; then
    cat > "$GAME_DIR/lazyLoad.ini" <<'EOF'
; LazyLoader by Church Guard
[LAZYLOAD]
dllModFolderName=dllMods
[LOADORDER]
[CHAINLOAD]
dll=er_effects_rs.dll
EOF
  else
    cat > "$GAME_DIR/lazyLoad.ini" <<EOF
; LazyLoader by Church Guard
[LAZYLOAD]
dllModFolderName=dllMods
[LOADORDER]
[CHAINLOAD]
dll=$RUNTIME_LAZYLOAD_CHAINLOAD_DLL
EOF
  fi
}

trap cleanup_runtime EXIT
validate_runtime_policy
setup_runtime_payload
python3 "$REPO_ROOT/scripts/er-readiness-watch.py" \
  --artifact-dir "${ARTIFACT_DIR:?ARTIFACT_DIR is required}" \
  --pid-file "${PID_FILE:?PID_FILE is required}" \
  --telemetry "${TELEMETRY_PATH:?TELEMETRY_PATH is required}" \
  --bootstrap "${BOOTSTRAP_PATH:?BOOTSTRAP_PATH is required}" \
  --bootstrap-state "${BOOTSTRAP_STATE_PATH:?BOOTSTRAP_STATE_PATH is required}" \
  --target "${RUNTIME_WATCH_TARGET:-world-stable}" \
  --max-runtime-seconds "$RUNTIME_TIMEOUT_SECONDS"
