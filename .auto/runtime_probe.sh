#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
# Single source of truth for the runtime-probe wall-clock cap (seconds); fail safe to the 45s hard truth.
RUNTIME_TIMEOUT_CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 45)"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-$RUNTIME_TIMEOUT_CAP_SECONDS}"
RUNTIME_LAZYLOAD_CHAINLOAD_DLL="${RUNTIME_LAZYLOAD_CHAINLOAD_DLL:-er_effects_rs.dll}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
POLICY_PATH="$REPO_ROOT/.auto/runtime_experiment_policy.rego"
SEAMLESS_DLL_PATH="$GAME_DIR/SeamlessCoop/ersc.dll"
SEAMLESS_STAGED_PATH="${SEAMLESS_STAGED_PATH:-$GAME_DIR/SeamlessCoop/ersc.dll.er-effects-staged}"

cleanup_runtime() {
  if [[ "$RUNTIME_EXPECTED_MODE" == "seamless" && -f "$SEAMLESS_STAGED_PATH" && ! -f "$SEAMLESS_DLL_PATH" ]]; then
    mkdir -p "$(dirname "$SEAMLESS_DLL_PATH")"
    mv -f "$SEAMLESS_STAGED_PATH" "$SEAMLESS_DLL_PATH"
  fi
}

stage_runtime_mode_payload() {
  case "$RUNTIME_EXPECTED_MODE" in
    vanilla)
      if [[ -f "$SEAMLESS_DLL_PATH" ]]; then
        mkdir -p "$(dirname "$SEAMLESS_STAGED_PATH")"
        mv -f "$SEAMLESS_DLL_PATH" "$SEAMLESS_STAGED_PATH"
      fi
      ;;
    seamless|any)
      ;;
    *)
      echo "RUNTIME_EXPECTED_MODE must be vanilla, seamless, or any" >&2
      exit 2
      ;;
  esac
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
    "legal_popup_check": "native_messagebox_and_packed_asset_tos_fmg_fail_fast",
    "timeout_seconds": timeout_seconds,
}, sort_keys=True))
PY
}

validate_runtime_policy() {
  python3 - "$RUNTIME_TIMEOUT_SECONDS" "$RUNTIME_TIMEOUT_CAP_SECONDS" <<'PY'
import sys

timeout_seconds = int(sys.argv[1])
cap = int(sys.argv[2])
if timeout_seconds <= 0 or timeout_seconds > cap:
    raise SystemExit(f"RUNTIME_TIMEOUT_SECONDS must be greater than 0 and no more than {cap}")
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
  # Crash logging ON BY DEFAULT for every probe (file channel -- reliable through Proton, unlike env
  # vars). Installs the vectored AV handler (logs the faulting RVA + caller stack of an access
  # violation, e.g. a wrong-arg native call) + the process-exit hooks, into er-effects-crash.log.
  # Opt out by exporting RUNTIME_DISABLE_CRASH_LOG=1. The product DLL stays opt-in; this is probe-only.
  if [[ "${RUNTIME_DISABLE_CRASH_LOG:-0}" == "1" ]]; then
    rm -f "$GAME_DIR/er-effects-crash-log.txt"
  else
    : > "$GAME_DIR/er-effects-crash-log.txt"
  fi
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
stage_runtime_mode_payload
setup_runtime_payload

# Focus-independent telemetry-only validation mode (opt-in). When RUNTIME_SKIP_VISUAL_CAPTURE=1
# the watcher relies solely on native in-process telemetry for popup/world detection and never
# requires a focused, screenshot-safe target window -- so a run is not bailed with
# target_window_capture_unsafe before the title flow advances. The native fail-closed checks
# (--fail-on-messagebox-dialog / --fail-on-native-legal-popup / --fail-on-server-status-semaphore)
# stay active. Default off: scored product-proof runs keep the supplemental visual OCR checks.
watch_extra_args=()
if [[ "${RUNTIME_SKIP_VISUAL_CAPTURE:-0}" == "1" ]]; then
  watch_extra_args+=(--skip-visual-capture)
fi
# Propagate the TRUE bash launch epoch (captured at the eldenring.exe fire) so the watcher computes
# every milestone delta + the world-load fail-fast deadline from the real launch, not watcher-start.
# The watcher also reads ER_PROBE_LAUNCH_EPOCH directly; passing --launch-epoch is the explicit form.
if [[ -n "${ER_PROBE_LAUNCH_EPOCH:-}" ]]; then
  watch_extra_args+=(--launch-epoch "$ER_PROBE_LAUNCH_EPOCH")
fi
# Extra watcher args passthrough (space-separated), e.g. for a moment-of-truth load run that should not
# be killed by the continue+30s deadline: RUNTIME_EXTRA_WATCH_ARGS="--no-world-load-deadline". The 3s
# per-phase stall watchdog + the runtime cap still bound the run.
if [[ -n "${RUNTIME_EXPECTED_SAVE_ORACLE:-}" ]]; then
  watch_extra_args+=(--expected-save-oracle "$RUNTIME_EXPECTED_SAVE_ORACLE")
fi
if [[ -n "${RUNTIME_EXTRA_WATCH_ARGS:-}" ]]; then
  # shellcheck disable=SC2206
  watch_extra_args+=(${RUNTIME_EXTRA_WATCH_ARGS})
fi

python3 "$REPO_ROOT/scripts/er-readiness-watch.py" \
  --artifact-dir "${ARTIFACT_DIR:?ARTIFACT_DIR is required}" \
  --pid-file "${PID_FILE:?PID_FILE is required}" \
  --telemetry "${TELEMETRY_PATH:?TELEMETRY_PATH is required}" \
  --bootstrap "${BOOTSTRAP_PATH:?BOOTSTRAP_PATH is required}" \
  --bootstrap-state "${BOOTSTRAP_STATE_PATH:?BOOTSTRAP_STATE_PATH is required}" \
  --target "${RUNTIME_WATCH_TARGET:-world-stable}" \
  --expected-runtime-mode "$RUNTIME_EXPECTED_MODE" \
  --fail-on-messagebox-dialog \
  --fail-on-native-legal-popup \
  --fail-on-server-status-semaphore \
  --visual-save-data-popup-check \
  --defer-unsafe-visual-capture-until-telemetry \
  "${watch_extra_args[@]}" \
  --max-runtime-seconds "$RUNTIME_TIMEOUT_SECONDS"
