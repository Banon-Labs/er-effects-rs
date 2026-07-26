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
#     TIGHTENED 2026-07-17: the signal also emits VERBOSEPAUSE:<n-chars> when a turn ends as a PURE
#     PAUSE (no substantive tool_use, not blocked on the user) whose message is LONG / multi-topic. A
#     genuinely blocked pause must be ONLY a short, precise statement of what it is blocked on; a long
#     "justified" hold is NOT acceptable. VERBOSEPAUSE halts under rule ER-EFFECTS-NO-VERBOSE-PAUSE
#     with a "be terse" correction, distinct from the IDLEHOLD "do work / justify" halt.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_idle_hold"]
package cupcake.policies.claude.idle_hold

import rego.v1

# Enforcement (VERBOSEPAUSE, tightened rule 2026-07-17): block turn-end when the just-emitted turn was
# a pure pause (no substantive work) that wrote a LONG message instead of a terse blocked-note. This is
# a distinct, stronger violation than IDLEHOLD -- its correction is "be terse", not "do work / justify".
halt contains decision if {
	input.hook_event_name == "Stop"
	some msg in [verbose_reason]
	decision := {
		"rule_id": "ER-EFFECTS-NO-VERBOSE-PAUSE",
		"reason": msg,
		"severity": "HIGH",
	}
}

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

verbose_reason := msg if {
	some c in [verbose_chars]
	msg := sprintf("You paused while blocked on a background task but wrote a long message (%s chars). When blocked, reply with ONLY a short, precise statement of what you're blocked on -- no status summaries, findings recaps, plans, or next-step narration. A long 'justified' hold is not acceptable.", [c])
}

# Parse a VERBOSEPAUSE:<n-chars> tag into the char count. Undefined when the signal is not a
# VERBOSEPAUSE hit -> verbose_reason undefined -> no verbose halt.
verbose_chars := c if {
	startswith(raw, "VERBOSEPAUSE:")
	c := trim(trim_prefix(raw, "VERBOSEPAUSE:"), " \t\r\n")
}

# Parse the tagged signal into the matched phrase. Untagged-but-non-empty falls back to the raw value
# so a bare/crafted signal still halts; a VERBOSEPAUSE tag is excluded from the fallback so it does not
# ALSO trip the idle-hold halt. Empty -> phrase undefined -> no halt.
phrase := p if {
	startswith(raw, "IDLEHOLD:")
	p := trim(trim_prefix(raw, "IDLEHOLD:"), " \t\r\n")
} else := p if {
	raw != ""
	not startswith(raw, "VERBOSEPAUSE:")
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
