package auto.runtime_experiment

import rego.v1

default allow := false

max_timeout_seconds := 120

valid_timeout if {
	is_number(input.timeout_seconds)
	input.timeout_seconds > 0
	input.timeout_seconds <= max_timeout_seconds
}

manual_event_driver_ready if {
	input.explicit_opt_in == true
	valid_timeout
	input.readiness_watcher == "scripts/er-readiness-watch.py"
	input.no_telemetry_bootstrap_failure == "window_without_bootstrap_or_task_ready"
	input.host_input == "none"
	input.teardown == "process_tree_and_save_restore"
	input.runtime_entrypoint == "measure_runtime_trigger"
	not input.native_title_accept_gate
	input.launch_mode in {"direct", "direct-protected", "steam", "attach-existing", "offline-launcher", "seamless"}
}

allow if {
	manual_event_driver_ready
}

deny contains message if {
	not input.explicit_opt_in
	message := "runtime probes are disabled fail-closed unless AUTO_ALLOW_RUNTIME_PROBE=1 is set for a deliberate manual readiness probe"
}

deny contains message if {
	input.native_title_accept_gate
	message := "runtime probe rejected: native title accept-gate mutation is banned after user-visible framerate/menu perturbation"
}

deny contains message if {
	not valid_timeout
	message := "runtime probe rejected: timeout_seconds must be present, numeric, greater than 0, and no more than 120"
}

deny contains message if {
	input.explicit_opt_in
	not manual_event_driver_ready
	message := "runtime probe rejected: require measure runtime trigger, timeout_seconds<=120, scripts/er-readiness-watch.py, no-telemetry bootstrap failure, host_input=none, process/save teardown, no native title accept-gate mutation, and an approved launch mode"
}
