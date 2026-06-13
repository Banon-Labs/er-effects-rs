package auto.runtime_experiment

import rego.v1

allowed_launch_modes := ["direct", "direct-protected", "steam", "attach-existing"]

default allow := false

allow if {
	input.explicit_opt_in == true
	launch_mode_allowed
	count(deny) == 0
}

launch_mode_allowed if {
	some index
	input.launch_mode == allowed_launch_modes[index]
}

deny contains message if {
	input.explicit_opt_in != true
	message := "runtime experiment requires explicit AUTO_ALLOW_RUNTIME_PROBE=1 opt-in"
}

deny contains message if {
	not launch_mode_allowed
	message := sprintf("unsupported launch_mode %q", [input.launch_mode])
}

deny contains message if {
	count(trim(object.get(input, "readiness_strategy", ""), " \t\r\n")) < 60
	message := "runtime experiment must describe observable readiness/completion signals"
}

deny contains message if {
	count(trim(object.get(input, "structured_failure", ""), " \t\r\n")) < 60
	message := "runtime experiment must describe structured failure and teardown evidence"
}

deny contains message if {
	count(trim(object.get(input, "user_impact", ""), " \t\r\n")) < 60
	message := "runtime experiment must describe user-impact controls for explicit runtime opt-in"
}
