#!/usr/bin/env bash
# AGENT-DRIVEN vanilla native reload capture (Milestone-1 baseline, acceptance 2026-07-22/23).
#
# The Milestone-1 acceptance diff needs a VANILLA imprint -- the game's OWN native
# Continue -> play -> System->Quit->Continue, captured flow-faithfully -- NOT the mod's autoload/reload
# machinery (bd oracle-reference-is-vanilla-continue-not-load1-autoload; the mod's own load1 is
# contaminated, see bd STEADYSTATE-DIFF-TOOL-...-FALSIFIED). This is the agent-driven replacement for the
# deprecated USER-driven scripts/run-vanilla-reload-fps.sh (bd DURABLE-agent-can-do-any-input; the user
# is never asked to drive).
#
# WIRING (differs from run-samechar-3x-threedll.sh):
#   1. er-quickload-telemetry-only.txt  -> ER_QUICKLOAD_TELEMETRY_ONLY: DISARMS the product autoload (product
#      product_autoload_gates.rs), so the game boots to the NATIVE title and the product loads NO character
#      of its own -- it only emits the rich oracle_* telemetry. With the 2026-07-23 present-hook decoupling
#      (bd present-cadence-gx-instrumentation-coupled-...), the present detour still installs under
#      telemetry-only to record present-cadence + GX semaphores, but SKIPS the overlay composite -> flow-
#      faithful vanilla with FULL cadence telemetry.
#   2. er-harness-drive-mode.txt = "full" -> input-harness FullBootReload DRIVE mode (drive.rs): the
#      HARNESS drives title->Continue->play->System->Quit->Continue via the raw pad device (Up/Confirm,
#      TabLeft to the Quit tab, Down/Confirm), each step gated on its own pane semaphore. NOT companion mode.
#   3. NO er-quickload.toml redirect: the game reads the REAL APPDATA active save (pure vanilla), not a
#      staged/redirected source. Whatever character is last-active in APPDATA is the vanilla Continue target.
#   4. capture-samechar-3x.py --observe-only: records the full timeseries with NO probe/verdict/fps
#      teardowns (the harness drives; the capture just observes) -> the vanilla native reload sequence.
#
# The resulting telemetry-timeseries.jsonl is the VANILLA baseline for scripts/oracle-steadystate-diff.py
# (steady-state) and scripts/oracle-compare.py (trajectory). REQUIRES: Steam running; correct GAME_DIR.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/vanilla-reload-agentdriven-$(date +%Y%m%d-%H%M%S)}"
PRODUCT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll"
TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace.dll"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness.dll"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry.dll"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 300)"
OBSERVE_SECONDS="${OBSERVE_SECONDS:-$CAP_SECONDS}"

