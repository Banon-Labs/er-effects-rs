#!/usr/bin/env bash
# Launch Elden Ring, wait until the world is up, then attach a Frida agent to it.
#
# Why the wait is in the script rather than between two agent turns: `scripts/er-frida-up.py` has
# to run `frida-server.exe` inside the game's pressure-vessel container, which means the game
# process must already exist -- and if it does not, the script falls back to the host namespace,
# where the server talks to a different wineserver, accepts the connection and then never answers.
# That fallback is silent, so a run can look attached while nothing is.
#
# Waiting for `oracle_player_present` rather than for the process is deliberate. The DLL's own
# telemetry is the readiness signal the rest of this repo uses, and attaching during the load
# window is the one thing measured to be riskier than attaching after it.
#
#   bash scripts/er-run-with-frida.sh scripts/frida/ersc-handoff.js --save '<path>:3'
set -o pipefail
cd "$(dirname "$0")/.." || exit 1

AGENT="${1:?usage: er-run-with-frida.sh <agent.js> [er-run-branch.py args...]}"
shift

LAUNCH_LOG="${ER_FRIDA_LAUNCH_LOG:-$HOME/.cache/er-frida/launch.log}"
mkdir -p "$(dirname "$LAUNCH_LOG")"
python3 scripts/er-run-branch.py "$@" > "$LAUNCH_LOG" 2>&1 || {
    tail -25 "$LAUNCH_LOG" >&2
    exit 1
}
RUN_ID=$(sed -n 's/^  run  *\(br-[0-9a-z-]*\).*/\1/p' "$LAUNCH_LOG" | head -1)
if [ -z "$RUN_ID" ]; then
    echo "no run id in $LAUNCH_LOG -- the launch did not report one" >&2
    tail -25 "$LAUNCH_LOG" >&2
    exit 1
fi
TELEMETRY="$HOME/.cache/er-me3-runs/$RUN_ID/er-quickload-telemetry.json"
echo "run $RUN_ID -- waiting for the world"

# Bounded by the same canonical runtime cap the rest of the repo reads, so this cannot outlive a
# game that never reaches the world. The cap is only the bound: the wait itself blocks on inotify
# events in the run directory, so it returns as the DLL rewrites its telemetry rather than up to
# four seconds later, and it costs nothing while the game boots.
CAP=$(python3 scripts/runtime_timeout_cap.py 2>/dev/null || echo 300)
python3 - "$TELEMETRY" "$CAP" <<'PY'
import json
import os
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path("scripts").resolve()))
import er_run_lib

telemetry = pathlib.Path(sys.argv[1])
cap = float(sys.argv[2])
# The cap on one slice, not on the wait. An inotify event ends a slice at once; this only bounds a
# slice in which nothing is written at all.
slice_seconds = 4.0


def player_present() -> bool:
    try:
        document = json.loads(telemetry.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        # Absent, half-written, or not yet valid json. All three mean "not yet", and the next
        # write brings another look.
        return False
    return bool(document.get("oracle_player_present"))


deadline = time.monotonic() + cap
# The run directory, not the telemetry file: the document is replaced rather than appended, so a
# watch pinned to its inode would go deaf at the first write.
watch = er_run_lib.DirectoryWatch(telemetry.parent)
try:
    while True:
        if player_present():
            sys.exit(0)
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            sys.exit(2)
        if watch.available:
            watch.wait(min(slice_seconds, remaining))
        else:
            # No inotify: a bounded wait on this process, which never exits. Still a block on a
            # real descriptor, and the deadline above still bounds the loop.
            er_run_lib.wait_for_exit(os.getpid(), min(slice_seconds, remaining))
finally:
    watch.close()
PY
if [ "$?" -ne 0 ]; then
    echo "the world never came up within ${CAP}s -- not attaching" >&2
    exit 2
fi
echo "world is up; bringing the wine-side server up inside the container"
python3 scripts/er-frida-up.py --force || exit 1
exec uv run --with frida python3 -u scripts/er-frida-watch.py --agent "$AGENT"
