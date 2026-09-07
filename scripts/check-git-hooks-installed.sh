#!/usr/bin/env bash
# Is the pre-push gate actually installed -- By either of the two routes git can take?
#
# Measured 2026-08-31, twice, in the same direction both times: the hook layer failing open while
# looking installed.
#
#   * This clone's `core.hooksPath` was the absolute path
#     /home/banon/projects/er-effects-rs/.githooks, left behind by commit 39a919e0, which renamed
#     the repository directory to er-mods-rs. Git resolved its hooks directory to somewhere that no
#     longer existed, so no hook ran at all -- not the main-push guard, not the gate suite
#     -- and nothing said so, because a hook that cannot be found is indistinguishable from a hook
#     that passed.
#
#   * Later the same day `core.hooksPath` was UNSET for about ninety minutes. Git then used its
#     fallback, $GIT_COMMON_DIR/hooks, which is not version-controlled and which held a 537-byte
#     block-main-only pre-push from 2026-07-27 -- no scripts/check-committed-compiles.sh, no
#     the gate suite. A push reached origin through it. It happened to be green.
#
# So this asserts, in the order they fail:
#   1. core.hooksPath is set at all;
#   2. it resolves to a real directory holding an executable pre-push;
#   3. the configured value is relative. An absolute path is correct until the day the checkout
#      moves or is renamed, and then it is silently wrong;
#   4. The FALLBACK is safe too. $GIT_COMMON_DIR/hooks must hold scripts/hooks-fallback-shim
#      verbatim, under every name scripts/hooks/ carries -- so that whichever way git resolves the
#      hook, the same checks run. Checks 1-3 describe a value that several tools write (beads
#      rewrites core.hooksPath: see the header of scripts/hooks-fallback-shim), so the fallback is
#      not a theoretical path. Byte-identical, not merely present: a stale shim is the 2026-07-27
#      failure with a newer date on it.
#   5. The hook git will run is *ours*. Checks 1-4 are all about where the hook is; none of them
#      opens it. See check_hook_identity below for the measured hazard that closes -- in one
#      sentence: `bd hooks install` honours an existing core.hooksPath and writes its own shims
#      into it, which here is the version-controlled scripts/hooks, and every check above stays
#      green afterwards because a hook is still installed and still executable.
#
# Not run in CI: a fresh runner has no local hook configuration and does not push.
set -euo pipefail

fail() {
	echo "[check-git-hooks-installed] FAIL: $1" >&2
	cat >&2 <<'FIXEOF'

fix:  bash scripts/install-git-hooks.sh
then: git config --get core.hooksPath        # must print a relative path: scripts/hooks
FIXEOF
	exit 1
}

