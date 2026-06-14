#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
cd "$REPO_ROOT"

if [[ -f "$HOME/.cargo/env" ]]; then
  # Non-interactive agent shells may not have Cargo on PATH.
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

if [[ "${AUTO_MEASURE_INNER:-0}" != "1" && -f "$REPO_ROOT/.auto/run-runtime-once" ]]; then
  runtime_request_path="$REPO_ROOT/.auto/run-runtime-once"
  runtime_request=$(tr -cd '[:print:]' < "$runtime_request_path" || true)
  mkdir -p "$REPO_ROOT/.auto"
  {
    printf 'started_at=%s\n' "$(date -Is)"
    printf 'request=%s\n' "$runtime_request"
    printf 'entrypoint=%s\n' "measure_runtime_trigger"
  } > "$REPO_ROOT/.auto/last-runtime-request-started"
  rm -f "$runtime_request_path"
  export AUTO_ALLOW_RUNTIME_PROBE=1
  echo "[measure] running explicit runtime probe request=$runtime_request" >&2
  exec "$REPO_ROOT/.auto/runtime_probe.sh"
fi

LOG_DIR="$REPO_ROOT/.auto/last-measure"
mkdir -p "$LOG_DIR"
rm -f "$LOG_DIR"/*.log 2>/dev/null || true

GATE_FAILED=0
TEST_PASS=1
BUILD_SECONDS="0"

now_ms() {
  python3 - <<'PY'
import time
print(int(time.time() * 1000))
PY
}

run_gate() {
  local name=$1
  shift
  local start end elapsed rc log_path
  log_path="$LOG_DIR/$name.log"
  start=$(now_ms)
  echo "[measure] gate $name: $*" >&2
  if "$@" >"$log_path" 2>&1; then
    rc=0
  else
    rc=$?
    GATE_FAILED=1
    TEST_PASS=0
    echo "[measure] gate $name failed with rc=$rc; log=$log_path" >&2
    tail -80 "$log_path" >&2 || true
  fi
  end=$(now_ms)
  elapsed=$((end - start))
  if [[ "$name" == "xwin_check" ]]; then
    BUILD_SECONDS=$(python3 - <<PY
print(f"{$elapsed / 1000:.3f}")
PY
)
  fi
  return 0
}

run_gate cargo_fmt cargo fmt --check
run_gate cargo_tests cargo test -p er-safe-input -p er-save-loader
run_gate runtime_probe_contract python3 scripts/check-runtime-probe-contract.py
run_gate runtime_probe_contract_tests python3 scripts/test-runtime-probe-contract.py
run_gate readiness_watch_tests python3 scripts/test-er-readiness-watch.py
run_gate xwin_check cargo xwin check --target x86_64-pc-windows-msvc --no-default-features
run_gate shellcheck_scripts shellcheck scripts/er-smoke-driver.sh
run_gate shellcheck_auto_runtime shellcheck .auto/runtime_probe.sh .auto/run_runtime_experiment.sh
run_gate runtime_experiment_rego opa check .auto/runtime_experiment_policy.rego
run_gate cupcake_validate cupcake validate --log-level error
run_gate cupcake_policy_regressions python3 scripts/test-cupcake-policies.py
run_gate smoke_preflight scripts/er-smoke-driver.sh preflight --no-build --no-install --no-launch --max-nudges 0
run_gate static_re_export python3 .auto/static_re_export.py "$LOG_DIR/static-re-evidence.json"

ER_PROCESS_COUNT=$(python3 - <<'PY'
import re
import subprocess
pattern = re.compile(r'(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)', re.I)
count = 0
output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
for line in output.splitlines():
    if pattern.search(line):
        count += 1
print(count)
PY
)
if [[ "$ER_PROCESS_COUNT" != "0" ]]; then
  GATE_FAILED=1
  TEST_PASS=0
  echo "[measure] hard gate failed: Elden Ring process still running" >&2
  python3 - <<'PY' >&2
import re
import subprocess
pattern = re.compile(r'(?:^|[/\\])(eldenring\.exe|start_protected_game\.exe)(?:\s|$)', re.I)
output = subprocess.check_output(["ps", "-eo", "pid=,args="], text=True)
for line in output.splitlines():
    if pattern.search(line):
        print(line)
PY
fi

export REPO_ROOT GATE_FAILED TEST_PASS BUILD_SECONDS ER_PROCESS_COUNT
python3 - <<'PY'
import json
import os
import re
import subprocess
from pathlib import Path

repo = Path(os.environ["REPO_ROOT"])
gate_failed = int(os.environ.get("GATE_FAILED", "0"))
test_pass = int(os.environ.get("TEST_PASS", "0"))
build_seconds = float(os.environ.get("BUILD_SECONDS", "0") or 0)
er_process_count = int(os.environ.get("ER_PROCESS_COUNT", "0") or 0)

metrics = {
    "autoload_success": 0,
    "player_available": 0,
    "selected_slot_loaded": 0,
    "time_to_player_seconds": -1,
    "game_save_state": -1,
    "game_save_slot": -1,
    "game_requested_save_slot_load_index": -1,
    "game_save_requested": 0,
    "title_bootstrap_seen": 0,
    "native_request_consumed": 0,
    "crash_detected": 0,
    "save_safety_ok": 1,
    "er_process_teardown_ok": 1 if er_process_count == 0 else 0,
    "host_pointer_input_used": 0,
    "simulated_button_presses_total": 0,
    "simulated_confirm_presses": 0,
    "simulated_cancel_presses": 0,
    "simulated_start_presses": 0,
    "simulated_dpad_up_presses": 0,
    "simulated_dpad_down_presses": 0,
    "simulated_dpad_left_presses": 0,
    "simulated_dpad_right_presses": 0,
    "simulated_left_bumper_presses": 0,
    "simulated_right_bumper_presses": 0,
    "menu_condition_evidence_score": 0,
    "input_reason_known": 0,
    "state_gated_input": 0,
    "input_explanation_bonus": 0,
    "trace_invasiveness_score": 0,
    "static_evidence_score": 0,
    "runtime_probe_seconds": 0,
    "build_seconds": build_seconds,
    "test_pass": test_pass,
    "code_complexity_delta": 0,
    "artifact_bytes": 0,
    "false_positives": 0,
}

src = (repo / "src/lib.rs").read_text(encoding="utf-8", errors="replace") if (repo / "src/lib.rs").exists() else ""
loader = (repo / "crates/er-save-loader/src/lib.rs").read_text(encoding="utf-8", errors="replace") if (repo / "crates/er-save-loader/src/lib.rs").exists() else ""
safe_input = (repo / "crates/er-safe-input/src/lib.rs").read_text(encoding="utf-8", errors="replace") if (repo / "crates/er-safe-input/src/lib.rs").exists() else ""
smoke_driver = (repo / "scripts/er-smoke-driver.sh").read_text(encoding="utf-8", errors="replace") if (repo / "scripts/er-smoke-driver.sh").exists() else ""
static_score = 0
if all(needle in src for needle in ["0xb72", "0xb73", "0xb78", "0xbc4", "game_man_trace_summary"]):
    static_score += 80
if all(needle in src for needle in ["MENU_CONTINUE_WRAPPER_RVA", "menu_continue_wrapper_hook", "TITLE_BOOTSTRAP_SEEN"]):
    static_score += 80
if all(needle in src for needle in ["SET_SAVE_SLOT_RVA", "SAVE_REQUEST_PROFILE_RVA", "REQUEST_SAVE_RVA", "CURRENT_SLOT_LOAD_RVA", "CONTINUE_LOAD_RVA"]):
    static_score += 80
if all(needle in loader for needle in ["DirectMenuLoad", "request_direct_menu_load", "save_buffer_allocator_ready", "title_bootstrap_seen"]):
    static_score += 80
if all(needle in src for needle in ["game_requested_save_slot_load_index", "game_save_state", "game_save_requested", "autoload_last_status"]):
    static_score += 80
# Bonus static evidence for the exact high-priority addresses when future RE notes/code add them.
combined_static_text = "\n".join([src, loader])
for needle in ["0x140af7a50", "0x140afab5f", "0x140afab6a", "0x140af1aa0", "0x1406793c0"]:
    if needle.lower() in combined_static_text.lower():
        static_score += 20

static_re_evidence_path = repo / ".auto/last-measure/static-re-evidence.json"
static_re_evidence = {}
if static_re_evidence_path.exists():
    try:
        static_re_evidence = json.loads(static_re_evidence_path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        static_re_evidence = {}
summary = static_re_evidence.get("summary", {}) if isinstance(static_re_evidence, dict) else {}
if summary.get("menu_other_load_calls_map_load"):
    static_score += 40
if summary.get("menu_load_wrappers_immediate_primitives_submit_state"):
    static_score += 40
if summary.get("menu_load_pump_update_maps_return_to_submit_state"):
    static_score += 40
if summary.get("menu_load_pump_update_selects_delta_or_default_by_delay"):
    static_score += 40
if summary.get("menu_load_state_submit_helper_field_store_mapped"):
    static_score += 40
if summary.get("menu_load_state_submit_helper_generic_fan_in_mapped"):
    static_score += 40
if summary.get("menu_load_state_submit_sources_exact_subset_mapped"):
    static_score += 40
if summary.get("menu_load_state_helper_family_bodies_mapped"):
    static_score += 40
if summary.get("menu_load_state_helper_family_ref_counts_mapped"):
    static_score += 40
if summary.get("menu_load_state_helper_family_timed_queue_consumer_mapped"):
    static_score += 40
if summary.get("menu_load_state_terminal_compare_consumers_mapped"):
    static_score += 40
if summary.get("menu_load_state_empty_check_consumer_mapped"):
    static_score += 40
if summary.get("menu_load_state_pending_success_failure_semantics_mapped"):
    static_score += 40
if summary.get("menu_load_state_consumer_vtables_mapped"):
    static_score += 40
if summary.get("menu_load_state_consumer_roles_classified"):
    static_score += 40
if summary.get("menu_load_state_consumer_constructor_callers_mapped"):
    static_score += 40
if summary.get("pump_wrapper_calls_delta_pump") and summary.get("pump_wrapper_calls_default_pump"):
    static_score += 40
if summary.get("pump_wrapper_has_vtable_entry"):
    static_score += 40
if summary.get("move_map_scheduler_calls_dispatch"):
    static_score += 40
if summary.get("dispatcher_calls_requested_slot_validation"):
    static_score += 40
if summary.get("dispatcher_calls_all_queue_helpers"):
    static_score += 40
if summary.get("b72_accessor_reads_b72_and_bc4"):
    static_score += 40
if summary.get("b73_accessor_reads_b73_and_bc4"):
    static_score += 40
if summary.get("b78_accessor_reads_requested_slot_index"):
    static_score += 40
if summary.get("b75_accessor_reads_load_arg"):
    static_score += 40
if summary.get("save_load_pumps_touch_transition_fields"):
    static_score += 40
if summary.get("title_menu_continue_calls_request_save"):
    static_score += 40
if summary.get("move_map_continue_calls_request_save_and_profile"):
    static_score += 40
if summary.get("menu_set_slot_wrapper_calls_set_save_slot"):
    static_score += 40
if summary.get("gate_function_sets_b72_b73_bc4"):
    static_score += 40
if summary.get("gate_function_has_known_callers"):
    static_score += 40
if summary.get("bc4_value_accessor_called_from_move_map"):
    static_score += 40
if summary.get("bc4_is_three_accessor_has_task_callers"):
    static_score += 40
if summary.get("set_bc4_called_from_move_map"):
    static_score += 40
if summary.get("promote_bc4_after_save_load_pump"):
    static_score += 40
if summary.get("bc4_windows_touch_bc4"):
    static_score += 40
if summary.get("post_pump_switch_table_has_ten_cases"):
    static_score += 40
if summary.get("post_pump_return_zero_promotes_bc4"):
    static_score += 40
if summary.get("post_pump_return_one_marks_completion"):
    static_score += 40
if summary.get("post_pump_returns_three_seven_nine_promote_bc4"):
    static_score += 40
if summary.get("switch_case0_calls_bc4_promoter"):
    static_score += 40
if summary.get("switch_case1_checks_task_and_sets_completion_flag"):
    static_score += 40
if summary.get("switch_case2_calls_notify_and_event_reset"):
    static_score += 40
if summary.get("switch_case8_loads_context_global"):
    static_score += 40
if summary.get("pump_owner_vtable_constructor_sets_base"):
    static_score += 40
if summary.get("pump_owner_vtable_update_entry_is_82a0f0"):
    static_score += 40
if summary.get("pump_owner_vtable_name_entry_is_82c170"):
    static_score += 40
if summary.get("vtable_family_has_all_related_bases"):
    static_score += 40
if summary.get("pump_owner_update_reads_task_plus8_float"):
    static_score += 40
if summary.get("pump_owner_update_branches_on_task_plus8"):
    static_score += 40
if summary.get("pump_owner_clone_preserves_task_plus8_float"):
    static_score += 40
if summary.get("pump_owner_task_plus8_controls_pump_choice"):
    static_score += 40
if summary.get("pump_owner_local_builder_writes_vtable"):
    static_score += 40
if summary.get("pump_owner_local_builder_stores_xmm2_to_plus8"):
    static_score += 40
if summary.get("pump_owner_local_builder_calls_task_wrapper"):
    static_score += 40
if summary.get("pump_owner_local_builder_calls_task_enqueue"):
    static_score += 40
if summary.get("pump_owner_builder_has_four_callers"):
    static_score += 40
if summary.get("builder_positive_delay_call_uses_300s"):
    static_score += 40
if summary.get("builder_menu_wrappers_cover_continue_new_other"):
    static_score += 40
if summary.get("builder_continue_new_other_use_zero_xmm2"):
    static_score += 40
if summary.get("builder_continue_new_other_call_builder"):
    static_score += 40
if summary.get("zero_xmm2_menu_wrappers_select_default_pump"):
    static_score += 40
if summary.get("positive_xmm2_wrapper_selects_delta_pump"):
    static_score += 40
if summary.get("builder_callers_have_expected_function_ranges"):
    static_score += 40
if summary.get("menu_wrappers_have_expected_function_ranges"):
    static_score += 40
if summary.get("pump_owner_builder_range_contains_local_builder"):
    static_score += 40
if summary.get("builder_rcx_flows_from_caller_rdx"):
    static_score += 40
if summary.get("builder_rdx_is_stack_task_descriptor"):
    static_score += 40
if summary.get("zero_xmm2_callers_share_scheduler_argument_flow"):
    static_score += 40
if summary.get("zero_xmm2_wrapper_constructors_have_callers"):
    static_score += 40
if summary.get("zero_xmm2_wrapper_constructor_callers_have_pdata"):
    static_score += 40
if summary.get("positive_and_zero_wrapper_constructor_refs_mapped"):
    static_score += 40
if summary.get("zero_xmm2_thunks_have_callers"):
    static_score += 40
if summary.get("zero_xmm2_thunk_callers_have_pdata"):
    static_score += 40
if summary.get("positive_and_zero_thunk_refs_mapped"):
    static_score += 40
if summary.get("zero_xmm2_outer_thunks_have_callers"):
    static_score += 40
if summary.get("zero_xmm2_outer_thunk_callers_have_pdata"):
    static_score += 40
if summary.get("positive_and_zero_outer_thunk_refs_mapped"):
    static_score += 40
if summary.get("zero_xmm2_entry_thunks_have_callers"):
    static_score += 40
if summary.get("zero_xmm2_entry_thunk_callers_have_pdata"):
    static_score += 40
if summary.get("entry_thunk_callers_supply_task_plus8_as_r8"):
    static_score += 40
if summary.get("positive_and_zero_entry_thunk_refs_mapped"):
    static_score += 40
if summary.get("entry_helpers_have_absolute_vtable_refs"):
    static_score += 40
if summary.get("zero_xmm2_entry_helpers_have_vtable_refs"):
    static_score += 40
if summary.get("entry_helper_refs_are_rdata_vtable_slots"):
    static_score += 40
if summary.get("continue_entry_helper_vtable_slot_is_142ac7888"):
    static_score += 40
if summary.get("other_and_new_entry_helpers_share_neighbor_vtables"):
    static_score += 40
if summary.get("entry_helper_vtable_layout_is_base_plus_0x10"):
    static_score += 40
if summary.get("entry_helper_vtables_have_task_plus8_accessors"):
    static_score += 40
if summary.get("entry_helper_constructors_store_vtable_base"):
    static_score += 40
if summary.get("entry_helper_copy_or_clone_entries_store_vtable_base"):
    static_score += 40
if summary.get("continue_vtable_roles_decoded"):
    static_score += 40
if summary.get("entry_vtable_bases_have_three_rip_refs"):
    static_score += 40
if summary.get("continue_vtable_refs_are_ctor_dtor_clone"):
    static_score += 40
if summary.get("other_vtable_refs_are_ctor_dtor_clone"):
    static_score += 40
if summary.get("entry_task_ctor_copy_have_no_direct_rel32_callers"):
    static_score += 40
if summary.get("pump_owner_local_builder_calls_task_enqueue_link"):
    static_score += 40
if summary.get("menu_region_task_local_wrapper_contexts_mapped"):
    static_score += 40
if summary.get("menu_region_task_local_wrapper_contexts_have_enqueue"):
    static_score += 40
if summary.get("menu_region_task_local_wrapper_contexts_have_vtable_leas"):
    static_score += 40
if summary.get("pump_owner_local_wrapper_sequence_has_link_enqueue"):
    static_score += 40
if summary.get("task_local_wrapper_sequences_grouped_by_function"):
    static_score += 40
if summary.get("pump_owner_sequence_descriptor_and_enqueue_decoded"):
    static_score += 40
if summary.get("entry_family_8279e0_sequence_descriptor_and_enqueue_decoded"):
    static_score += 40
if summary.get("entry_family_and_pump_owner_share_enqueue_shape"):
    static_score += 40
if summary.get("entry_family_builder_has_single_callsite"):
    static_score += 40
if summary.get("entry_family_builder_call_uses_300s_and_ba00_wrapper"):
    static_score += 40
if summary.get("entry_family_descriptor_vtable_has_four_refs"):
    static_score += 40
if summary.get("entry_family_descriptor_refs_include_builder_lifecycle"):
    static_score += 40
if summary.get("entry_selector_has_two_known_callers"):
    static_score += 40
if summary.get("entry_selector_callers_cover_selector_7_and_1"):
    static_score += 40
if summary.get("entry_selector_writes_selector_to_descriptor"):
    static_score += 40
if summary.get("entry_selector_uses_two_alloc_paths_and_enqueue"):
    static_score += 40
if summary.get("entry_selector_parent_thunks_have_single_callers"):
    static_score += 40
if summary.get("entry_selector_parent_thunks_forward_r8_plus_0x50"):
    static_score += 40
if summary.get("selector6_parent_thunk_adjacent_to_continue_zero_thunk"):
    static_score += 40
if summary.get("entry_selector_entry_functions_are_thin_selector_wrappers"):
    static_score += 40
if summary.get("selector6_entry_has_no_direct_descriptor_vtable_refs"):
    static_score += 40
if summary.get("selector6_entry_immediately_precedes_continue_builder"):
    static_score += 40
if summary.get("selector_parent_thunks_have_single_outer_callers"):
    static_score += 40
if summary.get("selector6_parent_outer_context_is_8237d0"):
    static_score += 40
if summary.get("selector_outer_thunks_have_single_entry_callers"):
    static_score += 40
if summary.get("selector6_outer_entry_context_is_822af0"):
    static_score += 40
if summary.get("selector6_outer_and_continue_outer_are_adjacent"):
    static_score += 40
if summary.get("selector6_entry_level_has_single_helper_caller"):
    static_score += 40
if summary.get("continue_entry_level_has_single_continue_helper_caller"):
    static_score += 40
if summary.get("selector6_entry_helper_supplies_task_plus8_and_selector_arg"):
    static_score += 40
if summary.get("continue_entry_helper_supplies_task_plus8_without_selector_arg"):
    static_score += 40
if summary.get("selector6_entry_helper_has_distinct_rdata_vtable_slot"):
    static_score += 40
if summary.get("selector6_helper_vtable_distinct_from_continue_vtable"):
    static_score += 40
if summary.get("selector6_vtable_roles_decoded"):
    static_score += 40
if summary.get("selector6_vtable_refs_are_lifecycle_plus_builder"):
    static_score += 40
if summary.get("selector6_vtable_extra_ref_is_builder_context"):
    static_score += 40
if summary.get("selector6_and_continue_vtable_roles_share_shape"):
    static_score += 40
if summary.get("selector6_builder_context_range_mapped"):
    static_score += 40
if summary.get("selector6_builder_uses_incoming_scheduler_args"):
    static_score += 40
if summary.get("selector6_builder_constructs_six_descriptor_vtables"):
    static_score += 40
if summary.get("selector6_builder_calls_local_wrappers_for_five_tags"):
    static_score += 40
if summary.get("selector6_builder_chain_appends_six_descriptors"):
    static_score += 40
if summary.get("selector6_builder_appends_selector6_descriptor_last"):
    static_score += 40
if summary.get("selector6_builder_submits_chain_to_incoming_rcx_owner"):
    static_score += 40
if summary.get("selector6_builder_distinct_from_continue_builder_path"):
    static_score += 40
if summary.get("selector6_builder_context_has_single_direct_caller"):
    static_score += 40
if summary.get("selector6_builder_direct_caller_range_mapped"):
    static_score += 40
if summary.get("selector6_builder_direct_caller_has_global_fast_path_gate"):
    static_score += 40
if summary.get("selector6_builder_direct_fast_path_arg_shuffle_mapped"):
    static_score += 40
if summary.get("selector6_builder_direct_fallback_reconstructs_inputs"):
    static_score += 40
if summary.get("selector6_builder_direct_fallback_composes_and_enqueues"):
    static_score += 40
if summary.get("selector6_builder_parent_wrapper_arg_swap_mapped"):
    static_score += 40
if summary.get("selector6_builder_outer_entry_chain_mapped"):
    static_score += 40
if summary.get("selector6_builder_entry_to_builder_arg_flow_mapped"):
    static_score += 40
if summary.get("selector6_builder_entry_thunk_has_single_helper_caller"):
    static_score += 40
if summary.get("selector6_builder_entry_helper_supplies_task_plus8_and_selector_arg"):
    static_score += 40
if summary.get("selector6_builder_entry_helper_vtable_slot_is_142ac7658"):
    static_score += 40
if summary.get("selector6_builder_entry_vtable_roles_decoded"):
    static_score += 40
if summary.get("selector6_builder_entry_slot0_thunk_target_mapped"):
    static_score += 40
if summary.get("selector6_builder_entry_alloc_clone_stores_vtable"):
    static_score += 40
if summary.get("selector6_builder_entry_alloc_clone_allocates_and_copies_plus8"):
    static_score += 40
if summary.get("selector6_builder_entry_alloc_clone_called_from_slot0_thunk"):
    static_score += 40
if summary.get("selector6_builder_entry_vtable_refs_mapped"):
    static_score += 40
if summary.get("selector6_builder_entry_vtable_refs_include_alloc_and_copy_bodies"):
    static_score += 40
if summary.get("selector6_builder_entry_helper_distinct_from_continue_helper"):
    static_score += 40
if summary.get("selector6_builder_entry_copy_ctor_has_single_wrapper_caller"):
    static_score += 40
if summary.get("selector6_builder_entry_copy_ctor_body_copies_payload"):
    static_score += 40
if summary.get("selector6_builder_entry_copy_wrapper_has_single_owner_init_caller"):
    static_score += 40
if summary.get("selector6_builder_entry_copy_wrapper_allocates_and_installs_owner_38"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_init_clears_then_builds_owner_38"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_init_has_single_compose_caller"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_compose_clones_two_sources"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_compose_enqueues_three_stage_result"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_compose_builds_container_vtable"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_compose_range_mapped"):
    static_score += 40
if summary.get("selector6_builder_entry_owner_compose_has_single_parent_caller"):
    static_score += 40
if summary.get("selector6_owner_compose_has_three_parent_callers"):
    static_score += 40
if summary.get("selector6_owner_compose_parent_callers_have_expected_ranges"):
    static_score += 40
if summary.get("selector6_owner_compose_parent_variants_cover_local_wrapper_and_input_key"):
    static_score += 40
if summary.get("selector6_owner_compose_local_wrapper_variants_use_tags_7120_7130"):
    static_score += 40
if summary.get("selector6_owner_compose_input_variant_uses_guard_and_low_alloc"):
    static_score += 40
if summary.get("selector6_owner_compose_parent_variants_pass_args_to_common_owner"):
    static_score += 40
if summary.get("selector6_owner_compose_parent_dispatch_vtables_mapped"):
    static_score += 40
if summary.get("selector6_owner_compose_parent_callbacks_mapped"):
    static_score += 40
if summary.get("selector6_owner_variant_callers_are_singletons"):
    static_score += 40
if summary.get("selector6_owner_variant_callers_have_expected_ranges"):
    static_score += 40
if summary.get("selector6_owner_input_variant_caller_copies_selector_payload"):
    static_score += 40
if summary.get("selector6_owner_local_variant_828e10_arg_flow_mapped"):
    static_score += 40
if summary.get("selector6_owner_local_variant_828cb0_builds_three_descriptors"):
    static_score += 40
if summary.get("selector6_owner_local_variant_828cb0_chains_enqueue_link"):
    static_score += 40
if summary.get("selector6_owner_local_variant_828cb0_calls_parent_with_rsi_payload"):
    static_score += 40
if summary.get("selector6_owner_variant_callers_are_continue_adjacent"):
    static_score += 40
if summary.get("continue_selector_entry_helpers_share_dispatch_shape"):
    static_score += 40
if summary.get("selector_builder_helper_carries_extra_r8_not_continue"):
    static_score += 40
if summary.get("continue_selector_owner_vtable_slots_resolved"):
    static_score += 40
if summary.get("selector_owner_preflight_has_vtable_slot_ref"):
    static_score += 40
if summary.get("selector_owner_preflight_calls_wrapper_once"):
    static_score += 40
if summary.get("selector_owner_preflight_lazy_initialization_gate_mapped"):
    static_score += 40
if summary.get("selector_owner_preflight_fallback_selector3_mapped"):
    static_score += 40
if summary.get("selector_owner_preflight_success_dispatches_virtual_slot10"):
    static_score += 40
if summary.get("selector_owner_vtable_71c0_entry_roles_resolved"):
    static_score += 40
if summary.get("selector_owner_vtable_71c0_refs_are_ctor_and_dtor"):
    static_score += 40
if summary.get("selector_owner_ctor_initializes_plus60_plus68"):
    static_score += 40
if summary.get("selector_owner_dtor_releases_plus68_and_vector"):
    static_score += 40
if summary.get("selector_owner_delete_wrapper_calls_dtor_and_frees_0x70"):
    static_score += 40
if summary.get("selector_owner_ctor_wrapper_calls_ctor_with_stack_payload"):
    static_score += 40
if summary.get("selector_owner_factory_instantiates_preflight_owner_with_shared_input"):
    static_score += 40
if summary.get("selector_owner_factory_chains_preflight_with_sibling_owner"):
    static_score += 40
if summary.get("selector_owner_factory_thunk_chain_mapped"):
    static_score += 40
if summary.get("selector_owner_factory_outer_caller_arg_flow_mapped"):
    static_score += 40
if summary.get("selector_owner_factory_entry_helper_dispatch_shape_mapped"):
    static_score += 40
if summary.get("selector_owner_factory_entry_helper_has_vtable_slot"):
    static_score += 40
if summary.get("selector_factory_entry_vtable_b5c8_entries_resolved"):
    static_score += 40
if summary.get("selector_factory_entry_vtable_b5c8_lifecycle_refs_mapped"):
    static_score += 40
if summary.get("selector_factory_entry_copy_clone_dtor_store_vtable_b5c8"):
    static_score += 40
if summary.get("selector_owner_factory_path_connects_table_helper_to_preflight_factory"):
    static_score += 40
if summary.get("selector_factory_neighbor_vtable_bcc8_entries_resolved"):
    static_score += 40
if summary.get("selector_factory_neighbor_vtable_bcc8_lifecycle_refs_mapped"):
    static_score += 40
if summary.get("selector_factory_neighbor_helper_dispatch_shape_mapped"):
    static_score += 40
if summary.get("selector_factory_neighbor_helper_has_vtable_slot"):
    static_score += 40
if summary.get("selector_factory_neighbor_outer_chain_mapped"):
    static_score += 40
if summary.get("selector_factory_complex_builder_8394b0_mapped"):
    static_score += 40
if summary.get("selector_factory_complex_builder_calls_submit_834b40"):
    static_score += 40
if summary.get("selector_submit_helper_range_mapped"):
    static_score += 40
if summary.get("selector_submit_helper_has_six_known_callers"):
    static_score += 40
if summary.get("selector_submit_helper_captures_args_and_stack_context"):
    static_score += 40
if summary.get("selector_submit_helper_reads_owner_plus38_and_virtual_builder"):
    static_score += 40
if summary.get("selector_submit_helper_builds_descriptor_vtable_bde0"):
    static_score += 40
if summary.get("selector_submit_helper_calls_builder_clone_final_enqueue"):
    static_score += 40
if summary.get("selector_submit_helper_final_enqueue_preserves_owner_and_descriptor"):
    static_score += 40
if summary.get("selector_submit_helper_cleans_temporaries_and_returns_owner"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_range_mapped"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_has_single_submit_caller"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_captures_args_and_allocates_pair"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_copies_both_sources"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_calls_pair_builder"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_stores_result_and_refs"):
    static_score += 40
if summary.get("selector_final_enqueue_helper_cleans_r8_source_and_returns_output"):
    static_score += 40
if summary.get("selector_final_enqueue_callers_have_expected_ranges"):
    static_score += 40
if summary.get("selector_final_enqueue_selector_submit_callsite_identified"):
    static_score += 40
if summary.get("selector_final_enqueue_entry_family_callsite_identified"):
    static_score += 40
if summary.get("selector_final_enqueue_selector_builder_callsite_identified"):
    static_score += 40
if summary.get("selector_final_enqueue_global_runtime_callsite_identified"):
    static_score += 40
if summary.get("selector_final_enqueue_outside_menu_callsite_identified"):
    static_score += 40
if summary.get("selector_final_enqueue_runtime_trace_filter_subset_identified"):
    static_score += 40
if summary.get("selector_pair_builder_range_mapped"):
    static_score += 40
if summary.get("selector_pair_builder_has_single_final_enqueue_caller"):
    static_score += 40
if summary.get("selector_pair_builder_captures_output_and_sources"):
    static_score += 40
if summary.get("selector_pair_builder_initializes_pair_vtable_and_slots"):
    static_score += 40
if summary.get("selector_pair_builder_clones_both_source_descriptors"):
    static_score += 40
if summary.get("selector_pair_builder_cleans_sources_and_returns_output"):
    static_score += 40
if summary.get("set_save_slot_has_five_known_callers"):
    static_score += 40
if summary.get("set_save_slot_callers_have_expected_ranges"):
    static_score += 40
if summary.get("set_save_slot_menu_wrapper_callsite_mapped"):
    static_score += 40
if summary.get("set_save_slot_runtime_reset_callsite_mapped"):
    static_score += 40
if summary.get("set_save_slot_runtime_reset_has_counter_gate_and_cleanup"):
    static_score += 40
if summary.get("slot_reset_parent_loop_range_mapped"):
    static_score += 40
if summary.get("slot_reset_parent_entry_thunks_mapped"):
    static_score += 40
if summary.get("slot_reset_parent_state_table_dispatch_mapped"):
    static_score += 40
if summary.get("slot_reset_parent_default_label_and_loop_guard_mapped"):
    static_score += 40
if summary.get("slot_reset_handler_range_mapped"):
    static_score += 40
if summary.get("slot_reset_handler_counter_gate_and_minus_one_slot_mapped"):
    static_score += 40
if summary.get("slot_reset_handler_global_notify_and_state_cleanup_mapped"):
    static_score += 40
if summary.get("slot_reset_parent_vtable_refs_mapped"):
    static_score += 40
if summary.get("slot_reset_parent_vtable_slot28_is_loop"):
    static_score += 40
if summary.get("slot_reset_parent_vtable_entry20_variants_mapped"):
    static_score += 40
if summary.get("slot_reset_parent_vtable_dispatch_tail_shared"):
    static_score += 40
if summary.get("slot_reset_state_table_init_range_mapped"):
    static_score += 40
if summary.get("slot_reset_state_table_zeroes_global_0xd0"):
    static_score += 40
if summary.get("slot_reset_state_table_has_12_initialized_pairs"):
    static_score += 40
if summary.get("slot_reset_state_table_handler_index_11_is_finish_reset"):
    static_score += 40
if summary.get("slot_reset_state_table_neighbor_handlers_mapped"):
    static_score += 40
if summary.get("slot_reset_menu_job_wait_range_mapped"):
    static_score += 40
if summary.get("slot_reset_menu_job_wait_builds_owner_tasks"):
    static_score += 40
if summary.get("slot_reset_menu_job_wait_global_toggle_mapped"):
    static_score += 40
if summary.get("slot_reset_global_toggle_helper_store_mapped"):
    static_score += 40
if summary.get("slot_reset_global_toggle_callers_mapped"):
    static_score += 40
if summary.get("slot_reset_global_toggle_callers_pass_true_after_first_submit"):
    static_score += 40
if summary.get("slot_reset_global_toggle_extra_parent_range_mapped"):
    static_score += 40
if summary.get("slot_reset_global_toggle_extra_parent_first_call_can_clear_or_set"):
    static_score += 40
if summary.get("slot_reset_global_toggle_extra_parent_second_call_sets_19_and_true"):
    static_score += 40
if summary.get("slot_reset_global_toggle_extra_parent_clears_19_and_gates_first_toggle"):
    static_score += 40
if summary.get("slot_reset_global_toggle_extra_parent_second_toggle_follows_failed_gate"):
    static_score += 40
if summary.get("slot_reset_global_toggle_extra_parent_callback_chains_mapped"):
    static_score += 40
if summary.get("slot_reset_global_job_context_known_functions_mapped"):
    static_score += 40
if summary.get("slot_reset_global_job_context_parent_writes_plus19"):
    static_score += 40
if summary.get("slot_reset_global_job_context_menujobwait_refs_without_direct_plus19"):
    static_score += 40
if summary.get("slot_reset_global_context_plus19_readers_identified"):
    static_score += 40
if summary.get("slot_reset_global_context_plus19_reader_75811b_gate_mapped"):
    static_score += 40
if summary.get("slot_reset_global_context_plus19_reader_a832a0_list_mapped"):
    static_score += 40
if summary.get("slot_reset_global_context_plus19_gate_range_mapped"):
    static_score += 40
if summary.get("slot_reset_global_context_plus19_gate_callers_mapped"):
    static_score += 40
if summary.get("slot_reset_global_context_plus19_gate_conditions_mapped"):
    static_score += 40
if summary.get("slot_reset_menu_job_wait_sets_finish_state_11"):
    static_score += 40
if summary.get("slot_reset_set_state_helper_range_mapped"):
    static_score += 40
if summary.get("slot_reset_set_state_helper_writes_owner_4c"):
    static_score += 40
if summary.get("slot_reset_set_state_helper_connects_menujobwait_and_finish"):
    static_score += 40
if summary.get("slot_reset_timed_submit_helper_range_mapped"):
    static_score += 40
if summary.get("slot_reset_timed_submit_iterates_existing_tasks"):
    static_score += 40
if summary.get("slot_reset_timed_queue_helper_range_mapped"):
    static_score += 40
if summary.get("slot_reset_timed_queue_consumes_existing_job_before_finish"):
    static_score += 40
if summary.get("slot_reset_timed_submit_empty_descriptor_mapped"):
    static_score += 40
if summary.get("slot_reset_menujobwait_queue_before_finish_gate_mapped"):
    static_score += 40
if summary.get("slot_reset_menujobwait_queue_slots_linked_across_title_and_ending"):
    static_score += 40
if summary.get("slot_reset_finish_gate_global_has_broad_refs"):
    static_score += 40
if summary.get("slot_reset_finish_gate_links_menujobwait_and_finish"):
    static_score += 40
if summary.get("slot_reset_finish_gate_links_save_request_family"):
    static_score += 40
if summary.get("slot_reset_end_flow_wait_range_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_wait_probe_and_reset_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_wait_branch_gate_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_wait_finish_gate_to_state11_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_probe_range_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_probe_owner_c0_semantics_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_reset_clears_gameman_b5e"):
    static_score += 40
if summary.get("game_man_b5e_direct_accessors_mapped"):
    static_score += 40
if summary.get("game_man_b5e_getter_refs_include_move_map_sites"):
    static_score += 40
if summary.get("game_man_b5e_getter_gates_continue_request_path"):
    static_score += 40
if summary.get("game_man_b5e_setter_refs_include_move_map_and_title_reset"):
    static_score += 40
if summary.get("game_man_b5e_setter_clear_paths_mapped"):
    static_score += 40
if summary.get("game_man_b5e_setter_set_path_mapped"):
    static_score += 40
if summary.get("game_man_b5e_bulk_clear_called_from_move_map"):
    static_score += 40
if summary.get("slot_reset_end_flow_tail_branches_range_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_success_gate_selects_tail_branch"):
    static_score += 40
if summary.get("slot_reset_end_flow_e650_sets_state5_and_b5e"):
    static_score += 40
if summary.get("slot_reset_end_flow_e650_restores_selected_value_from_counter"):
    static_score += 40
if summary.get("slot_reset_end_flow_e780_sets_state5_without_b5e"):
    static_score += 40
if summary.get("slot_reset_end_flow_e780_resets_selected_value_and_counter"):
    static_score += 40
if summary.get("slot_reset_end_flow_tail_branches_return_to_playgame"):
    static_score += 40
if summary.get("slot_reset_playgame_handler_range_mapped"):
    static_score += 40
if summary.get("slot_reset_playgame_submits_owner_bc_job"):
    static_score += 40
if summary.get("slot_reset_playgame_consumes_tail_active_flag"):
    static_score += 40
if summary.get("slot_reset_playgame_updates_nonnegative_save_slot_global"):
    static_score += 40
if summary.get("slot_reset_playgame_menu_object_probe_path_mapped"):
    static_score += 40
if summary.get("slot_reset_playgame_tail_increments_state_counter"):
    static_score += 40
if summary.get("slot_reset_playgame_submit_helper_range_mapped"):
    static_score += 40
if summary.get("slot_reset_playgame_submit_args_mapped"):
    static_score += 40
if summary.get("slot_reset_playgame_submit_rejects_minus_one_slot"):
    static_score += 40
if summary.get("slot_reset_playgame_submit_stores_validated_load_pair"):
    static_score += 40
if summary.get("slot_reset_playgame_submit_appends_payload_vector"):
    static_score += 40
if summary.get("slot_reset_selected_value_get_set_mapped"):
    static_score += 40
if summary.get("slot_reset_selected_value_validate_flags_mapped"):
    static_score += 40
if summary.get("slot_reset_selected_value_pair_normalization_mapped"):
    static_score += 40
if summary.get("slot_reset_playgame_owner_bc_to_gameman14_flow_mapped"):
    static_score += 40
if summary.get("slot_reset_selected_value_direct_field_accesses_mapped"):
    static_score += 40
if summary.get("slot_reset_selected_value_validate_ready_flag_lifecycle_mapped"):
    static_score += 40
if summary.get("slot_reset_selected_value_getter_callers_mapped"):
    static_score += 40
if summary.get("slot_reset_selected_value_ac0_callers_store_or_gate_global_slot"):
    static_score += 40
if summary.get("slot_reset_selected_value_b5f_caller_forces_slot_zero_and_b5e"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_refs_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_state_table_init_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_state_table_entries_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_state_table_labels_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_state_table_links_builder_and_countdown"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_wait_handler_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_menujobwait_handler_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_finish_handler_mapped"):
    static_score += 40
if summary.get("finish_gate_synchronization_refs_mapped"):
    static_score += 40
if summary.get("finish_gate_links_ending_and_title_stage_machines"):
    static_score += 40
if summary.get("finish_gate_links_save_request_and_movemap_dispatch"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_countdown_clear_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_countdown_done_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_tiny_wrappers_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_table_entries_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_action_slot_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_builder_range_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_builder_installs_set_clear_tables"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_builder_enqueues_three_descriptors"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_builder_resource_ids_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_builder_chain_helpers_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_submit_helper_mapped"):
    static_score += 40
if summary.get("slot_reset_end_flow_branch_gate_stage_helper_mapped"):
    static_score += 40
metrics["static_evidence_score"] = static_score

# Gated tracing is present but not always active. Runtime trace artifacts raise this score.
if "install_continue_trace_hooks" in src:
    metrics["trace_invasiveness_score"] = 5

# Complexity delta is advisory only: changed lines relative to HEAD.
try:
    diff = subprocess.check_output(["git", "diff", "--numstat"], cwd=repo, text=True, stderr=subprocess.DEVNULL)
    total = 0
    for line in diff.splitlines():
        parts = line.split("\t")
        if len(parts) >= 2 and parts[0].isdigit() and parts[1].isdigit():
            total += int(parts[0]) + int(parts[1])
    metrics["code_complexity_delta"] = total
except Exception:
    pass

# Latest structured runtime evidence, if any. The default measurement does not launch ER.
artifact_dir = None
telemetry_path = None
runtime_driver_rc = None
candidates = []
preferred_artifact_path = repo / ".auto/current-evidence-artifact"
if os.environ.get("AUTO_MEASURE_INNER") != "1" and preferred_artifact_path.exists():
    preferred_artifact = Path(preferred_artifact_path.read_text(encoding="utf-8", errors="replace").strip())
    for name in ["final-telemetry.json", "telemetry.json"]:
        candidate = preferred_artifact / name
        if candidate.exists():
            candidates.append(candidate)
elif os.environ.get("AUTO_INCLUDE_RUNTIME_EVIDENCE") == "1":
    for pattern in ["target/smoke/**/final-telemetry.json", "target/smoke/**/telemetry.json"]:
        candidates.extend(repo.glob(pattern))
