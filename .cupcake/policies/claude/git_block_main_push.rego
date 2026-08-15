# METADATA
# scope: package
# title: Block Direct Pushes to Main
# authors: ["er-effects-rs agents"]
# custom:
#   severity: CRITICAL
#   id: ER-EFFECTS-BLOCK-MAIN-PUSH
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["current_branch", "worktree_branches"]
package cupcake.policies.claude.git_block_main_push

import rego.v1

# Agents must not push directly to main. Work lands on feature/tooling branches;
# main is updated only through the user's preferred review/merge path. If the
# branch signal is missing, fail closed for push commands because bare `git push`
# inherits the current branch/refspec from Git config.
#
# Worktree-target exception (2026-07-30, bd
# guard-blocks-worktree-commits-from-main-session-cwd-2026-07-29): the
# current_branch signal reads the session checkout's branch, so a push issued as
# `git -C <worktree> push` from a session whose primary checkout sits on main was
# denied even when the worktree is on a feature branch. Such a push is allowed
# when every push invocation targets a registered non-main worktree.
#
# Explicit-refspec exception (2026-08-15): the same stale signal can report main
# when this session itself is rooted in a non-main worktree. A command that names
# a non-main upstream destination (`git push -u origin feature/name`) is safe to
# permit: it cannot update remote main. Bare pushes and unparsed options remain
# fail-closed. Explicit main refspecs (push_targets_main) stay denied
# unconditionally.
deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	cmd := input.tool_input.command
	is_git_push(lower(cmd))
	blocked_push_context(cmd)

	decision := {
		"rule_id": "ER-EFFECTS-BLOCK-MAIN-PUSH",
		"reason": "Do not push directly to main. Push a feature/tooling branch and let main update through the review/merge path.",
		"severity": "CRITICAL",
	}
}

blocked_push_context(cmd) if {
	current_branch == "main"
	not pushes_target_only_nonmain_worktrees(cmd)
	not pushes_target_only_explicit_nonmain_branches(cmd)
}

blocked_push_context(cmd) if {
	current_branch == ""
	not pushes_target_only_nonmain_worktrees(cmd)
	not pushes_target_only_explicit_nonmain_branches(cmd)
}

blocked_push_context(cmd) if {
	push_targets_main(lower(cmd))
}

# Match a real git push invocation, including common global-option forms such
# as `git -C <repo> push`.
git_push_command_pattern := `(^|[;&|(
])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+push([ \t;&|)\n]|$)`

is_git_push(cmd) if {
	regex.match(git_push_command_pattern, cmd)
}

# Main-target forms we block even from a feature branch:
#   git push origin main
#   git push origin HEAD:main
#   git push origin feature:refs/heads/main
#   git -C repo push origin refs/heads/main
push_targets_main(cmd) if {
	regex.match(git_push_main_target_pattern, cmd)
}

git_push_main_target_pattern := `(?m)(^|[;&|]\s*|\n)\s*(command\s+)?git(\s+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|\s+)("[^"\n]*"|'[^'\n]*'|[^\s;&|()]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^\s;&|()]+))?))*\s+push(\s+[^;&|\n]*)?(\s|:)(refs/heads/)?main(\s|$|[;&|\n])`

current_branch := branch if {
	branch := trim(input.signals.current_branch, " \t\r\n")
} else := branch if {
	branch := trim(input.signals.current_branch.output, " \t\r\n")
} else := "" if {
	true
}

# --- Worktree-target exception helpers ---------------------------------------

# Case-insensitive find-all twin of git_push_command_pattern, applied to the
# RAW (unlowered) command so worktree path capitalization survives extraction.
git_push_findall_pattern := `(?i)(^|[;&|(\n])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+push([ \t;&|)\n]|$)`

# Strict single-target form the exception recognizes: `git -C <path> push`.
# Group 2 captures the path token (optionally quoted).
git_c_push_extract_pattern := `(?i)(^|[;&|(\n])\s*(?:command\s+)?git[ \t]+-C[ \t]+("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)[ \t]+push(?:[ \t;&|)\n]|$)`

# Every push invocation in the command must be the strict `git -C <path> push`
# form and every extracted path must be a registered worktree on a non-main
# branch. Explicit main refspecs are still denied by push_targets_main.
pushes_target_only_nonmain_worktrees(cmd) if {
	general := regex.find_all_string_submatch_n(git_push_findall_pattern, cmd, -1)
	strict := regex.find_all_string_submatch_n(git_c_push_extract_pattern, cmd, -1)
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

# --- Explicit non-main destination exception ---------------------------------

# Deliberately narrow: this only recognizes the conventional explicit-upstream
# form used to publish a feature branch. Other push option arrangements fail
# closed until they have their own parser and regression coverage.
git_push_explicit_branch_extract_pattern := `(?i)(^|[;&|(\n])\s*(?:command\s+)?git\s+push\s+(?:-u|--set-upstream)\s+("[^"\n]*"|'[^'\n]*'|[^\s;&|()\n]+)\s+("[^"\n]*"|'[^'\n]*'|[^\s;&|()\n]+)(?:\s|$|[;&|)\n])`

# A stale/missing branch signal must not block a command when every push in it
# has the explicit, non-main destination form above. The independent
# push_targets_main rule still denies `main`, `HEAD:main`, and
# `feature:refs/heads/main` unconditionally.
pushes_target_only_explicit_nonmain_branches(cmd) if {
	general := regex.find_all_string_submatch_n(git_push_findall_pattern, cmd, -1)
	explicit := regex.find_all_string_submatch_n(git_push_explicit_branch_extract_pattern, cmd, -1)
	count(general) > 0
	count(explicit) == count(general)
	every m in explicit {
		explicit_nonmain_destination(m[3])
	}
}

explicit_nonmain_destination(token) if {
	destination := trim(token, "\"'")
	destination != ""
	not startswith(destination, "-")
}
