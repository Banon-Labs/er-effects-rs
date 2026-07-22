package auto.env_gate_comment

import rego.v1

# Env FEATURE GATES are forbidden (deprecate-env-marker-gate-allowlists-no-gated-features-2026-07-19).
# An "env gate" is any read of std::env::var("ER_EFFECTS_...") in
# crates/er-effects-rs/src/**/*.rs. User directive: "we don't want any env gated features."
#
# The former grandfathering allowlists (sanctioned_env_vars, sanctioned_env_gate_locations,
# baseline) in .auto/env_gate_comment_baseline.json are DEPRECATED and must stay EMPTY; the
# enforcing checker fails if any is re-populated. With the behavioral allowlist empty, every env
# gate is denied UNLESS it is a sanctioned DIAGNOSTIC read: its exact key (ENV_VAR@repo/path.rs)
# appears in `diagnostic_gates` with a non-empty rationale. diagnostic_gates is the ONLY exception
# and is reserved for reads that change NO game behavior (passive log/telemetry/trace, read-only
# sampling, or a diagnostic output-path/tuning override). A behavioral feature must be DEFAULT
# behavior (gated only on a real runtime condition) or removed.
#
# The enforcing checker is scripts/check-env-gate-comments.py (rego cannot read the source tree at
# eval time); this policy is the declarative statement of intent the checker asserts-as-text so it
# cannot silently drift or disappear.

default allow := false

# A gate is allowed ONLY when it is a sanctioned diagnostic read carrying a rationale. The checker
# sets input.env_gate_diagnostic_sanctioned = (this gate's ENV_VAR@path key is in diagnostic_gates)
# and input.env_gate_rationale_present = (that entry's rationale is non-empty).
allow if {
	input.env_gate_diagnostic_sanctioned
	input.env_gate_rationale_present
}

deny contains message if {
	not input.env_gate_diagnostic_sanctioned
	message := "env feature gates are forbidden. Make the behavior DEFAULT (gated only on a real runtime condition) or remove it. Only a genuinely-diagnostic read that changes NO game behavior may be added to `diagnostic_gates` in .auto/env_gate_comment_baseline.json as a reviewed exception."
}

deny contains message if {
	input.env_gate_diagnostic_sanctioned
	not input.env_gate_rationale_present
	message := "a diagnostic_gates entry must carry a non-empty rationale justifying why this env read changes no game behavior."
}
