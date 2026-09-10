# METADATA
# scope: package
# title: Ban ending a turn on future work the turn could have done
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-FUTURE-TENSE-COMMITMENT
#   description: >-
#     User directive 2026-09-09. The turn closed on this sentence, verbatim:
#
#       "Next run I'll build this in and re-attach Frida to confirm zero `DISCARDING` lines across a
#        full invade-reject-reinvade cycle."
#
#     Nothing blocked it. `scripts/er-build-dlls.sh` and the launch script were both available, and
#     the Frida attach path is the one this repo reaches for first. The promise cost the user a round
#     trip worth no information, and their reply was "'Next run I'll' SOUND FAMILIAR?" -- a repeat
#     offence, which is why it is a rule now rather than a note.
#
#     Why the three neighbouring Stop guards do not already cover it, measured rather than assumed:
#     the real turn was replayed through each signal and all three stayed silent.
#       * ER-EFFECTS-NO-UNEXECUTED-PROMISE finds the promise and then exempts it twice over. Its
#         handoff pattern reads "next session/turn/time" as the ball being handed to the user, and
#         its live-background arm exempts any promise sitting near a running job -- the game was up,
#         so a promise to build and re-attach was covered by a session it did not depend on. A
#         deadline disarmed the guard that exists to refuse deadlines.
#       * ER-EFFECTS-NO-DESCRIBED-NEXT-STEP needs an impersonal prescription ("the next step is");
#         this sentence commits the agent by name.
#       * ER-EFFECTS-NO-ZERO-INFORMATION-STOP did produce a handback fact for it and exempted on the
#         same live-background reading.
#     The fact that convicts here is narrower than any of theirs: whether the turn made a tool call
#     of the class the promise named. An `Edit` does not keep a promise to rebuild, and a build does
#     not keep a promise to relaunch.
#
#     The signal emits FUTUREFACTS. The conjunction lives here so it is unit-testable against the
#     verbatim corpus instead of hiding in shell regexes:
#       commit      -- a first-person future-tense promise in the last two sentences of the closing
#                      prose, naming a concrete action ("next run I'll build ...", "then I'll
#                      re-attach ...", "I'm going to rerun it", "I plan to commit that").
#       actionclass -- which kind of work it promised: edit, build, launch, attach, measure, read or
#                      vcs. Reported so the halt can say what was owed.
#       timemarker  -- the deferral was explicit ("next run", "afterwards", "once ..."). Recorded
#                      rather than required: a bare "I'll rebuild it" closer is the same defect
#                      without the deadline, and the directive names both.
#       acted       -- the turn made a tool call of that class. This is the exemption that keeps a
#                      truthful report truthful: a turn that rebuilt and then said so passes.
#       blocked     -- a dependency outside the agent's reach (an observation only the user can
#                      make, a credential, a login, a purchase, a decision that is theirs, an
#                      instruction to stop). Deliberately not the broad "blocked on the user" fact
#                      the siblings fold in, since "I'll do it next run" depends on nobody.
#       planasked   -- the user asked what the plan is, so stating one is the answer. Narrow on
#                      purpose: a bare question mark cannot exempt, or nothing here would ever halt.
#       deferred    -- the work waits on something that does not exist yet, and this turn started
#                      that thing. Both halves are required; a promise parked behind a run somebody
#                      else started is exactly the hole the instance above walked through. A watcher
#                      this turn started -- a Monitor, a SendMessage, a detached shell -- covers a
#                      promise on its own, because it is the mechanism that brings the agent back.
#       conditional -- the promise hangs on something that has not happened: a decision the user has
#                      not made ("say the word and I'll build it"), or an event that may never occur
#                      ("I'll relaunch them that way if either trips over the other"). The first
#                      belongs to ER-EFFECTS-NO-ZERO-INFORMATION-STOP, which convicts an offer to do
#                      authorised work as a handback; the second is a contingency that is not yet
#                      due. Five of the eleven first-draft hits over 2,622 real turns were one of
#                      these two shapes.
#
#     ER-EFFECTS-NO-DELEGATION-AS-COMPLETION is the second rule in this package: the same defect with
#     the subject swapped from the agent to a delegate. Verbatim, from the turn that dispatched the
#     agent which wrote this file:
#
#       "It has the verbatim sentence, the exemptions that must not trip (a real blocker, the user
#        owning the observation, work already started in-turn), and the same evidence bar the others
#        got: measured false-positive rate over real transcripts, all four repo gates, and a
#        `cupcake eval` returning `decision:block` on that exact line rather than a passing unit
#        test."
#
#     The `Agent` tool's own result says the caller knows nothing about a delegate's results until
#     the completion notification arrives. Enumerating what a just-dispatched agent contains converts
#     a dispatch into a claim of delivery, and the user reads it as work already banked. Its facts:
#       delegation -- a closing sentence asserting what the delegate has, covers, includes or will
#                     produce, together with a list of deliverables. A bare dispatch statement ("a
#                     subagent is working on the policy") names no deliverables and never matches.
#       reported   -- the completion notification for every agent this turn dispatched is in the
#                     transcript, so the message relays results instead of predicting them.
#       instructed -- the sentence describes what was asked of the agent ("I asked it to measure the
#                     rate", "the brief says ...") rather than what came back. That is a fact about
#                     the caller's own tool call, which the caller does know. Separable at sentence
#                     level, and measured to be: the verbatim sentence above carries no instruction
#                     framing. The stricter reading was chosen where the two meet -- a message that
#                     describes its instruction and then asserts the output still convicts on the
#                     assertion, so the signal never reports instructed beside a surviving claim.
#
#     Both rules read only the closing prose of a turn that ended on prose. A mid-turn "I'll check
#     the offsets" followed by the check is the correct shape and is never seen.
#
#     Biased hard toward not firing, and the bias was measured rather than asserted. Replayed over
#     2,643 real turn boundaries from 30 session transcripts by
#     scripts/audit-future-commitment-false-positives.py, it halts 2 of them (0.08%), and both are
#     the instances quoted above. The first draft halted 23 (0.88%), and reading every one of them
#     changed four things:
#       * an action word now counts only in a verb position -- straight after the opener or after a
#         coordinator. Scanning every word read "I'll colourise at display time" as a promise to
#         time something, because `time` is in the measuring group, and "I'll get the events as they
#         land" as a promise to land something;
#       * `pull` left the version-control group: "I'll pull the release condition out of that file"
#         is not `git pull`, and it was the whole of one hit;
#       * the offer exemption widened into the conditional one above, which was five of the eleven;
#       * the delegation subject must open its clause, the sentence must name two deliverables, and
#         "it's instructed that ..." counts as instruction framing. Without those, "The run after
#         that is the real test, and it has four specific things to prove" and five more sentences
#         about runs, claims and reviewers were read as claims about a subagent.
#
#     Known gap, measured and left open: the conditional exemption fires on any "if <clause>" that
#     sits anywhere inside the promise sentence, so a promise carrying an incidental condition
#     escapes with it. Narrowing it
#     would take a parse rather than a pattern, and the traffic it lets through is small next to
#     what a false halt costs.
#
#     Known gap, stated so its silence is not mistaken for proof: a delegate dispatched in an
#     earlier turn and described in this one is not caught, because the rule scopes the dispatch to
#     the turn that makes the claim. Widening it would fire on every legitimate progress report about
#     a long-running agent.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_future_commitment"]
package cupcake.policies.claude.no_future_tense_commitment

