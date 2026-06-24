#!/usr/bin/env bash
# Capture the validated ER window repeatedly during an ONSCREEN run, so the
# end-state (e.g. a blocking dialog the teardown-focus race would miss) is grabbed
# while the game is still stable. Each capture focuses the exact-class ER window
# first (capture-er-window.py), so it succeeds where the teardown capture loses
# the focus race. Bounded iteration count; stops early when ER exits.
#   capture-er-loop.sh <out_dir> [max_iters]
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out_dir="$1"
max_iters="${2:-40}"
mkdir -p "$out_dir"
i=0
while (( i < max_iters )); do
  pgrep -x eldenring.exe >/dev/null 2>&1 || { echo "loop: ER gone after $i frames"; break; }
  printf -v n '%03d' "$i"
  python3 "$repo_root/scripts/capture-er-window.py" "$out_dir/frame-$n.jpg" >/dev/null 2>&1 || true
  i=$((i + 1))
done
echo "loop done: $i frames -> $out_dir"
