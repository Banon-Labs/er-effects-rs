# METADATA
# scope: package
# title: Reinforce the authority-coded-agreement ban every turn (and catch interrupted-turn slips)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: LOW
#   id: ER-EFFECTS-NO-AUTHORITY-AGREEMENT-REMINDER
#   description: >-
#     Companion to no_authority_agreement (the Stop-event halt). Two jobs on UserPromptSubmit:
#     (1) inject the ban into every turn's context so the phrasing is avoided pre-emptively; and
#     (2) INTERLOCK BACKSTOP -- if the just-finished assistant turn actually used the banned phrasing
#     but the Stop halt did not catch it (e.g. the user INTERRUPTED the turn, so no Stop event fired),
#     inject a HIGH-priority mandatory-correction directive on the next prompt. UserPromptSubmit always
#     runs, even after an interrupt, so this closes the hole a Stop-only guard leaves open.
#   routing:
#     required_events: ["UserPromptSubmit"]
#     required_signals: ["last_assistant_authority_agreement"]
package cupcake.policies.claude.no_authority_agreement_reminder

import rego.v1

# (1) Standing every-turn reminder.
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	context := "BANNED PHRASING: never use authority-coded agreement -- \"You're right\", \"You're correct\", \"That's right\", or sentence-initial \"Correct,\"/\"Exactly,\"/\"Absolutely,\"/\"Precisely,\". Do not open with agreement to be agreeable. If the user's claim is verified against evidence in context, state the verified fact and its proof directly; if it is only plausible, say so and state what would prove it; otherwise just proceed. Turn-end is guarded: this phrasing halts the stop and forces a correction + bd memory."
}

# (2) Interlock backstop: the previous turn slipped and the Stop halt missed it (interrupted turn).
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	phrase := trim(matched_phrase, " \t\r\n")
	phrase != ""
	context := sprintf("INTERLOCK TRIPPED: your PREVIOUS turn used the banned authority-coded agreement '%s' and it was not caught at turn-end (the turn was likely interrupted, so no Stop halt fired). Before addressing the new request, acknowledge the slip in one line WITHOUT repeating the banned phrasing and restate the point as the verified fact plus its proof (or simply proceed). Do not use authority-coded agreement again.", [phrase])
}

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_phrase := p if {
	p := input.signals.last_assistant_authority_agreement
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_authority_agreement.output
} else := ""
