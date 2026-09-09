#!/usr/bin/env bash
# Launch Elden Ring and get a Frida exception handler armed before the 29.3s fault.
#
# Why the three steps are one script rather than three agent turns: with
# `ersc_invade_observer = true` the game raises STATUS_ILLEGAL_INSTRUCTION at
# eldenring.exe+0x10043 at ms_since_install 29288 and 29299 across two runs -- deterministic to
# 11ms -- and the DLL testimony that ends the launch step lands around +4s. That leaves roughly
# twenty seconds to bring the wine-side server up inside the container and attach. Round-tripping
# each step through the agent loop spends most of that budget on latency, and a catcher armed at
# +31s catches nothing.
#
# frida-server must start inside the game's pressure-vessel container, which needs the game
# process to exist -- hence the order. `--force` replaces a server left behind by a previous run,
# whose container is gone and which would accept the connection and then never answer.
#
#   bash scripts/er-run-with-fault-catcher.sh [er-run-branch.py args...]
set -o pipefail
cd "$(dirname "$0")/.." || exit 1
python3 scripts/er-run-branch.py "$@" || exit 1
python3 scripts/er-frida-up.py --force || exit 1
exec uv run --with frida python3 -u scripts/er-frida-watch.py --agent scripts/frida/fault-catch.js
