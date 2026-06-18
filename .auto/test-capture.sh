#!/usr/bin/env bash
# ZERO-INPUT TEST capture (M1/M2). The DELIVERABLE path: the DLL drives the autoload with ZERO
# simulated input (own_stepper + direct-build, input-block ON), NO gamepad, NO keyboard. Produces
# the same proof bundle as the golden and asserts simulated_button_presses_total == 0. The bundle
# must MATCH that save's golden (same map/level/grounded) for the run to PASS. Save-safe.
#
# Pin the save (match the golden): ER_TEST_SAVE=save-files/9-Menace bash .auto/test-capture.sh
set -u

SAVE_NAME="${ER_TEST_SAVE:-save-files/9-Menace}"
REPO=/home/banon/projects/er-effects-rs
GAME="$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game"
SAVE_DIR="$HOME/.local/share/Steam/steamapps/compatdata/1245620/pfx/drive_c/users/steamuser/AppData/Roaming/EldenRing/76561197986456766"
PROTON="$HOME/.local/share/Steam/steamapps/common/Proton - Experimental/proton"
BUNDLE="${BUNDLE_DIR:-$REPO/target/test/$(basename "$SAVE_NAME")}"
TEL="$GAME/er-effects-telemetry.json"
MAX_SECONDS=90

rm -rf "$BUNDLE"; mkdir -p "$BUNDLE"; rm -f "$TEL"
echo "save_used=$SAVE_NAME (ZERO-INPUT test)" | tee "$BUNDLE/progress.log"

# --- save safety ---
cp -f "$SAVE_DIR/ER0000.sl2" "$BUNDLE/live.sl2.bak"
SHA_BEFORE=$(sha256sum "$SAVE_DIR/ER0000.sl2" | cut -d' ' -f1)
cp -f "$REPO/$SAVE_NAME/ER0000.sl2" "$SAVE_DIR/ER0000.sl2"
echo "sha_before=$SHA_BEFORE" >> "$BUNDLE/progress.log"

cleanup() {
  pkill -9 -f offline-launcher.exe 2>/dev/null
  pkill -9 -f eldenring.exe 2>/dev/null
  cp -f "$BUNDLE/live.sl2.bak" "$SAVE_DIR/ER0000.sl2" 2>/dev/null
  SHA_AFTER=$(sha256sum "$SAVE_DIR/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
  echo "sha_restored=$SHA_AFTER save_safe=$([[ "$SHA_AFTER" == "$SHA_BEFORE" ]] && echo 1 || echo 0)" >> "$BUNDLE/progress.log"
  SAVE_SRC_SHA=$(sha256sum "$REPO/$SAVE_NAME/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
  python3 - "$BUNDLE/save.json" "$SAVE_NAME" "$SAVE_SRC_SHA" "$SHA_BEFORE" "$SHA_AFTER" <<'PY'
import json, sys
path, save_name, save_sha, before, after = sys.argv[1:6]
json.dump({"save_used": save_name, "save_sha256": save_sha,
           "live_sha_before": before, "live_sha_after_restore": after,
           "save_safe": before == after}, open(path, "w"), indent=2)
PY
}
trap cleanup EXIT

# --- DLL drives the autoload, ZERO input: own_stepper(slot 0) + direct-build + offline + input-block ---
printf 'slot=0\n' > "$GAME/er-effects-own-stepper.txt"
printf '1\n' > "$GAME/er-effects-direct-build.txt"
printf '1\n' > "$GAME/er-effects-offline.txt"
printf '1\n' > "$GAME/er-effects-block-input.txt"
cp -f "$REPO/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll" "$GAME/dllMods/er_effects_rs.dll"

( cd "$GAME" && STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.local/share/Steam" \
  STEAM_COMPAT_DATA_PATH="$HOME/.local/share/Steam/steamapps/compatdata/1245620" \
  "$PROTON" run ./offline-launcher.exe > "$BUNDLE/proton.out" 2>&1 ) &

jfield() { python3 -c "import json;print(json.load(open('$TEL')).get('$1','na'))" 2>/dev/null || echo na; }
shot() { hyprctl -j clients 2>/dev/null | python3 -c "import json,sys,subprocess
ws=[w for w in json.load(sys.stdin) if w.get('class')=='steam_app_1245620']
if ws:
 x,y=ws[0]['at']; w,h=ws[0]['size']; subprocess.run(['grim','-g',f'{x},{y} {w}x{h}','$1'])" 2>/dev/null; }

# --- wait (zero-input) for the autoload to land the player in-world ---
START=$(date +%s)
while :; do
  el=$(( $(date +%s) - START ))
  [[ $el -ge $MAX_SECONDS ]] && { echo "TIMEOUT ${el}s player=$(jfield oracle_player_present) grounded=$(jfield oracle_grounded) presses=$(jfield simulated_button_presses_total)" >> "$BUNDLE/progress.log"; break; }
  if [[ "$(jfield oracle_grounded)" == "True" ]] && [[ "$(jfield oracle_player_present)" == "True" ]]; then
    echo "IN-WORLD at ${el}s presses=$(jfield simulated_button_presses_total)" >> "$BUNDLE/progress.log"; break
  fi
  echo "t=${el}s player=$(jfield oracle_player_present) grounded=$(jfield oracle_grounded) b80=$(jfield oracle_load_in_progress_b80) presses=$(jfield simulated_button_presses_total)" >> "$BUNDLE/progress.log"
  sleep 1
done

# --- assemble bundle + assert zero-input ---
sleep 1
shot "$BUNDLE/shot.png"
cp -f "$TEL" "$BUNDLE/state.json" 2>/dev/null
SHA_NOW=$(sha256sum "$SAVE_DIR/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
SAVE_SHA=$(sha256sum "$REPO/$SAVE_NAME/ER0000.sl2" 2>/dev/null | cut -d' ' -f1)
python3 - "$BUNDLE" "$SAVE_NAME" "$SAVE_SHA" "$SHA_BEFORE" "$SHA_NOW" "$TEL" <<'PY'
import json, sys
bundle, save_name, save_sha, sha_before, sha_now, tel = sys.argv[1:7]
try:
    t = json.load(open(tel))
except Exception:
    t = {}
presses = t.get("simulated_button_presses_total")
json.dump({"mode": "test", "input_method": "none_zero_input",
           "simulated_button_presses_total": presses,
           "zero_input_ok": presses == 0}, open(f"{bundle}/input.json", "w"), indent=2)
# save.json is written by cleanup() AFTER the restore (post-restore hash = true save-safety).
open(f"{bundle}/cmd.txt", "w").write(f"ER_TEST_SAVE={save_name} bash .auto/test-capture.sh\n")
print("bundle assembled; zero_input_ok =", presses == 0)
PY
echo "bundle assembled" >> "$BUNDLE/progress.log"