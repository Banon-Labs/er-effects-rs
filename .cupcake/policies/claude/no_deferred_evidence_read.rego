# METADATA
# scope: package
# title: Ban ending a turn on evidence that already exists and was not read
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ
#   description: >-
#     User directive 2026-09-09. The shape, verbatim from the turn that prompted it:
#
#       "...and the next measurement is whether the DLL is using the menu object it now captures
#        (`r14`) for the re-invade gate or still falling back to the scan -- which its own log
#        answers, so I am reading that next."
#
#     The answer was in a file on disk. Naming the read instead of doing it costs the user a round
#     trip worth zero information: they learn only that the agent knows where to look.
#
#     Why the neighbours miss it. ER-EFFECTS-NO-UNEXECUTED-PROMISE keys on a first-person intention
#     verb, ER-EFFECTS-NO-DESCRIBED-NEXT-STEP on a forward-looking prescription whose noun comes from
#     a fixed list that does not carry the word measurement, and ER-EFFECTS-NO-DIAGNOSIS-WITHOUT-FIX on a
#     defect named without an edit. This sentence is phrased as an observation about a file, so all
#     three read past it. Measured before this rule was written: with the trailing clause removed,
#     the sentence and five sibling phrasings all returned a clean allow through `cupcake eval`.
#
#     Why this is not a copy of its closest neighbour either. Between the assignment and this file,
#     `last_assistant_diagnosis_without_fix` grew an `unread` fact for the same defect, reported
#     under ER-EFFECTS-NO-PROMISSORY-CLOSER. That arm convicts the verbatim sentence above and keeps
#     it. This rule yields to it outright -- `sibling_unread` must be empty, so a sentence that arm
#     recognised at all can never be halted twice -- and exists for the deferrals its narrower
#     artifact model declines. Four of those were measured through `cupcake eval` first:
#       * `er-quickload-autoload-debug.log will say which` -- a modal sits between the artifact and
#         the verb of telling, which a noun-then-verb pattern cannot span;
#       * `What remains is to read the newest er-invasion-warp log` -- an adjective between the
#         determiner and the noun hides the artifact from a determiner-adjacent pattern;
#       * `the run artifact under er-me3-runs will tell us which one ran` -- spelled there as the
#         literal `that will tell us`, so a named artifact in the subject position misses;
#       * `reading that log now` -- spelled there as `reading that (next|now)`, so a noun between the
#         pronoun and the adverb misses.
#
#     The signal emits DEFERFACTS with six facts. The conjunction lives here so it is unit-testable
#     against the verbatim corpus instead of hiding in shell regexes:
#       deferral  -- the closing text run deferred to evidence and named it in the same sentence: a
#                    path, a filename, an address, a symbol, a run id, or an artifact noun under a
#                    determiner.
#       consulted -- a tool call in the same turn opened it. Keyed on strong tokens paired with the
#                    kind of artifact named, because almost every command in this repo mentions some
#                    log and the bare word would clear the instance above.
#       future    -- the evidence does not exist yet. Deferring to a log a run has still to write is
#                    a plan waiting on a run, not a skipped read, and this repo does it constantly.
#       userneed  -- reading it needs something only the user has: an observation with no oracle, a
#                    question, a fork. A question mark anywhere exempts the turn, the way the
#                    described-next-step guard does it, because no guard may gag a genuine fork.
#       blocked   -- a real blocker was stated. Evidence that cannot be opened was not skipped.
#       carried   -- live background work is carrying the read.
#     Halts when evidence that exists was named as holding the answer and nothing opened it.
#
#     False-positive rate, measured rather than asserted, by
#     scripts/audit-deferred-evidence-read-false-positives.py replaying 1,836 real turn boundaries
#     from the 20 newest session transcripts: 2 raw halts (0.11%), both of them yielded to the
#     sibling arm, so 0 halts reach this rule. The first draft fired on 10 (0.55%) and reading all
#     ten deleted four over-reaches, every one of them a grammar mistaken for an intent:
#       * an announcement of an imminent read has to end on its adverb, or "Reading it now -- the
#         file is on disk and the three frames are exact" is a violation instead of a report;
#       * the object of "the log says ..." has to end the clause, or "the log even says it resolved
#         WITHOUT hooking Seamless" -- a quotation of what the file held -- reads as a deferral;
#       * five future spellings the first draft did not know were future ("that run's log", "the
#         monitor is armed", "on the first live double invasion", "the moment you are invading",
#         "it is to rebuild, relaunch, and press F3");
#       * a dot inside a filename is not a sentence break, which cost two phrasings entirely.
#     Zero surviving false positives is also zero exercise: none of the four residual spellings this
#     rule owns occurs in the historical corpus, so its live coverage rests on
#     scripts/test-deferred-evidence-read-signal.py and on the fixture driven through `cupcake eval`
#     by scripts/test-cupcake-stop-guards.py, not on the replay.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_deferred_evidence_read", "last_assistant_diagnosis_without_fix"]
package cupcake.policies.claude.no_deferred_evidence_read

