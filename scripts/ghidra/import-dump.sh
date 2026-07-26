#!/usr/bin/env bash
# One-shot persistent import of the ER runtime DUMP gzf into the reusable `ermaporch` project
# (the SEMANTICS project: real symbols/types, but addresses carry the ~0x10 dump-vs-deobf shift).
# The MCP daemon defaults to this project. Companion to import-deobf.sh (which builds erdeobf).
#
# The gzf is a pre-analyzed export, so this is just an import (-noanalysis) -- fast (~2 min) vs
# the deobf binary's multi-hour raw analysis. Supply your own gzf (copyrighted game data, not in
# the repo) via GZF=... ; default points at the known local export.
#
# Same tmpdir gotcha as the other helpers: force java.io.tmpdir onto /home (the /tmp tmpfs is a
# near-full 32G and overflows when Ghidra unpacks the ~1.5GB gzf).
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# shellcheck source=scripts/ghidra/resolve-ghidra.sh
source "$REPO/scripts/ghidra/resolve-ghidra.sh"

GZF=${GZF:-/home/banon/projects/reverse/ghidra-projects/pc_eldenring_runtime.1.16.1.exe.gzf}
PROJ=${PROJ:-$HOME/ghidra_maporch/proj}
PROJ_NAME=${PROJ_NAME:-ermaporch}
TMP="${GHIDRA_TMPDIR:-$HOME/ghidra_maporch/tmp}"
HEADLESS="$(resolve_ghidra_headless)" || { echo "no executable Ghidra analyzeHeadless found; set GHIDRA_INSTALL_DIR or GHIDRA_HEADLESS" >&2; exit 3; }

if [[ ! -f "$GZF" ]]; then
  echo "dump gzf not found: $GZF  (set GZF=/path/to/runtime.gzf)" >&2
  exit 2
fi

mkdir -p "$TMP" "$PROJ"
export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

"$HEADLESS" "$PROJ" "$PROJ_NAME" \
  -import "$GZF" \
  -noanalysis \
  -overwrite
echo "IMPORT_EXIT=$?"
