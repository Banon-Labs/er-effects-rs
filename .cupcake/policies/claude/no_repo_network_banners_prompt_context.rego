# METADATA
# scope: package
# title: Inject concise banner boundary reminder
# description: Remind agents that loud user-visible banners are only for live UI/runtime launch/attach/input/teardown, not git/network/repo operations.
# custom:
#   severity: LOW
#   id: ER-EFFECTS-NO-REPO-NETWORK-BANNERS-PROMPT-CONTEXT
#   routing:
#     required_events: ["UserPromptSubmit"]
package cupcake.policies.no_repo_network_banners_prompt_context

import rego.v1

add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	context := "Loud user-visible warning banners are only for live UI/runtime launch, attach, input, or teardown operations. Do not emit loud banners for git, GitHub, Beads/Dolt sync, ordinary network, repository, commit, pull, push, status, or validation operations; just run the command and report only meaningful results."
}
