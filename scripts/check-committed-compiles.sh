#!/usr/bin/env bash
# DOES THE COMMITTED STATE COMPILE?  -- not "does my working tree compile".
#
# WHY THIS IS A DIFFERENT QUESTION, and why every other gate in this repo answers the wrong one.
# Agents here commit with explicit pathspecs (correctly -- several of them share this checkout and
# a bare `git commit -a` would sweep up each other's work). A pathspec is also the exact mechanism
# by which a CONSUMER gets committed without its PRODUCER: the new caller is named on the command
# line, the new function/crate it calls is not. The author's working tree still holds the producer,
# so it compiles for them, and every gate that builds the working tree agrees with them. The
# pushed commit does not compile for anybody else.
#
# Measured on this branch, 2026-08-31 -- two commits, hours apart, both this shape:
#   15b32ab0  crates/er-save-suppress/src/save_state_witness.rs referencing
#             er_game_base::mem::game_rva_for_hook, er_game_base::rva::SL_LOAD_POLL_WRAPPER_RVA
#             and SL_SAVE_LANE_WRAPPER_RVA -- none of which existed in that commit's er-game-base.
#   11af0c60  boot_progress.rs referencing the er_boot_background and er_cover_fade crates, which
#             were not in the tree at all (not even as unlisted directories).
# a210af7f then landed an 18-file compile closure and made the branch green again. In between,
# `origin` did not compile either, and a dozen agents built on top of it.
#
# HOW IT ANSWERS THE RIGHT QUESTION: it type-checks a git WORKTREE pinned to the commit under
# test, so the only files in scope are the ones actually in that commit. An uncommitted producer
# sitting in the author's checkout is invisible to it, which is the whole point.
#
# WHAT IT COMPILES, and why every word of that invocation is load-bearing:
#   cargo xwin clippy --workspace --all-targets --keep-going --target x86_64-pc-windows-msvc
#   * --workspace, because the workspace sets `default-members = ["crates/er-quickload"]`. A bare
#     `cargo xwin check`/`build` selects that one package, exits 0 in a fraction of a second having
#     compiled nothing else, and reads exactly like a successful incremental build. That bare form
#     is what the pre-push path used to end with, and it is decorative for this purpose:
#     er-save-suppress is a workspace member that nothing in default-members reaches.
#   * --all-targets, so `#[cfg(test)]` modules, benches and examples are compiled too. A lib-only
#     check reports OK over a test module that names a helper the commit does not carry -- the
#     same defect one layer down. MEASURED 2026-08-31 on a cold cache: 100 s with --all-targets
#     against 105 s without, and 3.1 GB against 2.6 GB. The dev-dependency graph is almost
#     entirely shared with the normal one, so the wider check is free.
#   * the windows target, because most crates here are `#![cfg(windows)]` -- a host `cargo check`
#     compiles them to an empty crate and then reports OK over nothing. And a host check is not a
#     substitute for a different reason too: `cargo check --workspace --all-targets` on the HOST
#     fails at a green HEAD (er-invasion-path, windows-future), because this workspace is not
#     meant to build for Linux as a whole. Measured the same day. Do not "fix" that by adding it.
#   * --keep-going, so one broken crate does not hide the state of the rest. Without it the run
#     stops at er-save-suppress and never reaches er-quickload, so the report understates the
#     damage.
#   * clippy RATHER THAN check, since 2026-09-03. `cargo clippy` runs everything `cargo check`
#     runs and then applies the lints, so nothing is lost and it is one pass, not two -- the only
#     cost is a one-time cache rebuild, because clippy's fingerprints differ from check's.
#
#     WHY IT HAD TO CHANGE: `clippy` appeared ZERO times across the entire pre-push path --
#     scripts/hooks/pre-push, the pre-push gate suite and this file -- while the workspace root
#     sets `[workspace.lints.clippy]` to deny. So every lint denial in this repo was enforced
#     exclusively by CI, and `cargo check` is happy with code `cargo clippy` rejects. Measured on
#     PR #388, run 33793058851: `clippy::manual_is_multiple_of` at
#     crates/er-npc-possess/src/possess/fall.rs:206 reached origin, went red in CI, and no local
#     gate had anything to say about it -- not because anyone bypassed the hook, but because the
#     hook ran and did not ask the question. rustc was 1.98.0 on both sides; this was never a
#     toolchain skew.
#
# NO BYPASS. There is no --force, no skip flag and no environment escape, by design: the other
# gates in this repo have none either, and a compile gate that can be waved through is the gate
# that was not running in the first place.
#
# Usage:
#   scripts/check-committed-compiles.sh [<rev>...]        # default: HEAD
#   scripts/check-committed-compiles.sh --selftest        # prove the gate can go red
#
# Env:
#   ER_COMMITTED_CHECK_WORKTREE   worktree path   (default <repo>/.worktrees/committed-compiles)
#   ER_COMMITTED_CHECK_TARGET_DIR CARGO_TARGET_DIR (default <repo>/target/committed-compiles)
# Both default inside gitignored directories. The worktree and target dir are REUSED between runs
# on purpose: cargo fingerprints include the absolute source path, so a fresh directory per run is
# a cold build every time (~10 min here) while a stable one is incremental (~seconds when the
# commit under test is close to the last one checked). Concurrent runs serialise on a flock rather
# than racing each other's checkout.
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

