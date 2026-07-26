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


def test_parse_imported_dlls_flags_case_insensitively() -> None:
    checker = load_checker()
    output = """
The Import Tables:
    DLL Name: USER32.dll
    DLL Name: d3d12.dll
    DLL Name: DXGI.dll
    DLL Name: kernel32.dll
"""
    imports = checker.parse_imported_dlls(output)
    banned = imports & checker.BANNED_IMPORT_DLLS
    assert banned == {"d3d12.dll", "dxgi.dll"}


def test_source_scan_rejects_hard_linked_graphics_entrypoint() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        src.mkdir()
        (src / "bad.rs").write_text(
            "use windows::Win32::Graphics::Direct3D12::D3D12CreateDevice;\n"
            "fn bad() { unsafe { D3D12CreateDevice(None, todo!(), todo!()); } }\n",
            encoding="utf-8",
        )
        old_src_root = checker.SRC_ROOT
        try:
            checker.SRC_ROOT = src
            findings = checker.source_findings()
        finally:
            checker.SRC_ROOT = old_src_root
    assert findings
    assert any("D3D12CreateDevice" in finding.line for finding in findings)


def test_source_scan_rejects_top_level_native_bridge_window() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        src.mkdir()
        (src / "native_overlay.rs").write_text(
            "CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class_name, title, WS_POPUP, 0, 0, SM_CXSCREEN, SM_CYSCREEN, None, None, None, None);\n",
            encoding="utf-8",
        )
        old_src_root = checker.SRC_ROOT
        try:
            checker.SRC_ROOT = src
            findings = checker.source_findings()
        finally:
            checker.SRC_ROOT = old_src_root
    assert findings
    assert any("child/owned game-window" in finding.message for finding in findings)


def test_source_scan_allows_dynamic_wrapper_names() -> None:
    checker = load_checker()
    with tempfile.TemporaryDirectory() as tmp:
        src = Path(tmp) / "src"
        src.mkdir()
        (src / "ok.rs").write_text(
            "type D3D12CreateDeviceRaw = unsafe extern \"system\" fn();\n"
            "let Some(proc) = dynamic_graphics_proc(w!(\"d3d12.dll\"), s!(\"D3D12CreateDevice\"));\n",
            encoding="utf-8",
        )
        old_src_root = checker.SRC_ROOT
        try:
            checker.SRC_ROOT = src
            findings = checker.source_findings()
        finally:
            checker.SRC_ROOT = old_src_root
    assert not findings


def test_native_overlay_release_uses_semantic_render_ready() -> None:
    task_registration = (
        REPO_ROOT
        / "crates"
        / "er-effects-rs"
        / "src"
        / "lib_parts"
        / "dll_entry_parts"
        / "task_registration.rs"
    ).read_text(encoding="utf-8", errors="replace")
    release_block = task_registration.split("let native_overlay_release_ready =", 1)[1].split(
        "native_overlay_release_tick", 1
    )[0]
    assert "is_onscreen()" in release_block
    assert "is_render_group_enabled()" in release_block
    assert "enable_render()" in release_block
    assert "draw_group_enabled()" not in release_block

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
    assert "chr_onscreen" in oracle_block
    assert "chr_render_group_enabled" in oracle_block
    assert "chr_enable_render" in oracle_block
    assert "chr_draw_group_enabled" not in oracle_block


def main() -> int:
    tests = [
        test_parse_imported_dlls_flags_case_insensitively,
        test_source_scan_rejects_hard_linked_graphics_entrypoint,
        test_source_scan_rejects_top_level_native_bridge_window,
        test_source_scan_allows_dynamic_wrapper_names,
        test_native_overlay_release_uses_semantic_render_ready,
    ]
    for test in tests:
        test()
    print("test-windows-proof-render passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
