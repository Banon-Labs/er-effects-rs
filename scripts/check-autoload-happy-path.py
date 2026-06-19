#!/usr/bin/env python3
"""Fail-closed checks for the supported zero-input autoload release path."""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
EXPERIMENTS = REPO_ROOT / "src" / "experiments.rs"
LIB = REPO_ROOT / "src" / "lib.rs"
STAGE_SCRIPT = REPO_ROOT / "scripts" / "stage-autoload-release.sh"
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
    runtime_probe = read(RUNTIME_PROBE) if RUNTIME_PROBE.exists() else ""
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
        "product autoload must define semantic readiness helpers for title boot, menu action, modals, and ProfileLoadDialog",
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
        "MSGBOX_LAST_DIALOG" in lib
        and "MSGBOX_POSTLOAD_BUILDS" in lib
        and "oracle_postload_modal_seen" in read(REPO_ROOT / "src" / "telemetry.rs")
        and "oracle_blocking_modal_present" in read(REPO_ROOT / "src" / "telemetry.rs"),
        "telemetry must expose post-load MessageBoxDialog/blocking-modal oracle evidence",
        failures,
    )
    require(
        "oracle_player_render_ready" in read(REPO_ROOT / "src" / "telemetry.rs")
        and "chr_flags1c5.enable_render" in read(REPO_ROOT / "src" / "telemetry.rs")
        and "load_state.draw_group_enabled" in read(REPO_ROOT / "src" / "telemetry.rs"),
        "telemetry must expose rendered-player readiness from ChrIns render state, not just save identity",
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