fail() {
	echo "run-vanilla-reload-agentdriven: $*" >&2
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
# FRESHNESS, NOT EXISTENCE. The loop here used to assert only that four files exist -- but the
# profile below points me3 straight at target/.../release, so existence says nothing about which
# code loads. This run produces the VANILLA BASELINE that oracle-steadystate-diff.py subtracts
# from every later product run; a baseline captured against a stale harness or telemetry DLL
# poisons every comparison drawn from it afterwards, silently and for as long as the file is kept.
# All four are checked: the product is present here even in telemetry-only mode.
# shellcheck source=scripts/er-dll-freshness.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/er-dll-freshness.sh"
require_fresh_dlls "$PRODUCT_DLL" "$TRACE_DLL" "$HARNESS_DLL" "$TELEM_DLL" ||
	fail "refusing to launch against DLLs that are not this source tree (see above)"
if tasklist.exe 2>/dev/null | grep -qiE 'eldenring\.exe|start_protected_game\.exe'; then
	fail "An Elden Ring process is already running. Tear it down before launching (never a blanket kill)."
fi

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage the 4 DLLs + a 4-native me3 profile (product FIRST for the union export) ---
PRODUCT_GAMEDIR="$GAME_DIR/er_quickload.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace.dll"
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
# EVERY per-run artifact goes into THIS run's directory. A GAME_DIR artifact is SINGLE-SLOT: the DLL
# rotates `<name>` to `<name>.prev` on its first write, so two launches lose the run before last, and
# several sessions launch concurrently here. A copy after the run cannot fix it -- by then this run
# clobbered the previous one's file -- and a crashed run never reaches the copy at all. This line used
# to `rm -f "$GAME_DIR/er-telemetry-timeseries.jsonl"`, which was ANOTHER run's evidence.
LAUNCH_ENV_VARS=(
	"ER_QUICKLOAD_TELEMETRY_PATH=$ARTIFACT_DIR/er-quickload-telemetry.json"
	"ER_QUICKLOAD_AUTOLOAD_DEBUG_PATH=$ARTIFACT_DIR/er-quickload-autoload-debug.log"
	"ER_QUICKLOAD_CRASH_LOG_PATH=$ARTIFACT_DIR/er-quickload-crash-log.txt"
	"ER_QUICKLOAD_TRACE_CONTINUE_PATH=$ARTIFACT_DIR/er-quickload-continue-trace.log"
	"ER_QUICKLOAD_INPUT_TRACE_PATH=$ARTIFACT_DIR/er-quickload-input-trace.jsonl"
	"ER_QUICKLOAD_BOOTSTRAP_PATH=$ARTIFACT_DIR/er-quickload-bootstrap.jsonl"
	"ER_QUICKLOAD_BOOTSTRAP_STATE_PATH=$ARTIFACT_DIR/er-quickload-bootstrap-state.json"
	"ER_QUICKLOAD_PROFILE_PATH=$ARTIFACT_DIR/er-quickload-profile.jsonl"
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

PROFILE="$ARTIFACT_DIR/vanilla-reload-agentdriven.me3"
# NOTE: the old RENDERDOC=1 me3-native path (stage renderdoc.dll as the first native so it hooks ER's
# D3D12 device at init) was REMOVED 2026-07-23: it STALLS ER's boot (RenderDoc-at-device-creation on ER's
# heavy asset load hangs; user confirmed "Game stalled on boot", run59). RenderDoc must be run the
# NATIVE-WINDOWS way (renderdoccmd inject/capture against eldenring.exe), NOT injected through me3.
# bd STEP4-me3-native-renderdoc-dll-STALLS-boot.
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
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$HARNESS_GAMEDIR")'"
	echo
	echo '[[natives]]'
	echo "path = '$(win_path "$TELEM_GAMEDIR")'"
} >"$PROFILE"

# --- VANILLA wiring markers ---
# telemetry-only: disarm the product autoload; product emits telemetry (+ present-cadence via the
# decoupled detour) but loads no character. The NATIVE Continue is driven by the harness below.
# MOD_ARMED=1: skip telemetry-only so the PRODUCT is ARMED (autoload + full composite) -- the mod-side of
# the Milestone-1 A/B diff (vanilla=disarmed run vs mod=armed run, same native reload). Default: disarmed.
[[ -z "${MOD_ARMED:-}" ]] && : >"$GAME_DIR/er-quickload-telemetry-only.txt"
[[ -n "${NO_COMPOSITE:-}" ]] && : >"$GAME_DIR/er-quickload-measure-no-composite.txt"
# harness drive mode (DRIVE_MODE env, default 'full'): 'full' drives the whole
# boot->Continue->play->System->Quit->Continue reload; 'boot' drives boot->Continue and holds in-world
# (no quit) -- use 'boot' for a CLEAN vanilla in-world steady-state window when the reload nav derails.
echo -n "${DRIVE_MODE:-full}" >"$GAME_DIR/er-harness-drive-mode.txt"
# PROBE HOLD-ID (diagnostic): with DRIVE_MODE=probe, HOLD_VKID=<1000..1080> holds one vk-id instead of
# sweeping, to isolate which index drives a menu action (e.g. HOLD_VKID=1034 tests return-to-title).
[[ -n "${HOLD_VKID:-}" ]] && echo -n "${HOLD_VKID}" >"$GAME_DIR/er-harness-probe-hold-id.txt"
[[ -n "${OS_INPUT:-}" ]] && : >"$GAME_DIR/er-harness-os-input.txt"
[[ -n "${NATIVE_QUIT:-}" ]] && : >"$GAME_DIR/er-harness-native-quit.txt"
if [[ -n "${DIAG_NO_AUTOLOAD:-}" ]]; then : >"$GAME_DIR/er-quickload-diag-no-autoload.txt"; else rm -f "$GAME_DIR/er-quickload-diag-no-autoload.txt"; fi
# CLEAN-A/B: DISABLE_SWITCH_OWNLOAD=1 skips the menu-free own_load_switch_reload_fire so the harness's
# menu-driven Continue is the sole reload path (isolates menu-free vs menu-driven; bd STEP4-RUNTIME-TRACE).
if [[ -n "${DISABLE_SWITCH_OWNLOAD:-}" ]]; then : >"$GAME_DIR/er-quickload-disable-switch-reload-ownload.txt"; else rm -f "$GAME_DIR/er-quickload-disable-switch-reload-ownload.txt"; fi
# FORCE-DRIVE: the harness normally stands down to Passive when the product DLL is loaded (companion
# design). This vanilla capture loads the product for its telemetry but needs the HARNESS to drive, so
# override that stand-down (bd VANILLA-BASELINE-blocked-harness-forces-passive-when-product-loaded).
: >"$GAME_DIR/er-harness-force-drive.txt"
# Save source: default = pure APPDATA vanilla save (whatever is last-active). If BOOT_FILE is set,
# write an in-memory READ-ONLY redirect to it (e.g. the angrE 100-Lilbro corpus save) so D_van is
# measured on the SAME character as D_mod -- and WITHOUT writing the live APPDATA save (save-safer).
[[ -f "$GAME_DIR/er-quickload.toml" ]] && cp -f "$GAME_DIR/er-quickload.toml" "$ARTIFACT_DIR/er-quickload.toml.bak"
if [[ -n "${BOOT_FILE:-}" ]]; then
	[[ -f "$BOOT_FILE" ]] || {
		echo "BOOT_FILE not found: $BOOT_FILE" >&2
		exit 1
	}
	echo "save_file = '$(win_path "$BOOT_FILE")'" >"$GAME_DIR/er-quickload.toml"
	echo "==   SAVE REDIRECT (read-only, D_van same-char as D_mod): $BOOT_FILE"
else
	rm -f "$GAME_DIR/er-quickload.toml"
fi
# Sweep stale probe/switch markers so a prior run cannot pollute this vanilla capture.
rm -f "$GAME_DIR"/er-quickload-system-quit-repro.txt "$GAME_DIR"/er-quickload-system-quit-load-switch.txt \
	"$GAME_DIR"/er-quickload-switch-slot.txt "$GAME_DIR"/er-quickload-switch-save-file.txt \
	"$GAME_DIR"/er-quickload-prove-movement.txt 2>/dev/null
# THE GAME_DIR LOG SWEEP IS GONE ON PURPOSE. It used to clear er-quickload-*.log, er-reload-trace.log,
# er-input-harness.log and the telemetry json out of the game directory, which destroyed TWO
# generations of somebody ELSE's run at a time: `begin_fresh_run` removes `<name>.prev`
# unconditionally when the live file is absent. Every log this run writes is redirected into a fresh
# per-run ARTIFACT_DIR, so the clean slate is free.
# SAFETY (bd never-blanket-kill-eldenring): only tear down the PIDs THIS run spawns.
win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}
PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "

