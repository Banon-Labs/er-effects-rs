#!/usr/bin/env python3
"""Static regressions for the input-harness in-world menu drive path."""

from __future__ import annotations

from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def test_pad_inject_uses_both_cs_ingame_pad_typeids() -> None:
    src = (REPO_ROOT / "crates/er-input-harness-dll/src/pad_inject.rs").read_text()
    assert "const CS_INGAME_PAD_TYPEID_RVAS: [usize; 2] = [0x3d5df27, 0x3d5df28];" in src
    assert "let targets = CS_INGAME_PAD_TYPEID_RVAS.map(|rva| base + rva);" in src
    assert "for target in targets" in src


def test_pad_inject_direct_stamp_writes_are_enabled() -> None:
    src = (REPO_ROOT / "crates/er-input-harness-dll/src/pad_inject.rs").read_text()
    assert "BISect: write disabled" not in src
    assert "BISECT: write disabled" not in src
    assert "crate::win32::write_u8(cached + off, val)" in src
    assert "crate::win32::write_u8(pad + off, val)" in src


def test_input_harness_manifest_names_actual_hook_layer() -> None:
    manifest = (REPO_ROOT / "crates/er-input-harness-dll/Cargo.toml").read_text()
    assert "FD4PadDevice::poll" not in manifest
    assert "DLUID virtual-key builders/writer" in manifest
    assert "0x240e70/0x241130/0x26634a0" in manifest


def test_continue_and_boot_view_timing_oracles_exist() -> None:
    counters = (REPO_ROOT / "crates/er-telemetry/src/counters.rs").read_text()
    assert "pub static BOOT_VIEW_PUMP_STOP_MS" in counters
    assert "pub static OWN_LOAD_CONTINUE_FIRED_MS" in counters
    assert "pub static TFC_CONTINUE_FIRED_MS" in counters

    product_core = (REPO_ROOT / "crates/er-effects-rs/src/experiments/mod/product_core_own_stepper.rs").read_text()
    assert "pub(crate) fn mark_own_load_continue_fired()" in product_core
    assert "OWN_LOAD_CONTINUE_FIRED_MS.compare_exchange" in product_core
    assert "pub(crate) fn mark_tfc_continue_fired()" in product_core
    assert "TFC_CONTINUE_FIRED_MS.compare_exchange" in product_core

    present = (REPO_ROOT / "crates/er-effects-rs/src/experiments/present_overlay.rs").read_text()
    assert "fn set_boot_view_pump_stop_reason" in present
    assert "BOOT_VIEW_PUMP_STOP_MS.store" in present

    game_oracles = (REPO_ROOT / "crates/er-effects-rs/src/telemetry/runtime_oracles/write_game_module_oracles.rs").read_text()
    assert '"oracle_boot_view_pump_stop_ms"' in game_oracles
    telemetry = (REPO_ROOT / "crates/er-effects-rs/src/telemetry/runtime_oracles/write_telemetry.rs").read_text()
    assert 'oracle_own_load_continue_fired_ms' in telemetry
    assert 'oracle_tfc_continue_fired_ms' in telemetry


def main() -> int:
    tests = [
        test_pad_inject_uses_both_cs_ingame_pad_typeids,
        test_pad_inject_direct_stamp_writes_are_enabled,
        test_input_harness_manifest_names_actual_hook_layer,
        test_continue_and_boot_view_timing_oracles_exist,
    ]
    for test in tests:
        test()
    print("input harness static checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
