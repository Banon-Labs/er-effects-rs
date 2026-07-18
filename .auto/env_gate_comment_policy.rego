package auto.env_gate_comment

import rego.v1

# Env-gated features must justify themselves. An "env-gated feature" is any read of
# std::env::var("ER_EFFECTS_...") in crates/er-effects-rs/src/**/*.rs. Reverse engineering breeds dozens of
# such gates; an undocumented one is a landmine for the next agent (does enabling it
# write a save? perturb the mount? is it a dead/disproven path?). Every NEW or
# newly-moved gate must carry a justifying comment directly above its enclosing fn.
#
# The enforcing checker is scripts/check-env-gate-comments.py (rego cannot read the
# source tree at eval time); this policy is the declarative statement of intent that
# the checker asserts-as-text so it cannot silently drift or disappear.

default allow := false

# FROZEN LOCATION ALLOWLIST (the hard gate). The exact set of sanctioned env-gate
# LOCATIONS lives under `sanctioned_env_gate_locations` in
# .auto/env_gate_comment_baseline.json. The checker sets
# input.env_gate_location_sanctioned = (this gate's stable key is in that list).
# A NEW env gate is denied UNCONDITIONALLY -- a rationale comment, a reused env-var
# name, or adding the name to `sanctioned_env_vars` does NOT rescue it. If behavior
# needs to become product/default behavior, tie it to existing product state or a
# dedicated non-env policy/contract instead of creating another runtime env knob.
deny contains message if {
	not input.env_gate_location_sanctioned
	message := "this env gate location is NOT in the frozen sanctioned location allowlist (`sanctioned_env_gate_locations` in .auto/env_gate_comment_baseline.json). No new env gates: do not add another ER_EFFECTS_* runtime knob. Tie the behavior to existing product state or a dedicated Rego/runtime contract instead."
}

# FROZEN NAME ALLOWLIST (secondary guard). The set of sanctioned ER_EFFECTS_* env-var
# NAMES lives under `sanctioned_env_vars` in .auto/env_gate_comment_baseline.json.
# The checker sets input.env_var_sanctioned = (this gate's name is in that list).
# An UNKNOWN name is denied UNCONDITIONALLY too -- this catches renamed gates and
# keeps the name inventory explicit even for already-known locations.
deny contains message if {
	input.env_gate_location_sanctioned
	not input.env_var_sanctioned
	message := "this env var is NOT in the frozen sanctioned name allowlist (`sanctioned_env_vars` in .auto/env_gate_comment_baseline.json). No new env gates: prefer existing product state or a dedicated Rego/runtime contract."
}

# A sanctioned (location + name allowlisted) gate is then additionally allowed when it carries a
# justifying comment (the checker sets input.has_rationale_comment by reading the
# contiguous //-comment block directly above the enclosing fn -- satisfied by
# EITHER a line containing the canonical marker `ENV-GATE RATIONALE`, OR a >=2-line
# `///` doc comment), OR when it is an already-known pre-existing gate recorded in
# the baseline ratchet (.auto/env_gate_comment_baseline.json), so day-one adoption
# did not explode.
allow if {
	input.env_gate_location_sanctioned
	input.env_var_sanctioned
	input.has_rationale_comment == true
}

allow if {
	input.env_gate_location_sanctioned
	input.env_var_sanctioned
	input.in_baseline == true
}

deny contains message if {
	input.env_gate_location_sanctioned
	input.env_var_sanctioned
	not allow
	message := "env-gated features (std::env::var(\"ER_EFFECTS_...\")) must carry a justifying comment directly above the enclosing fn -- a line with the marker `ENV-GATE RATIONALE` or a >=2-line `///` doc comment -- or be deleted so the feature is unconditional. To clear a baselined gate: add the comment, then remove its key from .auto/env_gate_comment_baseline.json."
}
