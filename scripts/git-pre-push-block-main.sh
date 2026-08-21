#!/usr/bin/env bash
set -euo pipefail

remote_name=${1:-unknown}
remote_url=${2:-unknown}
blocked=0

block_push() {
	local reason=$1
	local local_ref=${2:-unknown}
	local remote_ref=${3:-unknown}
	cat >&2 <<EOF
ER-EFFECTS-BLOCK-MAIN-PUSH: refusing direct push involving main.
reason: ${reason}
remote: ${remote_name} (${remote_url})
local_ref: ${local_ref}
remote_ref: ${remote_ref}

Push a feature/tooling branch instead and update main through the review/merge path.
EOF
	blocked=1
}

current_branch=$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)
if [[ "$current_branch" == "main" ]]; then
	block_push "current checkout is local main" "refs/heads/main" "unknown"
fi

while read -r local_ref _local_sha remote_ref _remote_sha; do
	case "$local_ref" in
		refs/heads/main|main)
			block_push "local ref is main" "$local_ref" "$remote_ref"
			;;
	esac
	case "$remote_ref" in
		refs/heads/main|main)
			block_push "remote ref is main" "$local_ref" "$remote_ref"
			;;
	esac
done

if [[ "$blocked" != 0 ]]; then
	exit 1
fi
