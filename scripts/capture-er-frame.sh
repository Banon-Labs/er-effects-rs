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
#     1) ER_EFFECTS_GOLD_SAVE=/abs/ER0000.sl2 ./scripts/capture-er-frame.sh
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
#   ER_EFFECTS_CAPTURE_SAVE_DIRECT=1 -> point the game at the gold save ITSELF, read+write
#               (chmod u+w on the original; the game WILL write/autosave to your real save).
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
GAME_DIR="${GAME_DIR:-$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game}"
PROTON="${PROTON:-$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton}"
STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$HOME/.local/share/Steam/steamapps/compatdata/1245620}"
STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.local/share/Steam}"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/capture-$(date +%Y%m%d-%H%M%S)}"
BUILT_DLL="${BUILT_DLL:-$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll}"
GOLD_SAVE="${ER_EFFECTS_GOLD_SAVE:-}"
SAVE_DIRECT="${ER_EFFECTS_CAPTURE_SAVE_DIRECT:-0}"
GOLD_SAVE_MIN_BYTES="${GOLD_SAVE_MIN_BYTES:-1048576}"
ACTIVE_STEAMID="${ER_EFFECTS_ACTIVE_STEAMID:-76561197986456766}"
APPDATA_ER_ROOT="${APPDATA_ER_ROOT:-$STEAM_COMPAT_DATA_PATH/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing}"
# RenderDoc capture file template; the layer appends _frameN.rdc.
RENDERDOC_CAPFILE="${RENDERDOC_CAPFILE:-$ARTIFACT_DIR/er_cap}"

fatal() { echo "capture-er-frame: $*" >&2; exit 2; }
require_file() { [[ -f "$1" ]] || fatal "missing file: $1"; }
require_exec() { [[ -x "$1" ]] || fatal "missing executable: $1"; }

preflight() {
  pgrep -x steam >/dev/null 2>&1 || fatal "Steam is not running; start Steam first (the offline launch reuses Steam's environment)"
  require_exec "$PROTON"
  require_file "$GAME_DIR/eldenring.exe"
  require_file "$BUILT_DLL" \
    || fatal "built DLL not found: $BUILT_DLL (run: cargo xwin build --release --target x86_64-pc-windows-msvc)"
  command -v qrenderdoc >/dev/null 2>&1 || fatal "qrenderdoc not in PATH (install the renderdoc package)"
  [[ -f /etc/vulkan/implicit_layer.d/renderdoc_capture.json ]] \
    || fatal "RenderDoc Vulkan implicit layer JSON not found; is renderdoc installed?"
  [[ -d "$STEAM_COMPAT_DATA_PATH" ]] || fatal "missing compatdata path: $STEAM_COMPAT_DATA_PATH"
  [[ -n "$GOLD_SAVE" ]] || fatal "ER_EFFECTS_GOLD_SAVE is unset; supply the absolute path to your gold ER0000.sl2"
  [[ -f "$GOLD_SAVE" ]] || fatal "gold save not found: $GOLD_SAVE"
  local bytes; bytes=$(stat -c '%s' "$GOLD_SAVE" 2>/dev/null || echo 0)
  (( bytes >= GOLD_SAVE_MIN_BYTES )) || fatal "gold save too small ($bytes bytes): $GOLD_SAVE"
  if pgrep -x eldenring.exe >/dev/null 2>&1; then
    fatal "eldenring.exe is already running; refusing to mix ownership (another agent may be using it)"
  fi
}

clean_stale_mod_dlls() { [[ -d "$GAME_DIR/dllMods" ]] && rm -f "$GAME_DIR/dllMods/"*.dll 2>/dev/null || true; }

preflight
mkdir -p "$ARTIFACT_DIR"
clean_stale_mod_dlls
cp -f "$BUILT_DLL" "$GAME_DIR/er_effects_rs.dll"
echo "deploy: fresh chainload DLL -> $GAME_DIR/er_effects_rs.dll"

# --- save source (read+write, per directive) --------------------------------------------
if [[ "$SAVE_DIRECT" == "1" ]]; then
  chmod u+w "$GOLD_SAVE"
  export ER_EFFECTS_SAVE_FILE="$GOLD_SAVE"
  echo "save-source: DIRECT gold save (read+write) -> $GOLD_SAVE  *** the game will write/autosave to your real save ***"
else
  STAGED_SAVE_DIR="$ARTIFACT_DIR/save/EldenRing/$ACTIVE_STEAMID"
  mkdir -p "$STAGED_SAVE_DIR"
  STAGED_SAVE="$STAGED_SAVE_DIR/ER0000.sl2"
  cp -f "$GOLD_SAVE" "$STAGED_SAVE"
  chmod u+w "$STAGED_SAVE"   # read+write so the save-update gate passes; autosaves land here
  export ER_EFFECTS_SAVE_FILE="$STAGED_SAVE"
  echo "save-source: WRITABLE COPY of gold save (read+write) -> $STAGED_SAVE  (your gold save is only read)"
fi
[[ -n "${ER_EFFECTS_GOLD_SLOT:-}" && "${ER_EFFECTS_GOLD_SLOT}" != "-1" ]] && export ER_EFFECTS_AUTOLOAD_SLOT="$ER_EFFECTS_GOLD_SLOT"

# Reach a lit, in-world frame to capture: autoload Continues the gold character.
printf 'continue\n' > "$GAME_DIR/er-effects-autoload.txt"

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
# exec -> this shell BECOMES the foreground Proton process and holds the game until you quit
# (Proton `run` tears the wine tree down if its parent dies). RenderDoc capture is enabled via
# the implicit Vulkan layer's enable var; VKD3D_CONFIG=force_host_cached stabilises capture.
exec env \
  STEAM_COMPAT_CLIENT_INSTALL_PATH="$STEAM_COMPAT_CLIENT_INSTALL_PATH" \
  STEAM_COMPAT_DATA_PATH="$STEAM_COMPAT_DATA_PATH" \
  ENABLE_VULKAN_RENDERDOC_CAPTURE=1 \
  RENDERDOC_CAPFILE="$RENDERDOC_CAPFILE" \
  VKD3D_CONFIG="${VKD3D_CONFIG:-force_host_cached}" \
  ER_EFFECTS_TELEMETRY_PATH="$ARTIFACT_DIR/er-effects-telemetry.json" \
  ER_EFFECTS_AUTOLOAD_DEBUG_PATH="$ARTIFACT_DIR/er-effects-autoload-debug.log" \
  "$PROTON" run "$GAME_DIR/eldenring.exe" > "$ARTIFACT_DIR/proton-run.out" 2>&1
