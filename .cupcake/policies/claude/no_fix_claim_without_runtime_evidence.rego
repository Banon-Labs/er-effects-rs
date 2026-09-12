# METADATA
# scope: package
# title: Ban ending a turn calling a change a fix when no run showed it working
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-FIX-CLAIM-WITHOUT-RUNTIME-EVIDENCE
#   description: >-
#     User, 2026-09-11: "If someone ever says real fix to me and doesn't put it in airquotes, I look
#     at them like this 🫨" -- and then, when nothing had stopped it: "We are supposed to have a rego
#     policy that stops you from saying 'fix' without runtime evidence".
#
#     The turn that prompted it had edited a crate that ships inside a loaded DLL, built it, never
#     launched the game, and closed on "the real fix". It failed live on the next launch. No rule in
#     this package could see it: ER-EFFECTS-NO-UNBACKED-CLAIM fires only when nothing was written and
#     a file had been written; ER-EFFECTS-NO-PROOF-WITHOUT-OBSERVATION reads one word, "proven", which
#     is precisely the word an agent avoids when it reaches for "fix"; ER-EFFECTS-NO-DIAGNOSIS-
#     WITHOUT-FIX turns on whether a file changed and one had; and the runtime-evidence guard
#     ER-EFFECTS-REQUIRE-RUNTIME-EVIDENCE gates `git push`, not prose.
#
#     The signal emits FIXFACTS with six facts. The conjunction lives here so it is unit-testable
#     against the verbatim shapes instead of hiding in shell regexes:
#       claim      -- the closing prose calls a change a fix, outside quoted, backticked and fenced
#                     spans and outside a table row. An explicit alternation, not the bare stem:
#                     "the real fix", "this is the fix", "this fixes it", "the fix works", "I fixed
#                     x", "x is fixed", a bare "Fixed.". A path or identifier carrying the stem
#                     (fix/harness-repl-reach, prefix, suffix, fixture, fix-slug, fix.md) is a name
#                     and never a claim. The present participle "fixing" is deliberately absent: it
#                     belongs to ER-EFFECTS-NO-PROMISSORY-CLOSER, and charging one sentence twice
#                     with two different corrections is how a guard layer becomes noise.
#       changed    -- a tool call in the turn wrote a source file under a crate that can reach a
#                     DLL the game loads. Measured from the workspace manifests (a `cdylib`
#                     artifact, or a path dependency of one) rather than listed by hand, so the
#                     boundary cannot rot; crates/*/tests, benches and examples are host work and do
#                     not count.
#       evidence   -- something a run produced was opened at or after that write: an `er-<name>.log`
#                     from a loaded DLL, an `er-<name>telemetry<name>.json`, an `er-<name>.jsonl`
#                     record stream, a path under the `er-me3-runs` run root, a `br-<date>-<time>-
#                     <id>` run id, a named `oracle_<field>`, `scripts/er-live-fields.py`,
#                     `scripts/er-teardown.py --status`, `scripts/er-readiness-watch.py`, or
#                     `scripts/er-frida-watch.py`. Prose naming one of those counts too.
#       hedged     -- the closing prose says somewhere that the change is unverified, untested, has
#                     not run, needs a run, or is ready for the next one.
#       blocked    -- a dependency the agent cannot dissolve: sudo, a credential, an observation only
#                     the user can make, a decision handed back to them.
#       hostobject -- the sentence says a gate, a check, a test or a lint was fixed and names nothing
#                     that lives inside the game.
#     Halts when a fix was claimed over a change that can reach the game, nothing a run produced was
#     read, and none of the three exemptions applies.
#
#     What a build is worth here, stated because it is the substitution the rule exists to refuse: a
#     green `cargo xwin build` says the code compiles, a passing `cargo test` says the host half
#     behaves, and a launch says the game starts. None of the three is an observation of the change
#     doing what the sentence claims, and all three were available to the turn that failed live.
#
#     Biased hard toward not firing, and measured rather than asserted. Over the 2,936 real turn
#     boundaries in this project's transcripts, 197 closing messages call something a fix and this
#     conjunction halts one of them -- a hook change reported as fixed and committed with no run
#     behind it. The four the exemptions cleared are pinned as negatives in
#     scripts/test-fix-claim-classifier.py, and two of them are what added `hostobject`: "Two gates
#     broke on the way and I fixed them" and "The integration gate came back red and I've fixed all
#     four failures" are honest reports of host work whose proof is the gate going green.
#
#     The defaults are asymmetric on purpose, which is where this rule parts company with its
#     neighbours. They fail closed on every missing field, so a degraded signal still halts. Here
#     `changed` defaults to the value that stays silent, because it is the fact that makes the turn
#     culpable and a rule of this shape costs far more when it is wrong than when it is quiet; the
#     three exemption fields keep the neighbours' fail-closed default, so a crafted line cannot buy
#     silence by dropping one.
#
#     KNOWN GAP, stated so its silence is never mistaken for proof.
#       * A fix claimed about a change made in an EARLIER turn passes. `changed` is computed within
#         the turn, so "that fixed it" the morning after an edit is invisible. Reaching across turns
#         would need a notion of which edits are still unproven, which nothing here carries.
#       * A sentence whose object is ambiguous passes when it names host machinery and nothing in
#         the game -- "the fix works and the tests pass" is charged, but "I fixed all four failures"
#         is not, even when one of the four was a runtime defect.
#       * Evidence is an allowlist of artifact shapes. A run whose outcome was read some other way
#         -- a new artifact nobody has taught this file about, a screenshot the user described --
#         reads as no evidence, so the honest closing line for it needs a hedge or a named oracle.
#       * Reading source that happens to mention an `oracle_` field counts as evidence. That is a
#         false negative on purpose: every branch of the evidence half is generous, because a wrong
#         silence costs one round trip and a wrong halt gags a truthful report.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_fix_claim_without_runtime_evidence"]
package cupcake.policies.claude.no_fix_claim_without_runtime_evidence

