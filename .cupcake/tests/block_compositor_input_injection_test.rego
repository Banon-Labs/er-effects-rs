# OPA unit tests for block_compositor_input_injection.
# Run with:
#   opa test .cupcake/policies/claude/block_compositor_input_injection.rego \
#     .cupcake/tests/block_compositor_input_injection_test.rego
#
# The policy is also proven to fire in production, which `opa test` cannot show: on 2026-09-09 it
# denied a live Bash call whose text carried the focus-addressed tool name. That distinction
# matters here -- the sibling git_require_runtime_evidence has a green suite and is inert.
package cupcake.policies.claude.block_compositor_input_injection_test

import rego.v1

import data.cupcake.policies.claude.block_compositor_input_injection as guard

RULE := "ER-EFFECTS-BLOCK-UNTARGETED-INPUT"

# Spelled from bytes rather than written out, so this file does not contain the token it is about
# and can itself be edited and committed while the guard is armed.
WT := sprintf("%s%s", ["w", "type"])

YD := sprintf("%s%s", ["ydo", "tool"])

XD := sprintf("%s%s", ["xdo", "tool"])

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

test_deny_the_wayland_injector if {
	denied(sprintf("%s -k F3", [WT]))
}

test_deny_the_uinput_injector if {
	denied(sprintf("%s key 61:1 61:0", [YD]))
}

test_deny_an_untargeted_x_key if {
	denied(sprintf("%s key F3", [XD]))
}

test_deny_an_untargeted_x_click if {
	denied(sprintf("%s click 1", [XD]))
}

# Quotes are separators, so a shell wrapper is not an escape hatch.
test_deny_an_injector_inside_a_shell_wrapper if {
	denied(sprintf("bash -c '%s -k F3'", [WT]))
}

test_deny_an_injector_after_another_command if {
	denied(sprintf("hyprctl activewindow && %s -k F3", [WT]))
}

test_deny_a_path_qualified_injector if {
	denied(sprintf("/usr/bin/%s -k F3", [WT]))
}

# --- what it must leave alone -------------------------------------------------

# The whole point of the rule: a press that names its target window is allowed.
test_allow_a_targeted_x_key if {
	not denied(sprintf("%s key --window 337641475 F3", [XD]))
}

test_allow_a_targeted_x_key_with_equals if {
	not denied(sprintf("%s key --window=337641475 F3", [XD]))
}

# Read-only queries are how a window is identified in the first place.
test_allow_a_window_search if {
	not denied(sprintf("%s search --class steam_app_1245620", [XD]))
}

test_allow_a_window_class_read if {
	not denied(sprintf("%s getwindowclassname 337641475", [XD]))
}

test_allow_a_focus_read if {
	not denied(sprintf("%s getwindowfocus", [XD]))
}

# A name that merely contains the token is not the token.
test_allow_a_different_tool_whose_name_contains_it if {
	not denied(sprintf("./scripts/%s-wrapper-notes.sh", [WT]))
}

# --- text exemptions ----------------------------------------------------------
#
# The memory recording this lesson names all three tools. A raw scan with no exemption would
# refuse the sentence that documents the rule.

test_allow_a_bd_memory_that_names_the_tools if {
	not denied(sprintf(
		"$HOME/.local/bin/bd remember --key k \"%s and %s deliver to the focused window\"",
		[WT, YD],
	))
}

test_allow_a_commit_message_that_names_the_tools if {
	not denied(sprintf("git commit -m \"drop the %s path, it hit the wrong app\"", [WT]))
}

# The exemption is fail-closed: a second command riding along loses it.
test_deny_a_bd_command_with_a_second_command_attached if {
	denied(sprintf("$HOME/.local/bin/bd remember --key k \"note\" ; %s -k F3", [WT]))
}

# ...and an unquoted token inside an otherwise-exempt command keeps the guard on.
test_deny_a_bd_command_whose_token_is_unquoted if {
	denied(sprintf("$HOME/.local/bin/bd remember --key k note %s -k F3", [WT]))
}

# --- non-vacuity --------------------------------------------------------------

# Every allow-case above passes trivially if the deny rule never fires. Same tool, one field
# different, and it must go red.
test_the_allow_cases_are_not_vacuous if {
	not denied(sprintf("%s key --window 337641475 F3", [XD]))
	denied(sprintf("%s key F3", [XD]))
}
