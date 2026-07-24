#!/usr/bin/env bash
# Weapon-upgrade menu probe: stages input-harness + telemetry only, drives boot -> Continue ->
# pause menu -> native weapon-upgrade menu open, then dwells for semaphore logging. No upgrade
# confirm inputs are sent.
set -euo pipefail

fail() {
	echo "ERROR: $*" >&2
	exit 1
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness_dll.dll"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/weapon-upgrade-menu-$(date +%Y%m%d-%H%M%S)}"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 300)"

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
for d in "$HARNESS_DLL" "$TELEM_DLL"; do
	[[ -f "$d" ]] || fail "DLL not built: $d (cargo xwin build --release --target x86_64-pc-windows-msvc)"
done

win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}

if [[ -n "$(win_pids_for eldenring.exe)$(win_pids_for start_protected_game.exe)" ]]; then
	fail "An Elden Ring process is already running. Tear it down before launching."
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

PROFILE="$ARTIFACT_DIR/weapon-upgrade-menu-probe.me3"
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

echo -n upgrade >"$GAME_DIR/er-harness-drive-mode.txt"
[[ -f "$GAME_DIR/er-effects.toml" ]] && mv -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
rm -f "$GAME_DIR"/er-input-harness.log "$GAME_DIR"/er-input-harness-phases.jsonl \
	"$GAME_DIR"/er-telemetry-timeseries.jsonl "$GAME_DIR"/er-harness-force-drive.txt 2>/dev/null

PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "

# shellcheck disable=SC2317
cleanup() {
	local pid
	for pid in $(win_pids_for eldenring.exe); do
		[[ "$PRE_ER_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	for pid in $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe); do
		[[ "$PRE_ME3_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	rm -f "$GAME_DIR/er-harness-drive-mode.txt" 2>/dev/null
	[[ -f "$ARTIFACT_DIR/er-effects.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-effects.toml.bak" "$GAME_DIR/er-effects.toml"
}
trap cleanup EXIT

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- WEAPON-UPGRADE MENU PROBE =="
echo "==   harness drive 'upgrade': boot -> Continue -> pause -> native upgrade menu"
echo "==   no upgrade confirms; semaphore logging only; cap=${CAP_SECONDS}s backstop"
echo "==   INPUT WILL BE DRIVEN (menu/pad/native open) -- agent-owned bounded run"
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
ME3_PID=$!
echo "$ME3_PID" >"$ARTIFACT_DIR/me3-launch.pid"

set +e
python3 "$SCRIPT_DIR/weapon-upgrade-menu-watch.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--pre-er-pids "$PRE_ER_PIDS" \
	--pre-me3-pids "$PRE_ME3_PIDS"
RC=$?
set -e

{
	echo "--- profile ---"
	cat "$PROFILE"
	echo "--- dll fingerprints ---"
	for d in er_input_harness_dll.dll er_telemetry_dll.dll; do
		f="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/$d"
		[[ -f "$f" ]] && echo "$d: mtime=$(date -r "$f" +%Y%m%d-%H%M%S) sha=$(sha256sum "$f" | cut -c1-16)"
	done
} >>"$ARTIFACT_DIR/report.txt"

echo "== weapon-upgrade menu probe done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
