# METADATA
# scope: package
# title: Refuse a push of game code that has never run
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-REQUIRE-RUNTIME-EVIDENCE
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["runtime_evidence_for_head", "runtime_evidence_note"]
package cupcake.policies.claude.git_require_runtime_evidence

import rego.v1

import data.cupcake.system.commands

# This policy does not fire, and `scripts/check-runtime-evidence.sh` is the enforcement.
#
# Read that sentence before trusting anything below it. Written 2026-09-09 with 17 passing OPA
# tests, this was driven through `cupcake eval` and produced zero decisions. A literal probe added
# to this same file fired on its own and stopped firing the moment any rule referenced
# `input.signals` -- including `current_branch`, which the sibling `git_block_main_push` reads
# successfully in production. Every parsing form was tried and none of them mattered: a colon split
# rejoined with `array.slice`, a pipe split without it, `else` chains, `default` rules, and a bare
# signal read with no parsing at all. The cause is inside cupcake's optimised WASM lowering and is
# not visible from outside it; `opa test`, `opa check` and the `cupcake.system.evaluate`
# aggregation entrypoint all agree the policy is correct.
#
# It is kept rather than deleted because the tests document the contract and a later cupcake may
# lower it, and it is labelled rather than left quiet because a guard that looks present and does
# nothing is the exact failure this file was written about. Do not cite it as coverage.
#
# A push of code that runs inside ELDEN RING is a claim that the code works. This refuses that
# claim when no run has executed the code being pushed.
#
# The failure it was written for, 2026-09-09. Two commits were authored and a push attempted for
# both: `enable_toggle_key` on F3, a key that had never been pressed in the game, and a
# stall-watchdog fix whose code had never executed, because the DLL in the running process had been
# built two commits earlier. The user stopped the push and asked for this guard in the same breath.
# AGENTS.md already carried the rule -- commit after a runtime validation run completes, and only
# if the run showed the change is worth keeping -- as prose. Prose did not stop it, twice in one
# evening, and the standing repo rule is that a correction which must survive the turn belongs in
# executable enforcement rather than in a note.
#
# What counts as evidence is decided by the signal, not here, and it is deliberately narrow: a DLL
# log whose own first line says `build git=<sha>` for the tip commit, with no `+dirty`. The first
# draft of that signal compared file mtimes instead and answered OK on a log written by a build two
# commits old that happened to still be running -- newer file, older code. A guard that reads a
# clock and calls it provenance reproduces the bug it is guarding.
#
# What this does NOT do, on purpose:
#   * It does not fire when no crate changed. A docs, scripts or policy push has nothing for a run
#     to prove, and a guard that fires on everything is one the next agent overrides by reflex.
#   * It does not fire when the signal cannot measure. UNKNOWN is not MISSING. A guard that cannot
#     see must not invent a verdict; the pre-push hook and CI still stand behind it.
#   * It does not adjudicate whether the run PASSED. That is a judgement about evidence, and this
#     asks only whether evidence for this code exists at all. A run that executed the code and went
#     badly is a fact worth pushing with; a run that never executed it is not evidence of anything.
deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	any_executed_push
	evidence_verdict == "MISSING"

	decision := {
		"rule_id": "ER-EFFECTS-REQUIRE-RUNTIME-EVIDENCE",
		"reason": sprintf(
			"This push carries changes under crates/ that have never run. %s. Build (scripts/er-build-dlls.sh), launch, and let the DLL write its log before pushing -- or push a commit that does not change game code. Override deliberately with ER_ALLOW_UNPROVEN_PUSH=1 in the signal's environment if you are pushing something you know is unproven and have said so.",
			[evidence_note],
		),
		"severity": "HIGH",
	}
}

executed_texts := commands.executed_texts(input.tool_input.command)

any_executed_push if {
	some text in executed_texts
	is_git_push(lower(text))
}

# The same invocation shape the main-push guard recognises, including `git -C <repo> push` and the
# global options that may sit before the verb. Kept as its own copy rather than imported: these two
# policies are evaluated independently and a shared helper would couple their failure modes.
git_push_command_pattern := `(^|[;&|(
])\s*(command\s+)?git([ \t]+((-c|--git-dir|--work-tree|--namespace|--config-env)(=|[ \t]+)("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+)|--(bare|no-pager|paginate|literal-pathspecs|no-replace-objects|exec-path)(=("[^"\n]*"|'[^'\n]*'|[^ \t;&|()\n]+))?))*[ \t]+push([ \t;&|)\n]|$)`

is_git_push(cmd) if {
	regex.match(git_push_command_pattern, cmd)
}

# The signal is a bare word and this is a string comparison, which is the whole design.
#
# Measured on 2026-09-09, and the reason this file looks simpler than its siblings: every parsing
# form tried here passed `opa test` and produced ZERO decisions inside cupcake's optimised WASM
# module. A colon-split rejoined with `array.slice`, a pipe-split without it, `else` chains and
# `default` rules were each confirmed inert by a literal probe that fired on its own and stopped
# firing the moment the rule referenced a parsed value. String equality against a signal that does
# its own parsing in bash is the one shape measured to survive the round trip.
#
# The lesson generalises past this file: a cupcake policy is only real if it has been driven
# through `cupcake eval`. `opa test` green is necessary and is not evidence.
default evidence_verdict := "UNKNOWN"

evidence_verdict := trim_space(input.signals.runtime_evidence_for_head)

default evidence_note := "no measurement was available"

evidence_note := trim_space(input.signals.runtime_evidence_note)

