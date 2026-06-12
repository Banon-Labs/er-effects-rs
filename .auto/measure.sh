#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  # Non-interactive agent shells may not have Cargo on PATH.
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

LOG_DIR="$REPO_ROOT/.auto/last-measure"
mkdir -p "$LOG_DIR"
rm -f "$LOG_DIR"/*.log 2>/dev/null || true

GATE_FAILED=0
TEST_PASS=1
BUILD_SECONDS="0"

now_ms() {
  python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
}

run_gate() {
  local name=$1
  shift
  local start end elapsed rc log_path
  log_path="$LOG_DIR/$name.log"
  start=$(now_ms)
  echo "[measure] gate $name: $*" >&2
  if "$@" >"$log_path" 2>&1; then
    rc=0
  else
    rc=$?
    GATE_FAILED=1
    TEST_PASS=0
    echo "[measure] gate $name failed with rc=$rc; log=$log_path" >&2
    tail -80 "$log_path" >&2 || true
  fi
  end=$(now_ms)
  elapsed=$((end - start))
  if [[ "$name" == "xwin_check" ]]; then
    BUILD_SECONDS=$(python3 - <<PY
print(f"{$elapsed / 1000:.3f}")
PY
)
  fi
  return 0
}

run_gate cargo_fmt cargo fmt --check
run_gate cargo_tests cargo test -p er-safe-input -p er-save-loader
run_gate xwin_check cargo xwin check --target x86_64-pc-windows-msvc --no-default-features
run_gate shellcheck_scripts shellcheck scripts/er-smoke-driver.sh target/validate-cupcake-bash-guards.sh
run_gate cupcake_guards target/validate-cupcake-bash-guards.sh
run_gate smoke_preflight scripts/er-smoke-driver.sh preflight --no-build --no-install --no-launch --max-nudges 0

ER_PROCESS_COUNT=$(python3 - <<'PY'
import re
import subprocess
pattern = re.compile(r'(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)', re.I)
count = 0
output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
for line in output.splitlines():
    if pattern.search(line):
        count += 1
print(count)
PY
)
if [[ "$ER_PROCESS_COUNT" != "0" ]]; then
  GATE_FAILED=1
  TEST_PASS=0
  echo "[measure] hard gate failed: Elden Ring process still running" >&2
  python3 - <<'PY' >&2
import re
import subprocess
pattern = re.compile(r'(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)', re.I)
output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
for line in output.splitlines():
    if pattern.search(line):
        print(line)
PY
fi

export REPO_ROOT GATE_FAILED TEST_PASS BUILD_SECONDS ER_PROCESS_COUNT
python3 - <<'PY'
import json
import os
import re
import subprocess
from pathlib import Path

repo = Path(os.environ["REPO_ROOT"])
gate_failed = int(os.environ.get("GATE_FAILED", "0"))
test_pass = int(os.environ.get("TEST_PASS", "0"))
build_seconds = float(os.environ.get("BUILD_SECONDS", "0") or 0)
er_process_count = int(os.environ.get("ER_PROCESS_COUNT", "0") or 0)

metrics = {
    "autoload_success": 0,
    "player_available": 0,
    "selected_slot_loaded": 0,
    "time_to_player_seconds": -1,
    "game_save_state": -1,
    "game_save_slot": -1,
    "game_requested_save_slot_load_index": -1,
    "game_save_requested": 0,
    "title_bootstrap_seen": 0,
    "native_request_consumed": 0,
    "crash_detected": 0,
    "save_safety_ok": 1,
    "er_process_teardown_ok": 1 if er_process_count == 0 else 0,
    "host_pointer_input_used": 0,
    "trace_invasiveness_score": 0,
    "static_evidence_score": 0,
    "runtime_probe_seconds": 0,
    "build_seconds": build_seconds,
    "test_pass": test_pass,
    "code_complexity_delta": 0,
    "artifact_bytes": 0,
    "false_positives": 0,
}

src = (repo / "src/lib.rs").read_text(encoding="utf-8", errors="replace") if (repo / "src/lib.rs").exists() else ""
loader = (repo / "crates/er-save-loader/src/lib.rs").read_text(encoding="utf-8", errors="replace") if (repo / "crates/er-save-loader/src/lib.rs").exists() else ""
safe_input = (repo / "crates/er-safe-input/src/lib.rs").read_text(encoding="utf-8", errors="replace") if (repo / "crates/er-safe-input/src/lib.rs").exists() else ""
smoke_driver = (repo / "scripts/er-smoke-driver.sh").read_text(encoding="utf-8", errors="replace") if (repo / "scripts/er-smoke-driver.sh").exists() else ""
static_score = 0
if all(needle in src for needle in ["0xb72", "0xb73", "0xb78", "0xbc4", "game_man_trace_summary"]):
    static_score += 80
if all(needle in src for needle in ["MENU_CONTINUE_WRAPPER_RVA", "menu_continue_wrapper_hook", "TITLE_BOOTSTRAP_SEEN"]):
    static_score += 80
if all(needle in src for needle in ["SET_SAVE_SLOT_RVA", "SAVE_REQUEST_PROFILE_RVA", "REQUEST_SAVE_RVA", "CURRENT_SLOT_LOAD_RVA", "CONTINUE_LOAD_RVA"]):
    static_score += 80
if all(needle in loader for needle in ["DirectMenuLoad", "request_direct_menu_load", "save_buffer_allocator_ready", "title_bootstrap_seen"]):
    static_score += 80
if all(needle in src for needle in ["game_requested_save_slot_load_index", "game_save_state", "game_save_requested", "autoload_last_status"]):
    static_score += 80
# Bonus static evidence for the exact high-priority addresses when future RE notes/code add them.
combined_static_text = "\n".join([src, loader])
for needle in ["0x140af7a50", "0x140afab5f", "0x140afab6a", "0x140af1aa0", "0x1406793c0"]:
    if needle.lower() in combined_static_text.lower():
        static_score += 20
metrics["static_evidence_score"] = static_score

# Gated tracing is present but not always active. Runtime trace artifacts raise this score.
if "install_continue_trace_hooks" in src:
    metrics["trace_invasiveness_score"] = 5

# Complexity delta is advisory only: changed lines relative to HEAD.
try:
    diff = subprocess.check_output(["git", "diff", "--numstat"], cwd=repo, text=True, stderr=subprocess.DEVNULL)
    total = 0
    for line in diff.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2 and parts[0].isdigit() and parts[1].isdigit():
            total += int(parts[0]) + int(parts[1])
    metrics["code_complexity_delta"] = total
except Exception:
    pass

# Latest structured runtime evidence, if any. The default measurement does not launch ER.
artifact_dir = None
telemetry_path = None
candidates = []
for pattern in ["target/smoke/**/final-telemetry.json", "target/smoke/**/telemetry.json"]:
    candidates.extend(repo.glob(pattern))
