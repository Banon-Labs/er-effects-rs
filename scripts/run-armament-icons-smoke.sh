#!/usr/bin/env bash
# ARMAMENT-ICONS badge oracle smoke (bd er-effects-rs-pe98): three-native me3 run --
# input-harness (drive mode `equip`: boot -> Continue -> in-world -> pause menu ->
# Confirm into Equipment -> dwell), telemetry DLL (timeseries semaphores), and
# er_armament_icons.dll (TilePopulate post-hook + ArtsIcon badge draw + oracle
# counters in er-armament-icons.log). NO product DLL: the harness drives standalone.
#
# ORACLE (semaphore-progress teardown, not wall-clock): PASS when the badge log shows
# "badge sample: DRAWN" lines (tile hook fired, ArtsIcon bound + un-hidden + icon
# set); teardown a short settle after the harness dwell_equip phase completes or
# after the first DRAWN evidence, whichever is later; the canonical runtime cap is
# only the idle/stall backstop. REQUIRES: Steam running; correct GAME_DIR.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/armament-icons-smoke-$(date +%Y%m%d-%H%M%S)}"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness_dll.dll"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll"
BADGE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_armament_icons.dll"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 300)"
# Settle window after the decisive semaphore before teardown.
SETTLE_SECONDS="${SETTLE_SECONDS:-10}"

fail() {
	echo "run-armament-icons-smoke: $*" >&2
	exit 2
}

# --- GAME_DIR resolution (current-user-aware; never hard-code /home/<user>) ---
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
	"GAME_DIR not resolved. Set GAME_DIR=<linux path to the '.../ELDEN RING/Game' dir with eldenring.exe>."

# shellcheck source=scripts/steam-running.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"
steam_running || fail "Steam is not running. Start Steam (interactive login) first."
for d in "$HARNESS_DLL" "$TELEM_DLL" "$BADGE_DLL"; do
	[[ -f "$d" ]] || fail "DLL not built: $d (cargo xwin build --release --target x86_64-pc-windows-msvc)"
done
if tasklist.exe 2>/dev/null | grep -qiE 'eldenring\.exe|start_protected_game\.exe'; then
	fail "An Elden Ring process is already running. Tear it down before launching (never a blanket kill)."
fi

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage the 3 DLLs + profile ---
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness_dll.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry_dll.dll"
BADGE_GAMEDIR="$GAME_DIR/er_armament_icons.dll"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
cp -f "$BADGE_DLL" "$BADGE_GAMEDIR"

PROFILE="$ARTIFACT_DIR/armament-icons-smoke.me3"
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
	# BADGE=0 omits the badge DLL entirely -> VANILLA baseline capture (no glyph) for the
	# pixel-diff oracle. Default includes it.
	if [[ "${BADGE:-1}" != "0" ]]; then
		echo
		echo '[[natives]]'
		echo "path = '$(win_path "$BADGE_GAMEDIR")'"
	fi
} >"$PROFILE"

# --- wiring markers: harness drive mode (MODE=equip|inv, default inv -- the Inventory tabs
#     are the user's primary target and their cells carry the bottom-left ArtsIcon child) ---
echo -n "${MODE:-inv}" >"$GAME_DIR/er-harness-drive-mode.txt"
# FORCE_ICON=<u16>: diagnostic -- draw a fixed visible icon into every badge (locator / oracle proof).
export ER_ARMAMENT_ICONS_FORCE_ICON="${FORCE_ICON:-}"
# NO save redirect: pure APPDATA vanilla save (whatever character is last-active).
[[ -f "$GAME_DIR/er-effects.toml" ]] && mv -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
# Sweep stale logs/markers so a prior run cannot pollute this one.
rm -f "$GAME_DIR"/er-armament-icons.log "$GAME_DIR"/er-input-harness.log \
	"$GAME_DIR"/er-input-harness-phases.jsonl "$GAME_DIR"/er-telemetry-timeseries.jsonl \
	"$GAME_DIR"/er-harness-probe-hold-id.txt "$GAME_DIR"/er-harness-os-input.txt \
	"$GAME_DIR"/er-harness-native-quit.txt "$GAME_DIR"/er-harness-force-drive.txt 2>/dev/null

# SAFETY (bd never-blanket-kill-eldenring): only tear down the PIDs THIS run spawns.
win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}
PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "

# Last-resort safety-net trap: a SINGLE kill pass for this run's PIDs (no sleep -- the
# Python watcher owns the graceful two-pass teardown + verify). Runs only if the watcher
# is interrupted before it tears down.
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
echo "== LAUNCHING ELDEN RING (offline, me3) -- ARMAMENT-ICONS badge smoke   =="
echo "==   harness drive 'equip': boot -> Continue -> pause menu -> Equipment=="
echo "==   er_armament_icons.dll TilePopulate hook + ArtsIcon badge oracle   =="
echo "==   pure APPDATA save (no redirect)   cap=${CAP_SECONDS}s backstop    =="
echo "==   INPUT WILL BE DRIVEN (raw-pad taps) -- agent-owned bounded run    =="
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &

# --- delegate the timed watch + teardown to the Python watcher (no shell sleep;
#     scripts/check-no-timeouts.py bans shell sleeps, Python time.sleep is fine). ---
python3 "$REPO_ROOT/scripts/armament-icons-watch.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--settle-seconds "$SETTLE_SECONDS" \
	--pre-er-pids "$PRE_ER_PIDS" \
	--pre-me3-pids "$PRE_ME3_PIDS" \
	--repo-root "$REPO_ROOT"
RC=$?

# The watcher already tore the game down; disable the safety-net trap and append DLL
# provenance + harness phases to the report it wrote.
trap - EXIT
rm -f "$GAME_DIR/er-harness-drive-mode.txt" 2>/dev/null
[[ -f "$ARTIFACT_DIR/er-effects.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-effects.toml.bak" "$GAME_DIR/er-effects.toml"
{
	echo "git_head: $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
	for d in er_input_harness_dll.dll er_telemetry_dll.dll er_armament_icons.dll; do
		f="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/$d"
		[[ -f "$f" ]] && echo "$d: mtime=$(date -r "$f" +%Y%m%d-%H%M%S) sha=$(sha256sum "$f" | cut -c1-16)"
	done
	echo "--- harness phases ---"
	[[ -f "$ARTIFACT_DIR/er-input-harness-phases.jsonl" ]] && cat "$ARTIFACT_DIR/er-input-harness-phases.jsonl"
} >>"$ARTIFACT_DIR/report.txt"

echo "== armament-icons smoke done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
