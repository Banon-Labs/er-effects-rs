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
if [[ -f .auto/runtime-env ]]; then
  # shellcheck source=/dev/null
  . ./.auto/runtime-env
fi

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/smoke/autoload-runtime-$(date +%Y%m%d-%H%M%S)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
MAX_SECONDS="${MAX_SECONDS:-150}"
mkdir -p "$ARTIFACT_DIR"
ARTIFACT_DIR=$(realpath -m "$ARTIFACT_DIR")

START_MS=$(python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
)
DRIVER_RC=0

save_roots=(
  "$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing"
  "/mnt/c/Users/choza/AppData/Roaming/EldenRing"
)

snapshot_saves() {
  local output=$1
  : > "$output"
  for root in "${save_roots[@]}"; do
    [[ -d "$root" ]] || continue
    find "$root" -type f \( -name 'ER0000.sl2' -o -name 'ER0000.co2' -o -name '*.sl2' -o -name '*.co2' \) -print0
  done | sort -z | xargs -0 --no-run-if-empty sha256sum > "$output"
}

cleanup_runtime() {
  local end_ms runtime_ms save_safety_ok
  end_ms=$(python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
)
  runtime_ms=$((end_ms - START_MS))
  snapshot_saves "$ARTIFACT_DIR/save-hashes-after.txt"
  if cmp -s "$ARTIFACT_DIR/save-hashes-before.txt" "$ARTIFACT_DIR/save-hashes-after.txt"; then
    save_safety_ok=1
  else
    save_safety_ok=0
  fi

  python3 - "$ARTIFACT_DIR" "$runtime_ms" "$DRIVER_RC" "$save_safety_ok" <<'PY'
import json
import os
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

artifact = Path(sys.argv[1])
runtime_ms = int(sys.argv[2])
driver_rc = int(sys.argv[3])
save_safety_ok = int(sys.argv[4])
pattern = re.compile(r'(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)', re.I)

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
    except ProcessLookupError:
        pass
    except PermissionError:
        pass
if before:
    time.sleep(2)
mid = procs()
for pid, _ in mid:
    try:
        os.kill(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    except PermissionError:
        pass
if mid:
    time.sleep(1)
after = procs()
(artifact / "teardown-after.txt").write_text("".join(f"{pid} {args}\n" for pid, args in after), encoding="utf-8")
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
metrics = {
    "driver_rc": driver_rc,
    "runtime_probe_seconds": round(runtime_ms / 1000, 3),
    "time_to_player_seconds": round(runtime_ms / 1000, 3) if player_available else -1,
    "er_process_teardown_ok": 1 if not after else 0,
    "host_pointer_input_used": 0,
    "save_safety_ok": save_safety_ok,
}
(artifact / "runtime-metrics.json").write_text(json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8")
print(f"runtime_artifact_dir={artifact}")
print(f"driver_rc={driver_rc}")
print(f"er_process_teardown_ok={metrics['er_process_teardown_ok']}")
print(f"save_safety_ok={save_safety_ok}")
PY
}

snapshot_saves "$ARTIFACT_DIR/save-hashes-before.txt"

export ER_EFFECTS_AUTOLOAD_SLOT="${ER_EFFECTS_AUTOLOAD_SLOT:-9}"
export ER_EFFECTS_AUTOLOAD_METHOD="${ER_EFFECTS_AUTOLOAD_METHOD:-direct_menu_load}"
export ER_EFFECTS_TRACE_CONTINUE="${ER_EFFECTS_TRACE_CONTINUE:-1}"

if scripts/er-smoke-driver.sh drive \
  --artifact-dir "$ARTIFACT_DIR" \
  --game-dir "$GAME_DIR" \
  --launch-mode direct-protected \
  --max-seconds "$MAX_SECONDS" \
  --max-nudges 0 \
  --screenshot-ext jpg \
  > "$ARTIFACT_DIR/driver.out" 2>&1; then
  DRIVER_RC=0
else
  DRIVER_RC=$?
fi

cleanup_runtime
AUTO_MEASURE_INNER=1 ./.auto/measure.sh
