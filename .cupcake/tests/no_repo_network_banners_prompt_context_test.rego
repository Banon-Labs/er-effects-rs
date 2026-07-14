package cupcake.policies.no_repo_network_banners_prompt_context_test

import rego.v1

import data.cupcake.policies.no_repo_network_banners_prompt_context as policy

prompt_event := {
	"hook_event_name": "UserPromptSubmit",
	"tool_name": "",
	"tool_input": {},
}

pretool_event := {
	"hook_event_name": "PreToolUse",
	"tool_name": "Bash",
	"tool_input": {"command": "git push"},
}

test_injects_no_repo_network_banner_context_on_prompt if {
	contexts := policy.add_context with input as prompt_event
	count(contexts) == 1
	some context in contexts
	contains(context, "Do not emit loud banners for git")
	contains(context, "Beads/Dolt sync")
	contains(context, "live UI/runtime launch")
}

test_does_not_inject_for_pretool_events if {
	contexts := policy.add_context with input as pretool_event
	count(contexts) == 0
}
