#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi
if [[ -f .envrc ]]; then
  # shellcheck disable=SC1091
  . ./.envrc
fi
RUNTIME_ENV_FILE="${AUTO_RUNTIME_ENV_FILE:-.auto/runtime-env}"
if [[ -n "$RUNTIME_ENV_FILE" && -f "$RUNTIME_ENV_FILE" ]]; then
  # shellcheck source=/dev/null
  . "$RUNTIME_ENV_FILE"
fi

now_ms() {
  python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
}

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/smoke/autoload-runtime-$(date +%Y%m%d-%H%M%S)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
LAUNCH_MODE="${LAUNCH_MODE:-direct-protected}"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-30}"
RUNTIME_MAX_NUDGES="${RUNTIME_MAX_NUDGES:-0}"
RUNTIME_READINESS_RATIONALE="${RUNTIME_READINESS_RATIONALE:-$REPO_ROOT/.auto/runtime-readiness-rationale}"
mkdir -p "$ARTIFACT_DIR"
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")
TELEMETRY_PATH="${TELEMETRY_PATH:-$ARTIFACT_DIR/telemetry-live.json}"
BOOTSTRAP_PATH="${BOOTSTRAP_PATH:-$ARTIFACT_DIR/bootstrap.jsonl}"
BOOTSTRAP_STATE_PATH="${BOOTSTRAP_STATE_PATH:-$ARTIFACT_DIR/bootstrap-state.json}"
COMMAND_PATH="${COMMAND_PATH:-$GAME_DIR/er-effects-command.txt}"
AUTOLOAD_PATH="${AUTOLOAD_PATH:-$GAME_DIR/er-effects-autoload.txt}"
SAFE_INPUT_PATH="${SAFE_INPUT_PATH:-$GAME_DIR/er-effects-safe-input.txt}"
AUTOLOAD_DEBUG_PATH="${AUTOLOAD_DEBUG_PATH:-$ARTIFACT_DIR/autoload-debug.log}"
TRACE_CONTINUE_PATH="${TRACE_CONTINUE_PATH:-$ARTIFACT_DIR/continue-trace.log}"
TRACE_MENU_TASK_UPDATE_PATH="${TRACE_MENU_TASK_UPDATE_PATH:-$GAME_DIR/er-effects-trace-menu-task-update.txt}"
TRACE_TITLE_STAGE_PATH="${TRACE_TITLE_STAGE_PATH:-$GAME_DIR/er-effects-trace-title-stage.txt}"
PUMP_MOVE_MAP_PATH="${PUMP_MOVE_MAP_PATH:-$GAME_DIR/er-effects-pump-move-map.txt}"
FORCE_TITLE_STATE_PATH="${FORCE_TITLE_STATE_PATH:-$GAME_DIR/er-effects-force-title-state.txt}"
NATIVE_TITLE_JOB_PATH="${NATIVE_TITLE_JOB_PATH:-$GAME_DIR/er-effects-native-title-job.txt}"
NATIVE_TITLE_TOGGLE_PATH="${NATIVE_TITLE_TOGGLE_PATH:-$GAME_DIR/er-effects-native-title-toggle.txt}"
NATIVE_EXTRA_PARENT_PATH="${NATIVE_EXTRA_PARENT_PATH:-$GAME_DIR/er-effects-native-extra-parent.txt}"
TRACE_TASK_NODE_BYTES_PATH="${TRACE_TASK_NODE_BYTES_PATH:-$GAME_DIR/er-effects-trace-task-node-bytes.txt}"
SELECTBOT_PROBE_PATH="${SELECTBOT_PROBE_PATH:-$GAME_DIR/er-effects-selectbot-probe.txt}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
if [[ "$LAUNCH_MODE" == "steam" ]]; then
  # Steam launch options do not reliably inherit per-run environment from this
  # wrapper. Use the DLL's game-directory defaults for live IPC and copy them
  # into the artifact directory during cleanup.
  TELEMETRY_PATH="$GAME_DIR/er-effects-telemetry.json"
  BOOTSTRAP_PATH="$GAME_DIR/er-effects-bootstrap.jsonl"
  BOOTSTRAP_STATE_PATH="$GAME_DIR/er-effects-bootstrap-state.json"
  AUTOLOAD_DEBUG_PATH="$GAME_DIR/er-effects-autoload-debug.log"
  TRACE_CONTINUE_PATH="$GAME_DIR/er-effects-continue-trace.log"
