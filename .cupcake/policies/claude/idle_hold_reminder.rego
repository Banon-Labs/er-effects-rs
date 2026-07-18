# METADATA
# scope: package
# title: Reinforce the no-idle-hold rule every turn (and catch interrupted-turn slips)
# authors: ["er-effects-rs agents"]
# custom:
#   severity: LOW
#   id: ER-EFFECTS-NO-IDLE-HOLD-REMINDER
#   description: >-
#     Companion to idle_hold (the Stop-event halt). Two jobs on UserPromptSubmit:
#     (1) inject the never-idle rule into every turn's context so the anti-pattern is avoided
#     pre-emptively; and (2) INTERLOCK BACKSTOP -- if the just-finished assistant turn announced an
#     unjustified hold but the Stop halt did not catch it (e.g. the user INTERRUPTED the turn, so no
#     Stop event fired), inject a HIGH-priority correction directive on the next prompt.
#     UserPromptSubmit always runs, even after an interrupt, so this closes the hole a Stop-only guard
#     leaves open. Mirrors no_authority_agreement_reminder.
#     TIGHTENED 2026-07-17: the standing reminder now also states the terse-when-blocked rule, and a
#     separate VERBOSEPAUSE interlock fires when the prior turn paused-while-blocked with a long
#     message (distinct "be terse" correction from the idle-hold interlock).
#   routing:
#     required_events: ["UserPromptSubmit"]
#     required_signals: ["last_assistant_idle_hold"]
package cupcake.policies.claude.idle_hold_reminder

import rego.v1

# (1) Standing every-turn reminder.
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	context := "NEVER IDLE-HOLD: do not announce that you are holding / standing by / waiting while a background task runs (\"I'm holding\", \"holding for\", \"standing by\", \"I'll wait for\", \"waiting for X before\", \"nothing to do but wait\", \"I'll pause here\") UNLESS the SAME turn either (a) does substantive non-overlapping work (edit/write, launch a subagent, or run a real command -- not just tail/cat/grep a log), or (b) states explicitly 'I would normally have done <X> while waiting but didn't because <reason>'. Default posture: while any background task runs, pull forward independent work (RE a different function, prep the next fix, analyze prior logs, clean a gate). AND when you genuinely ARE blocked (a background task is running and there is nothing non-overlapping to do), your pause message must be ONLY a SHORT, precise statement of what you're blocked on -- NO status summaries, findings recaps, plans, or next-step narration. A long 'justified' hold is NOT acceptable. A wait genuinely blocked on the user is fine. Turn-end is guarded: an unjustified hold OR a long blocked-pause message halts the stop and forces a correction."
}

# (2) Interlock backstop (VERBOSEPAUSE): the previous turn paused-while-blocked with a long message and
# the Stop halt missed it (interrupted turn). Distinct correction from the idle-hold interlock.
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	some c in [verbose_chars]
	context := verbose_interlock_for(c)
}

# (2b) Interlock backstop: the previous turn idled and the Stop halt missed it (interrupted turn).
add_context contains context if {
	input.hook_event_name == "UserPromptSubmit"
	some p in [phrase]
	context := interlock_for(p)
}

interlock_for(p) := msg if {
	msg := sprintf("INTERLOCK TRIPPED: your PREVIOUS turn announced an unjustified hold/idle ('%s') while a background task ran, and it was not caught at turn-end (the turn was likely interrupted, so no Stop halt fired). Before addressing the new request, do the non-overlapping work you skipped (RE a different function, prep the next fix, analyze prior logs, clean a gate) or state 'I would normally have done <X> while waiting but didn't because <reason>'. Do not just wait again.", [p])
}

verbose_interlock_for(c) := msg if {
	msg := sprintf("INTERLOCK TRIPPED: your PREVIOUS turn paused while blocked on a background task but wrote a long message (%s chars), and it was not caught at turn-end (the turn was likely interrupted, so no Stop halt fired). When blocked, your pause message must be ONLY a short, precise statement of what you're blocked on -- no status summaries, findings recaps, plans, or next-step narration. Keep the next blocked-pause terse.", [c])
}

# Parse a VERBOSEPAUSE:<n-chars> tag into the char count. Undefined when the signal is not a
# VERBOSEPAUSE hit -> verbose interlock undefined -> none.
verbose_chars := c if {
	startswith(raw, "VERBOSEPAUSE:")
	c := trim(trim_prefix(raw, "VERBOSEPAUSE:"), " \t\r\n")
}

# Parse the tagged signal into the matched phrase. Untagged-but-non-empty falls back to the raw value
# so a bare/crafted signal still trips the interlock; a VERBOSEPAUSE tag is excluded from the fallback
# so it trips only the verbose interlock. Empty -> phrase undefined -> none.
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
