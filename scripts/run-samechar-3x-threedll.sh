#!/usr/bin/env bash
# Same-character-3x runner, THREE DLLs via me3 -- the multi-DLL-per-feature architecture
# (bd multi-dll-separate-crates-per-feature-single-me3-profile-2026-07-19). Sibling of
# run-samechar-3x-twodll.sh; the difference is a THIRD native and NO env/marker arming.
#
#   1. er_effects_rs.dll         (PRODUCT): boot autoload = load1; owns the single MinHook instance +
#                                the er_effects_union_register export; its ProfileSelect hooks arm the
#                                native reload from menu transitions.
#   2. er_reload_trace_dll.dll   (COMPANION, log-only): unions its load/menu trace hooks through the
#                                product export and logs the pipeline.
#   3. er_input_harness_dll.dll  (COMPANION, self-drive): DEFAULT-ON by PRESENCE (no env/marker gate).
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

CORPUS_ROOT="${ER_SAVE_CORPUS_ROOT:-/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files}"
BOOT_FILE="${BOOT_FILE:-$CORPUS_ROOT/100-Lilbro/ER0000.sl2}"
BOOT_SLOT="${BOOT_SLOT:-0}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/samechar-3x-threedll-$(date +%Y%m%d-%H%M%S)}"
PRODUCT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"
TRACE_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_reload_trace_dll.dll"
HARNESS_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_input_harness_dll.dll"
# TELEMETRY (semaphore DLL): standalone read-side oracle -- writes er-telemetry-timeseries.jsonl with
# fixed_spf / now_loading / play_time AND per-core CPU (oracle_core_max_busy / proc_cpu_cores), aligned by
# oracle_tick_ms, so a product load2/load3 run can be tested for single-core contention (bd NEXT-telemetry
# -capture-per-core-cpu). Shipped alongside the product per the goal (product + semaphore/oracle DLLs).
TELEM_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry_dll.dll"
# RENDERDOC=1: the Windows RenderDoc DLL, loaded as a me3 native to hook ER's D3D12 device.
RDOC_DLL="${RENDERDOC_DLL:-/mnt/c/Program Files/RenderDoc/renderdoc.dll}"
CAP_SECONDS="$(cat "$REPO_ROOT/.auto/runtime_timeout_cap_seconds" 2>/dev/null || echo 180)"

