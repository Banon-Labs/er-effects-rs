package auto.runtime_experiment

import rego.v1

default allow := false

max_timeout_seconds := 60

manual_event_driver_ready if {
    input.readiness_watcher == "scripts/er-readiness-watch.py"
    input.no_telemetry_bootstrap_failure == "window_without_bootstrap_or_task_ready"
    input.host_input == "none"
    input.teardown == "process_tree_and_save_restore"
    input.timeout_seconds > 0
    input.timeout_seconds <= max_timeout_seconds
}

allow if {
    manual_event_driver_ready
}

deny contains message if {
    not allow
    message := "runtime probes are disabled fail-closed unless the manual readiness-driver contract is satisfied"
}
