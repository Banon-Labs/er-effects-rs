#!/usr/bin/env bash
# GOLDEN-REFERENCE capture (M0). Drives the NATIVE menu with REAL mashed input (ydotool) to load a
# save, stops when GameMan b80 (load-in-progress) fires, then captures the in-world oracle bundle.
# DLL is OBSERVE-ONLY (no autoload drive, no input-block) so the mashed input actually reaches the
# game. Save-safe: backs up + restores the live save. Input here is FINE -- this is the reference
# baseline, NOT the zero-input deliverable.
#
# Pin a save with: ER_TEST_SAVE=save-files/139-STR bash .auto/golden-capture.sh
set -u

SAVE_NAME="${ER_TEST_SAVE:-save-files/9-Menace}"
REPO=/home/banon/projects/er-effects-rs
GAME="$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game"
SAVE_DIR="$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/76561197986456766"
PROTON="$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton"
BUNDLE="${BUNDLE_DIR:-$REPO/target/golden/$(basename "$SAVE_NAME")}"
# DLL writes telemetry to its cwd-relative default (env vars don't reliably reach the game process).
TEL="$GAME/er-effects-telemetry.json"
export YDOTOOL_SOCKET=/run/user/1000/.ydotool_socket
KEY_ENTER=28   # KEY_ENTER
KEY_E=18       # KEY_E (ER keyboard "Confirm" default)
MAX_SECONDS=90

rm -rf "$BUNDLE"; mkdir -p "$BUNDLE"; rm -f "$TEL"
echo "save_used=$SAVE_NAME" | tee "$BUNDLE/progress.log"

# --- save safety: backup + record hashes ---
cp -f "$SAVE_DIR/ER0000.sl2" "$BUNDLE/live.sl2.bak"
SHA_BEFORE=$(sha256sum "$SAVE_DIR/ER0000.sl2" | cut -d' ' -f1)
cp -f "$REPO/$SAVE_NAME/ER0000.sl2" "$SAVE_DIR/ER0000.sl2"
SAVE_SHA=$(sha256sum "$REPO/$SAVE_NAME/ER0000.sl2" | cut -d' ' -f1)
echo "sha_before=$SHA_BEFORE save_sha=$SAVE_SHA" >> "$BUNDLE/progress.log"

restore() {
  cp -f "$BUNDLE/live.sl2.bak" "$SAVE_DIR/ER0000.sl2"
  SHA_AFTER=$(sha256sum "$SAVE_DIR/ER0000.sl2" | cut -d' ' -f1)
  echo "sha_restored=$SHA_AFTER save_safe=$([[ "$SHA_AFTER" == "$SHA_BEFORE" ]] && echo 1 || echo 0)" >> "$BUNDLE/progress.log"
  pkill -f offline-launcher.exe 2>/dev/null
  pkill -f eldenring.exe 2>/dev/null
}
trap restore EXIT

# --- observe-only DLL: no autoload, no input-block ---
rm -f "$GAME/er-effects-own-stepper.txt" "$GAME/er-effects-direct-build.txt" "$GAME/er-effects-block-input.txt"
printf '1\n' > "$GAME/er-effects-offline.txt"
cp -f "$REPO/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll" "$GAME/dllMods/er_effects_rs.dll"

# --- launch (background) ---
( cd "$GAME" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.local/share/Steam" \
  STEAM_COMPAT_DATA_PATH="$HOME/.local/share/Steam/steamapps/compatdata/1245620" \
  "$PROTON" run ./offline-launcher.exe > "$BUNDLE/proton.out" 2>&1 ) &

b80_field() { python3 -c "import json,sys;print(json.load(open('$TEL')).get('oracle_load_in_progress_b80','na'))" 2>/dev/null || echo na; }
player_field() { python3 -c "import json,sys;print(json.load(open('$TEL')).get('oracle_player_present','na'))" 2>/dev/null || echo na; }

# focus the ER window so ydotool input lands on it
focus_er() { hyprctl -j clients 2>/dev/null | python3 -c "import json,sys,subprocess;
ws=[w for w in json.load(sys.stdin) if w.get('class')=='steam_app_1245620']
if ws: subprocess.run(['hyprctl','dispatch','focuswindow','address:'+ws[0]['address']])" 2>/dev/null; }

mash() { ydotool key ${KEY_ENTER}:1 ${KEY_ENTER}:0 ${KEY_E}:1 ${KEY_E}:0 2>/dev/null; }

shot() { hyprctl -j clients 2>/dev/null | python3 -c "import json,sys,subprocess;
ws=[w for w in json.load(sys.stdin) if w.get('class')=='steam_app_1245620']
if ws:
 x,y=ws[0]['at']; w,h=ws[0]['size']
 subprocess.run(['grim','-g',f'{x},{y} {w}x{h}','$1'])" 2>/dev/null; }

# --- mash loop: drive the native menu, stop when b80 fires ---
START=$(date +%s); i=0
while :; do
  now=$(date +%s); el=$((now-START))
  [[ $el -ge $MAX_SECONDS ]] && { echo "TIMEOUT at ${el}s b80=$(b80_field) player=$(player_field)" >> "$BUNDLE/progress.log"; break; }
  focus_er; mash
  b80=$(b80_field)
  if [[ $((i % 10)) -eq 0 ]]; then
    echo "t=${el}s i=$i b80=$b80 player=$(player_field)" >> "$BUNDLE/progress.log"
    shot "$BUNDLE/menu-$el.png"
  fi
  # stop mashing once the load commits (b80 nonzero), BEFORE the player is controllable
  if [[ "$b80" =~ ^[0-9]+$ ]] && [[ "$b80" -ne 0 ]]; then
    echo "LOAD COMMITTED at ${el}s (b80=$b80) -- stop mashing, let it load" >> "$BUNDLE/progress.log"
    break
  fi
  i=$((i+1))
done

echo "post-mash: waiting for in-world, capturing bundle progression" >> "$BUNDLE/progress.log"
