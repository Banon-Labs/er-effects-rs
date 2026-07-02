#!/usr/bin/env bash
# Post-reload switch watcher: unlike switch-failfast-poll.sh, this does NOT tear down on the
# TRANSIENT wrong-character state that exists BEFORE the reload deserialize runs (the stale
# pre-switch character is still resident while the world tears down). It only reaches a verdict once
# the reload has actually committed (telemetry system_quit_continue_confirm_fresh_deser_count >= 1),
# then reads the switch-character oracle. Tears down on: ER self-exit (crash = FAIL), a post-reload
# oracle verdict, or a hard deadline. Bounded, agent-owned teardown.
set -u
REPO=/home/banon/projects/er-effects-rs
ART="${ARTIFACT_DIR:-$REPO/target/runtime-probe/system-quit-repro-selfdrive}"
TELEM="$ART/er-effects-telemetry.json"
ORACLE="$REPO/scripts/switch-character-oracle.py"
DEADLINE_S=${DEADLINE_S:-80}
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
verdict_rc=99
while (( SECONDS - start < DEADLINE_S )); do
  t=$(( SECONDS - start ))
  alive=$(er_alive)
  [[ "$alive" == "1" ]] && saw_er=1
  deser=$(tf system_quit_continue_confirm_fresh_deser_count)
  block=$(tf system_quit_continue_confirm_block_count)
  reset_inv=$(tf system_quit_gaitem_reset_invocations)
  released=$(tf system_quit_gaitem_reset_released_count)
  slack_b=$(tf system_quit_gaitem_reset_last_slack_before)
  slack_a=$(tf system_quit_gaitem_reset_last_slack_after)
  charn=$(python3 -c "import json;print(json.load(open('$TELEM')).get('oracle_char_name',''))" 2>/dev/null)
  echo "[watch +${t}s] alive=$alive deser_ok=$deser blocked=$block gaitem_reset=$reset_inv released=$released slack ${slack_b}->${slack_a} char='$charn'"

  if [[ "$saw_er" == "1" && "$alive" == "0" ]]; then
    echo "===== ER EXITED (+${t}s) -- crash or teardown. deser_ok=$deser reset=$reset_inv released=$released. If deser_ok>=1 and this was a crash, the gaitem fix did NOT prevent it. ====="
    verdict_rc=3
    break
  fi

  # Only judge the oracle AFTER the reload deserialize has committed (post-reload window).
  if [[ "${deser:-0}" -ge 1 ]]; then
    SAVE=$(ls "$ART"/save/EldenRing/*/ER0000.sl2 2>/dev/null | head -1)
    if [[ -n "$SAVE" && -f "$TELEM" ]]; then
      out=$(python3 "$ORACLE" --save "$SAVE" --telemetry "$TELEM" 2>/dev/null)
      rc=$?
      echo "  [post-reload oracle] rc=$rc $out"
      if [[ "$rc" == "0" ]]; then
        echo "===== PASS (+${t}s): reload committed (deser_ok=$deser, gaitem released=$released) and loaded the CORRECT (picked) character in a stable world ====="
        verdict_rc=0; break
      fi
      if [[ "$rc" == "2" ]]; then
        echo "===== FAIL (+${t}s): reload committed but loaded the WRONG character ====="
        verdict_rc=2; break
      fi
      # rc==10 waiting for a stable world: keep polling.
    fi
  fi
  sleep "$POLL_S"
done

if (( SECONDS - start >= DEADLINE_S )); then
  echo "[watch] hard deadline ${DEADLINE_S}s reached; last deser_ok=$(tf system_quit_continue_confirm_fresh_deser_count) reset=$(tf system_quit_gaitem_reset_invocations)"
fi

echo "[watch] tearing down eldenring.exe"
pkill -x eldenring.exe 2>/dev/null
sleep 1
echo "[watch] final ER alive=$(er_alive)  verdict_rc=$verdict_rc"
exit "$verdict_rc"