# Invoked only through the EXIT trap below, which shellcheck cannot see.
# shellcheck disable=SC2317,SC2329
cleanup() {
	local pid
	for pid in $(win_pids_for eldenring.exe); do
		[[ "$PRE_ER_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	for pid in $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe); do
		[[ "$PRE_ME3_PIDS" == *" $pid "* ]] || taskkill.exe /F /PID "$pid" >/dev/null 2>&1
	done
	# Restore: remove vanilla markers so a later product run is not accidentally telemetry-only / driven.
	rm -f "$GAME_DIR/er-quickload-measure-no-composite.txt" "$GAME_DIR/er-quickload-telemetry-only.txt" "$GAME_DIR/er-harness-drive-mode.txt" \
		"$GAME_DIR/er-harness-force-drive.txt" "$GAME_DIR/er-harness-probe-hold-id.txt" "$GAME_DIR/er-harness-os-input.txt" "$GAME_DIR/er-harness-native-quit.txt" 2>/dev/null
	# Only the teardown-on-title-rebuild path creates the read-side oracle markers; remove them so a later
	# run is not accidentally left emitting oracle streams. Guarded so the default / A/B-driver path (which
	# sets the .on markers itself and shares them across arms) is untouched.
	[[ "${TEARDOWN_ON_TITLE_REBUILD:-0}" == "1" ]] &&
		rm -f "$GAME_DIR/er-oracle-title-binding.on" "$GAME_DIR/er-oracle-stream-overlap.on" "$GAME_DIR/er-oracle-dialog-active.on" 2>/dev/null
	[[ -f "$ARTIFACT_DIR/er-quickload.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-quickload.toml.bak" "$GAME_DIR/er-quickload.toml"
}
trap cleanup EXIT

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- VANILLA agent-driven reload  =="
echo "==   telemetry-only product (no autoload) + harness FULL drive mode    =="
echo "==   harness drives: Continue -> play -> System->Quit -> Continue      =="
echo "==   pure APPDATA vanilla save (no redirect)   cap=${CAP_SECONDS}s     =="
echo "==   INPUT WILL BE DRIVEN (raw-pad taps) -- agent-owned bounded run    =="
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

# RDC_CAPTURE=1: wrap the me3 launch in the NATIVE-Windows RenderDoc `renderdoccmd capture` so RenderDoc
# is present at ER's D3D12 device creation (the ONLY point it can hook -- inject-after-boot cannot hook an
# already-created device, verified run62; me3-native DLL load stalls boot, run59). --opt-hook-children
# propagates the hook through me3 -> me3-launcher -> eldenring (grandchild). The telemetry DLL's
# trigger_capture (re-check enabled) then fires at the slow reload frame (er-quickload-rdoc-slow-ms.txt).
# Accepts a slower boot (RenderDoc serializes D3D12); give a long window. bd VERIFIED-game-is-NATIVE-WINDOWS.
if [[ "${RDC_CAPTURE:-0}" == "1" ]]; then
	RDCMD="${RENDERDOCCMD:-/mnt/c/Program Files/RenderDoc/renderdoccmd.exe}"
	[[ -f "$RDCMD" ]] || fail "RDC_CAPTURE=1 but renderdoccmd.exe not found at '$RDCMD'"
	rm -f "$GAME_DIR"/er_cap_frame*.rdc
	[[ -f "$GAME_DIR/er-quickload-rdoc-slow-ms.txt" ]] || echo -n "15" >"$GAME_DIR/er-quickload-rdoc-slow-ms.txt"
	"$RDCMD" capture --opt-hook-children \
		--capture-file 'C:\SteamLibrary\steamapps\common\ELDEN RING\Game\er_cap_frame' \
		env "${LAUNCH_ENV_VARS[@]}" \
		"$(wslpath -w "$ME3")" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" \
		>"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
else
	env "${LAUNCH_ENV_VARS[@]}" "$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &
fi

if [[ "${TEARDOWN_ON_TITLE_REBUILD:-0}" == "1" ]]; then
	# PROMPT teardown scoped to THIS test's decisive oracle (bd teardown-must-be-prompt-scoped-to-decisive-
	# oracle-no-long-waits-2026-07-24): the dialog+0xb78 title-binding at the WARM switch title, captured
	# seconds after the warm title appears post-quit. The watcher RETURNS PROMPTLY the instant that crux is
	# captured so the PID-scoped cleanup() EXIT trap tears the game down -- instead of the 300s
	# --observe-only ride that lingers 45s past settled world. Enable the two read-side oracles + sweep
	# their stale (append-only, un-truncated) streams so the watcher sees only THIS run.
	: >"$GAME_DIR/er-oracle-title-binding.on"
	: >"$GAME_DIR/er-oracle-stream-overlap.on"
	: >"$GAME_DIR/er-oracle-dialog-active.on"
	rm -f "$GAME_DIR"/er-oracle-title-binding.jsonl "$GAME_DIR"/er-oracle-stream-overlap.jsonl "$GAME_DIR"/er-oracle-dialog-active.jsonl 2>/dev/null
	WATCH_MAX="$CAP_SECONDS"
	[[ "$WATCH_MAX" -gt 180 ]] && WATCH_MAX=180 # clamp the backstop SMALL (NOT the 300s runtime cap)
	python3 "$REPO_ROOT/scripts/wait-title-rebuild-teardown.py" \
		--game-dir "$GAME_DIR" \
		--artifact-dir "$ARTIFACT_DIR" \
		--max-seconds "$WATCH_MAX" \
		--feature "${TEARDOWN_FEATURE:-title-binding}"
	RC=$?
else
	python3 "$REPO_ROOT/scripts/capture-samechar-3x.py" \
		--game-dir "$GAME_DIR" \
		--artifact-dir "$ARTIFACT_DIR" \
		--max-seconds "$CAP_SECONDS" \
		--report "$ARTIFACT_DIR/vanilla-reload-report.md" \
		--observe-only --observe-seconds "$OBSERVE_SECONDS"
	RC=$?
fi

# FALLBACK ONLY -- both are redirected into ARTIFACT_DIR at launch. These cover the case where the
# env did not survive me3 -> Proton and the DLL fell back to the game directory.
[[ -f "$ARTIFACT_DIR/er-input-harness.log" ]] ||
	{ [[ -f "$GAME_DIR/er-input-harness.log" ]] && cp -f "$GAME_DIR/er-input-harness.log" "$ARTIFACT_DIR/er-input-harness.log"; }
[[ -f "$ARTIFACT_DIR/er-reload-trace.log" ]] ||
	{ [[ -f "$GAME_DIR/er-reload-trace.log" ]] && cp -f "$GAME_DIR/er-reload-trace.log" "$ARTIFACT_DIR/er-reload-trace.log"; }
{
	echo "git_head: $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
	for d in er_quickload.dll er_reload_trace.dll er_input_harness.dll er_telemetry.dll; do
		f="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/$d"
		[[ -f "$f" ]] && echo "$d: mtime=$(date -r "$f" +%Y%m%d-%H%M%S) sha=$(sha256sum "$f" | cut -c1-16)"
	done
} >"$ARTIFACT_DIR/dll-versions.txt"

echo "== vanilla capture done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
