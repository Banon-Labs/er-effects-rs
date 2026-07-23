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
#   1. er-effects-telemetry-only.txt  -> ER_EFFECTS_TELEMETRY_ONLY: DISARMS the product autoload (product
#      product_autoload_gates.rs), so the game boots to the NATIVE title and the product loads NO character
#      of its own -- it only emits the rich oracle_* telemetry. With the 2026-07-23 present-hook decoupling
#      (bd present-cadence-gx-instrumentation-coupled-...), the present detour still installs under
#      telemetry-only to record present-cadence + GX semaphores, but SKIPS the overlay composite -> flow-
#      faithful vanilla with FULL cadence telemetry.
#   2. er-harness-drive-mode.txt = "full" -> input-harness FullBootReload DRIVE mode (drive.rs): the
#      HARNESS drives title->Continue->play->System->Quit->Continue via the raw pad device (Up/Confirm,
#      TabLeft to the Quit tab, Down/Confirm), each step gated on its own pane semaphore. NOT companion mode.
#   3. NO er-effects.toml redirect: the game reads the REAL APPDATA active save (pure vanilla), not a
#      staged/redirected source. Whatever character is last-active in APPDATA is the vanilla Continue target.
#   4. capture-samechar-3x.py --observe-only: records the full timeseries with NO probe/verdict/fps
#      teardowns (the harness drives; the capture just observes) -> the vanilla native reload sequence.
#
# The resulting telemetry-timeseries.jsonl is the VANILLA baseline for scripts/oracle-steadystate-diff.py
# (steady-state) and scripts/oracle-compare.py (trajectory). REQUIRES: Steam running; correct GAME_DIR.
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/vanilla-reload-agentdriven-$(date +%Y%m%d-%H%M%S)}"
PRODUCT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"
TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace_dll.dll"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness_dll.dll"
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll"
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
for d in "$PRODUCT_DLL" "$TRACE_DLL" "$HARNESS_DLL" "$TELEM_DLL"; do
	[[ -f "$d" ]] || fail "DLL not built: $d (cargo xwin build --release --target x86_64-pc-windows-msvc)"
done
if tasklist.exe 2>/dev/null | grep -qiE 'eldenring\.exe|start_protected_game\.exe'; then
	fail "An Elden Ring process is already running. Tear it down before launching (never a blanket kill)."
fi

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage the 4 DLLs + a 4-native me3 profile (product FIRST for the union export) ---
PRODUCT_GAMEDIR="$GAME_DIR/er_effects_rs.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace_dll.dll"
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness_dll.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry_dll.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
rm -f "$GAME_DIR/er-telemetry-timeseries.jsonl"

PROFILE="$ARTIFACT_DIR/vanilla-reload-agentdriven.me3"
# RENDERDOC=1: stage renderdoc.dll as the FIRST me3 native so it hooks ER's D3D12 device at init; the
# telemetry DLL auto-fires TriggerCapture at a steady reload frame (ER_RENDERDOC_SLOW_MS=0 => any steady
# frame, incl. the fast vanilla). Under RenderDoc the product render hooks stand down (renderdoc_active),
# so the .rdc shows the GAME's own render -> diff a vanilla-reload vs mod-reload frame (bd CORRECTION-
# RenderDoc-GPU-frame-analysis-IS-AUTONOMOUS). .rdc lands in GAME_DIR (Windows-writable).
RDOC_DLL="${RENDERDOC_DLL:-/mnt/c/Program Files/RenderDoc/renderdoc.dll}"
if [[ "${RENDERDOC:-0}" == "1" ]]; then
	[[ -f "$RDOC_DLL" ]] || fail "RENDERDOC=1 but renderdoc.dll not found at '$RDOC_DLL'"
	rm -f "$GAME_DIR"/er_cap_frame*.rdc
	echo -n "15" >"$GAME_DIR/er-effects-rdoc-slow-ms.txt"
