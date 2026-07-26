# METADATA
# scope: package
# title: Reinforce the agreement/acknowledgement ban every turn (and catch interrupted-turn slips)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: LOW
#   id: ER-EFFECTS-NO-AUTHORITY-AGREEMENT-REMINDER
#   description: >-
#     Companion to no_authority_agreement (the Stop-event halt). Two jobs on UserPromptSubmit:
#     (1) inject the ban into every turn's context so the phrasing is avoided pre-emptively; and
#     (2) INTERLOCK BACKSTOP -- if the just-finished assistant turn used banned phrasing (Category A
#     authority-coded agreement, or Category B unbacked feedback-acknowledgement) but the Stop halt did
#     not catch it (e.g. the user INTERRUPTED the turn, so no Stop event fired), inject a HIGH-priority
#     mandatory-correction directive on the next prompt. UserPromptSubmit always runs, even after an
#     interrupt, so this closes the hole a Stop-only guard leaves open.
#   routing:
#     required_events: ["UserPromptSubmit"]
#     required_signals: ["last_assistant_authority_agreement"]
package cupcake.policies.claude.no_authority_agreement_reminder

import rego.v1

# (1) Standing every-turn reminder.
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	context := "BANNED PHRASING: (A) never use authority-coded agreement -- \"You're right\", \"You're correct\", \"That's right\", or sentence-initial \"Correct,\"/\"Exactly,\"/\"Absolutely,\"/\"Precisely,\" (banned outright). (B) never emit feedback-acknowledgement / receipt-announcement prose -- \"Point taken\", \"Got it\", \"Understood\", \"Noted\", \"Fair point\", \"Makes sense\", etc. -- UNLESS you actually internalize the feedback by recording a `bd remember` beads memory in the SAME turn. Do not open with agreement or a receipt to be agreeable. If the user's claim is verified against evidence in context, state the verified fact and its proof directly; if it is only plausible, say so and state what would prove it; otherwise just proceed. Turn-end is guarded: these halt the stop and force a correction (+ bd memory)."
}

# (2) Interlock backstop: the previous turn slipped and the Stop halt missed it (interrupted turn).
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	some h in [hit]
	context := interlock_for(h)
}

# Correction directive per case.
interlock_for(h) := msg if {
	h.case == "AUTH"
	msg := sprintf("INTERLOCK TRIPPED: your PREVIOUS turn used the banned authority-coded agreement '%s' and it was not caught at turn-end (the turn was likely interrupted, so no Stop halt fired). Before addressing the new request, acknowledge the slip in one line WITHOUT repeating the banned phrasing and restate the point as the verified fact plus its proof (or simply proceed). Do not use authority-coded agreement again.", [h.phrase])
}

interlock_for(h) := msg if {
	h.case == "ACK"
	msg := sprintf("INTERLOCK TRIPPED: your PREVIOUS turn emitted feedback-acknowledgement prose '%s' with NO beads memory recorded, and it was not caught at turn-end (the turn was likely interrupted, so no Stop halt fired). Announcing you internalized feedback is only acceptable when you actually did: before addressing the new request, either record a `bd remember` memory of that feedback now, or drop the receipt-prose. Do not emit unbacked acknowledgement prose again.", [h.phrase])
}

# Parse the tagged signal into {case, phrase}. Untagged-but-non-empty falls back to a Category-A hit
# so a bare/crafted "You're right" value still trips the interlock. Empty -> hit undefined -> none.
hit := h if {
	startswith(raw, "AUTH:")
	h := {"case": "AUTH", "phrase": trim(trim_prefix(raw, "AUTH:"), " \t\r\n")}
} else := h if {
	startswith(raw, "ACKUNBACKED:")
	h := {"case": "ACK", "phrase": trim(trim_prefix(raw, "ACKUNBACKED:"), " \t\r\n")}
} else := h if {
	raw != ""
	h := {"case": "AUTH", "phrase": raw}
}

raw := trim(matched_phrase, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_phrase := p if {
	p := input.signals.last_assistant_authority_agreement
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_authority_agreement.output
} else := ""