if candidates:
    telemetry_path = max(candidates, key=lambda path: path.stat().st_mtime)
    artifact_dir = telemetry_path.parent
    try:
        telemetry = json.loads(telemetry_path.read_text(encoding="utf-8", errors="replace"))
    except Exception:
        telemetry = {}
else:
    telemetry = {}

if artifact_dir and artifact_dir.exists():
    metrics["artifact_bytes"] = sum(path.stat().st_size for path in artifact_dir.rglob("*") if path.is_file())
    logs = []
    for pattern in ["*.log", "*.txt", "*.out"]:
        for path in artifact_dir.glob(pattern):
            try:
                logs.append(path.read_text(encoding="utf-8", errors="replace"))
            except Exception:
                pass
    joined_logs = "\n".join(logs)
    if re.search(r"--allow-pointer-input|ALLOW_POINTER_INPUT=1|ydotool mousemove|click_center_ok", joined_logs):
        metrics["host_pointer_input_used"] = 1
    if re.search(r"save corruption|unquarantined save|destructive save", joined_logs, re.I):
        metrics["save_safety_ok"] = 0
    if re.search(r"crash|exception|fatal", joined_logs, re.I):
        metrics["crash_detected"] = 1
    if "continue-trace" in joined_logs or "ENTER menu_continue_wrapper" in joined_logs:
        metrics["trace_invasiveness_score"] = max(metrics["trace_invasiveness_score"], 20)
    if re.search(r"LEAVE map_load_67bc10 ret=1", joined_logs):
        metrics["title_bootstrap_seen"] = 1
    menu_condition_score = 0
    input_reason_known = False
    known_condition_patterns = [
        (r"input_reason(?:\[[^\]]+\]|=|:)\s*(unsafe[-_ ]shutdown|seamless[-_ ]coop[-_ ]notice|online[-_ ]warning|title[-_ ]accept|continue[-_ ]menu)", 40),
        (r"menu_condition\[(unsafe[-_ ]shutdown|seamless[-_ ]coop[-_ ]notice|online[-_ ]warning|title[-_ ]accept|continue[-_ ]menu)\]", 40),
        (r"unsafe[-_ ]shutdown|did not (?:shut down|quit)|quit game properly", 40),
        (r"seamless co-?op|free mod|welcome dialog", 30),
        (r"online[-_ ]play warning|online play warning", 30),
        (r"title[-_ ]accept|press any button", 20),
        (r"continue[-_ ]selected|continue menu selected", 20),
    ]
    unknown_condition_patterns = [
        (r"unknown_confirmable_modal", 30),
        (r"menu_semaphore", 25),
        (r"confirm_probe", 20),
        (r"barrier_id=hook_0x[0-9a-f]+/table_", 20),
        (r"ENTER menu_other_load_wrapper|LEAVE menu_other_load_wrapper", 20),
        (r"LEAVE map_load_67bc10 ret=1", 20),
    ]
    for pattern, value in known_condition_patterns:
        if re.search(pattern, joined_logs, re.I):
            menu_condition_score += value
            input_reason_known = True
    for pattern, value in unknown_condition_patterns:
        if re.search(pattern, joined_logs, re.I):
            menu_condition_score += value
    if menu_condition_score:
        metrics["menu_condition_evidence_score"] = min(200, menu_condition_score)
    if input_reason_known:
        metrics["input_reason_known"] = 1
    if re.search(r"state[-_ ]gated input|menu[-_ ]aware input|input_gate(?:\[[^\]]+\]|=|:)", joined_logs, re.I):
        metrics["state_gated_input"] = 1
    runtime_metrics_path = artifact_dir / "runtime-metrics.json"
    if runtime_metrics_path.exists():
        try:
            runtime_metrics = json.loads(runtime_metrics_path.read_text(encoding="utf-8", errors="replace"))
            driver_rc_value = runtime_metrics.get("driver_rc")
            if isinstance(driver_rc_value, (int, float)):
                runtime_driver_rc = int(driver_rc_value)
            for key in ["runtime_probe_seconds", "time_to_player_seconds", "host_pointer_input_used", "menu_condition_evidence_score"]:
                value = runtime_metrics.get(key)
                if isinstance(value, (int, float)):
                    metrics[key] = value
            for key in ["input_reason_known", "state_gated_input"]:
                value = runtime_metrics.get(key)
                if isinstance(value, bool):
                    metrics[key] = 1 if value else 0
                elif isinstance(value, (int, float)):
                    metrics[key] = 1 if value else 0
            if runtime_metrics.get("er_process_teardown_ok") in (0, False):
                metrics["er_process_teardown_ok"] = 0
            if runtime_metrics.get("save_safety_ok") in (0, False):
                metrics["save_safety_ok"] = 0
        except Exception:
            metrics["crash_detected"] = 1
    queued_load_request = bool(re.search(r"queuing (?:traced continue flags|b72-only continue profile request)|direct continue sequence requested", joined_logs))
    load_hook_seen = bool(re.search(r"ENTER (current_slot_load_67b570|continue_load_67b750|combined_load_67b940|map_load_67bc10|save_load_state_init_67b030)", joined_logs))
    trace_confirms_state_transition = bool(load_hook_seen and re.search(r"state=(?!0\b)\d+", joined_logs))
