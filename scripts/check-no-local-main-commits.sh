#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

remote_ref=${1:-origin/main}
local_ref=${2:-main}

if ! git show-ref --verify --quiet "refs/remotes/$remote_ref"; then
  echo "missing remote baseline ref: $remote_ref" >&2
  echo "Run this check after fetching the intended baseline, or pass an explicit remote ref." >&2
  exit 2
fi

if ! git show-ref --verify --quiet "refs/heads/$local_ref"; then
  echo "local branch not present: $local_ref" >&2
  exit 0
fi

ahead_count=$(git rev-list --count "$remote_ref..$local_ref")
if [[ "$ahead_count" != "0" ]]; then
  echo "local '$local_ref' contains $ahead_count commit(s) not in '$remote_ref'." >&2
  echo "Do not commit on local main. Move those commits to a feature/tooling branch based on the intended base, then reset local main to $remote_ref." >&2
  git log --oneline "$remote_ref..$local_ref" >&2
  exit 1
fi

exit 0
