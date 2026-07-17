# METADATA
# scope: package
# title: Reinforce the authority-coded-agreement ban every turn
# authors: ["er-effects-rs agents"]
# custom:
#   severity: LOW
#   id: ER-EFFECTS-NO-AUTHORITY-AGREEMENT-REMINDER
#   description: >-
#     Companion to no_authority_agreement (the Stop-event halt). Injects the ban into every turn's
#     context so the phrasing is avoided pre-emptively; the Stop policy is the teeth if it slips.
#   routing:
#     required_events: ["UserPromptSubmit"]
package cupcake.policies.claude.no_authority_agreement_reminder

import rego.v1

add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	context := "BANNED PHRASING: never use authority-coded agreement -- \"You're right\", \"You're correct\", \"That's right\", or sentence-initial \"Correct,\"/\"Exactly,\"/\"Absolutely,\"/\"Precisely,\". Do not open with agreement to be agreeable. If the user's claim is verified against evidence in context, state the verified fact and its proof directly; if it is only plausible, say so and state what would prove it; otherwise just proceed. Turn-end is guarded: this phrasing halts the stop and forces a correction + bd memory."
}
