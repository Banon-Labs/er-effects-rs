package auto.run_experiment

import rego.v1

default allow := false

max_timeout_seconds := 60

valid_command if {
	input.command == "./.auto/measure.sh"
}

valid_timeout if {
	is_number(input.timeout_seconds)
	input.timeout_seconds > 0
	input.timeout_seconds <= max_timeout_seconds
}

valid_checks_timeout if {
	not input.checks_timeout_seconds
}

valid_checks_timeout if {
	is_number(input.checks_timeout_seconds)
	input.checks_timeout_seconds > 0
	input.checks_timeout_seconds <= max_timeout_seconds
}

allow if {
	valid_command
	valid_timeout
	valid_checks_timeout
}

deny contains message if {
	not valid_command
	message := "run_experiment rejected: command must be exactly ./.auto/measure.sh"
}

deny contains message if {
	not valid_timeout
	message := "run_experiment rejected: timeout_seconds must be present, numeric, greater than 0, and no more than 60"
}

deny contains message if {
	not valid_checks_timeout
	message := "run_experiment rejected: checks_timeout_seconds, when present, must be numeric, greater than 0, and no more than 60"
}
