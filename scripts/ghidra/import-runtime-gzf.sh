#!/usr/bin/env bash
# Import an ELDEN RING runtime-dump .gzf into a reusable, persistent Ghidra project.
#
# Replaces the hardcoded one-shot at ~/ghidra_maporch/scripts/import_persistent.sh, which pinned
# 1.16.1 paths, a /home/banon literal, and the 12.1 install that cannot read a 1.16.2-or-newer gzf.
#
# Critical, and the reason this wrapper exists at all: the gzf unpacks several GB into
# `java.io.tmpdir`. That defaults to /tmp -- a 32G tmpfs which is already about half full here.
# Plain TMPDIR is not enough: the JVM reads `java.io.tmpdir`, so it must be set through
# GHIDRA_JAVA_OPTIONS or the import dies partway with a misleading error. (An earlier agent
# mis-diagnosed that out-of-space as a `BadDataType` JPMS save failure; the log line is cosmetic.)
#
#   scripts/ghidra/import-runtime-gzf.sh --gzf ~/pc_eldenring_runtime.1.17.0.exe.gzf \
#       --proj-dir ~/ghidra_maporch/proj1170 --proj-name ermaporch1170
#
# Env overrides: GHIDRA_INSTALL_DIR, GHIDRA_TMPDIR.
set -uo pipefail

GZF=""
PROJ_DIR=""
PROJ_NAME=""
while [[ $# -gt 0 ]]; do
	case "$1" in
		--gzf) GZF="$2"; shift 2 ;;
		--proj-dir) PROJ_DIR="$2"; shift 2 ;;
		--proj-name) PROJ_NAME="$2"; shift 2 ;;
		*) echo "unknown argument: $1" >&2; exit 2 ;;
	esac
done

[[ -n "$GZF" && -n "$PROJ_DIR" && -n "$PROJ_NAME" ]] || {
	echo "usage: $0 --gzf <file.gzf> --proj-dir <dir> --proj-name <name>" >&2; exit 2; }
[[ -f "$GZF" ]] || { echo "gzf not found: $GZF" >&2; exit 2; }

# Resolve the install: env first, then bounded known locations, newest-capable first. 12.1 cannot
# read a 1.16.2+ gzf (x86 language V4.7+), so it is deliberately not a fallback -- failing loudly
# beats importing nothing and reporting success.
if [[ -z "${GHIDRA_INSTALL_DIR:-}" ]]; then
	for c in "$HOME/tools/ghidra_12.1.2_PUBLIC" /mnt/d/ghidra/ghidra_12.1.2_PUBLIC /opt/ghidra_12.1.2_PUBLIC; do
		[[ -x "$c/support/analyzeHeadless" ]] && { GHIDRA_INSTALL_DIR="$c"; break; }
	done
fi
[[ -n "${GHIDRA_INSTALL_DIR:-}" && -x "$GHIDRA_INSTALL_DIR/support/analyzeHeadless" ]] || {
	echo "no Ghidra 12.1.2 install found (set GHIDRA_INSTALL_DIR)" >&2; exit 2; }

# Keep the unpack off /tmp. Both vars: TMPDIR for child processes, java.io.tmpdir for the JVM.
GHIDRA_TMPDIR="${GHIDRA_TMPDIR:-$HOME/ghidra_maporch/tmp}"
mkdir -p "$GHIDRA_TMPDIR" "$PROJ_DIR"
export TMPDIR="$GHIDRA_TMPDIR"
export GHIDRA_JAVA_OPTIONS="-Djava.io.tmpdir=$GHIDRA_TMPDIR"

# Refuse rather than half-import. The 1.16.2 project is 26G from a 3.6G gzf (~7x), so require
# 8x the gzf free on the project filesystem. A truncated project looks importable and then
# answers queries wrongly, which is the failure mode this whole migration exists to avoid.
gzf_bytes=$(stat -c %s "$GZF")
need_kb=$(( gzf_bytes / 1024 * 8 ))
free_kb=$(df -Pk "$PROJ_DIR" | awk 'NR==2 {print $4}')
if (( free_kb < need_kb )); then
	echo "REFUSED: need ~$(( need_kb / 1024 / 1024 ))G free for $PROJ_DIR, have $(( free_kb / 1024 / 1024 ))G" >&2
	exit 3
fi

echo "== importing $(basename "$GZF") -> $PROJ_DIR/$PROJ_NAME =="
echo "== ghidra: $GHIDRA_INSTALL_DIR   tmpdir: $GHIDRA_TMPDIR =="
echo "== free before: $(( free_kb / 1024 / 1024 ))G   gzf: $(( gzf_bytes / 1024 / 1024 ))M =="

# -noanalysis is correct and not a shortcut: a .gzf is an exported analyzed program, so its
# functions, symbols, types and RTTI travel with it. Re-analysing would burn hours to rediscover
# what the file already carries, and would overwrite the source project's curated names.
"$GHIDRA_INSTALL_DIR/support/analyzeHeadless" "$PROJ_DIR" "$PROJ_NAME" \
	-import "$GZF" \
	-noanalysis \
	-overwrite
rc=$?
echo "IMPORT_EXIT=$rc"
[[ $rc -eq 0 ]] && echo "== project: $PROJ_DIR/$PROJ_NAME.gpr =="
exit $rc
