#!/usr/bin/env bash
# Launch the APPROVED direct/offline eldenring.exe Proton path with RenderDoc's Vulkan
# capture layer enabled, so a real frame (vkd3d-proton -> native Vulkan) can be captured
# and a single object's draw replayed offline through its native .vpo/.ppo with the GAME'S
# actual lighting cbuffers + IBL/GI textures.
#
# This does NOT auto-tear-down: the game runs on your monitor, you reach a lit spot, then
# YOU trigger the capture (qrenderdoc target control or the F12 hotkey), then tear down
# with `pkill -x eldenring.exe`. There is intentionally NO Steam/AppID/EAC launch path.
#
#   Capture flow:
#     1) ER_QUICKLOAD_GOLD_SAVE=/abs/ER0000.sl2 ./scripts/capture-er-frame.sh
#     2) reach a lit area; trigger a capture:
#          qrenderdoc --targetcontrol localhost:38920   (Queue Capture at a frame)   OR   F12
#     3) pkill -x eldenring.exe
#     4) extract:  QT_QPA_PLATFORM=offscreen qrenderdoc --python scripts/extract-capture.py -- \
#                    <ARTIFACT_DIR>/er_cap_frameN.rdc target/capture/aeg301 --match cbLight
#     5) replay:   cargo run -p er-objectkit --example replay_capture -- target/capture/aeg301
#
# Save handling (per user directive: use the gold save, read+write):
#   default  -> stage a WRITABLE COPY of the gold save (read+write) and redirect the game at
#               it (save-safe: autosaves land in the copy, the gold is only read once).
#   ER_QUICKLOAD_CAPTURE_SAVE_DIRECT=1 -> point the game at the gold save ITSELF, read+write
#               (chmod u+w on the original; the game WILL write/autosave to your real save).
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
# me3 delivers the DLL as a native (LazyLoader removed 2026-07-04).
# shellcheck source=scripts/me3-launch-lib.sh
source "$REPO_ROOT/scripts/me3-launch-lib.sh"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/capture-$(date +%Y%m%d-%H%M%S)}"
BUILT_DLL="${BUILT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_quickload.dll}"
GOLD_SAVE="${ER_QUICKLOAD_GOLD_SAVE:-}"
SAVE_DIRECT="${ER_QUICKLOAD_CAPTURE_SAVE_DIRECT:-0}"
GOLD_SAVE_MIN_BYTES="${GOLD_SAVE_MIN_BYTES:-1048576}"
ACTIVE_STEAMID="${ER_QUICKLOAD_ACTIVE_STEAMID:-76561197986456766}"
APPDATA_ER_ROOT="${APPDATA_ER_ROOT:-$STEAM_COMPAT_DATA_PATH/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing}"
# RenderDoc capture file template; the layer appends _frameN.rdc.
RENDERDOC_CAPFILE="${RENDERDOC_CAPFILE:-$ARTIFACT_DIR/er_cap}"

fatal() { echo "capture-er-frame: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }
require_exec() { [[ -x "$1" ]] || fatal "missing executable: $1"; }

preflight() {
  pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first (the offline launch reuses Steam's environment)"
  me3_preflight || fatal "me3 preflight failed"
  me3_require_no_lazyloader "$GAME_DIR" || fatal "leftover LazyLoader proxy in $GAME_DIR"
  require_file "$GAME_DIR/eldenring.exe"
  require_file "$BUILT_DLL" \
    || fatal "built DLL not found: $BUILT_DLL (run: cargo xwin build --release --target x86_64-pc-windows-msvc)"
  command -v qrenderdoc >/dev/null 2>&1 || fatal "qrenderdoc not in PATH (install the renderdoc package)"
  [[ -f /etc/vulkan/implicit_layer.d/renderdoc_capture.json ]] \
    || fatal "RenderDoc Vulkan implicit layer JSON not found; is renderdoc installed?"
  [[ -d "$STEAM_COMPAT_DATA_PATH" ]] || fatal "missing compatdata path: $STEAM_COMPAT_DATA_PATH"
  [[ -n "$GOLD_SAVE" ]] || fatal "ER_QUICKLOAD_GOLD_SAVE is unset; supply the absolute path to your gold ER0000.sl2"
  [[ -f "$GOLD_SAVE" ]] || fatal "gold save not found: $GOLD_SAVE"
  local bytes; bytes=$(stat -c '%s' "$GOLD_SAVE" 2>/dev/null || echo 0)
  (( bytes >= GOLD_SAVE_MIN_BYTES )) || fatal "gold save too small ($bytes bytes): $GOLD_SAVE"
  if pgrep -x eldenring.exe >/dev/null 2>&1; then
    fatal "eldenring.exe is already running; refusing to mix ownership (another agent may be using it)"
  fi
}

preflight
mkdir -p "$ARTIFACT_DIR"
cp -f "$BUILT_DLL" "$ARTIFACT_DIR/er_quickload.dll"
me3_write_profile "$ARTIFACT_DIR/er-quickload-capture.me3" "$ARTIFACT_DIR/er_quickload.dll"
echo "deploy: staged fresh DLL + me3 profile -> $ARTIFACT_DIR/er-quickload-capture.me3"

# --- save source (read+write, per directive) --------------------------------------------
if [[ "$SAVE_DIRECT" == "1" ]]; then
  chmod u+w "$GOLD_SAVE"
  export ER_QUICKLOAD_SAVE_FILE="$GOLD_SAVE"
  echo "save-source: DIRECT gold save (read+write) -> $GOLD_SAVE  *** the game will write/autosave to your real save ***"
else
  STAGED_SAVE_DIR="$ARTIFACT_DIR/save/EldenRing/$ACTIVE_STEAMID"
  mkdir -p "$STAGED_SAVE_DIR"
  STAGED_SAVE="$STAGED_SAVE_DIR/ER0000.sl2"
  cp -f "$GOLD_SAVE" "$STAGED_SAVE"
  chmod u+w "$STAGED_SAVE"   # read+write so the save-update gate passes; autosaves land here
  export ER_QUICKLOAD_SAVE_FILE="$STAGED_SAVE"
  echo "save-source: WRITABLE COPY of gold save (read+write) -> $STAGED_SAVE  (your gold save is only read)"
fi
[[ -n "${ER_QUICKLOAD_GOLD_SLOT:-}" && "${ER_QUICKLOAD_GOLD_SLOT}" != "-1" ]] && export ER_QUICKLOAD_AUTOLOAD_SLOT="$ER_QUICKLOAD_GOLD_SLOT"

# Reach a lit, in-world frame to capture: autoload Continues the gold character.
printf 'continue\n' > "$GAME_DIR/er-quickload-autoload.txt"

cat <<EOF

============================================================================
 RENDERDOC FRAME CAPTURE RUN  (no auto-teardown)
 Booting Elden Ring with RenderDoc's Vulkan capture layer enabled.
 1) Wait for the zero-input autoload to reach a LIT in-world area.
 2) Trigger a capture:
      qrenderdoc --targetcontrol localhost:38920   ->  Queue Capture
      (or press F12 in the game window)
    .rdc lands at:  ${RENDERDOC_CAPFILE}_frameN.rdc
 3) Tear down:  pkill -x eldenring.exe
 4) Extract:  QT_QPA_PLATFORM=offscreen qrenderdoc --python $REPO_ROOT/scripts/extract-capture.py -- \\
                ${RENDERDOC_CAPFILE}_frameN.rdc target/capture/aeg301 --list   # then pick an --event-id
 Artifacts: $ARTIFACT_DIR
