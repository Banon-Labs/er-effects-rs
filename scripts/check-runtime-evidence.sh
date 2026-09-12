#!/usr/bin/env bash
# Refuse a push of code that runs inside ELDEN RING when nothing has run it.
#
# Called by scripts/hooks/pre-push before the gate suite. Run its selftest directly:
#
#     bash scripts/check-runtime-evidence.sh --selftest
#
# The question it asks is whether a run artifact exists whose DLL says it was built from the commit
# being pushed. The evidence is the first line every shell in this workspace writes to its own log:
#
#     build git=b6b459560dfa module=er_invasion_warp.dll base=0x... pe=0x... (2026-09-09T02:05:18Z)
#
# That sha is what ties a run to code, and a file timestamp does not. The first version of this
# check compared mtimes and answered "evidence present" for head 0e084240 on a log whose own first
# line read `build git=b6b45956` -- a DLL two commits older, still running and still writing, so its
# file was newer than the commit it could not possibly have executed. Reading a clock and calling it
# provenance is the exact mistake this exists to stop.
#
# Which directories hold those logs, and when a `+dirty` line still counts, are decided by
# `scripts/er-runtime-evidence.py` rather than here -- the same module the two cupcake signals read,
# so the pre-push hook and the tool-call guard cannot answer differently about one push. Its header
# carries the reasoning; the short version is that the game directory counts as well as the run root
# (a `~/Elden/launch.sh` run writes only there), and that a `+dirty` log is evidence only when a
# provenance record proves the shell's own dependency closure was committed.
#
# Three things it deliberately leaves alone:
#   * a push that changes no crate. Docs, scripts and policies have nothing for a run to prove, and
#     a guard that fires on everything is one the next agent overrides by reflex.
#   * whether the run went well. A run that executed the code and went badly is a fact worth
#     pushing with; a run that never executed it is not evidence of anything.
#   * a tree it cannot measure. No run root, no git, no readable log -- it says so and allows,
#     because a check that cannot see must not invent a verdict.
#
# The override is `ER_ALLOW_UNPROVEN_PUSH=1`, and it prints what is being waived.
set -uo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# evidence_for <tip sha> [changed-paths file] -> 0 evidence, 1 no evidence, 2 unmeasurable.
# Prints one line of prose on stdout and the carry-forward candidates on stderr, in the
# `candidate <sha> <where>` shape `main` parses below.
#
# The changed-paths file is how the scan learns what this push contains, which it needs before it
# can accept a `+dirty` log: such a log counts only when the shell that wrote it compiles every
# crate the push changes. Without the file no `+dirty` log is ever accepted, which is the safe
# direction for a caller that does not know its own diff.
evidence_for() {
	local tip="$1" changed_file="${2:-}"
	local out status
	if [ -n "$changed_file" ] && [ -f "$changed_file" ]; then
		out="$(python3 "$repo_root/scripts/er-runtime-evidence.py" --head "$tip" --exhaustive \
			<"$changed_file" 2>/dev/null)"
	else
		out="$(python3 "$repo_root/scripts/er-runtime-evidence.py" --head "$tip" --exhaustive \
			</dev/null 2>/dev/null)"
	fi
	status=$?
	printf '%s\n' "$out" | sed -n 's/^note //p' | head -1
	printf '%s\n' "$out" | grep '^candidate ' >&2 || true
	return "$status"
}

