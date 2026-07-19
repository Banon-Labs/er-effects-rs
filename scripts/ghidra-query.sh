#!/usr/bin/env bash
# Query the PERSISTENT pre-analyzed ER runtime Ghidra project WITHOUT re-importing the
# ~1.5GB gzf every time. Runs analyzeHeadless in -process mode against the saved program.
#
#   scripts/ghidra-query.sh <postScript.java> [scriptArg ...]
#
# The .java GhidraScript may live anywhere; its directory is added to -scriptPath.
# Defaults are current-user/home-aware and can be overridden:
#   GHIDRA_PROJ_DIR=/path/to/proj
#   GHIDRA_PROJ_NAME=ermaporch
#   GHIDRA_TMPDIR=/path/to/tmp
#   GHIDRA_SCRIPT_CACHE=/path/to/ghidra_scripts_cache
#   GHIDRA_HEADLESS=/path/to/analyzeHeadless
#   GHIDRA_INSTALL_DIR=/path/to/ghidra_12.1_PUBLIC
#
# Env gotchas baked in:
#   - java.io.tmpdir is forced away from /tmp (the /tmp tmpfs can overflow).
#   - project dir should be dotless (Ghidra rejects dot-prefixed project dirs).
set -euo pipefail

usage() {
	echo "Usage: scripts/ghidra-query.sh <postScript.java> [scriptArg ...]" >&2
}

first_existing_executable() {
	local candidate
	for candidate in "$@"; do
		if [[ -n "$candidate" && -x "$candidate" ]]; then
			printf '%s\n' "$candidate"
			return 0
		fi
	done
	return 1
}

resolve_headless() {
	if [[ -n "${GHIDRA_HEADLESS:-}" ]]; then
		if [[ -x "$GHIDRA_HEADLESS" ]]; then
			printf '%s\n' "$GHIDRA_HEADLESS"
			return 0
		fi
		echo "GHIDRA_HEADLESS is set but not executable: $GHIDRA_HEADLESS" >&2
		return 1
	fi

	local install_headless=""
	if [[ -n "${GHIDRA_INSTALL_DIR:-}" ]]; then
		install_headless="$GHIDRA_INSTALL_DIR/support/analyzeHeadless"
	fi

	local command_headless=""
	command_headless="$(command -v analyzeHeadless 2>/dev/null || true)"

	local globbed=()
	shopt -s nullglob
	globbed+=("$HOME"/tools/ghidra*/support/analyzeHeadless)
	globbed+=(/mnt/d/ghidra/ghidra*/support/analyzeHeadless)
	globbed+=(/opt/ghidra*/support/analyzeHeadless)
	shopt -u nullglob

	first_existing_executable \
		"$install_headless" \
		"$HOME/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless" \
		"/mnt/d/ghidra/ghidra_12.1_PUBLIC/support/analyzeHeadless" \
		"/home/banon/tools/ghidra_12.1_PUBLIC/support/analyzeHeadless" \
		"$command_headless" \
		"${globbed[@]}"
}

if [[ $# -lt 1 ]]; then
	usage
	exit 2
fi

SCRIPT_FILE="$1"
shift
if [[ ! -f "$SCRIPT_FILE" ]]; then
	echo "postScript not found: $SCRIPT_FILE" >&2
	exit 2
fi

PROJ_NAME="${GHIDRA_PROJ_NAME:-ermaporch}"
if [[ -n "${GHIDRA_PROJ_DIR:-}" ]]; then
	PROJ_DIR="$GHIDRA_PROJ_DIR"
elif [[ -d "$HOME/ghidra_maporch/proj" ]]; then
	PROJ_DIR="$HOME/ghidra_maporch/proj"
elif [[ -d "/home/banon/ghidra_maporch/proj" ]]; then
	PROJ_DIR="/home/banon/ghidra_maporch/proj"
else
	echo "Ghidra persistent project not found. Set GHIDRA_PROJ_DIR=/path/to/proj." >&2
	exit 2
fi

if [[ ! -d "$PROJ_DIR" ]]; then
	echo "GHIDRA_PROJ_DIR does not exist: $PROJ_DIR" >&2
	exit 2
fi

TMP="${GHIDRA_TMPDIR:-$HOME/ghidra_maporch/tmp}"
HEADLESS="$(resolve_headless)" || {
	echo "Ghidra analyzeHeadless not found. Set GHIDRA_HEADLESS or GHIDRA_INSTALL_DIR." >&2
	exit 2
}

SCRIPT_SOURCE_DIR="$(cd "$(dirname "$SCRIPT_FILE")" && pwd)"
SCRIPT_NAME="$(basename "$SCRIPT_FILE")"
SCRIPT_CACHE="${GHIDRA_SCRIPT_CACHE:-$HOME/ghidra_maporch/gscripts}"

mkdir -p "$TMP" "$SCRIPT_CACHE"
# Ghidra's JavaScriptProvider can fail to form an OSGi bundle for arbitrary repo paths on some
# WSL/Windows-mounted installs. Keep scripts versioned in the repo, but execute a fresh copy from
# the current user's stable Ghidra script cache.
if [[ "$SCRIPT_SOURCE_DIR" != "$SCRIPT_CACHE" ]]; then
	cp -f "$SCRIPT_FILE" "$SCRIPT_CACHE/$SCRIPT_NAME"
fi

export TMPDIR="$TMP"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$TMP"

# -process (no -import) reopens the SAVED program. -noanalysis: it's already analyzed.
exec "$HEADLESS" "$PROJ_DIR" "$PROJ_NAME" \
	-process \
	-noanalysis \
	-readOnly \
	-scriptPath "$SCRIPT_CACHE" \
	-postScript "$SCRIPT_NAME" "$@"