============================================================================
EOF

cd "$GAME_DIR"
# exec -> this shell BECOMES the foreground me3 CLI, which owns the compat-tool/wine tree and holds
# the game until you quit. RenderDoc capture is enabled via the implicit Vulkan layer's enable var;
# VKD3D_CONFIG=force_host_cached stabilises capture.
#
# EVERY per-run artifact is redirected into ARTIFACT_DIR. Anything left in GAME_DIR is SINGLE-SLOT:
# the DLL rotates `<name>` to `<name>.prev` on its first write, so run N-2 is already gone and a
# harness that pre-deletes the log drops the surviving `.prev` with it. Measured 2026-08-31: two
# launches destroyed a 5.4 MB continue trace nobody had read. Add a line here for any future log
# rather than copying it out at teardown -- a copy after the run cannot recover a file this run
# clobbered at launch, and a crashed run never reaches the copy at all. `ER_QUICKLOAD_
# AUTOLOAD_DEBUG_PATH` also relocates the portrait-capture-slot*.bin dumps, which the DLL writes
# beside it (er-loading-portrait-core `dump_portrait_rgba`).
exec env \
  ENABLE_VULKAN_RENDERDOC_CAPTURE=1 \
  RENDERDOC_CAPFILE="$RENDERDOC_CAPFILE" \
  VKD3D_CONFIG="${VKD3D_CONFIG:-force_host_cached}" \
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
  ER_QUICKLOAD_CRASH_LOGGING_LOG_PATH="$ARTIFACT_DIR/er-crash-log.txt" \
  ER_QUICKLOAD_CRASH_LOGGING_LATEST_PATH="$ARTIFACT_DIR/er-crash-latest.txt" \
  ER_QUICKLOAD_CRASH_LOGGING_BREADCRUMB_PATH="$ARTIFACT_DIR/er-crash-breadcrumb-latest.txt" \
  ER_QUICKLOAD_CRASH_LOGGING_MODULES_PATH="$ARTIFACT_DIR/er-crash-modules.txt" \
  ER_QUICKLOAD_ARMAMENT_ICONS_PATH="$ARTIFACT_DIR/er-armament-icons.log" \
  ER_QUICKLOAD_SAVE_DISABLE_LOG_PATH="$ARTIFACT_DIR/er-save-disable.log" \
  ER_QUICKLOAD_SAVE_DISABLE_TELEMETRY_PATH="$ARTIFACT_DIR/er-save-disable-telemetry.json" \
  ER_QUICKLOAD_LOADING_PORTRAIT_PATH="$ARTIFACT_DIR/er-loading-portrait.log" \
  ER_QUICKLOAD_LOADING_PORTRAIT_CRASH_LOG_PATH="$ARTIFACT_DIR/er-loading-portrait-crash-log.txt" \
  "$ME3_BIN" --steam-dir "$ME3_STEAM_DIR" launch -g eldenring -p "$ARTIFACT_DIR/er-quickload-capture.me3" > "$ARTIFACT_DIR/me3-launch.out" 2>&1
