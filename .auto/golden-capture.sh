#!/usr/bin/env bash
# GOLDEN-REFERENCE capture (M0), v2 -- GAMEPAD edition.
# Drives the NATIVE menu with a VIRTUAL XBOX CONTROLLER (vgamepad.py via uinput). Gamepad input is
# POLLED by the game via XInput, so it reaches ONLY the game -- no window focus needed, nothing
# leaks to the chat / other windows (fixing the keyboard/focus-steal disaster). DLL is OBSERVE-ONLY
# (no autoload, no input-block). Stops mashing A when GameMan b80 (load committed) fires. Save-safe.
# Paced ~1 tap/sec so you can watch + steer. Pin a save: ER_TEST_SAVE=save-files/139-STR bash ...
set -u

SAVE_NAME="${ER_TEST_SAVE:-save-files/9-Menace}"
REPO=/home/banon/projects/er-effects-rs
GAME="$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game"
SAVE_DIR="$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/76561197986456766"
PROTON="$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton"
BUNDLE="${BUNDLE_DIR:-$REPO/target/golden/$(basename "$SAVE_NAME")}"
TEL="$GAME/er-effects-telemetry.json"     # DLL default telemetry path (env doesn't reach the game)
MAX_SECONDS=120
TAP_EVERY=1                                 # seconds between A taps (paced, watchable)

rm -rf "$BUNDLE"; mkdir -p "$BUNDLE"; rm -f "$TEL"
echo "save_used=$SAVE_NAME" | tee "$BUNDLE/progress.log"

# --- save safety ---
cp -f "$SAVE_DIR/ER0000.sl2" "$BUNDLE/live.sl2.bak"
SHA_BEFORE=$(sha256sum "$SAVE_DIR/ER0000.sl2" | cut -d' ' -f1)
cp -f "$REPO/$SAVE_NAME/ER0000.sl2" "$SAVE_DIR/ER0000.sl2"
echo "sha_before=$SHA_BEFORE" >> "$BUNDLE/progress.log"

FIFO=$(mktemp -u); mkfifo "$FIFO"
cleanup() {
  { echo quit >&3; } 2>/dev/null
  exec 3>&- 2>/dev/null
  rm -f "$FIFO"
  pkill -9 -f vgamepad.py 2>/dev/null
  pkill -9 -f offline-launcher.exe 2>/dev/null
  pkill -9 -f eldenring.exe 2>/dev/null
  cp -f "$BUNDLE/live.sl2.bak" "$SAVE_DIR/ER0000.sl2" 2>/dev/null
  SHA_AFTER=$(sha256sum "$SAVE_DIR/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
  echo "sha_restored=$SHA_AFTER save_safe=$([[ "$SHA_AFTER" == "$SHA_BEFORE" ]] && echo 1 || echo 0)" >> "$BUNDLE/progress.log"
}
trap cleanup EXIT

# --- virtual controller BEFORE launch so SDL enumerates it ---
python3 "$REPO/.auto/vgamepad.py" create-and-listen < "$FIFO" > "$BUNDLE/vgamepad.log" 2>&1 &
exec 3>"$FIFO"      # hold the write end open
echo "vgamepad started" >> "$BUNDLE/progress.log"

# --- observe-only DLL ---
rm -f "$GAME/er-effects-own-stepper.txt" "$GAME/er-effects-direct-build.txt" "$GAME/er-effects-block-input.txt"
printf '1\n' > "$GAME/er-effects-offline.txt"
cp -f "$REPO/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll" "$GAME/dllMods/er_effects_rs.dll"

# --- launch (background) ---
( cd "$GAME" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.local/share/Steam" \
  STEAM_COMPAT_DATA_PATH="$HOME/.local/share/Steam/steamapps/compatdata/1245620" \
  "$PROTON" run ./offline-launcher.exe > "$BUNDLE/proton.out" 2>&1 ) &

jfield() { python3 -c "import json;print(json.load(open('$TEL')).get('$1','na'))" 2>/dev/null || echo na; }
shot() { hyprctl -j clients 2>/dev/null | python3 -c "import json,sys,subprocess
ws=[w for w in json.load(sys.stdin) if w.get('class')=='steam_app_1245620']
if ws:
 x,y=ws[0]['at']; w,h=ws[0]['size']; subprocess.run(['grim','-g',f'{x},{y} {w}x{h}','$1'])" 2>/dev/null; }

# --- paced A-mash, NO focus change, stop on b80 ---
START=$(date +%s); i=0; last_tap=0
while :; do
  now=$(date +%s); el=$((now-START))
  [[ $el -ge $MAX_SECONDS ]] && { echo "TIMEOUT ${el}s b80=$(jfield oracle_load_in_progress_b80) player=$(jfield oracle_player_present)" >> "$BUNDLE/progress.log"; break; }
  if [[ $((now-last_tap)) -ge $TAP_EVERY ]]; then
    echo A >&3; last_tap=$now; i=$((i+1))
    b80=$(jfield oracle_load_in_progress_b80)
    echo "t=${el}s tap#$i A b80=$b80 player=$(jfield oracle_player_present) grounded=$(jfield oracle_grounded)" >> "$BUNDLE/progress.log"
    [[ $((i % 4)) -eq 0 ]] && shot "$BUNDLE/menu-${el}s.png"
    # stop the moment the character is in the playable world (grounded on solid ground), BEFORE
    # over-mashing opens an in-game menu -- so the golden shot is the gameplay HUD.
    if [[ "$(jfield oracle_grounded)" == "True" ]] && [[ "$(jfield oracle_player_present)" == "True" ]]; then
      echo "IN-WORLD at ${el}s (grounded) -- stop mashing, settle, capture" >> "$BUNDLE/progress.log"; break
    fi
  fi
  sleep 0.25
done

# --- assemble the GOLDEN proof bundle ---
sleep 1   # let the spawn settle / menu close
shot "$BUNDLE/shot.png"
cp -f "$TEL" "$BUNDLE/state.json" 2>/dev/null
SHA_NOW=$(sha256sum "$SAVE_DIR/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
SAVE_SHA=$(sha256sum "$REPO/$SAVE_NAME/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
python3 - "$BUNDLE" "$SAVE_NAME" "$SAVE_SHA" "$SHA_BEFORE" "$SHA_NOW" "$i" "$TEL" <<'PY'
import json, sys
bundle, save_name, save_sha, sha_before, sha_now, taps, tel = sys.argv[1:8]
try:
    t = json.load(open(tel))
except Exception:
    t = {}
json.dump({"mode": "golden", "input_method": "virtual_xbox_gamepad",
           "dll_simulated_button_presses_total": t.get("simulated_button_presses_total"),
           "gamepad_taps_sent": int(taps)}, open(f"{bundle}/input.json", "w"), indent=2)
json.dump({"save_used": save_name, "save_sha256": save_sha,
           "live_sha_before": sha_before, "live_sha_after": sha_now,
           "save_safe": sha_before == sha_now}, open(f"{bundle}/save.json", "w"), indent=2)
open(f"{bundle}/cmd.txt", "w").write(f"ER_TEST_SAVE={save_name} bash .auto/golden-capture.sh\n")
print("bundle assembled")
PY
echo "bundle assembled" >> "$BUNDLE/progress.log"
