# METADATA
# scope: package
# title: No Authoring Scripts Into /tmp
# description: Author scripts/source in the repo (scripts/, scripts/ghidra/), not /tmp; /tmp is for artifacts only.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-NO-TMP-SCRIPTS-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Write", "Edit", "MultiEdit", "NotebookEdit"]
package cupcake.policies.edit_no_tmp_scripts_guard

import rego.v1

# Source/script extensions that should live in the repo (reviewable, version-controlled,
# reusable across sessions) -- never authored into the volatile /tmp tree. DATA artifacts
# (.tsv, .log, .json, .bin, .txt, .csv ...) are deliberately NOT listed: writing those to
# /tmp is fine and intended.
script_exts := {
	".py", ".sh", ".bash", ".rs", ".java", ".rego", ".js", ".ts", ".tsx", ".jsx",
	".go", ".c", ".cc", ".cpp", ".h", ".hpp", ".rb", ".pl", ".lua", ".ps1",
}

file_path := input.tool_input.file_path

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	authoring_tool
	in_tmp
	is_script_file

	decision := {
		"rule_id": "ER-EFFECTS-NO-TMP-SCRIPTS-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake paused authoring a script into /tmp: ",
			file_path,
			"\n\nWhy this policy exists: /tmp is volatile (lost on reboot) and unversioned. Scripts authored there cannot be reviewed, reused across sessions, or shared with other agents. GhidraScripts repeatedly ended up stranded in /tmp/ghidra_scripts/.",
			"\n\nHappy path: author the script under the repo -- general helpers in `scripts/`, Ghidra postScripts in `scripts/ghidra/` -- and have IT write its data ARTIFACTS to /tmp or the session scratchpad if needed. (Data files like .tsv/.log/.json/.bin to /tmp are allowed; only source/scripts are blocked.)",
		]),
	}
}

authoring_tool if input.tool_name == "Write"

authoring_tool if input.tool_name == "Edit"

authoring_tool if input.tool_name == "MultiEdit"

authoring_tool if input.tool_name == "NotebookEdit"

in_tmp if startswith(file_path, "/tmp/")

is_script_file if {
	some ext in script_exts
	endswith(file_path, ext)
}
