package auto.runtime_experiment

import rego.v1

default allow := false

manual_event_driver_ready if {
	input.explicit_opt_in == true
	input.readiness_watcher == "scripts/er-readiness-watch.py"
	input.no_telemetry_bootstrap_failure == "window_without_bootstrap_or_task_ready"
	input.host_input == "none"
	input.teardown == "process_tree_and_save_restore"
	input.runtime_entrypoint == "measure_runtime_trigger"
	input.launch_mode in {"direct", "direct-protected", "steam", "attach-existing"}
}

allow if {
	manual_event_driver_ready
}

deny contains message if {
	not input.explicit_opt_in
	message := "runtime probes are disabled fail-closed unless AUTO_ALLOW_RUNTIME_PROBE=1 is set for a deliberate manual readiness probe"
}

deny contains message if {
	input.explicit_opt_in
	not manual_event_driver_ready
	message := "runtime probe rejected: require measure runtime trigger, scripts/er-readiness-watch.py, no-telemetry bootstrap failure, host_input=none, process/save teardown, and an approved launch mode"
}
