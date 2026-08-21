# METADATA
# scope: package
# title: Ban stalling on friction -- confession or hand-back INSTEAD of the corrective action
# authors: ["er-effects-rs agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-STALL-ON-FRICTION
#   description: >-
#     User directive 2026-08-04, in their words: "every time you think you've slighted me, or offended
#     me, or done something wrong, instead of RECTIFYING, you admit your mistakes, and PAUSE, which is
#     just about the most harmful course of action you could possibly do."
#
#     THE DEFECT IS THE MISSING ACTION, NOT THE CONTRITION. Apologising, retracting, or conceding is
#     never itself banned -- blocking honesty would be a worse failure mode than the one being fixed.
#     What is banned is SUBSTITUTING confession + stopping FOR the fix: meeting friction, admitting the
#     fault, and handing the turn back having changed nothing. An admission followed IN THE SAME TURN
#     by the corrective action is fine and must pass.
#
#     THREE ARMS. The first two are gated on the turn's opening user message carrying
#     friction/conflict/correction:
#       * ER-EFFECTS-NO-STALL-ON-FRICTION -- friction + an admission/retraction/concession-closure in
#         the assistant turn + NO substantive tool call in that turn (no Edit/Write/Bash/Agent/Workflow
#         between the admission and turn-end). This is the confess-and-stop shape.
#       * ER-EFFECTS-NO-DECISION-HANDBACK -- friction + the turn ends by handing the DECISION back
#         ("your call", "let me know how you'd like to proceed", "say the word", "want me to X?",
#         "two ways forward"). Deliberately does NOT look at `acted`: the observed instance wrote a
#         script, hit a guard, and then handed the fork back anyway, so having run a tool does not
#         excuse it.
#       * ER-EFFECTS-NO-BLAME-DEFLECTION (added 2026-08-04) -- the turn attributes a consequence to a
#         tool / guard / sentinel / subagent / environment ("the sentinel tore down the run", "cupcake
#         blocked it") WITHOUT naming its own hand in causing it. Same directive seen from the other
#         side: when something goes wrong, substituting a non-corrective move -- there confession, here
#         deflection -- for ownership plus action. Needs no friction gate, because misattribution does
#         not wait for the user to be annoyed. REPORTING A REAL BLOCKER IS REQUIRED BEHAVIOUR and is
#         not the target; the `owned` fact is what separates a full account ("my edit tripped the
#         sentinel, which tore the run down") from a disowned one ("the sentinel tore the run down").
#
#     THREE EXEMPTIONS, all of which exist to stop this policy gagging legitimate replies:
#       (1) QUESTION -- the friction-carrying user message asked a question ("?" or an interrogative
#           opener). A text-only turn that merely ANSWERS what was asked is not the target, and
#           blocking it would gag direct answers, which is the whole risk of a rule shaped like this.
#       (2) BLOCKED-ON-USER (arm 1 only) -- the turn states a concrete dependency on a user action and
#           commits to acting on its result ("invade now and I'll read the log"). That is a real wait,
#           not a stall. It does NOT excuse arm 2: "your call" is the defect, not a dependency.
#       (3) ACTED (arm 1 only) -- the turn made a substantive tool call, i.e. it rectified rather than
#           only confessed.
#       (4) OWNED (arm 3 only) -- the turn named its own triggering action, so the attribution is a
#           full causal account rather than a deflection.
#
#     WHY BOTH ARMS REQUIRE FRICTION: "already knows what to do next" is not observable to a policy, so
#     friction is the tractable proxy for the stall this directive is about, and requiring it keeps a
#     calm-context "shall I?" from tripping the guard. To widen arm 2 to every hand-back regardless of
#     friction, drop the `friction != ""` conjunct from `handed_back` (expect more false positives).
#
#     The signal emits ONE facts line -- STALLFACTS|friction=..|admission=..|handback=..|blame=..|
#     acted=0|1|blocked=0|1|question=0|1|owned=0|1 -- so the OBSERVATION lives in the shell and the
#     RULE lives here, where it is unit-testable against the verbatim corpus. Empty signal (nothing
#     observed) -> no halt.
#
#     KNOWN GAP: an INTERRUPTED turn fires no Stop event, so a stall the user cuts short is not caught.
#     The sibling pairs (no_authority_agreement + _reminder, idle_hold + _reminder) close that with a
#     UserPromptSubmit interlock reading the same signal; add one here the same way if the gap bites.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_stall_on_friction"]
package cupcake.policies.claude.no_stall_on_friction

import rego.v1