# THE HEAVIEST THING IN THE PUSH PATH, AND IT WAS THE ONE STEP STILL RUNNING AT THE PRIORITY IT
# INHERITED. This gate cross-compiles the WHOLE workspace with --all-targets on a cold cache
# (~100 s, 3.1 GB) and the pre-push hook runs it BEFORE check.sh -- so on 2026-09-06, with
# check.sh already yielding, the desktop still froze for the first two minutes of every push.
# Yield here too, from inside, so no caller has to remember.
# shellcheck source=lib/cpu-courtesy.sh
. "$repo_root/scripts/lib/cpu-courtesy.sh"
cpu_courtesy check-committed-compiles
target="x86_64-pc-windows-msvc"
worktree="${ER_COMMITTED_CHECK_WORKTREE:-$repo_root/.worktrees/committed-compiles}"
target_dir="${ER_COMMITTED_CHECK_TARGET_DIR:-$repo_root/target/committed-compiles}"

# Git hooks run with repository-local GIT_* variables exported for the caller checkout. If those
# leak into `git -C "$worktree" checkout`, Git ignores the -C worktree and checks out the caller
# instead. That is exactly how this gate stranded a push attempt at its historical red selftest
# commit 11af0c60. Keep the path-derived repo_root above, then clear every local Git variable
# before touching the scratch worktree.
_git_local_env_seen=0
while IFS= read -r _git_env_name; do
	if [[ -n $_git_env_name && -v $_git_env_name ]]; then
		_git_local_env_seen=1
		unset "$_git_env_name"
	fi
done < <(git rev-parse --local-env-vars)
unset _git_env_name

selftest=0
git_env_selftest_child=0
revs=()
for arg in "$@"; do
	case "$arg" in
		--selftest) selftest=1 ;;
		--selftest-git-env-child) git_env_selftest_child=1 ;;
		-*) echo "[committed-compiles] unknown flag: $arg" >&2; exit 2 ;;
		*) revs+=("$arg") ;;
	esac
