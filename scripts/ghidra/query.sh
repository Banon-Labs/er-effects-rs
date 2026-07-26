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

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROJ_NAME="${GHIDRA_PROJ_NAME:-ermaporch}"
MAPORCH="${ER_GHIDRA_MAPORCH:-$HOME/ghidra_maporch}"
PROJ_DIR="${GHIDRA_PROJ_DIR:-$MAPORCH/proj}"
TMP="${GHIDRA_TMPDIR:-$MAPORCH/tmp}"
# Pick the newest installed Ghidra by default; explicit GHIDRA_* overrides still win.
# shellcheck source=scripts/ghidra/resolve-ghidra.sh
source "$REPO/scripts/ghidra/resolve-ghidra.sh"
HEADLESS="$(resolve_ghidra_headless)" || {
  echo "ghidra query: no executable Ghidra analyzeHeadless found (set GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS)" >&2
  exit 3
}

if [[ ! -d "$PROJ_DIR" ]]; then
  echo "ghidra query: persistent project not found at $PROJ_DIR (set GHIDRA_PROJ_DIR / ER_GHIDRA_MAPORCH, or import the gzf into that project as program '$PROJ_NAME')" >&2
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
