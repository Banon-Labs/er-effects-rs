#!/usr/bin/env bash
# Fast -process query against the PERSISTENT pre-analyzed ER runtime Ghidra dump project, without
# re-importing the ~1.5GB gzf. Reopens the saved program in ~5-10s (vs ~2min for a fresh -import).
#
#   bash scripts/ghidra/query.sh <postScript.java> [scriptArg ...]
#
# The .java GhidraScript's OWN directory is added to -scriptPath, so pass a script that lives in an
# ISOLATED directory (one that contains ONLY compiling .java files). Ghidra 12.1 compiles every .java in
# a -scriptPath dir as one OSGi bundle, and a single sibling that fails to compile poisons the whole
# bundle ("class could not be found"). The repo's top-level scripts/ghidra/ tree has ~80 historical
# postScripts and at least one that does NOT compile under 12.1, so do NOT pass a script from there.
# Instead use scripts/ghidra/rt/ -- an IN-REPO, version-controlled dir kept deliberately CLEAN (every
# .java in it must compile). All query postScripts we use live there and are tracked; see
# scripts/ghidra/rt/README.md. (Everything we run must be in the repo -- no out-of-tree .java.)
#
# Machine auto-detect: this repo is developed on more than one box. Tries each known (maporch, ghidra
# install) pair and uses the first that exists, so the same committed script works on choza and banon.
# Env gotcha baked in: java.io.tmpdir is forced onto /home (the /tmp tmpfs is small and overflows on the
# gzf unpack); plain TMPDIR is ignored for java.io.tmpdir, so GHIDRA_JAVA_OPTIONS sets it explicitly.
set -euo pipefail

PROJ_NAME=ermaporch
PROJ_DIR=""
HEADLESS=""
TMP=""
# "<maporch dir>|<ghidra install dir>"
for cand in \
  "/home/choza/ghidra_maporch|/mnt/d/ghidra/ghidra_12.1_PUBLIC" \
  "/home/banon/ghidra_maporch|/home/banon/tools/ghidra_12.1_PUBLIC" \
  "${ER_GHIDRA_MAPORCH:-/nonexistent}|${ER_GHIDRA_INSTALL:-/nonexistent}"; do
  m="${cand%%|*}"; g="${cand##*|}"
  if [[ -d "$m/proj" && -x "$g/support/analyzeHeadless" ]]; then
    PROJ_DIR="$m/proj"; TMP="$m/tmp"; HEADLESS="$g/support/analyzeHeadless"; break
  fi
done

if [[ -z "$PROJ_DIR" ]]; then
  echo "ghidra query: no persistent project + ghidra install found (set ER_GHIDRA_MAPORCH / ER_GHIDRA_INSTALL, or import the gzf into <maporch>/proj as program '$PROJ_NAME')" >&2
  exit 3
fi
if [[ $# -lt 1 ]]; then
  echo "Usage: bash scripts/ghidra/query.sh <postScript.java> [scriptArg ...]" >&2
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

exec "$HEADLESS" "$PROJ_DIR" "$PROJ_NAME" \
  -process \
  -noanalysis \
  -readOnly \
  -scriptPath "$SCRIPT_DIR" \
  -postScript "$SCRIPT_NAME" "$@"