else:
    joined_logs = ""
    queued_load_request = False
    load_hook_seen = False
    trace_confirms_state_transition = False

if telemetry:
    metrics["player_available"] = 1 if telemetry.get("player_available") is True else 0
    for key, metric_name in [
        ("game_save_state", "game_save_state"),
        ("game_save_slot", "game_save_slot"),
        ("game_requested_save_slot_load_index", "game_requested_save_slot_load_index"),
    ]:
        value = telemetry.get(key)
        if isinstance(value, bool):
            metrics[metric_name] = int(value)
        elif isinstance(value, (int, float)):
            metrics[metric_name] = value
    metrics["game_save_requested"] = 1 if telemetry.get("game_save_requested") is True else 0
    slot = telemetry.get("autoload_slot")
    game_slot = telemetry.get("game_save_slot")
    if isinstance(slot, (int, float)) and isinstance(game_slot, (int, float)) and int(slot) == int(game_slot):
        metrics["selected_slot_loaded"] = 1 if metrics["player_available"] else 0
    safe_input_pulses = telemetry.get("safe_input_pulses_sent")
    if isinstance(safe_input_pulses, (int, float)):
        simulated_confirms = max(0, int(safe_input_pulses))
        metrics["simulated_confirm_presses"] = simulated_confirms
        metrics["simulated_button_presses_total"] = simulated_confirms
    for key in ["menu_condition_evidence_score"]:
        value = telemetry.get(key)
        if isinstance(value, (int, float)):
            metrics[key] = max(metrics[key], value)
    for key in ["input_reason_known", "state_gated_input"]:
        value = telemetry.get(key)
        if isinstance(value, bool):
            metrics[key] = max(metrics[key], 1 if value else 0)
        elif isinstance(value, (int, float)):
            metrics[key] = max(metrics[key], 1 if value else 0)
    if metrics["player_available"] and metrics["selected_slot_loaded"]:
        metrics["autoload_success"] = 1
    status = str(telemetry.get("autoload_last_status") or "")
    if "direct continue sequence requested" in status:
        queued_load_request = True

