# METADATA
# scope: package
# title: Ban answering a challenge to your own choice by defending it
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION
#   description: >-
#     User directive 2026-09-10, after three consecutive turns of it. Verbatim:
#
#       user:      Do you crutch on 1.16.2 addreses for a specific reason?
#       assistant: a table of why the convention exists, closing "I'll ask the agent for the
#                  return-value contract rather than the 1.16.2 address". No file changed.
#       user:      Why on earth would I want that convention?
#       assistant: "You wouldn't -- and there's no need to invent a new field, because line 1022
#                  already does the right thing ...", then 160 words of rationale. No file changed.
#       user:      Right, but it exists. Why do you insist on going 'My real point <em-dash>
#                  massive amount of prose that is never worth reading' followed by me going 'Yes I
#                  understand that I wouldn't so now you're pausing on something I clearly want you
#                  to correct'
#
#     The third message is the user's own diagnosis and it is the specification: a question about a
#     convention the assistant chose IS a correction, and the assistant kept spending turns
#     justifying rather than fixing. Each explanation was accurate. None of them was wanted, and
#     the edit that should have replaced them took one tool call.
#
#     Why the neighbouring Stop guards do not already cover it, measured rather than assumed by
#     replaying the three turns through each signal:
#       * ER-EFFECTS-NO-STALL-ON-FRICTION exempts any turn whose opening prompt asked a question,
#         and all three prompts are questions. That exemption exists so a direct answer cannot be
#         gagged, and it is correct for that rule; it is also what makes this shape invisible to it.
#       * ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX needs the reply to name a defect. A defence of a
#         convention names none -- it says the convention is fine.
#       * ER-EFFECTS-NO-FUTURE-TENSE-COMMITMENT needs a first-person future promise of a class the
#         turn did not perform. Turn one closed on one, but the promised class was a read and the
#         turn had already read, so its own exemption cleared it.
#     The fact that convicts here is the conjunction none of them computes: a second-person
#     challenge in the prompt beside a turn that produced justification where it owed a diff.
#
#     The signal emits CHALLENGEFACTS. The conjunction lives here so it is unit-testable against
#     the verbatim corpus instead of hiding in shell regexes:
#       challenge -- the most recent user message aimed an interrogative or an evaluative remark at
#                    a choice the assistant made. Second person is the whole discriminator: "why do
#                    you crutch on ..." and "why on earth would I want ..." convict, while "why does
#                    the engine park the disconnect?" has a third-person subject and never matches.
#                    A factual question about the codebase must always be answerable.
#       defence   -- the closing prose is long enough to be an explanation rather than a reply
#                    (40 words) and carries a justification marker ("because", "the reason", "which
#                    is why", "the convention exists", "so that").
#       changed   -- the turn wrote something or dispatched an agent to. This is the main exemption
#                    and it is deliberately generous: any edit at all clears the rule, because a
#                    false "nothing changed" accuses a turn that did the work while a false
#                    "something changed" is a quiet non-event.
#       blocked   -- a dependency the agent cannot dissolve by working harder: a credential, sudo, a
#                    live game, a guard that refused the write, a tool that is not installed.
#                    Narrow on purpose. "I need X before I can do Y" was the stall in the real
#                    transcript -- the assistant said it needed a return-value contract from a
#                    subagent it had itself dispatched, which is unfinished research -- so a bare
#                    statement of need is not a blocker.
#       asked     -- the user explicitly asked for an explanation ("explain", "how come", "why does
#                    it work that way"). Then explaining is the deliverable and this rule stays out
#                    of it. Deliberately not a bare question mark: every prompt in the corpus above
#                    ends in one, so a question-mark exemption would gut the rule.
#     The exemption the brief names and this file deliberately does not implement is
#     `agreed_and_acted_next`: at Stop time there is no next turn to look at.
#
#     ER-EFFECTS-NO-CONCESSION-PIVOT is the second rule in this package: the em-dash pivot the user
#     has now named twice, and its verbatim instance is the second turn above -- "You wouldn't --
#     and there's no need to invent a new field, because ...", a two-word concession followed by
#     160 words of the thing the concession was supposed to have made unnecessary. Their words for
#     the shape: "My real point <em-dash> massive amount of prose that is never worth reading".
#
#     It is separable because it convicts on grammar rather than on the prompt: a short concession
#     head, an em dash or an adversative, and a long tail, in a turn that changed nothing. That
#     makes it reach turns where the challenge detector cannot see a second-person subject. It
#     reads `changed`, `blocked` and `asked` for the same reasons the first rule does -- prose the
#     user asked for is not the defect.
#
#     Tuned against the transcripts rather than guessed, by
#     scripts/audit-challenged-convention-false-positives.py over 2,699 real turn boundaries from
#     the 30 newest session transcripts. The pair halts 3 of them (0.11%), and all three sit in the
#     one transcript that carries the verbatim corpus -- the session that wrote this file, whose
#     prompt quotes the three challenges and whose fixtures quote the three defences. Over the other
#     29 transcripts, 2,617 boundaries, it halts nothing.
#
#     Two things were measured out, and the second was most of the noise:
#       * the challenge arm read "why do you think loadgame-builder didn't run?" as a challenge. It
#         is a request for the assistant's diagnosis of the game, and the turn it accused answered
#         in full with a four-column comparison of two boot logs. "why do you think/believe/say" now
#         suppresses the challenge and sets `asked`.
#       * the pivot arm accepted a bare "yes", "no", "right", "true" and "correct" as concessions,
#         and halted 46 of the same 2,699 (1.7%). Every one was a direct answer to a yes-or-no
#         question -- "Yes -- invade now.", "No -- that 819 s ran with most of the profile switched
#         off." -- which is the answer-first shape this repo asks for. What survives is a concession
#         that concedes something the assistant had argued.
#     The detector is not merely quiet: the challenge fact fires on 16 of the 2,129 turns that ended
#     on prose, and 7 of those 16 turns answered with an edit -- the wanted behaviour, cleared by
#     `changed`.
#
#     Known gaps, stated so the silence is not mistaken for proof:
#       * a challenge made without a second-person subject ("that convention is nonsense") is not
#         caught. Widening past second person is what starts convicting ordinary questions about the
#         game binary, so the evaluative imperatives that are caught ("stop explaining", "quit
#         adding") are enumerated rather than inferred.
#       * `defence` needs an explicit justification marker, so an explanation phrased without one
#         escapes. Measured instance, and it is a real miss: "Not fear of the teardown -- asking is a
#         cheap way to look careful, and it shifts the blame for a destructive act onto you ..."
#         answered "Why are you scared of tearing down the game?" with an account of the habit and
#         no edit. Catching it needs a marker family broad enough ("so the reflex fires") to start
#         convicting reports, which is the trade this file refuses.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_challenged_convention"]
package cupcake.policies.claude.no_explanation_instead_of_correction