fi
LAUNCH_PID_FILE="$ARTIFACT_DIR/launcher.pid"

START_MS=$(now_ms)
RUN_START_MS=0
DRIVER_RC=0
CLEANUP_ARMED=0
TIMELINE_LOG="$ARTIFACT_DIR/runtime-timeline.jsonl"
RUNTIME_CAPTURE_INPUT_OCR="${RUNTIME_CAPTURE_INPUT_OCR:-0}"
RUNTIME_SKIP_FINAL_MEASURE="${RUNTIME_SKIP_FINAL_MEASURE:-0}"
INPUT_CAPTURE_PID=0
INPUT_CAPTURE_STOP="$ARTIFACT_DIR/input-capture.stop"

save_roots=(
  "$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing"
  "/mnt/c/Users/choza/AppData/Roaming/EldenRing"
)

runtime_process_pattern='(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)'

log_timeline() {
  local event=$1
  shift || true
  local details="$*"
  python3 - "$TIMELINE_LOG" "$START_MS" "$RUN_START_MS" "$event" "$details" <<'PY'
import json
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

log_path = Path(sys.argv[1])
start_ms = int(sys.argv[2])
run_start_ms = int(sys.argv[3])
event = sys.argv[4]
details = sys.argv[5]
now_ms = int(time.time() * 1000)
entry = {
    "ts": datetime.now(timezone.utc).isoformat(),
    "event": event,
    "total_s": round((now_ms - start_ms) / 1000, 3),
    "run_s": None if run_start_ms <= 0 else round((now_ms - run_start_ms) / 1000, 3),
    "details": details,
}
with log_path.open("a", encoding="utf-8") as handle:
    handle.write(json.dumps(entry, sort_keys=True) + "\n")
PY
}

