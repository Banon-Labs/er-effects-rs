#!/usr/bin/env bash
# Launch a long-running command fully detached so the invoking shell (and any
# 45s agent Bash-tool cap) returns immediately while the work continues as a
# separate OS process. Progress is observable via the log file and a DONE marker
# (sleep-free, file-based synchronization per repo runtime hygiene).
#
#   scripts/run-detached.sh <logfile> <cmd...>
#
# Writes <cmd...> stdout+stderr to <logfile> and, on completion, writes
# "EXIT_CODE=<n>" to <logfile>.done. Poll for that marker instead of sleeping.
set -euo pipefail
log="$1"; shift
done_marker="${log}.done"
rm -f -- "$log" "$done_marker"
setsid bash -c '
  "$@" >>"'"$log"'" 2>&1
  echo "EXIT_CODE=$?" >"'"$done_marker"'"
' _ "$@" >/dev/null 2>&1 &
disown || true
echo "launched pid=$! log=$log done=$done_marker"
