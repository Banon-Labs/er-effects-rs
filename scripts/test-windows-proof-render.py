#!/usr/bin/env python3
"""Regression tests for scripts/check-windows-proof-render.py."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
CHECKER_PATH = REPO_ROOT / "scripts" / "check-windows-proof-render.py"


def load_checker():
    spec = importlib.util.spec_from_file_location("check_windows_proof_render", CHECKER_PATH)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_parse_imported_dlls_flags_vulkan_case_insensitively() -> None:
    checker = load_checker()
    output = """
The Import Tables:
    DLL Name: USER32.dll
    DLL Name: d3d12.dll
    DLL Name: DXGI.dll
    DLL Name: VULKAN-1.dll
"""
    imports = checker.parse_imported_dlls(output)
    banned = imports & checker.BANNED_IMPORT_DLLS
    assert banned == {"vulkan-1.dll"}
    assert "d3d12.dll" not in checker.BANNED_IMPORT_DLLS
    assert "dxgi.dll" not in checker.BANNED_IMPORT_DLLS


def test_source_scan_rejects_vulkan_entrypoint_in_code() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        src.mkdir()
        (src / "bad.rs").write_text(
            "extern \"system\" { fn vkCreateInstance(); }\n",
            encoding="utf-8",
        )
        old_src_root = checker.SRC_ROOT
        try:
            checker.SRC_ROOT = src
            findings = checker.source_findings()
        finally:
            checker.SRC_ROOT = old_src_root
    assert findings
    assert any("Vulkan" in finding.message for finding in findings)


def test_source_scan_allows_native_d3d12_dxgi_code() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        src.mkdir()
        (src / "ok.rs").write_text(
            "use windows::Win32::Graphics::Direct3D12::D3D12CreateDevice;\n"
            "use windows::Win32::Graphics::Dxgi::CreateDXGIFactory2;\n",
            encoding="utf-8",
        )
        exp = src / "experiments"
        exp.mkdir()
        (exp / "native_overlay.rs").write_text(
            "//! Native-Windows loading-experience overlay: a SEPARATE top-level window with our OWN D3D12 device\n"
            "fn ok() { let _ = WS_EX_TOPMOST | WS_POPUP; D3D12CreateDevice(); CreateSwapChainForHwnd(); renderdoc_active(); }\n",
            encoding="utf-8",
        )
        old_src_root = checker.SRC_ROOT
        try:
            checker.SRC_ROOT = src
            findings = checker.source_findings()
        finally:
            checker.SRC_ROOT = old_src_root
    assert not findings


def test_source_scan_rejects_child_native_overlay_window() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        exp = src / "experiments"
        exp.mkdir(parents=True)
        (exp / "native_overlay.rs").write_text(
            "//! Native-Windows loading-experience overlay: a SEPARATE top-level window with our OWN D3D12 device\n"
            "fn bad() { let style = WS_CHILD; }\n"
            "fn ok() { let _ = WS_EX_TOPMOST | WS_POPUP; D3D12CreateDevice(); CreateSwapChainForHwnd(); renderdoc_active(); }\n",
            encoding="utf-8",
        )
        old_src_root = checker.SRC_ROOT
        try:
            checker.SRC_ROOT = src
            findings = checker.source_findings()
        finally:
            checker.SRC_ROOT = old_src_root
    assert findings
    assert any("must not be a child window" in finding.message for finding in findings)


def test_script_scan_rejects_proton_command_requirement() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        scripts = Path(tmp) / "scripts"
        scripts.mkdir()
        (scripts / "run-me3-product-smoke.sh").write_text("proton run eldenring.exe\n", encoding="utf-8")
        old_scripts_root = checker.SCRIPTS_ROOT
        try:
            checker.SCRIPTS_ROOT = scripts
            findings = checker.script_findings()
        finally:
            checker.SCRIPTS_ROOT = old_scripts_root
    assert findings
    assert any("script" in finding.message for finding in findings)


def test_native_overlay_is_top_level_isolated_d3d12_overlay() -> None:
    # Portrait crate split (2026-07-29): native_overlay.rs lives in er-loading-portrait.
    native_overlay = (
        REPO_ROOT / "crates" / "er-loading-portrait" / "src" / "native_overlay.rs"
    ).read_text(encoding="utf-8", errors="replace")
    assert "SEPARATE top-level window" in native_overlay
    assert "OWN D3D12 device" in native_overlay
    assert "WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW" in native_overlay
    assert "WS_POPUP" in native_overlay
    assert "D3D12CreateDevice" in native_overlay
    assert "CreateSwapChainForHwnd" in native_overlay
    assert "renderdoc_active()" in native_overlay
    assert "WS_CHILD" not in native_overlay
    assert "NATIVE_OVERLAY_SHOW" in native_overlay
    assert "NATIVE_OVERLAY_FRAMES" in native_overlay
    assert "NATIVE_OVERLAY_STAGE" in native_overlay


def test_movement_probe_and_oracle_use_semantic_render_ready() -> None:
    task_registration = (
        REPO_ROOT
        / "crates"
        / "er-effects-rs"
        / "src"
        / "lib_parts"
        / "dll_entry_parts"
        / "task_registration.rs"
    ).read_text(encoding="utf-8", errors="replace")
    render_block = task_registration.split("let char_rendered =", 1)[1].split(
        "if !sq_menu_nav && char_rendered",
        1,
    )[0]
    assert "is_render_group_enabled()" in render_block
    assert "enable_render()" in render_block
    assert "draw_group_enabled()" not in render_block

    write_oracle = (
        REPO_ROOT
        / "crates"
        / "er-effects-rs"
        / "src"
        / "telemetry"
        / "runtime_oracles"
        / "write_oracle.rs"
    ).read_text(encoding="utf-8", errors="replace")
    oracle_block = write_oracle.split("let player_render_ready =", 1)[1].split(
        "body.push_str", 1
    )[0]
    assert "chr_model_ins_ptr" in oracle_block
    assert "chr_render_group_enabled" in oracle_block
    assert "chr_enable_render" in oracle_block
    assert "chr_draw_group_enabled" not in oracle_block


def main() -> int:
    tests = [
        test_parse_imported_dlls_flags_vulkan_case_insensitively,
        test_source_scan_rejects_vulkan_entrypoint_in_code,
        test_source_scan_allows_native_d3d12_dxgi_code,
        test_source_scan_rejects_child_native_overlay_window,
        test_script_scan_rejects_proton_command_requirement,
        test_native_overlay_is_top_level_isolated_d3d12_overlay,
        test_movement_probe_and_oracle_use_semantic_render_ready,
    ]
    for test in tests:
        test()
    print("test-windows-proof-render passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
