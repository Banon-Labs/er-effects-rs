#!/usr/bin/env bash
# Query the PERSISTENT pre-analyzed ER runtime Ghidra project WITHOUT re-importing the
# ~1.5GB gzf every time. Runs analyzeHeadless in -process mode against the saved program.
#
#   scripts/ghidra-query.sh <postScript.java> [scriptArg ...]
#
# The .java GhidraScript may live anywhere; its directory is added to -scriptPath.
# The persistent project (see ghidra-persistent-project-reuse-2026-06-22 bd memory) is at
# /home/banon/ghidra_maporch/proj, program name "ermaporch". This wrapper is the FAST path:
# a trivial -process query returns in seconds vs ~2min for a fresh -import.
#
# Env gotchas baked in:
#   - java.io.tmpdir is forced to /home (the /tmp tmpfs is a near-full 32G and overflows).
#   - project dir is dotless on /home (Ghidra rejects dot-prefixed project dirs).
set -euo pipefail

PROJ_DIR=/home/banon/ghidra_maporch/proj
PROJ_NAME=ermaporch
TMP=/home/banon/ghidra_maporch/tmp
HEADLESS=/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless

if [[ $# -lt 1 ]]; then
  echo "Usage: scripts/ghidra-query.sh <postScript.java> [scriptArg ...]" >&2
  exit 2
fi

SCRIPT_FILE="$1"; shift
if [[ ! -f "$SCRIPT_FILE" ]]; then
  echo "postScript not found: $SCRIPT_FILE" >&2
  exit 2
fi
SCRIPT_DIR="$(cd "$(dirname "$SCRIPT_FILE")" && pwd)"
SCRIPT_NAME="$(basename "$SCRIPT_FILE")"

mkdir -p "$TMP"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

# -process (no -import) reopens the SAVED program. -noanalysis: it's already analyzed.
exec "$HEADLESS" "$PROJ_DIR" "$PROJ_NAME" \
  -process \
  -noanalysis \
  -readOnly \
  -scriptPath "$SCRIPT_DIR" \
  -postScript "$SCRIPT_NAME" "$@"
