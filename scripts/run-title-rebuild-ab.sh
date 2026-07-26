#!/usr/bin/env bash
# Armed-vs-disarmed warm-title-rebuild A/B (bd decoupled-diagnostics-architecture-buildplan-2026-07-24).
#
# Runs the same-char switch reload TWICE with an IDENTICAL diagnostic set -- the decoupled read-side
# oracles (er-oracle-title-binding + er-oracle-stream-overlap, in er-telemetry-dll) enabled in BOTH arms:
#   ARMED    = product autoload/own_load reload (MOD_ARMED=1, DRIVE_MODE=reload)
#   DISARMED = telemetry-only, harness-driven native reload (the run49 path; product feature off)
# Because the diagnostics are decoupled + identical in both arms, any delta between the two oracle
# streams is attributable to the PRODUCT FEATURE alone. The read: title-binding proxy_handle_nonnull
# True(disarmed) vs False(armed) pins the product hold that deadlocks the armed native Continue; the
# stream-overlap window is the dip mechanism in both arms.
#
# Crash logging is held default-on + CONSTANT in both arms (ER_EFFECTS_CRASH_LOG=1); RE-instrumentation
# markers are left ABSENT. No product-feature toggle differs except MOD_ARMED itself.
set -uo pipefail

REPO="/home/choza/projects/er-effects-rs"
GAME_DIR="${GAME_DIR:-/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game}"
CORPUS="${BOOT_FILE:-/mnt/a/Code Projects/Elden Ring Save Manager/data/save-files/100-Lilbro/ER0000.sl2}"
ROOT="${AB_ROOT:-$REPO/target/runtime-probe/title-rebuild-ab-$(date +%H%M%S)}"
mkdir -p "$ROOT/armed" "$ROOT/disarmed"

bash "$REPO/scripts/steam-running.sh" >/dev/null 2>&1 || { echo "FAIL: Steam not running"; exit 1; }
[[ -f "$CORPUS" ]] || { echo "FAIL: BOOT_FILE not found: $CORPUS"; exit 1; }

# Enable the two read-side oracles (independent .on markers; the run sweep does not touch er-oracle-*).
: >"$GAME_DIR/er-oracle-title-binding.on"
: >"$GAME_DIR/er-oracle-stream-overlap.on"

run_arm() { # $1=name ; remaining args = extra VAR=val env for the run
  local name="$1"
  shift
  echo "== ARM: $name =="
  rm -f "$GAME_DIR"/er-oracle-title-binding.jsonl "$GAME_DIR"/er-oracle-stream-overlap.jsonl
  env "$@" ER_EFFECTS_CRASH_LOG=1 TEARDOWN_ON_TITLE_REBUILD=1 "BOOT_FILE=$CORPUS" \
    ARTIFACT_DIR="$ROOT/$name" \
    bash "$REPO/scripts/run-vanilla-reload-agentdriven.sh" >"$ROOT/$name/run.log" 2>&1 || true
  cp -f "$GAME_DIR"/er-oracle-title-binding.jsonl "$ROOT/$name/" 2>/dev/null || true
  cp -f "$GAME_DIR"/er-oracle-stream-overlap.jsonl "$ROOT/$name/" 2>/dev/null || true
}

# ARMED: the armed autoload produces load1, so DRIVE_MODE=reload (harness drives only the reloads).
run_arm armed MOD_ARMED=1 DRIVE_MODE=reload
# DISARMED: telemetry-only disables autoload, so DRIVE_MODE=full -- the harness must drive the initial
# Continue (load1) itself; DRIVE_MODE=reload would stall at phase0 wait_load_in with no autoload world.
run_arm disarmed DRIVE_MODE=full

echo "== A/B DIFF (title-binding + stream-overlap) =="
python3 "$REPO/scripts/analyze-title-binding-stream.py" --compare "$ROOT/armed" "$ROOT/disarmed" || true
echo "artifacts: $ROOT"
