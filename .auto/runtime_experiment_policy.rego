package auto.runtime_experiment

import rego.v1

default allow := false

# Kept in sync with the single source of truth .auto/runtime_timeout_cap_seconds by
# scripts/check-runtime-probe-contract.py (rego cannot read files at eval time). This is the GAME
# runtime idle/stall backstop (semaphore-driven early teardown is the primary bound); non-game
# timeouts are separately hard-capped at 30s by scripts/check-no-timeouts.py.
max_timeout_seconds := 180

manual_event_driver_ready if {
    input.readiness_watcher == "scripts/er-readiness-watch.py"
    input.no_telemetry_bootstrap_failure == "window_without_bootstrap_or_task_ready"
    input.host_input == "none"
    input.teardown == "process_tree_and_save_restore"
    input.legal_popup_check == "native_messagebox_and_packed_asset_tos_fmg_fail_fast"
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
