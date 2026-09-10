# METADATA
# scope: package
# title: Ban ending a turn on a diagnosis that was not fixed
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX
#   description: >-
#     User directive 2026-09-09, given mid-session after five consecutive turns of it: "I don't care
#     what is broken or why, I only know what the fix looks like, and that you're still not there at
#     every turn you pause and dump prose". Naming a defect and stopping is banned. A turn that says
#     what is broken must also change a file, or state why it cannot.
#
#     The signal emits DIAGFACTS with four facts. The conjunction lives here so it is unit-testable
#     against the verbatim corpus instead of hiding in shell regexes:
#       diagnosis -- the turn named a defect / cause / fix ("the real defect is", "the bug is in",
#                    "worth fixing", "the fix is").
#       fixed     -- an Edit / Write / MultiEdit / NotebookEdit tool_use came LATER in the same turn
#                    than that sentence. Reads do not count: reading is how a diagnosis is built.
#       asked     -- the opening user prompt asked a question. Answering is the deliverable and must
#                    never be gagged; this exemption is deliberately broad.
#       blocked   -- the turn stated a real dependency (sudo, a live game, an approval, waiting on
#                    the user). A diagnosis that cannot yet be acted on is not a stall.
#     Halts when a diagnosis was made, nothing was written, and neither exemption applies.
#
#     Why a new rule rather than widening a neighbour: ER-EFFECTS-NO-DESCRIBED-NEXT-STEP excludes a
#     bare "the fix is X" on purpose (it is how a completed fix gets reported), ER-EFFECTS-NO-IDLE-
#     HOLD needs an announcement of waiting, and ER-EFFECTS-NO-STALL-ON-FRICTION requires the prompt
#     to carry frustration and counts any tool call as having acted. This one turns on the single
#     fact those three cannot see: whether a file changed.
#
#     ER-EFFECTS-NO-PROMISSORY-CLOSER (added 2026-09-09) is the second rule in this package, and it
#     turns on the same fact. The shape it refuses, verbatim from the turn that prompted it:
#
#       "Fixing both: bypass the union so the naked capture is the detour entry (`MhHook::new`
#        exists for exactly that), and pass the adopted menu object to `invade` instead of the
#        synthesized box."
#
#     A present participle with no subject, announcing work as though it were already in flight,
#     closing a turn that changed nothing. It is a plan wearing the grammar of a report, and it
#     walked past every Stop guard in the repo: ER-EFFECTS-NO-UNEXECUTED-PROMISE needs a
#     first-person opener ("I'll", "I'm going to", "let me") and this sentence commits nobody, while
#     ER-EFFECTS-NO-DESCRIBED-NEXT-STEP needs a forward-looking prescription and "Fixing" points at
#     now. It lives here rather than in a fourth file because the fact that convicts it is the one
#     this signal already computes.
#
#     Its conjunction is three facts, not four:
#       promise -- the closing statement announces work in the present participle ("Fixing both:",
#                  "Now wiring the detour", "Porting the offsets.", "Next I bypass the union").
#       edited  -- a write ANYWHERE in the turn, not one after the sentence. A closer is the last
#                  thing in the turn, so nothing can follow it and `fixed` is always 0 for one; the
#                  question that separates a truthful report from an announcement is whether the
#                  turn wrote anything at all. A turn that made the edit and then said so passes.
#       blocked -- the same real-dependency exemption as above.
#     A second closing shape reports under the same rule id, because it is the same defect wearing
#     different grammar -- an observation rather than an intention, which is how it slipped past a
#     pattern keyed on "I'll" or a work gerund. Verbatim, 2026-09-09: "... and the next measurement
#     is whether the DLL is using the menu object it now captures (`r14`) for the re-invade gate or
#     still falling back to the scan -- which its own log answers, so I am reading that next." The
#     answer was in a file on disk; naming the read instead of performing it buys a round trip worth
#     nothing. Its facts:
#       unread    -- the closing prose names a concrete artifact as holding the answer and defers
#                    the read past the turn.
#       consulted -- a tool call in the turn opened a strong artifact token (a path, a filename, an
#                    address, a symbol) that the closing prose also names. A weak noun such as "its
#                    own log" cannot clear this, because almost every command in this repo mentions
#                    some log, and letting the word exempt the shape would exempt the instance
#                    above, whose turn had grepped a different run's directory.
#       future    -- the evidence does not exist yet ("the next run's log will say"), so the read
#                    could not have happened in this turn. That is a plan, not a skipped read.
#     `userneed` and `blocked` exempt it as they do elsewhere.
#
#     `asked` is deliberately NOT an exemption here, and that is measured rather than assumed: the
#     prompt "Would you expect me to have seen any feature changes?" was answered with a correct
#     explanation that closed "Fixing that now." and changed nothing. The `asked` exemption exists
#     so an EXPLANATION cannot be gagged; this arm does not fire on explanations, only on an
#     announcement of work, so inheriting the exemption would have exempted the defect itself.
#
#     ER-EFFECTS-NO-ZERO-INFORMATION-STOP (added 2026-09-09) is the third rule here, from the user
#     directive: "If nothing is the ideal response from me, in every case, this means we are lacking
#     a rego policy that adds a stop hook. Every. Single. Time. This. Happens." A turn whose ideal
#     next user reply is nothing has spent a round trip of the user's attention to say so.
#
#     The instance. Asked "What's my ideal response for you at this moment?", the turn closed
#     "Nothing -- the ball is in my court. Rebuilding and relaunching now; the one thing I'll need
#     from you afterwards is a single use of the item." The build and the relaunch were the agent's
#     own next actions, known and unblocked. Every existing guard was disarmed by the last clause:
#     ER-EFFECTS-NO-UNEXECUTED-PROMISE and ER-EFFECTS-NO-DESCRIBED-NEXT-STEP both exempt a turn that
#     hands the obligation to the user, and "I'll need from you ... a single use of the item" reads
#     as exactly that -- except that in this repo the agent drives every in-game input itself, so it
#     is not a handoff at all.
#
#     Its facts:
#       handback     -- the closing sentence that hands work back.
#       handbackkind -- a: the user is told they need do nothing; b: the agent announced its own
#                       next action instead of taking it; c: an offer to do work it is already
#                       authorised to do. b and c fire on their own. `a` fires only when the turn
#                       did no work at all (`didwork`), because "nothing on your side" after a turn
#                       that delivered is the honest end of a finished task, not a handback.
#       userneed     -- the closing prose asks for something only the user has: an observation with
#                       no memory-read oracle, a subjective or external-only decision. This is the
#                       exemption that keeps a legitimate ending legitimate. A request for in-game
#                       input is not one, because the agent drives input itself.
#       extblocked   -- a dependency outside the agent's reach: sudo, credentials, a login, a
#                       purchase, a live game, network, a gate it is waiting on. Deliberately not
#                       the `blocked` fact the other two arms read, which folds in "blocked on the
#                       user": "I need you to press X" is a dependency on the user in the letter and
#                       a handback in fact, so reading the broad fact would exempt the shape this
#                       rule exists to refuse.
#       carried      -- a subagent or a backgrounded command is genuinely still running. "Nothing to
#                       do until they report" is then a true statement about work that exists, not a
#                       handback; the same `live_background_work` helper the idle-hold guard reads.
#
#     Tuned against the transcripts, not guessed: the first draft fired on 255 of 3,749 real turn
#     boundaries (6.8%), and reading them deleted a whole fourth shape (a request for in-game input,
#     which the directive names as something that cannot exempt a handback rather than as a handback
#     itself), end-anchored the participle spelling of `b`, cut its first-person spelling to the
#     build/launch/run family, and added the offer exemption for a destructive or user-visible action
#     and for a genuine fork. See scripts/audit-diagnosis-signal-false-positives.py.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_diagnosis_without_fix"]
package cupcake.policies.claude.no_diagnosis_without_fix

