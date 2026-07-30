#!/usr/bin/env bash
# Prove a refactor changes the SHIPPING DLL by exactly nothing.
#
# Why this exists: the save-picker crate-extraction epic (docs/plans/save-picker-crate-extraction.md)
# lands in slices whose headline claim is "the product behaves byte-for-byte as before". A file list
# showing no `crates/er-effects-rs/src/` change is INDIRECT evidence -- a workspace member, a feature
# unification, or a Cargo.lock bump can move the shipping bytes without touching a product source
# file. This does the direct measurement: build the DLL at both refs and compare the bytes.
#
# The builds MUST share one directory. rustc embeds absolute source paths in the binary, so building
# two refs in two worktrees yields different bytes for identical source and the comparison is
# worthless. This checks out both refs into the SAME tree, in sequence, and restores the starting ref.
#
#   bash scripts/verify-dll-unchanged.sh <baseline-ref> <candidate-ref> [worktree-dir]
#   bash scripts/verify-dll-unchanged.sh origin/main refactor/save-picker-crates
#
# Exit: 0 identical, 1 differs, 2 usage/build failure.
set -uo pipefail

BASELINE="${1:-}"
CANDIDATE="${2:-}"
TREE="${3:-$(git rev-parse --show-toplevel 2>/dev/null)}"

if [ -z "$BASELINE" ] || [ -z "$CANDIDATE" ]; then
	echo "usage: $0 <baseline-ref> <candidate-ref> [worktree-dir]" >&2
	exit 2
fi
if [ ! -d "$TREE" ]; then
	echo "verify-dll-unchanged: not a directory: $TREE" >&2
	exit 2
fi
cd "$TREE" || exit 2

TARGET=x86_64-pc-windows-msvc
DLL="$TREE/target/$TARGET/release/er_effects_rs.dll"

dirty=$(git status --porcelain | wc -l)
if [ "$dirty" -ne 0 ]; then
	echo "verify-dll-unchanged: $TREE has $dirty uncommitted change(s); refusing to check out over them" >&2
	git status --porcelain >&2
	exit 2
fi

START_REF=$(git symbolic-ref --quiet --short HEAD || git rev-parse HEAD)
restore() { git checkout --quiet "$START_REF" 2>/dev/null; }
trap restore EXIT

build_at() { # <ref> -> echoes "<md5> <size>"
	local ref="$1"
	git checkout --quiet --detach "$ref" || return 1
	rm -f "$DLL"
	cargo xwin build --release --target "$TARGET" >/tmp/verify-dll-unchanged.build.log 2>&1 || {
		echo "BUILD FAILED at $ref; tail of log:" >&2
		tail -20 /tmp/verify-dll-unchanged.build.log >&2
		return 1
	}
	[ -f "$DLL" ] || { echo "no DLL produced at $ref" >&2; return 1; }
	echo "$(md5sum "$DLL" | cut -d' ' -f1) $(stat -c%s "$DLL")"
}

echo "verify-dll-unchanged: tree=$TREE"
echo "verify-dll-unchanged: baseline=$BASELINE ($(git rev-parse --short "$BASELINE"))"
echo "verify-dll-unchanged: candidate=$CANDIDATE ($(git rev-parse --short "$CANDIDATE"))"

echo "[1/2] building baseline $BASELINE ..."
read -r BASE_MD5 BASE_SIZE < <(build_at "$BASELINE") || exit 2
echo "      baseline  md5=$BASE_MD5 size=$BASE_SIZE"

echo "[2/2] building candidate $CANDIDATE ..."
read -r CAND_MD5 CAND_SIZE < <(build_at "$CANDIDATE") || exit 2
echo "      candidate md5=$CAND_MD5 size=$CAND_SIZE"

echo
if [ "$BASE_MD5" = "$CAND_MD5" ]; then
	echo "IDENTICAL -- $CANDIDATE does not change the shipping DLL (md5=$BASE_MD5, $BASE_SIZE bytes)"
	exit 0
fi
echo "DIFFERENT -- shipping DLL moved:"
echo "    $BASELINE  md5=$BASE_MD5 size=$BASE_SIZE"
echo "    $CANDIDATE md5=$CAND_MD5 size=$CAND_SIZE"
exit 1