fail() {
	echo "run-samechar-3x-threedll: $*" >&2
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
[[ -f "$HARNESS_DLL" ]] || fail "input-harness DLL not built: $HARNESS_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-input-harness-dll)"
[[ -f "$TELEM_DLL" ]] || fail "telemetry DLL not built: $TELEM_DLL (cargo xwin build --release --target x86_64-pc-windows-msvc -p er-telemetry-dll)"
[[ "${RENDERDOC:-0}" != "1" || -f "$RDOC_DLL" ]] || fail "RENDERDOC=1 but renderdoc.dll not found at '$RDOC_DLL' (set RENDERDOC_DLL=<path to Windows renderdoc.dll>)."
[[ -f "$BOOT_FILE" ]] || fail "boot save not found: $BOOT_FILE"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage ALL THREE DLLs to GAME_DIR + a THREE-native me3 profile (product FIRST) ---
PRODUCT_GAMEDIR="$GAME_DIR/er_effects_rs.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace_dll.dll"
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness_dll.dll"
TELEM_GAMEDIR="$GAME_DIR/er_telemetry_dll.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
cp -f "$TELEM_DLL" "$TELEM_GAMEDIR"
rm -f "$GAME_DIR/er-telemetry-timeseries.jsonl" # fresh per-run core/fps timeseries
# COMPANION: the harness auto-detects the product DLL is loaded and stands down (passive) on its own -- a
# real runtime condition, not a marker file. Clear any stale standalone-run mode flag so nothing leaks in.
rm -f "$GAME_DIR/er-harness-drive-mode.txt"
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

# --- boot TOML (in-memory read-only redirect) for load1 ---
[[ -f "$GAME_DIR/er-effects.toml" ]] && cp -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
{
	echo "# staged by run-samechar-3x-threedll.sh"
	echo "save_file = '$(win_path "$BOOT_FILE")'"
	echo "slot = $BOOT_SLOT"
} >"$GAME_DIR/er-effects.toml"

# NO env/marker arming: the input-harness DLL is enabled purely by its PRESENCE in the profile above.
# Sweep any stale legacy sq-repro/probe markers so a prior run cannot pollute this one.
rm -f "$GAME_DIR"/er-effects-system-quit-repro.txt "$GAME_DIR"/er-effects-system-quit-load-switch.txt \
	"$GAME_DIR"/er-effects-sq-target-switches.txt "$GAME_DIR"/er-effects-sq-target-slots.txt \
	"$GAME_DIR"/er-effects-prove-movement.txt "$GAME_DIR"/er-effects-stay-active.txt \
	"$GAME_DIR"/er-effects-probe-foreground.txt 2>/dev/null

# --- CLEAN SLATE: recreate every log so no PRIOR run pollutes this one. ---
rm -f "$GAME_DIR"/er-effects-*.log "$GAME_DIR"/er-reload-trace.log "$GAME_DIR"/er-input-harness.log \
	"$GAME_DIR"/er-effects-telemetry.json 2>/dev/null

# SAFETY (bd never-blanket-kill-eldenring-killed-user-game-2026-07-22): capture the eldenring.exe/me3
# PIDs that already exist BEFORE we launch (a user's live game, another agent's run) so teardown can
# NEVER touch them. A blanket `taskkill /IM eldenring.exe` here once killed the user's active session.
win_pids_for() {
	tasklist.exe /FI "IMAGENAME eq $1" /FO CSV /NH 2>/dev/null |
		python3 -c "import sys,csv; print(' '.join(r[1] for r in csv.reader(sys.stdin) if len(r)>1 and r[1].isdigit()))"
}
PRE_ER_PIDS=" $(win_pids_for eldenring.exe) "
PRE_ME3_PIDS=" $(win_pids_for me3.exe) $(win_pids_for me3-launcher.exe) "

# shellcheck disable=SC2317
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
	[[ -f "$ARTIFACT_DIR/er-effects.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-effects.toml.bak" "$GAME_DIR/er-effects.toml"
}
trap cleanup EXIT

echo "======================================================================"
echo "== LAUNCHING ELDEN RING (offline, me3) -- same-char-3x, THREE DLLs =="
echo "==   product + trace + input-harness (direct input-memory self-drive)"
echo "==   boot=$BOOT_FILE slot=$BOOT_SLOT  cap=${CAP_SECONDS}s"
echo "==   INPUT WILL BE DRIVEN (direct keystate-bitmap injection) -- agent-owned bounded run"
echo "==   tab-switch finish is a KNOWN GAP (mouse-only; see er-input-harness.log)"
echo "==   artifacts -> $ARTIFACT_DIR"
echo "======================================================================"

# RENDERDOC=1: renderdoc.dll is loaded as the FIRST me3 native (see the profile above) so RenderDoc hooks
# ER's D3D12 device at process init; the telemetry DLL then auto-fires TriggerCapture at the reload
# playable window (bd RENDERDOC-inject-via-me3-native). ER is NATIVE WINDOWS -> Windows RenderDoc. The
# .rdc must land on a Windows-accessible path (GAME_DIR under /mnt/c), NOT the WSL artifact dir; copied
# back after the run. ER_RENDERDOC_CAPFILE (a Windows path) is read inside ER by the telemetry DLL.
RDOC_LAUNCH_ENV=()
if [[ "${RENDERDOC:-0}" == "1" ]]; then
	RDOC_CAP_WSL="$GAME_DIR/er_cap"
	rm -f "$GAME_DIR"/er_cap_frame*.rdc # fresh captures this run
	RDOC_LAUNCH_ENV=(env "ER_RENDERDOC_CAPFILE=$(win_path "$RDOC_CAP_WSL")")
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
"${RDOC_LAUNCH_ENV[@]}" "$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &

CAPTURE_ARGS=()
if [[ "${OBSERVE_ONLY:-0}" == "1" ]]; then
	# Pure observation of the full load1->load2 sequence (havok teleports, mms) -- no probe/verdict
	# teardowns. Used to test whether load2 shows the same teleport-to-spawn as load1.
	CAPTURE_ARGS+=(--observe-only --observe-seconds "${OBSERVE_SECONDS:-140}")
else
	CAPTURE_ARGS+=(--require-reload-settled)
	# DETERMINISTIC SWITCH DRIVER (2026-07-21, bd DETERMINISTIC-switch-trigger-recipe): drive each
	# subsequent load by writing the product control file (er-effects-switch-slot.txt) once the prior
	# load proves movement, instead of the flaky input-harness menu-nav. DRIVE_RELOAD_SLOTS default
	# '0,0' = load2+load3 reload angrE slot 0 (the 3x-angrE goal); set DRIVE_RELOAD_SLOTS='' to fall
	# back to the legacy menu-nav. DRIVE_CROSS_SAVE_FILE (Windows path to a NON-angrE .sl2/.co2) +
	# DRIVE_CROSS_SAVE_SLOT add the final cross-save load. The input-harness DLL still drives the 3s
	# forward-movement proof; only the SWITCH trigger moves to the control file.
	DRIVE_RELOAD_SLOTS="${DRIVE_RELOAD_SLOTS-0,0}"
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

# Preserve the harness self-drive evidence log alongside the trace + report.
[[ -f "$GAME_DIR/er-input-harness.log" ]] && cp -f "$GAME_DIR/er-input-harness.log" "$ARTIFACT_DIR/er-input-harness.log"
[[ -f "$GAME_DIR/er-reload-trace.log" ]] && cp -f "$GAME_DIR/er-reload-trace.log" "$ARTIFACT_DIR/er-reload-trace.log"
# RENDERDOC: the Windows ER wrote .rdc captures to GAME_DIR (/mnt/c, Windows-writable); move them to the
# WSL artifact dir for offline diff with qrenderdoc.exe / the RenderDoc python API.
if [[ "${RENDERDOC:-0}" == "1" ]]; then
	rdc_n=0
	for r in "$GAME_DIR"/er_cap_frame*.rdc; do
		[[ -f "$r" ]] || continue
		mv -f "$r" "$ARTIFACT_DIR/" && rdc_n=$((rdc_n + 1))
	done
	echo "== RenderDoc: $rdc_n .rdc capture(s) -> $ARTIFACT_DIR (0 = renderdoc.dll did not hook / TriggerCapture never fired -- check er-effects-telemetry oracle_renderdoc_captures)"
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
	echo "dll_build_id: $(grep -oE 'dll:[0-9a-f]{6,}' "$ARTIFACT_DIR/er-effects-autoload-debug.log" 2>/dev/null | head -1 || echo '?')"
	for d in er_effects_rs.dll er_reload_trace_dll.dll er_input_harness_dll.dll; do
		if [[ -f "$REL_DIR/$d" ]]; then
			echo "$d: mtime=$(date -r "$REL_DIR/$d" +%Y%m%d-%H%M%S 2>/dev/null) sha=$(sha256sum "$REL_DIR/$d" 2>/dev/null | cut -c1-16)"
		fi
	done
} > "$ARTIFACT_DIR/dll-versions.txt"
echo "== DLL versions: $(tr '\n' '; ' < "$ARTIFACT_DIR/dll-versions.txt")"

echo "== capture done rc=$RC ; artifacts in $ARTIFACT_DIR =="
exit "$RC"
