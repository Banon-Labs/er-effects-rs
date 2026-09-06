#!/usr/bin/env bash
# Same-character-3x runner, THREE DLLs via me3 -- the multi-DLL-per-feature architecture
# (bd multi-dll-separate-crates-per-feature-single-me3-profile-2026-07-19). Sibling of
# run-samechar-3x-twodll.sh; the difference is a THIRD native and NO env/marker arming.
#
#   1. er_quickload.dll         (PRODUCT): boot autoload = load1; owns the single MinHook instance +
#                                the er_effects_union_register export; its ProfileSelect hooks arm the
#                                native reload from menu transitions.
#   2. er_reload_trace.dll   (COMPANION, log-only): unions its load/menu trace hooks through the
#                                product export and logs the pipeline.
#   3. er_input_harness.dll  (COMPANION, self-drive): DEFAULT-ON by PRESENCE (no env/marker gate).
#                                Drives the reversed menu-nav via DIRECT input memory -- CSMenuMan
#                                keystate bitmap (inputmgr+0x90+eventId) + DLUID stay-active (+0x88d),
#                                game-thread-timed through the product union. NOTE: the OptionSetting
#                                -> Quit TAB-SWITCH has no reversed menu-event id (mouse-only); the
#                                harness halts there. Omit this DLL from the profile for production.
#
# Load order is PRODUCT FIRST so its union export is mapped before the companions' install threads
# resolve it. REQUIRES: Steam running; a correct GAME_DIR (the '.../ELDEN RING/Game' dir).
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# SAVE SOURCE IS ALWAYS THE GAME'S DEFAULT APPDATA SAVE (staged-save/ER_QUICKLOAD_SAVE_FILE was
# deprecated 2026-07-08 and stripped from this harness). BOOT_FILE used to select a corpus save,
# but nothing staged it anymore -- it was validated + echoed and silently ignored (observed
# 2026-07-29: BOOT_FILE=100-Lilbro run mounted the APPDATA Banon save). Fail closed instead of lying.
if [[ -n "${BOOT_FILE:-}" ]]; then
	echo "run-samechar-3x-threedll: BOOT_FILE is not supported -- this harness always loads the" >&2
	echo "game-owned default APPDATA save (deprecated staged-save path removed). To test a specific" >&2
	echo "character, make it the active default save first. Refusing to run with a lying parameter." >&2
	exit 2
