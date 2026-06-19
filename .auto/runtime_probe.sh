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
LAUNCH_MODE="${LAUNCH_MODE:-offline-launcher}"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-30}"
RUNTIME_REPO_DINPUT8="${RUNTIME_REPO_DINPUT8:-1}"
RUNTIME_MAX_NUDGES="${RUNTIME_MAX_NUDGES:-0}"
SCREENSHOT_LLM_MAX_WIDTH="${SCREENSHOT_LLM_MAX_WIDTH:-480}"
SCREENSHOT_LLM_JPEG_QUALITY="${SCREENSHOT_LLM_JPEG_QUALITY:-35}"
RUNTIME_WORLD_STABLE_VISUAL_CHECK="${RUNTIME_WORLD_STABLE_VISUAL_CHECK:-1}"
RUNTIME_WORLD_STABLE_DWELL_SECONDS="${RUNTIME_WORLD_STABLE_DWELL_SECONDS:-5}"
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
NO_CONTINUE_TRACE_PATH="${NO_CONTINUE_TRACE_PATH:-$GAME_DIR/er-effects-no-continue-trace.txt}"
TRACE_MENU_TASK_UPDATE_PATH="${TRACE_MENU_TASK_UPDATE_PATH:-$GAME_DIR/er-effects-trace-menu-task-update.txt}"
TRACE_TITLE_STAGE_PATH="${TRACE_TITLE_STAGE_PATH:-$GAME_DIR/er-effects-trace-title-stage.txt}"
PUMP_MOVE_MAP_PATH="${PUMP_MOVE_MAP_PATH:-$GAME_DIR/er-effects-pump-move-map.txt}"
FORCE_TITLE_STATE_PATH="${FORCE_TITLE_STATE_PATH:-$GAME_DIR/er-effects-force-title-state.txt}"
NATIVE_TITLE_JOB_PATH="${NATIVE_TITLE_JOB_PATH:-$GAME_DIR/er-effects-native-title-job.txt}"
NATIVE_TITLE_TOGGLE_PATH="${NATIVE_TITLE_TOGGLE_PATH:-$GAME_DIR/er-effects-native-title-toggle.txt}"
NATIVE_EXTRA_PARENT_PATH="${NATIVE_EXTRA_PARENT_PATH:-$GAME_DIR/er-effects-native-extra-parent.txt}"
TRACE_TASK_NODE_BYTES_PATH="${TRACE_TASK_NODE_BYTES_PATH:-$GAME_DIR/er-effects-trace-task-node-bytes.txt}"
SELECTBOT_PROBE_PATH="${SELECTBOT_PROBE_PATH:-$GAME_DIR/er-effects-selectbot-probe.txt}"
TITLE_PROCEED_GATE_PATH="${TITLE_PROCEED_GATE_PATH:-$GAME_DIR/er-effects-title-proceed-gate.txt}"
INGAMESTEP_PUMP_PATH="${INGAMESTEP_PUMP_PATH:-$GAME_DIR/er-effects-ingamestep-pump.txt}"
INGAMESTEP_UNPIN_PATH="${INGAMESTEP_UNPIN_PATH:-$GAME_DIR/er-effects-ingamestep-unpin.txt}"
NATIVE_AUTOLOAD_PATH="${NATIVE_AUTOLOAD_PATH:-$GAME_DIR/er-effects-native-autoload.txt}"
INGAMEINIT_DRIVE_PATH="${INGAMEINIT_DRIVE_PATH:-$GAME_DIR/er-effects-ingameinit-drive.txt}"
CONTINUE_DRIVE_PATH="${CONTINUE_DRIVE_PATH:-$GAME_DIR/er-effects-continue-drive.txt}"
ARM_PROBE_PATH="${ARM_PROBE_PATH:-$GAME_DIR/er-effects-arm-probe.txt}"
NATIVE_ARM_LOOP_PATH="${NATIVE_ARM_LOOP_PATH:-$GAME_DIR/er-effects-native-arm-loop.txt}"
TITLE_ACCEPT_PATH="${TITLE_ACCEPT_PATH:-$GAME_DIR/er-effects-title-accept.txt}"
TITLE_ACCEPT_FILL_PATH="${TITLE_ACCEPT_FILL_PATH:-$GAME_DIR/er-effects-title-accept-fill.txt}"
TITLE_ACCEPT_INJECT_PATH="${TITLE_ACCEPT_INJECT_PATH:-$GAME_DIR/er-effects-title-accept-inject.txt}"
SPLASH_SKIP_PATH="${SPLASH_SKIP_PATH:-$GAME_DIR/er-effects-splash-skip.txt}"
SUBMIT_PLAY_GAME_PATH="${SUBMIT_PLAY_GAME_PATH:-$GAME_DIR/er-effects-submit-play-game.txt}"
CRASH_LOG_TRIGGER_PATH="${CRASH_LOG_TRIGGER_PATH:-$GAME_DIR/er-effects-crash-log.txt}"
OWN_STEPPER_PATH="${OWN_STEPPER_PATH:-$GAME_DIR/er-effects-own-stepper.txt}"
DIRECT_BUILD_PATH="${DIRECT_BUILD_PATH:-$GAME_DIR/er-effects-direct-build.txt}"
LIVE_DIALOG_PATH="${LIVE_DIALOG_PATH:-$GAME_DIR/er-effects-live-dialog.txt}"
MENU_WINDOW_LATCH_PATH="${MENU_WINDOW_LATCH_PATH:-$GAME_DIR/er-effects-menu-window-latch.txt}"
D180_UPDATE_PATH="${D180_UPDATE_PATH:-$GAME_DIR/er-effects-d180-update.txt}"
NATIVE_LOAD_PATH="${NATIVE_LOAD_PATH:-$GAME_DIR/er-effects-native-load.txt}"
NATIVE_FULLREAD_PATH="${NATIVE_FULLREAD_PATH:-$GAME_DIR/er-effects-native-fullread.txt}"
FULLREAD_COMMIT_PATH="${FULLREAD_COMMIT_PATH:-$GAME_DIR/er-effects-fullread-commit.txt}"
C30_DIAG_PATH="${C30_DIAG_PATH:-$GAME_DIR/er-effects-c30-diag.txt}"
# The DLL's default crash-log location (when ER_EFFECTS_CRASH_LOG_PATH is unset);
# copied into the artifact dir after the run.
CRASH_LOG_SRC="${CRASH_LOG_SRC:-$GAME_DIR/er-effects-crash.log}"
FORCE_PLAY_GAME_PATH="${FORCE_PLAY_GAME_PATH:-$GAME_DIR/er-effects-force-play-game.txt}"
OFFLINE_LAUNCHER_SRC="${OFFLINE_LAUNCHER_SRC:-$REPO_ROOT/offline-launcher.exe}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"

