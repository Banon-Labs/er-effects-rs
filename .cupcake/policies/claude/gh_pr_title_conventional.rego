# METADATA
# scope: package
# title: Refuse a pull request title CI will reject
# authors: ["er-quickload agents"]
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-PR-TITLE-CONVENTIONAL
#   description: >-
#     Refuse `gh pr create` / `gh pr edit` whose --title is not a Conventional
#     Commits subject. The `pr-title` job runs
#     scripts/conventional-commit-subject.py against the PR title and fails the
#     check suite; nothing local said so until the red X arrived.
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
package cupcake.policies.claude.gh_pr_title_conventional

import rego.v1

# A commit subject is gated locally by a hook; the PR title was not gated anywhere.
#
# 2026-09-09: PR #421 opened with the title "Cancel a local invasion that lands in the wrong
# place" and the `pr-title` job failed five seconds later --
# `scripts/conventional-commit-subject.py --subject "$PR_TITLE"` refusing it for having no type.
# The identical rule already fires on every commit through
# `scripts/hooks/commit-msg`, so the type was written correctly fifteen times in a row on the
# same branch and then omitted at the one place nothing was checking.
#
# The pattern here is a deliberate SUBSET of the python checker: the type list and the
# `<type>[(scope)][!]: <description>` shape, which is what the failure was. Length limits,
# trailing punctuation and mood are left to the gate that owns them -- this is a cheap pre-flight
# at the moment the title is typed, not a second implementation to drift against the first.
# `scripts/conventional-commit-subject.py` remains the authority.

command := object.get(input.tool_input, "command", "")

# `gh pr create` and `gh pr edit`, the two commands that set a title.
gh_pr_title_command if {
	regex.match(`(^|[[:space:];|&('"\x60])gh[[:space:]]+pr[[:space:]]+(create|edit)([[:space:]]|$)`, command)
}

# The title as typed, from either quoting style or bare. Only the first `--title` is read: a
# second one would be a different command's, and this guard is about the one being written.
# Three partial rules rather than one list, because a list literal holding an undefined value is
# itself undefined: with `[title_double, title_single, title_bare]` a single-quoted title made the
# whole set vanish and the guard fell silent. A partial set contributes only the arms that match.
titles contains found[1] if {
	found := regex.find_all_string_submatch_n(`--title[[:space:]]+"([^"]*)"`, command, 1)[0]
}

titles contains found[1] if {
	found := regex.find_all_string_submatch_n(`--title[[:space:]]+'([^']*)'`, command, 1)[0]
}

titles contains found[1] if {
	found := regex.find_all_string_submatch_n(`--title[[:space:]]+([^-"'[:space:]][^[:space:];|&]*)`, command, 1)[0]
}

# The Conventional Commits shape the CI job requires, and the same type list its error message
# prints. `!` marks a breaking change and is allowed between the scope and the colon.
conventional(subject) if {
	regex.match(`^(feat|fix|chore|docs|style|refactor|perf|test|build|ci|revert)(\([^)\n]+\))?!?: .+`, subject)
}

offending_title := t if {
	some t in titles
	not conventional(t)
}

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	input.tool_name == "Bash"
	gh_pr_title_command
	subject := offending_title

	decision := {
		"rule_id": "ER-EFFECTS-PR-TITLE-CONVENTIONAL",
		"severity": "HIGH",
		"reason": concat("", [
			"This pull request title is not a Conventional Commits subject, and CI's `pr-title` job ",
			"runs scripts/conventional-commit-subject.py against it -- so the check suite goes red ",
			"about five seconds after the PR is opened. Expected `<type>[(scope)][!]: ",
			"<description>` where the type is one of feat, fix, chore, docs, style, refactor, perf, ",
			"test, build, ci, revert. The type is what Release Please reads to pick the next ",
			"version: feat -> minor, fix -> patch, ! -> major.\n\n  title : ",
			subject,
			"\n\nRun `python3 scripts/conventional-commit-subject.py --subject \"<title>\"` to see ",
			"the authoritative verdict.",
		]),
	}
}
