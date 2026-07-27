#!/usr/bin/env bash
set -euo pipefail

remote_name=${1:-unknown}
remote_url=${2:-unknown}
blocked=0

while read -r local_ref _local_sha remote_ref _remote_sha; do
	case "$remote_ref" in
		refs/heads/main|main)
			cat >&2 <<EOF
ER-EFFECTS-BLOCK-MAIN-PUSH: refusing direct push to main.
remote: ${remote_name} (${remote_url})
local_ref: ${local_ref}
remote_ref: ${remote_ref}

Push a feature/tooling branch instead and update main through the review/merge path.
EOF
			blocked=1
			;;
	esac
done

if [[ "$blocked" != 0 ]]; then
	exit 1
fi
