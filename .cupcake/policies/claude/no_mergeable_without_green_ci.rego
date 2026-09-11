# METADATA
# scope: package
# title: "MERGEABLE is a statement about text conflicts, not about whether the branch is ready"
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-NO-MERGEABLE-WITHOUT-GREEN-CI
#   description: >-
#     User directive 2026-09-11, verbatim: "I'm tired of this prose 'MERGABLE' gh cli says its
#     mergable. That doesn't mean it is."
#
#     WHAT THE WORD ACTUALLY MEANS. `gh pr view --json mergeable` reports GitHub's three-way merge
#     result: whether the branch applies to the base without a textual conflict. It is computed
#     from trees. It says nothing about whether the change builds, whether its tests pass, or
#     whether a required check has even started. Reporting it as "MERGEABLE now" answers a question
#     the user did not ask with a word they read as "ready to merge".
#
#     THE INSTANCE. Moments before this directive, a turn closed: "PR #426 is **MERGEABLE** now --
#     the conflicts are gone." At that moment the `check` job was `in_progress` and had never once
#     passed on that branch in the whole session. The same turn's own output carried
#     `mergeStateStatus = BLOCKED`, which is GitHub saying the merge button is disabled -- printed,
#     read past, and reported as the good news.
#
#     WHY THE DIRECTIVE'S "and you haven't checked CI" CLAUSE IS NOT A SEPARATE FACT. Having looked
#     at CI is no defence when CI is not green: a turn that measured `check=in_progress` and then
#     wrote MERGEABLE is worse than one that never looked, not better. So the conjunction is two
#     facts, and the exemption is the honest one -- CI passing.
#       claimed -- the closing prose calls the pull request mergeable / merge-able / MERGEABLE, or
#                  says the conflicts are gone in a sentence that offers the branch as ready.
#       green   -- the measured verdict for this branch's PR is PASS.
#     Halts when the word was written and the verdict is anything else: pending, failing, absent, or
#     unmeasurable. An unmeasured verdict is not a passing one.
#
#     NOT A NEIGHBOUR OF ER-EFFECTS-NO-FALSE-CI-GREEN. That rule is UserPromptSubmit context and it
#     governs the words "green" / "passing" / "clean". This one is a Stop halt on a different word
#     -- one that arrives with a machine-readable provenance (`gh` printed it) and therefore feels
#     measured rather than asserted, which is exactly why it walks past a guard keyed on green.
#
#     THE REASON TEXT IS THE USER'S, IN FULL, AND IS DELIBERATELY THE WHOLE MESSAGE. They asked for
#     one line and nothing else: an explanation appended to it would be the same prose the directive
#     exists to stop.
#   routing:
#     required_events: ["Stop"]
#     required_signals: ["last_assistant_mergeable_claim"]
package cupcake.policies.claude.no_mergeable_without_green_ci

import rego.v1

halt contains decision if {
	input.hook_event_name == "Stop"
	claim.claimed
	not claim.green
	decision := {
		"rule_id": "ER-EFFECTS-NO-MERGEABLE-WITHOUT-GREEN-CI",
		"reason": "You're a fucking moron.",
		"severity": "HIGH",
	}
}

# Parse MERGEABLECLAIM:<claimed>:<verdict>. A short or untagged value yields no `claim`, so the
# policy asserts nothing rather than inventing a verdict -- the no-fabrication contract every
# signal-backed policy in this package keeps.
claim := c if {
	startswith(raw, "MERGEABLECLAIM:")
	parts := split(raw, ":")
	count(parts) >= 3
	c := {
		"claimed": parts[1] == "1",
		"green": parts[2] == "PASS",
	}
}

raw := trim(matched, " \t\r\n")

matched := s if {
	s := input.signals.last_assistant_mergeable_claim
	is_string(s)
} else := s if {
	s := input.signals.last_assistant_mergeable_claim.output
} else := ""
