# METADATA
# scope: package
# title: One paragraph of prose per turn, hard limit
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-WALL-OF-TEXT
#   description: >-
#     User directive 2026-08-21, stated absolutely: "I'll never. Repeat: NEVER read more than a
#     single paragraph of response text from you. Everything else is a skim."
#     Every word past the first paragraph is not merely unwanted, it is UNREAD. That makes long
#     prose actively worse than short prose: the agent believes it has communicated when it has
#     not, and any caveat, blocker or correction buried past paragraph one has effectively been
#     withheld from the person who needed it. Advisory reminders did not fix this -- the agent
#     wrote multi-paragraph answers for an entire session while notes telling it not to were in
#     context -- so it halts turn-end instead.
#     Structure is NOT prose: fenced code blocks, tables and list blocks are exempt in the signal,
#     because the objection is to READING, and those are scanned. One paragraph plus a table is
#     fine; two paragraphs are not.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_wall_of_text"]
package cupcake.policies.claude.wall_of_text

import rego.v1

halt contains decision if {
	input.hook_event_name == "Stop"
	some msg in [violation]
	decision := {
		"rule_id": "ER-EFFECTS-WALL-OF-TEXT",
		"reason": msg,
		"severity": "HIGH",
	}
}

signal := s if {
	s := input.signals.last_assistant_wall_of_text
	is_string(s)
} else := s if {
	s := input.signals.last_assistant_wall_of_text.output
}

violation := msg if {
	raw := trim_space(signal)
	startswith(raw, "WALLOFTEXT:")
	parts := split(raw, ":")
	count(parts) >= 3
	msg := sprintf("You wrote %s paragraphs of prose. The user reads ONE and skims the rest, so everything after the first paragraph was NOT read -- including anything important you put there. Rewrite the answer as a SINGLE paragraph. If it will not fit, that is the signal to cut content, not to add a paragraph: lead with the answer, drop the reasoning the user did not ask for, and put any remaining structure in a table or code block (those are exempt). First paragraph began: %s", [parts[1], concat(":", array.slice(parts, 2, count(parts)))])
}
