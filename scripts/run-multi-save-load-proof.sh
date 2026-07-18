#!/usr/bin/env bash
# Autonomous repeatable-multi-save-load PROOF runner (docs/goals/repeatable-multi-save-load-acceptance.md).
#
# Composes the proven pieces into ONE push-button run:
#   1. resolves GAME_DIR (the Linux dir the game's er-effects.toml + control files live in);
#   2. stages the boot er-effects.toml (save_file+slot) so the initial auto-load is a chosen character
#      loaded read-only via the save_redirect DirectFile in-memory redirect;
#   3. arms the System->Quit switch autopilot + the switch-count control file for N back-to-back
#      genuine cross-character reloads (FIX: the arm-while-player-present discriminator lets these
#      proceed; only the spurious boot self-reload is disarmed);
#   4. launches ER through the approved offline probe (run-product-continue-direct-probe.sh), which
#      handles Steam preflight + me3 + teardown;
#   5. runs multi-load-proof-monitor.py against the live artifact dir to RAM-verify every load
#      (identity+stats+gear+controllable), log timings, detect crash/stall, and emit the report.
#
# This is a HARNESS/ground-truth runner (uses the simulated-input autopilot, explicitly allowed by the
# refined goal). Cross-FILE coverage is added once the within-file switch path is proven and the
# programmatic (file,slot) channel lands. Exit 0 == every expected load verified, zero crashes.
#
# REQUIRES: Steam running (interactive login) and a correct GAME_DIR. Both are machine-specific; set
#   GAME_DIR=<linux path to '.../ELDEN RING/Game'>  (the dir whose eldenring.exe the game runs).
set -uo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CORPUS_ROOT="${ER_SAVE_CORPUS_ROOT:-/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files}"
BOOT_FILE="${BOOT_FILE:-$CORPUS_ROOT/139-Taunts/ER0000.sl2}"
BOOT_SLOT="${BOOT_SLOT:-4}"                    # boot a slot NOT in TARGET_SLOTS so every switch is cross-char
# Explicit within-file DISTINCT-character switch sequence (139-Taunts: s0 Bonky Bean, s1 Nephilim,
# s2 Bean Smith, s4 Speed Bean). Drives the goal's ">=3 per-character within-file" axis in one launch.
TARGET_SLOTS="${TARGET_SLOTS:-0,1,2}"
SWITCHES="$(python3 -c "import sys;print(len([x for x in '$TARGET_SLOTS'.replace(',',' ').split()]))")"
ARTIFACT_DIR="${ARTIFACT_DIR:-$REPO_ROOT/target/runtime-probe/multi-save-load-$(date +%Y%m%d-%H%M%S)}"
BUILT_DLL="$REPO_ROOT/target/x86_64-pc-windows-msvc/release/er_effects_rs.dll"

fail() { echo "run-multi-save-load-proof: $*" >&2; exit 2; }

# --- GAME_DIR resolution (current-user-aware; ask via env, never hard-code /home/<user>) ---
if [[ -z "${GAME_DIR:-}" ]]; then
  for c in \
      "$HOME/.local/share/Steam/steamapps/common/ELDEN RING/Game" \
      "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game" \
      "$HOME/.steam/steam/steamapps/common/ELDEN RING/Game"; do
    [[ -f "$c/eldenring.exe" ]] && { GAME_DIR="$c"; break; }
  done
fi
[[ -n "${GAME_DIR:-}" && -f "$GAME_DIR/eldenring.exe" ]] || fail \
  "GAME_DIR not resolved. Set GAME_DIR=<linux path to the '.../ELDEN RING/Game' dir containing eldenring.exe>. \
On this box the game reports its win path as C:\\SteamLibrary\\steamapps\\common\\ELDEN RING\\Game -- its Linux/Proton mapping is machine-specific."

# shellcheck source=scripts/steam-running.sh
# shellcheck disable=SC1091
source "$REPO_ROOT/scripts/steam-running.sh"  # WSL-aware: Windows steam.exe via tasklist.exe, not pgrep
steam_running || fail "Steam is not running. Start Steam (interactive login) first; the offline eldenring.exe launch reuses its environment."
[[ -f "$BUILT_DLL" ]] || fail "DLL not built: $BUILT_DLL (run: cargo xwin build --release --target x86_64-pc-windows-msvc)"
[[ -f "$BOOT_FILE" ]] || fail "boot save not found: $BOOT_FILE"

mkdir -p "$ARTIFACT_DIR"
echo "== multi-save-load proof =="
echo "GAME_DIR=$GAME_DIR"
echo "boot: $BOOT_FILE slot=$BOOT_SLOT ; switches=$SWITCHES ; artifacts=$ARTIFACT_DIR"

