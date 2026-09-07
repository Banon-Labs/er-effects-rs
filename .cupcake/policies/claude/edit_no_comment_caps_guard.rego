# METADATA
# scope: package
# title: No Shouted Words In Comments
# description: Capitals in a comment name something; they are never emphasis. Refuse the write.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-COMMENT-CAPS-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Write", "Edit", "MultiEdit", "NotebookEdit"]
package cupcake.policies.claude.edit_no_comment_caps_guard

import rego.v1

# WHY A GUARD WHEN `scripts/check-comment-caps.py` ALREADY GATES THIS.
#
# That gate is the authority: it carries 24,627 English words, reads every comment and Python
# docstring in the tree, and knows the exemptions. It also runs at `check.sh` time, which is
# minutes-to-hours after the sentence was written and often a whole session later, and by then the
# shouting is spread across a dozen files. This refuses the write itself, while the author is still
# holding the sentence.
#
# It is DELIBERATELY NARROWER THAN THE GATE, in one direction only: it under-reports. It carries a
# short list of English function words that are never acronyms, never registers and never mnemonics
# in this repo, and it stops reading a line at the first backtick or double quote rather than
# tracking spans. So `the ONLY writer` is refused, `` `NOT` is the x86 instruction `` is not read at
# all, and a shouted word after a quotation on the same line is missed. A miss costs a red gate
# later, which is the status quo; a false positive costs a refused edit the author cannot fix, which
# is worse. The asymmetry is the design.
#
# The word list is the head of the measured distribution, from the sweep that took this tree to
# zero: `NOT` 1637, `THE` 1603, `IS` 750, `THIS` 518, `ONLY` 483, `ONE` 465, `BEFORE` 302, `SAME`
# 286, `EVERY` 241, `FIRST` 240. Excluded on purpose: `AND`, `OR`, `SET`, `CALL`, `TEST`, `PUSH`,
# `OUT`, `IN` -- all x86 mnemonics this repo quotes constantly.
shouted_words_re := `\b(NOT|THE|THIS|THAT|ONLY|ONE|TWO|EVERY|NEVER|ALWAYS|MUST|SAME|BOTH|OWN|WHY|HOW|WHAT|WHICH|WHO|BEFORE|AFTER|FIRST|SECOND|NOTHING|ANY|ALL|HERE|ACTUALLY|EXACTLY|REAL|WRONG|INSTEAD|WITHOUT|WHOLE|ONCE|STILL|EITHER|NEITHER|RATHER|ITSELF|EACH|THEN|THAN|BECAUSE|CANNOT|ALREADY|DELIBERATELY|GENUINELY|SILENTLY|MEASURED|PROVEN|IDENTICAL|IS|WAS|ARE|SHOULD|WOULD|COULD)\b`

# The file types `scripts/check-comment-caps.py` scans. Vendored upstream source is excluded there
# and here: its prose belongs to its authors.
scanned_exts := {".rs", ".py", ".sh", ".bash"}

tool_input := object.get(input, "tool_input", {})

file_path := object.get(tool_input, "file_path", object.get(tool_input, "path", ""))

lower_tool_name := lower(object.get(input, "tool_name", ""))

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	scanned_file
	some text in written_texts
	some line in split(text, "\n")
	shouted_line(line)

	decision := {
		"rule_id": "ER-EFFECTS-COMMENT-CAPS-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake stopped a shouted word going into a comment in ",
			file_path,
			":\n\n    ",
			trim_space(line),
			"\n\nCapitals in a comment name something -- a const/static, an env var, an acronym, an x86 register or mnemonic, a file or section name, an upstream FUN_<addr> symbol. They are never emphasis. This tree was swept to zero shouted words on 2026-09-07 and `scripts/check-comment-caps.py` (wired into check.sh) holds it there, so this edit would go red at push time.",
			"\n\nHappy path: lowercase the word. If it really is a name -- the x86 NOT, a \"NOT RUN\" status string, a constant that happens to be an English word -- put it in backticks and this guard stops reading there. If it is load-bearing, the fix is usually a rewritten first sentence, not a louder one. See AGENTS.md, `Conventions & Patterns`.",
		]),
	}
}

# Only the tools that author file content, and only the field each one writes.
written_texts contains text if {
	lower_tool_name in {"write", "notebookedit"}
	text := object.get(tool_input, "content", object.get(tool_input, "new_source", ""))
}

written_texts contains text if {
	lower_tool_name == "edit"
	text := object.get(tool_input, "new_string", "")
}

written_texts contains text if {
	lower_tool_name == "multiedit"
	some edit in object.get(tool_input, "edits", [])
	text := object.get(edit, "new_string", "")
}

scanned_file if {
	some ext in scanned_exts
	endswith(file_path, ext)
	not vendored
}

# Both spellings reach this guard: an absolute path from the harness, and the repo-relative one a
# test (or a relative edit) carries.
vendored if contains(file_path, "/third_party/")

vendored if startswith(file_path, "third_party/")

shouted_line(line) if {
	comment_line(line)

	# A comment-shaped line inside a string literal is generated source, not prose: `build.rs`
	# writes Rust that contains comments, and those carry an escaped newline on every line.
	not contains(line, "\\n")

	regex.match(shouted_words_re, quotable_prefix(line))
}

comment_line(line) if startswith(trim_space(line), "//")

comment_line(line) if startswith(trim_space(line), "#")

# Everything before the first backtick or double quote. Past that point a shouted word is probably
# being QUOTED rather than shouted, and this guard would rather miss one than refuse a correct edit.
quotable_prefix(line) := substring(line, 0, cut) if {
	cut := first_quote(line)
}

first_quote(line) := tick if {
	tick := quote_at(line, "`")
	quote := quote_at(line, "\"")
	tick <= quote
}

first_quote(line) := quote if {
	tick := quote_at(line, "`")
	quote := quote_at(line, "\"")
	quote < tick
}

quote_at(line, mark) := at if {
	at := indexof(line, mark)
	at >= 0
} else := count(line)
