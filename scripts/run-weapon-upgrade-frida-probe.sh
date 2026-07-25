#!/usr/bin/env bash
# Weapon-upgrade Frida probe: launch the input harness in `upgrade_frida` mode, wait until the
# native OpenEnhanceShop(0) menu is open, attach from Windows Python/Frida, then tear down.
set -euo pipefail

fail() {
	echo "ERROR: $*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
Usage: scripts/run-weapon-upgrade-frida-probe.sh

Launches the approved offline ME3 Elden Ring probe with the input harness in upgrade_frida mode,
holds the armament-upgrade menu open, runs Windows Python Frida diagnostics, writes artifacts under
 target/runtime-probe/weapon-upgrade-frida-<timestamp>, then tears down the launched process tree.

Environment overrides:
  GAME_DIR=<linux path to ELDEN RING/Game>
  ME3=<linux path to Windows me3.exe>
  ARTIFACT_DIR=<artifact output dir>
  FRIDA_TIMEOUT_SECONDS=<seconds to keep Frida attached after menu open> (default: 90)
  FRIDA_SCRIPT=<linux path to Frida JS script> (default: scripts/frida/weapon-upgrade-menu-trace.js)
  HARNESS_DRIVE_MODE=<drive mode> (default: upgrade_frida)
  FRIDA_WAIT_PHASE=<phase name to wait for before attaching> (default: hold_before_weapon_upgrade_menu_for_frida)
  FRIDA_RELEASE_MARKER=0 to attach without releasing a marker-gated hold phase (default: 1)
EOF
}

if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
	usage
	exit 0
fi

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness_dll.dll"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll"
ATTACH_PY="$REPO_ROOT/scripts/windows-frida-attach.py"
FRIDA_JS="${FRIDA_SCRIPT:-$REPO_ROOT/scripts/frida/weapon-upgrade-menu-trace.js}"
DRIVE_MODE="${HARNESS_DRIVE_MODE:-upgrade_frida}"
FRIDA_WAIT_PHASE="${FRIDA_WAIT_PHASE:-hold_before_weapon_upgrade_menu_for_frida}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/weapon-upgrade-frida-$(date +%Y%m%d-%H%M%S)}"
ARTIFACT_DIR="$(python3 -c 'from pathlib import Path; import sys; print(Path(sys.argv[1]).resolve())' "$ARTIFACT_DIR")"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 300)"
FRIDA_TIMEOUT_SECONDS="${FRIDA_TIMEOUT_SECONDS:-90}"
OPEN_WAIT_SECONDS="${OPEN_WAIT_SECONDS:-120}"
FRIDA_RELEASE_MARKER="${FRIDA_RELEASE_MARKER:-1}"

[[ -f "$ATTACH_PY" ]] || fail "missing Windows Frida attach helper: $ATTACH_PY"
[[ -f "$FRIDA_JS" ]] || fail "missing Frida script: $FRIDA_JS"

if [[ -z "${GAME_DIR:-}" ]]; then
	for c in \
		"/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game" \
		"$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game" \
		"$HOME/.steam/steam/steamapps/common/ELDEN RING/Game"; do
		[[ -f "$c/eldenring.exe" ]] && {
			GAME_DIR="$c"
			break
		}
	done
fi
[[ -n "${GAME_DIR:-}" && -f "$GAME_DIR/eldenring.exe" ]] || fail \
	"GAME_DIR not resolved. Set GAME_DIR=<linux path to the '.../ELDEN RING/Game' dir>."

# shellcheck source=scripts/steam-running.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"
steam_running || fail "Steam is not running. Start Steam (interactive login) first."

require_fresh_dll() {
	local dll="$1"
	shift
	[[ -f "$dll" ]] || fail "DLL not built: $dll"
	python3 - "$dll" "$@" <<'PY'
import sys
from pathlib import Path

dll = Path(sys.argv[1])
roots = [Path(p) for p in sys.argv[2:]]
dll_mtime = dll.stat().st_mtime
stale = []
for root in roots:
    candidates = [root] if root.is_file() else [p for p in root.rglob("*") if p.is_file()]
    for path in candidates:
        if path.suffix in {".rs", ".toml"} and path.stat().st_mtime > dll_mtime:
            stale.append(path)
            if len(stale) >= 8:
                break
    if len(stale) >= 8:
        break
if stale:
    print(f"STALE_DLL: {dll} is older than source files:", file=sys.stderr)
    for path in stale:
        print(f"  {path}", file=sys.stderr)
    sys.exit(1)
PY
}

require_fresh_dll \
	"$HARNESS_DLL" \
	"$REPO_ROOT/crates/er-input-harness-dll/Cargo.toml" \
	"$REPO_ROOT/crates/er-input-harness-dll/src" \
	"$REPO_ROOT/crates/er-safe-input/Cargo.toml" \
	"$REPO_ROOT/crates/er-safe-input/src" \
	"$REPO_ROOT/crates/er-hook/Cargo.toml" \
	"$REPO_ROOT/crates/er-hook/src" \
	"$REPO_ROOT/crates/er-game-base/Cargo.toml" \
	"$REPO_ROOT/crates/er-game-base/src"
