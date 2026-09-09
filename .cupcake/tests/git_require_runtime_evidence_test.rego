# OPA unit tests for git_require_runtime_evidence.
# Run with:
#   opa test .cupcake/system/commands.rego \
#     .cupcake/policies/claude/git_require_runtime_evidence.rego \
#     .cupcake/tests/git_require_runtime_evidence_test.rego
package cupcake.policies.claude.git_require_runtime_evidence_test

import rego.v1

import data.cupcake.policies.claude.git_require_runtime_evidence as guard

RULE := "ER-EFFECTS-REQUIRE-RUNTIME-EVIDENCE"

# The signal is a bare verdict word; the prose lives in the sibling `runtime_evidence_note` signal
# and is never branched on. See the policy header for why it is not one parsed line.
MISSING := "MISSING"

OK := "OK"

NOTRUNTIME := "NOTRUNTIME"

UNKNOWN := "UNKNOWN"

NOTE := "er-quickload-autoload-debug.log was built from b6b45956, not 0e084240"

bash_event(cmd, evidence) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"runtime_evidence_for_head": evidence, "runtime_evidence_note": NOTE},
}

bash_event_object_signal(cmd, evidence) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {"runtime_evidence_for_head": evidence, "runtime_evidence_note": NOTE},
}

bash_event_no_signal(cmd) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": cmd, "timeout": 30000},
	"signals": {},
}

rule_ids(denials) := {d.rule_id | some d in denials}

# --- the case this exists for -------------------------------------------------

test_deny_push_when_the_running_build_is_not_the_tip if {
	denials := guard.deny with input as bash_event("git push -u origin feat/x", MISSING)
	RULE in rule_ids(denials)
}

test_deny_still_fires_through_the_second_event_helper if {
	denials := guard.deny with input as bash_event_object_signal("git push", MISSING)
	RULE in rule_ids(denials)
}

# The note must reach the user intact: "no evidence" without "and here is what ran instead" is not
# actionable.
test_the_denial_names_what_actually_ran if {
	denials := guard.deny with input as bash_event("git push", MISSING)
	some d in denials
	contains(d.reason, "was built from b6b45956, not 0e084240")
}

test_deny_a_push_hidden_in_a_shell_wrapper if {
	denials := guard.deny with input as bash_event("bash -c 'git push -u origin feat/x'", MISSING)
	RULE in rule_ids(denials)
}

test_deny_a_push_after_another_command if {
	denials := guard.deny with input as bash_event("cargo fmt && git push", MISSING)
	RULE in rule_ids(denials)
}

test_deny_git_c_push if {
	denials := guard.deny with input as bash_event("git -C /tmp/repo push", MISSING)
	RULE in rule_ids(denials)
}

# --- the cases it must leave alone -------------------------------------------

test_allow_push_when_the_tip_is_what_ran if {
	denials := guard.deny with input as bash_event("git push -u origin feat/x", OK)
	not RULE in rule_ids(denials)
}

# A docs or policy push has nothing for a run to prove. A guard that fires on everything is one the
# next agent overrides by reflex, which is worse than not having it.
test_allow_push_when_no_crate_changed if {
	denials := guard.deny with input as bash_event("git push", NOTRUNTIME)
	not RULE in rule_ids(denials)
}

# UNKNOWN is not MISSING. A guard that cannot measure must not invent a verdict.
test_allow_push_when_the_signal_could_not_measure if {
	denials := guard.deny with input as bash_event("git push", UNKNOWN)
	not RULE in rule_ids(denials)
}

test_allow_push_when_the_signal_is_absent if {
	denials := guard.deny with input as bash_event_no_signal("git push")
	not RULE in rule_ids(denials)
}

# Anything that is not the exact word MISSING must allow, including a verdict this policy has
# never heard of. Fail-open on an unrecognised word is the same rule as UNKNOWN above.
test_allow_push_when_the_verdict_is_unrecognised if {
	denials := guard.deny with input as bash_event("git push", "something else entirely")
	not RULE in rule_ids(denials)
}

# Only a push. This guard has no opinion about any other git command, and saying so is what stops
# it growing into a general "did you test it" nag on commands that publish nothing.
test_allow_a_commit_with_no_evidence if {
	denials := guard.deny with input as bash_event("git commit -m x", MISSING)
	not RULE in rule_ids(denials)
}

test_allow_a_fetch_with_no_evidence if {
	denials := guard.deny with input as bash_event("git fetch origin", MISSING)
	not RULE in rule_ids(denials)
}

test_allow_a_status_with_no_evidence if {
	denials := guard.deny with input as bash_event("git status --short", MISSING)
	not RULE in rule_ids(denials)
}

# The word "push" in prose, or a path that contains it, is not a push. This is the false positive
# that made the sibling main-push guard route through commands.executed_texts.
test_allow_prose_mentioning_a_push if {
	denials := guard.deny with input as bash_event("echo 'remember to git push later'", MISSING)
	not RULE in rule_ids(denials)
}

test_allow_a_non_git_command_named_push if {
	denials := guard.deny with input as bash_event("./scripts/push-notes.sh", MISSING)
	not RULE in rule_ids(denials)
}

# --- non-vacuity --------------------------------------------------------------

# Every allow-case above passes trivially if the deny rule never fires at all. This is the control
# that proves the suite is measuring something: the same command, the same guard, one field
# different, and it must go red.
test_the_allow_cases_are_not_vacuous if {
	allowed := guard.deny with input as bash_event("git push -u origin feat/x", OK)
	denied := guard.deny with input as bash_event("git push -u origin feat/x", MISSING)
	not RULE in rule_ids(allowed)
	RULE in rule_ids(denied)
}
