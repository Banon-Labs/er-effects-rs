# METADATA
# scope: package
# title: Require Fresh origin/main for PR Rebase and Force Push
# custom:
#   severity: HIGH
#   id: ER-EFFECTS-REQUIRE-FRESH-ORIGIN-MAIN
#   routing:
#     required_events: ["PreToolUse"]
#     required_tools: ["Bash"]
#     required_signals: ["origin_main_oids"]
package cupcake.policies.claude.git_require_fresh_origin_main

import rego.v1

# Missing or unequal OIDs fail closed. Fetch origin/main before rebasing a PR
# branch onto it or force-pushing that branch.
deny contains decision if {
    input.hook_event_name == "PreToolUse"
    input.tool_name == "Bash"
    command := lower(input.tool_input.command)
    guarded(command)
    not fresh
    decision := {"rule_id": "ER-EFFECTS-REQUIRE-FRESH-ORIGIN-MAIN", "reason": "origin/main is stale or could not be verified against origin. Run `git fetch origin main`, then retry the rebase or force-push.", "severity": "HIGH"}
}

guarded(command) if { rebase_origin_main(command) }
guarded(command) if { force_push(command) }
rebase_origin_main(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+rebase([[:space:]]+[^;&|\n]+)*[[:space:]]+origin/main([[:space:]]|$|[;&|\n])`, command) }
force_push(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+push([[:space:]]+[^;&|\n]+)*[[:space:]]+--force(-with-lease)?(=[^[:space:];&|]+)?([[:space:]]|$|[;&|\n])`, command) }
force_push(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+push([[:space:]]+[^;&|\n]+)*[[:space:]]+-[[:alnum:]]*f[[:alnum:]]*([[:space:]]|$|[;&|\n])`, command) }
force_push(command) if { regex.match(`(?m)(^|[;&|][[:space:]]*|\n)[[:space:]]*(command[[:space:]]+)?git([[:space:]]+(-C|-c)[[:space:]]+[^[:space:];&|]+)*[[:space:]]+push([[:space:]]+[^;&|\n]+)*[[:space:]]+\+[^[:space:];&|]+`, command) }

fresh if { parts := split(trim_space(signal), " "); count(parts) == 2; valid_oid(lower(parts[0])); valid_oid(lower(parts[1])); lower(parts[0]) == lower(parts[1]) }
valid_oid(oid) if { regex.match(`^[0-9a-f]{40}([0-9a-f]{24})?$`, oid) }
signal := value if { value := input.signals.origin_main_oids; is_string(value) } else := value if { value := input.signals.origin_main_oids.output; is_string(value) } else := "" if { true }