require_fresh_dll \
	"$TELEM_DLL" \
	"$REPO_ROOT/crates/er-telemetry-dll/Cargo.toml" \
	"$REPO_ROOT/crates/er-telemetry-dll/src" \
	"$REPO_ROOT/crates/er-telemetry/Cargo.toml" \
	"$REPO_ROOT/crates/er-telemetry/src"

win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}

if [[ -n "$(win_pids_for eldenring.exe)$(win_pids_for start_protected_game.exe)$(win_pids_for me3.exe)$(win_pids_for me3-launcher.exe)" ]]; then
	fail "An Elden Ring/me3 process is already running. Tear it down before launching."
fi

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() {
	python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"
}
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness_dll.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry_dll.dll"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"

PROFILE="$ARTIFACT_DIR/weapon-upgrade-frida.me3"
{
	echo 'profileVersion = "v1"'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$HARNESS_GAMEDIR")'"
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$TELEM_GAMEDIR")'"
} >"$PROFILE"

echo -n "$DRIVE_MODE" >"$GAME_DIR/er-harness-drive-mode.txt"
[[ -f "$GAME_DIR/er-effects.toml" ]] && mv -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
rm -f "$GAME_DIR"/er-input-harness.log "$GAME_DIR"/er-input-harness-phases.jsonl \
	"$GAME_DIR"/er-telemetry-timeseries.jsonl "$GAME_DIR"/er-harness-force-drive.txt \
	"$GAME_DIR"/er-harness-frida-release-open.txt 2>/dev/null

PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "

new_pids_for() {
	local image="$1"
	local pre_pids="$2"
	local pid
	for pid in $(win_pids_for "$image"); do
		[[ "$pre_pids" == *" $pid "* ]] || echo "$pid"
	done
}

launched_process_running() {
	[[ -n "$(new_pids_for eldenring.exe "$PRE_ER_PIDS")$(new_pids_for me3.exe "$PRE_ME3_PIDS")$(new_pids_for me3-launcher.exe "$PRE_ME3_PIDS")" ]]
}

