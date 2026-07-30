#!/usr/bin/env python3
"""Fail if the Windows product render path requires Vulkan, VKD3D, DXVK, Proton, or Wine.

The product target is native Windows D3D12. D3D12/DXGI imports are allowed; Vulkan/Proton
requirements are not. The check has three layers:
  1. Rust source scan for Vulkan/VKD3D/DXVK/Proton requirements in executable code;
  2. script scan for launch/proof commands that require Proton/Vulkan tooling;
  3. optional PE import audit rejecting vulkan-1.dll imports in release DLLs.
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
# The er-loading-portrait feature crate is product render code linked into
# er_effects_rs.dll (portrait crate split, 2026-07-29); it hosts native_overlay.rs and is
# scanned with the same Vulkan/Proton bans. Only scanned when SRC_ROOT is the real product
# tree, so the fixture-based regression tests stay hermetic.
PRODUCT_SRC_ROOT = SRC_ROOT
PORTRAIT_SRC_ROOT = REPO_ROOT / "crates" / "er-loading-portrait" / "src"
SCRIPTS_ROOT = REPO_ROOT / "scripts"
DEFAULT_DLL = REPO_ROOT / "target" / "x86_64-pc-windows-msvc" / "release" / "er_effects_rs.dll"

BANNED_IMPORT_DLLS = {"vulkan-1.dll"}

BANNED_RUST_PATTERNS = (
    re.compile(r"\bvk(?:Create|Get|Enumerate|Destroy|Queue|Cmd|Allocate|Free|Begin|End)[A-Z][A-Za-z0-9_]*\b"),
    re.compile(r"\bVk(?:Instance|Device|PhysicalDevice|Queue|Result|AllocationCallbacks|Surface)[A-Za-z0-9_]*\b"),
    re.compile(r"vulkan-1\.dll", re.IGNORECASE),
)

BANNED_SCRIPT_PATTERNS = (
    re.compile(r"\b(?:proton|wine|wine64|vkd3d|dxvk)\b", re.IGNORECASE),
    re.compile(r"vulkan-1\.dll", re.IGNORECASE),
)

# Comments may document why Proton/VKD3D is not sufficient. Executable code and launch commands may not
# require it.
COMMENT_PREFIXES = ("//", "///", "//!", "#")

SCAN_SCRIPTS = (
    "run-me3-product-smoke.sh",
    "run-windows-proof-render-smoke.sh",
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


def is_comment_or_blank(line: str) -> bool:
    stripped = line.strip()
    return not stripped or stripped.startswith(COMMENT_PREFIXES)


def rust_banned_pattern(line: str) -> str | None:
    for pattern in BANNED_RUST_PATTERNS:
        if pattern.search(line):
            return pattern.pattern
    return None


def script_banned_pattern(line: str) -> str | None:
    for pattern in BANNED_SCRIPT_PATTERNS:
        if pattern.search(line):
            return pattern.pattern
    return None


def rust_files() -> list[Path]:
    paths = list(SRC_ROOT.rglob("*.rs"))
    if SRC_ROOT == PRODUCT_SRC_ROOT and PORTRAIT_SRC_ROOT.exists():
        paths.extend(PORTRAIT_SRC_ROOT.rglob("*.rs"))
    return sorted(paths)


def source_findings() -> list[Finding]:
    findings: list[Finding] = []
    for path in rust_files():
        for line_number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
            if is_comment_or_blank(line):
                continue
            matched = rust_banned_pattern(line)
            if matched:
                findings.append(
                    Finding(
                        path,
                        line_number,
                        "Windows product render path must not require Vulkan/VKD3D/DXVK/Proton/Wine",
                        line.strip(),
                    )
                )
    findings.extend(native_overlay_shape_findings())
    return findings


def script_findings() -> list[Finding]:
    findings: list[Finding] = []
    for name in SCAN_SCRIPTS:
        path = SCRIPTS_ROOT / name
        if not path.exists():
            continue
        for line_number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
            if is_comment_or_blank(line):
                continue
            matched = script_banned_pattern(line)
            if matched:
                findings.append(
                    Finding(
                        path,
                        line_number,
                        "product/proof script must not require Vulkan/VKD3D/DXVK/Proton/Wine",
                        line.strip(),
                    )
                )
    return findings


def native_overlay_shape_findings() -> list[Finding]:
    # Real tree: native_overlay.rs lives in the er-loading-portrait crate (portrait crate
    # split). The SRC_ROOT-relative spelling is kept first so the monkeypatching
    # regression-test fixtures (which predate the split) still exercise the shape rules.
    path = SRC_ROOT / "experiments" / "native_overlay.rs"
    if not path.exists():
        path = PORTRAIT_SRC_ROOT / "native_overlay.rs"
    if not path.exists():
        return [Finding(path, 0, "native Windows overlay implementation is missing", "")]
    text = path.read_text(encoding="utf-8", errors="replace")
    required = {
        "SEPARATE top-level window": "native overlay must be a separate top-level window, not a Proton-tolerated game-device composite",
        "OWN D3D12 device": "native overlay must own its D3D12 device",
        "WS_EX_TOPMOST": "native overlay must cover the game on native Windows",
        "WS_POPUP": "native overlay must be top-level/borderless",
        "D3D12CreateDevice": "native overlay must render through native Windows D3D12",
        "CreateSwapChainForHwnd": "native overlay must own its swapchain",
        "renderdoc_active()": "native overlay must skip RenderDoc hook runs instead of depending on Proton/VKD3D tolerance",
    }
    findings: list[Finding] = []
    for token, message in required.items():
        if token not in text:
            findings.append(Finding(path, 0, message, f"missing token: {token}"))
    for line_number, line in enumerate(text.splitlines(), 1):
        if is_comment_or_blank(line):
            continue
        if "WS_CHILD" in line:
            findings.append(
                Finding(
                    path,
                    line_number,
                    "native Windows overlay must not be a child window tied to game rendering lifetime",
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
        help="PE DLL to audit. When omitted, only source/script scans run.",
    )
    parser.add_argument(
        "--require-dll",
        action="store_true",
        help="fail if --dll (or the default release DLL) does not exist",
    )
    args = parser.parse_args()

    failures: list[str] = []
    findings = source_findings() + script_findings()
    if findings:
        failures.append("Vulkan/Proton-dependent render requirements found:")
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
                failures.append("Windows product DLL imports Vulkan: " + ", ".join(banned))
            else:
                print(f"windows-proof import audit passed: {dll}")

    if failures:
        print("\n".join(failures), file=sys.stderr)
        return 1
    print("windows-proof render audit passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