write_state_snapshot() {
  local output=$1
  python3 - "$output" "$ARTIFACT_DIR" "$GAME_DIR" "$runtime_process_pattern" <<'PY'
import json
import re
import subprocess
import sys
from pathlib import Path

output = Path(sys.argv[1])
artifact = Path(sys.argv[2])
game_dir = Path(sys.argv[3])
pattern = re.compile(sys.argv[4], re.I)
process_rows = []
try:
    ps_output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
    process_rows = [line.strip() for line in ps_output.splitlines() if pattern.search(line)]
except Exception as exc:
    process_rows = [f"ps_error={exc}"]
windows = []
try:
    clients = json.loads(subprocess.check_output(["hyprctl", "clients", "-j"], text=True))
    for client in clients:
        title = client.get("title") or ""
        klass = client.get("class") or ""
        if "elden" in title.lower() or klass == "steam_app_1245620":
            windows.append({
                "class": klass,
                "title": title,
                "at": client.get("at"),
                "size": client.get("size"),
                "workspace": (client.get("workspace") or {}).get("name"),
            })
except Exception as exc:
    windows = [{"error": str(exc)}]
telemetry = {}
for candidate in [artifact / "telemetry.json", artifact / "final-telemetry.json", game_dir / "er-effects-telemetry.json"]:
    if candidate.exists():
        try:
            telemetry = json.loads(candidate.read_text(encoding="utf-8", errors="replace"))
            break
        except Exception as exc:
            telemetry = {"error": str(exc), "path": str(candidate)}
latest_capture = None
captures = list(artifact.glob("*.jpg")) + list(artifact.glob("*.png"))
if captures:
    latest = max(captures, key=lambda path: path.stat().st_mtime)
    latest_capture = {"path": str(latest), "mtime": latest.stat().st_mtime, "bytes": latest.stat().st_size}
output.write_text(json.dumps({
    "process_rows": process_rows,
    "windows": windows,
    "telemetry": telemetry,
    "latest_capture": latest_capture,
}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

validate_runtime_policy() {
  command -v opa >/dev/null 2>&1 || {
    echo "missing required command: opa" >&2
    exit 127
  }
  local input_path allowed
  input_path="$ARTIFACT_DIR/runtime-policy-input.json"
  python3 - "$input_path" "$LAUNCH_MODE" "$RUNTIME_TIMEOUT_SECONDS" "$RUNTIME_READINESS_RATIONALE" <<'PY'
import json
import os
import sys
from pathlib import Path

output, launch_mode, timeout_seconds, rationale_path = sys.argv[1:5]
try:
    parsed_timeout_seconds = float(timeout_seconds)
except ValueError:
    parsed_timeout_seconds = timeout_seconds
rationale = {}
path = Path(rationale_path)
if path.exists():
    for raw_line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        rationale[key.strip()] = value.strip().strip('"').strip("'")
payload = {
    "explicit_opt_in": os.environ.get("AUTO_ALLOW_RUNTIME_PROBE") == "1",
    "launch_mode": launch_mode,
    "timeout_seconds": parsed_timeout_seconds,
    "native_title_accept_gate": os.environ.get("ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_ACCEPT_GATE") == "1",
    "runtime_entrypoint": "measure_runtime_trigger",
    "readiness_watcher": rationale.get("readiness_watcher", ""),
    "no_telemetry_bootstrap_failure": rationale.get("no_telemetry_bootstrap_failure", ""),
    "host_input": rationale.get("host_input", ""),
    "teardown": rationale.get("teardown", ""),
    "readiness_strategy": rationale.get("readiness_strategy", ""),
    "structured_failure": rationale.get("structured_failure", ""),
    "user_impact": rationale.get("user_impact", ""),
}
Path(output).write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
  opa check "$REPO_ROOT/.auto/runtime_experiment_policy.rego"
  allowed=$(opa eval --format raw -d "$REPO_ROOT/.auto/runtime_experiment_policy.rego" -i "$input_path" 'data.auto.runtime_experiment.allow')
  if [[ "$allowed" != "true" ]]; then
    echo "runtime experiment rejected by Rego policy:" >&2
    opa eval --format pretty -d "$REPO_ROOT/.auto/runtime_experiment_policy.rego" -i "$input_path" 'data.auto.runtime_experiment.deny' >&2
    exit 2
  fi
}

snapshot_saves() {
  local output=$1
  : > "$output"
  for root in "${save_roots[@]}"; do
    [[ -d "$root" ]] || continue
    find "$root" -type f \( -name 'ER0000.sl2' -o -name 'ER0000.co2' -o -name '*.sl2' -o -name '*.co2' \) -print0
  done | sort -z | xargs -0 --no-run-if-empty sha256sum > "$output"
}

backup_saves() {
  python3 - "$ARTIFACT_DIR" "${save_roots[@]}" <<'PY'
import json
import shutil
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
roots = [Path(value).expanduser() for value in sys.argv[2:]]
backup_dir = artifact / "save-backup"
files_dir = backup_dir / "files"
files_dir.mkdir(parents=True, exist_ok=True)
manifest = []
index = 0
for root in roots:
    if not root.is_dir():
        continue
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.suffix.lower() not in {".sl2", ".co2"}:
            continue
        target = files_dir / f"{index:04d}-{path.name}"
        shutil.copy2(path, target)
        manifest.append({"source": str(path), "backup": str(target)})
        index += 1
(backup_dir / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(f"backed_up_saves={len(manifest)}")
PY
}

runtime_process_rows() {
  python3 - "$runtime_process_pattern" <<'PY'
import re
import subprocess
import sys

pattern = re.compile(sys.argv[1], re.I)
output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
for line in output.splitlines():
    stripped = line.strip()
    if not stripped:
        continue
    pid_text, _, args = stripped.partition(" ")
    if pid_text.isdigit() and pattern.search(args):
        print(stripped)
PY
}

teardown_runtime_processes() {
  python3 - "$ARTIFACT_DIR" "$runtime_process_pattern" <<'PY'
import os
import re
import signal
import subprocess
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
pattern = re.compile(sys.argv[2], re.I)

def procs():
    output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
    rows = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        pid_text, _, args = stripped.partition(" ")
        if pid_text.isdigit() and pattern.search(args):
            rows.append((int(pid_text), args))
    return rows

before = procs()
(artifact / "teardown-before.txt").write_text("".join(f"{pid} {args}\n" for pid, args in before), encoding="utf-8")
for pid, _ in before:
    try:
        os.kill(pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        pass
mid = procs()
for pid, _ in mid:
    try:
        os.kill(pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass
after = procs()
(artifact / "teardown-after.txt").write_text("".join(f"{pid} {args}\n" for pid, args in after), encoding="utf-8")
PY
}

restore_saves_from_backup() {
  if [[ "${RESTORE_RUNTIME_SAVES:-1}" != "1" ]]; then
    printf 'restore_runtime_saves=disabled\n' > "$ARTIFACT_DIR/save-restore.log"
    return 0
  fi
  python3 - "$ARTIFACT_DIR" <<'PY'
import json
import shutil
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
manifest_path = artifact / "save-backup" / "manifest.json"
log_path = artifact / "save-restore.log"
if not manifest_path.exists():
    log_path.write_text("restore_runtime_saves=missing_manifest\n", encoding="utf-8")
    raise SystemExit(0)
manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
lines = []
for entry in manifest:
    source = Path(entry["source"])
    backup = Path(entry["backup"])
    if backup.exists():
        source.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(backup, source)
        lines.append(f"restored {source} from {backup}")
log_path.write_text("\n".join(lines) + ("\n" if lines else ""), encoding="utf-8")
PY
}

copy_runtime_logs() {
  cp -f "$TELEMETRY_PATH" "$ARTIFACT_DIR/telemetry.json" 2>/dev/null || true
  cp -f "$TELEMETRY_PATH" "$ARTIFACT_DIR/final-telemetry.json" 2>/dev/null || true
  cp -f "$AUTOLOAD_DEBUG_PATH" "$ARTIFACT_DIR/autoload-debug-default.log" 2>/dev/null || true
  cp -f "$TRACE_CONTINUE_PATH" "$ARTIFACT_DIR/continue-trace.log" 2>/dev/null || true
  cp -f "$BOOTSTRAP_PATH" "$ARTIFACT_DIR/bootstrap.jsonl" 2>/dev/null || true
  cp -f "$BOOTSTRAP_STATE_PATH" "$ARTIFACT_DIR/bootstrap-state.json" 2>/dev/null || true
}

write_runtime_metrics() {
  local end_ms runtime_ms run_runtime_ms save_safety_ok process_count
  end_ms=$(now_ms)
  runtime_ms=$((end_ms - START_MS))
  if (( RUN_START_MS > 0 )); then
    run_runtime_ms=$((end_ms - RUN_START_MS))
  else
    run_runtime_ms=$runtime_ms
  fi
  if cmp -s "$ARTIFACT_DIR/save-hashes-before.txt" "$ARTIFACT_DIR/save-hashes-after.txt"; then
    save_safety_ok=1
  else
    save_safety_ok=0
  fi
  process_count=$(runtime_process_rows | wc -l)
  python3 - "$ARTIFACT_DIR" "$runtime_ms" "$run_runtime_ms" "$DRIVER_RC" "$save_safety_ok" "$process_count" <<'PY'
import json
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
runtime_ms = int(sys.argv[2])
run_runtime_ms = int(sys.argv[3])
driver_rc = int(sys.argv[4])
save_safety_ok = int(sys.argv[5])
process_count = int(sys.argv[6])
telemetry = artifact / "telemetry.json"
final_telemetry = artifact / "final-telemetry.json"
if not telemetry.exists() and not final_telemetry.exists():
    telemetry.write_text("{}\n", encoding="utf-8")
player_available = False
for candidate in [final_telemetry, telemetry]:
    if candidate.exists():
        try:
            player_available = json.loads(candidate.read_text(encoding="utf-8")).get("player_available") is True
            break
        except Exception:
            pass
readiness = {}
readiness_path = artifact / "readiness-result.json"
if readiness_path.exists():
    try:
        readiness = json.loads(readiness_path.read_text(encoding="utf-8"))
    except Exception:
        readiness = {}
input_reason_known = 0
input_reason_summary_path = artifact / "input-reason-summary.json"
autoload_debug = artifact / "autoload-debug-default.log"
if input_reason_summary_path.exists():
    try:
        summary = json.loads(input_reason_summary_path.read_text(encoding="utf-8", errors="replace"))
        reasons_by_pulse = summary.get("reasons_by_pulse", {})
        debug_text = autoload_debug.read_text(encoding="utf-8", errors="replace") if autoload_debug.exists() else ""
        final_gate_known = "input_gate[post_map_continuation]" in debug_text
        pulse_count = 0
        for candidate in [final_telemetry, telemetry]:
            if candidate.exists():
                try:
                    pulse_count = int(json.loads(candidate.read_text(encoding="utf-8", errors="replace")).get("safe_input_pulses_sent") or 0)
                    break
                except Exception:
                    pass
        if pulse_count > 0 and final_gate_known and all(reasons_by_pulse.get(str(pulse)) for pulse in range(1, pulse_count)):
            input_reason_known = 1
    except Exception:
        input_reason_known = 0
metrics = {
    "driver_rc": driver_rc,
    "runtime_probe_seconds": round(run_runtime_ms / 1000, 3),
    "runtime_total_seconds": round(runtime_ms / 1000, 3),
    "time_to_player_seconds": round(run_runtime_ms / 1000, 3) if player_available else -1,
    "er_process_teardown_ok": 1 if process_count == 0 else 0,
    "host_pointer_input_used": 0,
    "save_safety_ok": save_safety_ok,
    "readiness_ready": 1 if readiness.get("ready") is True else 0,
    "readiness_reason": readiness.get("reason"),
    "input_reason_known": input_reason_known,
}
(artifact / "runtime-metrics.json").write_text(json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(f"runtime_artifact_dir={artifact}")
print(f"driver_rc={driver_rc}")
print(f"er_process_teardown_ok={metrics['er_process_teardown_ok']}")
print(f"save_safety_ok={save_safety_ok}")
PY
}

cleanup_runtime() {
  (( CLEANUP_ARMED )) || return 0
  CLEANUP_ARMED=0
  log_timeline "cleanup_start"
  if [[ "${ER_EFFECTS_TRACE_MENU_TASK_UPDATE:-0}" == "1" ]]; then
    rm -f "$TRACE_MENU_TASK_UPDATE_PATH"
  fi
  if [[ "${ER_EFFECTS_TRACE_TITLE_STAGE:-0}" == "1" ]]; then
    rm -f "$TRACE_TITLE_STAGE_PATH"
  fi
  if [[ "${ER_EFFECTS_AUTOLOAD_PUMP_MOVE_MAP:-0}" == "1" ]]; then
    rm -f "$PUMP_MOVE_MAP_PATH"
  fi
  if [[ -n "${ER_EFFECTS_AUTOLOAD_FORCE_TITLE_STATE:-}" ]]; then
    rm -f "$FORCE_TITLE_STATE_PATH"
  fi
  if [[ "${ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB:-0}" == "1" ]]; then
    rm -f "$NATIVE_TITLE_JOB_PATH"
  fi
  if [[ "${ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_TOGGLE:-0}" == "1" ]]; then
    rm -f "$NATIVE_TITLE_TOGGLE_PATH"
  fi
  if [[ "${ER_EFFECTS_AUTOLOAD_NATIVE_EXTRA_PARENT:-0}" == "1" ]]; then
    rm -f "$NATIVE_EXTRA_PARENT_PATH"
  fi
  if [[ "${ER_EFFECTS_TRACE_TASK_NODE_BYTES:-0}" == "1" ]]; then
    rm -f "$TRACE_TASK_NODE_BYTES_PATH"
  fi
  if [[ "${ER_EFFECTS_SELECTBOT_PROBE:-0}" == "1" ]]; then
    rm -f "$SELECTBOT_PROBE_PATH"
  fi
  copy_runtime_logs || true
  write_state_snapshot "$ARTIFACT_DIR/final-state-before-cleanup.json" || true
  teardown_runtime_processes || true
  snapshot_saves "$ARTIFACT_DIR/save-hashes-after-pre-restore.txt"
  restore_saves_from_backup || true
  snapshot_saves "$ARTIFACT_DIR/save-hashes-after.txt"
  copy_runtime_logs || true
  stop_input_capture_ocr || true
  write_runtime_metrics || true
  write_state_snapshot "$ARTIFACT_DIR/final-state-after-cleanup.json" || true
  log_timeline "cleanup_finish"
}

setup_runtime_payload() {
  local lazyloader_dir
  lazyloader_dir="${LAZYLOADER_DIR:-$GAME_DIR/dllMods.disabled/lazyloader-20260611-234916}"
  {
    printf '[runtime_probe] setup phase: build DLL and install LazyLoader payload\n'
    cargo xwin build --target x86_64-pc-windows-msvc --release
    cp -f "$lazyloader_dir/dinput8.dll" "$GAME_DIR/dinput8.dll"
    cp -f "$lazyloader_dir/lazyLoad.ini" "$GAME_DIR/lazyLoad.ini"
    python3 - "$GAME_DIR/lazyLoad.ini" <<'PY'
import sys
from pathlib import Path
path = Path(sys.argv[1])
text = path.read_text(encoding="utf-8", errors="replace")
if "0=er_effects_rs.dll" not in text:
    text = text.replace("[LOADORDER]\n", "[LOADORDER]\n0=er_effects_rs.dll\n", 1)
path.write_text(text, encoding="utf-8")
PY
    mkdir -p "$GAME_DIR/dllMods"
    cp -f "$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll" "$GAME_DIR/dllMods/er_effects_rs.dll"
    {
      printf 'slot=%s\n' "$ER_EFFECTS_AUTOLOAD_SLOT"
      printf 'method=%s\n' "$ER_EFFECTS_AUTOLOAD_METHOD"
      printf 'require_title_bootstrap=%s\n' "$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP"
    } > "$AUTOLOAD_PATH"
    rm -f "$GAME_DIR/er-effects-safe-input.txt"
    if [[ "${ER_EFFECTS_SAFE_INPUT_CONFIRM_COUNT:-0}" != "0" ]]; then
      {
        printf 'backend=%s\n' "${ER_EFFECTS_SAFE_INPUT_BACKEND:-post_message}"
        printf 'confirm_count=%s\n' "${ER_EFFECTS_SAFE_INPUT_CONFIRM_COUNT:-0}"
        printf 'interval_ticks=%s\n' "${ER_EFFECTS_SAFE_INPUT_INTERVAL_TICKS:-30}"
        if [[ -n "${ER_EFFECTS_SAFE_INPUT_INITIAL_DELAY_TICKS:-}" ]]; then
          printf 'initial_delay_ticks=%s\n' "$ER_EFFECTS_SAFE_INPUT_INITIAL_DELAY_TICKS"
        fi
      } > "$SAFE_INPUT_PATH"
      cp -f "$SAFE_INPUT_PATH" "$ARTIFACT_DIR/safe-input-request.txt"
    else
      printf 'confirm_count=0\n' > "$ARTIFACT_DIR/safe-input-request.txt"
    fi
    if [[ "${ER_EFFECTS_TRACE_MENU_TASK_UPDATE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TRACE_MENU_TASK_UPDATE_PATH"
      cp -f "$TRACE_MENU_TASK_UPDATE_PATH" "$ARTIFACT_DIR/trace-menu-task-update-request.txt"
    else
      rm -f "$TRACE_MENU_TASK_UPDATE_PATH"
    fi
    if [[ "${ER_EFFECTS_TRACE_TITLE_STAGE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TRACE_TITLE_STAGE_PATH"
      cp -f "$TRACE_TITLE_STAGE_PATH" "$ARTIFACT_DIR/trace-title-stage-request.txt"
    else
      rm -f "$TRACE_TITLE_STAGE_PATH"
    fi
    if [[ "${ER_EFFECTS_AUTOLOAD_PUMP_MOVE_MAP:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$PUMP_MOVE_MAP_PATH"
      cp -f "$PUMP_MOVE_MAP_PATH" "$ARTIFACT_DIR/pump-move-map-request.txt"
    else
      rm -f "$PUMP_MOVE_MAP_PATH"
    fi
    if [[ -n "${ER_EFFECTS_AUTOLOAD_FORCE_TITLE_STATE:-}" ]]; then
      printf '%s\n' "$ER_EFFECTS_AUTOLOAD_FORCE_TITLE_STATE" > "$FORCE_TITLE_STATE_PATH"
      cp -f "$FORCE_TITLE_STATE_PATH" "$ARTIFACT_DIR/force-title-state-request.txt"
    else
      rm -f "$FORCE_TITLE_STATE_PATH"
    fi
    if [[ "${ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_JOB:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_TITLE_JOB_PATH"
      cp -f "$NATIVE_TITLE_JOB_PATH" "$ARTIFACT_DIR/native-title-job-request.txt"
    else
      rm -f "$NATIVE_TITLE_JOB_PATH"
    fi
    if [[ "${ER_EFFECTS_AUTOLOAD_NATIVE_TITLE_TOGGLE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_TITLE_TOGGLE_PATH"
      cp -f "$NATIVE_TITLE_TOGGLE_PATH" "$ARTIFACT_DIR/native-title-toggle-request.txt"
    else
      rm -f "$NATIVE_TITLE_TOGGLE_PATH"
    fi
    if [[ "${ER_EFFECTS_AUTOLOAD_NATIVE_EXTRA_PARENT:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_EXTRA_PARENT_PATH"
      cp -f "$NATIVE_EXTRA_PARENT_PATH" "$ARTIFACT_DIR/native-extra-parent-request.txt"
    else
      rm -f "$NATIVE_EXTRA_PARENT_PATH"
    fi
    if [[ "${ER_EFFECTS_TRACE_TASK_NODE_BYTES:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TRACE_TASK_NODE_BYTES_PATH"
      cp -f "$TRACE_TASK_NODE_BYTES_PATH" "$ARTIFACT_DIR/trace-task-node-bytes-request.txt"
    else
      rm -f "$TRACE_TASK_NODE_BYTES_PATH"
    fi
    if [[ "${ER_EFFECTS_SELECTBOT_PROBE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$SELECTBOT_PROBE_PATH"
      cp -f "$SELECTBOT_PROBE_PATH" "$ARTIFACT_DIR/selectbot-probe-request.txt"
    else
      rm -f "$SELECTBOT_PROBE_PATH"
    fi
    rm -f "$TELEMETRY_PATH" "$COMMAND_PATH" "$AUTOLOAD_DEBUG_PATH" "$TRACE_CONTINUE_PATH" "$BOOTSTRAP_PATH" "$BOOTSTRAP_STATE_PATH"
  } > "$ARTIFACT_DIR/setup.out" 2>&1
}

start_input_capture_ocr() {
  [[ "$RUNTIME_CAPTURE_INPUT_OCR" == "1" ]] || return 0
  rm -f "$INPUT_CAPTURE_STOP"
  log_timeline "input_capture_ocr_start" "log=$AUTOLOAD_DEBUG_PATH"
  python3 .auto/input_capture_ocr.py \
    --artifact-dir "$ARTIFACT_DIR" \
    --log-path "$AUTOLOAD_DEBUG_PATH" \
    --stop-file "$INPUT_CAPTURE_STOP" \
    > "$ARTIFACT_DIR/input-capture-ocr.out" 2>&1 &
  INPUT_CAPTURE_PID=$!
}

stop_input_capture_ocr() {
  (( INPUT_CAPTURE_PID > 0 )) || return 0
  touch "$INPUT_CAPTURE_STOP"
  kill "$INPUT_CAPTURE_PID" 2>/dev/null || true
  wait "$INPUT_CAPTURE_PID" 2>/dev/null || true
  INPUT_CAPTURE_PID=0
  log_timeline "input_capture_ocr_stop"
}

launch_runtime() {
  case "$LAUNCH_MODE" in
    direct)
      log_timeline "launch" "eldenring.exe via Proton"
      (cd "$GAME_DIR" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
      ;;
    direct-protected)
      log_timeline "launch" "start_protected_game.exe via Proton"
      (cd "$GAME_DIR" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" "$PROTON" run "$GAME_DIR/start_protected_game.exe" > "$ARTIFACT_DIR/proton-protected-run.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
      ;;
    steam)
      log_timeline "launch" "steam://rungameid/1245620 via Steam"
      (cd "$GAME_DIR" && ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" steam steam://rungameid/1245620 > "$ARTIFACT_DIR/steam-launch.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
      ;;
    attach-existing)
      runtime_process_rows | awk 'NR == 1 {print $1}' > "$LAUNCH_PID_FILE"
      ;;
    *)
      echo "unknown launch mode: $LAUNCH_MODE" >&2
      return 2
      ;;
  esac
}

watch_readiness() {
  local -a readiness_args=(
    --artifact-dir "$ARTIFACT_DIR"
    --pid-file "$LAUNCH_PID_FILE"
    --telemetry "$TELEMETRY_PATH"
    --bootstrap "$BOOTSTRAP_PATH"
    --bootstrap-state "$BOOTSTRAP_STATE_PATH"
    --target "$RUNTIME_WATCH_TARGET"
    --autoload-attempt-budget "$RUNTIME_AUTOLOAD_ATTEMPT_BUDGET"
    --post-request-tick-budget "$RUNTIME_POST_REQUEST_TICK_BUDGET"
    --spawn-poll-budget "$RUNTIME_SPAWN_POLL_BUDGET"
    --readiness-poll-budget "$RUNTIME_READINESS_POLL_BUDGET"
    --max-runtime-seconds "$RUNTIME_TIMEOUT_SECONDS"
  )
  if [[ -n "$RUNTIME_ALLOW_ASYNC_LAUNCHER_EXIT" ]]; then
    readiness_args+=(--allow-async-launcher-exit)
  fi
  python3 scripts/er-readiness-watch.py "${readiness_args[@]}" > "$ARTIFACT_DIR/driver.out" 2>&1
}

trap cleanup_runtime EXIT

log_timeline "runtime_probe_start" "launch_mode=$LAUNCH_MODE readiness=event-driven timeout_seconds=$RUNTIME_TIMEOUT_SECONDS"
validate_runtime_policy
snapshot_saves "$ARTIFACT_DIR/save-hashes-before.txt"
backup_saves > "$ARTIFACT_DIR/save-backup.log" 2>&1

export ER_EFFECTS_AUTOLOAD_SLOT="${ER_EFFECTS_AUTOLOAD_SLOT:-9}"
export ER_EFFECTS_AUTOLOAD_METHOD="${ER_EFFECTS_AUTOLOAD_METHOD:-direct_menu_load}"
export ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="${ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP:-true}"
export ER_EFFECTS_TRACE_CONTINUE="${ER_EFFECTS_TRACE_CONTINUE:-1}"
export ER_EFFECTS_TRACE_MENU_TASK_UPDATE="${ER_EFFECTS_TRACE_MENU_TASK_UPDATE:-0}"
RUNTIME_WATCH_TARGET="${RUNTIME_WATCH_TARGET:-game-man}"
RUNTIME_AUTOLOAD_ATTEMPT_BUDGET="${RUNTIME_AUTOLOAD_ATTEMPT_BUDGET:-300}"
RUNTIME_POST_REQUEST_TICK_BUDGET="${RUNTIME_POST_REQUEST_TICK_BUDGET:-300}"
RUNTIME_READINESS_POLLS_PER_TASK_TICK="${RUNTIME_READINESS_POLLS_PER_TASK_TICK:-16}"
RUNTIME_READINESS_BASE_POLL_BUDGET="${RUNTIME_READINESS_BASE_POLL_BUDGET:-8192}"
RUNTIME_SPAWN_POLL_BUDGET="${RUNTIME_SPAWN_POLL_BUDGET:-32768}"
RUNTIME_READINESS_POLL_BUDGET="${RUNTIME_READINESS_POLL_BUDGET:-$((RUNTIME_POST_REQUEST_TICK_BUDGET * RUNTIME_READINESS_POLLS_PER_TASK_TICK + RUNTIME_READINESS_BASE_POLL_BUDGET))}"
if [[ "$LAUNCH_MODE" == "steam" ]]; then
  RUNTIME_ALLOW_ASYNC_LAUNCHER_EXIT=1
else
  RUNTIME_ALLOW_ASYNC_LAUNCHER_EXIT="${RUNTIME_ALLOW_ASYNC_LAUNCHER_EXIT:-}"
fi

if [[ "$LAUNCH_MODE" != "attach-existing" ]]; then
  setup_runtime_payload
else
  printf '[runtime_probe] setup phase: attach-existing leaves the running game payload untouched\n' > "$ARTIFACT_DIR/setup.out"
fi

start_input_capture_ocr
RUN_START_MS=$(now_ms)
CLEANUP_ARMED=1
launch_runtime
if watch_readiness; then
  DRIVER_RC=0
else
  DRIVER_RC=$?
fi

cleanup_runtime
if [[ "$RUNTIME_SKIP_FINAL_MEASURE" == "1" ]]; then
  exit "$DRIVER_RC"
fi
AUTO_MEASURE_INNER=1 AUTO_INCLUDE_RUNTIME_EVIDENCE=1 ./.auto/measure.sh
exit "$DRIVER_RC"
