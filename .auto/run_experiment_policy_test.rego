package auto.run_experiment

import rego.v1

base_input := {
	"command": "./.auto/measure.sh",
	"timeout_seconds": 60,
	"checks_timeout_seconds": 60,
}

over_timeout_input := object.union(base_input, {"timeout_seconds": 61})
missing_timeout_input := {key: value |
	some key, value in base_input
	key != "timeout_seconds"
}
over_checks_timeout_input := object.union(base_input, {"checks_timeout_seconds": 61})
wrong_command_input := object.union(base_input, {"command": "AUTO_RUNTIME_ENV_FILE=.auto/runtime-env ./.auto/measure.sh"})

test_base_allowed if {
	allow with input as base_input
}

test_over_timeout_denied if {
	not allow with input as over_timeout_input
	deny["run_experiment rejected: timeout_seconds must be present, numeric, greater than 0, and no more than 60"] with input as over_timeout_input
}

test_missing_timeout_denied if {
	not allow with input as missing_timeout_input
	deny["run_experiment rejected: timeout_seconds must be present, numeric, greater than 0, and no more than 60"] with input as missing_timeout_input
}

test_over_checks_timeout_denied if {
	not allow with input as over_checks_timeout_input
	deny["run_experiment rejected: checks_timeout_seconds, when present, must be numeric, greater than 0, and no more than 60"] with input as over_checks_timeout_input
}

test_wrong_command_denied if {
	not allow with input as wrong_command_input
	deny["run_experiment rejected: command must be exactly ./.auto/measure.sh"] with input as wrong_command_input
}
