# METADATA
# scope: package
# title: Ban authority-coded agreement ("You're right")
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-AUTHORITY-AGREEMENT
#   description: >-
#     User directive 2026-07-17: never use authority-coded agreement ("You're right", "Correct,",
#     "Exactly,", "Absolutely,", "That's right"). Reinforced into every turn's context, and enforced
#     at turn-end: if the just-emitted assistant message used the banned phrasing, HALT so the agent
#     must record a bd memory of the slip and revise the response with the phrase removed. rego cannot
#     pre-filter prose (no pre-response hook), so this is reinforce-every-turn + halt-and-correct.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_authority_agreement"]
package cupcake.policies.claude.no_authority_agreement

import rego.v1

# Enforcement: block turn-end when the just-emitted assistant message used the banned phrasing.
# (The every-turn reminder lives in no_authority_agreement_reminder.rego -- cupcake forbids a
# Stop-routed policy from also emitting add_context.)
halt contains decision if {
	input.hook_event_name == "Stop"
	phrase := trim(matched_phrase, " \t\r\n")
	phrase != ""

	decision := {
		"rule_id": "ER-EFFECTS-NO-AUTHORITY-AGREEMENT",
		"reason": sprintf("Banned authority-coded agreement detected in your reply: '%s'. Per the 2026-07-17 directive this phrasing is forbidden. Record a bd memory noting this slip, then send a corrected reply that removes the phrase and instead states the verified fact plus its proof (or simply proceeds).", [phrase]),
		"severity": "HIGH",
	}
}

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_phrase := p if {
	p := input.signals.last_assistant_authority_agreement
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_authority_agreement.output
} else := ""