import rego.v1

# Enforcement: block turn-end when the closing sentence promised work of a class this turn never
# performed, with nothing blocking it.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-FUTURE-TENSE-COMMITMENT",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

# Enforcement: block turn-end when the closing sentence asserted a just-dispatched delegate's
# deliverables before its completion notification existed.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [delegation_claim]
	decision := {
		"rule_id": "ER-EFFECTS-NO-DELEGATION-AS-COMPLETION",
		"reason": delegation_reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn promising work you could have done in it: '",
		clause,
		"'. Nothing in this turn performed that class of action (",
		action_class,
		"), you named no blocker, and nothing you started is carrying it -- so the user pays a round trip that carries no information, and has to tell you to do the thing you just said you would do. Do it NOW, in this turn: run the build, run the launch, make the attach, read the file. If it is genuinely too large for one turn, START it (a backgrounded command or a subagent) so something real is carrying it. Defer only when the work needs something you cannot get yourself -- an observation with no memory-read oracle, a credential, a purchase, a decision only the user owns -- and then say in ONE line what that is, instead of naming a future run.",
	])
}

delegation_reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn describing what a subagent you just dispatched contains or will produce: '",
		clause,
		"'. Its completion notification is not in the transcript, so you know none of that -- the Agent tool says so in its own result text. Written as a list of deliverables it reads to the user as work already banked, and if the agent comes back with something else the record now carries a claim nobody made. Rewrite it as what you ASKED for ('I asked it to measure the false-positive rate'), or say plainly that it is running and its results are not in yet, or wait for the notification and then relay what actually came back.",
	])
}

# --- signal parsing ------------------------------------------------------------------------------
# FUTUREFACTS|commit=..|actionclass=..|timemarker=..|acted=..|blocked=..|planasked=..|deferred=..
#            |conditional=..|delegation=..|reported=..|instructed=..
# The leading tag carries no "=" so it drops out of the fact map on its own. A field the signal omits
# falls back to a default that does not exempt: a degraded or crafted signal fails closed and halts,
# matching how the neighbouring guards treat one.
fact[k] := v if {
	some kv in split(raw, "|")
	n := indexof(kv, "=")
	n > 0
	k := trim(substring(kv, 0, n), " \t\r\n")
	v := trim(substring(kv, n + 1, -1), " \t\r\n")
}

commit := object.get(fact, "commit", "")

action_class := object.get(fact, "actionclass", "work")

acted := object.get(fact, "acted", "0")

blocked := object.get(fact, "blocked", "0")

planasked := object.get(fact, "planasked", "0")

deferred := object.get(fact, "deferred", "0")

conditional := object.get(fact, "conditional", "0")

delegation := object.get(fact, "delegation", "")

reported := object.get(fact, "reported", "0")

instructed := object.get(fact, "instructed", "0")

offending := clause if {
	clause := commit
	clause != ""
	acted == "0"
	blocked == "0"
	planasked == "0"
	deferred == "0"
	conditional == "0"
}

delegation_claim := clause if {
	clause := delegation
	clause != ""
	reported == "0"
	instructed == "0"
}

raw := trim(matched_facts, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_facts := p if {
	p := input.signals.last_assistant_future_commitment
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_future_commitment.output
	is_string(p)
} else := ""