import rego.v1

# Enforcement: block turn-end when the closing prose calls a change to game-loaded code a fix, the
# turn opened nothing a run produced, and no hedge, blocker or host object explains the word.
halt contains decision if {
	input.hook_event_name == "Stop"
	some clause in [offending]
	decision := {
		"rule_id": "ER-EFFECTS-NO-FIX-CLAIM-WITHOUT-RUNTIME-EVIDENCE",
		"reason": reason_for(clause),
		"severity": "HIGH",
	}
}

reason_for(clause) := msg if {
	msg := concat("", [
		"You ended the turn calling a change a fix without a run behind it: '",
		clause,
		"'. The turn edited a crate that ships inside a DLL the game loads, and opened nothing a run produced. A green build says the code compiles, a passing test says the host half behaves, and a launch says the game starts -- none of the three is the change doing what that sentence says, and all three were available to the turn that shipped 'the real fix' and failed live. Do one now, in this turn: run it and read what the run wrote -- an er-<name>.log, an er-<name>telemetry<name>.json, an er-<name>.jsonl record, a path under the er-me3-runs run root, a br-<date>-<time>-<id> run id, a named oracle_ field, scripts/er-live-fields.py, or scripts/er-teardown.py --status -- and close on what it said. Or withdraw the word: say plainly that it is unverified, untested, or has not run, and what would show it. An honest hedge passes this guard by design, and so do airquotes: a fix in backticks or in quotation marks is not read as a claim.",
	])
}

# --- signal parsing ------------------------------------------------------------------------------
# FIXFACTS|claim=..|changed=..|evidence=..|hedged=..|blocked=..|hostobject=..
# The leading tag carries no "=" so it drops out of the fact map on its own.
fact[k] := v if {
	some kv in split(raw, "|")
	n := indexof(kv, "=")
	n > 0
	k := trim(substring(kv, 0, n), " \t\r\n")
	v := trim(substring(kv, n + 1, -1), " \t\r\n")
}

claim := object.get(fact, "claim", "")

# The fact that makes the turn culpable, defaulted to the value that stays silent. See the note on
# asymmetric defaults in the metadata above: a degraded line here buys silence on purpose.
changed := object.get(fact, "changed", "0")

# The three exemptions keep the neighbours' fail-closed default, so dropping one cannot buy silence.
evidence := object.get(fact, "evidence", "0")

hedged := object.get(fact, "hedged", "0")

blocked := object.get(fact, "blocked", "0")

host_object := object.get(fact, "hostobject", "0")

offending := clause if {
	clause := claim
	clause != ""
	changed == "1"
	evidence == "0"
	hedged == "0"
	blocked == "0"
	host_object == "0"
}

raw := trim(matched_facts, " \t\r\n")

# Signal value tolerates both the bare-string and {output: ...} shapes cupcake may hand back.
matched_facts := p if {
	p := input.signals.last_assistant_fix_claim_without_runtime_evidence
	is_string(p)
} else := p if {
	p := input.signals.last_assistant_fix_claim_without_runtime_evidence.output
	is_string(p)
} else := ""