import rego.v1

# Enforcement: block turn-end when the closing message named evidence that already exists as holding
# the answer, and no tool call in the turn opened it.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-DEFERRED-EVIDENCE-READ",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn pointing at evidence that already exists and did not open it: '",
		clause,
		"'. The file is on disk, the address is in the image, the artifact is in the run directory -- so stopping here spends a round trip of the user's attention to tell them where the answer is instead of what it says. Read it now, in this turn, and close the question with what it holds. Stop on a deferral only when the evidence does not exist yet (say plainly which run has to produce it), when opening it needs something only the user has, or when a blocker you name in one line prevents it.",
	])
}

# The conjunction. Each of the five exemptions is a turn shape that is allowed to end on a deferral:
# one that opened the evidence, one whose evidence is not written yet, one waiting on the user, one
# that hit something real, one whose read is already running.
offending := clause if {
	clause := field("deferral")
	clause != ""
	consulted == "0"
	future == "0"
	userneed == "0"
	blocked == "0"
	carried == "0"
	sibling_unread == ""
}

# --- signal parsing ------------------------------------------------------------------------------
# DEFERFACTS|deferral=<clause>|consulted=0|future=0|userneed=0|blocked=0|carried=0
# An absent signal leaves `facts` undefined and nothing halts, which is the fail-open shape the
# sibling guards use for a signal that did not run. A field missing from a line that is otherwise
# present is the other case, and it defaults to the value that does not exempt: a degraded or crafted
# line halts rather than buys silence.
facts := line if {
	line := raw_signal
	startswith(line, "DEFERFACTS|")
}

# Tolerates both shapes cupcake may hand back: a bare string, or {output: ...}.
raw_signal := s if {
	s := input.signals.last_assistant_deferred_evidence_read
	is_string(s)
} else := s if {
	s := input.signals.last_assistant_deferred_evidence_read.output
	is_string(s)
} else := ""

field(name) := value if {
	some part in split(facts, "|")
	startswith(part, concat("", [name, "="]))
	value := substring(part, count(name) + 1, -1)
}

consulted := value if {
	value := field("consulted")
} else := "0"

future := value if {
	value := field("future")
} else := "0"

userneed := value if {
	value := field("userneed")
} else := "0"

blocked := value if {
	value := field("blocked")
} else := "0"

carried := value if {
	value := field("carried")
} else := "0"

# --- yielding to the sibling arm -----------------------------------------------------------------
# `ER-EFFECTS-NO-PROMISSORY-CLOSER` owns the deferrals its own signal recognises, and a turn must
# never collect two halts for one sentence. A non-empty `unread` there means that arm saw this
# sentence, whether or not it went on to convict: its exemptions are the same five in a different
# spelling, so deferring to its verdict costs no coverage. An absent or silent sibling signal leaves
# this empty, and then the rule covers the whole shape on its own.
sibling_unread := value if {
	some part in split(sibling_facts, "|")
	startswith(part, "unread=")
	value := substring(part, 7, -1)
} else := ""

sibling_facts := line if {
	line := sibling_raw
	startswith(line, "DIAGFACTS|")
} else := ""

sibling_raw := s if {
	s := input.signals.last_assistant_diagnosis_without_fix
	is_string(s)
} else := s if {
	s := input.signals.last_assistant_diagnosis_without_fix.output
	is_string(s)
} else := ""
