# OPA unit tests for edit_no_comment_caps_guard.
#
# Run with:
#   opa test .cupcake/policies/claude/edit_no_comment_caps_guard.rego \
#            .cupcake/tests/edit_no_comment_caps_guard_test.rego
#
# The negative cases are the load-bearing half. This guard refuses an edit, so a false positive
# costs the author a write they cannot make; every shape it must NOT fire on is pinned below.
package cupcake.policies.claude.edit_no_comment_caps_guard_test

import rego.v1

import data.cupcake.policies.claude.edit_no_comment_caps_guard as guard

write_event(path, content) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Write",
	"tool_input": {"file_path": path, "content": content},
}

edit_event(path, new_string) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Edit",
	"tool_input": {"file_path": path, "old_string": "x", "new_string": new_string},
}

multiedit_event(path, strings) := {
	"hook_event_name": "PreToolUse",
	"tool_name": "MultiEdit",
	"tool_input": {
		"file_path": path,
		"edits": [{"old_string": "x", "new_string": s} | some s in strings],
	},
}

rule_ids(denials) := {d.rule_id | some d in denials}

denied(event) if {
	denials := guard.deny with input as event
	"ER-EFFECTS-COMMENT-CAPS-GUARD" in rule_ids(denials)
}

# --- it fires on shouting, in every authoring tool -------------------------------------------

test_deny_rust_line_comment if {
	denied(write_event("crates/x/src/lib.rs", "// this is NOT the same pointer\n"))
}

test_deny_rust_doc_comment if {
	denied(write_event("crates/x/src/lib.rs", "/// the ONLY writer of the field\n"))
}

test_deny_python_comment if {
	denied(write_event("scripts/x.py", "# EVERY caller must hold the lock\n"))
}

test_deny_shell_comment if {
	denied(write_event("scripts/x.sh", "# BEFORE the launch, check the profile\n"))
}

test_deny_edit_new_string if {
	denied(edit_event("crates/x/src/lib.rs", "// it is ALWAYS installed\n"))
}

test_deny_multiedit_second_edit if {
	denied(multiedit_event("crates/x/src/lib.rs", ["// fine\n", "// it is NEVER installed\n"]))
}

test_deny_line_buried_in_a_longer_write if {
	denied(write_event(
		"crates/x/src/lib.rs",
		"fn f() {}\n\n// a paragraph that runs\n// on for a WHILE and then shouts THIS\nfn g() {}\n",
	))
}

test_reason_names_the_file_and_the_line if {
	denials := guard.deny with input as write_event("crates/x/src/lib.rs", "// the ONLY writer\n")
	some d in denials
	contains(d.reason, "crates/x/src/lib.rs")
	contains(d.reason, "// the ONLY writer")
}

# --- and NOT on any of these ------------------------------------------------------------------

test_allow_lowercase_prose if {
	not denied(write_event("crates/x/src/lib.rs", "// this is not the same pointer\n"))
}

test_allow_backticked_name if {
	not denied(write_event("crates/x/src/lib.rs", "// the x86 `NOT` instruction, quoted\n"))
}

test_allow_quoted_status_string if {
	not denied(write_event("scripts/x.sh", "# prints \"NOT RUN\" and exits\n"))
}

test_allow_acronyms if {
	not denied(write_event("crates/x/src/lib.rs", "// the DLL exports RVA 0x140 via its ABI\n"))
}

test_allow_x86_mnemonics if {
	not denied(write_event("crates/x/src/lib.rs", "// AND RAX, RCX then CALL the thunk; TEST and PUSH follow\n"))
}

test_allow_underscored_symbol if {
	not denied(write_event("crates/x/src/lib.rs", "// SAVE_OWNS_THE_SLOT is set by the caller\n"))
}

test_allow_code_line_that_is_not_a_comment if {
	not denied(write_event("crates/x/src/lib.rs", "let s = \"THE ONLY WRITER\";\n"))
}

test_allow_generated_source_inside_a_string_literal if {
	not denied(write_event("crates/x/build.rs", "    \"// TWO tables, because THIS is generated\\n\\\n"))
}

test_allow_vendored_source if {
	not denied(write_event("third_party/hudhook/src/util.rs", "// this is NOT the same pointer\n"))
}

test_allow_unscanned_file_type if {
	not denied(write_event("docs/notes.md", "// this is NOT the same pointer\n"))
}

test_allow_other_events if {
	not denied({
		"hook_event_name": "PostToolUse",
		"tool_name": "Write",
		"tool_input": {"file_path": "crates/x/src/lib.rs", "content": "// it is NOT live\n"},
	})
}

test_allow_read_tool if {
	not denied({
		"hook_event_name": "PreToolUse",
		"tool_name": "Read",
		"tool_input": {"file_path": "crates/x/src/lib.rs"},
	})
}
