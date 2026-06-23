#!/usr/bin/env bash
# Lightweight, bounded recovery monitor. Polls /proc/loadavg and swap free until the system has
# clearly recovered from the swap-thrash hang (so we never relaunch the game on a starved machine
# and contaminate the run). Exits READY when healthy, TIMEOUT if it never settles. No game, no Ghidra.
set -euo pipefail
MAX_WAIT_SECONDS="${MAX_WAIT_SECONDS:-900}"
LOAD_THRESHOLD="${LOAD_THRESHOLD:-5.0}"        # 1-min load average must drop below this
SWAP_FREE_MIB_MIN="${SWAP_FREE_MIB_MIN:-8192}" # and at least this much swap freed back
POLL_SECONDS="${POLL_SECONDS:-20}"
waited=0
while (( waited < MAX_WAIT_SECONDS )); do
  load1=$(cut -d' ' -f1 /proc/loadavg)
  swap_free_mib=$(awk '/SwapFree/ {print int($2/1024)}' /proc/meminfo)
  if awk -v l="$load1" -v t="$LOAD_THRESHOLD" 'BEGIN{exit !(l<t)}' \
     && (( swap_free_mib >= SWAP_FREE_MIB_MIN )); then
    echo "READY load1=$load1 swap_free_mib=$swap_free_mib waited=${waited}s"
    exit 0
  fi
  echo "waiting load1=$load1 swap_free_mib=$swap_free_mib waited=${waited}s"
  sleep "$POLL_SECONDS"
  waited=$(( waited + POLL_SECONDS ))
done
echo "TIMEOUT system did not settle within ${MAX_WAIT_SECONDS}s load1=$load1 swap_free_mib=$swap_free_mib"
exit 1