check_repo() {
	local root=$1 configured resolved
	# The state that arrives with it. Twice on 2026-08-31 -- once observed live, 21 seconds after
	# the fact -- .git/config was rewritten with [core] reduced to exactly the four keys a fresh
	# `git init` writes (repositoryformatversion, filemode, bare, logallrefupdates), with `bare`
	# flipped to true and `hooksPath` gone, in a single write, everything below [core] untouched.
	# One writer replacing the whole section, not two `git config` edits; still unattributed (`bd
	# dolt push`, `bd remember` and nested `git worktree add` were each measured innocent). What it
	# looks like from inside is `fatal: this operation must be run in a work tree` out of every git
	# command in the main checkout, which reads like a broken checkout rather than a config key.
	# Name it here so the next person gets a diagnosis instead of a puzzle.
	if [[ -d "$root/.git" ]] && [[ "$(git -C "$root" rev-parse --is-bare-repository 2>/dev/null || echo unknown)" == true ]]; then
		fail "$root has a .git directory but core.bare is true, so git treats this checkout as BARE -- every 'git status' / 'git rev-parse --show-toplevel' in it dies with 'fatal: this operation must be run in a work tree'. Repair with 'git config core.bare false' FIRST, then re-run scripts/install-git-hooks.sh (which no longer dies in this state, but the hooks it installs do). See bd main-checkout-went-bare-config-worktree-is-inert-at-repoformat-0-2026-08-31"
	fi
	configured=$(git -C "$root" config --get core.hooksPath || true)
	[[ -n "$configured" ]] || fail "core.hooksPath is unset in $root, so the version-controlled hooks in scripts/hooks are not installed"
	[[ "$configured" != /* ]] || fail "core.hooksPath is ABSOLUTE ($configured); it breaks the moment this checkout is renamed or moved, which is exactly what happened on 2026-08-31"
	resolved=$(cd -- "$root" && git rev-parse --path-format=absolute --git-path hooks)
	[[ -d "$resolved" ]] || fail "core.hooksPath ($configured) resolves to $resolved, which does not exist -- no hook can run"
	[[ -x "$resolved/pre-push" ]] || fail "$resolved/pre-push is missing or not executable -- nothing gates a push"
	printf '[check-git-hooks-installed] ok -- core.hooksPath=%s -> %s (pre-push executable)\n' \
		"$configured" "$resolved"
	check_hook_identity "$root" "$resolved"
	check_fallback "$root"
}

# The route git takes when core.hooksPath is gone. Skipped when the checkout carries no shim
# template (an older tree, and the selftest fixtures), because there is then nothing to compare
# against and the shape simply does not exist yet.
check_fallback() {
	local root=$1 template fallback_dir name hook
	template="$root/scripts/hooks-fallback-shim"
	[[ -f "$template" ]] || return 0

	fallback_dir=$(cd -- "$root" && git rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
	[[ -n "$fallback_dir" ]] || fail "cannot resolve --git-common-dir for $root, so the .git/hooks fallback cannot be verified"
	fallback_dir="$fallback_dir/hooks"
	[[ -d "$fallback_dir" ]] || fail "the fallback hooks directory $fallback_dir does not exist; if core.hooksPath is ever unset, NO hook runs and a push is ungated"

	for hook in "$root"/scripts/hooks/*; do
		[[ -f "$hook" ]] || continue
		name=$(basename -- "$hook")
		[[ -f "$fallback_dir/$name" ]] || fail "$fallback_dir/$name is missing -- with core.hooksPath unset git would run no $name at all"
		[[ -x "$fallback_dir/$name" ]] || fail "$fallback_dir/$name is not executable -- git would skip it and the push would be ungated"
		cmp -s "$template" "$fallback_dir/$name" || fail "$fallback_dir/$name is not scripts/hooks-fallback-shim. A hand-written or stale fallback is how a 537-byte block-main-only pre-push stayed installed for five weeks while the real gate grew around it"
	done
	printf '[check-git-hooks-installed] ok -- fallback %s carries the shim for: %s\n' \
		"$fallback_dir" "$(cd -- "$root/scripts/hooks" && echo *)"
}

# What each hook must still call. A manifest of invocations rather than a hash, deliberately:
# these hooks are edited often and legitimately, and a gate that forces a digest to be
# regenerated on every edit gets switched off instead of updated. Every entry here is a script
# whose absence is the whole failure -- the 2026-07-27 fallback ran neither
# check-committed-compiles.sh nor the gate suite and looked installed for five weeks.
# A hook name with no entry gets the byte-identity checks only; add its calls here when it grows
# any.
hook_required_invocations() {
	case "$1" in
	pre-push)
		printf '%s\n' \
			scripts/git-pre-push-block-main.sh \
			scripts/check-committed-compiles.sh \
			scripts/check.sh
		;;
	pre-commit)
		printf '%s\n' \
			scripts/check-marker-file-gates.py \
			scripts/check-env-gate-comments.py \
			'cargo fmt'
		;;
	commit-msg)
		# The hook is four lines of forwarding, and this is the line that makes it a gate. Without
		# it the file could be reduced to `exit 0` and every check above would stay green -- the
		# 2026-07-27 fallback's story, which is why this map exists.
		printf '%s\n' \
			scripts/conventional-commit-subject.py
		;;
	esac
}

# 0 = worktree file matches the blob committed at head, 1 = it differs, 2 = no such blob to
# compare against (an unborn head, or a path not yet committed -- both real states, neither a
# finding).
tracked_matches_head() {
	local root=$1 rel=$2 tmpf rc=0
	git -C "$root" rev-parse --verify -q HEAD >/dev/null 2>&1 || return 2
	git -C "$root" cat-file -e "HEAD:$rel" >/dev/null 2>&1 || return 2
	tmpf=$(mktemp "${TMPDIR:-/tmp}/er-hooks-blob.XXXXXX") || return 2
	if ! git -C "$root" cat-file blob "HEAD:$rel" >"$tmpf" 2>/dev/null; then
		rm -f -- "$tmpf"
		return 2
	fi
	cmp -s "$tmpf" "$root/$rel" || rc=$?
	rm -f -- "$tmpf"
	return "$rc"
}

# Is the hook git will run *ours*? The question checks 1-4 cannot ask, because none of them opens
# the file.
#
# Measured 2026-08-31, in a throwaway repo: `bd hooks install` honours an existing
# core.hooksPath and writes its five shim files into that directory. Here core.hooksPath is
# `scripts/hooks`, which is version-controlled, so one `bd hooks install` -- a command every
# agent in this tree is one keystroke from, and which `bd doctor` is a front door to -- replaces
# the tracked pre-commit and pre-push with beads' shims. Checks 1-4 all stay green afterwards: a
# hook is still installed, still executable, at exactly the configured path. The next agent then
# commits the overwrite as an ordinary file change and the gate is gone for everyone.
#
# Three depths, because each sees a state the other two cannot:
#
#   (a) what git will execute is either scripts/hooks/<name> itself, a byte-identical copy of it,
#       or one of the two WRAPPERS this repo ships -- scripts/hooks-fallback-shim and
#       .githooks/<name>. Those two are allowed to differ because they exec the tracked file
#       instead of reimplementing it, and clones exist configured for either directory. Content
#       identity is not assertable for a wrapper (its whole job is to have different bytes), so
#       the weaker property asserted there is: it is byte-identical to a wrapper this repo
#       ships, that wrapper still names scripts/hooks/, and it matches its own committed blob.
#       This is the check for core.hooksPath aimed somewhere else entirely (`.beads/hooks` is
#       the literal beads writes when the key is absent; see the header of
#       scripts/hooks-fallback-shim for the binary's own strings).
#
#   (b) scripts/hooks/<name> is byte-identical to the BLOB committed at head. In today's
#       configuration git executes the worktree file directly, so (a) compares that file with
#       itself and proves nothing; the committed blob is the only independent copy in existence.
#       An overwrite shows up here the moment it lands, before anyone commits it. The cost is
#       that an UNCOMMITTED edit to a hook is also a refusal -- which is the intended reading:
#       the thing gating the push should be the thing a reviewer can see.
#
#   (c) scripts/hooks/<name> still INVOKES the gate it exists to run. (b) is defeated by a single
#       commit, and "somebody committed the weaker hook" is not hypothetical here -- it is the
#       2026-07-27 fallback's entire life story. hook_required_invocations above is that floor.
check_hook_identity() {
	local root=$1 resolved=$2 name src exe shim wrapper accepted needle rc
	local head_seen=0 head_missing=0
	shim="$root/scripts/hooks-fallback-shim"

	for src in "$root"/scripts/hooks/*; do
		[[ -f "$src" ]] || continue
		name=$(basename -- "$src")
		exe="$resolved/$name"

		[[ -f "$exe" ]] || fail "scripts/hooks/$name is version-controlled but $exe does not exist, so git runs no $name at all"
		[[ -x "$exe" ]] || fail "$exe is not executable; git skips a non-executable hook without a word, so nothing runs"

		# (a) -- only meaningful when git will execute a different file from the tracked one.
		if ! [[ "$exe" -ef "$src" ]]; then
			accepted=""
			cmp -s "$src" "$exe" && accepted="$src"
			if [[ -z "$accepted" ]]; then
				for wrapper in "$shim" "$root/.githooks/$name"; do
					[[ -f "$wrapper" ]] || continue
					cmp -s "$wrapper" "$exe" || continue
					# A wrapper earns its exemption by forwarding. Both of this repo's
					# wrappers exec scripts/hooks/<name> rather than carrying their own
					# copy of the checks -- which is the divergence .githooks/pre-push's
					# own header exists to warn about. One that stops naming that
					# directory has become a second implementation, and a second
					# implementation is how the 2026-07-27 stub happened.
					grep -qF -- 'scripts/hooks/' "$wrapper" ||
						fail "$exe is $wrapper, which no longer forwards to scripts/hooks/$name -- a wrapper that stopped forwarding is a second implementation of the gate, and nothing keeps a second implementation in step"
					accepted="$wrapper"
					break
				done
			fi
			[[ -n "$accepted" ]] || fail "git will run $exe, which is neither scripts/hooks/$name nor either wrapper this repo ships (scripts/hooks-fallback-shim, .githooks/$name) -- some other tool installed its own $name. 'bd hooks install' writes its shims into whatever core.hooksPath already names. Repair: bash scripts/install-git-hooks.sh"
		fi

		# (b)
		rc=0
		tracked_matches_head "$root" "scripts/hooks/$name" || rc=$?
		case "$rc" in
		0) head_seen=1 ;;
		2) head_missing=1 ;;
		*) fail "scripts/hooks/$name does not match the blob committed at HEAD. Either you edited the hook and have not committed it -- commit it, because git is already running it -- or something overwrote it: 'git diff -- scripts/hooks/$name' will show which. 'bd hooks install' rewrites the hooks in whatever directory core.hooksPath names, and here that directory is version-controlled" ;;
		esac

		# (c)
		while read -r needle; do
			[[ -n "$needle" ]] || continue
			grep -qF -- "$needle" "$src" ||
				fail "scripts/hooks/$name no longer invokes '$needle'. That call IS the gate; a hook that skips it passes every 'is it installed' test while checking nothing (scripts/hooks-fallback-shim's header, 2026-07-27). If the hook was deliberately restructured, update hook_required_invocations in $0"
		done < <(hook_required_invocations "$name")
	done

	# The WRAPPERS are executed too -- the shim whenever core.hooksPath goes missing, .githooks/*
	# in any clone configured for that directory -- so hold both to (b). Their bytes must differ
	# from the hook's; what must not differ is their bytes from their own committed blob.
	for wrapper in scripts/hooks-fallback-shim .githooks/pre-commit .githooks/pre-push; do
		[[ -f "$root/$wrapper" ]] || continue
		rc=0
		tracked_matches_head "$root" "$wrapper" || rc=$?
		[[ "$rc" -ne 1 ]] || fail "$wrapper does not match the blob committed at HEAD -- git runs it whenever core.hooksPath is unset or points at its directory, and nothing else reviews it"
	done

	if [[ "$head_seen" == 1 ]]; then
		printf '[check-git-hooks-installed] ok -- hook CONTENT verified: executed == tracked, tracked == HEAD blob, required invocations present\n'
	else
		printf '[check-git-hooks-installed] ok -- hook CONTENT verified: executed == tracked, required invocations present (no committed blob to compare against)\n'
	fi
	[[ "$head_missing" == 0 ]] || printf '[check-git-hooks-installed] note -- at least one hook has no blob at HEAD; the committed-content check was skipped for it\n'
}

# --- selftest ---------------------------------------------------------------------------------
# A gate is not trusted on its own say-so. Rebuild the exact failures in throwaway repos -- an
# absolute hooksPath whose directory has been renamed out from under it, and a fallback directory
# holding a weaker stub than the real hook -- and require a refusal for each.
if [[ "${1:-}" == "--selftest" ]]; then
	# A hook'S environment REDIRECTS every fixture command below at the real repository, and that
	# is the writer that kept BLANKING core.hooksPath: It is this script.
	#
	# ...BUT only when the push came from a linked WORKTREE, which is the detail two agents
	# contradicted each other over on 2026-08-31 and which decides whether this is a real route or
	# only a theoretical one. Both were measuring correctly; they were measuring different repos.
	# scripts/measure-git-hook-env.sh settles it on git 2.55, and refuses to report at all unless it
	# saw hooks actually fire (a hook that never ran reports the same empty environment as one that
	# inherited nothing):
	#   Main checkout   pre-push -> GIT_EDITOR, GIT_EXEC_PATH, GIT_PREFIX. No GIT_DIR.
	#   Linked WORKTREE pre-push -> the same plus GIT_DIR=<main>/.git/worktrees/<name>.
	#                               (also pre-commit, prepare-commit-msg, post-checkout)
	#
	# That GIT_DIR's basename is not `.git`, so `git init` under it concludes the repository is bare
	# and writes core.bare = true into the shared <main>/.git/config -- which is where the "fatal:
	# this operation must be run in a work tree" came from. `git -C <dir>` does not override
	# GIT_DIR, so `config core.hooksPath <abs>` and `config --unset core.hooksPath` landed there
	# too. Every negative control "passed" because the fixture was reading the real repo's value, so
	# the gate failed itself, on damage it had just done, once per push.
	#
	# Not what an earlier version of this comment claimed, and worth stating so nobody re-derives
	# it: `git init` re-run on an existing repo drops no keys -- a sentinel [core] key survives it
	# in every form (cwd inside or outside, with or without a path argument, GIT_DIR set or not).
	# [core] here only ever held five keys, so unsetting hooksPath leaves four that merely look like
	# a fresh init.
	#
	# Reproduced 2026-08-31 against the historical file, GIT_DIR aimed at a linked worktree:
	#   849cc89b -> shared config: core.bare=true and core.hooksPath UNSET  (the observed damage)
	#   db109e1d -> shared config: core.bare=true                          (it restores hooksPath)
	#   bb9fe569 -> config unchanged                                       (the unset below)
	#
	# shellcheck disable=SC2046  # word splitting is the point: one variable name per word.
	unset $(git rev-parse --local-env-vars)
	# ...AND then prove it, because the unset above is only as good as the list it unsets. The
	# snapshot below does not care which variable or which route: if this selftest changes the
	# ambient config at all, it goes red instead of silently disarming the push gate it exists to
	# protect. Belt and braces on purpose -- the measurement above is of one git version, and the
	# failure it guards is invisible from inside (with GIT_DIR exported the 849cc89b version left
	# this repo's core.hooksPath UNSET and still printed "selftest passed").
	ambient_config=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)/.git/config
	ambient_before=""
	[[ -f "$ambient_config" ]] && ambient_before=$(cat "$ambient_config")

	tmp=$(mktemp -d "${TMPDIR:-/tmp}/er-hooks-installed-selftest.XXXXXX")
	# Invoked indirectly, by the `trap ... EXIT` below. ShellCheck 0.11 does not follow trap
	# handlers, so it reported SC2329 and `shellcheck scripts/check-git-hooks-installed.sh` in
	# scripts/check.sh exited 1 -- a gate red since this function landed, in the one file whose
	# whole subject is gates that fail without saying so. (The prose goes above the directive:
	# a following comment line beginning with the word `shellcheck` is parsed as another
	# directive and becomes a hard SC1073 parse error.)
	# shellcheck disable=SC2329
	selftest_cleanup() {
		local rc=$?
		rm -rf -- "$tmp"
		if [[ -n "$ambient_before" ]] && [[ "$ambient_before" != "$(cat "$ambient_config")" ]]; then
			echo "[check-git-hooks-installed] SELFTEST FAIL: the selftest MUTATED $ambient_config." >&2
			echo "  Fixture-only work reached the real repository. Look for an inherited git" >&2
			echo "  environment variable, or a git call without -C, and repair the checkout with:" >&2
			echo "      bash scripts/install-git-hooks.sh" >&2
			exit 1
		fi
		exit "$rc"
	}
	trap selftest_cleanup EXIT
	# The fixture's pre-push must satisfy check (c) or every positive arm below turns red for the
	# wrong reason, so it names the three scripts the real hook runs. It does not run them: what is
	# under test here is the checker, not the gate.
	fixture_hook=$'#!/usr/bin/env bash\n# stands in for the real hook, which runs:\n#   scripts/git-pre-push-block-main.sh\n#   scripts/check-committed-compiles.sh\n#   scripts/check.sh\nexit 0\n'
	# The mutant, in the measured shape of the hazard: `bd hooks install` honours an existing
	# core.hooksPath and writes shims like this one into it -- here, into version control.
	beads_shim=$'#!/bin/sh\n# beads git hook (managed by bd)\nexec bd hooks run pre-push "$@"\n'

	git init -q "$tmp/before"
	mkdir -p "$tmp/before/scripts/hooks"
	printf '%s' "$fixture_hook" >"$tmp/before/scripts/hooks/pre-push"
	chmod 0755 "$tmp/before/scripts/hooks/pre-push"

	git -C "$tmp/before" config core.hooksPath "$tmp/before/scripts/hooks"
	mv "$tmp/before" "$tmp/after" # the rename that broke it
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a renamed absolute hooksPath was accepted" >&2
		exit 1
	fi

	git -C "$tmp/after" config core.hooksPath scripts/hooks # the durable form
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: a correct relative hooksPath was rejected" >&2
		exit 1
	}

	git -C "$tmp/after" config --unset core.hooksPath
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: an unset hooksPath was accepted" >&2
		exit 1
	fi
	git -C "$tmp/after" config core.hooksPath scripts/hooks

	# --- the fallback half. Give the fixture a shim template, which switches check_fallback on.
	# shellcheck disable=SC2016  # the $(...) and "$@" are the SHIM's text, to be expanded when git
	# runs it, not when this line writes it. Double quotes here would bake this repo's paths into
	# the fixture and the test would stop resembling the shim it is standing in for.
	printf '#!/usr/bin/env bash\nexec bash "$(git rev-parse --show-toplevel)/scripts/hooks/$(basename -- "$0")" "$@"\n' \
		>"$tmp/after/scripts/hooks-fallback-shim"
	chmod 0755 "$tmp/after/scripts/hooks-fallback-shim"

	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a missing fallback shim was accepted" >&2
		exit 1
	fi

	# The 2026-07-27 shape: a fallback that exists, is executable, and is weaker than the real hook.
	printf '#!/usr/bin/env bash\nexit 0\n' >"$tmp/after/.git/hooks/pre-push"
	chmod 0755 "$tmp/after/.git/hooks/pre-push"
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a weaker hand-written fallback stub was accepted" >&2
		exit 1
	fi

	cp -f "$tmp/after/scripts/hooks-fallback-shim" "$tmp/after/.git/hooks/pre-push"
	chmod 0755 "$tmp/after/.git/hooks/pre-push"
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: a correctly installed fallback shim was rejected" >&2
		exit 1
	}

	# A non-executable shim is skipped by git, which is the same hole wearing the right bytes.
	chmod 0644 "$tmp/after/.git/hooks/pre-push"
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a non-executable fallback shim was accepted" >&2
		exit 1
	fi

	chmod 0755 "$tmp/after/.git/hooks/pre-push"

	# ...and the bare flag that came with the unset hooksPath both times it happened.
	git -C "$tmp/after" config core.bare true
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a bare-flagged working checkout was accepted" >&2
		exit 1
	fi
	git -C "$tmp/after" config core.bare false
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: a repaired checkout was rejected" >&2
		exit 1
	}

	# --- Content identity. Everything above proves a hook is installed. These prove it is ours,
	# which is a different claim, and this repo has been burned by conflating them twice.

	# (a)+(c): the overwrite in place -- the tracked hook replaced where it stands. This is
	# literally what `bd hooks install` does here, and it defeats every check above it.
	printf '%s' "$beads_shim" >"$tmp/after/scripts/hooks/pre-push"
	chmod 0755 "$tmp/after/scripts/hooks/pre-push"
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a beads shim written OVER the tracked scripts/hooks/pre-push was accepted -- that is the 'bd hooks install' overwrite verbatim" >&2
		exit 1
	fi
	printf '%s' "$fixture_hook" >"$tmp/after/scripts/hooks/pre-push"
	chmod 0755 "$tmp/after/scripts/hooks/pre-push"
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: the restored hook was rejected" >&2
		exit 1
	}

	# (a): core.hooksPath redirected at another directory -- the other shape beads writes, and the
	# one where the tracked hook is left untouched and simply stops being the one git runs.
	mkdir -p "$tmp/after/.beads/hooks"
	printf '%s' "$beads_shim" >"$tmp/after/.beads/hooks/pre-push"
	chmod 0755 "$tmp/after/.beads/hooks/pre-push"
	git -C "$tmp/after" config core.hooksPath .beads/hooks
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: core.hooksPath aimed at a foreign directory holding a foreign pre-push was accepted" >&2
		exit 1
	fi
	# ...and the specificity arm: a relocated directory holding the forwarding shim is legitimate,
	# because that shim execs the tracked hook rather than replacing it. A gate red on every
	# wrapper is a gate people route around.
	cp -f "$tmp/after/scripts/hooks-fallback-shim" "$tmp/after/.beads/hooks/pre-push"
	chmod 0755 "$tmp/after/.beads/hooks/pre-push"
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: a relocated hooks directory carrying the forwarding shim was rejected" >&2
		exit 1
	}

	# The other wrapper this REPO SHIPS: .githooks/<name>, kept because clones exist configured
	# for that directory. It is a forwarder, so its bytes must differ from the hook's -- content
	# identity is not assertable for it, and the property that replaces it is that it still names
	# scripts/hooks/. Prove both directions, or the exemption is a hole with a comment on it.
	mkdir -p "$tmp/after/.githooks"
	printf '#!/usr/bin/env bash\nexec bash scripts/hooks/pre-push "$@"\n' \
		>"$tmp/after/.githooks/pre-push"
	chmod 0755 "$tmp/after/.githooks/pre-push"
	git -C "$tmp/after" config core.hooksPath .githooks
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: the legacy .githooks forwarder was rejected -- a gate red on a legitimate wrapper is a gate people route around" >&2
		exit 1
	}
	printf '#!/usr/bin/env bash\n# a second implementation, no longer forwarding\nexit 0\n' \
		>"$tmp/after/.githooks/pre-push"
	chmod 0755 "$tmp/after/.githooks/pre-push"
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a .githooks wrapper that stopped forwarding was accepted -- that is a second copy of the gate, and nothing keeps a second copy in step" >&2
		exit 1
	fi
	rm -rf -- "$tmp/after/.githooks"
	git -C "$tmp/after" config core.hooksPath scripts/hooks

	# --- (b) and (c) in isolation. Each mutant below is invisible to the other layer, which is the
	# only way to show that both are load-bearing rather than one covering for the other.
	# Plumbing, not `git add`: this repo forbids the porcelain form.
	fixture_commit() {
		local repo=$1 blob tree commit
		blob=$(git -C "$repo" hash-object -w -- ./scripts/hooks/pre-push)
		git -C "$repo" update-index --add --cacheinfo "100755,$blob,scripts/hooks/pre-push"
		tree=$(git -C "$repo" write-tree)
		commit=$(git -C "$repo" -c user.name=hooks-selftest -c user.email=hooks@invalid \
			commit-tree "$tree" -m "hooks fixture")
		git -C "$repo" update-ref HEAD "$commit"
	}
	fixture_commit "$tmp/after"
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: a checkout whose hook matches its committed blob was rejected" >&2
		exit 1
	}

	# (b) alone: an uncommitted edit that keeps every required invocation, so the invocation floor
	# cannot see it and only the committed blob can.
	printf '%s# appended after the commit\n' "$fixture_hook" >"$tmp/after/scripts/hooks/pre-push"
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: an uncommitted rewrite of the hook was accepted -- git is running content nobody can review" >&2
		exit 1
	fi
	printf '%s' "$fixture_hook" >"$tmp/after/scripts/hooks/pre-push"

	# (c) alone: the same overwrite, committed, so (b) is satisfied and the invocation floor is the
	# only thing left standing. One commit is all it takes to defeat a blob comparison -- which is
	# exactly how the 2026-07-27 stub lived for five weeks.
	printf '%s' "$beads_shim" >"$tmp/after/scripts/hooks/pre-push"
	chmod 0755 "$tmp/after/scripts/hooks/pre-push"
	fixture_commit "$tmp/after"
	if "$0" "$tmp/after" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a COMMITTED beads shim was accepted -- the blob comparison agrees with it, so only the required-invocation floor could have caught it" >&2
		exit 1
	fi
	printf '%s' "$fixture_hook" >"$tmp/after/scripts/hooks/pre-push"
	chmod 0755 "$tmp/after/scripts/hooks/pre-push"
	fixture_commit "$tmp/after"
	"$0" "$tmp/after" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: the restored, committed hook was rejected" >&2
		exit 1
	}

	# --- The linked WORKTREE, the third place git can resolve a hook from. A relative
	# core.hooksPath is resolved against the WORKTREE's top-level, so the file git runs there is
	# that worktree's own copy -- which can be overwritten independently of the main checkout's.
	git -C "$tmp/after" worktree add -q -b hooks-selftest-wt "$tmp/wt" HEAD || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: could not create the linked-worktree fixture" >&2
		exit 1
	}
	"$0" "$tmp/wt" >/dev/null || {
		echo "[check-git-hooks-installed] SELFTEST FAIL: a linked worktree with a correct hook was rejected" >&2
		exit 1
	}
	printf '%s' "$beads_shim" >"$tmp/wt/scripts/hooks/pre-push"
	chmod 0755 "$tmp/wt/scripts/hooks/pre-push"
	if "$0" "$tmp/wt" >/dev/null 2>&1; then
		echo "[check-git-hooks-installed] SELFTEST FAIL: a linked worktree whose own scripts/hooks/pre-push had been overwritten was accepted" >&2
		exit 1
	fi

	# The negative control for the UNSET above, because the bug it fixes is invisible from inside:
	# with GIT_DIR inherited every assertion still ran, still printed, and still passed the two
	# positive cases -- only the refusals stopped refusing, and the damage landed somewhere this
	# script never looks. So re-run the whole selftest with GIT_DIR aimed at a repository that must
	# not be touched, and compare its config byte for byte. If the unset ever regresses, the
	# fixtures land in the bystander and this fails instead of the next person's checkout.
	if [[ -z "${ER_HOOKS_SELFTEST_NO_RECURSE:-}" ]]; then
		git init -q "$tmp/bystander"
		bystander_before=$(cat "$tmp/bystander/.git/config")
		if ! GIT_DIR="$tmp/bystander/.git" ER_HOOKS_SELFTEST_NO_RECURSE=1 "$0" --selftest >/dev/null 2>&1; then
			echo "[check-git-hooks-installed] SELFTEST FAIL: the selftest does not survive an inherited GIT_DIR, which is the environment every git hook runs in" >&2
			exit 1
		fi
		if [[ "$bystander_before" != "$(cat "$tmp/bystander/.git/config")" ]]; then
			echo "[check-git-hooks-installed] SELFTEST FAIL: with GIT_DIR inherited the fixtures rewrote the bystander repository's config -- that is the live-checkout corruption, reproduced" >&2
			exit 1
		fi
	fi

	echo "[check-git-hooks-installed] selftest passed"
	exit 0
fi

check_repo "${1:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)}"