if candidates:
    telemetry_path = max(candidates, key=lambda path: path.stat().st_mtime)
    artifact_dir = telemetry_path.parent
    try:
        telemetry = json.loads(telemetry_path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        telemetry = {}
else:
    telemetry = {}

if artifact_dir and artifact_dir.exists():
    metrics["artifact_bytes"] = sum(path.stat().st_size for path in artifact_dir.rglob("*") if path.is_file())
    logs = []
    for pattern in ["*.log", "*.txt", "*.out"]:
        for path in artifact_dir.glob(pattern):
            try:
                logs.append(path.read_text(encoding="utf-8", errors="replace"))
            except Exception:
                pass
    joined_logs = "\n".join(logs)
    if re.search(r"--allow-pointer-input|ALLOW_POINTER_INPUT=1|ydotool mousemove|click_center_ok", joined_logs):
        metrics["host_pointer_input_used"] = 1
    if re.search(r"save corruption|unquarantined save|destructive save", joined_logs, re.I):
        metrics["save_safety_ok"] = 0
    if re.search(r"crash|exception|fatal", joined_logs, re.I):
        metrics["crash_detected"] = 1
    if "continue-trace" in joined_logs or "ENTER menu_continue_wrapper" in joined_logs:
        metrics["trace_invasiveness_score"] = max(metrics["trace_invasiveness_score"], 20)
    if re.search(r"LEAVE map_load_67bc10 ret=1", joined_logs):
        metrics["title_bootstrap_seen"] = 1
    queued_load_request = bool(re.search(r"queuing traced continue flags|direct continue sequence requested", joined_logs))
    load_hook_seen = bool(re.search(r"ENTER (current_slot_load_67b570|continue_load_67b750|combined_load_67b940|map_load_67bc10|save_load_state_init_67b030)", joined_logs))
    trace_confirms_state_transition = bool(load_hook_seen and re.search(r"state=(?!0\b)\d+", joined_logs))
else:
    joined_logs = ""
    queued_load_request = False
    load_hook_seen = False
    trace_confirms_state_transition = False

if telemetry:
    metrics["player_available"] = 1 if telemetry.get("player_available") is True else 0
    for key, metric_name in [
        ("game_save_state", "game_save_state"),
        ("game_save_slot", "game_save_slot"),
        ("game_requested_save_slot_load_index", "game_requested_save_slot_load_index"),
    ]:
        value = telemetry.get(key)
        if isinstance(value, bool):
            metrics[metric_name] = int(value)
        elif isinstance(value, (int, float)):
            metrics[metric_name] = value
    metrics["game_save_requested"] = 1 if telemetry.get("game_save_requested") is True else 0
    slot = telemetry.get("autoload_slot")
    game_slot = telemetry.get("game_save_slot")
    if isinstance(slot, (int, float)) and isinstance(game_slot, (int, float)) and int(slot) == int(game_slot):
        metrics["selected_slot_loaded"] = 1 if metrics["player_available"] else 0
    if metrics["player_available"] and metrics["selected_slot_loaded"]:
        metrics["autoload_success"] = 1
    status = str(telemetry.get("autoload_last_status") or "")
    if "direct continue sequence requested" in status:
        queued_load_request = True

if queued_load_request and load_hook_seen and (trace_confirms_state_transition or metrics["game_save_state"] not in (-1, 0)):
    metrics["native_request_consumed"] = 1

hard_zero = bool(
    gate_failed
    or not metrics["test_pass"]
    or not metrics["er_process_teardown_ok"]
    or metrics["host_pointer_input_used"]
    or not metrics["save_safety_ok"]
    or metrics["crash_detected"]
)

if hard_zero:
    score = 0
elif metrics["autoload_success"] and metrics["player_available"] and metrics["selected_slot_loaded"]:
    score = 1000
elif metrics["native_request_consumed"]:
    score = 800
elif trace_confirms_state_transition and static_score >= 400:
    score = 600
elif static_score >= 400:
    score = 400
elif metrics["test_pass"]:
    score = 200
else:
    score = 0

metrics["north_star_score"] = score
print(f"METRIC north_star_score={metrics['north_star_score']}")
for key in [
    "autoload_success",
    "player_available",
    "selected_slot_loaded",
    "time_to_player_seconds",
    "game_save_state",
    "game_save_slot",
    "game_requested_save_slot_load_index",
    "game_save_requested",
    "title_bootstrap_seen",
    "native_request_consumed",
    "crash_detected",
    "save_safety_ok",
    "er_process_teardown_ok",
    "host_pointer_input_used",
    "trace_invasiveness_score",
    "static_evidence_score",
    "runtime_probe_seconds",
    "build_seconds",
    "test_pass",
    "code_complexity_delta",
    "artifact_bytes",
    "false_positives",
]:
    value = metrics[key]
    if isinstance(value, float):
        print(f"METRIC {key}={value:.3f}")
    else:
        print(f"METRIC {key}={value}")

if telemetry_path:
    print(f"ASI telemetry_path={telemetry_path}")
if artifact_dir:
    print(f"ASI artifact_dir={artifact_dir}")
PY

if [[ "$GATE_FAILED" != "0" ]]; then
  exit 1
fi
