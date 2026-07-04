#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
# Single source of truth for the runtime-probe wall-clock cap (seconds); fail safe to the 45s hard truth.
RUNTIME_TIMEOUT_CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 45)"
RUNTIME_TIMEOUT_SECONDS="${RUNTIME_TIMEOUT_SECONDS:-$RUNTIME_TIMEOUT_CAP_SECONDS}"
RUNTIME_EXPECTED_MODE="${RUNTIME_EXPECTED_MODE:-vanilla}"
POLICY_PATH="$REPO_ROOT/.auto/runtime_experiment_policy.rego"
SEAMLESS_DLL_PATH="$GAME_DIR/SeamlessCoop/ersc.dll"
SEAMLESS_STAGED_PATH="${SEAMLESS_STAGED_PATH:-$GAME_DIR/SeamlessCoop/ersc.dll.er-effects-staged}"
HOST_PROCESS_TRACE_PATH="${HOST_PROCESS_TRACE_PATH:-${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe}/host-process-lifetime.jsonl}"
host_process_sampler_pid=""

cleanup_runtime() {
  if [[ -n "$host_process_sampler_pid" ]] && kill -0 "$host_process_sampler_pid" 2>/dev/null; then
    kill "$host_process_sampler_pid" 2>/dev/null || true
    wait "$host_process_sampler_pid" 2>/dev/null || true
  fi
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
  command -v opa >/dev/null 2>&1 || { echo "missing required command: opa" >&2; exit 127; }
  local decision
  decision=$(runtime_policy_input | opa eval --format raw -d "$POLICY_PATH" -I 'data.auto.runtime_experiment.allow')
  if [[ "$decision" != "true" ]]; then
    echo "runtime policy denied manual probe" >&2
    runtime_policy_input | opa eval --format pretty -d "$POLICY_PATH" -I 'data.auto.runtime_experiment.deny' >&2 || true
    exit 2
  fi
}

