#!/usr/bin/env bash
# "Is this DLL the code I am about to test?" -- the launch-time half of er-dll-provenance.py.
#
# Why `[[ -f "$DLL" ]]` was never a gate
# --------------------------------------
# Every .me3 profile in this repo points its `[[natives]]` entries straight at
# target/<triple>/release/*.dll. There is no staging copy anywhere, so "staging" is "building",
# and the only thing between a run and week-old code is a check made at launch time. Existence
# is not that check, because the stale DLL from last week exists. `cargo xwin build --release`
# honours `default-members = ["crates/er-quickload"]`, so it exits 0 in a fraction of a second
# having compiled none of the other shells -- and a run against what it left behind produces
# evidence for code that is not in the tree, which is indistinguishable from the feature not
# working. That is worse than not running at all, so this refuses.
#
# Why not MTIME / the PE TIMESTAMP
# --------------------------------
# Because cargo is right to skip relinking a crate whose forward dependency closure has not
# changed: a DLL two days older than its siblings can be perfectly current. A timestamp
# comparison therefore manufactures false staleness, and a gate that cries wolf is a gate people
# learn to route around -- the exact failure this exists to prevent. The sound test is a content
# hash over the compiled closure plus a build-noise-masked PE fingerprint, which is what
# scripts/er-dll-provenance.py records at build time and re-derives here.
#
# Usage -- source it, like scripts/steam-running.sh:
#     source "$REPO_ROOT/scripts/er-dll-freshness.sh"
#     require_fresh_dlls "$PRODUCT_DLL" "$HARNESS_DLL" || exit 3
#
# Callers pass paths. The cargo package each artifact belongs to is resolved from
# scripts/me3-dll-list.py -- the single source of truth for which cdylibs this workspace ships --
# so no caller keeps a second copy of the package->filename map (four crates override [lib] name,
# so that map cannot be derived by swapping dashes for underscores). An artifact this workspace
# does not build is a refusal rather than a skip: guessing which crate a stray DLL came from is
# how an unchecked binary gets into a run.
#
# There is deliberately no bypass flag. The only way past this is to rebuild:
#     scripts/er-build-dlls.sh <package>...
#
# Self-test, both directions, no game and no cargo:
#     bash scripts/er-dll-freshness.sh --selftest

_ERDF_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
declare -A _ERDF_PACKAGE_OF=()
_ERDF_MAP_LOADED=0

