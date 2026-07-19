# METADATA
# scope: package
# title: Ban authority-coded agreement + unbacked feedback-acknowledgement prose
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-AUTHORITY-AGREEMENT
#   description: >-
#     User directives 2026-07-17. Category A -- authority-coded agreement ("You're right", "Correct,",
#     "Exactly,", "Absolutely,", "That's right") -- is banned OUTRIGHT. Category B --
#     feedback-acknowledgement / receipt-announcement prose ("Point taken", "Got it", "Understood",
#     "Noted", "Fair point", "Makes sense", ...) -- is banned ONLY when the same turn did NOT record a
#     beads memory (a Bash tool_use running `bd remember`): announcing that you internalized feedback is
#     acceptable only when you actually internalized it durably. The signal returns AUTH:<phrase> for a
#     Category-A hit and ACKUNBACKED:<phrase> for an unbacked Category-B hit; both HALT turn-end so the
#     agent must correct. rego cannot pre-filter prose (no pre-response hook), so this is
#     reinforce-every-turn + halt-and-correct.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_authority_agreement"]
package cupcake.policies.claude.no_authority_agreement

import rego.v1

# Enforcement: block turn-end when the just-emitted assistant turn used banned phrasing.
# (The every-turn reminder lives in no_authority_agreement_reminder.rego -- cupcake forbids a
# Stop-routed policy from also emitting add_context.)
halt contains decision if {
	input.hook_event_name == "Stop"
	some h in [hit]
	decision := {
		"rule_id": "ER-EFFECTS-NO-AUTHORITY-AGREEMENT",
		"reason": reason_for(h),
		"severity": "HIGH",
	}
}

# Correction directive per case.
reason_for(h) := msg if {
	h.case == "AUTH"
	msg := sprintf("Banned authority-coded agreement detected in your reply: '%s'. Per the 2026-07-17 directive this phrasing is forbidden. Record a bd memory noting this slip, then send a corrected reply that removes the phrase and instead states the verified fact plus its proof (or simply proceeds).", [h.phrase])
}

reason_for(h) := msg if {
	h.case == "ACK"
	msg := sprintf("Banned feedback-acknowledgement prose detected in your reply: '%s' -- and this turn recorded NO beads memory. Per the 2026-07-17 directive, announcing that you received/internalized feedback is only acceptable when you actually internalized it: record a `bd remember` memory of the feedback in THIS turn (then the acknowledgement is fine), or send a corrected reply that drops the receipt-prose and simply proceeds.", [h.phrase])
}

# Parse the tagged signal into {case, phrase}. Untagged-but-non-empty falls back to a Category-A hit
# so a bare/crafted "You're right" value still halts. Empty -> hit undefined -> no halt.
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