done
[[ ${#revs[@]} -eq 0 ]] && revs=(HEAD)

# --- serialise -------------------------------------------------------------------------------
mkdir -p "$(dirname -- "$worktree")" "$target_dir"
exec 9>"$target_dir/.gate.lock"
flock 9

# --- the sibling checkout and the vendored C the workspace cannot load without -----------------
# The root crate uses `../fromsoftware-rs` PATH dependencies, resolved relative to the manifest.
# From <repo>/.worktrees/committed-compiles that is <repo>/.worktrees/fromsoftware-rs, which does
# not exist -- so without this link cargo cannot even parse the workspace, and the gate would fail
# for a reason that has nothing to do with the commit under test.
link_sibling() {
	local real="$repo_root/../fromsoftware-rs" link
	link="$(dirname -- "$worktree")/fromsoftware-rs"
	if [[ ! -d "$real" ]]; then
		echo "[committed-compiles] FAIL: no sibling checkout at $real" >&2
		echo "  the workspace uses ../fromsoftware-rs path dependencies and cannot be loaded without it" >&2
		exit 2
	fi
	# Only ever replace a symlink of our own making; never touch a real directory.
	#
	# `pwd -P`, NOT `pwd`. bash's logical pwd echoes back the path you arrived by, symlinks and
	# all -- so when `$real` and `$link` name the SAME path, `ln -sfn` points the link at itself
	# and every later read of it dies with ELOOP ("Too many levels of symbolic links"), which
	# cargo reports as `failed to load manifest for dependency eldenring`. That collision is not
	# hypothetical: it is what happens whenever the INVOKING checkout is itself a worktree under
	# `<repo>/.worktrees/` and ER_COMMITTED_CHECK_WORKTREE points back at the family's shared
	# scratch dir -- then `$repo_root/../fromsoftware-rs` and `$(dirname $worktree)/fromsoftware-rs`
	# are both `<repo>/.worktrees/fromsoftware-rs`. Measured 2026-09-03; it also POISONS the link
	# for every later run, including the main checkout's, because the damage is on disk.
	# `pwd -P` resolves to the real sibling and can never name the link.
	if [[ -L "$link" || ! -e "$link" ]]; then
		ln -sfn "$(cd -- "$real" && pwd -P)" "$link"
	fi
}

link_vendor() {
	# vendor/ is gitignored, so a worktree starts without MinHook while the checkout it was made
	# from already has it. Same self-heal scripts/check.sh does, for the same reason.
	#
	# Two places are searched, and the second is not optional. `$repo_root` is whichever checkout
	# invoked this script -- and when that is itself an agent worktree (`.claude/worktrees/...`,
	# `.worktrees/...`), it is gitignored-empty too, so looking only there fails the gate with
	# "clone MinHook" for a tree that has a perfectly good copy one directory up. The MAIN
	# checkout is found through `--git-common-dir`, which every linked worktree shares: its
	# parent is the checkout the whole worktree family was made from.
	if [[ -f "$worktree/vendor/minhook/src/buffer.c" ]]; then
		return
	fi
	local common main_checkout source=""
	common="$(git -C "$repo_root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)"
	main_checkout="${common:+$(dirname -- "$common")}"
	for candidate in "$repo_root" "$main_checkout"; do
		if [[ -n "$candidate" && -f "$candidate/vendor/minhook/src/buffer.c" ]]; then
			source="$candidate/vendor/minhook"
			break
		fi
	done
	if [[ -z "$source" ]]; then
		echo "[committed-compiles] FAIL: no vendor/minhook in $repo_root${main_checkout:+ or $main_checkout}" >&2
		echo "  git clone --depth 1 --branch v1.3.4 https://github.com/TsudaKageyu/minhook.git vendor/minhook" >&2
		exit 2
	fi
	mkdir -p "$worktree/vendor"
	ln -sfn "$source" "$worktree/vendor/minhook"
}

# --- pin the worktree to one commit -----------------------------------------------------------
pin_worktree() {
	local sha=$1
	if [[ -d "$worktree/.git" || -f "$worktree/.git" ]]; then
		# `--force` twice: the first lets the detached checkout move even when the previous run
		# left the tree dirty, the second lets it discard an untracked file that a tracked file
		# in the target commit wants to occupy.
		git -C "$worktree" checkout --detach --force --force "$sha" >/dev/null 2>&1 ||
			{ git worktree remove --force "$worktree" >/dev/null 2>&1 || rm -rf -- "$worktree"
			  git -C "$repo_root" worktree add --detach --force "$worktree" "$sha" >/dev/null; }
	else
		rm -rf -- "$worktree"
		git -C "$repo_root" worktree prune >/dev/null 2>&1 || true
		git -C "$repo_root" worktree add --detach --force "$worktree" "$sha" >/dev/null
	fi
	# Remove leftovers from the previous commit under test so a deleted file cannot linger and
	# make a broken commit look whole. -x because the interesting leftovers (a stray crate
	# directory, a generated module) are exactly the gitignored/untracked ones. CARGO_TARGET_DIR
	# lives OUTSIDE the worktree, so this never touches the build cache.
	git -C "$worktree" clean -qxfd
	link_vendor
}

run_one() {
	local rev=$1 sha subject status
	sha=$(git -C "$repo_root" rev-parse --verify "$rev^{commit}")
	subject=$(git -C "$repo_root" log -1 --format=%s "$sha")
	echo "[committed-compiles] $rev -> ${sha:0:8}  $subject"
	pin_worktree "$sha"
	echo "[committed-compiles] cargo xwin clippy --workspace --all-targets --keep-going --target $target"
	status=0
	( cd "$worktree" && CARGO_TARGET_DIR="$target_dir" \
		cargo xwin clippy --workspace --all-targets --keep-going --target "$target" ) || status=$?
	if [[ "$status" != 0 ]]; then
		echo "[committed-compiles] FAIL: commit ${sha:0:8} does not compile or fails clippy (exit $status)" >&2
		return 1
	fi
	echo "[committed-compiles] ok: ${sha:0:8}"
	return 0
}

# --- a deliberately broken commit, built without touching any working tree -------------------
# Used by the selftest when the historical failures below are no longer reachable (a squash-merge,
# a shallow clone). It appends a call to a symbol that does not exist onto a LEAF crate -- the same
# error class as the real failures, E0433/E0425 -- and assembles the commit with plumbing against a
# temporary index, so the main checkout, its index and its five agents' uncommitted work are never
# touched. The result is a dangling commit object; `git gc` reaps it.
synth_broken_rev() {
	local base=$1 tmp_index src_path blob tree orig
	src_path=crates/er-enemynpc-effects/src/lib.rs
	tmp_index=$(mktemp "${TMPDIR:-/tmp}/er-committed-compiles-index.XXXXXX")
	rm -f -- "$tmp_index"
	GIT_INDEX_FILE="$tmp_index" git -C "$repo_root" read-tree "$base^{tree}"
	orig=$(git -C "$repo_root" show "$base:$src_path")
	blob=$(printf '%s\npub fn __committed_compiles_selftest() { er_game_base::mem::__no_such_symbol(); }\n' \
		"$orig" | git -C "$repo_root" hash-object -w --stdin)
	GIT_INDEX_FILE="$tmp_index" git -C "$repo_root" update-index --add --cacheinfo "100644,$blob,$src_path"
	tree=$(GIT_INDEX_FILE="$tmp_index" git -C "$repo_root" write-tree)
	rm -f -- "$tmp_index"
	git -C "$repo_root" commit-tree "$tree" -p "$base" -m 'committed-compiles selftest: deliberately broken'
}

git_env_child_selftest() {
	if [[ "$_git_local_env_seen" != 1 ]]; then
		echo "[committed-compiles] SELFTEST FAIL: child saw no local Git env to clear" >&2
		exit 1
	fi
	git -C "$repo_root" worktree remove --force "$worktree" >/dev/null 2>&1 || rm -rf -- "$worktree"
	git -C "$repo_root" worktree add --detach --force "$worktree" HEAD >/dev/null
	trap 'git -C "$repo_root" worktree remove --force "$worktree" >/dev/null 2>&1 || rm -rf -- "$worktree"' EXIT
	local top expected
	top=$(git -C "$worktree" rev-parse --show-toplevel)
	expected=$(cd -- "$worktree" && pwd -P)
	if [[ "$top" != "$expected" ]]; then
		echo "[committed-compiles] SELFTEST FAIL: local Git env leaked into scratch worktree" >&2
		echo "  got:      $top" >&2
		echo "  expected: $expected" >&2
		exit 1
	fi
	echo "[committed-compiles] selftest: local Git hook env does not leak into -C worktree"
}

run_git_env_selftest() {
	local git_dir probe_worktree probe_target_dir
	git_dir=$(git -C "$repo_root" rev-parse --path-format=absolute --git-dir)
	probe_worktree="$repo_root/.worktrees/committed-compiles-env-selftest-$$"
	probe_target_dir="$repo_root/target/committed-compiles-env-selftest-$$"
	GIT_DIR="$git_dir" GIT_WORK_TREE="$repo_root" \
		ER_COMMITTED_CHECK_WORKTREE="$probe_worktree" \
		ER_COMMITTED_CHECK_TARGET_DIR="$probe_target_dir" \
		bash "$repo_root/scripts/check-committed-compiles.sh" --selftest-git-env-child
}

if [[ "$git_env_selftest_child" == 1 ]]; then
	git_env_child_selftest
	exit 0
fi

link_sibling

# --- selftest ---------------------------------------------------------------------------------
# A gate is not trusted on its own say-so, and "it passed" is worthless from a gate that cannot
# fail. Two-sided: a broken commit must go RED and HEAD must go GREEN, because a gate wedged red
# is as useless as one wedged green.
#
# The red half prefers the two REAL historical failures over a synthetic mutant -- they are the
# commits this gate exists for, they are free, and they exercise the exact shapes seen in the
# wild. They stop being reachable after a squash-merge or in a shallow clone, so the synthetic
# path above takes over rather than letting the selftest quietly pass on nothing.
if [[ "$selftest" == 1 ]]; then
	run_git_env_selftest
	proved=0
	for bad in 15b32ab0 11af0c60; do
		git -C "$repo_root" rev-parse --verify --quiet "$bad^{commit}" >/dev/null || continue
		if run_one "$bad" >/dev/null 2>&1; then
			echo "[committed-compiles] SELFTEST FAIL: known-broken commit $bad compiled clean" >&2
			exit 1
		fi
		echo "[committed-compiles] selftest: historical failure $bad went red, as it must"
		proved=$((proved + 1))
	done
	if [[ "$proved" == 0 ]]; then
		synth=$(synth_broken_rev HEAD)
		if run_one "$synth" >/dev/null 2>&1; then
			echo "[committed-compiles] SELFTEST FAIL: a synthesised broken tree compiled clean" >&2
			exit 1
		fi
		echo "[committed-compiles] selftest: no historical failure reachable; synthesised ${synth:0:8} went red"
	fi
	if ! run_one HEAD >/dev/null 2>&1; then
		echo "[committed-compiles] SELFTEST FAIL: HEAD does not compile, so a red verdict proves nothing" >&2
		exit 1
	fi
	echo "[committed-compiles] selftest: HEAD went green, as it must"
	echo "[committed-compiles] selftest passed"
	exit 0
fi

failed=()
for rev in "${revs[@]}"; do
	run_one "$rev" || failed+=("$rev")
done

if [[ ${#failed[@]} -ne 0 ]]; then
	cat >&2 <<EOF

[committed-compiles] REFUSING: ${#failed[@]} commit(s) fail: ${failed[*]}

READ THE cargo OUTPUT ABOVE FIRST -- this gate now asks two questions, and they have opposite
fixes. Do not reach for the second diagnosis when the error says lint.

  A LINT error (\`error: <lint text>\` ending in a clippy note such as
  \`-D clippy::all\`) means the code compiles and the workspace's own
  [workspace.lints.clippy] denial rejects it. Fix the code, or add a scoped
  \`#[allow(...)]\` with a reason. Your working tree has the same defect.

  A RESOLUTION error (unresolved import, no such module, cannot find) is the
  "consumer without its producer" shape: your working tree compiles because it
  still holds a file the commit does not carry. Find it with

      git status --short     # the producer is probably untracked or modified here
      git show --stat <rev>  # what the commit actually contains

  then amend or add a follow-up commit carrying the closure.
EOF
	exit 1
fi