if runtime_driver_rc not in (None, 0) and metrics["autoload_success"]:
    metrics["false_positives"] = 1
    metrics["autoload_success"] = 0
    metrics["selected_slot_loaded"] = 0

if queued_load_request and load_hook_seen and (trace_confirms_state_transition or metrics["game_save_state"] not in (-1, 0)):
    metrics["native_request_consumed"] = 1

hard_zero = bool(
    gate_failed
    or not metrics["test_pass"]
    or not metrics["er_process_teardown_ok"]
    or metrics["host_pointer_input_used"]
    or not metrics["save_safety_ok"]
    or metrics["crash_detected"]
)

if hard_zero:
    score = 0
elif metrics["autoload_success"] and metrics["player_available"] and metrics["selected_slot_loaded"]:
    simulated_buttons = int(metrics["simulated_button_presses_total"])
    if simulated_buttons == 0:
        score = 1000
    else:
        explanation_bonus = 0
        if metrics["input_reason_known"]:
            explanation_bonus += 40
        if metrics["state_gated_input"]:
            explanation_bonus += 20
        explanation_bonus += min(20, int(metrics["menu_condition_evidence_score"]) // 10)
        metrics["input_explanation_bonus"] = explanation_bonus
        score = min(980, max(850, 900 - simulated_buttons) + explanation_bonus)
elif metrics["native_request_consumed"]:
    score = 800
elif trace_confirms_state_transition and static_score >= 400:
    score = 600
elif static_score >= 400:
    score = 400
elif metrics["test_pass"]:
    score = 200
else:
    score = 0

metrics["north_star_score"] = score
print(f"METRIC north_star_score={metrics['north_star_score']}")
for key in [
    "autoload_success",
    "player_available",
    "selected_slot_loaded",
    "time_to_player_seconds",
    "game_save_state",
    "game_save_slot",
    "game_requested_save_slot_load_index",
    "game_save_requested",
    "title_bootstrap_seen",
    "native_request_consumed",
    "crash_detected",
    "save_safety_ok",
    "er_process_teardown_ok",
    "host_pointer_input_used",
    "simulated_button_presses_total",
    "simulated_confirm_presses",
    "simulated_cancel_presses",
    "simulated_start_presses",
    "simulated_dpad_up_presses",
    "simulated_dpad_down_presses",
    "simulated_dpad_left_presses",
    "simulated_dpad_right_presses",
    "simulated_left_bumper_presses",
    "simulated_right_bumper_presses",
    "menu_condition_evidence_score",
    "input_reason_known",
    "state_gated_input",
    "input_explanation_bonus",
    "trace_invasiveness_score",
    "static_evidence_score",
    "runtime_probe_seconds",
    "build_seconds",
    "test_pass",
    "code_complexity_delta",
    "artifact_bytes",
    "false_positives",
]:
    value = metrics[key]
    if isinstance(value, float):
        print(f"METRIC {key}={value:.3f}")
    else:
        print(f"METRIC {key}={value}")

if telemetry_path:
    print(f"ASI telemetry_path={telemetry_path}")
if artifact_dir:
    print(f"ASI artifact_dir={artifact_dir}")
if static_re_evidence_path.exists():
    print(f"ASI static_re_evidence_path={static_re_evidence_path}")
PY

if [[ "$GATE_FAILED" != "0" ]]; then
  exit 1
fi
