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
        and "SEAMLESS_MODULE_MARKERS" in watcher,
        "readiness watcher must fail closed when Seamless/vanilla runtime mode mismatches the experiment precondition",
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
        and "visual_save_data_popup_detected" in watcher
        and "failed to load save data" in watcher,
        "readiness watcher must expose a visual semaphore for the failed-save-data popup",
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
        "MENU_CONTINUE_WRAPPER" in native_static_check
        and "MENU_WINDOW_JOB_CTOR" in native_static_check
        and "MENU_ACCEPT_IDLE" in native_static_check
        and "MENU_ACCEPT_NATIVE" in native_static_check
        and "MENU_SUBMIT" in native_static_check
        and "MENU_MEMBER_FUNC_JOB_RUN" in native_static_check
        and "MENU_REGISTRY_INSERT_COPY" in native_static_check
        and "node+0x18" in native_static_check
        and "node+0x20" in native_static_check
        and "node+0x10" in native_static_check
        and "check-native-continue-static.py" in check_sh,
        "quality gates must include skip-safe native Continue/MenuWindowJob/MenuMemberFuncJob static byte-window validation",
        failures,
    )
    require(
        "check-native-continue-static.py" in measure
        and "MenuMemberFuncJob" in measure,
        "measure must fail closed if the native Continue/MenuMemberFuncJob static checker is not wired into quality gates",
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
