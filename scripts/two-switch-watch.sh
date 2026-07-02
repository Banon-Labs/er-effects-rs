#!/usr/bin/env bash
# Verify TWO back-to-back harness-driven switches load TWO DIFFERENT characters after one startup.
# Watches telemetry: fresh_deser_count should reach 2, gaitem_reset invocations 2, and the debug log
# records each switch's deserialized slot. Reads the final loaded identity via the switch oracle.
# Does NOT tear down on the transient stale state; only on ER self-exit, both switches done, or a cap.
set -u
REPO=/home/banon/projects/er-effects-rs
ART="${ARTIFACT_DIR:-$REPO/target/runtime-probe/system-quit-repro-selfdrive}"
TELEM="$ART/er-effects-telemetry.json"
LOG="$ART/er-effects-autoload-debug.log"
DEADLINE_S=${DEADLINE_S:-115}
POLL_S=${POLL_S:-3}

er_alive() { python3 -c "
import glob
def comm(p):
    try: return open(p).read().strip()
    except OSError: return ''
print('1' if any(comm(p)=='eldenring.exe' for p in glob.glob('/proc/[0-9]*/comm')) else '0')
"; }
tf() { python3 -c "
import json,sys
try: t=json.load(open('$TELEM'))
except Exception: print(0); sys.exit()
print(t.get('$1',0))
"; }

start=$SECONDS
saw_er=0
verdict=99
while (( SECONDS - start < DEADLINE_S )); do
  t=$(( SECONDS - start ))
  alive=$(er_alive)
  [[ "$alive" == "1" ]] && saw_er=1
  deser=$(tf system_quit_continue_confirm_fresh_deser_count)
  block=$(tf system_quit_continue_confirm_block_count)
  reset_inv=$(tf system_quit_gaitem_reset_invocations)
  swidx=$(python3 -c "
import re
try: lines=open('$LOG',encoding='utf-8',errors='replace').read()
except Exception: lines=''
import re
m=re.findall(r'switch #(\d+)/\d+ load CONFIRMED',lines)
print(m[-1] if m else '0')")
  # per-switch deserialized slots from the own-load-feed lines
  slots=$(python3 -c "
import re
try: lines=open('$LOG',encoding='utf-8',errors='replace').readlines()
except Exception: lines=[]
s=[re.search(r'parser 0x[0-9a-f]+\(slot=(\d+)\) ret=1',l).group(1) for l in lines if re.search(r'own-load-feed: parser .*ret=1',l)]
print(','.join(s))")
  charn=$(python3 -c "import json;print(json.load(open('$TELEM')).get('oracle_char_name',''))" 2>/dev/null)
  echo "[2sw +${t}s] alive=$alive deser_ok=$deser blocked=$block gaitem_reset=$reset_inv confirmed_switch=$swidx deser_slots=[$slots] char='$charn'"

  if [[ "$saw_er" == "1" && "$alive" == "0" ]]; then
    echo "===== ER EXITED (+${t}s). deser_ok=$deser slots=[$slots]. If deser_ok<2 this is a crash/early-exit before the 2nd load. ====="
    verdict=3; break
  fi

  # Both switches committed: deser_ok>=2 and two DISTINCT slots recorded.
  if [[ "${deser:-0}" -ge 2 ]]; then
    distinct=$(python3 -c "
s='$slots'.split(',') if '$slots' else []
s=[x for x in s if x!='']
print('1' if len(set(s))>=2 and len(s)>=2 else '0')")
    if [[ "$distinct" == "1" ]]; then
      echo "===== PASS (+${t}s): TWO back-to-back harness-driven switches loaded TWO DIFFERENT characters (deser_slots=[$slots], gaitem_reset=$reset_inv). ====="
      # HOLD before teardown so the loaded world is visible + screenshotted (user feedback: teardown
      # was too fast to see/capture the post-load state). Capture the validated ER window now.
      HOLD_AFTER_PASS_S=${HOLD_AFTER_PASS_S:-12}
      mkdir -p "$ART/two-switch-capture"
      python3 "$REPO/scripts/capture-er-window.py" "$ART/two-switch-capture/final-2sw-loaded-world.jpg" >/dev/null 2>&1
      echo "[2sw] captured post-load world -> $ART/two-switch-capture/final-2sw-loaded-world.jpg; holding ${HOLD_AFTER_PASS_S}s before teardown so it stays on screen"
      for _i in $(seq 1 "$HOLD_AFTER_PASS_S"); do
        [[ "$(er_alive)" == "0" ]] && break
        sleep 1
      done
      verdict=0; break
    else
      echo "===== FAIL (+${t}s): 2 deserializes but slots not distinct (slots=[$slots]) ====="
      verdict=2; break
    fi
  fi
  sleep "$POLL_S"
done

if (( SECONDS - start >= DEADLINE_S )); then
  echo "[2sw] hard deadline ${DEADLINE_S}s: last deser_ok=$(tf system_quit_continue_confirm_fresh_deser_count) slots=[$slots]"
  verdict=4
fi

echo "[2sw] tearing down eldenring.exe"
pkill -x eldenring.exe 2>/dev/null
sleep 1
echo "[2sw] final ER alive=$(er_alive)  verdict=$verdict"
exit "$verdict"
