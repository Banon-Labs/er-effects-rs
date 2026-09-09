# OPA unit tests for gh_pr_title_conventional.
# Run with:
#   opa test .cupcake/policies/claude/gh_pr_title_conventional.rego \
#     .cupcake/tests/gh_pr_title_conventional_test.rego
package cupcake.policies.claude.gh_pr_title_conventional_test

import rego.v1

import data.cupcake.policies.claude.gh_pr_title_conventional as guard

RULE := "ER-EFFECTS-PR-TITLE-CONVENTIONAL"

bash_event(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd},
}

rule_ids(denials) := {d.rule_id | some d in denials}

denied(cmd) if {
	denials := guard.deny with input as bash_event(cmd)
	RULE in rule_ids(denials)
}

# --- the case this exists for -------------------------------------------------

# The exact title CI refused on 2026-09-09.
test_deny_the_title_ci_actually_refused if {
	denied(`gh pr create --draft --base main --title "Cancel a local invasion that lands in the wrong place" --body-file /tmp/b.md`)
}

test_deny_on_edit_too if {
	denied(`gh pr edit 421 --title "make the thing work"`)
}

test_deny_a_single_quoted_title if {
	denied(`gh pr create --title 'no type here'`)
}

test_deny_a_bare_title if {
	denied("gh pr create --title untyped")
}

# A type that is not on the list is not a type. `feature:` is the near miss worth naming.
test_deny_a_near_miss_type if {
	denied(`gh pr create --title "feature(x): close but not a listed type"`)
}

# A colon alone is not the shape either.
test_deny_a_colon_without_a_type if {
	denied(`gh pr create --title "invasion warp: cancel a local invasion"`)
}

test_deny_a_type_with_no_description if {
	denied(`gh pr create --title "feat:"`)
}

# --- what it must accept ------------------------------------------------------

test_allow_a_plain_type if {
	not denied(`gh pr create --title "feat: cancel a local invasion that lands in the wrong place"`)
}

test_allow_a_scope if {
	not denied(`gh pr create --title "fix(er-invasion-warp): re-pin the ersc.dll entry points"`)
}

test_allow_a_breaking_marker if {
	not denied(`gh pr create --title "chore!: drop the 1.16.2 address table"`)
}

test_allow_a_scoped_breaking_marker if {
	not denied(`gh pr create --title "feat(invasion-warp)!: change the config shape"`)
}

# --- what it must not reach ---------------------------------------------------

test_allow_a_pr_command_with_no_title if {
	not denied("gh pr create --draft --fill")
}

test_allow_a_pr_view if {
	not denied(`gh pr view 421 --json title`)
}

test_allow_a_pr_checks_read if {
	not denied("gh pr checks 421")
}

# A commit subject is a different gate's business; this one is only about PR titles.
test_allow_a_git_commit_with_a_bare_subject if {
	not denied(`git commit -m "no type here"`)
}

# The word appearing in prose is not the command.
test_allow_prose_mentioning_it if {
	not denied(`echo "remember to gh pr create later"`)
}

# --- non-vacuity --------------------------------------------------------------

test_the_allow_cases_are_not_vacuous if {
	not denied(`gh pr create --title "feat: a typed subject"`)
	denied(`gh pr create --title "an untyped subject"`)
}
