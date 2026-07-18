# METADATA
# scope: package
# title: Ban unjustified idle/hold announcements while a background task runs
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-IDLE-HOLD
#   description: >-
#     Persistent user directive 2026-07-17 (recurring Claude anti-pattern). Announcing that you are
#     IDLING / HOLDING / STANDING BY while a background task runs ("I'm holding", "holding for",
#     "standing by", "I'll wait for", "waiting for X before", "nothing to do but wait", "I'll pause
#     here", "let it run and wait") is banned UNLESS the SAME turn either (a) contains justification
#     prose "I would normally have <X> but <reason>", or (b) does substantive NON-OVERLAPPING work (an
#     Edit/Write/Agent tool_use, or a Bash call that is not a mere status/log peek). A wait genuinely
#     blocked on the user is excluded upstream in the signal. The signal returns IDLEHOLD:<phrase> for
#     an unjustified idle announcement; it HALTS turn-end so the agent pulls forward independent work
#     or states the justification. rego cannot pre-filter prose (no pre-response hook), so this is
#     reinforce-every-turn + halt-and-correct, mirroring no_authority_agreement.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_idle_hold"]
package cupcake.policies.claude.idle_hold

import rego.v1

# Enforcement: block turn-end when the just-emitted assistant turn announced an unjustified hold.
halt contains decision if {
	input.hook_event_name == "Stop"
	some p in [phrase]
	decision := {
		"rule_id": "ER-EFFECTS-NO-IDLE-HOLD",
		"reason": reason_for(p),
		"severity": "HIGH",
	}
}

reason_for(p) := msg if {
	msg := sprintf("You announced holding/idling ('%s') while a background task runs. Do productive NON-OVERLAPPING work now (RE a different function, prep the next fix, analyze prior logs, clean a gate) OR state explicitly 'I would normally have done <X> while waiting but didn't because <reason>'. Don't just wait.", [p])
}

# Parse the tagged signal into the matched phrase. Untagged-but-non-empty falls back to the raw value
# so a bare/crafted signal still halts. Empty -> phrase undefined -> no halt.
phrase := p if {
	startswith(raw, "IDLEHOLD:")
	p := trim(trim_prefix(raw, "IDLEHOLD:"), " \t\r\n")
} else := p if {
	raw != ""
	p := raw
}

raw := trim(matched_phrase, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_phrase := p if {
	p := input.signals.last_assistant_idle_hold
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_idle_hold.output
} else := ""
