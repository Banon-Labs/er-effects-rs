#!/usr/bin/env bash
# Same-character-3x milestone runner (docs/goals/repeatable-multi-save-load-acceptance.md SS4a).
#
# Two DLLs via me3, unioned hooks (single MinHook instance owned by the product DLL):
#   - er_quickload.dll        (PRODUCT, being tuned): boot autoload = load1; sq-repro XInput autopilot
#                              drives 2 same-slot reloads (load2 = freeze, load3 = recovery), with the
#                              load-2 freeze force-advanced to load3 by the DLL's freeze-recovery deadline.
#   - er_reload_trace.dll  (COMPANION, log-only): routes every native load/menu hook through the
#                              product's er_effects_union_register export and logs the pipeline.
# The capture watcher OBSERVES per-load RAM oracles + captures the mandatory loading-screen-portrait.
#
# REQUIRES: Steam running; a correct GAME_DIR (the '.../ELDEN RING/Game' dir the game runs from).
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

CORPUS_ROOT="${ER_SAVE_CORPUS_ROOT:-/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files}"
BOOT_FILE="${BOOT_FILE:-$CORPUS_ROOT/100-Lilbro/ER0000.sl2}" # angrE L100
BOOT_SLOT="${BOOT_SLOT:-0}"
TARGET_SLOTS="${TARGET_SLOTS:-0,0,0}" # same-slot (angrE) x3
SWITCHES="${SWITCHES:-3}"             # 3 reloads after autoload = 4 loads total
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/samechar-3x-$(date +%Y%m%d-%H%M%S)}"
PRODUCT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll"
TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace.dll"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 180)"

