# METADATA
# scope: package
# title: Ban ending a turn on a narration of the action instead of on its result
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-NARRATED-ACTION
#   description: >-
#     User directive 2026-09-10. Five turn-final sentences from one session, each with the action
#     either already dispatched or dispatchable in the same turn:
#
#       Re-running it now without the cap.
#       Rebuilding and relaunching now.
#       Bringing it up to read the pointer chain live rather than guessing another offset:
#       Making the empty read diagnosable - logging each of the three hops so the next invasion says
#         which one returned zero instead of just that one did:
#       Dispatching a subagent to enumerate the menu builder's rows properly, and unblocking you now
#         by narrowing the guard:
#
#     The user's words when they called it out:
#
#       "'Re-running it now without the cap' me: *points to the rego policy you `MUST` update to
#        disallow this prose without a stop hook*"
#
#     They had named the general form twice already in the same session -- "Announcing your own next
#     action instead of taking it", which is also how `AGENTS.md` spells it in the stop-too-early
#     list, and "Why do you insist on going 'My real point <em-dash> massive amount of prose that is
#     never worth reading'".
#
#     This is the present-progressive sibling of ER-EFFECTS-NO-FUTURE-TENSE-COMMITMENT, and the gap
#     is grammatical rather than a matter of degree: that rule needs a first-person future opener
#     ("next run I'll ...", "I'm going to ..."), and a bare participial clause commits nobody and
#     names no future, so its opener pattern never sees one.
#
#     Why the other neighbours do not already cover it. Measured rather than argued: a fixture of
#     each of the five sentences was replayed through every last_assistant_*.sh signal in the repo,
#     and four of the five produced no facts line from any of them. The fifth, "Rebuilding and
#     relaunching now.", also produces handbackkind=b from last_assistant_diagnosis_without_fix, and
#     that overlap is recorded in the gaps below.
#       * ER-EFFECTS-NO-PROMISSORY-CLOSER does convict a subjectless gerund, but only from the
#         code-change family (fixing, wiring, patching, ...), and it is gated on the turn having
#         written nothing at all. Four of the five sentences head on a verb it does not carry.
#       * ER-EFFECTS-NO-ZERO-INFORMATION-STOP reaches the build/launch/run family end-anchored on
#         "now", which is one spelling of one of the five. Its verbatim instance is "Rebuilding and
#         relaunching now; the one thing I'll need from you afterwards is a single use of the item."
#         -- the same narration wearing a trailing clause. This rule leaves that shape to it on
#         purpose, by requiring the narration to be the last sentence of the closing prose.
#       * ER-EFFECTS-NO-DESCRIBED-NEXT-STEP needs an impersonal prescription ("the next step is").
#         The present participle points at now, not next.
#       * ER-EFFECTS-NO-UNEXECUTED-PROMISE needs a first-person opener.
#
#     The signal emits NARRATIONFACTS. The conjunction lives here so it is unit-testable against the
#     verbatim corpus instead of hiding in shell regexes:
#       narration   -- the final sentence of the closing prose is a first-person progressive or a
#                      bare participial clause naming a concrete action.
#       actionclass -- which kind of work it named: build, launch, attach, measure, read, vcs,
#                      delegate or edit. Reported so the halt can say what was owed.
#       shape       -- which construction made the participle an announcement rather than the
#                      subject of a sentence: a colon closing the clause, an end-anchored "now", a
#                      short fragment with no finite verb, or a first-person progressive. Recorded
#                      for the audit rather than required, since the classifier has already applied
#                      it.
#       reported    -- the narration carries, or is followed by, something measured: a number with a
#                      unit, an exit code, a hash, an address, a path, a file name, or the fenced
#                      block a command's output lands in. Then the sentence reports what happened
#                      instead of announcing what has not.
#       banner      -- the loud launch or teardown banner `AGENTS.md` mandates immediately before a
#                      game launch, in the section that calls the banner a promise rather than a
#                      mood. It is a required form: the user stops what they are doing and turns to
#                      a screen because of it, and a rule that made it unspeakable would take a
#                      safety announcement away to save a round trip.
#       blocked     -- a dependency the agent cannot dissolve by working harder: an observation only
#                      the user can make, a credential, sudo, a login, a purchase, a decision that
#                      is theirs, a guard that refused the write, an instruction to stop.
#
#     Two structural narrowings do most of the work of keeping it quiet, and neither is a judgement
#     about intent. The signal exits when a tool call follows the turn's last prose, so a mid-turn
#     one-line preamble between two tool calls is never read -- the line `wall_of_text.rego` already
#     draws in its own correction text, and the shape this repo's ordinary work is made of. And the
#     narration has to be the final sentence, so a message that goes on to report something is not
#     convicted for the sentence that introduced it.
#
#     Tuned against the transcripts rather than guessed, by
#     scripts/audit-narrated-action-false-positives.py. The first draft halted 25 of 763 boundaries
#     (3.28%) over the 6 newest transcripts; reading every one anchored the first-person progressive
#     to the start of its sentence (a report ending "and I'm restarting the full 26-shell relink"
#     was being read as a narration), taught `reported` to recognise a bare measured pair, and fixed
#     the audit itself, which was counting a task notification as a turn boundary and so charged one
#     interrupted turn fifteen times.
#
#     Over all 74 transcripts this repo has produced the tuned guard halts 2 of 1973 real turn
#     boundaries (0.10%), and both were read rather than counted. "The wording needed explaining;
#     rebuilding and relaunching with it." closed a turn that built and did not relaunch, and the
#     relaunch never happened. "I'm instrumenting the child teardown next, not the evaluator." was answered by the
#     user with "What's next?", which is the zero-information round trip this rule exists to
#     prevent. Neither is a turn that was entitled to stop.
#
#     Counting every user event as a boundary, the way the neighbouring audits do, gives 10 of 1845
#     (0.54%) over the 30 newest transcripts, against 2 of 1331 for the same set here. The extra
#     eight sit at boundaries the Stop hook never
#     reaches: seven follow an interrupt, which aborts a turn rather than stopping it, and three of
#     those are one sentence counted once per slash-command bookkeeping event.
#
#     An `acted` exemption was considered and rejected on measurement, not taste. The turn that
#     closed "Re-running it now without the cap." had already run that same command once in the
#     turn, so a "the turn did work of this class" fact would have cleared the instance the rule was
#     written for. What separates a report from an announcement here is whether the closing sentence
#     carries the outcome, which is what `reported` reads.
#
#     Known gaps, stated so the silence is not mistaken for proof:
#       * a narration with a clause hung off it ("Rebuilding and relaunching now; the one thing I
#         need from you is ...") is not caught here. It is ER-EFFECTS-NO-ZERO-INFORMATION-STOP's
#         verbatim instance and charging it twice would be two rules quoting one clause with two
#         different corrections.
#       * an imperative-of-self spelling ("Next: bring it up.") is not caught. Reaching it needs a
#         pattern keyed on an ordinary colon-led fragment, which is also how a caption introducing a
#         table is written, and the trade is not worth a false halt.
#       * a participial head outside the verb table escapes. The table is enumerated rather than
#         derived, because deriving "any word ending in -ing" convicts every gerund subject in the
#         repo's prose.
#       * one spelling is charged twice. "Rebuilding and relaunching now." as a whole sentence
#         matches this rule and also ER-EFFECTS-NO-ZERO-INFORMATION-STOP's own launch-family
#         pattern, so cupcake returns both halts. It is left that way deliberately: dropping the
#         build and launch gerunds here to avoid it would drop one of the five verbatim closers,
#         and the two corrections say the same thing. The fixture pins the overlap rather than
#         leaving it to be discovered.
#       * ER-EFFECTS-NO-ZERO-INFORMATION-STOP has no banner exemption of its own, so a mandated
#         banner that happens to close on "Relaunching <character> now." is halted by that rule even
#         though this one exempts it. Measured, not hypothetical: it is what the first banner
#         fixture here did. Left alone rather than patched, because widening a neighbour's tuned
#         pattern is a change to its own measured rate and belongs in its own pass.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_narrated_action"]
package cupcake.policies.claude.no_narrated_action