patch_offline_launcher_gui_subsystem() {
  local launcher_path="$1"
  python3 - "$launcher_path" <<'PY'
import struct
import sys
from pathlib import Path

path = Path(sys.argv[1])
if not path.exists():
    raise SystemExit(0)
data = bytearray(path.read_bytes())
if data[:2] != b"MZ":
    raise SystemExit(f"not an MZ executable: {path}")
pe_offset = struct.unpack_from("<I", data, 0x3c)[0]
if data[pe_offset:pe_offset + 4] != b"PE\0\0":
    raise SystemExit(f"not a PE executable: {path}")
subsystem_offset = pe_offset + 24 + 0x44
subsystem = struct.unpack_from("<H", data, subsystem_offset)[0]
if subsystem == 2:
    print(f"[runtime_probe] offline-launcher already GUI subsystem: {path}")
    raise SystemExit(0)
if subsystem != 3:
    raise SystemExit(f"offline-launcher unexpected subsystem {subsystem}: {path}")
struct.pack_into("<H", data, subsystem_offset, 2)
path.write_bytes(data)
print(f"[runtime_probe] patched offline-launcher subsystem console->GUI: {path}")
PY
}
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
    "manual_load": os.environ.get("RUNTIME_MANUAL_LOAD") == "1",
    "repo_dinput8_payload": os.environ.get("RUNTIME_REPO_DINPUT8", "1") == "1",
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
  python3 - "$ARTIFACT_DIR" "$runtime_process_pattern" "$STEAM_COMPAT_DATA_PATH" <<'PY'
import os
import re
import signal
import subprocess
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
pattern = re.compile(sys.argv[2], re.I)
# Our Proton prefix path; wine infrastructure for this launch carries it in
# environ (WINEPREFIX / STEAM_COMPAT_DATA_PATH) or cmdline. Matching on it lets
# us sweep ONLY this game's wine processes, never another Proton game's.
compat_marker = sys.argv[3]
self_pid = os.getpid()
# Never touch the Steam client itself or its helpers.
protect = re.compile(r"steamwebhelper|/ubuntu12_|(?:^|/)steam(?:\s|$)")
# Only ever SIGKILL actual wine/proton infrastructure. The compat marker also
# appears in the environ of any process that merely inherited
# STEAM_COMPAT_DATA_PATH (this harness's own shell, python, etc.), so requiring a
# wine-infra name first prevents the sweep from killing the harness mid-run.
wine_infra = re.compile(r"\.exe(?:\s|$)|wineserver|wine64|wineboot|preloader|(?:^|/)bwrap(?:\s|$)|pressure-vessel", re.I)

def all_procs():
    output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
    rows = []
    for line in output.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        pid_text, _, args = stripped.partition(" ")
        if pid_text.isdigit():
            rows.append((int(pid_text), args))
    return rows

def game_procs():
    return [(pid, args) for pid, args in all_procs() if pattern.search(args)]

def belongs_to_prefix(pid):
    if not compat_marker:
        return False
    for fname in ("environ", "cmdline"):
        try:
            data = Path(f"/proc/{pid}/{fname}").read_bytes()
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
        # latin-1 never fails and preserves bytes for an ASCII path substring match.
        if compat_marker in data.decode("latin-1"):
            return True
    return False

# Phase 1: the game processes themselves (SIGTERM, then SIGKILL stragglers).
before = game_procs()
(artifact / "teardown-before.txt").write_text("".join(f"{pid} {args}\n" for pid, args in before), encoding="utf-8")
for pid, _ in before:
    try:
        os.kill(pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError):
        pass
for pid, _ in game_procs():
    try:
        os.kill(pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError):
        pass

# Phase 2: wine infrastructure for THIS prefix (winedevice.exe, services.exe,
# explorer.exe, plugplay.exe, wineserver, bwrap, ...). The old teardown left
# these orphaned, and they accumulated across runs until the game task stalled
# early. SIGKILL only those whose environ/cmdline names our compat prefix.
wine_swept = []
for pid, args in all_procs():
    if pid in (self_pid, 1):
        continue
    if protect.search(args):
        continue
    if not wine_infra.search(args):
        continue
    if belongs_to_prefix(pid):
        wine_swept.append((pid, args))
        try:
            os.kill(pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError):
            pass
(artifact / "teardown-wine-swept.txt").write_text("".join(f"{pid} {args}\n" for pid, args in wine_swept), encoding="utf-8")

after = game_procs()
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
  cp -f "$CRASH_LOG_SRC" "$ARTIFACT_DIR/crash.log" 2>/dev/null || true
  # Decode + translate the FromSoft (shift-JIS) assert strings to English so the
  # crash log is readable without reading Japanese.
  if [[ -f "$ARTIFACT_DIR/crash.log" ]]; then
    python3 "$REPO_ROOT/.auto/jp_translate.py" crashlog "$ARTIFACT_DIR/crash.log" \
      > "$ARTIFACT_DIR/crash-translated.log" 2>/dev/null || true
  fi
}