# A run proves the tip when the tip adds nothing cargo would compile on top of what ran.
#
# Without this the guard fires on its own author. The commit that added it changes only policy and
# a probe script, so no rebuild could produce a DLL different from the one already running -- and
# it was refused anyway, because the branch's cumulative diff touches crates/. A guard that demands
# a fifteen-minute rebuild-and-relaunch to publish a shell script is one the next agent overrides
# by reflex, which its own header warns against.
#
# The question is answered by scripts/er-change-scope.py, the same reverse-dependency walk the
# compile gate uses, rather than by a second opinion written here: `--rust-touched` exits 3 for
# "provably no cargo work required" and 0 otherwise, and it fails open -- a git failure, an
# unresolvable base, or any build input outside a single crate directory all answer 0, which keeps
# the refusal. So this can only ever forgive a diff that provably cannot change a DLL.
#
# One narrow exception on top of it, because that tool answers a different question than this one.
# `er-change-scope.py` decides what CI must RE-run, so it treats `.github/` as a build input and
# widens to everything -- correct there, since a workflow edit changes what CI compiles. It is the
# wrong answer here: a workflow file cannot end up inside a DLL. The refusal it produced was a
# commit that edits `.github/workflows/check.yml` and nothing else, told to rebuild and relaunch
# Elden Ring to prove a `curl` flag.
#
# So a diff confined to `.github/`, `.cupcake/` and `scripts/` also carries forward. The three are
# named explicitly rather than inferred, and the claim was checked rather than assumed: all nine
# build scripts in the workspace were read for the paths they open and the processes they spawn,
# and none reaches any of the three -- the only `Command::new` in any of them is `git`. The scope
# tool already forgives the latter two on their own; this exists for the diff that also touches
# `.github/`, which drags the whole answer to "everything".
#
# `docs/` is deliberately absent. `crates/er-game-base/build.rs` reads `docs/recon/*.tsv`, so a
# docs diff can change a DLL, which is why the safe set is a short measured list rather than a
# guess at what looks harmless.
# Split out so the selftest can assert the two halves separately: the second case below is only
# meaningful if the first one would have refused on its own.
scope_says_no_cargo_work() { # scope_says_no_cargo_work <base> <rev>
	local rc=0
	python3 scripts/er-change-scope.py --rust-touched --base "$1" --rev "$2" >/dev/null 2>&1 || rc=$?
	[ "$rc" -eq 3 ]
}

carried_forward() { # carried_forward <run build sha> <tip>
	scope_says_no_cargo_work "$1" "$2" && return 0

	local changed
	changed="$(git diff --name-only "$1".."$2" 2>/dev/null)" || return 1
	[ -n "$changed" ] || return 1
	# Every changed path must be under one of the safe roots. `grep -qv` finds the first that is
	# not, so its failure is what proves the diff is confined.
	! printf '%s\n' "$changed" | grep -qv '^\(\.github/\|\.cupcake/\|scripts/\)'
}

