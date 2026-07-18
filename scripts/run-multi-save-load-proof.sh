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

pgrep -x steam >/dev/null 2>&1 || fail "Steam is not running. Start Steam (interactive login) first; the offline eldenring.exe launch reuses its environment."
[[ -f "$BUILT_DLL" ]] || fail "DLL not built: $BUILT_DLL (run: cargo xwin build --release --target x86_64-pc-windows-msvc)"
[[ -f "$BOOT_FILE" ]] || fail "boot save not found: $BOOT_FILE"

mkdir -p "$ARTIFACT_DIR"
echo "== multi-save-load proof =="
echo "GAME_DIR=$GAME_DIR"
echo "boot: $BOOT_FILE slot=$BOOT_SLOT ; switches=$SWITCHES ; artifacts=$ARTIFACT_DIR"

# --- 1. boot TOML (in-memory read-only redirect of the source save) ---
BOOT_FILE_WIN="$(python3 - "$BOOT_FILE" <<'PY'
import sys
# /mnt/<d>/rest -> <D>:\rest  (the game sees the corpus mount as a Windows drive letter)
p=sys.argv[1]
if p.startswith('/mnt/') and len(p)>6 and p[6]=='/':
    print(p[5].upper()+':\\'+p[7:].replace('/','\\'))
else:
    print(p)
PY
)"
TOML_SRC="$ARTIFACT_DIR/er-effects.toml"
cat > "$TOML_SRC" <<EOF
# staged by run-multi-save-load-proof.sh for the initial auto-load
save_file = '$BOOT_FILE_WIN'
slot = $BOOT_SLOT
EOF
echo "staged boot TOML -> $TOML_SRC (save_file=$BOOT_FILE_WIN slot=$BOOT_SLOT)"

# --- 2. autopilot + switch-count + target-slots control files in GAME_DIR (game_directory_path()) ---
printf '1\n' > "$GAME_DIR/er-effects-system-quit-load-switch.txt"
printf '%s\n' "$SWITCHES" > "$GAME_DIR/er-effects-sq-target-switches.txt"
printf '%s\n' "$TARGET_SLOTS" > "$GAME_DIR/er-effects-sq-target-slots.txt"
echo "armed autopilot markers in GAME_DIR (load-switch; switches=$SWITCHES; target-slots=[$TARGET_SLOTS])"
cleanup_markers() { rm -f "$GAME_DIR/er-effects-system-quit-load-switch.txt" "$GAME_DIR/er-effects-sq-target-switches.txt" "$GAME_DIR/er-effects-sq-target-slots.txt" 2>/dev/null; }
trap cleanup_markers EXIT

# --- 3. targets.json for the monitor (boot char first, then the DISTINCT within-file switch sequence) ---
TARGETS_JSON="$ARTIFACT_DIR/targets.json"
python3 - "$BOOT_FILE" "$BOOT_SLOT" "$TARGET_SLOTS" "$TARGETS_JSON" <<'PY'
import json, sys
boot_file, boot_slot, target_slots, out = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
slots=[int(x) for x in target_slots.replace(',',' ').split()]
targets=[{"file": boot_file, "slot": boot_slot}]              # index 0 = initial TOML auto-load
targets += [{"file": boot_file, "slot": s} for s in slots]    # the cross-character reloads
json.dump(targets, open(out,"w"), indent=1)
print(f"wrote {out}: {len(targets)} targets (boot slot {boot_slot} -> switches {slots})")
PY

# --- 4. launch (approved probe owns Steam preflight + me3 + teardown); autopilot enabled ---
echo "launching ER via run-product-continue-direct-probe.sh (background) ..."
GAME_DIR="$GAME_DIR" \
ARTIFACT_DIR="$ARTIFACT_DIR" \
ER_EFFECTS_TOML_SOURCE="$TOML_SRC" \
ER_EFFECTS_ALLOW_DEPRECATED_STAGED_SAVE_PROBE=1 \
ER_EFFECTS_SYSTEM_QUIT_REPRO=1 \
ER_EFFECTS_SQ_LOAD_SWITCH=1 \
  bash "$REPO_ROOT/scripts/run-product-continue-direct-probe.sh" &
LAUNCH_PID=$!

# --- 5. monitor the live artifact dir until every load is verified / crash / stall ---
echo "monitoring loads (RAM-oracle verify + report) ..."
python3 "$REPO_ROOT/scripts/multi-load-proof-monitor.py" \
  --artifact-dir "$ARTIFACT_DIR" \
  --targets "$TARGETS_JSON" \
  --report "$ARTIFACT_DIR/proof-report.md" \
  --per-load-deadline "${PER_LOAD_DEADLINE:-120}" \
  --overall-deadline "${OVERALL_DEADLINE:-600}"
RC=$?

echo "monitor exit=$RC ; report -> $ARTIFACT_DIR/proof-report.md"
# tear down the launch (the probe script also self-tears-down via its watcher cap)
pkill -x eldenring.exe 2>/dev/null
wait "$LAUNCH_PID" 2>/dev/null
exit $RC