import rego.v1

halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

# Enforcement: block turn-end when the closing statement announced work in the present participle
# and the turn wrote nothing.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [promissory]
	decision := {
		"rule_id": "ER-EFFECTS-NO-PROMISSORY-CLOSER",
		"reason": promissory_reason_for(clause),
		"severity": "HIGH",
	}
}

# Enforcement: block turn-end when the closing statement named the evidence that answers the open
# question and left the read for later. Same rule id as the promissory closer: same defect.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [unread]
	decision := {
		"rule_id": "ER-EFFECTS-NO-PROMISSORY-CLOSER",
		"reason": unread_reason_for(clause),
		"severity": "HIGH",
	}
}

# Enforcement: block turn-end when the ideal next user reply is nothing -- the turn handed back work
# it could and should have done itself.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [handback]
	decision := {
		"rule_id": "ER-EFFECTS-NO-ZERO-INFORMATION-STOP",
		"reason": handback_reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn naming a defect ('",
		clause,
		"') and changed no file. Make the edit now -- the diagnosis is not the deliverable, the fix is. If it genuinely cannot be made yet, say in one line what blocks it.",
	])
}

promissory_reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn announcing a fix as though it were already underway ('",
		clause,
		"') and wrote nothing. That sentence is a plan in the grammar of a report: the user reads it as done, and no file changed. Make the edit now, in this turn -- the tool call that does what the sentence says. If it is genuinely too large for one turn, start it (a backgrounded command or a subagent) so something real is carrying it. If it cannot be made yet, say in one line what blocks it, in the past or future tense, so it cannot be misread as work already in flight.",
	])
}

