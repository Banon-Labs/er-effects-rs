#!/usr/bin/env bash
# The gate must not damage the thing it gates, and on 2026-08-31 it did -- Twice.
#
# Route, end to end (bd hooks-selftest-under-git-hook-blanks-the-live-config-2026-08-31): a push
# from a linked WORKTREE runs the gate suite from its pre-push hook, and git exports GIT_DIR to a
# linked worktree's hooks -- measured on git 2.55 by scripts/measure-git-hook-env.sh, which also
# measures that a main checkout's hooks get no GIT_DIR at all, which is why this looked unreachable
# for a day. `git -C <fixture>` does not override GIT_DIR, so every fixture command in a downstream
# gate lands on the shared config instead: `git init` saw a git dir not named `.git`, wrote
# core.bare = true, and every later `git status` in the main checkout died with "fatal: this
# operation must be run in a work tree"; `git config --unset core.hooksPath` disarmed the hooks for
# ninety minutes and a push reached origin ungated.
#
# scripts/check-git-hooks-installed.sh now scrubs its own environment, which closes the one gate
# that was caught. This closes the CLASS: any gate, today's or tomorrow's, that builds a git
# fixture without scrubbing gets caught here instead of in the next person's checkout.
#
# Why a sourced file rather than lines inside check.sh: scripts/test-check-config-guard.sh drives
# this logic against fixture repositories, and it must drive the real text, not a copy that can
# drift. A file both of them read is the only shape where that is structurally true. It used to be
# the opening trap of scripts/ci-local-check.sh, which was deleted on 2026-09-03 when the pre-push
# hook moved to parity with CI (both now run scripts/check.sh).
#
# Scoped to the two keys the damage lands on, deliberately. Comparing the whole config file would
# go red on an unrelated `[branch]` write from another agent working in this tree, and a gate that
# cries wolf gets its check deleted. Read through an explicit --git-dir, which outranks an
# inherited GIT_DIR and keeps working after core.bare has already gone true -- the state in which
# most other git commands stop working.
#
# Usage (sourced, never executed):
#   source scripts/check-gate-config-guard.sh
#   gate_config_snapshot "$repo_root"      # before any gate runs
#   ... gates ...
#   gate_config_report || failed=$((failed + 1))   # returns 1 and prints when the config moved

gate_config_key() {
	[[ -z "${GATE_CONFIG_COMMON_DIR:-}" ]] ||
		git --git-dir="$GATE_CONFIG_COMMON_DIR" config --get "$1" || true
}

gate_config_snapshot() {
	local root=${1:-.}
	GATE_CONFIG_COMMON_DIR=$(git -C "$root" rev-parse --path-format=absolute --git-common-dir 2>/dev/null || true)
	GATE_CONFIG_BARE_BEFORE=$(gate_config_key core.bare)
	GATE_CONFIG_HOOKSPATH_BEFORE=$(gate_config_key core.hooksPath)
}

# 0 = the config is unchanged. 1 = it moved, and the report naming both keys has been printed.
gate_config_report() {
	local bare_after hookspath_after
	[[ -n "${GATE_CONFIG_COMMON_DIR:-}" ]] || return 0
	bare_after=$(gate_config_key core.bare)
	hookspath_after=$(gate_config_key core.hooksPath)
	if [[ "$GATE_CONFIG_BARE_BEFORE" == "$bare_after" &&
		"$GATE_CONFIG_HOOKSPATH_BEFORE" == "$hookspath_after" ]]; then
		return 0
	fi
	echo "FAIL: this gate suite CHANGED the repository configuration it was checking." >&2
	echo "  $GATE_CONFIG_COMMON_DIR/config" >&2
	echo "    core.bare      ${GATE_CONFIG_BARE_BEFORE:-<unset>} -> ${bare_after:-<unset>}" >&2
	echo "    core.hooksPath ${GATE_CONFIG_HOOKSPATH_BEFORE:-<unset>} -> ${hookspath_after:-<unset>}" >&2
	echo "  A gate built a git fixture without scrubbing its environment first. If this ran from a" >&2
	echo "  LINKED WORKTREE, its hooks inherit GIT_DIR and 'git -C <fixture>' does not override it," >&2
	echo "  so fixture-only work lands on the shared config. Confirm with:" >&2
	echo "      bash scripts/measure-git-hook-env.sh" >&2
	echo "  Fix the offending script with: unset \$(git rev-parse --local-env-vars)" >&2
	echo "  Repair this checkout with: git config core.bare false && bash scripts/install-git-hooks.sh" >&2
	return 1
}
