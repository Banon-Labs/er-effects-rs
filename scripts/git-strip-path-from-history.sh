#!/usr/bin/env bash
# Rewrite every commit in <base>..<ref> with a path removed from its tree, then
# move <ref> to the rewritten tip.
#
# Why this exists rather than `git filter-branch`/`git filter-repo`: the case this
# repo keeps hitting is a large binary committed by accident a few commits ago --
# 35 MB of `vendor-archive/seamless/ersc-*.dll` in #385, against a `.gitignore`
# rule that already covered them. filter-repo is not installed here and
# filter-branch rewrites whole refs; both are the wrong size of hammer for three
# commits at the tip, and filter-branch's own manual now tells you not to use it.
# This walks exactly the range you name, in topological order, rebuilding each
# commit with `git commit-tree` against a parent map -- so merge commits keep both
# parents and the history shape is byte-identical apart from the removed path.
#
# It does not touch the working tree: the files stay on disk, which is the point
# when the path is a gitignored local reference archive that must survive the
# untracking (AGENTS.md, `vendor-archive/seamless/`).
#
# Usage:  scripts/git-strip-path-from-history.sh <path> <base> [ref]
#   <path>  pathspec to remove (a file or a directory)
#   <base>  exclusive lower bound; commits after this are rewritten
#   [ref]   branch to move (default: the current branch)
#
# Prints the old and new tip. The old tip is not deleted, so `git reset --hard
# <old>` undoes the whole thing until the next `git gc --prune`.
set -euo pipefail

path=${1:?usage: git-strip-path-from-history.sh <path> <base> [ref]}
base=${2:?usage: git-strip-path-from-history.sh <path> <base> [ref]}
ref=${3:-$(git rev-parse --abbrev-ref HEAD)}

repo_root=$(git rev-parse --show-toplevel)
cd "$repo_root"

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  echo "refusing to rewrite: tracked changes present in the working tree" >&2
  exit 1
fi

old_tip=$(git rev-parse "$ref")
tmp_index=$(mktemp)
map_file=$(mktemp)
trap 'rm -f "$tmp_index" "$map_file"' EXIT

map_lookup() { # old sha -> new sha, or the old sha when it was not rewritten
  local want=$1 line
  while read -r line; do
    case "$line" in "$want "*) echo "${line#* }"; return;; esac
  done <"$map_file"
  echo "$want"
}

count=0
while read -r commit; do
  new_parents=()
  for parent in $(git log -1 --format=%P "$commit"); do
    new_parents+=(-p "$(map_lookup "$parent")")
  done

  rm -f "$tmp_index"
  GIT_INDEX_FILE=$tmp_index git read-tree "$commit"
  GIT_INDEX_FILE=$tmp_index git rm -r --cached --ignore-unmatch -q -- "$path" >/dev/null
  tree=$(GIT_INDEX_FILE=$tmp_index git write-tree)

  # commit-tree drops any GPG signature; a rewritten commit is a different object
  # and cannot carry the old one. Everything else -- author, committer, both
  # dates, the message verbatim -- is preserved.
  new=$(
    GIT_AUTHOR_NAME=$(git log -1 --format=%an "$commit") \
    GIT_AUTHOR_EMAIL=$(git log -1 --format=%ae "$commit") \
    GIT_AUTHOR_DATE=$(git log -1 --format=%aI "$commit") \
    GIT_COMMITTER_NAME=$(git log -1 --format=%cn "$commit") \
    GIT_COMMITTER_EMAIL=$(git log -1 --format=%ce "$commit") \
    GIT_COMMITTER_DATE=$(git log -1 --format=%cI "$commit") \
    git commit-tree "$tree" "${new_parents[@]}" -F <(git log -1 --format=%B "$commit")
  )
  printf '%s %s\n' "$commit" "$new" >>"$map_file"
  count=$((count + 1))
done < <(git rev-list --topo-order --reverse "$base..$ref")

new_tip=$(map_lookup "$old_tip")
if [ "$new_tip" = "$old_tip" ]; then
  echo "nothing rewritten (is $base..$ref empty?)" >&2
  exit 1
fi

git update-ref -m "strip $path from $base..$ref" "refs/heads/${ref#refs/heads/}" "$new_tip" "$old_tip"
# Index only: the working-tree copies must survive, because the usual reason to
# run this is that the path should have been gitignored all along.
git reset --mixed --quiet "$new_tip"

echo "rewrote $count commit(s)"
echo "old tip: $old_tip"
echo "new tip: $new_tip"
