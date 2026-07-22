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
[[ -f "$BOOT_FILE" ]] || fail "boot save not found: $BOOT_FILE"

ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"

mkdir -p "$ARTIFACT_DIR"
win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- stage ALL THREE DLLs to GAME_DIR + a THREE-native me3 profile (product FIRST) ---
PRODUCT_GAMEDIR="$GAME_DIR/er_effects_rs.dll"
TRACE_GAMEDIR="$GAME_DIR/er_reload_trace_dll.dll"
HARNESS_GAMEDIR="$GAME_DIR/er_input_harness_dll.dll"
cp -f "$PRODUCT_DLL" "$PRODUCT_GAMEDIR"
cp -f "$TRACE_DLL" "$TRACE_GAMEDIR"
cp -f "$HARNESS_DLL" "$HARNESS_GAMEDIR"
PROFILE="$ARTIFACT_DIR/samechar-3x-threedll.me3"
# Product FIRST so its er_effects_union_register export is mapped before the companions' install
# threads resolve it (union chaining is load-order-safe either way; this just avoids the resolve poll).
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

# shellcheck disable=SC2317
cleanup() {
	taskkill.exe /F /IM eldenring.exe >/dev/null 2>&1
	taskkill.exe /F /IM me3.exe >/dev/null 2>&1
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

"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" >"$ARTIFACT_DIR/me3-launch.log" 2>&1 &

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
