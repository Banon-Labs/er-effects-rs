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
    "live_dialog_enabled",
    "native_fullread_commit_enabled",
    "menu_window_latch_enabled",
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


def enum_variant_value(source: str, enum_name: str, variant_name: str) -> int | None:
    enum_match = re.search(rf"enum\s+{re.escape(enum_name)}\s*\{{(.*?)\}}", source, re.S)
    if enum_match is None:
        return None
    variant_match = re.search(rf"\b{re.escape(variant_name)}\s*=\s*(\d+)\b", enum_match.group(1))
    if variant_match is None:
        return None
    return int(variant_match.group(1))


def live_dialog_settle_threshold_is_90(experiments: str, lib: str) -> bool:
    const_match = re.search(
        r"const\s+LIVE_DIALOG_ACTIVATE_SETTLE_WAITS:\s+u64\s+=\s*(.*?);",
        experiments,
        re.S,
    )
    if const_match is None:
        return False
    expr = " ".join(const_match.group(1).split())
    if expr == "90":
        return True
    if expr == "OwnStepperFrameBudget::Frames90 as u64":
        return enum_variant_value(lib, "OwnStepperFrameBudget", "Frames90") == 90
    return False


def main() -> int:
    failures: list[str] = []
    experiments = read(EXPERIMENTS)
    lib = read(LIB)
    stage = read(STAGE_SCRIPT)
    runtime_probe = read(RUNTIME_PROBE)
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

    arm_body = rust_fn_body(experiments, "arm_product_autoload_from_request")
    require("SaveLoadMethod::DirectMenuLoad" in arm_body, "product arm must be limited to direct_menu_load", failures)
    require("request.slot()" in arm_body, "product arm must require an explicit slot", failures)
    require("OWN_STEPPER_SLOT.store(slot" in arm_body, "product arm must propagate the requested slot", failures)
    require("PRODUCT_AUTOLOAD_ARMED.store" in arm_body, "product arm must latch PRODUCT_AUTOLOAD_ARMED", failures)
    require("append_autoload_debug" not in arm_body, "product arm must not perform early debug/file I/O", failures)

    for gate in sorted(REQUIRED_PRODUCT_GATES):
        body = rust_fn_body(experiments, gate)
        require("product_autoload_enabled()" in body, f"{gate} must be enabled by product_autoload_enabled()", failures)

    require(
        live_dialog_settle_threshold_is_90(experiments, lib),
        "product autoload live-dialog activation settle must stay at the proven 90-frame threshold",
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
        "scripts/check-refactor-equivalence.py" in measure,
        "measure must include the static refactor-equivalence oracle for this branch",
        failures,
    )
    require(
        "unproven_equivalence_total" in measure,
        "measure must expose unproven_equivalence_total as the primary proof metric",
        failures,
    )
    require(
        "autoload_static_failures" in measure and "autoload_stage_failures" in measure,
        "measure must count static product-gate and release-staging validation failures",
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
