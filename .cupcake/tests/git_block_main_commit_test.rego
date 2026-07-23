# OPA unit tests for git_block_main_commit.
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/git_block_main_commit.rego \
#     .cupcake/tests/git_block_main_commit_test.rego
package cupcake.policies.claude.git_block_main_commit_test

import rego.v1

import data.cupcake.policies.claude.git_block_main_commit as guard

bash_event(cmd, branch) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"current_branch": branch},
}

bash_event_object_signal(cmd, branch) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"current_branch": {"output": branch, "exit_code": 0}},
}

bash_event_no_branch_signal(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {},
}

rule_ids(denials) := {d.rule_id | some d in denials}

test_deny_git_commit_on_main_string_signal if {
	denials := guard.deny with input as bash_event("git commit -m 'bad'", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_commit_on_main_object_signal if {
	denials := guard.deny with input as bash_event_object_signal("git commit --allow-empty -m bad", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_c_commit_on_main if {
	denials := guard.deny with input as bash_event("git -C \"$repo\" commit -m bad", "main\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_add_then_commit_on_main if {
	denials := guard.deny with input as bash_event("git add . && git commit -m bad", "main")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_dash_c_commit_on_main if {
	denials := guard.deny with input as bash_event("git -C /tmp/repo commit -m bad", "main")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_allow_git_branch_reset_with_commit_prose_on_main if {
	cmd := concat("\n", [
		"repo=/home/banon/projects/fromsoftware-rs",
		"archive_branch=archive/local-inputblocker-injected-key-20260617",
		"old_head=$(git -C \"$repo\" rev-parse HEAD)",
		"# Preserve the old commit under a named local branch before resetting main.",
		"git -C \"$repo\" branch \"$archive_branch\" \"$old_head\"",
		"git -C \"$repo\" reset --hard origin/main",
		"printf 'archive_commit=%s\\n' \"$old_head\"",
	])
	denials := guard.deny with input as bash_event(cmd, "main")
	count(denials) == 0
}

test_deny_git_commit_when_branch_signal_missing if {
	denials := guard.deny with input as bash_event_no_branch_signal("git commit -m 'bad'")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_c_commit_when_branch_signal_missing if {
	denials := guard.deny with input as bash_event_no_branch_signal("git -C /tmp/repo commit -m 'bad'")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_deny_git_commit_when_branch_signal_empty if {
	denials := guard.deny with input as bash_event("git commit -m 'bad'", "\n")
	"ER-EFFECTS-BLOCK-MAIN-COMMIT" in rule_ids(denials)
}

test_allow_git_commit_on_feature_branch if {
	denials := guard.deny with input as bash_event("git commit -m 'ok'", "feature/title-fadein-gfx-flash\n")
	count(denials) == 0
}

test_allow_non_commit_git_on_main if {
	denials := guard.deny with input as bash_event("git status --short && git log --oneline -3", "main\n")
	count(denials) == 0
}

test_allow_commit_word_without_git_on_main if {
	denials := guard.deny with input as bash_event("echo commit", "main\n")
	count(denials) == 0
}

test_allow_git_archive_reset_script_with_commit_comments_on_main if {
	cmd := `set -euo pipefail
repo=/home/banon/projects/fromsoftware-rs
archive_branch=archive/local-inputblocker-injected-key-20260617
old_head=$(git -C "$repo" rev-parse HEAD)
# Preserve the old commit under a named local branch before resetting main.
if git -C "$repo" show-ref --verify --quiet "refs/heads/$archive_branch"; then
  existing=$(git -C "$repo" rev-parse "$archive_branch")
  if [ "$existing" != "$old_head" ]; then
    echo "archive branch exists at different commit: $archive_branch $existing" >&2
    exit 1
  fi
else
  git -C "$repo" branch "$archive_branch" "$old_head"
fi
git -C "$repo" reset --hard origin/main
printf 'archive_branch=%s\n' "$archive_branch"
printf 'archive_commit=%s\n' "$old_head"
printf '\nstatus\n'
git -C "$repo" status --short --branch
printf '\nremotes\n'
git -C "$repo" remote -v
printf '\nrecent refs\n'
git -C "$repo" log --oneline --decorate --max-count=6 --graph --all --simplify-by-decoration`
	denials := guard.deny with input as bash_event(cmd, "main\n")
	count(denials) == 0
}

test_allow_git_operations_with_commit_variable_names_on_main if {
	cmd := "commit_hash=$(git rev-parse HEAD); git reset --hard origin/main; printf 'archive_commit=%s\\n' \"$commit_hash\""
	denials := guard.deny with input as bash_event(cmd, "main\n")
	count(denials) == 0
}
