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
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness.dll"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry.dll"
BADGE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_armament_icons.dll"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 300)"
# Settle window after the decisive semaphore before teardown (user 2026-07-23: ~3s teardown).
SETTLE_SECONDS="${SETTLE_SECONDS:-2}"
# Wait after the menu opens before capturing (must fit within the 3s equip dwell).
CAPTURE_SETTLE_SECONDS="${CAPTURE_SETTLE_SECONDS:-2}"

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
# FRESHNESS, NOT EXISTENCE. This loop asserted only that three files exist, and the profile
# below points me3 straight at target/.../release -- so the badge oracle could be scored against
# a badge DLL from last week and report "0 DRAWN" for code that has since been fixed. The
# report.txt footer records mtime + sha AFTER the fact, which is provenance for the archive, not
# a gate: nothing consulted it before launching. This is the gate.
# shellcheck source=scripts/er-dll-freshness.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/er-dll-freshness.sh"
require_fresh_dlls "$HARNESS_DLL" "$TELEM_DLL" "$BADGE_DLL" ||
	fail "refusing to launch against DLLs that are not this source tree (see above)"
if python3 "$REPO_ROOT/scripts/detect-proc.py" 'eldenring\.exe|start_protected_game\.exe' >/dev/null 2>&1; then
	fail "An Elden Ring process is already running. Tear it down before launching (never a blanket kill)."
fi

# --- me3 resolution: platform-aware, never a hard-coded Windows-user path.
#     Native Linux box (Linux Steam/Proton): the `me3` binary on PATH, invoked the same
#     way as the known-good ~/Elden/launch.sh (me3 --steam-dir <root> launch -p ... -e ...).
#     WSL box (Windows Steam): the Windows me3.exe, discovered across /mnt/c/Users/*.
if [[ -z "${ME3:-}" ]]; then
	if command -v me3 >/dev/null 2>&1; then
		ME3="$(command -v me3)"
	else
		for c in /mnt/c/Users/*/AppData/Local/garyttierney/me3/bin/me3.exe; do
			[[ -f "$c" ]] && {
				ME3="$c"
				break
			}
		done
	fi
fi
[[ -n "${ME3:-}" ]] || fail "me3 not found: no 'me3' on PATH and no /mnt/c/Users/*/AppData/Local/garyttierney/me3/bin/me3.exe (set ME3=<path>)"
case "$ME3" in
*.exe) ME3_NATIVE=0 ;;
*) ME3_NATIVE=1 ;;
esac
# Process boundary flag (same test as armament-icons-watch.py / detect-proc.py).
if command -v tasklist.exe >/dev/null 2>&1; then IS_WSL=1; else IS_WSL=0; fi
if [[ "$ME3_NATIVE" == 1 ]]; then
	# Steam root that owns the game dir: .../<root>/steamapps/common/ELDEN RING/Game
	ME3_STEAM_DIR="${ME3_STEAM_DIR:-$(cd "$GAME_DIR/../../../.." && pwd)}"
	ME3_IMAGES=(me3)
else
	ME3_IMAGES=(me3.exe me3-launcher.exe)
fi

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage the 3 DLLs + profile ---
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry.dll"
BADGE_GAMEDIR="$GAME_DIR/er_armament_icons.dll"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
cp -f "$BADGE_DLL" "$BADGE_GAMEDIR"