import rego.v1

# Enforcement: block turn-end when the closing sentence narrates an action instead of carrying its
# outcome, with nothing measured beside it, no mandated banner, and no blocker named.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-NARRATED-ACTION",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn narrating the action instead of having taken it: '",
		clause,
		"'. That sentence is an announcement in the grammar of a report -- the user reads it, learns only that you know what to do next, and has to answer 'ok, go on', which is a round trip carrying no information. The work you named (",
		action_class,
		") was yours and unblocked. Take it NOW, in this turn: make the call, run the command, dispatch the subagent, open the file -- and then close on what came back, in the past tense, with the number or the path or the line that proves it. If it is genuinely too large for one turn, START it (a backgrounded command or a subagent) so something real is carrying it, and say what is carrying it. Mid-turn narration between tool calls is fine and is not measured; this is about the sentence you stop on.",
	])
}

# --- signal parsing ------------------------------------------------------------------------------
# NARRATIONFACTS|narration=..|actionclass=..|shape=..|reported=..|banner=..|blocked=..
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

narration := object.get(fact, "narration", "")

action_class := object.get(fact, "actionclass", "work")

reported := object.get(fact, "reported", "0")

banner := object.get(fact, "banner", "0")

blocked := object.get(fact, "blocked", "0")

offending := clause if {
	clause := narration
	clause != ""
	reported == "0"
	banner == "0"
	blocked == "0"
}

raw := trim(matched_facts, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_facts := p if {
	p := input.signals.last_assistant_narrated_action
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_narrated_action.output
	is_string(p)
} else := ""