# This box: WSL2 + Windows-native Steam. The game is loaded by the WINDOWS me3.exe with a .me3
# profile; the Linux run-product-continue-direct-probe.sh (--steam-dir) does not apply here. The DLL
# reads er-effects.toml + control files from game_directory_path() (= GAME_DIR) and writes telemetry/
# debug log there, so we stage into GAME_DIR and monitor GAME_DIR.
ME3="${ME3:-/mnt/c/Users/$USER/AppData/Local/garyttierney/me3/bin/me3.exe}"
[[ -f "$ME3" ]] || fail "Windows me3.exe not found at $ME3 (set ME3=<path to me3.exe>)"
DLL_GAMEDIR="$GAME_DIR/er_effects_rs.dll"

win_path() { python3 -c "import sys;p=sys.argv[1];print((p[5].upper()+':\\\\'+p[7:].replace('/','\\\\')) if p.startswith('/mnt/') and len(p)>6 and p[6]=='/' else p)" "$1"; }

# --- 1. stage the FIX build DLL to a Windows-native path + a .me3 profile ---
cp -f "$BUILT_DLL" "$DLL_GAMEDIR"
PROFILE="$ARTIFACT_DIR/multi-save-load.me3"
{ echo 'profileVersion = "v1"'; echo; echo '[[supports]]'; echo 'game = "eldenring"'; echo; echo '[[natives]]'; echo "path = '$(win_path "$DLL_GAMEDIR")'"; } > "$PROFILE"
echo "staged DLL -> $DLL_GAMEDIR ; profile -> $PROFILE"

# --- 2. boot TOML (in-memory read-only redirect) in GAME_DIR (back up the existing one) ---
[[ -f "$GAME_DIR/er-effects.toml" ]] && cp -f "$GAME_DIR/er-effects.toml" "$ARTIFACT_DIR/er-effects.toml.bak"
{ echo "# staged by run-multi-save-load-proof.sh for the initial auto-load"; echo "save_file = '$(win_path "$BOOT_FILE")'"; echo "slot = $BOOT_SLOT"; } > "$GAME_DIR/er-effects.toml"
echo "staged boot TOML (save_file=$(win_path "$BOOT_FILE") slot=$BOOT_SLOT)"

# --- 3. autopilot + switch-count + target-slots control files in GAME_DIR ---
# PROGRAMMATIC mode (default): the monitor drives each reload by writing the next slot to the DLL
# control file er-effects-switch-slot.txt (the DLL polls it in-world and arms a menu-free switch --
# zero simulated input). Start from a clean slate so no stale request fires. The legacy simulated-input
# autopilot markers are only armed when DRIVE_MODE=autopilot.
SWITCH_SLOT_FILE="$GAME_DIR/er-effects-switch-slot.txt"
SWITCH_FILE_OVERRIDE="$GAME_DIR/er-effects-switch-save-file.txt"  # cross-file: target save FILE per switch
rm -f "$SWITCH_SLOT_FILE" "$SWITCH_FILE_OVERRIDE" 2>/dev/null
if [[ "${DRIVE_MODE:-programmatic}" == "autopilot" ]]; then
  printf '1\n' > "$GAME_DIR/er-effects-system-quit-repro.txt"
  printf '1\n' > "$GAME_DIR/er-effects-system-quit-load-switch.txt"
  printf '%s\n' "$SWITCHES" > "$GAME_DIR/er-effects-sq-target-switches.txt"
  printf '%s\n' "$TARGET_SLOTS" > "$GAME_DIR/er-effects-sq-target-slots.txt"
  echo "armed AUTOPILOT markers (switches=$SWITCHES; target-slots=[$TARGET_SLOTS])"
else
  echo "PROGRAMMATIC drive: monitor will write reload slots to $SWITCH_SLOT_FILE (target-slots=[$TARGET_SLOTS])"
fi
# shellcheck disable=SC2317  # body runs via `trap cleanup EXIT`, not inline
cleanup() {
  taskkill.exe /F /IM eldenring.exe >/dev/null 2>&1
  taskkill.exe /F /IM me3.exe >/dev/null 2>&1
  rm -f "$GAME_DIR/er-effects-system-quit-repro.txt" "$GAME_DIR/er-effects-system-quit-load-switch.txt" \
        "$GAME_DIR/er-effects-sq-target-switches.txt" "$GAME_DIR/er-effects-sq-target-slots.txt" \
        "$SWITCH_SLOT_FILE" "$SWITCH_FILE_OVERRIDE" 2>/dev/null
  [[ -f "$ARTIFACT_DIR/er-effects.toml.bak" ]] && cp -f "$ARTIFACT_DIR/er-effects.toml.bak" "$GAME_DIR/er-effects.toml"
}
trap cleanup EXIT

