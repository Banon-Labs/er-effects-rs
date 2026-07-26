#!/usr/bin/env bash
# Open a NEW kitty TAB tailing a dynamic-workflow run with color/formatting.
#
# Usage: scripts/watch-workflow.sh [RUNID|latest|/abs/run/dir] [--full]
#   RUNID   a wf_... id (default: latest run under this project)
#   --full  include agent reasoning text, not just tool calls + results
#
# Requires kitty remote control (KITTY_LISTEN_ON set; `allow_remote_control yes`).
# Falls back to a separate kitty window, then to running inline, if it is off.
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RENDER="$REPO/scripts/watch-workflow.py"

# First non-flag positional is the run spec; everything else (flags + their values,
# e.g. --full, --result-lines N) is forwarded to the renderer verbatim.
ARG="latest"; PASS=(); expect_val=0; got_arg=0
for a in "$@"; do
  if [[ $expect_val == 1 ]]; then PASS+=("$a"); expect_val=0; continue; fi
  case "$a" in
    --result-lines) PASS+=("$a"); expect_val=1 ;;
    --*) PASS+=("$a") ;;
    *) if [[ $got_arg == 0 ]]; then ARG="$a"; got_arg=1; else PASS+=("$a"); fi ;;
  esac
done

# Resolve the run dir up front so the tab title can name it and a bad spec fails
# here, not in a detached tab. The renderer prints the dir on header line 2.
RUNDIR="$(python3 "$RENDER" "$ARG" --no-follow 2>/dev/null | sed -n '2p' | sed -E 's/\x1b\[[0-9;]*m//g')"
TITLE="wf:$(basename "${RUNDIR:-run}")"

CMD=(python3 "$RENDER" "$ARG" --follow "${PASS[@]}")

if [[ -n "${KITTY_LISTEN_ON:-}" ]] && kitty @ --to "$KITTY_LISTEN_ON" ls >/dev/null 2>&1; then
  kitty @ --to "$KITTY_LISTEN_ON" launch --type=tab --keep-focus \
    --tab-title "$TITLE" --cwd "$REPO" -- "${CMD[@]}"
  echo "opened kitty tab '$TITLE' tailing ${RUNDIR:-latest run} (focus kept on Claude)"
elif command -v kitty >/dev/null 2>&1; then
  kitty --title "$TITLE" --directory "$REPO" "${CMD[@]}" >/dev/null 2>&1 &
  echo "kitty remote control off; opened a separate kitty window '$TITLE'"
else
  echo "no kitty; running inline (Ctrl-C to stop):"
  exec "${CMD[@]}"
fi
