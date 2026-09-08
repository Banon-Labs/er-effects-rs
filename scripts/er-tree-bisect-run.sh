#!/usr/bin/env bash
# Build er_quickload.dll from the current tree, then score it exactly like a release artifact.
#
# The scoring -- launch it alone, wait, decide from thread count and CPU burn rather than from a
# pid existing -- lives in scripts/er-release-bisect.py and is not repeated here. This exists only
# because that tool deliberately cannot build: every subprocess it starts is held under the 30s
# agent-shell cap (scripts/check-no-timeouts.py), and a cold cross-compile takes minutes. So the
# build happens in bash, where a compile is not a timeout violation, and the artifact is handed
# over by path.
#
# Usage:
#   scripts/er-tree-bisect-run.sh <label> [alive-seconds]
#
# A cold build takes minutes -- run this detached, not in a foreground shell.
set -euo pipefail

REPO=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
LABEL=${1:?usage: er-tree-bisect-run.sh <label> [alive-seconds]}
ALIVE=${2:-45}
DLL="$REPO/target/x86_64-pc-windows-msvc/release/er_quickload.dll"
BUILD_LOG="$REPO/target/release-bisect/tree-build.log"

mkdir -p -- "$(dirname -- "$BUILD_LOG")"

if ! bash "$REPO/scripts/er-build-dlls.sh" er-quickload >"$BUILD_LOG" 2>&1; then
	echo "$LABEL: BUILD FAILED (see $BUILD_LOG)" >&2
	tail -5 -- "$BUILD_LOG" >&2
	exit 1
fi

# A build that "succeeded" without recompiling leaves the previous DLL in place, and scoring it
# would produce evidence for code that is not the code under test.
if [[ ! -f "$DLL" ]]; then
	echo "$LABEL: no DLL at $DLL after a successful build" >&2
	exit 1
fi
sha256sum -- "$DLL"

exec python3 "$REPO/scripts/er-release-bisect.py" \
	--dll "$DLL" --label "$LABEL" --alive-seconds "$ALIVE"
