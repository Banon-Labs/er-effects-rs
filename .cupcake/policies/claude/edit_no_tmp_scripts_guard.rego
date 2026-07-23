# METADATA
# scope: package
# title: No Authoring Scripts Into /tmp
# description: Author scripts/source in the repo (scripts/, scripts/ghidra/), not /tmp; /tmp is for artifacts only.
# custom:
#   severity: MEDIUM
#   id: ER-EFFECTS-NO-TMP-SCRIPTS-GUARD
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Write", "Edit", "MultiEdit", "NotebookEdit", "Bash"]
package cupcake.policies.claude.edit_no_tmp_scripts_guard

import rego.v1

# Source/script extensions that should live in the repo (reviewable, version-controlled,
# reusable across sessions) -- never authored into the volatile /tmp tree. DATA artifacts
# (.tsv, .log, .json, .bin, .txt, .csv ...) are deliberately NOT listed: writing those to
# /tmp is fine and intended.
script_exts := {
	".py", ".sh", ".bash", ".rs", ".java", ".rego", ".js", ".ts", ".tsx", ".jsx",
	".go", ".c", ".cc", ".cpp", ".h", ".hpp", ".rb", ".pl", ".lua", ".ps1",
}

tool_input := object.get(input, "tool_input", {})
file_path := object.get(tool_input, "file_path", object.get(tool_input, "path", ""))
command := object.get(tool_input, "command", "")
lower_tool_name := lower(object.get(input, "tool_name", ""))

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	authoring_tool
	in_tmp
	is_script_file

	decision := tmp_script_decision(file_path)
}

deny contains decision if {
	input.hook_event_name == "PreToolUse"
	bash_tool
	bash_authors_tmp_script

	decision := tmp_script_decision(tmp_script_path_from_command)
}

tmp_script_decision(path) := decision if {

	decision := {
		"rule_id": "ER-EFFECTS-NO-TMP-SCRIPTS-GUARD",
		"severity": "MEDIUM",
		"reason": concat("", [
			"🧁 Cupcake paused authoring a script into /tmp: ",
			path,
			"\n\nWhy this policy exists: /tmp is volatile (lost on reboot) and unversioned. Scripts authored there cannot be reviewed, reused across sessions, or shared with other agents. GhidraScripts repeatedly ended up stranded in /tmp/ghidra_scripts/.",
			"\n\nHappy path: author the script under the repo -- general helpers in `scripts/`, Ghidra postScripts in `scripts/ghidra/` -- and have IT write its data ARTIFACTS to /tmp or the session scratchpad if needed. (Data files like .tsv/.log/.json/.bin to /tmp are allowed; only source/scripts are blocked.)",
		]),
	}
}

authoring_tool if lower_tool_name == "write"

authoring_tool if lower_tool_name == "edit"

authoring_tool if lower_tool_name == "multiedit"

authoring_tool if lower_tool_name == "notebookedit"

authoring_tool if endswith(lower_tool_name, ".write")

authoring_tool if endswith(lower_tool_name, ".edit")

bash_tool if lower_tool_name == "bash"

bash_tool if lower_tool_name == "ctx_execute"

bash_tool if endswith(lower_tool_name, ".bash")

bash_tool if contains(lower_tool_name, "ctx_execute")

in_tmp if startswith(file_path, "/tmp/")

is_script_file if is_script_path(file_path)

is_script_path(path) if {
	some ext in script_exts
	endswith(path, ext)
}

tmp_script_path_from_command := path if {
	parts := regex.find_n(`/tmp/[^\s"'\\;&|<>]+`, command, -1)
	some path in parts
	is_script_path(path)
}

bash_authors_tmp_script if {
	tmp_script_path_from_command != ""
	regex.match(`(?is)(cat|tee|printf|echo|write-output|set-content|out-file|new-item|copy-item|python3?|perl|ruby|node|bun).*[/\\]tmp[/\\].*\.(py|sh|bash|rs|java|rego|js|ts|tsx|jsx|go|c|cc|cpp|h|hpp|rb|pl|lua|ps1)`, command)
}
