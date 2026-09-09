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

# This policy fires, and `scripts/check-runtime-evidence.sh` is the second enforcement point.
#
# It was inert for its first hours, and the header here said so at length and blamed
# `input.signals`, because a literal probe stopped firing the moment any rule read a signal. That
# diagnosis was wrong. The cause was `sprintf` in the deny reason: Cupcake's optimised WASM runtime
# does not implement it, so the builtin returns undefined and every rule whose body reaches one
# silently never fires while `cupcake eval` reports ALLOW and exits 0. The probe "stopped firing on
# `input.signals`" only because the signal-reading versions were the ones that also interpolated a
# note. Replacing it with `concat` made the policy deny on the first try.
#
# This repo already had a gate that names the whole class -- `scripts/check-cupcake-wasm-builtins.py`
# verifies 25 builtins against the live WASM runtime and prints the offending file. It was never run
# against this file, because `scripts/check.sh` was piped into `tail` once (which discards the
# verdict) and killed by a 120-second timeout the next time. The lesson is not about `sprintf`: a
# green `opa test` says nothing about production, and the gate that does say something was sitting
# in the suite the whole time.
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
		"reason": concat("", [
			"This push carries changes under crates/ that have never run. ",
			evidence_note,
			". Build (scripts/er-build-dlls.sh), launch, and let the DLL write its log before ",
			"pushing -- or push a commit that does not change game code. Override deliberately ",
			"with ER_ALLOW_UNPROVEN_PUSH=1 in the signal's environment if you are pushing ",
			"something you know is unproven and have said so.",
		]),
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

# The signal is a bare word and this is a string comparison, which keeps the parsing in bash where
# it can be selftested. That was originally chosen for a wrong reason -- see the header: the
# parsing forms that "did not survive the round trip" were not failing on parsing at all, they
# were failing on `sprintf` in the deny reason. The shape is kept because it is still the better
# one, not because the alternatives were measured to be broken.
#
# The lesson that does generalise: a cupcake policy is only real once it has been driven through
# `cupcake eval` and seen to DENY. A green `opa test` is necessary and is not evidence.
default evidence_verdict := "UNKNOWN"

evidence_verdict := trim_space(input.signals.runtime_evidence_for_head)

default evidence_note := "no measurement was available"

evidence_note := trim_space(input.signals.runtime_evidence_note)

