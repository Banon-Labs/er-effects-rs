#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
guard="$repo_root/scripts/git-pre-push-block-main.sh"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/er-quickload-pre-push-main-guard.XXXXXX")
trap 'rm -rf "$tmp_dir"' EXIT

repo="$tmp_dir/main-repo"
git_clean=(env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE git)
"${git_clean[@]}" init -q "$repo"
"${git_clean[@]}" -C "$repo" symbolic-ref HEAD refs/heads/main

expect_block() {
	local label=$1
	local input=${2:-}
	local output status
	set +e
	output=$(cd "$repo" && printf '%s' "$input" | env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE "$guard" origin https://example.invalid/repo.git 2>&1)
	status=$?
	set -e
	if [[ "$status" -eq 0 ]]; then
		echo "expected block but command passed: $label" >&2
		exit 1
	fi
	if [[ "$output" != *"ER-EFFECTS-BLOCK-MAIN-PUSH"* ]]; then
		echo "missing guard marker for blocked case: $label" >&2
		echo "$output" >&2
		exit 1
	fi
}

expect_allow() {
	local label=$1
	local input=${2:-}
	local output status
	set +e
	output=$(cd "$repo" && printf '%s' "$input" | env -u GIT_DIR -u GIT_WORK_TREE -u GIT_INDEX_FILE "$guard" origin https://example.invalid/repo.git 2>&1)
	status=$?
	set -e
	if [[ "$status" -ne 0 ]]; then
		echo "expected allow but command blocked: $label" >&2
		echo "$output" >&2
		exit 1
	fi
}

expect_block "empty stdin from local main" ""
expect_block "main to main" $'refs/heads/main 1111111111111111111111111111111111111111 refs/heads/main 2222222222222222222222222222222222222222\n'
expect_block "main to feature" $'refs/heads/main 1111111111111111111111111111111111111111 refs/heads/feature/main-copy 2222222222222222222222222222222222222222\n'

"${git_clean[@]}" -C "$repo" checkout -q -b feature/pre-push-guard
expect_allow "feature to feature" $'refs/heads/feature/pre-push-guard 1111111111111111111111111111111111111111 refs/heads/feature/pre-push-guard 2222222222222222222222222222222222222222\n'
expect_block "feature to remote main" $'refs/heads/feature/pre-push-guard 1111111111111111111111111111111111111111 refs/heads/main 2222222222222222222222222222222222222222\n'

# The shape that actually reached the guard, and that every case above misses. Each `$'...\n'`
# literal above ends in a newline, so `read` always saw a terminated line and the loop always ran.
# scripts/hooks/pre-push sent something else: it captures git's stdin with `pushed=$(cat)` (which
# strips the trailing newline) and replayed it. `read` returns non-zero at EOF-without-delimiter,
# bash skips the body, and on a single-ref push -- the only line there is -- this guard saw an
# empty stream and allowed `git push origin HEAD:refs/heads/main` from a feature branch. Measured
# 2026-08-31. Same rows as the two directly above, minus the final newline.
expect_block "feature to remote main, UNTERMINATED final line" 'refs/heads/feature/pre-push-guard 1111111111111111111111111111111111111111 refs/heads/main 2222222222222222222222222222222222222222'
expect_allow "feature to feature, UNTERMINATED final line" 'refs/heads/feature/pre-push-guard 1111111111111111111111111111111111111111 refs/heads/feature/pre-push-guard 2222222222222222222222222222222222222222'

# And the multi-line form, where only the last row is the dangerous one: the earlier rows are
# terminated and would be read even by the broken loop, so this fails only if the fix is absent.
expect_block "trailing main row after a terminated feature row" $'refs/heads/feature/pre-push-guard 1111111111111111111111111111111111111111 refs/heads/feature/pre-push-guard 2222222222222222222222222222222222222222\nrefs/heads/feature/pre-push-guard 1111111111111111111111111111111111111111 refs/heads/main 2222222222222222222222222222222222222222'

printf 'git pre-push main guard tests passed\n'