# --- 4. targets.json for the monitor: boot char first, then the reload sequence. Within-file uses
#        TARGET_SLOTS on the boot file; CROSS-FILE uses SWITCH_TARGETS="file1:slot1,file2:slot2,..." ---
TARGETS_JSON="$ARTIFACT_DIR/targets.json"
python3 - "$BOOT_FILE" "$BOOT_SLOT" "$TARGET_SLOTS" "${SWITCH_TARGETS:-}" "$TARGETS_JSON" <<'PY'
import json, sys
boot_file, boot_slot, target_slots, switch_targets, out = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4], sys.argv[5]
targets=[{"file": boot_file, "slot": boot_slot}]              # index 0 = initial TOML auto-load
if switch_targets.strip():
    # CROSS-FILE: "file1:slot1,file2:slot2" -- each reload names its own save file + slot.
    for pair in switch_targets.split(","):
        f, s = pair.rsplit(":", 1)
        targets.append({"file": f.strip(), "slot": int(s)})
else:
    slots=[int(x) for x in target_slots.replace(',',' ').split()]
    targets += [{"file": boot_file, "slot": s} for s in slots]  # within-file cross-character reloads
json.dump(targets, open(out,"w"), indent=1)
print(f"wrote {out}: {len(targets)} targets")
PY

# --- 5. read-only invariant (acceptance SS5): snapshot every source save's mtime+size BEFORE the run,
#        so we can ASSERT after that no source file was written (loads are read-only; the only write
#        path is the in-game Save button, which upserts to APPDATA, never the supplied file). ---
SRC_SNAP="$ARTIFACT_DIR/source-mtimes-before.json"
python3 -c "
import json, os
targets=json.load(open('$TARGETS_JSON'))
snap={}
for t in targets:
    p=t['file']
    try: st=os.stat(p); snap[p]=[st.st_mtime, st.st_size]
    except OSError: pass
json.dump(snap, open('$SRC_SNAP','w'))
print(f'read-only baseline: {len(snap)} source saves snapshotted')
"

# --- 6. record debug-log start offset (shared append-log), launch via Windows me3.exe, monitor GAME_DIR ---
OFFSET="$(stat -c%s "$GAME_DIR/er-effects-autoload-debug.log" 2>/dev/null || echo 0)"
echo "launching ER via Windows me3.exe (offline) ..."
"$ME3" launch -g eldenring --online false -p "$(wslpath -w "$PROFILE")" > "$ARTIFACT_DIR/me3-launch.log" 2>&1 &
LAUNCH_PID=$!

echo "monitoring loads (RAM-oracle verify + report) ..."
DRIVE_ARGS=()
[[ "${DRIVE_MODE:-programmatic}" != "autopilot" ]] && DRIVE_ARGS=(--drive-slot-file "$SWITCH_SLOT_FILE" --drive-file-override "$SWITCH_FILE_OVERRIDE")
python3 "$REPO_ROOT/scripts/multi-load-proof-monitor.py" \
  --artifact-dir "$GAME_DIR" \
  --targets "$TARGETS_JSON" \
  --report "$ARTIFACT_DIR/proof-report.md" \
  --debug-log-offset "$OFFSET" \
  "${DRIVE_ARGS[@]}" \
  --per-load-deadline "${PER_LOAD_DEADLINE:-120}" \
  --overall-deadline "${OVERALL_DEADLINE:-300}"
RC=$?

# capture my-run artifacts before teardown clears markers
cp -f "$GAME_DIR/er-effects-telemetry.json" "$ARTIFACT_DIR/er-effects-telemetry.json" 2>/dev/null
python3 -c "
off=$OFFSET
with open('$GAME_DIR/er-effects-autoload-debug.log','rb') as f: f.seek(off); d=f.read()
open('$ARTIFACT_DIR/my-run-debug.log','wb').write(d)
" 2>/dev/null
# --- read-only invariant assertion (acceptance SS5): every source save's mtime+size must be unchanged ---
python3 -c "
import json, os
before=json.load(open('$SRC_SNAP'))
changed=[]
for p,(mt,sz) in before.items():
    try:
        st=os.stat(p)
        if st.st_mtime!=mt or st.st_size!=sz: changed.append(p)
    except OSError: changed.append(p+' (vanished)')
rep=open('$ARTIFACT_DIR/proof-report.md','a')
if changed:
    rep.write('\n> READ-ONLY INVARIANT VIOLATED -- source save(s) written during the run: '+', '.join(changed)+'\n')
    print('READ-ONLY VIOLATION:', changed)
else:
    rep.write(f'\n> Read-only invariant OK: all {len(before)} source saves unchanged (mtime+size) across the run.\n')
    print(f'read-only invariant OK: {len(before)} source saves unchanged')
"
echo "monitor exit=$RC ; report -> $ARTIFACT_DIR/proof-report.md"
wait "$LAUNCH_PID" 2>/dev/null
exit $RC
