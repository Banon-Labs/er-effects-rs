package auto.marker_file_gate

import rego.v1

# A product BEHAVIORAL fix must not be gated behind a marker text file. A "marker-file
# gate" is any `.join("er-effects-<name>.txt")` in crates/er-effects-rs/src/**/*.rs whose
# result is consumed by `.exists()` -- the boolean on/off toggle shape from the
# recurring incident (user feedback 2026-07-19; bd memory
# no-marker-file-gating-for-product-fixes-2026-07-19):
#
#     fn reload_b73_hold_enabled() -> bool {
#         game_directory_path().unwrap_or_else(|| PathBuf::from("."))
#             .join("er-effects-reload-b73hold.txt").exists()
#     }
#     ... if reload_b73_hold_enabled() && <real condition> { <apply the fix> }
#
# A marker file consumed by `.exists()` is SEMANTICALLY IDENTICAL to an env-var gate,
# which AGENTS.md already forbids for product features ("Release/default behavior must
# not depend on agent-only environment variables"). A RE-backed behavioral fix must be
# DEFAULT, gated ONLY on the genuine runtime condition, and validated by booting (keep
# if it works, git revert if not). Diagnostic-only logging/telemetry MAY still be
# marker-gated; only behavioral FIXES must not be.
#
# The env half of this hole is frozen by scripts/check-env-gate-comments.py; this policy
# freezes the marker-file half. The enforcing checker is
# scripts/check-marker-file-gates.py (rego cannot read the source tree at eval time);
# this policy is the declarative statement of intent that the checker asserts-as-text so
# it cannot silently drift or disappear.

default allow := false

# FROZEN NAME ALLOWLIST (the hard gate). The exact set of sanctioned marker-file NAMES
# lives under `sanctioned_marker_gate_names` in .auto/marker_file_gate_baseline.json.
# The checker sets input.marker_name_sanctioned = (this gate's marker file name is in
# that list). A NEW marker gate is denied UNCONDITIONALLY -- reusing the ".exists()"
# toggle shape for another behavioral fix under a NEW name fails closed. Make the
# behavior default/product-state driven instead of adding another hidden toggle.
allow if {
	input.marker_name_sanctioned
}

deny contains message if {
	not input.marker_name_sanctioned
	message := "this marker file name is NOT in the frozen sanctioned allowlist (`sanctioned_marker_gate_names` in .auto/marker_file_gate_baseline.json). No new marker gates: a product behavioral fix consumed by `.exists()` must be DEFAULT behavior gated only on the genuine runtime condition, not hidden behind a `<game_dir>/er-effects-*.txt` toggle. Diagnostic-only logging may be marker-gated, in which case add the name as a reviewed exception."
}

# BEHAVIORAL classification is advisory (the checker attaches it to a finding and to the
# migrate_to_default TODO). A sanctioned marker that gates a BEHAVIORAL fix is
# allowlisted only transitionally and must be migrated to default-on behavior, then
# removed from both lists. That soft ratchet is a note, not a denial, so this policy does
# not fight the concurrent de-marker-gating work.
deny contains message if {
	input.marker_name_sanctioned
	input.classification == "behavioral"
	not input.migration_acknowledged
	message := "sanctioned marker gates a BEHAVIORAL fix -- migrate it to default-on (real runtime condition) and remove from the allowlist. Soft TODO, enforced as a note by the checker, not a hard failure."
}