fi
{
	echo 'profileVersion = "v1"'
	echo
	echo '[[supports]]'
	echo 'game = "eldenring"'
	echo
	if [[ "${RENDERDOC:-0}" == "1" ]]; then
		echo '[[natives]]'
		echo "path = '$(win_path "$RDOC_DLL")'"
		echo
	fi
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
[[ -z "${MOD_ARMED:-}" ]] && : >"$GAME_DIR/er-effects-telemetry-only.txt"
[[ -n "${NO_COMPOSITE:-}" ]] && : >"$GAME_DIR/er-effects-measure-no-composite.txt"
# harness drive mode (DRIVE_MODE env, default 'full'): 'full' drives the whole
# boot->Continue->play->System->Quit->Continue reload; 'boot' drives boot->Continue and holds in-world
# (no quit) -- use 'boot' for a CLEAN vanilla in-world steady-state window when the reload nav derails.
echo -n "${DRIVE_MODE:-full}" >"$GAME_DIR/er-harness-drive-mode.txt"
# PROBE HOLD-ID (diagnostic): with DRIVE_MODE=probe, HOLD_VKID=<1000..1080> holds one vk-id instead of
# sweeping, to isolate which index drives a menu action (e.g. HOLD_VKID=1034 tests return-to-title).
[[ -n "${HOLD_VKID:-}" ]] && echo -n "${HOLD_VKID}" >"$GAME_DIR/er-harness-probe-hold-id.txt"
[[ -n "${OS_INPUT:-}" ]] && : >"$GAME_DIR/er-harness-os-input.txt"
[[ -n "${NATIVE_QUIT:-}" ]] && : >"$GAME_DIR/er-harness-native-quit.txt"
# FORCE-DRIVE: the harness normally stands down to Passive when the product DLL is loaded (companion
# design). This vanilla capture loads the product for its telemetry but needs the HARNESS to drive, so
# override that stand-down (bd VANILLA-BASELINE-blocked-harness-forces-passive-when-product-loaded).
: >"$GAME_DIR/er-harness-force-drive.txt"
# NO save redirect: pure APPDATA vanilla save. Move any staged toml aside so nothing redirects.
[[ -f "$GAME_DIR/er-effects.toml" ]] && mv -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
# Sweep stale probe/switch markers so a prior run cannot pollute this vanilla capture.
rm -f "$GAME_DIR"/er-effects-system-quit-repro.txt "$GAME_DIR"/er-effects-system-quit-load-switch.txt \
	"$GAME_DIR"/er-effects-switch-slot.txt "$GAME_DIR"/er-effects-switch-save-file.txt \
	"$GAME_DIR"/er-effects-prove-movement.txt 2>/dev/null
rm -f "$GAME_DIR"/er-effects-*.log "$GAME_DIR"/er-reload-trace.log "$GAME_DIR"/er-input-harness.log \
	"$GAME_DIR"/er-effects-telemetry.json 2>/dev/null

# SAFETY (bd never-blanket-kill-eldenring): only tear down the PIDs THIS run spawns.
win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}
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
	# Restore: remove vanilla markers so a later product run is not accidentally telemetry-only / driven.
	rm -f "$GAME_DIR/er-effects-measure-no-composite.txt" "$GAME_DIR/er-effects-telemetry-only.txt" "$GAME_DIR/er-harness-drive-mode.txt" \
		"$GAME_DIR/er-harness-force-drive.txt" "$GAME_DIR/er-harness-probe-hold-id.txt" "$GAME_DIR/er-harness-os-input.txt" "$GAME_DIR/er-harness-native-quit.txt" 2>/dev/null
	[[ -f "$ARTIFACT_DIR/er-effects.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-effects.toml.bak" "$GAME_DIR/er-effects.toml"
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

"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &

python3 "$REPO_ROOT/scripts/capture-samechar-3x.py" \
	--game-dir "$GAME_DIR" \
	--artifact-dir "$ARTIFACT_DIR" \
	--max-seconds "$CAP_SECONDS" \
	--report "$ARTIFACT_DIR/vanilla-reload-report.md" \
	--observe-only --observe-seconds "$OBSERVE_SECONDS"
RC=$?

[[ -f "$GAME_DIR/er-input-harness.log" ]] && cp -f "$GAME_DIR/er-input-harness.log" "$ARTIFACT_DIR/er-input-harness.log"
[[ -f "$GAME_DIR/er-reload-trace.log" ]] && cp -f "$GAME_DIR/er-reload-trace.log" "$ARTIFACT_DIR/er-reload-trace.log"
{
	echo "git_head: $(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?')"
	for d in er_effects_rs.dll er_reload_trace_dll.dll er_input_harness_dll.dll er_telemetry_dll.dll; do
		f="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/$d"
		[[ -f "$f" ]] && echo "$d: mtime=$(date -r "$f" +%Y%m%d-%H%M%S) sha=$(sha256sum "$f" | cut -c1-16)"
	done
} >"$ARTIFACT_DIR/dll-versions.txt"

echo "== vanilla capture done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
