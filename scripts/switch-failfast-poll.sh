#!/usr/bin/env bash
# Live fail-fast poller: watch the running probe's telemetry with the
# switch-character-oracle and tear ER down the instant it flags the wrong
# character (or on a hard deadline / ER exit). Demonstrates the semaphore
# firing during a real run, then leaves no ER process behind.
#
# Usage: scripts/switch-failfast-poll.sh   (defaults to the repro-selfdrive artifact dir)
set -u
REPO=/home/banon/projects/er-effects-rs
ART="${ARTIFACT_DIR:-$REPO/target/runtime-probe/system-quit-repro-selfdrive}"
TELEM="$ART/er-effects-telemetry.json"
ORACLE="$REPO/scripts/switch-character-oracle.py"
DEADLINE_S=${DEADLINE_S:-125}
POLL_S=${POLL_S:-3}

er_alive() { python3 -c "
import glob
def comm(p):
    try: return open(p).read().strip()
    except OSError: return ''
print('1' if any(comm(p)=='eldenring.exe' for p in glob.glob('/proc/[0-9]*/comm')) else '0')
"; }

start=$SECONDS
saw_er=0
verdict_rc=0
while (( SECONDS - start < DEADLINE_S )); do
  t=$(( SECONDS - start ))
  alive=$(er_alive)
  [[ "$alive" == "1" ]] && saw_er=1
  if [[ "$saw_er" == "1" && "$alive" == "0" ]]; then
    echo "[poll +${t}s] eldenring.exe exited (crash or teardown) -- stopping poll"
    break
  fi
  SAVE=$(ls "$ART"/save/EldenRing/*/ER0000.sl2 2>/dev/null | head -1)
  if [[ -n "$SAVE" && -f "$TELEM" ]]; then
    out=$(python3 "$ORACLE" --save "$SAVE" --telemetry "$TELEM" 2>/dev/null)
    rc=$?
    echo "[poll +${t}s] rc=$rc $out"
    # 2 = wrong character (FAIL, stop); 0 = correct character in a stable world (PASS, stop);
    # 10 = not armed yet (keep polling).
    if [[ "$rc" == "2" ]]; then
      echo "===== FAIL-FAST TRIGGERED (+${t}s): switch loaded the WRONG character ====="
      verdict_rc=2
      break
    fi
    if [[ "$rc" == "0" ]]; then
      echo "===== PASS (+${t}s): switch loaded the CORRECT (picked) character in a stable world ====="
      verdict_rc=0
      break
    fi
  else
    echo "[poll +${t}s] waiting for telemetry/staged-save to appear (alive=$alive)"
  fi
  sleep "$POLL_S"
done

if (( SECONDS - start >= DEADLINE_S )); then
  echo "[poll] hard deadline ${DEADLINE_S}s reached without a wrong-character verdict"
fi

echo "[poll] tearing down eldenring.exe"
pkill -x eldenring.exe 2>/dev/null
sleep 1
echo "[poll] final ER alive=$(er_alive)  verdict_rc=$verdict_rc"
exit "$verdict_rc"
