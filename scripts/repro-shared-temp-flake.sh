#!/usr/bin/env bash
# Reproduce (or refute) the shared-temp-directory test flake by running N copies of one crate's
# windows test binary at once under wine.
#
# Why this exists. A test scratch path built from a fixed name under `std::env::temp_dir()` is
# the same directory in every process, because under this repo's wine runner `%TEMP%` resolves to
# the host `/tmp`. One run of `cargo test` therefore passes 100% of the time and proves nothing;
# the defect only appears when two copies overlap, which is the ordinary case here (two agents
# running `scripts/check.sh` at once, the host `cargo test` racing the wine `cargo xwin test`, or
# a second checkout). Every failure it produces accuses correct product code -- `WrongSize
# { len: 0 }`, `MissingOrNotFile`, `BridgeWriteFailed`, an identity probe answering `Unknown` --
# so without this harness the reader is sent into code that is not wrong.
#
# `scripts/check-test-temp-isolation.py` is the static gate that stops the defect being written.
# This is the empirical counterpart: it is what you run to prove the gate is describing something
# real, and what the six 2026-08-31 before/after measurements were taken with.
#
# Usage:
#   bash scripts/repro-shared-temp-flake.sh er-quit-menu-core            # 1 round of 8
#   bash scripts/repro-shared-temp-flake.sh er-save-redirect 10          # 10 rounds of 8
#   CONCURRENCY=16 bash scripts/repro-shared-temp-flake.sh er-soulsformats
#
# Exit status is the number of red runs, capped at 250 -- so `if bash ... ; then` reads as
# "the crate is isolated", and a reintroduced defect is a non-zero exit.
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
crate="${1:?usage: repro-shared-temp-flake.sh <crate> [rounds]}"
rounds="${2:-1}"
concurrency="${CONCURRENCY:-8}"
target="x86_64-pc-windows-msvc"
artifacts="${ARTIFACT_DIR:-${TMPDIR:-/tmp}/er-shared-temp-flake-$$}"
mkdir -p "$artifacts"

if ! command -v wine >/dev/null 2>&1; then
	echo "[repro-shared-temp-flake] wine not installed; nothing to reproduce" >&2
	exit 0
fi

echo "[repro-shared-temp-flake] building $crate test binary for $target"
build_log="$artifacts/build.log"
if ! cargo xwin test --lib -p "$crate" --target "$target" --no-run \
	--manifest-path "$repo_root/Cargo.toml" >"$build_log" 2>&1; then
	echo "[repro-shared-temp-flake] BUILD FAILED -- a mutant that does not compile is not a blind." >&2
	tail -30 "$build_log" >&2
	exit 250
fi

# The path cargo prints for the unit-test executable. Taken from the build output rather than
# globbed out of deps/, because a stale hash from an earlier build sits right beside the new one
# and running it would measure code that is not under test.
exe=$(sed -n 's/.*Executable unittests src\/lib\.rs (\(.*\.exe\)).*/\1/p' "$build_log" | tail -1)
if [[ -z "$exe" ]]; then
	echo "[repro-shared-temp-flake] cargo printed no unittests executable for $crate" >&2
	tail -30 "$build_log" >&2
	exit 250
fi
exe="$repo_root/$exe"
echo "[repro-shared-temp-flake] $exe"
echo "[repro-shared-temp-flake] $rounds round(s) of $concurrency concurrent copies"

red=0
total=0
for round in $(seq 1 "$rounds"); do
	pids=()
	for copy in $(seq 1 "$concurrency"); do
		(
			WINEDEBUG="${WINEDEBUG:--all}" wine "$exe" >"$artifacts/r$round-c$copy.log" 2>&1
			echo $? >"$artifacts/r$round-c$copy.rc"
		) &
		pids+=($!)
	done
	# Each copy runs the crate's own unit tests, which are pure logic and finish in well under a
	# second; `wait` on the real completion rather than a timer, per this repo's no-sleep rule.
	for pid in "${pids[@]}"; do wait "$pid"; done
	for copy in $(seq 1 "$concurrency"); do
		total=$((total + 1))
		if [[ "$(cat "$artifacts/r$round-c$copy.rc")" != "0" ]]; then
			red=$((red + 1))
		fi
	done
done

echo "[repro-shared-temp-flake] $crate: $red of $total RED"
if ((red > 0)); then
	echo "[repro-shared-temp-flake] artifacts: $artifacts"
	for rc in "$artifacts"/*.rc; do
		if [[ "$(cat "$rc")" != "0" ]]; then
			echo "--- first failing copy: ${rc%.rc}.log ---"
			grep -E "^(failures:|---- |thread |test result:)|panicked" "${rc%.rc}.log" | head -30
			break
		fi
	done
fi

((red > 250)) && red=250
exit "$red"