import rego.v1

# Enforcement: block turn-end when the user challenged a choice the assistant made and the turn
# answered with justification instead of a change.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-EXPLANATION-INSTEAD-OF-CORRECTION",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

# Enforcement: block turn-end on the concede-then-elaborate closer, in a turn that changed nothing.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [pivoting]
	decision := {
		"rule_id": "ER-EFFECTS-NO-CONCESSION-PIVOT",
		"reason": pivot_reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"The user challenged a choice you made ('",
		challenge,
		"') and you answered by explaining it ('",
		clause,
		"') without changing a single file. A question aimed at a convention you chose is a correction, not a request for its rationale -- they already know what it does, they are telling you it is wrong. Make the change now, in this turn: pick the thing they objected to and edit it. Do not restate why it exists, do not concede and then elaborate, and do not name the next step instead of taking it. If the change genuinely cannot be made, say in one line what external thing blocks it -- a credential, a guard that refused the write, a tool that is not installed -- and note that research you could do yourself is not a blocker.",
	])
}

pivot_reason_for(clause) := msg if {
	msg := concat("", [
		"You closed on a concession and then argued past it ('",
		clause,
		"'), in a turn that changed nothing. The user has named this shape twice: a short agreement, an em dash, and then the elaboration the agreement was supposed to have made unnecessary. It reads as a retraction of the concession, and it costs them a round trip in which nothing was fixed. Either the concession is true, in which case make the change now and let the diff be the argument, or it is not, in which case say so plainly in one sentence and say what you are doing instead. Cut the tail.",
	])
}

# --- signal parsing ------------------------------------------------------------------------------
# CHALLENGEFACTS|challenge=..|defence=..|changed=..|blocked=..|asked=..|pivot=..
# The leading tag carries no "=" so it drops out of the fact map on its own. A field the signal
# omits falls back to a default that does not exempt: a degraded or crafted signal fails closed and
# halts, matching how the neighbouring guards treat one.
fact[k] := v if {
	some kv in split(raw, "|")
	n := indexof(kv, "=")
	n > 0
	k := trim(substring(kv, 0, n), " \t\r\n")
	v := trim(substring(kv, n + 1, -1), " \t\r\n")
}

challenge := object.get(fact, "challenge", "")

defence := object.get(fact, "defence", "")

changed := object.get(fact, "changed", "0")

blocked := object.get(fact, "blocked", "0")

asked := object.get(fact, "asked", "0")

pivot := object.get(fact, "pivot", "")

offending := clause if {
	clause := defence
	clause != ""
	challenge != ""
	changed == "0"
	blocked == "0"
	asked == "0"
}

pivoting := clause if {
	clause := pivot
	clause != ""
	changed == "0"
	blocked == "0"
	asked == "0"
}

raw := trim(matched_facts, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_facts := p if {
	p := input.signals.last_assistant_challenged_convention
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_challenged_convention.output
	is_string(p)
} else := ""