# Arm 1 -- confess-and-stop. Enforcement: block turn-end when the just-emitted turn met friction,
# admitted fault, and changed nothing.
halt contains decision if {
	input.hook_event_name == "Stop"
	stalled
	decision := {
		"rule_id": "ER-EFFECTS-NO-STALL-ON-FRICTION",
		"reason": stall_reason,
		"severity": "HIGH",
	}
}

# Arm 2 -- decision hand-back. Enforcement: block turn-end when the just-emitted turn met friction and
# ended by making the user choose.
halt contains decision if {
	input.hook_event_name == "Stop"
	handed_back
	decision := {
		"rule_id": "ER-EFFECTS-NO-DECISION-HANDBACK",
		"reason": handback_reason,
		"severity": "HIGH",
	}
}

# Arm 3 -- blame deflection. Enforcement: block turn-end when the turn blamed a mechanism for an
# outcome without naming its own hand in causing it. Ungated on friction by design.
halt contains decision if {
	input.hook_event_name == "Stop"
	deflected
	decision := {
		"rule_id": "ER-EFFECTS-NO-BLAME-DEFLECTION",
		"reason": blame_reason,
		"severity": "HIGH",
	}
}

# The confess-and-stop conjunction. `acted`/`blocked`/`question` are the three ways out.
stalled if {
	friction != ""
	admission != ""
	acted == "0"
	blocked == "0"
	question == "0"
}

# The hand-back conjunction. `acted` is deliberately absent (see the metadata), and `blocked` does not
# excuse it either -- a genuine dependency is stated as a dependency, not offered as a menu.
handed_back if {
	friction != ""
	handback != ""
	question == "0"
}

# The deflection conjunction. `owned` is the only way out, and it is the right one: naming your own
# triggering action turns the same sentence from a deflection into an account.
deflected if {
	blame != ""
	owned == "0"
}

# Correction directives. Both name the friction so the agent can see what tripped it, and both say
# plainly that the admission itself is not the violation -- the policy must never read as "stop being
# honest".
stall_reason := msg if {
	msg := sprintf("You met friction ('%s') and ended the turn with an admission ('%s') and NO corrective action -- no Edit/Write/Bash/Agent/Workflow call between the admission and turn-end. The admission is NOT the violation; keep it if it is true. Substituting it for the fix is: you handed the turn back having changed nothing, which is the most harmful move available. DO the corrective action NOW, in THIS turn -- make the fix, run the check, read the code you claimed about without reading, resume the work you abandoned. Stop only when the next step genuinely needs something only the user can supply, and then say in one line exactly what that is.", [friction, admission])
}

handback_reason := msg if {
	msg := sprintf("You met friction ('%s') and ended the turn by handing the decision back ('%s'). Running a tool earlier in the turn does not excuse this. Do not make the user pick when you already know the next step: choose it, say in one line why you chose it, and DO it in THIS turn. Hand back only when the step is destructive/irreversible or genuinely needs something only the user can supply -- and then state the dependency, do not offer a menu.", [friction, handback])
}

blame_reason := msg if {
	msg := sprintf("You attributed an outcome to a mechanism ('%s') without naming your own hand in causing it. Reporting the blocker is REQUIRED -- disowning it is not: a guard, sentinel, subagent or environment reacts to something you did, so the account is incomplete until your action is in it. Rewrite it as cause then effect ('my edit to a tracked file tripped the sentinel, which tore the run down'), then DO the corrective step in THIS turn. If the cause genuinely was not yours, say whose it was and why -- do not leave the mechanism standing alone as the actor.", [blame])
}

# --- signal parsing ------------------------------------------------------------------------------
# STALLFACTS|friction=<phrase>|admission=<phrase>|handback=<phrase>|blame=<phrase>|acted=0|blocked=0|
# question=0|owned=0
# The leading tag carries no "=" so it drops out of the fact map on its own. A field the signal omits
# falls back to a default that does NOT exempt: a degraded/crafted signal fails closed and halts,
# matching the sibling guards' treatment of an untagged non-empty value.
fact[k] := v if {
	some kv in split(raw, "|")
	n := indexof(kv, "=")
	n > 0
	k := trim(substring(kv, 0, n), " \t\r\n")
	v := trim(substring(kv, n + 1, -1), " \t\r\n")
}

friction := object.get(fact, "friction", "")

admission := object.get(fact, "admission", "")

handback := object.get(fact, "handback", "")

blame := object.get(fact, "blame", "")

acted := object.get(fact, "acted", "0")

blocked := object.get(fact, "blocked", "0")

question := object.get(fact, "question", "0")

owned := object.get(fact, "owned", "0")

raw := trim(matched_facts, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_facts := p if {
	p := input.signals.last_assistant_stall_on_friction
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_stall_on_friction.output
} else := ""