_erdf_load_map() {
	[[ "$_ERDF_MAP_LOADED" == 1 ]] && return 0
	local pkg artifact
	while IFS=: read -r pkg artifact; do
		[[ -n "$pkg" && -n "$artifact" ]] || continue
		_ERDF_PACKAGE_OF["$artifact.dll"]="$pkg"
	done < <(python3 "$_ERDF_ROOT/scripts/me3-dll-list.py" --pairs)
	if [[ ${#_ERDF_PACKAGE_OF[@]} -eq 0 ]]; then
		echo "er-dll-freshness: scripts/me3-dll-list.py listed no shells; cannot map artifacts to packages" >&2
		return 1
	fi
	_ERDF_MAP_LOADED=1
}

# er_dll_package_for <path-or-filename> -> prints the cargo package, or returns 1 when this
# workspace does not build that artifact.
er_dll_package_for() {
	_erdf_load_map || return 1
	local base="${1##*/}"
	local pkg="${_ERDF_PACKAGE_OF[$base]:-}"
	[[ -n "$pkg" ]] || return 1
	printf '%s\n' "$pkg"
}

# require_fresh_dlls <path>... -> 0 only when every artifact verifies against this tree.
# On any failure it prints one loud block naming every offender and the exact rebuild command,
# then returns 3 (matching er-dll-provenance.py's "stale is a verdict, not an error" status).
require_fresh_dlls() {
	_erdf_load_map || return 1
	local dll base pkg out rc seen seen_pkg
	local -a stale=() packages=()
	for dll in "$@"; do
		base="${dll##*/}"
		pkg="${_ERDF_PACKAGE_OF[$base]:-}"
		if [[ -z "$pkg" ]]; then
			stale+=("UNKNOWN  $base
  This workspace does not build an artifact by that name (scripts/me3-dll-list.py --pairs).
  Refusing rather than guessing which crate it came from.")
			continue
		fi
		out="$(python3 "$_ERDF_ROOT/scripts/er-dll-provenance.py" verify --package "$pkg" --artifact "$dll" 2>&1)"
		rc=$?
		if [[ "$rc" != 0 ]]; then
			stale+=("$out")
			seen=0
			for seen_pkg in ${packages[@]+"${packages[@]}"}; do
				[[ "$seen_pkg" == "$pkg" ]] && seen=1
			done
			[[ "$seen" == 0 ]] && packages+=("$pkg")
		fi
	done

	[[ ${#stale[@]} -eq 0 ]] && return 0

	{
		echo "======================================================================"
		echo "== REFUSING TO PROCEED -- the DLLs on disk are not this source tree  =="
		echo "======================================================================"
		printf '%s\n' "${stale[@]}"
		if [[ ${#packages[@]} -gt 0 ]]; then
			echo
			echo "Rebuild them (this records the provenance as it builds):"
			echo "  scripts/er-build-dlls.sh ${packages[*]}"
		fi
		echo
		echo "A run against these would produce evidence for code that is not in the tree --"
		echo "a result indistinguishable from the feature not working."
		echo "======================================================================"
	} >&2
	return 3
}

# ---------------------------------------------------------------------------------------------
# Executed directly: self-test. Exercises both verdicts against copies in a temp dir, so it needs
# neither a game nor cargo, and never touches the real target/ sidecars. A gate nobody has
# watched refuse is not a gate.
# ---------------------------------------------------------------------------------------------
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
	[[ "${1:-}" == "--selftest" ]] || {
		sed -n '2,39p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
		exit 0
	}

	_erdf_ok=1
	# Assertions are functions so their status is a command status. Chaining bare `[[ ]]` and
	# then reading `$?` is shellcheck SC2319, and it is a real trap rather than pedantry: `$?`
	# silently becomes the status of the last condition instead of the whole assertion.
	_erdf_is() { [[ "$1" == "$2" ]]; }
	_erdf_says() { [[ "$1" == *"$2"* ]]; }
	_erdf_silent_about() { [[ "$1" != *"$2"* ]]; }
	_erdf_check() {
		if [[ "$1" == 0 ]]; then
			echo "  ok   $2"
		else
			echo "  FAIL $2"
			_erdf_ok=0
		fi
	}

	# er-crash-logging has the smallest forward closure of the shipped shells, so its source
	# hash is the cheapest to recompute; any built DLL would do. A real one is required, and a
	# placeholder file was tried and does not work: `er-dll-provenance.py write` fingerprints the
	# artifact's code through `dll-code-fingerprint.py`, which parses the PE header and raises on
	# anything that is not one.
	#
	# So the order matters, and check.sh now runs this after `check-rust-build.sh` links the 26
	# shells rather than ~900 lines before it. Measured 2026-09-01: in a fresh agent worktree --
	# empty target/, nothing built yet -- this exited 1 with "no built DLL" while the change under
	# test was fine, red at check.sh line 1643 for want of an artifact produced at line 1660. A
	# gate that goes red on a clean checkout for a reason unrelated to the change is one people
	# learn to read past.
	_erdf_src="$_ERDF_ROOT/target/x86_64-pc-windows-msvc/release/er_crash_logging.dll"
	if [[ ! -f "$_erdf_src" ]]; then
		echo "er-dll-freshness --selftest: no built DLL at $_erdf_src"
		echo "  It needs a real PE: the provenance record it exercises fingerprints the code"
		echo "  section, so a placeholder file fails to parse rather than standing in."
		echo "  Build one first: scripts/er-build-dlls.sh er-crash-logging"
		exit 1
	fi

	_erdf_tmp="$(mktemp -d)"
	# shellcheck disable=SC2064  # expand _erdf_tmp now, on purpose
	trap "rm -rf '$_erdf_tmp'" EXIT
	_erdf_copy="$_erdf_tmp/er_crash_logging.dll"
	cp -f "$_erdf_src" "$_erdf_copy"

	# 1. An artifact this workspace does not build.
	cp -f "$_erdf_src" "$_erdf_tmp/not_ours.dll"
	require_fresh_dlls "$_erdf_tmp/not_ours.dll" >/dev/null 2>&1
	_erdf_rc=$?
	_erdf_is "$_erdf_rc" 3
	_erdf_check "$?" "an artifact this workspace does not build is refused, never skipped"

	# 2. A real artifact with no provenance sidecar beside it.
	require_fresh_dlls "$_erdf_copy" >/dev/null 2>&1
	_erdf_rc=$?
	_erdf_is "$_erdf_rc" 3
	_erdf_check "$?" "a DLL with no provenance record REFUSES"

	# 3. The same artifact, once its provenance is recorded against this tree.
	python3 "$_ERDF_ROOT/scripts/er-dll-provenance.py" write \
		--package er-crash-logging --artifact "$_erdf_copy" >/dev/null
	require_fresh_dlls "$_erdf_copy" >/dev/null 2>&1
	_erdf_check "$?" "the same DLL PROCEEDS once its provenance matches the tree"

	# 4. Staleness forged through the provenance mechanism itself: move the recorded source hash
	#    off the tree. Deleting the DLL would only re-prove the existence check being replaced.
	python3 -c 'import json,sys
p = sys.argv[1]
r = json.load(open(p, encoding="utf-8"))
r["source_sha"] = "0" * 64
json.dump(r, open(p, "w", encoding="utf-8"))' "$_erdf_copy.provenance.json"
	_erdf_out="$(require_fresh_dlls "$_erdf_copy" 2>&1)"
	_erdf_rc=$?
	_erdf_is "$_erdf_rc" 3 &&
		_erdf_says "$_erdf_out" "SOURCE MOVED" &&
		_erdf_says "$_erdf_out" "er-build-dlls.sh er-crash-logging"
	_erdf_check "$?" "a recorded source hash that no longer matches the tree REFUSES, naming the rebuild"

	# 5/6. One stale DLL among fresh ones still stops everything and names only the offender --
	#      and the all-fresh set then proceeds, so 5 was about staleness, not about arity.
	_erdf_telem="$_ERDF_ROOT/target/x86_64-pc-windows-msvc/release/er_telemetry.dll"
	if [[ -f "$_erdf_telem" ]]; then
		cp -f "$_erdf_telem" "$_erdf_tmp/er_telemetry.dll"
		python3 "$_ERDF_ROOT/scripts/er-dll-provenance.py" write \
			--package er-telemetry --artifact "$_erdf_tmp/er_telemetry.dll" >/dev/null

		_erdf_out="$(require_fresh_dlls "$_erdf_tmp/er_telemetry.dll" "$_erdf_copy" 2>&1)"
		_erdf_rc=$?
		_erdf_is "$_erdf_rc" 3 &&
			_erdf_says "$_erdf_out" "er_crash_logging.dll" &&
			_erdf_silent_about "$_erdf_out" "er_telemetry.dll  (er-telemetry)"
		_erdf_check "$?" "a stale DLL among fresh ones is caught, and only it is named"

		python3 "$_ERDF_ROOT/scripts/er-dll-provenance.py" write \
			--package er-crash-logging --artifact "$_erdf_copy" >/dev/null
		require_fresh_dlls "$_erdf_tmp/er_telemetry.dll" "$_erdf_copy" >/dev/null 2>&1
		_erdf_check "$?" "an all-fresh set of several DLLs PROCEEDS"
	fi

	# 7. The package map is the shipped-shell list, not a dash/underscore guess.
	_erdf_is "$(er_dll_package_for /anywhere/mushroom_man.dll)" "mushroom-man-runtime"
	_erdf_check "$?" "the package map handles a crate whose [lib] name differs from its package"

	if [[ "$_erdf_ok" == 1 ]]; then
		echo "selftest: PASS"
		exit 0
	fi
	echo "selftest: FAIL"
	exit 1
fi