start_host_process_sampler() {
  mkdir -p "$(dirname "$HOST_PROCESS_TRACE_PATH")"
  python3 - "$PID_FILE" "$HOST_PROCESS_TRACE_PATH" "$RUNTIME_TIMEOUT_SECONDS" <<'PY' &
import json
import os
import sys
import time
from pathlib import Path

pid_file = Path(sys.argv[1])
out_path = Path(sys.argv[2])
timeout = float(sys.argv[3])
interval = float(os.environ.get("RUNTIME_HOST_PROCESS_SAMPLE_INTERVAL", "0.25"))
deadline = time.monotonic() + timeout + 5.0
start = time.monotonic()

def read_text(path: Path) -> str | None:
    try:
        return path.read_text(errors="replace")
    except OSError:
        return None

def executable_maps_for(pid: int, name: str | None) -> list[dict]:
    # Only the target game process gets map capture. This keeps the artifact bounded and avoids
    # dumping unrelated Wine helper process maps while still making crash addresses symbolizable.
    if name != "eldenring.exe":
        return []
    maps_text = read_text(Path("/proc") / str(pid) / "maps") or ""
    out = []
    for line in maps_text.splitlines():
        parts = line.split(None, 5)
        if len(parts) < 5:
            continue
        addr, perms = parts[0], parts[1]
        path = parts[5] if len(parts) >= 6 else ""
        if "x" not in perms or not path or path.startswith("["):
            continue
        try:
            start_s, end_s = addr.split("-", 1)
            start_i = int(start_s, 16)
            end_i = int(end_s, 16)
        except Exception:
            continue
        out.append({
            "start": f"0x{start_i:x}",
            "end": f"0x{end_i:x}",
            "perms": perms,
            "path": path[-220:],
        })
        if len(out) >= 120:
            break
    return out

def proc_entry(pid: int) -> dict:
    base = Path("/proc") / str(pid)
    status_text = read_text(base / "status") or ""
    ppid = None
    state = None
    name = None
    for line in status_text.splitlines():
        if line.startswith("Name:"):
            name = line.split(None, 1)[1] if len(line.split(None, 1)) > 1 else ""
        elif line.startswith("State:"):
            state = line.split(None, 1)[1] if len(line.split(None, 1)) > 1 else ""
        elif line.startswith("PPid:"):
            try:
                ppid = int(line.split()[1])
            except Exception:
                ppid = None
    cmd_raw = read_text(base / "cmdline")
    cmdline = (cmd_raw or "").replace("\x00", " ").strip()
    entry = {"pid": pid, "exists": base.exists(), "ppid": ppid, "state": state, "name": name, "cmdline": cmdline[:500]}
    maps = executable_maps_for(pid, name)
    if maps:
        entry["exec_maps"] = maps
    return entry

def direct_ppids() -> dict[int, list[int]]:
    parents: dict[int, list[int]] = {}
    for child_dir in Path("/proc").iterdir():
        if not child_dir.name.isdigit():
            continue
        pid = int(child_dir.name)
        status_text = read_text(child_dir / "status") or ""
        ppid = None
        for line in status_text.splitlines():
            if line.startswith("PPid:"):
                try:
                    ppid = int(line.split()[1])
                except Exception:
                    ppid = None
                break
        if ppid is not None:
            parents.setdefault(ppid, []).append(pid)
    return parents

def tree(root_pid: int) -> list[dict]:
    parents = direct_ppids()
    seen = set()
    pending = [root_pid]
    out = []
    while pending and len(seen) < 64:
        pid = pending.pop(0)
        if pid in seen:
            continue
        seen.add(pid)
        out.append(proc_entry(pid))
        pending.extend(parents.get(pid, []))
    return out

with out_path.open("a", encoding="utf-8") as fh:
    while time.monotonic() < deadline:
        pid_text = read_text(pid_file)
        root_pid = None
        if pid_text:
            try:
                root_pid = int(pid_text.strip().splitlines()[0])
            except Exception:
                root_pid = None
        sample = {
            "t": round(time.monotonic() - start, 3),
            "pid_file_pid": root_pid,
            "tree": tree(root_pid) if root_pid else [],
        }
        fh.write(json.dumps(sample, sort_keys=True) + "\n")
        fh.flush()
        time.sleep(interval)
PY
  host_process_sampler_pid=$!
  echo "host-process-sampler: pid=$host_process_sampler_pid trace=$HOST_PROCESS_TRACE_PATH" >&2
}

setup_runtime_payload() {
  # Crash logging ON BY DEFAULT for every probe (file channel -- reliable through Proton, unlike
  # env vars; the DLL reads the flag from the exe dir). Installs the vectored AV handler (logs the
  # faulting RVA + caller stack of an access violation, e.g. a wrong-arg native call) + the
  # process-exit hooks, into er-effects-crash.log. Opt out by exporting
  # RUNTIME_DISABLE_CRASH_LOG=1. The product DLL stays opt-in; this is probe-only.
  if [[ "${RUNTIME_DISABLE_CRASH_LOG:-0}" == "1" ]]; then
    rm -f "$GAME_DIR/er-effects-crash-log.txt"
  else
    : > "$GAME_DIR/er-effects-crash-log.txt"
  fi
  # me3 is the ONLY loader (LazyLoader removed 2026-07-04): the me3 mod host loads the DLL from a
  # [[natives]] profile entry written by the launcher script. A leftover dinput8 proxy would
  # DOUBLE-LOAD the DLL (two modules, two DllMains, double hooks); fail closed so such a run can
  # never be scored.
  if [[ -f "$GAME_DIR/dinput8.dll" ]]; then
    echo "$GAME_DIR/dinput8.dll is present (removed LazyLoader proxy) -- refusing the double-load run; delete or stage away the proxy" >&2
    exit 2
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

start_host_process_sampler

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