PROFILE="$ARTIFACT_DIR/armament-icons-smoke.me3"
{
	echo 'profileVersion = "v1"'
	echo 'start_online = false'
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
# Snapshot what the marker actually contained at launch time (attribution evidence if the
# in-game flag read misses, e.g. launcher CWD drift).
cp -f "$GAME_DIR/er-harness-drive-mode.txt" "$ARTIFACT_DIR/er-harness-drive-mode.txt.staged"
# Diagnostic overrides reach the Windows game via FILE markers, NOT env: WSL bash env vars do
# not cross the WSL->Windows boundary unless in WSLENV (bd wslenv-env-not-propagating-to-windows-game).
# The DLL reads er-armament-icons-force-icon.txt / -target.txt from the game dir (env is fallback).
#   FORCE_ICON=<u16>|mirror : draw a fixed visible icon into every badge (locator / oracle proof).
#   TARGET=<childName>      : approach-B draw target clip (e.g. AttributeIcon; default AutoReplenish/IconImage).
export ER_ARMAMENT_ICONS_FORCE_ICON="${FORCE_ICON:-}"
export ER_ARMAMENT_ICONS_TARGET="${TARGET:-}"
# NO save redirect: pure APPDATA vanilla save (whatever character is last-active).
[[ -f "$GAME_DIR/er-quickload.toml" ]] && mv -f "$GAME_DIR/er-quickload.toml" "$ARTIFACT_DIR/er-quickload.toml.bak"
# Sweep stale logs/markers so a prior run cannot pollute this one.
# THE THREE REDIRECTED LOGS ARE NO LONGER SWEPT FROM GAME_DIR. er-input-harness.log,
# er-input-harness-phases.jsonl and er-telemetry-timeseries.jsonl are written into ARTIFACT_DIR now,
# so a copy left in the game directory belongs to ANOTHER session -- and deleting the live file takes
# its `.prev` with it, because `begin_fresh_run` drops a stale `.prev` when the live file is absent.
# That is two runs' evidence, neither of them this one's. er-armament-icons.log joined them on
# 2026-08-31 (ER_QUICKLOAD_ARMAMENT_ICONS_PATH), so nothing this run writes is left in GAME_DIR and
# only the marker/control .txt files below are cleared.
rm -f "$GAME_DIR"/er-harness-probe-hold-id.txt "$GAME_DIR"/er-harness-os-input.txt \
	"$GAME_DIR"/er-harness-native-quit.txt "$GAME_DIR"/er-harness-force-drive.txt \
	"$GAME_DIR"/er-armament-icons-force-icon.txt "$GAME_DIR"/er-armament-icons-target.txt 2>/dev/null
# Write fresh diagnostic markers (AFTER the sweep) when set.
[[ -n "${FORCE_ICON:-}" ]] && printf '%s' "$FORCE_ICON" >"$GAME_DIR/er-armament-icons-force-icon.txt"
[[ -n "${TARGET:-}" ]] && printf '%s' "$TARGET" >"$GAME_DIR/er-armament-icons-target.txt"

# SAFETY (bd never-blanket-kill-eldenring): only tear down the PIDs THIS run spawns.
pids_for() {
	if [[ "$IS_WSL" == 1 ]]; then
		tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
			python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
	else
		python3 - "$1" <<'PY'
import os, sys
want = sys.argv[1].lower()
hits = []
for e in os.listdir('/proc'):
    if not e.isdigit():
        continue
    try:
        comm = open(f'/proc/{e}/comm', encoding='utf-8', errors='replace').read().strip()
        argv0 = open(f'/proc/{e}/cmdline', 'rb').read().split(b'\x00', 1)[0].decode('utf-8', 'replace')
    except OSError:
        continue
    base = argv0.replace('\\', '/').rsplit('/', 1)[-1].lower()
    if base == want or comm.lower() == want:
        hits.append(e)
print(' '.join(hits))
PY
	fi
}
# Invoked only from the cleanup trap path.
# shellcheck disable=SC2317,SC2329
kill_one() {
	if [[ "$IS_WSL" == 1 ]]; then
		taskkill.exe /F /PID "$1" >/dev/null 2>&1
	else
		kill -9 "$1" 2>/dev/null
	fi
}
PRE_ER_PIDS=" $(pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(for img in "${ME3_IMAGES[@]}"; do pids_for "$img"; done | tr '\n' ' ') "

# Last-resort safety-net trap: a SINGLE kill pass for this run's PIDs (no sleep -- the
# Python watcher owns the graceful two-pass teardown + verify). Runs only if the watcher
# is interrupted before it tears down.
# shellcheck disable=SC2317,SC2329
cleanup() {
	local pid img
	for pid in $(pids_for eldenring.exe); do
		[[ "$PRE_ER_PIDS" == *" $pid "* ]] || kill_one "$pid"
	done
	for img in "${ME3_IMAGES[@]}"; do
		for pid in $(pids_for "$img"); do
			[[ "$PRE_ME3_PIDS" == *" $pid "* ]] || kill_one "$pid"
		done
	done
	rm -f "$GAME_DIR/er-harness-drive-mode.txt" "$GAME_DIR/er-armament-icons-force-icon.txt" "$GAME_DIR/er-armament-icons-target.txt" 2>/dev/null
	[[ -f "$ARTIFACT_DIR/er-quickload.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-quickload.toml.bak" "$GAME_DIR/er-quickload.toml"
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

if [[ "$ME3_NATIVE" == 1 ]]; then
	# Same invocation shape as the known-good ~/Elden/launch.sh (offline comes from the
	# profile's start_online=false; me3 runs from the game dir).
	# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
	# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last,
	# and several sessions launch concurrently here. A copy after the run cannot fix that -- by then
	# this run has clobbered the previous one's file -- and a crashed run never reaches the copy.
	(cd "$GAME_DIR" && env \
		ER_QUICKLOAD_TELEMETRY_PATH="$ARTIFACT_DIR/er-quickload-telemetry.json" \
		ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$ARTIFACT_DIR/er-quickload-autoload-debug.log" \
		ER_QUICKLOAD_CRASH_LOG_PATH="$ARTIFACT_DIR/er-quickload-crash-log.txt" \
		ER_QUICKLOAD_TRACE_CONTINUE_PATH="$ARTIFACT_DIR/er-quickload-continue-trace.log" \
		ER_QUICKLOAD_INPUT_TRACE_PATH="$ARTIFACT_DIR/er-quickload-input-trace.jsonl" \
		ER_QUICKLOAD_BOOTSTRAP_PATH="$ARTIFACT_DIR/er-quickload-bootstrap.jsonl" \
		ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$ARTIFACT_DIR/er-quickload-bootstrap-state.json" \
		ER_QUICKLOAD_PROFILE_PATH="$ARTIFACT_DIR/er-quickload-profile.jsonl" \
		ER_QUICKLOAD_RELOAD_TRACE_PATH="$ARTIFACT_DIR/er-reload-trace.log" \
		ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$ARTIFACT_DIR/er-input-harness.log" \
		ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$ARTIFACT_DIR/er-input-harness-phases.jsonl" \
		ER_QUICKLOAD_DIAG_HARNESS_PATH="$ARTIFACT_DIR/er-diag-harness.log" \
		ER_QUICKLOAD_TIMESERIES_PATH="$ARTIFACT_DIR/er-telemetry-timeseries.jsonl" \
		ER_QUICKLOAD_CPU_PROFILE_PATH="$ARTIFACT_DIR/er-cpu-profile.txt" \
		ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$ARTIFACT_DIR/er-armament-icons.log" \
		ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$ARTIFACT_DIR/er-save-disable.log" \
		ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$ARTIFACT_DIR/er-save-disable-telemetry.json" \
		ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$ARTIFACT_DIR/er-loading-portrait.log" \
		ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$ARTIFACT_DIR/er-loading-portrait-crash-log.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$ARTIFACT_DIR/er-crash-log.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$ARTIFACT_DIR/er-crash-latest.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$ARTIFACT_DIR/er-crash-modules.txt" \
		"$ME3" --steam-dir "$ME3_STEAM_DIR" launch -p "$PROFILE" -g eldenring -e "$GAME_DIR/eldenring.exe") >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
else
	env \
		ER_QUICKLOAD_TELEMETRY_PATH="$ARTIFACT_DIR/er-quickload-telemetry.json" \
		ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH="$ARTIFACT_DIR/er-quickload-autoload-debug.log" \
		ER_QUICKLOAD_CRASH_LOG_PATH="$ARTIFACT_DIR/er-quickload-crash-log.txt" \
		ER_QUICKLOAD_TRACE_CONTINUE_PATH="$ARTIFACT_DIR/er-quickload-continue-trace.log" \
		ER_QUICKLOAD_INPUT_TRACE_PATH="$ARTIFACT_DIR/er-quickload-input-trace.jsonl" \
		ER_QUICKLOAD_BOOTSTRAP_PATH="$ARTIFACT_DIR/er-quickload-bootstrap.jsonl" \
		ER_QUICKLOAD_BOOTSTRAP_STATE_PATH="$ARTIFACT_DIR/er-quickload-bootstrap-state.json" \
		ER_QUICKLOAD_PROFILE_PATH="$ARTIFACT_DIR/er-quickload-profile.jsonl" \
		ER_QUICKLOAD_RELOAD_TRACE_PATH="$ARTIFACT_DIR/er-reload-trace.log" \
		ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH="$ARTIFACT_DIR/er-input-harness.log" \
		ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH="$ARTIFACT_DIR/er-input-harness-phases.jsonl" \
		ER_QUICKLOAD_DIAG_HARNESS_PATH="$ARTIFACT_DIR/er-diag-harness.log" \
		ER_QUICKLOAD_TIMESERIES_PATH="$ARTIFACT_DIR/er-telemetry-timeseries.jsonl" \
		ER_QUICKLOAD_CPU_PROFILE_PATH="$ARTIFACT_DIR/er-cpu-profile.txt" \
		ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$ARTIFACT_DIR/er-armament-icons.log" \
		ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$ARTIFACT_DIR/er-save-disable.log" \
		ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$ARTIFACT_DIR/er-save-disable-telemetry.json" \
		ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$ARTIFACT_DIR/er-loading-portrait.log" \
		ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$ARTIFACT_DIR/er-loading-portrait-crash-log.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$ARTIFACT_DIR/er-crash-log.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$ARTIFACT_DIR/er-crash-latest.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt" \
		ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$ARTIFACT_DIR/er-crash-modules.txt" \
		"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
fi

# --- delegate the timed watch + teardown to the Python watcher (no shell sleep;
#     scripts/check-no-timeouts.py bans shell sleeps, Python time.sleep is fine). ---
python3 "$REPO_ROOT/scripts/armament-icons-watch.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--settle-seconds "$SETTLE_SECONDS" \
	--capture-settle-seconds "$CAPTURE_SETTLE_SECONDS" \
	--pre-er-pids "$PRE_ER_PIDS" \
	--pre-me3-pids "$PRE_ME3_PIDS" \
	--repo-root "$REPO_ROOT" \
	${BASELINE_PNG:+--baseline "$BASELINE_PNG"} \
	${STAGE_BOX:+--stage-box "$STAGE_BOX"}
RC=$?

# The watcher already tore the game down; disable the safety-net trap and append DLL
# provenance + harness phases to the report it wrote.
trap - EXIT
rm -f "$GAME_DIR/er-harness-drive-mode.txt" "$GAME_DIR/er-armament-icons-force-icon.txt" "$GAME_DIR/er-armament-icons-target.txt" 2>/dev/null
[[ -f "$ARTIFACT_DIR/er-quickload.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-quickload.toml.bak" "$GAME_DIR/er-quickload.toml"
{
	echo "git_head: $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
	for d in er_input_harness.dll er_telemetry.dll er_armament_icons.dll; do
		f="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/$d"
		[[ -f "$f" ]] && echo "$d: mtime=$(date -r "$f" +%Y%m%d-%H%M%S) sha=$(sha256sum "$f" | cut -c1-16)"
	done
	echo "--- harness phases ---"
	[[ -f "$ARTIFACT_DIR/er-input-harness-phases.jsonl" ]] && cat "$ARTIFACT_DIR/er-input-harness-phases.jsonl"
} >>"$ARTIFACT_DIR/report.txt"

echo "== armament-icons smoke done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