selftest() {
	local failures=0
	local tmp
	tmp="$(mktemp -d)"
	# Cleaned up explicitly at every exit below rather than with a `RETURN` trap. A `RETURN` trap
	# set here stays armed for the caller's own return, and `main` has no `tmp` -- so the selftest
	# passed and then killed the script with `tmp: unbound variable` on the way out.

	expect() { # expect <wanted rc> <description> ... runs evidence_for with the caller's env
		local wanted="$1" description="$2"
		shift 2
		"$@" >/dev/null 2>&1
		local got=$?
		if [ "$got" = "$wanted" ]; then
			printf '  ok    %s\n' "$description"
		else
			printf '  FAIL  %s (wanted rc %s, got %s)\n' "$description" "$wanted" "$got"
			failures=$((failures + 1))
		fi
	}

	# Every case names both log directories. Leaving the game directory unset would let this
	# machine's real logs into a fixture, and a test whose result depends on what someone last
	# launched proves nothing about the logic.
	mkdir -p "$tmp/nogame"

	# A run directory holds the logs; a file sitting loose in the run root is not a run. The
	# three fixtures below used to write their log straight into the root, where the scan skips
	# it, so they passed on an empty directory rather than on the case they name.
	mkdir -p "$tmp/runs/br-good"
	printf 'build git=deadbeef1234 module=er_invasion_warp.dll base=0x1 pe=0x2 (t)\nmore\n' \
		>"$tmp/runs/br-good/er-invasion-warp.log"
	ER_ME3_RUN_ROOT="$tmp/runs" ER_GAME_DIR="$tmp/nogame" \
		expect 0 "a log whose build sha is the commit counts as evidence" \
		evidence_for "deadbeef1234"
	ER_ME3_RUN_ROOT="$tmp/runs" ER_GAME_DIR="$tmp/nogame" \
		expect 0 "an abbreviated tip sha matches the log's full sha" \
		evidence_for "deadbeef"
	ER_ME3_RUN_ROOT="$tmp/runs" ER_GAME_DIR="$tmp/nogame" \
		expect 1 "a log from a different build is refused" \
		evidence_for "0e0842402b18"

	# The gap that refused a proven push on 2026-09-11: a run launched through
	# `~/Elden/launch.sh` writes its logs into the game directory and nowhere else, and only the
	# run root was read. Both count now, and the second case keeps the sha doing the deciding.
	mkdir -p "$tmp/game"
	printf 'build git=cafebabe5678 module=er_quit_rows.dll base=0x1 pe=0x2 (t)\nmore\n' \
		>"$tmp/game/er-quit-rows-debug.log"
	ER_ME3_RUN_ROOT="$tmp/nonexistent" ER_GAME_DIR="$tmp/game" \
		expect 0 "a clean game-directory log naming the commit counts as evidence" \
		evidence_for "cafebabe5678"
	ER_ME3_RUN_ROOT="$tmp/nonexistent" ER_GAME_DIR="$tmp/game" \
		expect 1 "a game-directory log built from a different sha is still refused" \
		evidence_for "0e0842402b18"

	mkdir -p "$tmp/dirty/br-dirty"
	printf 'build git=deadbeef1234+dirty module=er_invasion_warp.dll base=0x1 pe=0x2 (t)\n' \
		>"$tmp/dirty/br-dirty/er-invasion-warp.log"
	ER_ME3_RUN_ROOT="$tmp/dirty" ER_GAME_DIR="$tmp/nogame" \
		expect 1 "a dirty build with a matching sha is refused when nothing proves its closure" \
		evidence_for "deadbeef1234"

	# The mtime trap, as a regression: a file newer than the commit, from an older build.
	mkdir -p "$tmp/mtime/br-old"
	printf 'build git=aaaaaaaaaaaa module=er_invasion_warp.dll\n' >"$tmp/mtime/br-old/er-old.log"
	touch -d '+1 hour' "$tmp/mtime/br-old/er-old.log" 2>/dev/null ||
		touch "$tmp/mtime/br-old/er-old.log"
	ER_ME3_RUN_ROOT="$tmp/mtime" ER_GAME_DIR="$tmp/nogame" \
		expect 1 "a newer file from an older build is still refused" \
		evidence_for "deadbeef1234"

	ER_ME3_RUN_ROOT="$tmp/nonexistent" ER_GAME_DIR="$tmp/nonexistent" \
		expect 2 "with neither log directory present the answer is unmeasurable, not refused" \
		evidence_for "deadbeef1234"

	mkdir -p "$tmp/nobuild/br-nobuild"
	printf 'some other log\n' >"$tmp/nobuild/br-nobuild/er-thing.log"
	ER_ME3_RUN_ROOT="$tmp/nobuild" ER_GAME_DIR="$tmp/nogame" \
		expect 1 "a log with no build line is not evidence" \
		evidence_for "deadbeef1234"

	# The carry-forward, against this repository's own history rather than a fixture: the scope
	# tool needs real commits to diff. Skipped rather than failed when the pair is not present,
	# so a shallow clone or a rewritten branch does not turn into a red gate about nothing.
	local ran_sha tip_sha other_sha
	ran_sha=466e3dd4 # policy + hook only on top of it
	tip_sha=63525d77
	other_sha=392b4b3c # a commit under crates/ sits between this one and the tip
	if git cat-file -e "$ran_sha^{commit}" 2>/dev/null &&
		git cat-file -e "$tip_sha^{commit}" 2>/dev/null &&
		git cat-file -e "$other_sha^{commit}" 2>/dev/null; then
		expect 0 "a run carries forward to a tip that adds no cargo work" \
			carried_forward "$ran_sha" "$tip_sha"
		expect 1 "a run does NOT carry forward across a change cargo compiles" \
			carried_forward "$other_sha" "$tip_sha"

		# The cupcake signal keeps its own copy of that decision, in python inside a heredoc, and
		# the plumbing that hands it the candidate list is easy to break in silence: a pipe into a
		# heredoc is swallowed, and the copy then answers `MISSING` having seen no candidates at all.
		# That is SC2259, caught by the linter on the way in. So the copy is driven here, over
		# the same fixture commits, because two enforcement points that disagree about one push
		# teach the next agent to ignore whichever is louder.
		local signal="$repo_root/.cupcake/signals/runtime_evidence_for_head.sh"
		if [ -f "$signal" ]; then
			sed -n "/<<'PY'/,/^PY\$/p" "$signal" | sed '1d;$d' >"$tmp/carry.py"
			signal_carries() { # signal_carries <tip> <candidate sha>
				[ "$(python3 "$tmp/carry.py" "$1" "$repo_root" "candidate $2 br-x/er-a.log")" = OK ]
			}
			expect 0 "the signal's copy carries forward from the same run" \
				signal_carries "$tip_sha" "$ran_sha"
			expect 1 "the signal's copy refuses across a change cargo compiles" \
				signal_carries "$tip_sha" "$other_sha"
			expect 1 "the signal's copy refuses when it was handed no candidates" \
				signal_carries "$tip_sha" ""
		fi
		# The `.github/` case, which the scope tool alone answers wrongly for this question: it
		# widens to everything on a workflow edit, so a commit touching only check.yml was told
		# to rebuild and relaunch the game to prove a `curl` flag.
		local workflow_only_sha workflow_base_sha
		workflow_only_sha=bd7cbc7b # ci: pin OPA -- .github/workflows/check.yml and nothing else
		workflow_base_sha=b69c1e79 # its parent, so the diff between them is that one file
		if git cat-file -e "$workflow_only_sha^{commit}" 2>/dev/null &&
			git cat-file -e "$workflow_base_sha^{commit}" 2>/dev/null; then
			expect 0 "a workflow-only commit carries forward" \
				carried_forward "$workflow_base_sha" "$workflow_only_sha"
			expect 1 "the scope tool alone would have refused it" \
				scope_says_no_cargo_work "$workflow_base_sha" "$workflow_only_sha"
		fi
	else
		printf '  skip  carry-forward (the fixture commits are not in this clone)\n'
	fi

	# A deletion under crates/ is unprovable, not unproven: the run it would ask for is a run of
	# the code being removed. Built as a throwaway repository rather than against this one's
	# history, because it has to contain a commit that deletes a crate and no such fixture pair is
	# guaranteed to be in every clone.
	local del="$tmp/deletion"
	mkdir -p "$del/crates/er-gone"
	(
		cd "$del" || exit 1
		git init -q . 2>/dev/null
		git config user.email selftest@example.invalid
		git config user.name selftest
		git config commit.gpgsign false
		printf 'fn main() {}\n' >crates/er-gone/lib.rs
		git add -A && git commit -qm "add the crate" --no-verify
		git rm -q -r crates/er-gone && git commit -qm "delete the crate" --no-verify
	) >/dev/null 2>&1
	if git -C "$del" rev-parse --verify --quiet HEAD >/dev/null 2>&1; then
		local deleted_paths kept_paths
		deleted_paths="$(git -C "$del" show --name-only --diff-filter=d --format= HEAD)"
		kept_paths="$(git -C "$del" show --name-only --format= HEAD)"
		expect 0 "a crates/ deletion leaves no path the gate would ask a run to prove" \
			bash -c '! printf "%s\n" "$1" | grep -q "^crates/"' _ "$deleted_paths"
		expect 0 "without the filter the same commit does look like changed game code" \
			bash -c 'printf "%s\n" "$1" | grep -q "^crates/"' _ "$kept_paths"
	else
		printf '  skip  deletion (a throwaway repository could not be created)\n'
	fi

	rm -rf "$tmp"
	if [ "$failures" -eq 0 ]; then
		printf 'check-runtime-evidence selftest: PASS\n'
		return 0
	fi
	printf 'check-runtime-evidence selftest: %d failure(s)\n' "$failures"
	return 1
}

