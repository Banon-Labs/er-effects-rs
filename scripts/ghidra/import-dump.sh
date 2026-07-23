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

GZF=${GZF:-/home/banon/projects/reverse/ghidra-projects/pc_eldenring_runtime.1.16.1.exe.gzf}
PROJ=${PROJ:-/home/banon/ghidra_maporch/proj}
PROJ_NAME=${PROJ_NAME:-ermaporch}
TMP=/home/banon/ghidra_maporch/tmp
HEADLESS=/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless

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