fi
BOOT_SLOT="${BOOT_SLOT:-0}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/samechar-3x-threedll-$(date +%Y%m%d-%H%M%S)}"
PRODUCT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll"
TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace.dll"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness.dll"
# TELEMETRY (semaphore DLL): standalone read-side oracle -- writes er-telemetry-timeseries.jsonl with
# fixed_spf / now_loading / play_time AND per-core CPU (oracle_core_max_busy / proc_cpu_cores), aligned by
# oracle_tick_ms, so a product load2/load3 run can be tested for single-core contention (bd NEXT-telemetry
# -capture-per-core-cpu). Shipped alongside the product per the goal (product + semaphore/oracle DLLs).
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry.dll"
# EVERY per-run log this probe relies on is redirected into ARTIFACT_DIR, because anything left in
# GAME_DIR is single-slot and the NEXT launch destroys it. Measured 2026-08-31: this harness already
# redirected the autoload debug log, but NOT the continue trace -- so the 11:09 run overwrote the
# 5.4 MB `er-quickload-continue-trace.log` belonging to the 09:07 run, whose evidence nobody had read
# yet. The DLL honours ER_QUICKLOAD_TRACE_CONTINUE_PATH (save_policy_logs.rs `continue_trace_log_path`)
# and falls back to GAME_DIR only when it is unset. Add a line here for any future log rather than
# copying it out afterwards: a copy after teardown cannot recover a file the run itself clobbered.
LAUNCH_ENV_VARS=(
	"ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH=$ARTIFACT_DIR/er-quickload-autoload-debug.log"
	"ER_QUICKLOAD_CRASH_LOG_PATH=$ARTIFACT_DIR/er-quickload-crash.log"
	"ER_QUICKLOAD_TRACE_CONTINUE_PATH=$ARTIFACT_DIR/er-quickload-continue-trace.log"
	"ER_QUICKLOAD_TELEMETRY_PATH=$ARTIFACT_DIR/er-quickload-telemetry.json"
	"ER_QUICKLOAD_INPUT_TRACE_PATH=$ARTIFACT_DIR/er-quickload-input-trace.jsonl"
	"ER_QUICKLOAD_BOOTSTRAP_PATH=$ARTIFACT_DIR/er-quickload-bootstrap.jsonl"
	"ER_QUICKLOAD_BOOTSTRAP_STATE_PATH=$ARTIFACT_DIR/er-quickload-bootstrap-state.json"
	"ER_QUICKLOAD_PROFILE_PATH=$ARTIFACT_DIR/er-quickload-profile.jsonl"
	"ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH=$ARTIFACT_DIR/er-crash-log.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH=$ARTIFACT_DIR/er-crash-latest.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH=$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH=$ARTIFACT_DIR/er-crash-modules.txt"
	# The OTHER three DLLs this probe loads. They had no redirect knob at all until 2026-08-31, so
	# their logs could only ever land in GAME_DIR -- including the reload trace, the largest producer
	# in the repo at ~655 MB/hour, which every launch rotated to `.prev` and the launch after that
	# destroyed. Copying them out after the run (further down) never preserved anything but this
	# run's own output, and a killed run never reached the copy at all.
	"ER_QUICKLOAD_RELOAD_TRACE_PATH=$ARTIFACT_DIR/er-reload-trace.log"
	"ER_QUICKLOAD_INPUT_HARNESS_LOG_PATH=$ARTIFACT_DIR/er-input-harness.log"
	"ER_QUICKLOAD_INPUT_HARNESS_PHASES_PATH=$ARTIFACT_DIR/er-input-harness-phases.jsonl"
	"ER_QUICKLOAD_DIAG_HARNESS_PATH=$ARTIFACT_DIR/er-diag-harness.log"
	"ER_QUICKLOAD_TIMESERIES_PATH=$ARTIFACT_DIR/er-telemetry-timeseries.jsonl"
	"ER_QUICKLOAD_CPU_PROFILE_PATH=$ARTIFACT_DIR/er-cpu-profile.txt"
	"ER_QUICKLOAD_ARMAMENT_ICONS_PATH=$ARTIFACT_DIR/er-armament-icons.log"
	"ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH=$ARTIFACT_DIR/er-save-disable.log"
	"ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH=$ARTIFACT_DIR/er-save-disable-telemetry.json"
	"ER_QUICKLOAD_LOADING_PORTRAIT_PATH=$ARTIFACT_DIR/er-loading-portrait.log"
	"ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH=$ARTIFACT_DIR/er-loading-portrait-crash-log.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH=$ARTIFACT_DIR/er-crash-log.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH=$ARTIFACT_DIR/er-crash-latest.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH=$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt"
	"ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH=$ARTIFACT_DIR/er-crash-modules.txt"
)
# RENDERDOC=1: the Windows RenderDoc DLL, loaded as a me3 native to hook ER's D3D12 device.
RDOC_DLL="${RENDERDOC_DLL:-/mnt/c/Program Files/RenderDoc/renderdoc.dll}"
CAP_SECONDS="${CAP_SECONDS:-$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 180)}"
DRIVE_RELOAD_SLOTS="${DRIVE_RELOAD_SLOTS-0,0}"
WORLD_STABLE_TIMEOUT_S="${WORLD_STABLE_TIMEOUT_S:-90}"
export WORLD_STABLE_TIMEOUT_S

fail() {
	echo "run-samechar-3x-threedll: $*" >&2
	exit 2
}

ME3_STEAM_DIR="${ME3_STEAM_DIR:-$HOME/.local/share/Steam}"

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
# FRESHNESS, NOT EXISTENCE. These four `[[ -f ]]` checks used to be the only thing between this
# probe and week-old code: the profile written below points me3 straight at target/.../release,
# so a DLL that merely EXISTS is what gets loaded. All four are checked, not just the product --
# this run's whole claim is about how the four interact, and one stale companion invalidates it
# exactly as thoroughly as a stale product would. Refusing beats running: a launch on the wrong
# bytes yields evidence indistinguishable from the feature not working.
# shellcheck source=scripts/er-dll-freshness.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/er-dll-freshness.sh"
require_fresh_dlls "$PRODUCT_DLL" "$TRACE_DLL" "$HARNESS_DLL" "$TELEM_DLL" ||
	fail "refusing to launch against DLLs that are not this source tree (see above)"