unread_reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn naming the evidence that answers your own open question, and did not read it: '",
		clause,
		"'. The file is on disk and the answer is in it, so stopping here buys a round trip worth nothing -- the user learns only that you know where to look. Read it now, in this turn, and close the question with what it says. Defer only when the evidence does not exist yet (a run has to happen first, and then say so plainly) or when reading it needs something only the user has.",
	])
}

handback_reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn with nothing for the user to reply: '",
		clause,
		"'. That costs them a round trip and carries no information -- they read it, learn nothing they can act on, and have to tell you to carry on with work you had already chosen. Take the action now, in this turn: run the build, run the launch, make the edit, start the subagent. Do not offer to do work you are already authorised to do, and do not announce your own next step as a closing line. End on the user only when their reply would carry something you cannot get yourself: an observation with no memory-read oracle ('tell me what you saw'), a subjective or external-only decision, or a genuine blocker you name in one line. Asking them to press a key or use an item is not one of those -- you drive every in-game input yourself.",
	])
}

facts := line if {
	line := input.signals.last_assistant_diagnosis_without_fix
	is_string(line)
	startswith(line, "DIAGFACTS|")
}

field(name) := value if {
	some part in split(facts, "|")
	startswith(part, concat("", [name, "="]))
	value := substring(part, count(name) + 1, -1)
}

offending := clause if {
	clause := field("diagnosis")
	clause != ""
	field("fixed") == "0"
	field("asked") == "0"
	field("blocked") == "0"
}

promissory := clause if {
	clause := field("promise")
	clause != ""
	edited == "0"
	blocked == "0"
}

unread := clause if {
	clause := field("unread")
	clause != ""
	consulted == "0"
	future == "0"
	userneed == "0"
	blocked == "0"
}

handback := clause if {
	clause := field("handback")
	clause != ""
	extblocked == "0"
	userneed == "0"
	carried == "0"
	handback_kind_fires
}

# `b` is an announcement of the agent's own next action and `c` an offer to do work it already has
# authority for. Both name work that did not happen, so neither needs corroboration.
handback_kind_fires if {
	handback_kind == "b"
}

handback_kind_fires if {
	handback_kind == "c"
}

# `a` says only that the user need do nothing, which is also how a finished task ends -- so it
# convicts a turn that did no work and exonerates one that did. It is the one shape that reads
# `asked`: "Nothing." answering a question is the deliverable, while the same word opening an
# unprompted closer is the defect, and a measured turn was halted for the former.
handback_kind_fires if {
	handback_kind == "a"
	didwork == "0"
	field("asked") == "0"
}

# A non-empty `promise` can only come from a signal that emits these two fields, so a missing one
# means a degraded or crafted line rather than an older signal: default it to the value that halts,
# the way the described-next-step guard does, so a broken signal cannot buy silence.
edited := value if {
	value := field("edited")
} else := "0"

blocked := value if {
	value := field("blocked")
} else := "0"

# Same fail-closed default for the handback fields: a degraded line that still carries a clause
# halts rather than buys silence. `handbackkind` defaults to the value that fires unaided.
handback_kind := value if {
	value := field("handbackkind")
} else := "b"

userneed := value if {
	value := field("userneed")
} else := "0"

didwork := value if {
	value := field("didwork")
} else := "0"

extblocked := value if {
	value := field("extblocked")
} else := "0"

carried := value if {
	value := field("carried")
} else := "0"

consulted := value if {
	value := field("consulted")
} else := "0"

future := value if {
	value := field("future")
} else := "0"
