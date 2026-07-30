# METADATA
# scope: package
# title: Block Local Commits on Main
# authors: ["er-effects-rs agents"]
# custom:
#   severity: CRITICAL
#   id: ER-EFFECTS-BLOCK-MAIN-COMMIT
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["current_branch", "worktree_branches"]
package cupcake.policies.claude.git_block_main_commit

import rego.v1

# Never allow local commits while the active branch is main. Agents must create a
# feature/tooling branch from the intended base first, then commit there. If the
# branch signal is missing, fail closed: a missing signal caused a live main
# commit to slip through this guard on 2026-07-13.
#
# Worktree-target exception (2026-07-30, bd
# guard-blocks-worktree-commits-from-main-session-cwd-2026-07-29): the
# current_branch signal reads the branch of the session checkout, not the tree a
# `git -C <path> commit` actually targets, so worktree-based feature commits were
# denied whenever the main checkout sat on main. A command whose every commit
# invocation explicitly targets a registered git worktree on a non-main branch is
# not a main commit and is allowed. Anything unparsed, unregistered, detached, or
# bare (`git commit` without -C) keeps the deny (fail closed).
deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	is_git_commit(lower(input.tool_input.command))
	blocked_branch_context
	not commits_target_only_nonmain_worktrees(input.tool_input.command)

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-MAIN-COMMIT",
		"reason": "Do not commit unless the guard can confirm the active branch is not main. Create/switch to a feature or tooling branch based on the intended base (or target a registered non-main worktree explicitly: git -C <worktree-path> commit), and ensure the current_branch signal is available.",
		"severity": "CRITICAL",
	}
}

blocked_branch_context if {
	current_branch == "main"
}

blocked_branch_context if {
	current_branch == ""
}

# Match a real git commit invocation instead of any command text containing both
# words. This avoids false positives from shell comments, printf labels, and
# variable names such as archive_commit while still blocking direct git commit
# calls, including common global-option forms such as `git -C <repo> commit`.
git_commit_command_pattern := `(^|[;&|(\n])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+commit([ \t;&|)\n]|$)`

is_git_commit(cmd) if {
	regex.match(git_commit_command_pattern, cmd)
}

current_branch := branch if {
	branch := trim(input.signals.current_branch, " \t\r\n")
} else := branch if {
	branch := trim(input.signals.current_branch.output, " \t\r\n")
} else := "" if {
	true
}

# --- Worktree-target exception helpers ---------------------------------------

# Case-insensitive find-all twin of git_commit_command_pattern, applied to the
# RAW (unlowered) command so worktree path capitalization survives extraction.
git_commit_findall_pattern := `(?i)(^|[;&|(\n])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+commit([ \t;&|)\n]|$)`

# Strict single-target form the exception recognizes: `git -C <path> commit`.
# Group 2 captures the path token (optionally quoted). Any other global-option
# arrangement deliberately fails the strict match and keeps the deny.
git_c_commit_extract_pattern := `(?i)(^|[;&|(\n])\s*(?:command\s+)?git[ \t]+-C[ \t]+("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)[ \t]+commit(?:[ \t;&|)\n]|$)`

# Every commit invocation in the command must be the strict `git -C <path>
# commit` form (strict count == general count) and every extracted path must be
# a registered worktree on a non-main branch.
commits_target_only_nonmain_worktrees(cmd) if {
	general := regex.find_all_string_submatch_n(git_commit_findall_pattern, cmd, -1)
	strict := regex.find_all_string_submatch_n(git_c_commit_extract_pattern, cmd, -1)
	count(general) > 0
	count(strict) == count(general)
	every m in strict {
		worktree_target_ok(m[2])
	}
}

worktree_target_ok(token) if {
	path := trim_right(trim(token, "\"'"), "/")
	branch := worktree_branch(path)
	branch != "main"
	branch != ""
}

# Resolve a worktree path to its branch from `git worktree list --porcelain`
# output. Detached worktrees have no `branch ` line and resolve to nothing
# (fail closed).
worktree_branch(path) := branch if {
	lines := split(worktree_branches_signal, "\n")
	some i, j
	lines[i] == concat("", ["worktree ", path])
	j > i
	startswith(lines[j], "branch refs/heads/")
	not worktree_entry_between(lines, i, j)
	branch := trim_space(trim_prefix(lines[j], "branch refs/heads/"))
}

worktree_entry_between(lines, i, j) if {
	some k
	k > i
	k < j
	startswith(lines[k], "worktree ")
}

worktree_branches_signal := out if {
	out := input.signals.worktree_branches
	is_string(out)
} else := out if {
	out := input.signals.worktree_branches.output
} else := "" if {
	true
}