fail() {
	echo "run-samechar-3x: $*" >&2
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
[[ -f "$PRODUCT_DLL" ]] || fail "product DLL not built: $PRODUCT_DLL"
[[ -f "$TRACE_DLL" ]] || fail "trace DLL not built: $TRACE_DLL"
[[ -f "$BOOT_FILE" ]] || fail "boot save not found: $BOOT_FILE"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage BOTH DLLs to GAME_DIR + a TWO-native me3 profile (trace first, product second) ---
PRODUCT_GAMEDIR="$GAME_DIR/er_quickload.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
PROFILE="$ARTIFACT_DIR/samechar-3x.me3"
# Product FIRST so its er_effects_union_register export is mapped before the trace DLL's install
# thread resolves it (union chaining is load-order-safe either way; this just avoids the trace poll).
{
	echo 'profileVersion = "v1"'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$PRODUCT_GAMEDIR")'"
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$TRACE_GAMEDIR")'"
} >"$PROFILE"

# --- boot TOML (in-memory read-only redirect) for load1 = angrE ---
[[ -f "$GAME_DIR/er-quickload.toml" ]] && cp -f "$GAME_DIR/er-quickload.toml" "$ARTIFACT_DIR/er-quickload.toml.bak"
{
	echo "# staged by run-samechar-3x-twodll.sh"
	echo "save_file = '$(win_path "$BOOT_FILE")'"
	echo "slot = $BOOT_SLOT"
} >"$GAME_DIR/er-quickload.toml"

# --- arm sq-repro XInput autopilot for SWITCHES same-slot reloads ---
# NO_SQREPRO=1 => pure AUTOLOAD-ONLY diagnostic (no sq-repro arming, no input-block, no driving):
# observes whether the boot autoload alone reaches render_ready, isolating "sq-repro arming breaks
# load1" from "the staged autoload itself freezes". Remove any stale sq-repro markers either way.
rm -f "$GAME_DIR/er-quickload-system-quit-repro.txt" "$GAME_DIR/er-quickload-system-quit-load-switch.txt" \
	"$GAME_DIR/er-quickload-sq-target-switches.txt" "$GAME_DIR/er-quickload-sq-target-slots.txt" \
	"$GAME_DIR/er-quickload-switch-slot.txt" 2>/dev/null
if [[ "${NO_SQREPRO:-0}" != "1" ]]; then
	printf '1\n' >"$GAME_DIR/er-quickload-system-quit-repro.txt"
	printf '1\n' >"$GAME_DIR/er-quickload-system-quit-load-switch.txt"
	printf '%s\n' "$SWITCHES" >"$GAME_DIR/er-quickload-sq-target-switches.txt"
	printf '%s\n' "$TARGET_SLOTS" >"$GAME_DIR/er-quickload-sq-target-slots.txt"
else
	echo "NO_SQREPRO=1: autoload-only diagnostic (no driving) -- watching if load1 renders"
fi

# --- CLEAN SLATE: recreate every log/telemetry text file so no PRIOR run pollutes this one. The
#     product DLL also self-truncates its debug log on first write per process; this sweep additionally
#     clears the trace log + stale telemetry + any other *.log the run left behind. Control files
#     (*.txt) and the boot TOML are intentionally NOT touched. ---
# THE SWEEP OF GAME_DIR IS GONE ON PURPOSE. It used to read
#   rm -f "$GAME_DIR"/er-quickload-*.log "$GAME_DIR"/er-reload-trace.log \
#         "$GAME_DIR"/er-quickload-telemetry.json "$GAME_DIR"/er-quickload-input-trace.jsonl
# and it destroyed TWO generations of ANOTHER run's evidence at a time: `begin_fresh_run` removes
# `<name>.prev` unconditionally when the live file is absent. Every log this run writes now lands in
# a fresh per-run ARTIFACT_DIR, so the clean slate costs nothing and takes nobody else's run with it.

# --- movement-proof gate: authorize the in-DLL can-move probe to inject a forward stick in-world and
#     prove input moves the character (havok delta) -- the only reliable good-vs-frozen signal. Proof-
#     only (absent in normal user sessions so it never fights the player). Skip in NO_SQREPRO+user-watch.
rm -f "$GAME_DIR/er-quickload-probe-foreground.txt" 2>/dev/null
# Canonical ordered semaphore trace for boot-vs-reload diffing. The DLL writes in GAME_DIR so the
# Windows process never needs to open a WSL path; we copy it into ARTIFACT_DIR after capture.
printf '1\n' >"$GAME_DIR/er-quickload-input-trace.txt"

if [[ "${PROVE_MOVEMENT:-1}" == "1" ]]; then
	printf '1\n' >"$GAME_DIR/er-quickload-prove-movement.txt"
	printf '1\n' >"$GAME_DIR/er-quickload-stay-active.txt" # accept injected input while unfocused
	# PROBE_FOREGROUND=1 (unattended proof only): let the probe force ER foreground so gameplay movement
	# input registers. Default OFF so a user-present run is never focus-stolen.
	[[ "${PROBE_FOREGROUND:-0}" == "1" ]] && printf '1\n' >"$GAME_DIR/er-quickload-probe-foreground.txt"
else
	rm -f "$GAME_DIR/er-quickload-prove-movement.txt" 2>/dev/null
fi

# shellcheck disable=SC2317
cleanup() {
	taskkill.exe /F /IM eldenring.exe >/dev/null 2>&1
	taskkill.exe /F /IM me3.exe >/dev/null 2>&1
	rm -f "$GAME_DIR/er-quickload-system-quit-repro.txt" "$GAME_DIR/er-quickload-system-quit-load-switch.txt" \
		"$GAME_DIR/er-quickload-sq-target-switches.txt" "$GAME_DIR/er-quickload-sq-target-slots.txt" \
		"$GAME_DIR/er-quickload-prove-movement.txt" "$GAME_DIR/er-quickload-stay-active.txt" \
		"$GAME_DIR/er-quickload-probe-foreground.txt" "$GAME_DIR/er-quickload-input-trace.txt" 2>/dev/null
	[[ -f "$ARTIFACT_DIR/er-quickload.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-quickload.toml.bak" "$GAME_DIR/er-quickload.toml"
}
trap cleanup EXIT

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- same-char-3x, TWO DLLs =="
echo "==   product+trace unioned; sq-repro XInput drives 2 same-slot reloads"
echo "==   boot=$BOOT_FILE slot=$BOOT_SLOT  target_slots=[$TARGET_SLOTS]  cap=${CAP_SECONDS}s"
echo "==   INPUT WILL BE CAPTURED (XInput autopilot) -- agent-owned bounded run"
echo "==   artifacts -> $ARTIFACT_DIR"
echo "==   semaphore trace -> $ARTIFACT_DIR/load-semaphore-trace.jsonl"
echo "======================================================================"

# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last, and
# several sessions launch concurrently here. A copy at teardown cannot fix it -- by then this run has
# already clobbered the previous one's file -- and a crashed or killed run never reaches the copy.
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

CAPTURE_ARGS=()
if [[ "${CAPTURE_LOAD1_IMPRINT:-0}" == "1" ]]; then
	# IMPRINT capture: record the clean load1 boot timeseries; tear down on sustained world-ready.
	CAPTURE_ARGS+=(--capture-load1-imprint)
elif [[ "${NO_SQREPRO:-0}" != "1" ]]; then
	CAPTURE_ARGS+=(--require-reload-move)    # full sequence: prove a RELOAD moves
	CAPTURE_ARGS+=(--require-reload-settled) # and that native MoveMap/requestCode handoff finished
fi
python3 "$REPO_ROOT/scripts/capture-samechar-3x.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--report "$ARTIFACT_DIR/samechar-3x-report.md" \
	"${CAPTURE_ARGS[@]}"
RC=$?

if [[ -f "$GAME_DIR/er-quickload-input-trace.jsonl" ]]; then
	cp -f "$GAME_DIR/er-quickload-input-trace.jsonl" "$ARTIFACT_DIR/load-semaphore-trace.jsonl"
fi

echo "== capture done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
