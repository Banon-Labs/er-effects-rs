#!/usr/bin/env bash
# End-to-end proof of scripts/er-stale-run-sentinel.sh against a REAL process.
#
# WHY THIS EXISTS SEPARATELY FROM `--selftest`
# -------------------------------------------
# `--selftest` proves the CLASSIFIER: given a profile, does a path get the right verdict. It
# deliberately never calls `teardown`, because scripts/check.sh runs it and a real game may be live.
# That leaves the other half unproven: /proc discovery of the live profile, and the kill itself.
# This script closes that, the same way the original sentinel was proven -- with a decoy process --
# so no claim rests on the selftest alone.
#
# A decoy binary named `me3` carries `-p <synthetic profile>` on its command line, so the sentinel
# must discover the profile from /proc exactly as it would for a real run:
#
#   inert path (scripts/frida-trace-ersc.py, .cupcake/*.rego)  -> decoy must SURVIVE
#   crate that builds an UNLOADED DLL (er-armament-icons)      -> decoy must SURVIVE
#   crate in the loaded closure (er-game-base)                 -> decoy must be KILLED
#
# NOT wired into scripts/check.sh on purpose: it calls `teardown`, and a gate that can kill the
# user's game is not a gate you run unattended. It REFUSES (exit 2) if a real run is live, so
# running it by hand can never take down a session in progress.
#
# Usage: bash scripts/test-er-stale-run-sentinel-e2e.sh
set -uo pipefail

REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
SENTINEL="$REPO_ROOT/scripts/er-stale-run-sentinel.sh"
TMPDIR_E2E="$(mktemp -d)"
ER_SENTINEL_LOG="$TMPDIR_E2E/sentinel.log"
export ER_SENTINEL_LOG
fails=0
DECOY=""

# Invoked by the EXIT trap below, which shellcheck does not model.
# shellcheck disable=SC2329
cleanup() {
  [[ -n "$DECOY" ]] && kill -KILL "$DECOY" 2>/dev/null
  rm -rf "$TMPDIR_E2E"
}
trap cleanup EXIT

# `status` exits 1 when something is live. Refuse rather than kill a session in progress.
if ! "$SENTINEL" status >/dev/null 2>&1; then
  echo "REFUSED: a real run is live; this proof calls teardown, so it will not run now." >&2
  exit 2
fi

cat >"$TMPDIR_E2E/decoy.me3" <<'TOML'
profileVersion = "v1"
[[supports]]
game = "eldenring"
[[natives]]
path = '/nonexistent/er_effects_rs.dll'
[[natives]]
path = '/nonexistent/er_invasion_warp_dll.dll'
TOML

# comm comes from the executable's basename, so the decoy must BE a binary called `me3`. A copied
# python interpreter is used because it accepts arbitrary trailing argv (which becomes the `-p
# <profile>` the sentinel has to parse out of /proc/<pid>/cmdline) and blocks on demand.
if ! cp -f "$(command -v python3)" "$TMPDIR_E2E/me3" 2>/dev/null; then
  echo "SKIP: could not stage a decoy binary" >&2
  exit 2
fi
"$TMPDIR_E2E/me3" -c 'import time; time.sleep(120)' -p "$TMPDIR_E2E/decoy.me3" &
DECOY=$!
read -r -t 1 </dev/null 2>/dev/null || true
if ! kill -0 "$DECOY" 2>/dev/null; then
  echo "SKIP: decoy did not start" >&2
  exit 2
fi

alive() { kill -0 "$DECOY" 2>/dev/null; }

survives() {
  local path="$1" label="$2"
  "$SENTINEL" check "$path" >/dev/null 2>&1
  if alive; then
    echo "  ok   $label left the run ALIVE"
  else
    echo "  FAIL $label KILLED the run"
    fails=$((fails + 1))
  fi
}

echo "  -- must NOT tear down --"
survives "$REPO_ROOT/scripts/frida-trace-ersc.py" "scripts/frida-trace-ersc.py (host-side frida)"
survives "$REPO_ROOT/scripts/er-launch-gate.py" "scripts/er-launch-gate.py (pre-launch gate)"
survives "$REPO_ROOT/.cupcake/policies/claude/idle_hold.rego" ".cupcake/*.rego (agent policy)"
survives "$REPO_ROOT/docs/plans/world-map-invasion-warp.md" "docs/ (prose)"
survives "$REPO_ROOT/crates/er-armament-icons/src/lib.rs" "crate building an UNLOADED DLL"

echo "  -- must TEAR DOWN --"
"$SENTINEL" check "$REPO_ROOT/crates/er-game-base/src/lib.rs" >/dev/null 2>&1
read -r -t 1 </dev/null 2>/dev/null || true
if alive; then
  echo "  FAIL transitive-dependency edit did NOT kill the run"
  fails=$((fails + 1))
else
  echo "  ok   transitive-dependency edit (crates/er-game-base) KILLED the run"
  DECOY=""
fi

echo "  -- the log, which is the point of the log --"
if [[ -s "$ER_SENTINEL_LOG" ]]; then
  while IFS= read -r line; do printf '    %s\n' "$line"; done <"$ER_SENTINEL_LOG"
  if [[ "$(awk -F'\t' '$2=="TEARDOWN"' "$ER_SENTINEL_LOG" | wc -l)" -eq 1 ]]; then
    echo "  ok   exactly one TEARDOWN line names the edit that killed the run"
  else
    echo "  FAIL the log does not identify exactly one killing edit"
    fails=$((fails + 1))
  fi
else
  echo "  FAIL no log written"
  fails=$((fails + 1))
fi

if [[ $fails -eq 0 ]]; then echo "e2e ok"; exit 0; fi
echo "e2e FAILED ($fails)"
exit 1
