#!/usr/bin/env bash
# Which GIT_* variables does a hook actually inherit? Measure it; Do not argue about it.
#
# This exists because two agents produced contradictory answers on 2026-08-31 and the
# disagreement was load-bearing: `scripts/check-git-hooks-installed.sh --selftest` builds its
# fixtures with `git init` and `git -C <fixture> config ...`, and `git -C` does not override
# GIT_DIR. So if a hook exports GIT_DIR, every one of those fixture commands is redirected at the
# live repository and the selftest silently rewrites the hook configuration it exists to protect.
# Whether that is a real route or only a theoretical one comes down to one measurable fact.
#
# The measurement has a trap in it, which is why this is a script and not a one-liner: a hook that
# never ran reports exactly the same thing as a hook that ran and inherited nothing -- an empty
# list. So the dumper always writes a marker line first, and --selftest refuses to pass unless
# every hook it expected actually fired.
#
# It covers the cell the earlier measurement missed: a linked WORKTREE. There, GIT_DIR is
# <main>/.git/worktrees/<name> -- a git dir not named `.git` -- and `git init` under it writes
# `core.bare = true` into the shared <main>/.git/config. That is the exact damage observed on the
# main checkout on 2026-08-31. See bd hooks-selftest-under-git-hook-blanks-the-live-config-2026-08-31.
#
# Usage:
#   bash scripts/measure-git-hook-env.sh            # report, exit 0
#   bash scripts/measure-git-hook-env.sh --selftest # report + assert the harness itself is sound
set -euo pipefail

HOOKS=(pre-commit prepare-commit-msg post-checkout post-merge pre-push)

tmp=""
cleanup() {
	local rc=$?
	[[ -n "$tmp" ]] && rm -rf -- "$tmp"
	exit "$rc"
}
trap cleanup EXIT

# Write a dumper for every hook name into $1, a core.hooksPath directory. Each dumper appends
# "<hook> FIRED" and then every GIT_* variable it can see, tagged with the scenario name.
install_dumpers() {
	local dir=$1 out=$2 tag=$3 name
	mkdir -p "$dir"
	for name in "${HOOKS[@]}"; do
		cat >"$dir/$name" <<EOF
#!/usr/bin/env bash
{
  echo "$tag/$name FIRED"
  env | sed -n 's/^\(GIT_[A-Z_]*\)=.*/$tag\/$name VAR \1/p' | sort
} >>"$out"
exit 0
EOF
		chmod 0755 "$dir/$name"
	done
}

# Drive every hook in $1 (a checkout) so each dumper gets a chance to run.
drive_hooks() {
	local repo=$1 remote=$2
	git -C "$repo" -c user.name=t -c user.email=t@t.t commit -q --allow-empty -m "hook probe" || true
	git -C "$repo" checkout -q -b "probe-$RANDOM" || true
	git -C "$repo" merge -q --allow-unrelated-histories --no-edit HEAD >/dev/null 2>&1 || true
	git -C "$repo" push -q "$remote" HEAD:"refs/heads/probe-$RANDOM" >/dev/null 2>&1 || true
}

tmp=$(mktemp -d "${TMPDIR:-/tmp}/er-hook-env.XXXXXX")
out="$tmp/observed.txt"
: >"$out"

# --- scenario 1: an ordinary main checkout -----------------------------------------------------
git init -q "$tmp/remote" --bare
git init -q "$tmp/main"
git -C "$tmp/main" -c user.name=t -c user.email=t@t.t commit -q --allow-empty -m base
# Absolute on purpose: a relative core.hooksPath resolves against each worktree's own root, so a
# relative value would leave the linked-worktree scenario with no hooks at all and the report would
# say "did not fire" for every row that matters. (It did, the first time this was run.)
install_dumpers "$tmp/hookdir" "$out" main
git -C "$tmp/main" config core.hooksPath "$tmp/hookdir"
drive_hooks "$tmp/main" "$tmp/remote"

# --- scenario 2: a linked WORKTREE of that checkout --------------------------------------------
# The hooks directory is shared (core.hooksPath lives in the common config), so the same dumpers
# fire; only the environment differs. Re-tagging means re-writing them for the worktree run.
git -C "$tmp/main" worktree add -q -b linked "$tmp/linked"
install_dumpers "$tmp/hookdir" "$out" linked
drive_hooks "$tmp/linked" "$tmp/remote"

echo "=== git $(git --version | awk '{print $3}') -- GIT_* variables observed inside hooks ==="
fired=0
for scenario in main linked; do
	for name in "${HOOKS[@]}"; do
		if grep -qx "$scenario/$name FIRED" "$out" 2>/dev/null; then
			fired=$((fired + 1))
			vars=$(sed -n "s#^$scenario/$name VAR ##p" "$out" | sort -u | tr '\n' ' ')
			printf '  %-8s %-20s %s\n' "$scenario" "$name" "${vars:-<none>}"
		else
			printf '  %-8s %-20s (did not fire -- NOT evidence of an empty environment)\n' \
				"$scenario" "$name"
		fi
	done
done

git_dir_seen=$(sed -n 's#^\([a-z]*\)/\([a-z-]*\) VAR GIT_DIR$#\1/\2#p' "$out" | sort -u | tr '\n' ' ')
echo
if [[ -n "$git_dir_seen" ]]; then
	echo "  GIT_DIR IS EXPORTED to: $git_dir_seen"
	echo "  => any script those hooks call must 'unset \$(git rev-parse --local-env-vars)' before"
	echo "     building git fixtures, or 'git -C <fixture>' will silently edit the live repository."
else
	echo "  GIT_DIR is exported to NONE of the hooks that fired above."
	echo "  => an inherited GIT_DIR is still a real hazard for any OTHER caller that exports it,"
	echo "     but a git hook is not how it arrives on this git version."
fi

if [[ "${1:-}" == "--selftest" ]]; then
	# 1. The harness must actually observe hooks running. A silent zero would make every
	#    conclusion above vacuous, which is the exact failure mode this whole file is about.
	if ((fired == 0)); then
		echo "[measure-git-hook-env] SELFTEST FAIL: no hook fired at all, so the report above is" >&2
		echo "  vacuous -- it cannot distinguish 'inherited nothing' from 'never ran'." >&2
		exit 1
	fi
	# 2. And it must observe some GIT_* variable, or the dumper's env filter is broken and every
	#    hook would report an empty inheritance no matter what git actually exported.
	if ! grep -q ' VAR GIT_' "$out"; then
		echo "[measure-git-hook-env] SELFTEST FAIL: hooks fired but not one GIT_* variable was" >&2
		echo "  captured. The dumper's filter is broken; a 'no GIT_DIR' finding would be an artefact." >&2
		exit 1
	fi
	# 3. Positive control for the claim the report makes: with GIT_DIR aimed at a linked WORKTREE,
	#    `git init` writes core.bare = true into the shared config. If that ever stops being true
	#    the diagnosis this script carries has expired and must be re-derived, not repeated.
	before=$(cat "$tmp/main/.git/config")
	(cd "$tmp/linked" && GIT_DIR=$(git rev-parse --absolute-git-dir) git init -q)
	if [[ "$(git -C "$tmp/main" config --get core.bare)" != true ]]; then
		echo "[measure-git-hook-env] SELFTEST FAIL: 'git init' under a linked-worktree GIT_DIR did" >&2
		echo "  NOT set core.bare=true in the shared config. The documented mechanism no longer holds." >&2
		exit 1
	fi
	printf '%s\n' "$before" >"$tmp/main/.git/config"
	echo "[measure-git-hook-env] selftest passed ($fired hook invocations observed)"
fi
