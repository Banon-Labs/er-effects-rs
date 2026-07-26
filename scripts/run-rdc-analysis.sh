#!/usr/bin/env bash
# Analyze a RenderDoc .rdc via the WINDOWS qrenderdoc.exe (which bundles the `renderdoc` python module)
# and print the draw-call count + per-event GPU-timing summary (scripts/analyze-rdc.py).
#
#   scripts/run-rdc-analysis.sh <path-to.rdc> [top-N]
#
# qrenderdoc.exe is a Windows app, so the script + config + .rdc must be Windows-accessible paths. We
# stage the analyzer + a JSON config under C:/temp and point the .rdc at a Windows path (wslpath -m).
set -uo pipefail
RDC="${1:?usage: run-rdc-analysis.sh <path-to.rdc> [top]}"
TOP="${2:-25}"
QRD="${QRENDERDOC:-/mnt/c/Program Files/RenderDoc/qrenderdoc.exe}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
[[ -f "$RDC" ]] || { echo "no such .rdc: $RDC" >&2; exit 2; }
[[ -f "$QRD" ]] || { echo "qrenderdoc.exe not found at $QRD (set QRENDERDOC=)" >&2; exit 2; }

WTMP=/mnt/c/temp; mkdir -p "$WTMP"
cp -f "$REPO_ROOT/scripts/analyze-rdc.py" "$WTMP/analyze-rdc.py"
# .rdc must be Windows-accessible; if it is on the WSL fs, copy it under /mnt/c first.
case "$RDC" in
	/mnt/[a-z]/*) RDC_WIN="$(wslpath -m "$RDC")" ;;
	*) cp -f "$RDC" "$WTMP/cap.rdc"; RDC_WIN="C:/temp/cap.rdc" ;;
esac
rm -f "$WTMP/rdc-summary.txt"
printf '{"rdc": "%s", "log": "C:/temp/rdc-summary.txt", "top": %s}\n' "$RDC_WIN" "$TOP" > "$WTMP/rdc-analyze.json"
echo "== analyzing $RDC_WIN via qrenderdoc.exe (replay loads all resources -- can take minutes on a multi-GB cap) =="
# Background qrenderdoc (NOT a >30s foreground timeout -- the replay legitimately needs minutes; bounded
# by the poll budget + taskkill below). --python runs the analyzer, which writes the summary + os._exit()s.
"$QRD" --python 'C:\temp\analyze-rdc.py' >/dev/null 2>&1 &
QPID=$!
# Poll for the summary in bounded 3s steps (background-job + poll pattern, per AGENTS long-op guidance).
for _ in $(seq 1 160); do
	[[ -s "$WTMP/rdc-summary.txt" ]] && grep -q 'TOTAL_GPU_MS\|EventGPUDuration counter NOT\|ERROR' "$WTMP/rdc-summary.txt" 2>/dev/null && break
	kill -0 "$QPID" 2>/dev/null || break
	python3 -c "import time;time.sleep(3)"
done
kill "$QPID" 2>/dev/null; "/mnt/c/Windows/System32/taskkill.exe" /F /IM qrenderdoc.exe >/dev/null 2>&1 || true
echo "== rdc-summary.txt =="
cat "$WTMP/rdc-summary.txt" 2>/dev/null || echo "(no summary written -- qrenderdoc/replay failed; check manually)"