[[ "${RENDERDOC:-0}" != "1" || -f "$RDOC_DLL" ]] || fail "RENDERDOC=1 but renderdoc.dll not found at '$RDOC_DLL' (set RENDERDOC_DLL=<path to Windows renderdoc.dll>)."

if [[ -z "${ME3:-}" ]]; then
	if command -v me3 >/dev/null 2>&1; then
		ME3="$(command -v me3)"
	else
		ME3="/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe"
	fi
fi
if [[ "$ME3" = */* ]]; then
	[[ -f "$ME3" ]] || fail "me3 executable not found at $ME3 (set ME3=<path to me3>)"
else
	command -v "$ME3" >/dev/null 2>&1 || fail "me3 executable not found on PATH: $ME3 (set ME3=<path to me3>)"
fi

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }
me3_path_arg() {
	case "$ME3" in
		*.exe) win_path "$1" ;;
		*) printf '%s\n' "$1" ;;
	esac
}
me3_profile_arg() {
	case "$ME3" in
		*.exe) wslpath -w "$1" ;;
		*) printf '%s\n' "$1" ;;
	esac
}

# --- stage ALL THREE DLLs to GAME_DIR + a THREE-native me3 profile (product FIRST) ---
PRODUCT_GAMEDIR="$GAME_DIR/er_quickload.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace.dll"
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
# (The timeseries is redirected into ARTIFACT_DIR, so this run starts with a fresh file by
# construction. Deleting the GAME_DIR copy would only destroy ANOTHER run's evidence.)
# COMPANION: in the deterministic control-file path the product owns movement proof + slot switching;
# the input-harness DLL should stay passive so the old menu-driven quit flow cannot fight it. Force-drive
# is only for the legacy menu-nav path or an explicit diagnostic override.
rm -f "$GAME_DIR/er-harness-drive-mode.txt" "$GAME_DIR/er-harness-force-drive.txt"
if [[ "${OBSERVE_ONLY:-0}" != "1" && ( -z "$DRIVE_RELOAD_SLOTS" || "${FORCE_HARNESS_DRIVE:-0}" == "1" ) ]]; then
	printf '%s\n' "${HARNESS_DRIVE_MODE:-full}" >"$GAME_DIR/er-harness-drive-mode.txt"
	printf '1\n' >"$GAME_DIR/er-harness-force-drive.txt"
	LAUNCH_ENV_VARS+=("ER_HARNESS_FORCE_DRIVE=1")
fi
PROFILE="$ARTIFACT_DIR/samechar-3x-threedll.me3"
# Product FIRST so its er_effects_union_register export is mapped before the companions' install
# threads resolve it (union chaining is load-order-safe either way; this just avoids the resolve poll).
{
	echo 'profileVersion = "v1"'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	echo
	# RENDERDOC=1: renderdoc.dll FIRST me3 native (renderdoccmd wrapping me3 does NOT inject into the ER
	# child + breaks me3's launch -- proven dead end 2026-07-22). The old double-capturer/resource assert
	# was the PRODUCT's dummy swapchain, now gated off under renderdoc via renderdoc_active().
	if [[ "${RENDERDOC:-0}" == "1" ]]; then
		echo '[[natives]]'
		echo "path = '$(win_path "$RDOC_DLL")'"
		echo
	fi
	echo '[[natives]]'
	echo "path = '$(win_path "$PRODUCT_GAMEDIR")'"
	# NO_TRACE=1 drops the reload-trace DLL to test whether its per-frame file I/O (which floods ~200x
	# during reloads) is the reload fps cost vs an innocent bystander tracing the real work.
	if [[ -z "${NO_TRACE:-}" ]]; then
		echo
		echo '[[natives]]'
		echo "path = '$(win_path "$TRACE_GAMEDIR")'"
	fi
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$HARNESS_GAMEDIR")'"
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$TELEM_GAMEDIR")'"
} >"$PROFILE"

# --- boot request for load1 ---
# Use the current title/product direct-menu request path for the initial autoload, not the old
# save_requested TOML route. The latter is now a known blocker for this proof: it sits at the hidden
# title/fake-loading surface with requestCode=0 and never gets to a drawable/movable load1.
[[ -f "$GAME_DIR/er-quickload.toml" ]] && cp -f "$GAME_DIR/er-quickload.toml" "$ARTIFACT_DIR/er-quickload.toml.bak"
cp -f "$GAME_DIR/er-quickload.toml" "$ARTIFACT_DIR/er-quickload.toml.effective" 2>/dev/null || true
{
	echo "slot=$BOOT_SLOT"
	echo "method=direct_menu_load"
	echo "require_title_bootstrap=false"
} >"$GAME_DIR/er-quickload-autoload.txt"
cp -f "$GAME_DIR/er-quickload-autoload.txt" "$ARTIFACT_DIR/autoload-request.txt"
LAUNCH_ENV_VARS+=("ER_QUICKLOAD_EXPERIMENTAL_DIRECT_MENU_LOAD=1")
# The DLL now IGNORES `slot=` in er-quickload-autoload.txt by default: a stale copy of that file
# silently chose the loading screen's character in run br-20260831-014208-b1d6, so it is no longer
# a product slot channel. This probe genuinely wants it, so it opts in explicitly through the same
# deprecated-probe gate the other smoke scripts already use (AGENTS.md 2026-07-08). Without this
# line $BOOT_SLOT above would be read, refused, and logged -- not silently obeyed.
LAUNCH_ENV_VARS+=("ER_QUICKLOAD_ALLOW_DEPRECATED_STAGED_SAVE_PROBE=1")

# NO env/marker arming: the input-harness DLL is enabled purely by its PRESENCE in the profile above.
# Sweep any stale legacy sq-repro/probe markers so a prior run cannot pollute this one.
rm -f "$GAME_DIR"/er-quickload-system-quit-repro.txt "$GAME_DIR"/er-quickload-system-quit-load-switch.txt \
	"$GAME_DIR"/er-quickload-sq-target-switches.txt "$GAME_DIR"/er-quickload-sq-target-slots.txt \
	"$GAME_DIR"/er-quickload-prove-movement.txt "$GAME_DIR"/er-quickload-stay-active.txt \
	"$GAME_DIR"/er-quickload-probe-foreground.txt "$GAME_DIR"/er-quickload-input-trace.txt 2>/dev/null

# Movement-proof gate: deterministic control-file reloads still wait for the product's in-DLL
# can-move probe to inject a forward stick and prove Havok movement in each load epoch. This is proof-only
# and absent from normal user sessions. Observe-only runs intentionally do not drive movement.
if [[ "${OBSERVE_ONLY:-0}" != "1" && "${PROVE_MOVEMENT:-1}" == "1" ]]; then
	printf '1\n' >"$GAME_DIR/er-quickload-prove-movement.txt"
	printf '1\n' >"$GAME_DIR/er-quickload-stay-active.txt"
	printf '1\n' >"$GAME_DIR/er-quickload-input-trace.txt"
	[[ "${PROBE_FOREGROUND:-0}" == "1" ]] && printf '1\n' >"$GAME_DIR/er-quickload-probe-foreground.txt"
elif [[ "${PROVE_MOVEMENT:-1}" != "1" ]]; then
	rm -f "$GAME_DIR/er-quickload-prove-movement.txt" 2>/dev/null
fi

# --- CLEAN SLATE, WITHOUT DELETING SOMEONE ELSE'S RUN. ---
# This used to be
#   rm -f "$GAME_DIR"/er-quickload-*.log "$GAME_DIR"/er-reload-trace.log \
#         "$GAME_DIR"/er-input-harness.log "$GAME_DIR"/er-quickload-telemetry.json
# and it was the WORSE of the two evidence destroyers. `begin_fresh_run` removes `<name>.prev`
# unconditionally when the live file is absent, so clearing the live file here dropped TWO
# generations -- neither of them this run's, since several sessions launch concurrently here.
# Every log this probe reads is now redirected into ARTIFACT_DIR, which is fresh per run, so the
# clean slate is free and nothing in GAME_DIR needs touching.

# SAFETY (bd never-blanket-kill-eldenring-killed-user-game-2026-07-22): capture the eldenring.exe/me3
# PIDs that already exist BEFORE we launch (a user's live game, another agent's run) so teardown can
# NEVER touch them. A blanket `taskkill /IM eldenring.exe` here once killed the user's active session.
win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}
native_pids_for() {
	python3 - "$1" <<'PY'
import os, sys
name = sys.argv[1]
out = []
for pid in filter(str.isdigit, os.listdir('/proc')):
    try:
        comm = open(f'/proc/{pid}/comm', encoding='utf-8', errors='replace').read().strip()
    except OSError:
        continue
    if comm == name:
        out.append(pid)
print(' '.join(out))
PY
}

PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "
PRE_NATIVE_ER_PIDS="$(native_pids_for eldenring.exe)"
PRE_NATIVE_ME3_PIDS="$(native_pids_for me3)"
export PRE_NATIVE_ER_PIDS PRE_NATIVE_ME3_PIDS

# shellcheck disable=SC2317,SC2329
cleanup() {
	# Kill ONLY the eldenring.exe/me3 PIDs THIS run spawned (current set minus the pre-launch set).
	# NEVER a blanket /IM -- that killed a user's live game (bd never-blanket-kill-eldenring-killed-user-game).
	local pid
	for pid in $(win_pids_for eldenring.exe); do
		[[ "$PRE_ER_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	for pid in $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe); do
		[[ "$PRE_ME3_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	for pid in $(native_pids_for eldenring.exe); do
		[[ " $PRE_NATIVE_ER_PIDS " == *" $pid "* ]] || kill -TERM "$pid" >/dev/null 2>&1 || true
	done
	for pid in $(native_pids_for me3); do
		[[ " $PRE_NATIVE_ME3_PIDS " == *" $pid "* ]] || kill -TERM "$pid" >/dev/null 2>&1 || true
	done
	[[ -f "$ARTIFACT_DIR/er-quickload.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-quickload.toml.bak" "$GAME_DIR/er-quickload.toml"
	rm -f "$GAME_DIR/er-harness-drive-mode.txt" "$GAME_DIR/er-harness-force-drive.txt" \
		"$GAME_DIR/er-quickload-prove-movement.txt" "$GAME_DIR/er-quickload-stay-active.txt" \
		"$GAME_DIR/er-quickload-probe-foreground.txt" "$GAME_DIR/er-quickload-input-trace.txt" \
		"$GAME_DIR/er-quickload-autoload.txt" 2>/dev/null || true
}
trap cleanup EXIT

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- same-char-3x, THREE DLLs =="
echo "==   product + trace + input-harness (direct input-memory self-drive)"
echo "==   save=default APPDATA (game-owned)  slot=$BOOT_SLOT  cap=${CAP_SECONDS}s"
echo "==   INPUT WILL BE DRIVEN (direct keystate-bitmap injection) -- agent-owned bounded run"
echo "==   tab-switch finish is a KNOWN GAP (mouse-only; see er-input-harness.log)"
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

# RENDERDOC=1: renderdoc.dll is loaded as the FIRST me3 native (see the profile above) so RenderDoc hooks
# ER's D3D12 device at process init; the telemetry DLL then auto-fires TriggerCapture at the reload
# playable window (bd RENDERDOC-inject-via-me3-native). ER is NATIVE WINDOWS -> Windows RenderDoc. The
# .rdc must land on a Windows-accessible path (GAME_DIR under /mnt/c), NOT the WSL artifact dir; copied
# back after the run. ER_RENDERDOC_CAPFILE (a Windows path) is read inside ER by the telemetry DLL.
if [[ "${RENDERDOC:-0}" == "1" ]]; then
	RDOC_CAP_WSL="$GAME_DIR/er_cap"
	rm -f "$GAME_DIR"/er_cap_frame*.rdc # fresh captures this run
	LAUNCH_ENV_VARS+=("ER_RENDERDOC_CAPFILE=$(win_path "$RDOC_CAP_WSL")")
	# RenderDoc BLOCKS ER's OLD amd_ags_x64.dll ("Blocked attempt to initialise old version of AGS") ->
	# ER's AMD device setup falls over -> DXGI_DEVICE_REMOVED (2026-07-22). ER REQUIRES AGS (removing it =
	# ER won't start), so SWAP in a newer RenderDoc-compatible amd_ags_x64.dll for the capture and RESTORE
	# the original on ANY exit via trap. RENDERDOC_AGS_DLL overrides the staged newer DLL; RENDERDOC_KEEP_AGS=1
	# opts out (then RenderDoc will device-remove on ER's old AGS).
	# STUB amd_ags_x64.dll (er-ags-stub): exports every name ER imports but agsInit reports "no AMD driver"
	# so ER takes its non-AGS D3D12 path -> no driver escape for RenderDoc to block. NOT the newer real AGS
	# (that dropped ER's 5.x export agsDeInit -> ER won't load).
	RDOC_AGS_NEW="${RENDERDOC_AGS_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/amd_ags_x64.dll}"
	if [[ "${RENDERDOC_KEEP_AGS:-0}" != "1" && -f "$GAME_DIR/amd_ags_x64.dll" && -f "$RDOC_AGS_NEW" ]]; then
		cp -f "$GAME_DIR/amd_ags_x64.dll" "$GAME_DIR/amd_ags_x64.dll.orig-bak"
		cp -f "$RDOC_AGS_NEW" "$GAME_DIR/amd_ags_x64.dll"
		trap 'mv -f "$GAME_DIR/amd_ags_x64.dll.orig-bak" "$GAME_DIR/amd_ags_x64.dll" 2>/dev/null || true' EXIT
		echo "==   RENDERDOC: swapped in STUB amd_ags_x64.dll ($(stat -c%s "$RDOC_AGS_NEW")B); ER's original restored on exit"
	elif [[ "${RENDERDOC_KEEP_AGS:-0}" != "1" && ! -f "$RDOC_AGS_NEW" ]]; then
		echo "==   RENDERDOC: WARNING no stub AGS at $RDOC_AGS_NEW -- RenderDoc will block ER's old AGS -> device-removed"
	fi
	echo "==   RENDERDOC=1: renderdoc.dll first me3 native; telemetry auto-TriggerCapture at the reload window -> $GAME_DIR/er_cap_frameN.rdc (copied to artifacts)"
fi
(
	cd "$GAME_DIR" &&
		env "${LAUNCH_ENV_VARS[@]}" "$ME3" --steam-dir "$(me3_path_arg "$ME3_STEAM_DIR")" launch \
			-g eldenring \
			-e "$(me3_path_arg "$GAME_DIR/eldenring.exe")" \
			--online false \
			-p "$(me3_profile_arg "$PROFILE")"
) >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &

CAPTURE_ARGS=()
if [[ "${OBSERVE_ONLY:-0}" == "1" ]]; then
	# Pure observation of the full load1->load2 sequence (havok teleports, mms) -- no probe/verdict
	# teardowns. Used to test whether load2 shows the same teleport-to-spawn as load1.
	CAPTURE_ARGS+=(--observe-only --observe-seconds "${OBSERVE_SECONDS:-140}")
else
	CAPTURE_ARGS+=(--require-reload-settled)
	# DETERMINISTIC SWITCH DRIVER (2026-07-21, bd DETERMINISTIC-switch-trigger-recipe): drive each
	# subsequent load by writing the product control file (er-quickload-switch-slot.txt) once the prior
	# load proves movement, instead of the flaky input-harness menu-nav. DRIVE_RELOAD_SLOTS default
	# '0,0' = load2+load3 reload angrE slot 0 (the 3x-angrE goal); set DRIVE_RELOAD_SLOTS='' to fall
	# back to the legacy menu-nav. DRIVE_CROSS_SAVE_FILE (Windows path to a NON-angrE .sl2/.co2) +
	# DRIVE_CROSS_SAVE_SLOT add the final cross-save load. The input-harness DLL still drives the 3s
	# forward-movement proof; only the SWITCH trigger moves to the control file.
	if [[ -n "$DRIVE_RELOAD_SLOTS" ]]; then
		CAPTURE_ARGS+=(--drive-reload-slots "$DRIVE_RELOAD_SLOTS")
	fi
	if [[ -n "${DRIVE_CROSS_SAVE_FILE:-}" && -n "${DRIVE_CROSS_SAVE_SLOT:-}" ]]; then
		CAPTURE_ARGS+=(--drive-cross-save-file "$DRIVE_CROSS_SAVE_FILE" \
			--drive-cross-save-slot "$DRIVE_CROSS_SAVE_SLOT")
	fi
fi
python3 "$REPO_ROOT/scripts/capture-samechar-3x.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--report "$ARTIFACT_DIR/samechar-3x-report.md" \
	"${CAPTURE_ARGS[@]}"
RC=$?

# FALLBACK ONLY. Both logs are redirected into ARTIFACT_DIR at launch, so this copy is for the case
# where the env did not survive launch.sh -> me3 -> Proton and the DLL fell back to GAME_DIR. It is
# NOT how the evidence is preserved: a copy here can only ever recover THIS run's output, never the
# previous run's, and a crashed or killed run never reaches it.
[[ -f "$ARTIFACT_DIR/er-input-harness.log" ]] ||
	{ [[ -f "$GAME_DIR/er-input-harness.log" ]] && cp -f "$GAME_DIR/er-input-harness.log" "$ARTIFACT_DIR/er-input-harness.log"; }
[[ -f "$ARTIFACT_DIR/er-reload-trace.log" ]] ||
	{ [[ -f "$GAME_DIR/er-reload-trace.log" ]] && cp -f "$GAME_DIR/er-reload-trace.log" "$ARTIFACT_DIR/er-reload-trace.log"; }
# RENDERDOC: the Windows ER wrote .rdc captures to GAME_DIR (/mnt/c, Windows-writable); move them to the
# WSL artifact dir for offline diff with qrenderdoc.exe / the RenderDoc python API.
if [[ "${RENDERDOC:-0}" == "1" ]]; then
	rdc_n=0
	for r in "$GAME_DIR"/er_cap_frame*.rdc; do
		[[ -f "$r" ]] || continue
		mv -f "$r" "$ARTIFACT_DIR/" && rdc_n=$((rdc_n + 1))
	done
	echo "== RenderDoc: $rdc_n .rdc capture(s) -> $ARTIFACT_DIR (0 = renderdoc.dll did not hook / TriggerCapture never fired -- check er-quickload-telemetry oracle_renderdoc_captures)"
	if [[ -f "$GAME_DIR/er-antiarxan.txt" ]]; then
		cp -f "$GAME_DIR/er-antiarxan.txt" "$ARTIFACT_DIR/"
		echo "== antiarxan: $(cat "$GAME_DIR/er-antiarxan.txt")"
	else
		echo "== antiarxan: marker ABSENT (er_antiarxan DllMain did not run / .text not patched)"
	fi
fi

# DLL VERSION MANIFEST (user 2026-07-19: track exact binaries per run so a result can be tied to a
# specific build during bisection). Records git HEAD, the in-process DLL build id (dll:XXXX from the
# debug log), and each staged DLL's mtime + short sha256.
REL_DIR="$REPO_ROOT/target/x86_64-pc-windows-msvc/release"
{
	echo "git_head: $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
	echo "dll_build_id: $(grep -oE 'dll:[0-9a-f]{6,}' "$ARTIFACT_DIR/er-quickload-autoload-debug.log" 2>/dev/null | head -1 || echo '?')"
	for d in er_quickload.dll er_reload_trace.dll er_input_harness.dll; do
		if [[ -f "$REL_DIR/$d" ]]; then
			echo "$d: mtime=$(date -r "$REL_DIR/$d" +%Y%m%d-%H%M%S 2>/dev/null) sha=$(sha256sum "$REL_DIR/$d" 2>/dev/null | cut -c1-16)"
		fi
	done
} > "$ARTIFACT_DIR/dll-versions.txt"
echo "== DLL versions: $(tr '\n' '; ' < "$ARTIFACT_DIR/dll-versions.txt")"

echo "== capture done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
