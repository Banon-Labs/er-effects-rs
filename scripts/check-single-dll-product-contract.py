#!/usr/bin/env python3
"""Enforce the shipped single-DLL product contract.

The customized System>Quit implementation lives in the `er-quit-menu-core` library, but the
required product remains one ME3 native: `er_quickload.dll`. The standalone
`er-quit-menu` package may exist as a test harness; it must never become a product
dependency, default workspace artifact, staged release DLL, or required profile native.

Usage:
    python3 scripts/check-single-dll-product-contract.py
    python3 scripts/check-single-dll-product-contract.py --selftest
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
WORKSPACE_MANIFEST = REPO_ROOT / "Cargo.toml"
PRODUCT_MANIFEST = REPO_ROOT / "crates" / "er-quickload" / "Cargo.toml"
QUIT_MENU_MANIFEST = REPO_ROOT / "crates" / "er-quit-menu-core" / "Cargo.toml"
STAGE_SCRIPT = REPO_ROOT / "scripts" / "stage-autoload-release.sh"

PRODUCT_PACKAGE = "er-quickload"
QUIT_MENU_PACKAGE = "er-quit-menu-core"
HARNESS_PACKAGE = "er-quit-menu"
PRODUCT_DLL = "er_quickload.dll"
PRODUCT_PROFILE = "er-quickload.me3"
# Crate types that emit a dynamic library. Exactly one of them may appear on the product, and it
# must be the `cdylib`; a `dylib` or `proc-macro` beside it would be a second shipped artifact.
DLL_CRATE_TYPES = ("cdylib", "dylib", "proc-macro")
# Crate types the linker consumes and never ships on their own.
LINK_ONLY_CRATE_TYPES = ("rlib", "lib")


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as stream:
        return tomllib.load(stream)


def dependency_tables(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    """Return every dependency table, including target-specific tables."""
    tables: list[dict[str, Any]] = []
    for key in ("dependencies", "dev-dependencies", "build-dependencies"):
        table = manifest.get(key)
        if isinstance(table, dict):
            tables.append(table)
    targets = manifest.get("target", {})
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for key in ("dependencies", "dev-dependencies", "build-dependencies"):
                table = target.get(key)
                if isinstance(table, dict):
                    tables.append(table)
    return tables


def package_for_dependency(name: str, value: Any) -> str:
    if isinstance(value, dict):
        package = value.get("package")
        if isinstance(package, str):
            return package
    return name


def check_cargo_contract(
    workspace_manifest_path: Path,
    product_manifest_path: Path,
    quit_menu_manifest_path: Path,
) -> list[str]:
    failures: list[str] = []
    workspace = load_toml(workspace_manifest_path)
    product = load_toml(product_manifest_path)
    quit_menu = load_toml(quit_menu_manifest_path)

    default_members = workspace.get("workspace", {}).get("default-members", [])
    workspace_root = workspace_manifest_path.parent
    expected_member = str(product_manifest_path.parent.relative_to(workspace_root))
    harness_member = str(Path("crates") / HARNESS_PACKAGE)
    if expected_member not in default_members:
        failures.append("workspace default-members must include crates/er-quickload")
    if harness_member in default_members:
        failures.append(
            "workspace default-members must exclude er-quit-menu; the harness requires "
            "an explicit build"
        )

    product_crate_types = product.get("lib", {}).get("crate-type", [])
    # The contract is one shipped DLL, not one crate-type. `cdylib` is the type that produces a
    # DLL, so exactly one of it must be declared and no second DLL-producing type beside it.
    # `rlib`/`lib` are link-time archives, never a shipped artifact, so they are permitted:
    # er-quickload declares an `rlib` so a feature-selected subset of the crate can be linked
    # into a smaller shell without that shell becoming a second product DLL.
    dll_types = [name for name in product_crate_types if name in DLL_CRATE_TYPES]
    if dll_types != ["cdylib"]:
        failures.append("er-quickload must emit exactly one cdylib product artifact")
    unknown_types = [
        name
        for name in product_crate_types
        if name not in DLL_CRATE_TYPES and name not in LINK_ONLY_CRATE_TYPES
    ]
    if unknown_types:
        failures.append(
            "er-quickload declares crate-type "
            f"{', '.join(sorted(set(unknown_types)))}, which this contract does not recognize "
            "as either the one DLL or a link-only archive"
        )

    product_dependencies = dependency_tables(product)
    quit_entries: list[Any] = []
    for table in product_dependencies:
        for name, value in table.items():
            package = package_for_dependency(name, value)
            if package == HARNESS_PACKAGE:
                failures.append(
                    "er-quickload must not depend on er-quit-menu; the DLL is harness-only"
                )
            if package == QUIT_MENU_PACKAGE:
                quit_entries.append(value)

    if len(quit_entries) != 1:
        failures.append(
            "er-quickload must have exactly one direct er-quit-menu-core dependency so quit behavior "
            "is linked into er_quickload.dll"
        )
    else:
        dependency = quit_entries[0]
        if not isinstance(dependency, dict):
            failures.append("the er-quit-menu-core dependency must be a local path dependency")
        else:
            dependency_path = dependency.get("path")
            if not isinstance(dependency_path, str):
                failures.append("the er-quit-menu-core dependency must name its local path")
            elif (product_manifest_path.parent / dependency_path).resolve() != quit_menu_manifest_path.parent.resolve():
                failures.append("the er-quit-menu-core dependency path does not resolve to crates/er-quit-menu-core")
            if dependency.get("optional") is True:
                failures.append("er-quit-menu-core must not be optional in the product DLL")

    quit_crate_types = quit_menu.get("lib", {}).get("crate-type", ["rlib"])
    if "cdylib" in quit_crate_types:
        failures.append(
            "er-quit-menu-core must remain a library linked into the product, not emit a required DLL"
        )

    return failures


def check_staged_product(stage_dir: Path) -> list[str]:
    failures: list[str] = []
    dlls = sorted(path.name for path in stage_dir.glob("*.dll"))
    if dlls != [PRODUCT_DLL]:
        failures.append(
            f"staged product must contain only {PRODUCT_DLL}; found {dlls or 'no DLLs'}"
        )

    profiles = sorted(path.name for path in stage_dir.glob("*.me3"))
    if profiles != [PRODUCT_PROFILE]:
        failures.append(
            f"staged product must contain exactly {PRODUCT_PROFILE}; found {profiles or 'no profiles'}"
        )
        return failures

    try:
        profile = load_toml(stage_dir / PRODUCT_PROFILE)
    except (OSError, tomllib.TOMLDecodeError) as error:
        failures.append(f"could not parse {PRODUCT_PROFILE}: {error}")
        return failures

    natives = profile.get("natives")
    if natives != [{"path": PRODUCT_DLL}]:
        failures.append(
            f"{PRODUCT_PROFILE} must have exactly one [[natives]] entry for {PRODUCT_DLL}; "
            f"found {natives!r}"
        )
    return failures


def stage_live_product(stage_dir: Path) -> None:
    dummy_dll = stage_dir.parent / "source-er_quickload.dll"
    dummy_dll.write_bytes(b"single-dll-contract-test\n")
    env = os.environ.copy()
    env["ER_QUICKLOAD_DLL"] = str(dummy_dll)
    subprocess.run(
        ["bash", str(STAGE_SCRIPT), "--no-build", "--output", str(stage_dir)],
        cwd=REPO_ROOT,
        env=env,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=30,
    )


def write_manifest(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def selftest() -> int:
    failures = 0
    ran = 0

    def case(name: str, condition: bool) -> None:
        nonlocal failures, ran
        ran += 1
        if not condition:
            print(f"selftest FAIL: {name}", file=sys.stderr)
            failures += 1

    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        workspace = root / "Cargo.toml"
        product = root / "crates" / "er-quickload" / "Cargo.toml"
        quit_menu = root / "crates" / "er-quit-menu-core" / "Cargo.toml"
        stage = root / "stage"
        stage.mkdir()

        write_manifest(
            workspace,
            '[workspace]\nmembers = ["crates/er-quickload", "crates/er-quit-menu-core", '
            '"crates/er-quit-menu"]\ndefault-members = ["crates/er-quickload"]\n',
        )
        write_manifest(
            product,
            '[package]\nname = "er-quickload"\n[lib]\ncrate-type = ["cdylib"]\n'
            "[target.'cfg(windows)'.dependencies]\n"
            'er-quit-menu-core = { path = "../er-quit-menu-core" }\n',
        )
        write_manifest(
            quit_menu,
            '[package]\nname = "er-quit-menu-core"\n[lib]\nname = "er_quit_menu_core"\n',
        )
        (stage / PRODUCT_DLL).write_bytes(b"dll")
        write_manifest(
            stage / PRODUCT_PROFILE,
            'profileVersion = "v1"\n[[supports]]\ngame = "eldenring"\n'
            f"[[natives]]\npath = '{PRODUCT_DLL}'\n",
        )

        case(
            "valid linked-library and one-native profile passes",
            not check_cargo_contract(workspace, product, quit_menu)
            and not check_staged_product(stage),
        )

        original_workspace = workspace.read_text(encoding="utf-8")
        write_manifest(
            workspace,
            original_workspace.replace(
                'default-members = ["crates/er-quickload"]',
                'default-members = ["crates/er-quickload", "crates/er-quit-menu"]',
            ),
        )
        problems = check_cargo_contract(workspace, product, quit_menu)
        case(
            "default harness build fails",
            any("harness requires an explicit build" in problem for problem in problems),
        )
        write_manifest(workspace, original_workspace)

        original_product = product.read_text(encoding="utf-8")
        # The `rlib` the feature-selected subset link needs is not a shipped artifact, so the
        # contract must accept it beside the one `cdylib`.
        write_manifest(
            product,
            original_product.replace(
                'crate-type = ["cdylib"]', 'crate-type = ["cdylib", "rlib"]'
            ),
        )
        case(
            "an rlib beside the one cdylib passes",
            not check_cargo_contract(workspace, product, quit_menu),
        )
        # A second dynamic library is what the contract exists to refuse, whichever type spells it.
        for second in ('"dylib"', '"cdylib"'):
            write_manifest(
                product,
                original_product.replace(
                    'crate-type = ["cdylib"]', f'crate-type = ["cdylib", {second}]'
                ),
            )
            problems = check_cargo_contract(workspace, product, quit_menu)
            case(
                f"a second {second.strip(chr(34))} artifact fails",
                any("exactly one cdylib" in problem for problem in problems),
            )
        write_manifest(product, original_product)

        write_manifest(product, original_product.replace("er-quit-menu-core", "er-quit-menu"))
        problems = check_cargo_contract(workspace, product, quit_menu)
        case(
            "harness dependency fails",
            any("harness-only" in problem for problem in problems),
        )
        write_manifest(product, original_product.replace(" }", ", optional = true }").replace("er-quit-menu-core\"", "er-quit-menu-core\""))
        problems = check_cargo_contract(workspace, product, quit_menu)
        case(
            "optional product library fails",
            any("must not be optional" in problem for problem in problems),
        )
        write_manifest(product, original_product)

        write_manifest(quit_menu, quit_menu.read_text(encoding="utf-8") + 'crate-type = ["cdylib"]\n')
        problems = check_cargo_contract(workspace, product, quit_menu)
        case(
            "quit-menu cdylib fails",
            any("must remain a library" in problem for problem in problems),
        )
        write_manifest(
            quit_menu,
            '[package]\nname = "er-quit-menu-core"\n[lib]\nname = "er_quit_menu_core"\n',
        )

        (stage / "er_quit_menu.dll").write_bytes(b"harness")
        problems = check_staged_product(stage)
        case(
            "extra staged harness DLL fails",
            any("must contain only" in problem for problem in problems),
        )
        (stage / "er_quit_menu.dll").unlink()

        with (stage / PRODUCT_PROFILE).open("a", encoding="utf-8") as stream:
            stream.write("[[natives]]\npath = 'er_quit_menu.dll'\n")
        problems = check_staged_product(stage)
        case(
            "second required profile native fails",
            any("exactly one [[natives]]" in problem for problem in problems),
        )

    if failures:
        print(f"selftest: {failures} case(s) failed", file=sys.stderr)
        return 1
    print(f"[check-single-dll-product-contract] selftest ok ({ran} cases)")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()
    if args.selftest:
        return selftest()

    failures = check_cargo_contract(
        WORKSPACE_MANIFEST,
        PRODUCT_MANIFEST,
        QUIT_MENU_MANIFEST,
    )
    with tempfile.TemporaryDirectory() as tmp:
        stage_dir = Path(tmp) / "autoload-release"
        try:
            stage_live_product(stage_dir)
        except subprocess.CalledProcessError as error:
            failures.append(
                "stage-autoload-release.sh failed during contract scan: "
                f"{error.stderr.strip() or error.stdout.strip() or error}"
            )
        else:
            failures.extend(check_staged_product(stage_dir))

    if failures:
        print("[check-single-dll-product-contract] FAIL:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1

    print(
        "[check-single-dll-product-contract] ok -- er-quit-menu-core is linked into "
        "er_quickload.dll; staged product has one DLL and one native entry"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
