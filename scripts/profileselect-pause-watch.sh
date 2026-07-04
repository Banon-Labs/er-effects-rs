#!/usr/bin/env bash
# PAUSE-AT-MENU watcher: proves the ER_EFFECTS_SQ_REPRO_SWITCHES=0 autopilot variant drove the
# character to the open 05_010_ProfileSelect (character-load) menu and STOPPED there without loading
# anything -- and then LEAVES THE GAME RUNNING. This watcher NEVER tears down (that is the variant's
# contract); it only reads RAM-oracle telemetry and reports a verdict.
#
# PASS gate (all RAM oracles, no screenshots):
#   sq_repro_paused_at_profile_select == 1   (autopilot reached ProfileSelect and went DONE there)
#   sq_repro_state == 6 (DONE)               (no further driving)
#   system_quit_profile_load_activate_count == 0        (no slot was picked)
#   system_quit_quickload_phase == 0 (IDLE)             (no save-safe switch was armed)
#   system_quit_continue_confirm_fresh_deser_count == 0 (no switch reload deserialized)
#   ER still alive.
# FAIL: any no-load semaphore fires, ER exits on its own (crash), or the hard deadline lapses.
# Exit codes: 0 PASS / 2 LOADED-ANYWAY / 3 ER-EXITED / 4 DEADLINE.
set -u
REPO=/home/banon/projects/er-effects-rs
ART="${ARTIFACT_DIR:-$REPO/target/runtime-probe/system-quit-repro-pause-at-menu}"
TELEM="$ART/er-effects-telemetry.json"
DEADLINE_S=${DEADLINE_S:-110}
POLL_S=${POLL_S:-3}

er_alive() { python3 -c "
import glob
def comm(p):
    try: return open(p).read().strip()
    except OSError: return ''
print('1' if any(comm(p)=='eldenring.exe' for p in glob.glob('/proc/[0-9]*/comm')) else '0')
"; }

# Read a single integer telemetry field (0 if absent).
tf() { python3 -c "
import json,sys
try: t=json.load(open('$TELEM'))
except Exception: print(0); sys.exit()
print(t.get('$1',0))
"; }

start=$SECONDS
saw_er=0
verdict_rc=4
while (( SECONDS - start < DEADLINE_S )); do
  t=$(( SECONDS - start ))
  alive=$(er_alive)
  [[ "$alive" == "1" ]] && saw_er=1
  paused=$(tf sq_repro_paused_at_profile_select)
  state=$(tf sq_repro_state)
  activate=$(tf system_quit_profile_load_activate_count)
  qphase=$(tf system_quit_quickload_phase)
  deser=$(tf system_quit_continue_confirm_fresh_deser_count)
  echo "[pause-watch +${t}s] alive=$alive paused=$paused state=$state activate=$activate quickload_phase=$qphase deser=$deser"

  # ER-exited detection must not depend on this watcher having SEEN the process: a crash before the
  # first poll (observed 2026-07-04: ~CSScaleformValue UAF at ProfileSelect open, +37.8s, watcher
  # started late) leaves saw_er=0 forever and degraded to a deadline verdict. The runner cleans the
  # artifact dir at deploy, so a telemetry file means THIS run's game was up; absent process = exited.
  if [[ "$alive" == "0" && ( "$saw_er" == "1" || -f "$TELEM" ) ]]; then
    echo "===== FAIL (+${t}s): ER exited on its own (crash?) before/after the pause -- paused=$paused; check er-effects-crash-log.txt in $ART ====="
    verdict_rc=3
    break
  fi

  # Any load-path semaphore firing means the variant did NOT pause -- it loaded. Hard FAIL.
  if [[ "${activate:-0}" -ne 0 || "${qphase:-0}" -ne 0 || "${deser:-0}" -ne 0 ]]; then
    echo "===== FAIL (+${t}s): a LOAD fired (activate=$activate quickload_phase=$qphase deser=$deser) -- pause-at-menu contract violated ====="
    verdict_rc=2
    break
  fi

  if [[ "$paused" == "1" && "$state" == "6" && "$alive" == "1" ]]; then
    echo "===== PASS (+${t}s): autopilot PAUSED at the open ProfileSelect (state=DONE, zero picks/arms/loads). Game left RUNNING -- no teardown. Tear down manually: pkill -x eldenring.exe ====="
    verdict_rc=0
    break
  fi
  sleep "$POLL_S"
done

if (( verdict_rc == 4 )); then
  echo "===== FAIL: hard deadline ${DEADLINE_S}s -- pause semaphore never latched (paused=$(tf sq_repro_paused_at_profile_select) state=$(tf sq_repro_state)) ====="
fi

echo "[pause-watch] NO teardown by design; final ER alive=$(er_alive) verdict_rc=$verdict_rc"
exit "$verdict_rc"
