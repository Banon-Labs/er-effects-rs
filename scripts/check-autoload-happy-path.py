#!/usr/bin/env python3
"""Fail-closed checks for the supported zero-input autoload release path."""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
EXPERIMENTS = REPO_ROOT / "src" / "experiments.rs"
LIB = REPO_ROOT / "src" / "lib.rs"
TELEMETRY = REPO_ROOT / "src" / "telemetry.rs"
WATCHER = REPO_ROOT / "scripts" / "er-readiness-watch.py"
STAGE_SCRIPT = REPO_ROOT / "scripts" / "stage-autoload-release.sh"
NATIVE_STATIC_CHECK = REPO_ROOT / "scripts" / "check-native-continue-static.py"
CHECK_SH = REPO_ROOT / "scripts" / "check.sh"
RUNTIME_PROBE = REPO_ROOT / ".auto" / "runtime_probe.sh"
DIRECT_PROBE = REPO_ROOT / "scripts" / "run-product-continue-direct-probe.sh"
MEASURE = REPO_ROOT / ".auto" / "measure.sh"

REQUIRED_PRODUCT_GATES = {
    "own_stepper_enabled",
    "splash_skip_enabled",
    "native_fullread_commit_enabled",
    "cleanup_title_dialog_after_world_enabled",
}


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def rust_fn_body(source: str, name: str) -> str:
    marker = f"fn {name}("
    start = source.find(marker)
    if start < 0:
        raise AssertionError(f"missing function {name}")
    brace = source.find("{", start)
    if brace < 0:
        raise AssertionError(f"missing function body for {name}")
    depth = 0
    for index in range(brace, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[brace + 1 : index]
    raise AssertionError(f"unterminated function body for {name}")


def require(condition: bool, message: str, failures: list[str]) -> None:
    if not condition:
        failures.append(message)


READINESS_HELPERS = {
    "product_core_autoload_ready",
    "product_continue_action_ready",
    "title_boot_ready",
    "title_menu_action_ready",
    "title_live_dialog_fire_ready",
    "startup_modal_blocking_state",
    "profile_load_dialog_ready",
}

FORBIDDEN_FIXED_WAIT_TOKENS = {
    "OWN_STEPPER_SETTLE_CALLS",
    "NATIVE_LOAD_SETTLE_FRAMES",
    "OWN_STEPPER_MODAL_GRACE",
    "LIVE_DIALOG_ACTIVATE_SETTLE_WAITS",
}


def semantic_readiness_helpers_present(experiments: str) -> bool:
    return all(re.search(rf"\bfn\s+{re.escape(name)}\b", experiments) for name in READINESS_HELPERS)


def fixed_wait_gates_absent(experiments: str, lib: str) -> bool:
    combined = experiments + "\n" + lib
    return not any(re.search(rf"\b{re.escape(name)}\b", combined) for name in FORBIDDEN_FIXED_WAIT_TOKENS)


def product_path_uses_semantic_readiness(experiments: str) -> bool:
    product_core = rust_fn_body(experiments, "product_core_autoload_tick")
    own_stepper = rust_fn_body(experiments, "own_stepper_idx10")
    live_dialog = rust_fn_body(experiments, "own_stepper_live_dialog_fire")
    native_load = rust_fn_body(experiments, "native_load_tick")
    stage2 = rust_fn_body(experiments, "own_stepper_stage2")
    return (
        "product_core_autoload_ready" in product_core
        and "own_stepper_stage2" in product_core
        and "product_continue_action_ready" in product_core
        and "product_continue_autoload_tick" in product_core
        and "CONTINUE_LOAD_RVA" in experiments
        and "cold_char_mount_drive" in stage2
        and "title_boot_ready" in own_stepper
        and "startup_modal_blocking_state" in own_stepper
        and "title_live_dialog_fire_ready" in live_dialog
        and "title_menu_action_ready" in native_load
        and "profile_load_dialog_ready" in stage2
    )


def main() -> int:
    failures: list[str] = []
    experiments = read(EXPERIMENTS)
    lib = read(LIB)
    stage = read(STAGE_SCRIPT)
    telemetry = read(TELEMETRY)
    watcher = read(WATCHER)
    runtime_probe = read(RUNTIME_PROBE) if RUNTIME_PROBE.exists() else ""
    direct_probe = read(DIRECT_PROBE) if DIRECT_PROBE.exists() else ""
    native_static_check = read(NATIVE_STATIC_CHECK) if NATIVE_STATIC_CHECK.exists() else ""
    check_sh = read(CHECK_SH)
    measure = read(MEASURE)

    require(
        "arm_product_autoload_from_request(&initial_state.autoload);" in lib,
        "DllMain must arm product autoload from the parsed request before startup gates run",
        failures,
    )
    require(
        lib.find("arm_product_autoload_from_request(&initial_state.autoload);")
        < lib.find("let state = Arc::new"),
        "product autoload must be armed before EffectsState is wrapped/shared",
        failures,
    )
    require(
        "product_core_autoload_tick" in lib,
        "game task must route product autoload to the minimal native save-load core",
        failures,
    )
    require(
        "BOOTSTRAP_EVENT_GAME_TASK_WAITING_INSTANCE" in lib
        and "TASK_INSTANCE_WAIT_LOG_INTERVAL" in lib
        and "attempts={wait_attempts}" in lib,
        "game task startup must report bounded CSTaskImp wait progress before recurring registration",
        failures,
    )
    require(
        lib.find("product_core_autoload_tick") < lib.find("own_stepper_patch_once"),
        "product autoload core must run before the idx10/title-front-end stepper patch path",
        failures,
    )
    require(
        lib.find("product_core_autoload_tick") < lib.find("title_accept_tick"),
        "product autoload core must run before legacy title-accept input injection paths",
        failures,
    )

    arm_body = rust_fn_body(experiments, "arm_product_autoload_from_request")
    require("SaveLoadMethod::DirectMenuLoad" in arm_body, "product arm must be limited to direct_menu_load", failures)
    require("request.slot()" in arm_body, "product arm must require an explicit slot", failures)
    require("OWN_STEPPER_SLOT.store(slot" in arm_body, "product arm must propagate the requested slot", failures)
    require("PRODUCT_AUTOLOAD_ARMED.store" in arm_body, "product arm must latch PRODUCT_AUTOLOAD_ARMED", failures)
    require("append_autoload_debug" not in arm_body, "product arm must not perform early debug/file I/O", failures)

    for gate in sorted(REQUIRED_PRODUCT_GATES):
        body = rust_fn_body(experiments, gate)
        require("product_autoload_enabled()" in body, f"{gate} must be enabled by product_autoload_enabled()", failures)
    for legacy_gate in ("live_dialog_enabled", "menu_window_latch_enabled"):
        body = rust_fn_body(experiments, legacy_gate)
        require(
            "product_autoload_enabled()" not in body,
            f"{legacy_gate} must remain opt-in and not be part of the product core path",
            failures,
        )

    require(
        semantic_readiness_helpers_present(experiments),
        "product autoload must define semantic readiness helpers for title boot, native Continue/menu action, modals, and ProfileLoadDialog",
        failures,
    )
    require(
        fixed_wait_gates_absent(experiments, lib),
        "product autoload must not redeclare or use the removed fixed frame/call wait gates",
        failures,
    )
    require(
        product_path_uses_semantic_readiness(experiments),
        "product autoload path must call semantic readiness helpers instead of fixed wait gates",
        failures,
    )
    product_core = rust_fn_body(experiments, "product_core_autoload_tick")
    product_ready = rust_fn_body(experiments, "product_core_autoload_ready")
    require(
        "TitleTopDialog::open_menu writes latch and does not require Loop/TextFadeout state" in product_core
        and "ready.title_in_loop\n            && ready.menu_opened_latch" not in product_core
        and "!title_state.in_loop\n        && !title_state.in_textfadeout" not in product_ready,
        "product open-menu gate must allow validated title dialog + latch-clear and must not require Loop/TextFadeout-only timing",
        failures,
    )
    continue_item_body = rust_fn_body(experiments, "product_continue_item_action")
    require(
        "MENU_ITEM_ACCEPT_IDLE_RVA" in continue_item_body
        and "MENU_ITEM_ACCEPT_NATIVE_RVA" in continue_item_body
        and "constant false idle predicate" in continue_item_body
        and "return None" in continue_item_body,
        "product Continue item validation must reject the constant-false idle accept predicate before native submit",
        failures,
    )
    menu_update_body = rust_fn_body(experiments, "cap_menu_item_update_hook")
    require(
        "captured semantic native Continue item" in menu_update_body
        and "semantic_continue_item" in menu_update_body
        and "MENU_TITLE_CONTINUE_DOCALL_RVA" in menu_update_body
        and "MENU_ITEM_ACCEPT_NATIVE_RVA" in menu_update_body
        and "captured first title item as native Continue" not in menu_update_body,
        "product Continue capture must latch a semantic Continue item, not the first ticked MenuWindowJob",
        failures,
    )
    ctor_body = rust_fn_body(experiments, "menu_window_job_ctor_hook")
    require(
        "MENU-WINDOW-CTOR captured semantic native Continue item" in ctor_body
        and "MENU_WINDOW_JOB_CTOR_RVA" in lib
        and "cap_menu_window_job_ctor_7ac8c0" in experiments
        and "MENU_WINDOW_JOB_CTOR_ORIG" in lib,
        "product Continue capture must observe MenuWindowJob construction before update-time first-item latching",
        failures,
    )
    idle_ctor_body = rust_fn_body(experiments, "menu_window_job_idle_ctor_hook")
    require(
        "MENU_WINDOW_JOB_IDLE_CTOR_RVA" in lib
        and "MENU_ITEM_ACCEPT_IDLE_RVA" in experiments
        and "cap_menu_window_job_idle_ctor_7acf80" in experiments
        and "MENU_WINDOW_JOB_IDLE_CTOR_ORIG" in lib
        and "MENU-WINDOW-IDLE-CTOR observed Continue-looking disabled item" in idle_ctor_body
        and "record_continue_candidate" in idle_ctor_body
        and "trace_first_game_caller_rva" in idle_ctor_body
        and "MENU_CONTINUE_ITEM.store" not in idle_ctor_body
        and "MENU_CONTINUE_ITEM.compare_exchange" not in idle_ctor_body,
        "product diagnostics must passively attribute disabled Continue rows to the 0x1407acf80 idle constructor without promoting them",
        failures,
    )
    member_latch_body = rust_fn_body(experiments, "capture_continue_member_node_candidate")
    require(
        "MENU_CONTINUE_MEMBER_NODE" in lib
        and "TRACE_MENU_CONTINUE_WRAPPER_RVA" in member_latch_body
        and "MEMBERFUNCJOB_VTABLE_RVA" in member_latch_body
        and "MEMBER_FN_18" in member_latch_body
        and "MEMBER_ADJ_20" in member_latch_body
        and "capture_continue_member_node_candidate(base, arg1" in experiments
        and "capture_continue_member_node_candidate(base, result" in experiments,
        "product tracing must passively latch registered TitleTopDialog Continue MenuMemberFuncJob nodes",
        failures,
    )
    require(
        "oracle_continue_task_node" in telemetry
        and "oracle_continue_member_node" in telemetry
        and "MENU_CONTINUE_MEMBER_NODE" in telemetry,
        "telemetry must expose passive Continue task/member semantic latch addresses",
        failures,
    )
    result_event_body = rust_fn_body(experiments, "result_event_handler_hook")
    result_action_body = rust_fn_body(experiments, "result_action_builder_hook")
    native_submit_body = rust_fn_body(experiments, "native_submit_hook")
    require(
        "NATIVE_SUBMIT_ORIG" in lib
        and "RESULT_EVENT_HANDLER_RVA" in lib
        and "RESULT_ACTION_BUILDER_RVA" in lib
        and "RESULT_EVENT_WRAPPER_BUILDER_RVA" in lib
        and "RESULT_EVENT_HANDLER_ORIG" in lib
        and "RESULT_ACTION_BUILDER_ORIG" in lib
        and "RESULT_EVENT_WRAPPER_BUILDER_ORIG" in lib
        and "native_submit_7ac890" in experiments
        and "result_event_handler_746e80" in experiments
        and "result_action_builder_746a00" in experiments
        and "result_event_wrapper_builder_744a60" in experiments
        and "call_result_void1_original" in experiments
        and "call_result_void2_original" in experiments
        and "call_wrapper_builder_original" in experiments
        and "continue_load" not in native_submit_body.lower()
        and "continue_load" not in result_event_body.lower()
        and "continue_load" not in result_action_body.lower(),
        "product tracing must passively hook native submit, result.vtable+0x60, action builder, and wrapper builder without direct load shortcuts",
        failures,
    )
    require(
        "oracle_native_submit_hits" in telemetry
        and "oracle_result_event_handler_hits" in telemetry
        and "oracle_result_action_builder_hits" in telemetry
        and "oracle_result_event_last_raw_qword0" in telemetry
        and "oracle_result_event_last_fd4_code" in telemetry
        and "oracle_result_event_last_fd4_arg" in telemetry
        and "oracle_result_action_last_word0" in telemetry
        and "oracle_result_action_last_word1" in telemetry
        and "oracle_result_action_wrapper_builder_hits" in telemetry
        and "oracle_result_action_last_wrapper_builder_ret" in telemetry
        and "oracle_result_action_last_wrapper_builder_ret_update_rva" in telemetry
        and "oracle_policy_window_backing_flag_ptr" in telemetry
        and "oracle_policy_window_stored_backing_flag_ptr" in telemetry
        and "oracle_policy_window_backing_flag_value" in telemetry
        and "oracle_policy_window_requested_flag_value" in telemetry
        and "oracle_policy_window_caller_rva" in telemetry
        and "write_policy_oracle_snapshot" in telemetry
        and "policy_oracle_snapshot" in telemetry
        and "telemetry_snapshot_reason" in telemetry
        and "oracle_policy_ctor_wrapper_hits" in telemetry
        and "oracle_policy_ctor_wrapper_original_this" in telemetry
        and "oracle_policy_ctor_wrapper_original_vtable" in telemetry
        and "oracle_policy_ctor_wrapper_backing_flag_ptr" in telemetry
        and "oracle_policy_ctor_wrapper_caller_rva" in telemetry
        and "oracle_policy_selector_wrapper_hits" in telemetry
        and "oracle_policy_selector_wrapper_requested_flag" in telemetry
        and "oracle_policy_selector_wrapper_selector_arg" in telemetry
        and "oracle_policy_selector_wrapper_caller_rva" in telemetry
        and "oracle_policy_selector_ctor_hits" in telemetry
        and "oracle_policy_selector_ctor_requested_flag_ptr" in telemetry
        and "oracle_policy_selector_ctor_stored_requested_flag_ptr" in telemetry
        and "oracle_policy_selector_ctor_caller_rva" in telemetry
        and "oracle_policy_status_predicate_hits" in telemetry
        and "oracle_policy_status_predicate_ret" in telemetry
        and "oracle_policy_status_predicate_caller_rva" in telemetry
        and "oracle_policy_flag_setter_hits" in telemetry
        and "oracle_policy_flag_setter_after" in telemetry
        and "oracle_policy_flag_setter_caller_rva" in telemetry
        and "oracle_result_action_insert_hits" in telemetry
        and "oracle_result_action_last_insert_arg1_update_rva" in telemetry
        and "oracle_result_action_last_insert_ret_update_rva" in telemetry
        and "RESULT_ACTION_WRAPPER_BUILDER_HITS" in telemetry
        and "RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA" in telemetry
        and "POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR" in telemetry
        and "POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR" in telemetry
        and "POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE" in telemetry
        and "POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE" in telemetry
        and "POLICY_TOS_TITLE_LAST_CALLER_RVA" in telemetry
        and "POLICY_TOS_TITLE_WRAPPER_HITS" in telemetry
        and "POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS" in telemetry
        and "POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE" in telemetry
        and "POLICY_TOS_TITLE_WRAPPER_LAST_BACKING_FLAG_PTR" in telemetry
        and "POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA" in telemetry
        and "POLICY_TOS_SELECTOR_WRAPPER_HITS" in telemetry
        and "POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG" in telemetry
        and "POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG" in telemetry
        and "POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA" in telemetry
        and "POLICY_TOS_SELECTOR_CTOR_HITS" in telemetry
        and "POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR" in telemetry
        and "POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR" in telemetry
        and "POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA" in telemetry
        and "POLICY_TOS_STATUS_HITS" in telemetry
        and "POLICY_TOS_STATUS_LAST_RET" in telemetry
        and "POLICY_TOS_STATUS_LAST_CALLER_RVA" in telemetry
        and "POLICY_TOS_FLAG_SETTER_HITS" in telemetry
        and "POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA" in telemetry
        and "RESULT_ACTION_INSERT_HITS" in telemetry
        and "RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA" in telemetry
        and "NATIVE_SUBMIT_HITS" in telemetry
        and "RESULT_EVENT_HANDLER_HITS" in telemetry
        and "RESULT_EVENT_LAST_FD4_CODE" in telemetry
        and "RESULT_ACTION_BUILDER_HITS" in telemetry
        and "RESULT_ACTION_LAST_WORD0" in telemetry
        and "native_submit_entered" in watcher
        and "native_result_chain_same_result" in watcher
        and "native_submit_fd4_event_match" in watcher
        and "native_result_chain_ready" in watcher
        and "native_continue_chain_stage" in watcher
        and "telemetry_native_submit_entered" in watcher
        and "telemetry_native_result_chain_same_result" in watcher
        and "telemetry_native_submit_fd4_event_match" in watcher
        and "telemetry_native_result_chain_ready" in watcher
        and "telemetry_result_action_wrapper_built" in watcher
        and "telemetry_result_action_wrapper_has_update_rva" in watcher
        and "telemetry_result_action_inserted" in watcher
        and "telemetry_result_action_insert_has_update_rva" in watcher
        and "telemetry_native_continue_chain_stage" in watcher
        and "result_chain_waiting_wrapper_builder" in watcher
        and "wrapper_builder_without_update_rva" in watcher
        and "wrapper_builder_waiting_action_insert" in watcher
        and "action_insert_without_update_rva" in watcher
        and "action_insert_waiting_continue_load" in watcher,
        "telemetry/watcher oracle must expose passive native submit/result-handler/action-builder/wrapper-builder/action-insert hit counts, wrapper/update-RVA proof, same-result proof, and chain stage",
        failures,
    )
    require(
        "RESULT_EVENT_WRAPPER_INNER_BUILD" in native_static_check
        and "POLICY_TOS_STATUS_PREDICATE" in native_static_check
        and "POLICY_TOS_FLAG_SETTER" in native_static_check
        and "POLICY_TOS_TITLE_CTOR_WRAPPER" in native_static_check
        and "POLICY_TOS_TITLE_CTOR_WRAPPER_VTABLE_SLOT" in native_static_check
        and "POLICY_TOS_TITLE_CTOR_WRAPPER_RTTI_COL" in native_static_check
        and "POLICY_TOS_SELECTOR_RTTI_COL" in native_static_check
        and "POLICY_TOS_SELECTOR_WRAPPER" in native_static_check
        and "POLICY_TOS_SELECTOR_CTOR" in native_static_check
        and "POLICY_TOS_SELECTOR_WRAPPER_VTABLE_SLOT" in native_static_check
        and "POLICY_TOS_TITLE_CTOR_CALLER" in native_static_check
        and "POLICY_TOS_FLAG_SETTER_CALLER" in native_static_check
        and "POLICY_TOS_REQUESTED_FLAG_INIT" in native_static_check
        and "POLICY_TOS_REQUESTED_FLAG_BIND" in native_static_check
        and "POLICY_TOS_REQUESTED_FLAG_COMMIT" in native_static_check
        and "wrapper builder returns the original output wrapper pointer" in native_static_check
        and "result event wrapper builder no longer finalizes payload" in native_static_check
        and "policy ToS status predicate reads fallback pointer at owner+0x29c0" in native_static_check
        and "policy ToS flag setter writes requested value to flag pointer" in native_static_check
        and "policy ToS flag setter caller loads requested flag from owner+0x29c8" in native_static_check
        and "policy ToS ctor wrapper vtable slot no longer points at 0x1409b7380" in native_static_check
        and "policy ToS selector wrapper vtable slot no longer points at 0x1409b7390" in native_static_check
        and "policy ToS ctor wrapper thunk adjusts this pointer by +0x8" in native_static_check
        and "policy ToS selector wrapper thunk adjusts this pointer by +0x8" in native_static_check
        and "CommandSelectDialog/SceneProxy/MenuWindow lambda" in native_static_check
        and "policy ToS selector wrapper passes owner+0x29c8 requested flag pointer" in native_static_check
        and "policy ToS selector wrapper passes owner+0x29d0 selector argument" in native_static_check
        and "policy ToS selector wrapper no longer calls 0x1409b49f0" in native_static_check
        and "policy ToS selector ctor stores selector arg at object+0x1260" in native_static_check
        and "policy ToS selector ctor stores requested flag pointer at object+0x1268" in native_static_check
        and "policy ToS selector ctor matches option id against requested flag value" in native_static_check
        and "policy ToS ctor wrapper preserves record pointer from rcx in rsi" in native_static_check
        and "policy ToS ctor wrapper loads backing flag pointer from record+0x8" in native_static_check
        and "policy ToS constructor stores backing flag pointer at owner+0x29c0" in native_static_check
        and "policy ToS constructor copies backing flag value into owner+0x29c8 requested flag" in native_static_check
        and "policy ToS constructor reads backing flag pointer from stack arg1" in native_static_check
        and "policy ToS ctor caller passes backing flag pointer as stack arg1" in native_static_check
        and "policy ToS constructor initializes requested flag owner+0x29c8 from current flag" in native_static_check
        and "policy ToS requested-flag binder passes pointer to owner+0x29c8" in native_static_check
        and "policy ToS requested-flag commit loads requested flag from owner+0x29c8" in native_static_check,
        "native static checker must pin wrapper-builder ABI, ToS wrapper vtable/thunk/RTTI provenance, selector requested-flag ABI, status predicate/setter/caller/requested-flag ABI, and inner finalize edge",
        failures,
    )

    policy_hook_names = [
        "policy_tos_title_ctor_wrapper_hook",
        "policy_tos_selector_wrapper_hook",
        "policy_tos_selector_ctor_hook",
        "policy_tos_flag_setter_hook",
        "policy_tos_status_predicate_hook",
        "policy_tos_title_ctor_hook",
    ]
    for hook_name in policy_hook_names:
        hook_body = rust_fn_body(experiments, hook_name) or ""
        caller_pos = hook_body.find("let caller_rva = trace_first_game_caller_rva();")
        orig_pos = hook_body.find("_ORIG.load")
        require(
            caller_pos >= 0 and (orig_pos < 0 or caller_pos < orig_pos),
            f"{hook_name} must capture caller RVA at hook entry before original call-through",
            failures,
        )

    require(
        "oracle_continue_phase" in telemetry
        and "oracle_continue_expected_slot" in telemetry
        and "oracle_continue_deser_fired" in telemetry
        and "oracle_continue_confirmed" in telemetry
        and "oracle_continue_mount_c30" in telemetry
        and "oracle_continue_guard_waits" in telemetry,
        "telemetry must expose native Continue product phase/guard state for result-chain interpretation",
        failures,
    )

    online_body = rust_fn_body(experiments, "online_disable_enabled")
    input_body = rust_fn_body(experiments, "block_input_enabled")
    require("own_stepper_enabled()" in online_body, "product autoload must inherit offline mode via own_stepper_enabled()", failures)
    require("own_stepper_enabled()" in input_body, "product autoload must inherit input blocking via own_stepper_enabled()", failures)

    require("dll=er_effects_rs.dll" in stage, "release staging must CHAINLOAD er_effects_rs.dll as the properly-loaded mod", failures)
    require("0=er_effects_rs.dll" not in stage, "release staging must not lazy-load er_effects_rs.dll through LOADORDER", failures)
    require("dllModFolderName=dllMods" in stage, "release staging must use dllMods as LazyLoader folder", failures)
    require("er_skip_splash_screens.dll" not in stage, "release staging must not include stale skip-splash DLLs", failures)
    require("er-effects-autoload.txt.example" in stage, "release staging must include an autoload request example", failures)
    require(
        re.search(r"method=direct_menu_load", stage) is not None,
        "release staging autoload example must use direct_menu_load",
        failures,
    )
    require(
        re.search(r"require_title_bootstrap=false", stage) is not None,
        "release staging autoload example must not require title/front-end bootstrap",
        failures,
    )

    if runtime_probe:
        require(
            "RUNTIME_LAZYLOAD_CHAINLOAD_DLL" in runtime_probe,
            "runtime probe must honor the LazyLoader CHAINLOAD payload mode used by the proven baseline",
            failures,
        )
        require(
            "dll=er_effects_rs.dll" in runtime_probe,
            "runtime probe CHAINLOAD mode must write lazyLoad.ini with er_effects_rs.dll as the chainload DLL",
            failures,
        )
        require(
            '"$GAME_DIR/er_effects_rs.dll"' in runtime_probe,
            "runtime probe CHAINLOAD mode must copy er_effects_rs.dll beside LazyLoader, not only into dllMods",
            failures,
        )
        require(
            'rm -f "$GAME_DIR/dllMods/er_effects_rs.dll"' in runtime_probe,
            "runtime probe CHAINLOAD mode must remove the stale LOADORDER er_effects_rs.dll payload",
            failures,
        )
    require(
        "readiness_gate_failures" in measure,
        "measure must expose readiness_gate_failures as the primary static readiness metric",
        failures,
    )
    require(
        all(name in measure for name in READINESS_HELPERS),
        "measure must check every semantic readiness helper",
        failures,
    )
    require(
        all(name in measure for name in FORBIDDEN_FIXED_WAIT_TOKENS),
        "measure must check every removed fixed wait gate",
        failures,
    )
    require(
        "OwnStepperFrameBudget" in measure,
        "measure must forbid OwnStepperFrameBudget regressions",
        failures,
    )
    require(
        "product_core_autoload_tick still calls broken direct_build path" in measure
        and "product_continue_autoload_tick" in measure
        and "product_continue_action_ready" in measure
        and "CONTINUE_LOAD_RVA" in measure,
        "measure must enforce product autoload uses the native Continue row load path, not direct_build",
        failures,
    )
    telemetry_src = read(REPO_ROOT / "src" / "telemetry.rs")
    require(
        "MSGBOX_LAST_DIALOG" in lib
        and "MSGBOX_TOTAL_BUILDS" in lib
        and "MSGBOX_POSTLOAD_BUILDS" in lib
        and "oracle_msgbox_total_builds" in telemetry_src
        and "oracle_msgbox_any_seen" in telemetry_src
        and "oracle_postload_modal_seen" in telemetry_src
        and "oracle_blocking_modal_present" in telemetry_src,
        "telemetry must expose zero-MessageBoxDialog and blocking-modal oracle evidence",
        failures,
    )
    require(
        "oracle_player_render_ready" in telemetry_src
        and "chr_flags1c5.enable_render" in telemetry_src
        and "load_state.draw_group_enabled" in telemetry_src,
        "telemetry must expose rendered-player readiness from ChrIns render state, not just save identity",
        failures,
    )
    require(
        "SERVER_STATUS_FORMATTER_RVA" in lib
        and "SERVER_STATUS_TOTAL_SEEN" in lib
        and "oracle_server_status_text_id" in telemetry_src
        and "oracle_server_status_any_seen" in telemetry_src,
        "telemetry must expose native server/login status semaphore evidence from GR_System_Message_win64.fmg IDs",
        failures,
    )
    require(
        "seamless_coop_loaded" in telemetry_src
        and "runtime_mode" in telemetry_src
        and "GetModuleHandleA" in telemetry_src
        and "ersc.dll" in telemetry_src,
        "telemetry must expose an ERSC/Seamless runtime-mode semaphore, not infer mode from launch command names",
        failures,
    )
    require(
        "--expected-runtime-mode" in watcher
        and "runtime_mode_mismatch" in watcher
        and "seamless_module_mappings" in watcher
        and "SEAMLESS_MODULE_MARKERS" in watcher
        and "preexisting_runtime_pids" in watcher
        and "row.pid not in preexisting_runtime_pids" in watcher,
        "readiness watcher must fail closed when Seamless/vanilla runtime mode mismatches the experiment precondition and must not select stale runtime PIDs",
        failures,
    )
    require(
        "target_window_capture_diagnostics" in watcher
        and '"target_window_capture"' in watcher
        and '"problems"' in watcher
        and '"candidate_count"' in watcher
        and "target_window_capture_problems(selected, window_class)" in watcher,
        "readiness watcher must report the exact target-window capture safety predicate in readiness-result.json",
        failures,
    )
    require(
        "autoload_progress_summary" in watcher
        and '"autoload_progress"' in watcher
        and '"product_core_ready_blocker"' in watcher
        and '"product_core_autoload_ticks"' in watcher
        and 'product_core_{product_core_blocker}' in watcher
        and '"native_continue_chain_stage"' in watcher
        and '"result_action_insert_hits"' in watcher,
        "readiness watcher must report a compact autoload/native-Continue/product-core progress summary in readiness-result.json",
        failures,
    )
    require(
        "PRODUCT_CORE_AUTOLOAD_TICKS" in experiments
        and "PRODUCT_CORE_READY_BLOCKS" in experiments
        and "PRODUCT_CORE_READY_SUCCESSES" in experiments
        and "PRODUCT_CORE_OWNER_TICKS" in experiments
        and "PRODUCT_CORE_LAST_OWNER" in experiments
        and "PRODUCT_CORE_LAST_TITLE_IN_LOOP" in experiments
        and "PRODUCT_CORE_LAST_MENU_OPENED_LATCH" in experiments
        and "PRODUCT_CORE_LAST_PRESS_START_CONTEXT" in experiments
        and "PRODUCT_CORE_LAST_BLOCKER" in experiments
        and "product_core_ready_blocker_label" in experiments
        and "TITLE_OWNER_SCAN_ATTEMPTS" in experiments
        and "TITLE_OWNER_SCAN_VTABLE_HITS" in experiments
        and "TITLE_OWNER_SCAN_TABLE_REJECTS" in experiments
        and "TITLE_OWNER_SCAN_STATE_REJECTS" in experiments
        and "PRODUCT_CORE_LAST_BLOCKER.store(blocker, Ordering::SeqCst);\n        if tick % OWN_STEPPER_LOG_INTERVAL" in experiments
        and "product_core_owner_ticks" in telemetry_src
        and "product_core_last_owner" in telemetry_src
        and "product_core_last_title_in_loop" in telemetry_src
        and "product_core_last_menu_opened_latch" in telemetry_src
        and "product_core_last_press_start_context" in telemetry_src
        and "MENU_WINDOW_JOB_CTOR_HITS" in experiments
        and "MENU_WINDOW_JOB_CTOR_SEMANTIC_HITS" in experiments
        and "MENU_WINDOW_JOB_IDLE_CTOR_HITS" in experiments
        and "MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_HITS" in experiments
        and "MENU_WINDOW_JOB_IDLE_CTOR_CONTINUE_LAST_CALLER_RVA" in experiments
        and "MENU_CONTINUE_IDLE_INSERT_HITS" in experiments
        and "MENU_CONTINUE_IDLE_INSERT_LAST_CALLER_RVA" in experiments
        and "MENU_CONTINUE_IDLE_INSERT_CALLER_RVA" in experiments
        and "MENU_CONTINUE_IDLE_INSERT_CALLER_START_RVA" in experiments
        and "MENU_CONTINUE_IDLE_INSERT_CALLER_END_RVA" in experiments
        and "callstack_contains_game_rva" in experiments
        and "MENU_ITEM_UPDATE_HITS" in experiments
        and "MENU_ITEM_UPDATE_SEMANTIC_HITS" in experiments
        and "MENU_CONTINUE_CANDIDATE_ITEM" in experiments
        and "MENU_CONTINUE_CANDIDATE_ACCEPT_CHANGES" in experiments
        and "record_continue_candidate" in experiments
        and "oracle_menu_window_ctor_hits" in telemetry_src
        and "oracle_menu_window_idle_ctor_hits" in telemetry_src
        and "oracle_menu_window_idle_ctor_continue_last_caller_rva" in telemetry_src
        and "oracle_menu_continue_idle_insert_hits" in telemetry_src
        and "oracle_menu_continue_idle_insert_last_caller_rva" in telemetry_src
        and "oracle_menu_window_idle_ctor_last_caller_rva" in telemetry_src
        and "oracle_menu_item_update_hits" in telemetry_src
        and "oracle_menu_continue_candidate_item" in telemetry_src
        and "oracle_menu_continue_candidate_accept_changes" in telemetry_src
        and "title_owner_scan_attempts" in telemetry_src
        and "title_owner_scan_vtable_hits" in telemetry_src
        and "title_owner_scan_last_candidate" in telemetry_src
        and "title_owner_scan_attempts" in watcher
        and "product_core_owner_ticks" in watcher
        and "product_core_last_owner" in watcher
        and "product_core_last_title_in_loop" in watcher
        and "product_core_last_menu_opened_latch" in watcher
        and "product_core_last_press_start_context" in watcher
        and "menu_window_ctor_hits" in watcher
        and "menu_window_idle_ctor_hits" in watcher
        and "menu_window_idle_ctor_continue_last_caller_rva" in watcher
        and "menu_continue_idle_insert_hits" in watcher
        and "menu_continue_idle_insert_last_caller_rva" in watcher
        and "menu_window_idle_ctor_last_caller_rva" in watcher
        and "menu_item_update_hits" in watcher
        and "menu_continue_candidate_item" in watcher
        and "menu_continue_candidate_last_accept" in watcher
        and "product_core_ready_blocker" in telemetry_src
        and "product_core_autoload_ticks" in telemetry_src,
        "DLL telemetry must expose product-core autoload tick/readiness blocker and title-owner scan evidence",
        failures,
    )
    require(
        "terminate_runtime_pids" in direct_probe
        and 'comm=$(<"$proc/comm")' in direct_probe
        and '[[ "$comm" == "eldenring.exe"' in direct_probe
        and 'ELDEN RING\\\\Game\\\\eldenring.exe' in direct_probe
        and '[[ "$cmdline" == *"$GAME_DIR/eldenring.exe"* ]]' in direct_probe
        and "kill -9 \"$pid\"" in direct_probe,
        "direct/offline probe wrapper must tear down exact owned Wine/POSIX eldenring.exe runtime PIDs, not only the Proton launcher PID",
        failures,
    )
    require(
        "--fail-on-messagebox-dialog" in watcher
        and "native_messagebox_dialog_detected" in watcher
        and "telemetry_messagebox_dialog_detected" in watcher,
        "readiness watcher must fail closed when telemetry observes any native MessageBoxDialog build",
        failures,
    )
    require(
        "--fail-on-server-status-semaphore" in watcher
        and "native_server_status_semaphore_detected" in watcher
        and "telemetry_server_status_semaphore_detected" in watcher
        and "401120" in watcher
        and "401160" in watcher,
        "readiness watcher must fail closed when native server/login status semaphores appear",
        failures,
    )
    require(
        "--visual-save-data-popup-check" in watcher
        and "--defer-unsafe-visual-capture-until-telemetry" in runtime_probe
        and "defer_unsafe_visual_capture_until_telemetry" in watcher
        and "visual_save_data_popup_detected" in watcher
        and "failed to load save data" in watcher,
        "readiness watcher must expose a visual semaphore for the failed-save-data popup while deferring unsafe screenshot failure until telemetry can arrive",
        failures,
    )
    require(
        "runtime_mode_failures" in measure
        and "seamless_coop_loaded" in measure
        and "runtime_mode_expected" in measure,
        "measure must penalize Seamless-contaminated vanilla runtime proof artifacts",
        failures,
    )
    require(
        "messagebox_dialog_failures" in measure
        and "oracle_msgbox_total_builds" in measure
        and "native_messagebox_dialog_detected" in measure,
        "measure must expose and penalize any native MessageBoxDialog build as a bad product-proof failure",
        failures,
    )
    require(
        "product autoload suppressed MessageBoxDialog builder before UI allocation but counted it as oracle failure" in experiments
        and "MSGBOX_TOTAL_BUILDS.fetch_add" in experiments
        and "MSGBOX_LAST_ARG_RDX.store" in experiments,
        "product-mode MessageBoxDialog suppression must preserve/count builder args so telemetry still fails closed",
        failures,
    )
    require(
        "constant-false idle accept predicate" in measure
        and "MENU_ITEM_ACCEPT_IDLE_RVA" in experiments
        and "MENU_ITEM_ACCEPT_NATIVE_RVA" in experiments,
        "measure must fail closed if product submit can use the constant-false idle accept predicate",
        failures,
    )
    require(
        "first ticked MenuWindowJob" in measure
        and "captured semantic native Continue item" in experiments
        and "semantic_continue_item" in experiments,
        "measure must fail closed if product capture regresses to first-ticked MenuWindowJob latching",
        failures,
    )
    require(
        "constructor hook" in measure
        and "MENU_WINDOW_JOB_CTOR_RVA" in lib
        and "cap_menu_window_job_ctor_7ac8c0" in experiments,
        "measure must fail closed if the product lacks a constructor-time semantic Continue latch",
        failures,
    )
    require(
        "idle MenuWindowJob constructor" in measure
        and "MENU_WINDOW_JOB_IDLE_CTOR_RVA" in lib
        and "cap_menu_window_job_idle_ctor_7acf80" in experiments
        and "oracle_menu_window_idle_ctor_hits" in telemetry_src,
        "measure must fail closed if disabled-row idle constructor provenance is missing",
        failures,
    )
    require(
        "registered TitleTopDialog Continue MenuMemberFuncJob" in measure
        and "MENU_CONTINUE_MEMBER_NODE" in lib
        and "capture_continue_member_node_candidate" in experiments
        and "oracle_continue_member_node" in telemetry_src,
        "measure must fail closed if the product lacks passive Continue MenuMemberFuncJob provenance latching/telemetry",
        failures,
    )
    require(
        "result.vtable+0x60" in measure
        and "result_chain" in measure
        and "native_submit_entered" in measure
        and "native_result_chain_same_result" in measure
        and "native_submit_fd4_event_match" in measure
        and "fd4_submit_event_match" in measure
        and "native_result_chain_ready" in measure
        and "native_continue_chain_stage" in measure
        and "result_action_inserted" in watcher
        and "result_action_insert_has_update_rva" in watcher
        and "action_insert_without_update_rva" in watcher
        and "action_insert_waiting_continue_load" in watcher
        and "oracle_native_submit_last_result" in measure
        and "oracle_native_submit_hits" in measure
        and "oracle_result_event_last_fd4_code" in telemetry_src
        and "oracle_result_action_last_word0" in telemetry_src
        and "oracle_result_action_wrapper_builder_hits" in telemetry_src
        and "oracle_result_action_last_wrapper_builder_ret" in telemetry_src
        and "oracle_result_action_last_wrapper_builder_ret_update_rva" in telemetry_src
        and "oracle_policy_window_backing_flag_ptr" in telemetry_src
        and "oracle_policy_window_stored_backing_flag_ptr" in telemetry_src
        and "oracle_policy_window_backing_flag_value" in telemetry_src
        and "oracle_policy_window_requested_flag_value" in telemetry_src
        and "oracle_policy_window_caller_rva" in telemetry_src
        and "write_policy_oracle_snapshot" in telemetry_src
        and "policy_oracle_snapshot" in telemetry_src
        and "telemetry_snapshot_reason" in telemetry_src
        and "oracle_policy_ctor_wrapper_hits" in telemetry_src
        and "oracle_policy_ctor_wrapper_original_this" in telemetry_src
        and "oracle_policy_ctor_wrapper_original_vtable" in telemetry_src
        and "oracle_policy_ctor_wrapper_backing_flag_ptr" in telemetry_src
        and "oracle_policy_ctor_wrapper_caller_rva" in telemetry_src
        and "oracle_policy_selector_wrapper_hits" in telemetry_src
        and "oracle_policy_selector_wrapper_requested_flag" in telemetry_src
        and "oracle_policy_selector_wrapper_selector_arg" in telemetry_src
        and "oracle_policy_selector_wrapper_caller_rva" in telemetry_src
        and "oracle_policy_selector_ctor_hits" in telemetry_src
        and "oracle_policy_selector_ctor_requested_flag_ptr" in telemetry_src
        and "oracle_policy_selector_ctor_stored_requested_flag_ptr" in telemetry_src
        and "oracle_policy_selector_ctor_caller_rva" in telemetry_src
        and "oracle_policy_status_predicate_hits" in telemetry_src
        and "oracle_policy_status_predicate_ret" in telemetry_src
        and "oracle_policy_status_predicate_caller_rva" in telemetry_src
        and "oracle_policy_flag_setter_hits" in telemetry_src
        and "oracle_policy_flag_setter_after" in telemetry_src
        and "oracle_policy_flag_setter_caller_rva" in telemetry_src
        and "oracle_result_action_insert_hits" in telemetry_src
        and "oracle_result_action_last_insert_arg1_update_rva" in telemetry_src
        and "oracle_result_action_last_insert_ret_update_rva" in telemetry_src
        and "RESULT_ACTION_WRAPPER_BUILDER_HITS" in lib
        and "RESULT_ACTION_LAST_WRAPPER_BUILDER_RET_UPDATE_RVA" in lib
        and "POLICY_TOS_TITLE_LAST_BACKING_FLAG_PTR" in lib
        and "POLICY_TOS_TITLE_LAST_STORED_BACKING_FLAG_PTR" in lib
        and "POLICY_TOS_TITLE_LAST_BACKING_FLAG_VALUE" in lib
        and "POLICY_TOS_TITLE_LAST_REQUESTED_FLAG_VALUE" in lib
        and "POLICY_TOS_TITLE_LAST_CALLER_RVA" in lib
        and "POLICY_TOS_TITLE_CTOR_WRAPPER_RVA" in lib
        and "POLICY_TOS_TITLE_CTOR_WRAPPER_ORIG" in lib
        and "POLICY_TOS_TITLE_WRAPPER_HITS" in lib
        and "POLICY_TOS_TITLE_WRAPPER_THIS_ADJUST" in lib
        and "POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_THIS" in lib
        and "POLICY_TOS_TITLE_WRAPPER_LAST_ORIGINAL_VTABLE" in lib
        and "POLICY_TOS_TITLE_WRAPPER_LAST_CALLER_RVA" in lib
        and "POLICY_TOS_SELECTOR_WRAPPER_RVA" in lib
        and "POLICY_TOS_SELECTOR_WRAPPER_HITS" in lib
        and "POLICY_TOS_SELECTOR_WRAPPER_LAST_REQUESTED_FLAG" in lib
        and "POLICY_TOS_SELECTOR_WRAPPER_LAST_SELECTOR_ARG" in lib
        and "POLICY_TOS_SELECTOR_WRAPPER_LAST_CALLER_RVA" in lib
        and "POLICY_TOS_SELECTOR_CTOR_RVA" in lib
        and "POLICY_TOS_SELECTOR_CTOR_HITS" in lib
        and "POLICY_TOS_SELECTOR_CTOR_LAST_REQUESTED_FLAG_PTR" in lib
        and "POLICY_TOS_SELECTOR_CTOR_LAST_STORED_REQUESTED_FLAG_PTR" in lib
        and "POLICY_TOS_SELECTOR_CTOR_LAST_CALLER_RVA" in lib
        and "POLICY_TOS_STATUS_PREDICATE_RVA" in lib
        and "POLICY_TOS_STATUS_PREDICATE_ORIG" in lib
        and "POLICY_TOS_STATUS_LAST_CALLER_RVA" in lib
        and "POLICY_TOS_FLAG_SETTER_RVA" in lib
        and "POLICY_TOS_FLAG_SETTER_ORIG" in lib
        and "POLICY_TOS_FLAG_SETTER_LAST_CALLER_RVA" in lib
        and "RESULT_ACTION_INSERT_HITS" in lib
        and "RESULT_ACTION_LAST_INSERT_ARG1_UPDATE_RVA" in lib
        and "text_section_bounds" in experiments
        and "update_target_in_text" in experiments
        and "raw_task_node_update_rva" in experiments
        and "shared_pointee" in experiments
        and "PE_TEXT_SECTION_NAME" in experiments
        and "policy_tos_title_ctor_wrapper_hook" in experiments
        and "write_policy_oracle_snapshot(\"tos_title_ctor\")" in experiments
        and "policy_tos_record_fields" in experiments
        and "let caller_rva = trace_first_game_caller_rva();" in experiments
        and "trace_first_game_caller_rva" in experiments
        and "backing_flag_ptr" in experiments
        and "stack_arg0" in experiments
        and "callstack_contains_game_rva" in experiments
        and "oracle_result_action_builder_hits" in measure
        and "NATIVE_SUBMIT_ORIG" in lib
        and "RESULT_EVENT_HANDLER_RVA" in lib
        and "RESULT_ACTION_BUILDER_RVA" in lib
        and "oracle_result_event_handler_hits" in telemetry_src
        and "oracle_continue_phase" in telemetry_src,
        "measure must fail closed if passive result-chain telemetry/proof hooks disappear",
        failures,
    )
    require(
        "MENU_CONTINUE_WRAPPER" in native_static_check
        and "MENU_WINDOW_JOB_CTOR" in native_static_check
        and "MENU_ACCEPT_IDLE" in native_static_check
        and "MENU_ACCEPT_NATIVE" in native_static_check
        and "MENU_SUBMIT" in native_static_check
        and "MENU_MEMBER_FUNC_JOB_RUN" in native_static_check
        and "MENU_REGISTRY_INSERT_COPY" in native_static_check
        and "RESULT_EVENT_HANDLER" in native_static_check
        and "RESULT_EVENT_WRAPPER_BUILDER" in native_static_check
        and "MENU_JOB_LIST_CONSUMER" in native_static_check
        and "MENU_JOB_SINGLE_CONSUMER" in native_static_check
        and "FD4 event code 3" in native_static_check
        and "FD4 event code 2" in native_static_check
        and "downstream action node" in native_static_check
        and "constructed FD4 event pointer" in native_static_check
        and "event+0x0" in native_static_check
        and "event+0x4" in native_static_check
        and "node+0x18" in native_static_check
        and "node+0x20" in native_static_check
        and "node+0x10" in native_static_check
        and "result+0x3b0" in native_static_check
        and "vtable +0x10 update" in native_static_check
        and "update return payload" in native_static_check
        and "check-native-continue-static.py" in check_sh,
        "quality gates must include skip-safe native Continue/MenuWindowJob/MenuMemberFuncJob/result-consumer static byte-window validation",
        failures,
    )
    require(
        "check-native-continue-static.py" in measure
        and "MenuMemberFuncJob" in measure
        and "result-consumer" in measure,
        "measure must fail closed if the native Continue/MenuMemberFuncJob/result-consumer static checker is not wired into quality gates",
        failures,
    )
    require(
        "native_server_status_semaphore_detected" in measure
        and "oracle_server_status_text_id" in measure
        and "server_status" in measure,
        "measure must expose and penalize native server/login status semaphore artifacts",
        failures,
    )
    require(
        "save_data_popup_failures" in measure
        and "visual_save_data_popup_detected" in measure
        and "save-data-popup-check" in measure,
        "measure must expose and penalize the failed-save-data popup semaphore",
        failures,
    )

    if failures:
        for failure in failures:
            print(f"autoload happy-path check failed: {failure}", file=sys.stderr)
        return 1
    print("autoload happy-path checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
