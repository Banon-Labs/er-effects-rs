#!/usr/bin/env bash
# What does each scripts/check.sh step actually COST? Wall clock per step, as a TSV.
#
# The stage split in scripts/check-stages.py is supposed to be chosen by measurement rather than
# by intuition about which gates look slow. This is that measurement, kept runnable so the next
# person can re-take it instead of trusting a number in a pull request body.
#
# It reads the step list back out of check.sh through scripts/check-stages.py, so it can never
# drift behind the suite, and it runs each step the way check.sh runs it: `bash -c` with
# `repo_root` exported, cwd at the repo root. What it deliberately does NOT reproduce is check.sh's
# shims -- no step is skipped here for a missing input or an unrelated diff, because the question
# is what a step costs when it runs.
#
# Usage:
#   bash scripts/check-step-timings.sh <out.tsv> [stage|kind-filter]
#     all      every step (default)
#     nocargo  every step whose first word is not `cargo`, and not the two bash gates that build
#     cargo    only those
set -uo pipefail

repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
out=${1:?usage: check-step-timings.sh <out.tsv> [all|cargo|nocargo]}
filter=${2:-all}
export repo_root

printf 'line\tsecs\trc\ttext\n' >"$out"

# The step list, one record per line, tab separated. python3 only parses; it spawns nothing, so
# the 30-second subprocess cap in scripts/check-no-timeouts.py has nothing to bind here.
while IFS=$'\t' read -r line text; do
	[[ -z ${line:-} ]] && continue
	head=${text%% *}
	builds=0
	[[ $head == cargo ]] && builds=1
	[[ $text == *check-rust-build.sh* ]] && builds=1
	[[ $text == *check-committed-compiles.sh* ]] && builds=1
	case "$filter" in
	cargo) [[ $builds -eq 1 ]] || continue ;;
	nocargo) [[ $builds -eq 0 ]] || continue ;;
	esac
	start=$(date +%s%N)
	bash -c "$text" >/dev/null 2>&1
	rc=$?
	end=$(date +%s%N)
	secs=$(((end - start) / 1000000))
	printf '%s\t%s.%03d\t%s\t%.180s\n' "$line" "$((secs / 1000))" "$((secs % 1000))" "$rc" "$text" >>"$out"
	printf '%6d.%03ds rc=%-4s line %-5s %.100s\n' "$((secs / 1000))" "$((secs % 1000))" "$rc" "$line" "$text"
done < <(python3 "$repo_root/scripts/check-stages.py" --steps-tsv)
