package auto.runtime_experiment

import rego.v1

base_ready_input := {
	"explicit_opt_in": true,
	"launch_mode": "steam",
	"timeout_seconds": 60,
	"native_title_accept_gate": false,
	"runtime_entrypoint": "measure_runtime_trigger",
	"readiness_watcher": "scripts/er-readiness-watch.py",
	"no_telemetry_bootstrap_failure": "window_without_bootstrap_or_task_ready",
	"host_input": "none",
	"teardown": "process_tree_and_save_restore",
}

accept_gate_input := {
	"explicit_opt_in": true,
	"launch_mode": "steam",
	"timeout_seconds": 60,
	"native_title_accept_gate": true,
	"runtime_entrypoint": "measure_runtime_trigger",
	"readiness_watcher": "scripts/er-readiness-watch.py",
	"no_telemetry_bootstrap_failure": "window_without_bootstrap_or_task_ready",
	"host_input": "none",
	"teardown": "process_tree_and_save_restore",
}

over_timeout_input := object.union(base_ready_input, {"timeout_seconds": 61})
boundary_timeout_input := object.union(base_ready_input, {"timeout_seconds": 60})
missing_timeout_input := {key: value |
	some key, value in base_ready_input
	key != "timeout_seconds"
}

test_ready_input_allowed if {
	allow with input as base_ready_input
}

test_boundary_timeout_allowed if {
	allow with input as boundary_timeout_input
}

test_native_title_accept_gate_denied if {
	not allow with input as accept_gate_input
	deny["runtime probe rejected: native title accept-gate mutation is banned after user-visible framerate/menu perturbation"] with input as accept_gate_input
}

test_over_timeout_denied if {
	not allow with input as over_timeout_input
	deny["runtime probe rejected: timeout_seconds must be present, numeric, greater than 0, and no more than 60"] with input as over_timeout_input
}

test_missing_timeout_denied if {
	not allow with input as missing_timeout_input
	deny["runtime probe rejected: timeout_seconds must be present, numeric, greater than 0, and no more than 60"] with input as missing_timeout_input
}
