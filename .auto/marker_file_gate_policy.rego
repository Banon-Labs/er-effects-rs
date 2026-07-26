package auto.marker_file_gate

import rego.v1

# Marker-text-file FEATURE GATES are forbidden
# (deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19). A "marker-file gate" is any
# .join("er-effects-<name>.txt") in crates/er-effects-rs/src/**/*.rs consumed by .exists() -- the
# boolean on/off toggle shape, SEMANTICALLY IDENTICAL to an env-var gate. User directive: "we don't
# want any env/marker gated features."
#
# The former grandfathering allowlist sanctioned_marker_gate_names and the migrate_to_default
# ratchet in .auto/marker_file_gate_baseline.json are DEPRECATED and must stay EMPTY; the enforcing
# checker fails if either is re-populated. With the behavioral allowlist empty, every marker gate is
# denied UNLESS it is a sanctioned DIAGNOSTIC toggle: its NAME appears in `diagnostic_gates` with a
# non-empty rationale AND the enclosing fn does not classify as behavioral. diagnostic_gates is the
# ONLY exception and is reserved for toggles that change NO game behavior (passive
# log/telemetry/trace, read-only sampling). A behavioral fix must be DEFAULT behavior (gated only on
# the genuine runtime condition) or removed.
#
# The enforcing checker is scripts/check-marker-file-gates.py (rego cannot read the source tree at
# eval time); this policy is the declarative statement of intent the checker asserts-as-text so it
# cannot silently drift or disappear.

default allow := false

# A marker gate is allowed ONLY when it is a sanctioned diagnostic toggle carrying a rationale. The
# checker sets input.marker_diagnostic_sanctioned = (this gate's NAME is in diagnostic_gates and its
# fn is not behavioral) and input.marker_rationale_present = (that entry's rationale is non-empty).
allow if {
	input.marker_diagnostic_sanctioned
	input.marker_rationale_present
}

deny contains message if {
	not input.marker_diagnostic_sanctioned
	message := "marker feature gates (`<game_dir>/er-effects-*.txt` consumed by `.exists()`) are forbidden. Make the behavior DEFAULT (gated only on the genuine runtime condition) or remove it. Only a genuinely-diagnostic toggle that changes NO game behavior may be added to `diagnostic_gates` in .auto/marker_file_gate_baseline.json as a reviewed exception; a behavioral fn is rejected even if listed."
}

deny contains message if {
	input.marker_diagnostic_sanctioned
	not input.marker_rationale_present
	message := "a diagnostic_gates entry must carry a non-empty rationale justifying why this marker toggle changes no game behavior."
}