write_expected_slot_oracle() {
  python3 - "$ARTIFACT_DIR" "$REPO_ROOT" "${RUNTIME_EXPECTED_SAVE_PATH:-}" "${ER_EFFECTS_AUTOLOAD_SLOT:-}" <<'PY'
import json
import subprocess
import sys
from pathlib import Path

artifact = Path(sys.argv[1])
repo = Path(sys.argv[2])
explicit_save = sys.argv[3]
env_slot = sys.argv[4]

slot = None
telemetry = {}
for candidate in [artifact / "final-telemetry.json", artifact / "telemetry.json"]:
    if not candidate.exists():
        continue
    try:
        current_telemetry = json.loads(candidate.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        continue
    if not telemetry and isinstance(current_telemetry, dict):
        telemetry = current_telemetry
    for key in ["game_save_slot", "autoload_slot"]:
        value = current_telemetry.get(key)
        if isinstance(value, (int, float)) and 0 <= int(value) < 10:
            slot = int(value)
            break
    if slot is not None:
        break
if slot is None and env_slot:
    try:
        parsed = int(env_slot, 0)
        if 0 <= parsed < 10:
            slot = parsed
    except ValueError:
        pass
if slot is None:
    (artifact / "expected-slot-oracle.log").write_text("slot_unavailable\n", encoding="utf-8")
    raise SystemExit(0)

save_candidates = []
if explicit_save:
    save_candidates.append(Path(explicit_save).expanduser())
manifest_path = artifact / "save-backup" / "manifest.json"
if manifest_path.exists():
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        manifest = []
    if isinstance(manifest, list):
        for entry in manifest:
            if not isinstance(entry, dict):
                continue
            backup = entry.get("backup")
            if backup:
                save_candidates.append(Path(backup))

def save_rank(path: Path) -> tuple[int, str]:
    name = path.name.lower()
    if name.endswith("er0000.co2"):
        return (0, name)
    if name.endswith(".co2"):
        return (1, name)
    if name.endswith("er0000.sl2"):
        return (2, name)
    if name.endswith(".sl2"):
        return (3, name)
    return (9, name)

def as_int(value, default=-1):
    if isinstance(value, bool):
        return int(value)
    if isinstance(value, (int, float)):
        return int(value)
    if isinstance(value, str):
        try:
            return int(value, 0)
        except ValueError:
            return default
    return default

runtime_face_sha = None
face_hex = telemetry.get("oracle_face_data_buffer_hex") if isinstance(telemetry, dict) else None
if isinstance(face_hex, str):
    try:
        import hashlib
        runtime_face_sha = hashlib.sha256(bytes.fromhex(face_hex)).hexdigest()
    except ValueError:
        runtime_face_sha = None

COMPARE_FIELDS = [
    ("saved_map_c30", "oracle_saved_map_c30"),
    ("health", "oracle_char_current_hp"),
    ("max_health", "oracle_char_current_max_hp"),
    ("max_base_health", "oracle_char_base_max_hp"),
    ("fp", "oracle_char_current_fp"),
    ("max_fp", "oracle_char_current_max_fp"),
    ("base_max_fp", "oracle_char_base_max_fp"),
    ("stamina", "oracle_char_current_stamina"),
    ("max_stamina", "oracle_char_current_max_stamina"),
    ("base_max_stamina", "oracle_char_base_max_stamina"),
    ("level", "oracle_char_level"),
    ("runes", "oracle_char_runes"),
    ("rune_memory", "oracle_char_rune_memory"),
    ("chr_type", "oracle_char_chr_type"),
    ("gender", "oracle_char_gender"),
    ("archetype", "oracle_char_archetype"),
    ("voice_type", "oracle_char_voice_type"),
    ("starting_gift", "oracle_char_starting_gift"),
    ("unlocked_talisman_slots", "oracle_char_unlocked_talisman_slots"),
    ("spirit_ash_level", "oracle_char_spirit_ash_level"),
    ("max_crimson_flask_count", "oracle_char_max_crimson_flask_count"),
    ("max_cerulean_flask_count", "oracle_char_max_cerulean_flask_count"),
]

def score_oracle(path: Path) -> tuple[int, int, int, Path, str]:
    tmp = artifact / f"expected-slot-oracle.{path.name}.json"
    cmd = [sys.executable, str(repo / "scripts" / "save-slot-oracle.py"), "--save", str(path), "--slot", str(slot), "--output", str(tmp)]
    run = subprocess.run(cmd, capture_output=True, text=True, timeout=30)
    detail = f"save={path} rc={run.returncode} stdout={run.stdout!r} stderr={run.stderr!r}"
    if run.returncode != 0 or not tmp.exists():
        return (-1_000_000, 0, 1, tmp, detail)
    try:
        payload = json.loads(tmp.read_text(encoding="utf-8", errors="replace"))
    except Exception as exc:
        return (-1_000_000, 0, 1, tmp, f"{detail} json_error={exc}")
    fields = payload.get("decoded_fields")
    if not isinstance(fields, dict) or not fields:
        return (0, 0, 0, tmp, f"{detail} layout={payload.get('layout')} decoded_fields=0")
    score = 1
    compared = 0
    mismatches = 0
    for expected_key, telemetry_key in COMPARE_FIELDS:
        if expected_key not in fields or telemetry_key not in telemetry:
            continue
        compared += 1
        if as_int(fields.get(expected_key), -1) == as_int(telemetry.get(telemetry_key), -1):
            score += 2
        else:
            mismatches += 1
    if fields.get("name") is not None and telemetry.get("oracle_char_name") is not None:
        compared += 1
        if str(fields.get("name")) == str(telemetry.get("oracle_char_name")):
            score += 2
        else:
            mismatches += 1
    if isinstance(fields.get("stats"), list) and isinstance(telemetry.get("oracle_char_stats"), list):
        compared += 1
        if [as_int(value, -1) for value in fields["stats"]] == [as_int(value, -1) for value in telemetry["oracle_char_stats"]]:
            score += 8
        else:
            mismatches += 1
    if isinstance(fields.get("face_body_fields"), dict) and isinstance(telemetry.get("oracle_face_body_fields"), dict):
        compared += 1
        if {key: as_int(value, -1) for key, value in fields["face_body_fields"].items()} == {key: as_int(value, -1) for key, value in telemetry["oracle_face_body_fields"].items()}:
            score += 8
        else:
            mismatches += 1
    if runtime_face_sha and fields.get("face_data_buffer_sha256"):
        compared += 1
        if str(fields.get("face_data_buffer_sha256")) == runtime_face_sha:
            score += 10
        else:
            mismatches += 1
    score -= mismatches * 5
    return (score, compared, mismatches, tmp, f"{detail} layout={payload.get('layout')} score={score} compared={compared} mismatches={mismatches}")

existing_candidates = [path for path in sorted(dict.fromkeys(save_candidates), key=save_rank) if path.exists()]
if not existing_candidates:
    (artifact / "expected-slot-oracle.log").write_text("save_unavailable\n", encoding="utf-8")
    raise SystemExit(0)
results = [score_oracle(path) for path in existing_candidates]
best = max(results, key=lambda item: (item[0], item[1], -item[2]))
output = artifact / "expected-slot-oracle.json"
if best[0] < 0:
    (artifact / "expected-slot-oracle.log").write_text("\n".join(result[4] for result in results) + "\n", encoding="utf-8")
    raise SystemExit(0)
output.write_text(best[3].read_text(encoding="utf-8", errors="replace"), encoding="utf-8")
(artifact / "expected-slot-oracle.log").write_text(
    f"slot={slot}\nchosen={best[3]}\n" + "\n".join(result[4] for result in results) + "\n",
    encoding="utf-8",
)
PY
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
screenshot = artifact / "inworld.png"
llm_image = artifact / "inworld-llm.jpg"
llm_request = artifact / "visual-llm-oracle-request.json"
llm_oracle = artifact / "visual-llm-oracle.json"
visual_llm_world_expected = 0
visual_llm_score = 0
if llm_oracle.exists():
    try:
        visual_payload = json.loads(llm_oracle.read_text(encoding="utf-8", errors="replace"))
        if visual_payload.get("world_expected") is True or visual_payload.get("looks_expected") is True:
            visual_llm_world_expected = 1
            visual_llm_score = 75
    except Exception:
        visual_llm_world_expected = 0
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
    "oracle_world_stable_samples": int(readiness.get("world_stable_samples") or 0),
    "final_screenshot_present": 1 if screenshot.exists() else 0,
    "final_llm_image_present": 1 if llm_image.exists() else 0,
    "visual_llm_request_ready": 1 if llm_request.exists() else 0,
    "visual_llm_world_expected": visual_llm_world_expected,
    "visual_llm_score": visual_llm_score,
}
(artifact / "runtime-metrics.json").write_text(json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(f"runtime_artifact_dir={artifact}")
print(f"driver_rc={driver_rc}")
print(f"er_process_teardown_ok={metrics['er_process_teardown_ok']}")
print(f"save_safety_ok={save_safety_ok}")
PY
}

capture_inworld_screenshot() {
  # Save-safe (read-only) screenshot of ONLY the ER window (class steam_app_1245620), captured at
  # the end of the run BEFORE teardown, to visually confirm the player reached the playable world
  # (the stronger oracle than player_available, which can fire on a loading screen). PRIVACY: query
  # + grim ONLY the ER window by its class; never enumerate or log any other window.
  python3 - "$ARTIFACT_DIR" "$SCREENSHOT_LLM_MAX_WIDTH" "$SCREENSHOT_LLM_JPEG_QUALITY" <<'PY' 2>/dev/null || true
import json, shutil, subprocess, sys
from pathlib import Path
artifact = Path(sys.argv[1])
max_width = int(sys.argv[2])
jpeg_quality = int(sys.argv[3])
log = artifact / "inworld-screenshot.log"
try:
    out = subprocess.run(["hyprctl", "-j", "clients"], capture_output=True, text=True, timeout=5).stdout
    er = [w for w in json.loads(out) if w.get("class") == "steam_app_1245620"]
except Exception as exc:
    log.write_text(f"hyprctl_failed={exc}\n", encoding="utf-8"); raise SystemExit(0)
if not er:
    log.write_text("er_window_not_found\n", encoding="utf-8"); raise SystemExit(0)
win = er[0]
x, y = win["at"]; w, h = win["size"]
geom = f"{x},{y} {w}x{h}"
png = artifact / "inworld.png"
small = artifact / "inworld-llm.jpg"
rc = subprocess.run(["grim", "-g", geom, str(png)], capture_output=True, text=True, timeout=10)
lines = [f"captured geom={geom} grim_rc={rc.returncode} stderr={rc.stderr.strip()}"]
if png.exists():
    magick = shutil.which("magick") or shutil.which("convert")
    if magick:
        resize = f"{max_width}x>"
        cmd = [magick, str(png), "-auto-orient", "-resize", resize, "-quality", str(jpeg_quality), str(small)]
        conv = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        lines.append(f"llm_jpeg={small} rc={conv.returncode} stderr={conv.stderr.strip()}")
    elif shutil.which("ffmpeg"):
        vf = f"scale='min({max_width},iw)':-2"
        cmd = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(png), "-vf", vf, "-q:v", "7", str(small)]
        conv = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
        lines.append(f"llm_jpeg={small} rc={conv.returncode} stderr={conv.stderr.strip()}")
    else:
        lines.append("llm_jpeg_skipped=no_magick_or_ffmpeg")
    request = {
        "image_path": str(small if small.exists() else png),
        "source_png": str(png),
        "prompt": (
            "This is a small compressed final Elden Ring runtime screenshot captured only after "
            "the structured map/character/runtime oracles fired. Answer JSON with "
            "world_expected=true when it looks like an in-world playable Elden Ring scene rather "
            "than a title menu, loading screen, crash dialog, or unrelated window."
        ),
        "expected_json_path": str(artifact / "visual-llm-oracle.json"),
    }
    (artifact / "visual-llm-oracle-request.json").write_text(json.dumps(request, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    oracle = artifact / "visual-llm-oracle.json"
    if not oracle.exists():
        try:
            previews = []
            for check_path in sorted(artifact.glob("world-stable-visual-check-*.json")):
                payload = json.loads(check_path.read_text(encoding="utf-8", errors="replace"))
                previews.append(str(payload.get("ocr_preview", "")))
                for item in payload.get("ocr_results", []):
                    previews.append(str(item.get("ocr_preview", "")))
            ocr_text = "\n".join(previews).upper()
            banned_terms = ["ELDEN RING", "LOADING", "PRESS ANY", "PRESS ANY BUTTON", "YOU DIED", "FAILED", "ERROR"]
            banned_hits = [term for term in banned_terms if term in ocr_text]
            features = {
                "banned_hits": banned_hits,
                "hud_red_pixels": 0,
                "hud_green_pixels": 0,
                "hud_dark_fraction": 1.0,
                "hud_pixel_count": 0,
            }
            magick_for_oracle = magick or shutil.which("magick") or shutil.which("convert")
            if magick_for_oracle:
                raw = subprocess.run(
                    [magick_for_oracle, str(png), "-crop", "220x80+0+0", "+repage", "-depth", "8", "rgb:-"],
                    capture_output=True,
                    check=True,
                    timeout=10,
                ).stdout
                pixel_count = len(raw) // 3
                red_pixels = green_pixels = dark_pixels = 0
                for offset in range(0, len(raw), 3):
                    r, g, b = raw[offset], raw[offset + 1], raw[offset + 2]
                    if r > 90 and r > g * 1.25 and r > b * 1.25:
                        red_pixels += 1
                    if g > 80 and g > r * 1.10 and g > b * 1.10:
                        green_pixels += 1
                    if r + g + b < 60:
                        dark_pixels += 1
                features.update(
                    {
                        "hud_red_pixels": red_pixels,
                        "hud_green_pixels": green_pixels,
                        "hud_dark_fraction": (dark_pixels / pixel_count) if pixel_count else 1.0,
                        "hud_pixel_count": pixel_count,
                    }
                )
            hud_present = (
                features["hud_red_pixels"] >= 120
                and features["hud_green_pixels"] >= 40
                and features["hud_dark_fraction"] <= 0.85
            )
            world_expected = bool(hud_present and not banned_hits)
            if world_expected or banned_hits:
                oracle.write_text(
                    json.dumps(
                        {
                            "world_expected": world_expected,
                            "looks_expected": world_expected,
                            "method": "deterministic_hud_ocr_fallback",
                            "reason": "HUD health/stamina bars present and no title/loading OCR text"
                            if world_expected
                            else "title/loading/error OCR text present or HUD evidence absent",
                            "features": features,
                        },
                        indent=2,
                        sort_keys=True,
                    )
                    + "\n",
                    encoding="utf-8",
                )
                lines.append(f"visual_oracle_fallback={world_expected} features={features}")
            else:
                lines.append(f"visual_oracle_fallback=undecided features={features}")
        except Exception as exc:
            lines.append(f"visual_oracle_fallback_failed={exc}")
log.write_text("\n".join(lines) + "\n", encoding="utf-8")
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
  if [[ "${ER_EFFECTS_NO_CONTINUE_TRACE:-0}" == "1" ]]; then
    rm -f "$NO_CONTINUE_TRACE_PATH"
  fi
  if [[ "${ER_EFFECTS_TRACE_TASK_NODE_BYTES:-0}" == "1" ]]; then
    rm -f "$TRACE_TASK_NODE_BYTES_PATH"
  fi
  if [[ "${ER_EFFECTS_SELECTBOT_PROBE:-0}" == "1" ]]; then
    rm -f "$SELECTBOT_PROBE_PATH"
  fi
  if [[ "${ER_EFFECTS_TITLE_PROCEED_GATE:-0}" == "1" ]]; then
    rm -f "$TITLE_PROCEED_GATE_PATH"
  fi
  if [[ "${ER_EFFECTS_INGAMESTEP_PUMP:-0}" == "1" ]]; then
    rm -f "$INGAMESTEP_PUMP_PATH" "$FORCE_PLAY_GAME_PATH"
  fi
  if [[ "${ER_EFFECTS_INGAMESTEP_UNPIN:-0}" == "1" ]]; then
    rm -f "$INGAMESTEP_UNPIN_PATH"
  fi
  if [[ "${ER_EFFECTS_NATIVE_AUTOLOAD:-0}" == "1" ]]; then
    rm -f "$NATIVE_AUTOLOAD_PATH"
  fi
  if [[ "${ER_EFFECTS_INGAMEINIT_DRIVE:-0}" == "1" ]]; then
    rm -f "$INGAMEINIT_DRIVE_PATH"
  fi
  if [[ "${ER_EFFECTS_CONTINUE_DRIVE:-0}" == "1" ]]; then
    rm -f "$CONTINUE_DRIVE_PATH"
  fi
  if [[ "${ER_EFFECTS_ARM_PROBE:-0}" == "1" ]]; then
    rm -f "$ARM_PROBE_PATH"
  fi
  if [[ "${ER_EFFECTS_NATIVE_ARM_LOOP:-0}" == "1" ]]; then
    rm -f "$NATIVE_ARM_LOOP_PATH"
  fi
  if [[ "${ER_EFFECTS_TITLE_ACCEPT:-0}" == "1" ]]; then
    rm -f "$TITLE_ACCEPT_PATH"
  fi
  if [[ "${ER_EFFECTS_SPLASH_SKIP:-0}" == "1" ]]; then
    rm -f "$SPLASH_SKIP_PATH"
  fi
  if [[ "${ER_EFFECTS_SUBMIT_PLAY_GAME:-0}" == "1" ]]; then
    rm -f "$SUBMIT_PLAY_GAME_PATH"
  fi
  if [[ "${ER_EFFECTS_TITLE_ACCEPT_FILL:-0}" == "1" ]]; then
    rm -f "$TITLE_ACCEPT_FILL_PATH"
  fi
  if [[ "${ER_EFFECTS_TITLE_ACCEPT_INJECT:-0}" == "1" ]]; then
    rm -f "$TITLE_ACCEPT_INJECT_PATH"
  fi
  if [[ "${ER_EFFECTS_CRASH_LOG:-0}" == "1" ]]; then
    rm -f "$CRASH_LOG_TRIGGER_PATH"
  fi
  if [[ "${ER_EFFECTS_OWN_STEPPER:-0}" == "1" ]]; then
    rm -f "$OWN_STEPPER_PATH"
  fi
  if [[ "${ER_EFFECTS_DIRECT_BUILD:-0}" == "1" ]]; then
    rm -f "$DIRECT_BUILD_PATH"
  fi
  if [[ "${ER_EFFECTS_LIVE_DIALOG:-0}" == "1" ]]; then
    rm -f "$LIVE_DIALOG_PATH"
  fi
  if [[ "${ER_EFFECTS_MENU_WINDOW_LATCH:-0}" == "1" ]]; then
    rm -f "$MENU_WINDOW_LATCH_PATH"
  fi
  if [[ "${ER_EFFECTS_D180_UPDATE:-0}" == "1" ]]; then
    rm -f "$D180_UPDATE_PATH"
  fi
  if [[ "${ER_EFFECTS_NATIVE_LOAD:-0}" == "1" ]]; then
    rm -f "$NATIVE_LOAD_PATH"
  fi
  if [[ "${ER_EFFECTS_NATIVE_FULLREAD:-0}" == "1" ]]; then
    rm -f "$NATIVE_FULLREAD_PATH"
  fi
  if [[ "${ER_EFFECTS_FULLREAD_COMMIT:-0}" == "1" ]]; then
    rm -f "$FULLREAD_COMMIT_PATH"
  fi
  if [[ "${ER_EFFECTS_C30_DIAG:-0}" == "1" ]]; then
    rm -f "$C30_DIAG_PATH"
  fi
  copy_runtime_logs || true
  write_state_snapshot "$ARTIFACT_DIR/final-state-before-cleanup.json" || true
  teardown_runtime_processes || true
  snapshot_saves "$ARTIFACT_DIR/save-hashes-after-pre-restore.txt"
  restore_saves_from_backup || true
  snapshot_saves "$ARTIFACT_DIR/save-hashes-after.txt"
  copy_runtime_logs || true
  stop_input_capture_ocr || true
  write_expected_slot_oracle || true
  write_runtime_metrics || true
  write_state_snapshot "$ARTIFACT_DIR/final-state-after-cleanup.json" || true
  log_timeline "cleanup_finish"
}

setup_runtime_payload() {
  if [[ "${RUNTIME_NO_DLL:-0}" == "1" ]]; then
    # Vanilla baseline: remove the LazyLoader proxy so NO DLL is injected, to
    # isolate "our DLL crashes the boot" from "the environment is degraded".
    printf '[runtime_probe] RUNTIME_NO_DLL=1: vanilla boot, no DLL injection\n'
    rm -f "$GAME_DIR/dinput8.dll"
    return 0
  fi
  local lazyloader_dir
  lazyloader_dir="${LAZYLOADER_DIR:-$GAME_DIR/dllMods.disabled/lazyloader-20260611-234916}"
  {
    printf '[runtime_probe] setup phase: build DLL and install runtime payload\n'
    cargo xwin build --target x86_64-pc-windows-msvc --release
    if [[ "$RUNTIME_REPO_DINPUT8" == "1" ]]; then
      printf '[runtime_probe] payload: repo-built dinput8.dll proxy only; removing LazyLoader config\n'
      cp -f "$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll" "$GAME_DIR/dinput8.dll"
      rm -f "$GAME_DIR/lazyLoad.ini" "$GAME_DIR/dllMods/er_effects_rs.dll"
      sha256sum "$GAME_DIR/dinput8.dll" > "$ARTIFACT_DIR/repo-dinput8.sha256"
    else
      printf '[runtime_probe] payload: LazyLoader plus repo DLL (non repo-only compatibility mode)\n'
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
    fi
    # Offline launcher (launch_modded_eldenring): finds eldenring.exe in its CWD and
    # boots it offline with SteamAppId set, so DInput/Steam init properly (avoids the
    # raw direct-mode early-exit + input-hook AV). Installed next to eldenring.exe.
    if [[ -f "$OFFLINE_LAUNCHER_SRC" ]]; then
      cp -f "$OFFLINE_LAUNCHER_SRC" "$GAME_DIR/offline-launcher.exe"
      patch_offline_launcher_gui_subsystem "$GAME_DIR/offline-launcher.exe"
    fi
    if [[ "${RUNTIME_MANUAL_LOAD:-0}" == "1" ]]; then
      rm -f "$AUTOLOAD_PATH"
      printf 'manual_load=1\n' > "$ARTIFACT_DIR/autoload-request.txt"
    else
      {
        printf 'slot=%s\n' "$ER_EFFECTS_AUTOLOAD_SLOT"
        printf 'method=%s\n' "$ER_EFFECTS_AUTOLOAD_METHOD"
        printf 'require_title_bootstrap=%s\n' "$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP"
      } > "$AUTOLOAD_PATH"
    fi
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
    if [[ "${ER_EFFECTS_NO_CONTINUE_TRACE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NO_CONTINUE_TRACE_PATH"
      cp -f "$NO_CONTINUE_TRACE_PATH" "$ARTIFACT_DIR/no-continue-trace-request.txt"
    else
      rm -f "$NO_CONTINUE_TRACE_PATH"
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
    if [[ "${ER_EFFECTS_TITLE_PROCEED_GATE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TITLE_PROCEED_GATE_PATH"
      cp -f "$TITLE_PROCEED_GATE_PATH" "$ARTIFACT_DIR/title-proceed-gate-request.txt"
    else
      rm -f "$TITLE_PROCEED_GATE_PATH"
    fi
    if [[ "${ER_EFFECTS_INGAMESTEP_PUMP:-0}" == "1" ]]; then
      # The pump piggybacks the InGameStep tick onto force_play_game's submit,
      # so both triggers are dropped together.
      printf 'enabled=1\n' > "$INGAMESTEP_PUMP_PATH"
      printf 'enabled=1\n' > "$FORCE_PLAY_GAME_PATH"
      cp -f "$INGAMESTEP_PUMP_PATH" "$ARTIFACT_DIR/ingamestep-pump-request.txt"
    else
      rm -f "$INGAMESTEP_PUMP_PATH"
    fi
    if [[ "${ER_EFFECTS_INGAMESTEP_UNPIN:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$INGAMESTEP_UNPIN_PATH"
      cp -f "$INGAMESTEP_UNPIN_PATH" "$ARTIFACT_DIR/ingamestep-unpin-request.txt"
    else
      rm -f "$INGAMESTEP_UNPIN_PATH"
    fi
    if [[ "${ER_EFFECTS_NATIVE_AUTOLOAD:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_AUTOLOAD_PATH"
      cp -f "$NATIVE_AUTOLOAD_PATH" "$ARTIFACT_DIR/native-autoload-request.txt"
    else
      rm -f "$NATIVE_AUTOLOAD_PATH"
    fi
    if [[ "${ER_EFFECTS_INGAMEINIT_DRIVE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$INGAMEINIT_DRIVE_PATH"
      cp -f "$INGAMEINIT_DRIVE_PATH" "$ARTIFACT_DIR/ingameinit-drive-request.txt"
    else
      rm -f "$INGAMEINIT_DRIVE_PATH"
    fi
    if [[ "${ER_EFFECTS_CONTINUE_DRIVE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$CONTINUE_DRIVE_PATH"
      cp -f "$CONTINUE_DRIVE_PATH" "$ARTIFACT_DIR/continue-drive-request.txt"
    else
      rm -f "$CONTINUE_DRIVE_PATH"
    fi
    if [[ "${ER_EFFECTS_ARM_PROBE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$ARM_PROBE_PATH"
    else
      rm -f "$ARM_PROBE_PATH"
    fi
    if [[ "${ER_EFFECTS_NATIVE_ARM_LOOP:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_ARM_LOOP_PATH"
    else
      rm -f "$NATIVE_ARM_LOOP_PATH"
    fi
    if [[ "${ER_EFFECTS_TITLE_ACCEPT:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TITLE_ACCEPT_PATH"
    else
      rm -f "$TITLE_ACCEPT_PATH"
    fi
    if [[ "${ER_EFFECTS_SPLASH_SKIP:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$SPLASH_SKIP_PATH"
    else
      rm -f "$SPLASH_SKIP_PATH"
    fi
    if [[ "${ER_EFFECTS_SUBMIT_PLAY_GAME:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$SUBMIT_PLAY_GAME_PATH"
    else
      rm -f "$SUBMIT_PLAY_GAME_PATH"
    fi
    if [[ "${ER_EFFECTS_TITLE_ACCEPT_FILL:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TITLE_ACCEPT_FILL_PATH"
    else
      rm -f "$TITLE_ACCEPT_FILL_PATH"
    fi
    if [[ "${ER_EFFECTS_TITLE_ACCEPT_INJECT:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$TITLE_ACCEPT_INJECT_PATH"
    else
      rm -f "$TITLE_ACCEPT_INJECT_PATH"
    fi
    if [[ "${ER_EFFECTS_CRASH_LOG:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$CRASH_LOG_TRIGGER_PATH"
    else
      rm -f "$CRASH_LOG_TRIGGER_PATH"
    fi
    if [[ "${ER_EFFECTS_OWN_STEPPER:-0}" == "1" ]]; then
      {
        printf 'enabled=1\n'
        printf 'slot=%s\n' "$ER_EFFECTS_AUTOLOAD_SLOT"
      } > "$OWN_STEPPER_PATH"
      cp -f "$OWN_STEPPER_PATH" "$ARTIFACT_DIR/own-stepper-request.txt"
    else
      rm -f "$OWN_STEPPER_PATH"
    fi
    if [[ "${ER_EFFECTS_DIRECT_BUILD:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$DIRECT_BUILD_PATH"
      cp -f "$DIRECT_BUILD_PATH" "$ARTIFACT_DIR/direct-build-request.txt"
    else
      rm -f "$DIRECT_BUILD_PATH"
    fi
    if [[ "${ER_EFFECTS_LIVE_DIALOG:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$LIVE_DIALOG_PATH"
      cp -f "$LIVE_DIALOG_PATH" "$ARTIFACT_DIR/live-dialog-request.txt"
    else
      rm -f "$LIVE_DIALOG_PATH"
    fi
    if [[ "${ER_EFFECTS_MENU_WINDOW_LATCH:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$MENU_WINDOW_LATCH_PATH"
      cp -f "$MENU_WINDOW_LATCH_PATH" "$ARTIFACT_DIR/menu-window-latch-request.txt"
    else
      rm -f "$MENU_WINDOW_LATCH_PATH"
    fi
    if [[ "${ER_EFFECTS_D180_UPDATE:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$D180_UPDATE_PATH"
      cp -f "$D180_UPDATE_PATH" "$ARTIFACT_DIR/d180-update-request.txt"
    else
      rm -f "$D180_UPDATE_PATH"
    fi
    if [[ "${ER_EFFECTS_NATIVE_LOAD:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_LOAD_PATH"
      cp -f "$NATIVE_LOAD_PATH" "$ARTIFACT_DIR/native-load-request.txt"
    else
      rm -f "$NATIVE_LOAD_PATH"
    fi
    if [[ "${ER_EFFECTS_NATIVE_FULLREAD:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$NATIVE_FULLREAD_PATH"
      cp -f "$NATIVE_FULLREAD_PATH" "$ARTIFACT_DIR/native-fullread-request.txt"
    else
      rm -f "$NATIVE_FULLREAD_PATH"
    fi
    if [[ "${ER_EFFECTS_FULLREAD_COMMIT:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$FULLREAD_COMMIT_PATH"
      cp -f "$FULLREAD_COMMIT_PATH" "$ARTIFACT_DIR/fullread-commit-request.txt"
    else
      rm -f "$FULLREAD_COMMIT_PATH"
    fi
    if [[ "${ER_EFFECTS_C30_DIAG:-0}" == "1" ]]; then
      printf 'enabled=1\n' > "$C30_DIAG_PATH"
      cp -f "$C30_DIAG_PATH" "$ARTIFACT_DIR/c30-diag-request.txt"
    else
      rm -f "$C30_DIAG_PATH"
    fi
    rm -f "$TELEMETRY_PATH" "$COMMAND_PATH" "$AUTOLOAD_DEBUG_PATH" "$TRACE_CONTINUE_PATH" "$BOOTSTRAP_PATH" "$BOOTSTRAP_STATE_PATH" "$CRASH_LOG_SRC"
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
  # Opt-in: let Wine log unhandled SEH exceptions (with backtraces) to the proton
  # output, complementing the in-DLL crash logger. Inherited by the direct-mode
  # game process; harmless otherwise.
  if [[ "${RUNTIME_SEH_TRACE:-0}" == "1" ]]; then
    export WINEDEBUG="+seh,+tid"
  fi
  case "$LAUNCH_MODE" in
    direct)
      log_timeline "launch" "eldenring.exe via Proton"
      (cd "$GAME_DIR" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
      ;;
    direct-protected)
      log_timeline "launch" "start_protected_game.exe via Proton"
      (cd "$GAME_DIR" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" "$PROTON" run "$GAME_DIR/start_protected_game.exe" > "$ARTIFACT_DIR/proton-protected-run.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
      ;;
    offline-launcher)
      log_timeline "launch" "offline-launcher.exe (launch_modded_eldenring) via Proton"
      (cd "$GAME_DIR" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" "$PROTON" run "$GAME_DIR/offline-launcher.exe" > "$ARTIFACT_DIR/proton-offline-launcher.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
      ;;
    seamless)
      log_timeline "launch" "ersc_launcher.exe (Seamless Co-op) via Proton -- ersc.dll + dllMods/er_effects_rs.dll both load"
      (cd "$GAME_DIR" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" ER_EFFECTS_TELEMETRY_PATH="$TELEMETRY_PATH" ER_EFFECTS_COMMAND_PATH="$COMMAND_PATH" ER_EFFECTS_AUTOLOAD_PATH="$AUTOLOAD_PATH" ER_EFFECTS_SAFE_INPUT_PATH="$SAFE_INPUT_PATH" ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$AUTOLOAD_DEBUG_PATH" ER_EFFECTS_TRACE_CONTINUE_PATH="$TRACE_CONTINUE_PATH" ER_EFFECTS_BOOTSTRAP_PATH="$BOOTSTRAP_PATH" ER_EFFECTS_BOOTSTRAP_STATE_PATH="$BOOTSTRAP_STATE_PATH" ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="$ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP" "$PROTON" run "$GAME_DIR/ersc_launcher.exe" > "$ARTIFACT_DIR/proton-seamless-launcher.out" 2>&1 & echo $! > "$LAUNCH_PID_FILE")
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
    --world-stable-samples "$RUNTIME_WORLD_STABLE_SAMPLES"
    --world-stable-dwell-seconds "$RUNTIME_WORLD_STABLE_DWELL_SECONDS"
    --spawn-poll-budget "$RUNTIME_SPAWN_POLL_BUDGET"
    --readiness-poll-budget "$RUNTIME_READINESS_POLL_BUDGET"
    --max-runtime-seconds "$RUNTIME_TIMEOUT_SECONDS"
  )
  if [[ -n "$RUNTIME_ALLOW_ASYNC_LAUNCHER_EXIT" ]]; then
    readiness_args+=(--allow-async-launcher-exit)
  fi
  if [[ "$RUNTIME_WATCH_TARGET" == "world-stable" && "$RUNTIME_WORLD_STABLE_VISUAL_CHECK" == "1" ]]; then
    readiness_args+=(--visual-world-check)
  fi
  python3 scripts/er-readiness-watch.py "${readiness_args[@]}" > "$ARTIFACT_DIR/driver.out" 2>&1
}

trap cleanup_runtime EXIT

log_timeline "runtime_probe_start" "launch_mode=$LAUNCH_MODE readiness=event-driven timeout_seconds=$RUNTIME_TIMEOUT_SECONDS"
validate_runtime_policy
snapshot_saves "$ARTIFACT_DIR/save-hashes-before.txt"
backup_saves > "$ARTIFACT_DIR/save-backup.log" 2>&1

if [[ "${RUNTIME_MANUAL_LOAD:-0}" != "1" ]]; then
  export ER_EFFECTS_AUTOLOAD_SLOT="${ER_EFFECTS_AUTOLOAD_SLOT:-9}"
fi
export ER_EFFECTS_AUTOLOAD_METHOD="${ER_EFFECTS_AUTOLOAD_METHOD:-direct_menu_load}"
export ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP="${ER_EFFECTS_AUTOLOAD_REQUIRE_TITLE_BOOTSTRAP:-true}"
export ER_EFFECTS_TRACE_CONTINUE="${ER_EFFECTS_TRACE_CONTINUE:-1}"
export ER_EFFECTS_TRACE_MENU_TASK_UPDATE="${ER_EFFECTS_TRACE_MENU_TASK_UPDATE:-0}"
RUNTIME_WATCH_TARGET="${RUNTIME_WATCH_TARGET:-game-man}"
RUNTIME_AUTOLOAD_ATTEMPT_BUDGET="${RUNTIME_AUTOLOAD_ATTEMPT_BUDGET:-300}"
RUNTIME_POST_REQUEST_TICK_BUDGET="${RUNTIME_POST_REQUEST_TICK_BUDGET:-300}"
RUNTIME_WORLD_STABLE_SAMPLES="${RUNTIME_WORLD_STABLE_SAMPLES:-3}"
RUNTIME_READINESS_POLLS_PER_TASK_TICK="${RUNTIME_READINESS_POLLS_PER_TASK_TICK:-16}"
RUNTIME_READINESS_BASE_POLL_BUDGET="${RUNTIME_READINESS_BASE_POLL_BUDGET:-8192}"
RUNTIME_SPAWN_POLL_BUDGET="${RUNTIME_SPAWN_POLL_BUDGET:-32768}"
RUNTIME_READINESS_POLL_BUDGET="${RUNTIME_READINESS_POLL_BUDGET:-$((RUNTIME_POST_REQUEST_TICK_BUDGET * RUNTIME_READINESS_POLLS_PER_TASK_TICK + RUNTIME_READINESS_BASE_POLL_BUDGET))}"
if [[ "$LAUNCH_MODE" == "steam" || "$LAUNCH_MODE" == "offline-launcher" || "$LAUNCH_MODE" == "seamless" ]]; then
  # The offline/Seamless launcher (like the Steam client) forks eldenring.exe and exits, so
  # the recorded launcher PID dying is expected -- the readiness watcher must track
  # the spawned game process instead of treating launcher exit as failure.
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

capture_inworld_screenshot
cleanup_runtime
if [[ "$RUNTIME_SKIP_FINAL_MEASURE" == "1" ]]; then
  exit "$DRIVER_RC"
fi
AUTO_MEASURE_INNER=1 AUTO_INCLUDE_RUNTIME_EVIDENCE=1 AUTO_RUNTIME_FAST_SCORE=1 ./.auto/measure.sh
exit "$DRIVER_RC"
