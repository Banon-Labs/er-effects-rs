#!/usr/bin/env python3
"""Fail if the Windows-proof product build can rely on Proton/vkd3d-only rendering.

This check has two layers:
  1. source scan: catch reintroduced hard-linked graphics entrypoints in Rust source;
  2. PE import audit: catch release DLLs that import D3D12/DXGI/compiler DLLs.

The point is not that Win32 is bad. The point is that product proof from Linux/Proton
must not depend on Proton's tolerance for game-shared D3D12/swapchain rendering.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = REPO_ROOT / "crates" / "er-effects-rs" / "src"
DEFAULT_DLL = REPO_ROOT / "target" / "x86_64-pc-windows-msvc" / "release" / "er_effects_rs.dll"

BANNED_IMPORT_DLLS = {
    "d3d12.dll",
    "dxgi.dll",
    "d3dcompiler_47.dll",
}

# These windows-rs functions generate hard PE imports via windows_core::link!.
# Product/proof code must dynamic-load them through GetProcAddress so strict
# import-table auditing can prove the release DLL is not structurally tied to a
# Proton/vkd3d rendering path. Keeping the names in dynamic wrapper type aliases
# or GetProcAddress strings is allowed.
BANNED_HARD_LINK_SYMBOLS = {
    "CreateDXGIFactory2",
    "D3D12CreateDevice",
    "D3D12SerializeRootSignature",
    "D3DCompile",
}

ALLOWED_SOURCE_LINE_SNIPPETS = (
    "Raw = unsafe extern",
    "s!(\"",
    "w!(\"",
    "GetProcAddress",
    "failed",
    "format_args!",
    "append_autoload_debug",
    "type ",
)


@dataclass(frozen=True)
class Finding:
    path: Path
    line_number: int
    message: str
    line: str

    def format(self) -> str:
        rel = self.path.relative_to(REPO_ROOT)
        return f"{rel}:{self.line_number}: {self.message}: {self.line}"


def rust_files() -> list[Path]:
    return sorted(SRC_ROOT.rglob("*.rs"))


def source_findings() -> list[Finding]:
    findings: list[Finding] = []
    # A direct function import commonly appears in a `use ...::{ D3D12CreateDevice, ... }`
    # line; a direct call appears as `D3D12CreateDevice(...)`. We allow dynamic-wrapper
    # metadata lines but reject normal source references.
    symbol_pattern = re.compile(r"\b(" + "|".join(map(re.escape, BANNED_HARD_LINK_SYMBOLS)) + r")\b")
    for path in rust_files():
        for line_number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
            if not symbol_pattern.search(line):
                continue
            stripped = line.strip()
            if stripped.startswith("//") or stripped.startswith("///") or stripped.startswith("//!"):
                continue
            if '"' in line:
                continue
            if any(snippet in line for snippet in ALLOWED_SOURCE_LINE_SNIPPETS):
                continue
            if re.search(r"\b(?:let|const)\s+[A-Za-z0-9_]+\s*=\s*", line):
                # Local variable names and prose constants are not hard imports by themselves.
                continue
            findings.append(
                Finding(
                    path,
                    line_number,
                    "Windows-proof render must not hard-link D3D12/DXGI/compiler entrypoints; dynamic-load or move behind a dev-only artifact",
                    line.strip(),
                )
            )
    return findings


def objdump_tool() -> str | None:
    for name in ("llvm-objdump", "objdump", "x86_64-w64-mingw32-objdump"):
        tool = shutil.which(name)
        if tool:
            return tool
    return None


def parse_imported_dlls(objdump_output: str) -> set[str]:
    imports: set[str] = set()
    for line in objdump_output.splitlines():
        stripped = line.strip()
        if stripped.lower().startswith("dll name:"):
            imports.add(stripped.split(":", 1)[1].strip().lower())
    return imports


def imported_dlls(dll: Path) -> set[str]:
    tool = objdump_tool()
    if tool is None:
        raise RuntimeError("missing objdump tool for PE import audit")
    result = subprocess.run(
        [tool, "-p", str(dll)],
        cwd=REPO_ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        timeout=30,
    )
    return parse_imported_dlls(result.stdout)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--dll",
        type=Path,
        default=None,
        help="PE DLL to audit. When omitted, only the source scan runs.",
    )
    parser.add_argument(
        "--require-dll",
        action="store_true",
        help="fail if --dll (or the default release DLL) does not exist",
    )
    args = parser.parse_args()

    failures: list[str] = []
    findings = source_findings()
    if findings:
        failures.append("Hard-linked Proton/vkd3d render entrypoints found in source:")
        failures.extend(finding.format() for finding in findings)

    dll = args.dll
    if dll is None and args.require_dll:
        dll = DEFAULT_DLL
    if dll is not None:
        dll = dll if dll.is_absolute() else (REPO_ROOT / dll)
        if not dll.exists():
            if args.require_dll:
                failures.append(f"missing DLL for Windows-proof import audit: {dll}")
        else:
            imports = imported_dlls(dll)
            banned = sorted(imports & BANNED_IMPORT_DLLS)
            if banned:
                failures.append(
                    "Windows-proof release DLL imports Proton/vkd3d-only render DLLs: "
                    + ", ".join(banned)
                )
            else:
                print(f"windows-proof import audit passed: {dll}")

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("windows-proof source audit passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