# shellcheck disable=SC2317
cleanup() {
	local pid
	for pid in $(win_pids_for eldenring.exe); do
		[[ "$PRE_ER_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	for pid in $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe); do
		[[ "$PRE_ME3_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	rm -f "$GAME_DIR/er-harness-drive-mode.txt" "$GAME_DIR/er-harness-frida-release-open.txt" 2>/dev/null
	[[ -f "$ARTIFACT_DIR/er-effects.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-effects.toml.bak" "$GAME_DIR/er-effects.toml"
}
trap cleanup EXIT

phase_has_outcome() {
	local phase_file="$1"
	local phase_name="$2"
	local outcome="$3"
	python3 - "$phase_file" "$phase_name" "$outcome" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
phase = sys.argv[2]
outcome = sys.argv[3]
if not path.exists():
    raise SystemExit(1)
for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
    try:
        row = json.loads(line)
    except json.JSONDecodeError:
        continue
    if row.get("phase") == phase and row.get("outcome") == outcome:
        raise SystemExit(0)
raise SystemExit(1)
PY
}

phase_has_entered() {
	phase_has_outcome "$1" "$2" enter
}

phase_has_derailed() {
	local phase_file="$1"
	python3 - "$phase_file" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
if not path.exists():
    raise SystemExit(1)
for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
    try:
        row = json.loads(line)
    except json.JSONDecodeError:
        continue
    if row.get("outcome") == "derailed":
        raise SystemExit(0)
raise SystemExit(1)
PY
}

wait_for_probe_event() {
	local phase_file="$1"
	local timeout_seconds="${2:-1}"
	if command -v inotifywait >/dev/null 2>&1; then
		if [[ -e "$phase_file" ]]; then
			inotifywait -q -t "$timeout_seconds" -e modify,close_write,move,create,delete_self "$phase_file" "$GAME_DIR" >/dev/null 2>&1 || true
		else
			inotifywait -q -t "$timeout_seconds" -e create,modify,close_write,move "$GAME_DIR" >/dev/null 2>&1 || true
		fi
	else
		read -r -t 1 _ < <(:) || true
	fi
}

copy_artifacts() {
	for name in er-input-harness.log er-input-harness-phases.jsonl er-telemetry-timeseries.jsonl; do
		[[ -f "$GAME_DIR/$name" ]] && cp -f "$GAME_DIR/$name" "$ARTIFACT_DIR/$name"
	done
}

write_report() {
	local verdict="$1"
	local detail="$2"
	{
		echo "verdict: $verdict"
		echo "detail: $detail"
		echo "mode: $DRIVE_MODE"
		echo "artifact_dir: $ARTIFACT_DIR"
		echo "frida_script: $FRIDA_JS"
		echo "frida_events: $ARTIFACT_DIR/frida-events.jsonl"
		echo "--- profile ---"
		cat "$PROFILE"
		echo "--- dll fingerprints ---"
		for d in er_input_harness_dll.dll er_telemetry_dll.dll; do
			f="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/$d"
			[[ -f "$f" ]] && echo "$d: mtime=$(date -r "$f" +%Y%m%d-%H%M%S) sha=$(sha256sum "$f" | cut -c1-16)"
		done
	} >"$ARTIFACT_DIR/report.txt"
}

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- WEAPON-UPGRADE FRIDA PROBE =="
echo "==   harness drive '$DRIVE_MODE'"
echo "==   opens OpenEnhanceShop(0), holds menu, then Windows Frida attaches"
echo "==   Frida script=$FRIDA_JS"
echo "==   Frida timeout=${FRIDA_TIMEOUT_SECONDS}s ; cap=${CAP_SECONDS}s backstop"
echo "==   INPUT WILL BE DRIVEN (menu/native open) -- agent-owned bounded run"
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

ER_HARNESS_DRIVE_MODE="$DRIVE_MODE" \
	"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
ME3_PID=$!
echo "$ME3_PID" >"$ARTIFACT_DIR/me3-launch.pid"

PHASES="$GAME_DIR/er-input-harness-phases.jsonl"
start_epoch=$(date +%s)
seen_launched_process=0
while true; do
	if launched_process_running; then
		seen_launched_process=1
	elif [[ "$seen_launched_process" == "1" ]]; then
		copy_artifacts
		write_report "GAME_PROCESS_EXITED_BEFORE_FRIDA" "launched ER/me3 process exited before Frida pre-open hold"
		exit 1
	fi
	if phase_has_entered "$PHASES" "$FRIDA_WAIT_PHASE"; then
		echo "weapon-upgrade-frida: Frida wait phase '$FRIDA_WAIT_PHASE' reached; attaching before release"
		break
	fi
	if phase_has_derailed "$PHASES"; then
		copy_artifacts
		write_report "DERAILED_BEFORE_FRIDA" "harness derailed before Frida pre-open hold"
		exit 1
	fi
	if (($(date +%s) - start_epoch >= OPEN_WAIT_SECONDS)); then
		copy_artifacts
		write_report "PREOPEN_WAIT_TIMEOUT" "$FRIDA_WAIT_PHASE did not enter within ${OPEN_WAIT_SECONDS}s"
		exit 1
	fi
	wait_for_probe_event "$PHASES" 1
done

WIN_ATTACH="$(wslpath -w "$ATTACH_PY")"
WIN_JS="$(wslpath -w "$FRIDA_JS")"
WIN_OUT="$(wslpath -w "$ARTIFACT_DIR/frida-events.jsonl")"
WIN_MARKER="$(wslpath -w "$GAME_DIR/er-harness-frida-release-open.txt")"
FRIDA_CMD=(cmd.exe /C py -3 "$WIN_ATTACH" --process eldenring.exe --script "$WIN_JS" --out "$WIN_OUT" --timeout "$FRIDA_TIMEOUT_SECONDS" --wait 15)
if [[ "$FRIDA_RELEASE_MARKER" == "1" ]]; then
	FRIDA_CMD+=(--ready-marker "$WIN_MARKER")
fi
"${FRIDA_CMD[@]}" >"$ARTIFACT_DIR/frida-helper.log" 2>&1 &
FRIDA_HELPER_PID=$!
FRIDA_RC=0
while true; do
	if ! kill -0 "$FRIDA_HELPER_PID" 2>/dev/null; then
		set +e
		wait "$FRIDA_HELPER_PID"
		FRIDA_RC=$?
		set -e
		break
	fi
	if phase_has_derailed "$PHASES"; then
		copy_artifacts
		write_report "DERAILED_AFTER_FRIDA" "harness derailed while Frida helper was attached"
		kill "$FRIDA_HELPER_PID" 2>/dev/null || true
		wait "$FRIDA_HELPER_PID" 2>/dev/null || true
		echo "== weapon-upgrade Frida probe derailed ; artifacts in $ARTIFACT_DIR =="
		exit 1
	fi
	if ! launched_process_running; then
		copy_artifacts
		write_report "GAME_PROCESS_EXITED_AFTER_FRIDA" "launched ER/me3 process exited while Frida helper was attached"
		kill "$FRIDA_HELPER_PID" 2>/dev/null || true
		wait "$FRIDA_HELPER_PID" 2>/dev/null || true
		echo "== weapon-upgrade Frida probe process exited ; artifacts in $ARTIFACT_DIR =="
		exit 1
	fi
	wait_for_probe_event "$PHASES" 1
done
copy_artifacts
if [[ $FRIDA_RC -eq 0 ]]; then
	write_report "FRIDA_DONE" "frida helper completed"
else
	write_report "FRIDA_FAILED" "frida helper rc=$FRIDA_RC"
fi

echo "== weapon-upgrade Frida probe done rc=$FRIDA_RC ; artifacts in $ARTIFACT_DIR =="
exit "$FRIDA_RC"