main() {
	if [ "${1:-}" = "--selftest" ]; then
		selftest
		return $?
	fi

	command -v git >/dev/null 2>&1 || return 0
	git rev-parse --git-dir >/dev/null 2>&1 || return 0

	local tip
	tip="$(git rev-parse --short HEAD 2>/dev/null)" || return 0
	[ -n "$tip" ] || return 0

	# Only code that ends up inside the game can be proven by a run.
	#
	# `--diff-filter=d` (lowercase, an exclusion) drops deleted paths. A deletion cannot be proven
	# by a run, because the thing a run would exercise is the code being removed: asking for one
	# means asking for the DLL under deletion to be built and launched. Measured 2026-09-11 on
	# chore/remove-er-lockon-filter, which deletes crates/er-lockon-filter entirely -- every path
	# came back as a `D`, this gate demanded runtime evidence for it, and the only way past was the
	# override, which is meant for an unproven change rather than an unprovable one.
	local changed
	if git rev-parse --verify --quiet refs/remotes/origin/main >/dev/null 2>&1; then
		changed="$(git diff --name-only --diff-filter=d refs/remotes/origin/main...HEAD 2>/dev/null)"
	else
		changed="$(git show --name-only --diff-filter=d --format= HEAD 2>/dev/null)"
	fi
	if ! printf '%s\n' "$changed" | grep -q '^crates/'; then
		return 0
	fi

	local note rc candidates changed_file
	candidates="$(mktemp)"
	changed_file="$(mktemp)"
	printf '%s\n' "$changed" >"$changed_file"
	note="$(evidence_for "$tip" "$changed_file" 2>"$candidates")"
	rc=$?
	rm -f "$changed_file"

	# No log names the tip, but a run may still have executed the same code. Ask the scope tool,
	# newest run first, and take the first sha whose diff to the tip provably reaches no cargo.
	# The candidates arrive newest first, so they are read in the order they were written.
	local carried_sha="" carried_where=""
	if [ "$rc" -eq 1 ]; then
		local sha where
		while read -r _ sha where; do
			[ -n "$sha" ] || continue
			if carried_forward "$sha" "$tip"; then
				carried_sha="$sha"
				carried_where="$where"
				break
			fi
		done <"$candidates"
	fi
	rm -f "$candidates"
	if [ -n "$carried_sha" ]; then
		printf 'pre-push: runtime evidence for %s carried forward -- %s ran %s, and %s adds nothing cargo compiles on top of it\n' \
			"$tip" "$carried_where" "${carried_sha:0:8}" "$tip" >&2
		return 0
	fi

	if [ "$rc" -eq 0 ]; then
		printf 'pre-push: runtime evidence for %s -- %s\n' "$tip" "$note" >&2
		return 0
	fi
	if [ "$rc" -eq 2 ]; then
		printf 'pre-push: runtime evidence unmeasurable for %s -- %s. Allowing.\n' "$tip" "$note" >&2
		return 0
	fi

	if [ "${ER_ALLOW_UNPROVEN_PUSH:-}" = "1" ]; then
		printf 'pre-push: pushing unproven code by ER_ALLOW_UNPROVEN_PUSH=1 -- %s\n' "$note" >&2
		return 0
	fi

	{
		printf '\n'
		printf 'pre-push: REFUSING -- this push changes code under crates/ that has never run.\n'
		printf '  tip       %s\n' "$tip"
		printf '  evidence  %s\n' "$note"
		printf '\n'
		printf '  A DLL log names the commit it was built from on its first line, and no log names\n'
		printf '  this one -- neither under the run root nor in the game directory, both of which\n'
		printf '  were read. Build it, launch it, and let the DLL write:\n'
		printf '\n'
		printf '    bash scripts/er-build-dlls.sh --all\n'
		printf '    python3 scripts/er-run-branch.py --save <save>:<slot>\n'
		printf '\n'
		printf '  A launcher run counts too -- its logs land in the game directory rather than the\n'
		printf '  run root. Commit before you build: the sha is stamped at build time, so building\n'
		printf '  first leaves the artifact carrying the previous commit (scripts/er-ship.sh does\n'
		printf '  the ordering for you). A tree left dirty in some other crate is fine; a tree\n'
		printf '  dirty inside the shell under test is not, and the evidence line above says which.\n'
		printf '\n'
		printf '  Or push a commit that does not change game code.\n'
		printf '  Deliberate override, which says so in the log: ER_ALLOW_UNPROVEN_PUSH=1\n'
		printf '\n'
	} >&2
	return 1
}

main "$@"
