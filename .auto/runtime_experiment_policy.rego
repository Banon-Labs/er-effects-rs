package auto.runtime_experiment

import rego.v1

default allow := false

deny contains message if {
	message := "runtime probes are disabled fail-closed; static autoresearch measurement only until the event-driven runtime driver has a no-telemetry bootstrap failure path"
}
