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

# `|| [[ -n ... ]]` is load-bearing, not defensive padding. `read` returns non-zero when it hits
# EOF without a delimiter, and bash then skips the loop body -- so a final line with no trailing
# newline is silently dropped. scripts/hooks/pre-push fed exactly that shape: it captures git's
# stdin with `pushed=$(cat)`, which strips the trailing newline, and replayed it with
# `printf '%s'`. For a single-ref push -- the normal case -- that one line was the only line, so
# this loop saw nothing and `git push origin HEAD:refs/heads/main` from a feature branch walked
# straight through. Measured 2026-08-31. The wrapper now sends `printf '%s\n'`, but the guard
# must not depend on being fed politely: it is the last thing between an agent and main.
while read -r local_ref _local_sha remote_ref _remote_sha || [[ -n "${local_ref:-}" ]]; do
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
