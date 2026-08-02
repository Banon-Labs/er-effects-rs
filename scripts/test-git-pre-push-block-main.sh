#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
guard="$repo_root/scripts/git-pre-push-block-main.sh"
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/er-effects-pre-push-main-guard.XXXXXX")
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

printf 'git pre-push main guard tests passed\n'
